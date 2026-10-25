import { describe, expect, it } from 'vitest'

import { prefetchWindowEnd } from './useGridWindow'

/**
 * 只测纯函数部分：hook 本身依赖 IntersectionObserver，而本仓库的 vitest
 * 跑在 node 环境（没有 DOM）。窗口语义（哪些算可见）交给浏览器，
 * 这里钉的是「可见范围 → 要预取到第几张」这一步的边界。
 */
describe('缩略图预取窗口', () => {
  it('可见 0..19 张时预取到 49（后 30 张）', () => {
    expect(prefetchWindowEnd(19, 1000, 30)).toBe(49)
  })

  it('没有可见项（空网格 / 列表模式）→ 不预取', () => {
    expect(prefetchWindowEnd(-1, 1000, 30)).toBe(-1)
    expect(prefetchWindowEnd(-1, 0, 30)).toBe(-1)
    expect(prefetchWindowEnd(5, 0, 30)).toBe(-1)
  })

  it('钳到列表末尾，不越界', () => {
    // 最后 10 张可见，后面只剩 0 张 → 窗口就停在最后一张
    expect(prefetchWindowEnd(1209, 1225, 30)).toBe(1224)
    // 只有 5 张时，窗口不超过 4
    expect(prefetchWindowEnd(0, 5, 30)).toBe(4)
  })

  it('ahead = 0 就是「只取可见的」（关掉预取）', () => {
    expect(prefetchWindowEnd(3, 100, 0)).toBe(3)
  })
})
