/**
 * 图像相关的后端命令封装（含解码结果缓存）。
 *
 * **为什么要缓存**：解码一张大图要几百毫秒。翻页时若每次都重新解码，
 * 体验会明显卡顿。这里按 `path + purpose` 缓存解码结果，
 * 配合 `prefetchImage` 预取相邻图片，使「上一张/下一张」几乎瞬时。
 */

import { invoke } from '@tauri-apps/api/core'
import { traceLog } from './app'
import { parseAnimation, parsePixels, toBytes } from './protocol'
import type { DecodePurpose, DecodedAnimation, DecodedImage, ImageInfo } from './types'

/** 探测图像信息（不解码像素，< 5ms） */
export async function probeImage(path: string): Promise<ImageInfo> {
  return invoke<ImageInfo>('probe_image', { path })
}

/**
 * 首批的**字节预算**（约 8MB）。
 *
 * 实测 IPC 传输约 95MB/s，所以首批 ≈ 90ms 到位（加上解码总共约 170ms）。
 */
const FIRST_BATCH_BYTES = 8 * 1024 * 1024

/**
 * 首批应至少覆盖的**播放时长**（毫秒）。
 *
 * 光有字节预算不够：同样 8MB，帧延时 100ms 时能播 3.3 秒（绰绰有余），
 * 延时 20ms 时只够 0.66 秒 —— 而后台那批要 1 秒多才到，动画会先"播完"
 * 再跳回开头，看起来就是卡了一下。时长下限交给 Rust 侧判断（它才知道
 * 每帧的延时），两个预算**都满足才收尾**。
 */
const FIRST_BATCH_MIN_MS = 3000

/**
 * 取动图的一个**帧区间**。
 *
 * `maxBytes` 与 `minDurationMs` 都为 0 表示「取到结尾」。
 */
async function decodeAnimationChunk(
  path: string,
  purpose: DecodePurpose,
  start: number,
  maxBytes: number,
  minDurationMs: number,
): Promise<DecodedAnimation> {
  const raw = await invoke<unknown>('decode_animation', {
    path,
    purpose,
    start,
    maxBytes,
    minDurationMs,
  })
  return parseAnimation(toBytes(raw))
}

/**
 * 渐进式解码动图（GIF）。
 *
 * ## 为什么不是「一次拿完」
 * 实测样本 299 帧 × 250×250 = **71MB 裸字节，光传输就要 1.2 秒**
 * （约 60MB/s）。一次拿完的话，用户要对着空白干等 1.2 秒才看到第一帧。
 *
 * ## 现在的做法
 * 1. **首批**只取约 8MB（≈头几十帧，0.13 秒到位）→ `onBatch` 立刻回调，
 *    调用方马上就能显示并开始播放；
 * 2. 剩下的帧在**后台**继续取（`await` 已经返回，不阻塞任何 UI），
 *    每批也交给 `onBatch`，调用方追加到播放列表尾部。
 *
 * 播放速度远低于传输速度，所以后台批次总是先到。
 *
 * @param onBatch 每批帧的回调；`isFirst` 为 true 表示这是首批（调用方据此立即进入可播放状态）
 * @param isStale  返回 true 表示这次加载已作废（用户翻了别的图）—— 立即停止取后续批次
 */
export async function decodeAnimationProgressive(
  path: string,
  purpose: DecodePurpose,
  onBatch: (batch: DecodedAnimation, isFirst: boolean) => void,
  isStale: () => boolean,
): Promise<void> {
  const t0 = performance.now()
  const first = await decodeAnimationChunk(path, purpose, 0, FIRST_BATCH_BYTES, FIRST_BATCH_MIN_MS)
  if (isStale()) return
  onBatch(first, true)
  traceLog(
    `动图首批: ${(performance.now() - t0).toFixed(0)}ms / ${first.count}/${first.totalCount} 帧`,
  )

  // 首批已经够播；剩下的在后台补齐，**不阻塞调用方**
  if (first.count >= first.totalCount) return
  void (async () => {
    let start = first.count
    try {
      while (!isStale() && start < first.totalCount) {
        const t = performance.now()
        const batch = await decodeAnimationChunk(path, purpose, start, 0, 0)
        if (isStale()) return
        if (batch.count === 0) return
        onBatch(batch, false)
        start += batch.count
        traceLog(
          `动图补帧: ${batch.count} 帧 / ${(performance.now() - t).toFixed(0)}ms / ` +
            `${start}/${batch.totalCount}`,
        )
      }
    } catch (err) {
      // 后台批次失败不该影响已经在播的动画 —— 只是后面几帧缺失（循环时会跳）
      traceLog(`动图补帧失败: ${String(err)}`)
    }
  })()
}

/**
 * 解码结果缓存上限（条）。
 *
 * 预览图一张约 1–11 MB（取决于原图大小），10 条约 100 MB 量级；
 * 覆盖「当前 + 后 5 + 前 2」的预取窗口后还有少量余量。
 */
const CACHE_LIMIT = 10

/**
 * 文件版本 —— 缓存条目的「保质期标记」。
 *
 * 后端 `file_version` 只 `stat`（不解码），所以可以放心在每次取缓存前比对。
 */
export interface FileVersion {
  /** 修改时间（Unix 毫秒） */
  modifiedMs: number
  /** 字节数 */
  size: number
}

/**
 * 取文件版本；失败返回 `null`（= 「无法校验」）。
 *
 * ⚠️ **绝不能让看图流程因它失败**：拿不到版本只是退化成"不校验"，
 * 与本次修复前的行为一致，而抛错会连图都打不开。
 */
export async function getFileVersion(path: string): Promise<FileVersion | null> {
  try {
    return await invoke<FileVersion>('file_version', { path })
  } catch (err) {
    traceLog(`file_version 失败（该条缓存将不做版本校验）：${String(err)}`)
    return null
  }
}

/** 版本指纹；`null` 表示未知（调用方没给，或 stat 失败） */
function versionKey(v: FileVersion | null | undefined): string | null {
  return v ? `${v.modifiedMs}:${v.size}` : null
}

/**
 * 缓存条目。
 *
 * ⚠️ **必须带 `version`**（2026-10-11 修）：只按路径缓存时，文件被覆盖后
 * 界面会一直显示**旧像素** —— 用户报的「已预读取的图像，如果被覆盖了，
 * 还是显示原图像」就是这个。实测复现：三张纯色图，预取过的 b.png 被
 * 外部改成黄色后翻过去，显示的仍是绿色。
 */
interface CacheEntry {
  /** 解码时**那个文件**的版本；`null` = 未知（无法校验，只能放行） */
  version: string | null
  /** 缓存的是 Promise，这样并发的相同请求会共享同一次解码（去重） */
  promise: Promise<DecodedImage>
}

const cache = new Map<string, CacheEntry>()

function cacheKey(path: string, purpose: DecodePurpose): string {
  return `${purpose}::${path}`
}

/** 超出上限时淘汰最久未使用的条目（Map 保持插入序） */
function evictOverflow(): void {
  while (cache.size > CACHE_LIMIT) {
    const oldest = cache.keys().next().value
    if (oldest === undefined) break
    cache.delete(oldest)
  }
}

/** 预取解码结果到缓存（失败静默）。返回 Promise 便于顺序调度。 */
export async function prefetchImage(
  path: string,
  purpose: DecodePurpose = 'preview',
): Promise<void> {
  // ⚠️ 预取时就要把版本记进缓存条目：否则以后拿它用时无法判断
  //    文件是否被改过，预取等于埋了一个"永远显示旧图"的坑。
  const version = await getFileVersion(path)
  return decodeImage(path, purpose, version).then(
    () => undefined,
    () => undefined,
  )
}

/**
 * 丢弃某个路径的全部缓存（所有 purpose）。
 *
 * 覆盖保存后调用。**注意**：这不是唯一防线 —— 文件被别的程序改过时
 * 没人会调它，真正的兜底是缓存条目的**版本校验**（见 `decodeImage`）。
 */
export function invalidateImage(path: string): void {
  // key 形如 `${purpose}::${path}`，按后缀匹配
  const suffix = `::${path}`
  for (const key of [...cache.keys()]) {
    if (key.endsWith(suffix)) cache.delete(key)
  }
}

/**
 * 解码为可直接 `putImageData` 的像素数据（命中缓存则直接返回）。
 *
 * **传输为裸字节**（`tauri::ipc::Response` + 自定义协议 `fetch` 的
 * `response.arrayBuffer()`）：不经 base64、不经 JSON。这是 2026-10-09
 * 修好 `tauri.conf.json` 里 CSP 的 `connect-src ipc: http://ipc.localhost`
 * 之后才成立的快路径；此前 fetch 被 CSP 拦下会回退到 postMessage/eval
 * 通道，大图解码明显变慢。
 *
 * ## 缓存校验（2026-10-11）
 * 命中缓存前先比对 `version`：
 * - **一致** → 复用（省掉 100–300ms 解码，翻页才够快）；
 * - **不一致** → 文件已被覆盖，**丢弃旧像素重新解码**；
 * - `version` 传 `undefined`/`null`（调用方不关心 / stat 失败）→ 放行复用。
 *
 * 为什么必须这么做：缓存键只有路径，而"同一个路径"完全可以是**另一个文件**
 * （覆盖保存、别的程序改图、脚本重跑导出）。不复核的话，界面会一直显示
 * 旧像素，用户会以为"没保存成功"，甚至拿错图去交付。
 *
 * @param version 当前文件版本。调用方一般刚 `stat` 过（`getFileVersion`）——
 *                顺序上有保证：`stat` 在读字节**之前**，所以记下的版本
 *                只会"偏旧"（写发生在 stat 与读之间时），那种条目在**下一次**
 *                比对时就会被判为不一致而重解码，不会永久残留脏数据。
 */
export async function decodeImage(
  path: string,
  purpose: DecodePurpose = 'preview',
  version?: FileVersion | null,
): Promise<DecodedImage> {
  const key = cacheKey(path, purpose)
  const want = versionKey(version)

  const cached = cache.get(key)
  if (cached && (want === null || cached.version === want)) {
    // 命中：移到末尾（标记为最近使用）
    cache.delete(key)
    cache.set(key, cached)
    return cached.promise
  }
  if (cached) {
    // 文件已被覆盖 —— 这是本机制存在的全部理由，别删这段
    traceLog(`[cache] 文件版本已变（${cached.version} → ${want}），重新解码：${path}`)
    cache.delete(key)
  }

  const entry: CacheEntry = {
    version: want,
    promise: invoke<unknown>('decode_image', { path, purpose }).then((raw) =>
      parsePixels(toBytes(raw)),
    ),
  }
  cache.set(key, entry)
  evictOverflow()

  try {
    return await entry.promise
  } catch (err) {
    // 失败不入缓存，下次可重试
    if (cache.get(key) === entry) cache.delete(key)
    throw err
  }
}
