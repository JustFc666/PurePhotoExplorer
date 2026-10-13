/**
 * 像素传输协议的**前端解析层**（PVI1 静图 / PVA1 动图）。
 *
 * 后端把像素打包成紧凑二进制（见 Rust `commands/pixel.rs`），
 * 经 `tauri::ipc::Response` **以裸字节**回传，前端 `toBytes()` 直接拿到
 * `ArrayBuffer` —— 不再有 base64，也没有 JSON 解析。这里负责：
 *   ArrayBuffer/TypedArray/number[]/base64 字符串 → Uint8Array
 *   → 校验包头 → 切出像素视图。
 *
 * **性能与正确性约束**：
 * 1. 绝不用展开运算符（`...bigArray`）——大图会有上千万个元素，参数个数撑爆调用栈。
 * 2. 返回的像素是**零拷贝视图**，直接指向解码后的 buffer。
 * 3. `base64ToBytes` 仅作**兜底**保留：若哪天 CSP 又漏了
 *    `connect-src ipc: http://ipc.localhost`，Tauri 会退化成旧通道
 *    （字节被编码成 base64 字符串 / 数字数组）。届时功能仍然正常，
 *    只是明显变慢。见 `tauri.conf.json` 里那条 CSP 的注释。
 */

import type { DecodedAnimation, DecodedAnimationFrame, DecodedImage } from './types'

/** 像素包头：magic(4) + width(4) + height(4) + flags(1) = 13 字节 */
export const PIXEL_HEADER_LEN = 13
export const PIXEL_MAGIC = 'PVI1'

/** 动图包头固定部分：magic(4)+ver(2)+flags(2)+w(4)+h(4)+loop(4)+count(4)+start(4)+total(4) */
export const ANIM_HEADER_FIXED = 32
export const ANIM_MAGIC = 'PVA1'
/** 协议版本（v2 起头部带 start/total，支持分块传输） */
export const ANIM_VERSION = 2
/** flags 位 0：被上限截断 */
const ANIM_FLAG_TRUNCATED = 1

/** base64 字符串 → 字节数组（逐元素赋值，不用 spread）
 *
 * ⚠️ **兜底路径**：正常情况后端返回裸字节，走不到这里（见文件头注释 3）。 */
export function base64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64)
  const out = new Uint8Array(bin.length)
  for (let i = 0; i < bin.length; i++) {
    out[i] = bin.charCodeAt(i)
  }
  return out
}

/**
 * 把后端返回值统一成 `Uint8Array`。
 *
 * 正常路径是 `ArrayBuffer`（后端返回裸字节 + 自定义协议 `fetch` 的
 * `response.arrayBuffer()`），**零拷贝**。其余分支是兼容兜底：
 * TypedArray / base64 字符串 / number[]。
 */
export function toBytes(raw: unknown): Uint8Array {
  if (typeof raw === 'string') return base64ToBytes(raw)
  if (raw instanceof ArrayBuffer) return new Uint8Array(raw)
  if (ArrayBuffer.isView(raw)) {
    return new Uint8Array(raw.buffer, raw.byteOffset, raw.byteLength)
  }
  if (Array.isArray(raw)) {
    const out = new Uint8Array(raw.length)
    for (let i = 0; i < raw.length; i++) {
      out[i] = raw[i] as number
    }
    return out
  }
  throw new Error(
    `decode_image 返回了意外类型：${Object.prototype.toString.call(raw)}`,
  )
}

/**
 * 解析 PVI1 包为可直接 `putImageData` 的像素数据。
 *
 * @throws 包头不匹配 / 长度不符时抛出可读错误
 */
export function parsePixels(u8: Uint8Array): DecodedImage {
  if (u8.byteLength < PIXEL_HEADER_LEN) {
    throw new Error(`像素数据长度不足：仅收到 ${u8.byteLength} 字节`)
  }

  // 校验包头（逐字节比较，不用展开运算符）
  const magic = String.fromCharCode(u8[0]!, u8[1]!, u8[2]!, u8[3]!)
  if (magic !== PIXEL_MAGIC) {
    throw new Error(`像素数据格式不正确：期望 ${PIXEL_MAGIC}，实际 ${magic}`)
  }

  const header = new DataView(u8.buffer, u8.byteOffset, u8.byteLength)
  const width = header.getUint32(4, true)
  const height = header.getUint32(8, true)

  const expected = width * height * 4
  const pixelLen = u8.byteLength - PIXEL_HEADER_LEN
  if (pixelLen !== expected) {
    throw new Error(
      `像素长度不符：声明 ${width}×${height}（需 ${expected} 字节），实际收到 ${pixelLen} 字节`,
    )
  }

  // 零拷贝视图：直接指向 u8 底层 buffer 的像素区
  // （后端现在返回裸字节 ArrayBuffer；即便走 base64 兜底，底层也是普通 ArrayBuffer）
  const data = new Uint8ClampedArray(
    u8.buffer as ArrayBuffer,
    u8.byteOffset + PIXEL_HEADER_LEN,
    expected,
  )
  return { width, height, data }
}

/**
 * 解析 PVA1 动图**数据块**为可直接播放的帧序列。
 *
 * 布局（小端，与 Rust `commands/pixel.rs::pack_animation` 一一对应）：
 * ```text
 * magic(4) ver(2) flags(2) width(4) height(4) loop(4) count(4) start(4) total(4)  ← 固定 32 字节
 * delay[count] × u32                                                             ← 每帧毫秒
 * rgba[count]  × width*height*4                                                  ← 逐帧首尾相连
 * ```
 * 帧长固定（每帧都是整张逻辑屏幕），所以第 i 帧 = `pixelStart + i*stride`，
 * **只切视图不复制** —— 299 帧 71MB 也不会有额外内存与拷贝开销。
 *
 * `start`/`total` 是**分块传输**用的：大动图会被拆成多块，调用方靠它们
 * 判断「这块接在哪儿、还差多少」（见 `image.ts::decodeAnimationProgressive`）。
 *
 * @throws 魔数/版本不符、或声明的帧数×尺寸超出实际长度时抛出可读错误
 */
export function parseAnimation(u8: Uint8Array): DecodedAnimation {
  if (u8.byteLength < ANIM_HEADER_FIXED) {
    throw new Error(`动图数据长度不足：仅收到 ${u8.byteLength} 字节`)
  }

  const magic = String.fromCharCode(u8[0]!, u8[1]!, u8[2]!, u8[3]!)
  if (magic !== ANIM_MAGIC) {
    throw new Error(`动图数据格式不正确：期望 ${ANIM_MAGIC}，实际 ${magic}`)
  }

  const dv = new DataView(u8.buffer as ArrayBuffer, u8.byteOffset, u8.byteLength)
  const version = dv.getUint16(4, true)
  if (version !== ANIM_VERSION) {
    throw new Error(`动图协议版本不支持：本程序支持 v${ANIM_VERSION}，实际 v${version}`)
  }

  const flags = dv.getUint16(6, true)
  const width = dv.getUint32(8, true)
  const height = dv.getUint32(12, true)
  const loop = dv.getInt32(16, true)
  const count = dv.getUint32(20, true)
  const startIndex = dv.getUint32(24, true)
  const totalCount = dv.getUint32(28, true)

  if (width === 0 || height === 0) {
    throw new Error(`动图尺寸非法：${width}×${height}`)
  }

  const stride = width * height * 4
  const tableStart = ANIM_HEADER_FIXED
  const pixelStart = tableStart + count * 4
  const need = pixelStart + count * stride
  if (u8.byteLength < need) {
    throw new Error(
      `动图数据长度不符：声明 ${count} 帧 ${width}×${height}（需 ${need} 字节），实际 ${u8.byteLength} 字节`,
    )
  }

  const frames: DecodedAnimationFrame[] = []
  for (let i = 0; i < count; i++) {
    const off = u8.byteOffset + pixelStart + i * stride
    frames.push({
      delayMs: dv.getUint32(tableStart + i * 4, true),
      // 零拷贝：同一块内的所有帧共享同一个 ArrayBuffer
      image: { width, height, data: new Uint8ClampedArray(u8.buffer as ArrayBuffer, off, stride) },
    })
  }

  return {
    width,
    height,
    count,
    truncated: (flags & ANIM_FLAG_TRUNCATED) !== 0,
    // 协议用 -1 表示无限循环（Rust 侧 `Option<u16>` 的 None）
    loopCount: loop < 0 ? null : loop,
    startIndex,
    totalCount,
    frames,
  }
}
