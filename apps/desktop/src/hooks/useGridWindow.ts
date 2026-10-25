/**
 * 网格的「当前可见窗口」—— 用来决定预取到哪一张。
 *
 * ## 为什么不用滚动位置算
 * 网格是 `repeat(auto-fill, minmax(--pv-card, 1fr))`，列数随容器宽度变；
 * 卡片里还套了 `content-visibility: auto`，离屏元素的**真实高度**要等浏览器
 * 渲染过才知道（`contain-intrinsic-size` 只是估算）。用 `scrollTop / 行高` 推
 * 索引，一旦估算高度和真实高度差几像素，累加到第 300 行就是十几张的偏差。
 *
 * `IntersectionObserver` 由浏览器在合成阶段算好再回调，滚动本身零开销，
 * 也不依赖任何布局假设 —— 和我们已有的 `useInView` 是同一套思路。
 *
 * ## 为什么在这里而不是每张卡片自己算
 * 「我在第几个」是**列表的顺序信息**，卡片自己不知道。父层统一观察一遍
 * （一次 `querySelectorAll` + 一批 `observe`），比让每个单元格互相打听便宜得多。
 *
 * ## 语义：当前可见，不累积历史最大值
 * 往回滚时，后面那几十张已经不需要预取了。这里刻意让 `visibleUntil` **可以变小**，
 * 否则滚到底再回顶部会让整个目录都进入预取窗口。
 */

import { useEffect, useState } from 'react'
import type { RefObject } from 'react'

/**
 * 预取窗口的结束索引（含）。抽成纯函数，便于钉边界。
 *
 * - 没有可见项（`visibleMax < 0`）或空列表 → `-1`，表示**不预取**
 * - 否则 = 可见最大索引 + `ahead`，并**钳到列表末尾**（不能越界）
 */
export function prefetchWindowEnd(visibleMax: number, count: number, ahead: number): number {
  if (visibleMax < 0 || count <= 0) return -1
  return Math.min(count - 1, visibleMax + ahead)
}

/**
 * @param gridRef   网格容器（同时是滚动容器，作为 observer 的 root）
 * @param count     当前铺在网格里的条目数（分页追加后要重新观察新元素）
 * @param resetKey  换目录 / 换排序时变化；此时旧的可见位置毫无意义，必须清零
 * @param ahead     视口之后再预取多少张
 *
 * @returns 预取窗口的最后一个索引（含）；`-1` = 没有窗口（空网格 / 列表模式）
 */
export function useGridWindow<T extends HTMLElement>(
  gridRef: RefObject<T | null>,
  count: number,
  resetKey: string,
  ahead: number,
): number {
  const [visibleMax, setVisibleMax] = useState(-1)
  /** 记录这个 `visibleMax` 属于哪一份数据，避免换目录后拿旧值当新窗口 */
  const [keyAtMax, setKeyAtMax] = useState(resetKey)

  // 换目录 / 换排序：旧的可见索引指的根本不是同一批图
  useEffect(() => {
    setVisibleMax(-1)
    setKeyAtMax(resetKey)
  }, [resetKey])

  useEffect(() => {
    const root = gridRef.current
    // 列表模式没有 `data-idx`（那里不显示缩略图），不必观察
    if (!root || count === 0) return
    if (typeof IntersectionObserver === 'undefined') return

    /** 当前与视口相交的索引集合（离开就删，所以 max 能跟着回落） */
    const seen = new Set<number>()
    let raf = 0

    const flush = () => {
      raf = 0
      let max = -1
      for (const i of seen) {
        if (i > max) max = i
      }
      // 值没变就不 setState：滚动一行会触发多次回调，避免无意义的重渲染
      setVisibleMax((prev) => (prev === max ? prev : max))
    }

    const io = new IntersectionObserver(
      (entries) => {
        let changed = false
        for (const e of entries) {
          const raw = (e.target as HTMLElement).dataset.idx
          if (raw == null) continue
          const i = Number(raw)
          if (e.isIntersecting) {
            if (!seen.has(i)) {
              seen.add(i)
              changed = true
            }
          } else if (seen.delete(i)) {
            changed = true
          }
        }
        if (changed && raf === 0) raf = requestAnimationFrame(flush)
      },
      // root 必须显式给：观察的是「在网格里可见」，不是「在窗口里可见」——
      // 网格顶部还有工具栏和筛选条，用视图层会在被遮住时仍算「可见」。
      { root },
    )

    root.querySelectorAll<HTMLElement>('[data-idx]').forEach((el) => io.observe(el))
    return () => {
      if (raf) cancelAnimationFrame(raf)
      io.disconnect()
    }
  }, [gridRef, count])

  if (keyAtMax !== resetKey) return -1
  return prefetchWindowEnd(visibleMax, count, ahead)
}
