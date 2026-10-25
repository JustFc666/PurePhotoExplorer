/**
 * 把后台补来的尺寸**原地合并**进浏览列表。
 *
 * 抽成纯函数而不是塞在 `setListing` 回调里，是因为这里有最容易错的
 * off-by-one：`start` 是**全局索引**，`dims[k]` 对应 `items[start + k]`。
 * 一旦算错，尺寸会贴到**别的照片**上 —— 界面看起来完全正常，但每张卡片的
 * 「1920 × 1080」都是错的。这种错只能靠测试钉住。
 *
 * 规则：
 * - 长度不一致时以**较短**的一侧为准（后端少回几个不该把列表搞乱）
 * - `dims[k] === null`（探不出 / RAW）→ 保留原来的 `null`，不写 0
 * - 索引越界 → 跳过
 * - 返回**新数组**（React 要靠引用变化重渲染），但不改传入的对象
 */

import type { FsImage } from '../ipc/browse'

export function mergeDimensions(
  items: FsImage[],
  start: number,
  dims: Array<[number, number] | null>,
): FsImage[] {
  const next = items.slice()
  const n = Math.min(dims.length, next.length - start)
  if (n <= 0) return next
  for (let k = 0; k < n; k++) {
    const d = dims[k]
    const idx = start + k
    const it = next[idx]
    if (!d || !it) continue
    next[idx] = { ...it, width: d[0], height: d[1] }
  }
  return next
}

/**
 * 把后台读来的**拍摄时间**合并进浏览列表。
 *
 * 与 [`mergeDimensions`] 同样的索引语义（`start` 是全局索引，最容易 off-by-one），
 * 区别只是「读不出时保持 null」—— 拍摄时间缺失是**常态**（不是所有文件都有 EXIF），
 * 不能因此把已读到的值清掉。
 */
export function mergeTakenAt(
  items: FsImage[],
  start: number,
  times: Array<number | null>,
): FsImage[] {
  const next = items.slice()
  const n = Math.min(times.length, next.length - start)
  if (n <= 0) return next
  for (let k = 0; k < n; k++) {
    const t = times[k]
    const idx = start + k
    const it = next[idx]
    if (t == null || !it) continue
    next[idx] = { ...it, takenAt: t }
  }
  return next
}
