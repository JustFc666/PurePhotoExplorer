/**
 * PVA1 动图块的**测试夹具**。
 *
 * 生产代码里没人 import 它（只被 `*.test.ts` 引用），所以不会进打包产物。
 * 抽成独立模块是为了让「协议解析测试」和「渐进加载测试」共用同一份构造器 ——
 * 两边各写一份的话，改了布局只会改到一处，另一处就悄悄测错了。
 */

import { ANIM_HEADER_FIXED, ANIM_VERSION } from './protocol'

/**
 * 按 PVA1 v2 布局造一个动图块（镜像 Rust 侧 `pack_animation`）。
 *
 * `start`/`total` 是分块传输用的：造「第二块」时把它们设成
 * 「接在原动画第 N 帧、整段共 M 帧」。
 */
export function buildAnim(
  width: number,
  height: number,
  delays: number[],
  opts: {
    loop?: number
    truncated?: boolean
    version?: number
    start?: number
    total?: number
    /** 每帧第 0 字节写什么（默认写「全局帧号」= start + i） */
    markerBase?: number
  } = {},
): { buf: ArrayBuffer; stride: number } {
  const {
    loop = -1,
    truncated = false,
    version = ANIM_VERSION,
    start = 0,
    total = delays.length,
    markerBase = start,
  } = opts
  const count = delays.length
  const stride = width * height * 4
  const total_bytes = ANIM_HEADER_FIXED + count * 4 + count * stride
  const ab = new ArrayBuffer(total_bytes)
  const dv = new DataView(ab)

  dv.setUint8(0, 0x50) // 'P'
  dv.setUint8(1, 0x56) // 'V'
  dv.setUint8(2, 0x41) // 'A'
  dv.setUint8(3, 0x31) // '1'
  dv.setUint16(4, version, true)
  dv.setUint16(6, truncated ? 1 : 0, true)
  dv.setUint32(8, width, true)
  dv.setUint32(12, height, true)
  dv.setInt32(16, loop, true)
  dv.setUint32(20, count, true)
  dv.setUint32(24, start, true)
  dv.setUint32(28, total, true)
  for (let i = 0; i < count; i++) dv.setUint32(ANIM_HEADER_FIXED + i * 4, delays[i]!, true)
  // 每帧第 0 字节写帧序号，便于校验「没有串位」
  // （stride 为 0 时不写 —— 那是「尺寸非法」的构造用例，块里本就没有像素区）
  if (stride > 0) {
    const pixelStart = ANIM_HEADER_FIXED + count * 4
    for (let i = 0; i < count; i++) dv.setUint8(pixelStart + i * stride, markerBase + i)
  }
  return { buf: ab, stride }
}
