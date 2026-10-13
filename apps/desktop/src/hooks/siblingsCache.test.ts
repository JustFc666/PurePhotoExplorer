/**
 * 同目录列表缓存决策的单测。
 *
 * **回归背景（2026-10-11）**：用户在当前文件夹下另存为出一个新图，
 * 或从别处拷一张图进来，翻页序列里根本没有它 —— 必须关掉重开才认出来。
 * 根因是列表缓存**只在目录路径变化时**才失效（为了省一次 IPC）。
 *
 * 这里把「要不要重列」的判据钉死，防止为了省一次 IPC 又把 `force` 通道删掉。
 */

import { describe, expect, it } from 'vitest'
import { emptySiblingsCache, locateInList, shouldRelist } from './siblingsCache'

const DIR = 'C:/photos'
const OTHER = 'C:/photos/sub'

describe('shouldRelist', () => {
  it('首次进入（无缓存）必须重列', () => {
    expect(shouldRelist(emptySiblingsCache(), DIR, false)).toBe(true)
  })

  it('目录变了必须重列', () => {
    const cache = { dir: DIR, list: ['a.jpg'] }
    expect(shouldRelist(cache, OTHER, false)).toBe(true)
  })

  it('目录没变且未强制时不重列（这是「省一次 IPC」的正常路径）', () => {
    const cache = { dir: DIR, list: ['a.jpg'] }
    expect(shouldRelist(cache, DIR, false)).toBe(false)
  })

  it('⭐ 目录没变但强制时仍要重列 —— 这条就是「新文件要重开才认出来」的根因', () => {
    const cache = { dir: DIR, list: ['a.jpg'] }
    // 少一个 force 分支，同目录新增的文件就永远看不见
    expect(shouldRelist(cache, DIR, true)).toBe(true)
  })

  it('force 优先于任何缓存状态', () => {
    for (const cached of [
      emptySiblingsCache(),
      { dir: DIR, list: [] },
      { dir: OTHER, list: ['x.jpg', 'y.jpg'] },
    ]) {
      expect(shouldRelist(cached, DIR, true)).toBe(true)
    }
  })

  it('空缓存的 dir 是 null，不会与任何真实目录相等', () => {
    // 曾用空字符串当「无缓存」标记 —— 空目录名会让首个文件被误判成「同目录」
    expect(emptySiblingsCache().dir).toBeNull()
    expect(emptySiblingsCache().list).toEqual([])
  })
})

describe('locateInList', () => {
  it('找到时返回下标', () => {
    const list = ['a.jpg', 'b.jpg', 'c.jpg']
    expect(locateInList(list, 'b.jpg')).toBe(1)
  })

  it('找不到时返回 -1（调用方显示「未知」而不是错误序号）', () => {
    expect(locateInList(['a.jpg'], 'z.jpg')).toBe(-1)
  })

  it('空列表时返回 -1', () => {
    expect(locateInList([], 'a.jpg')).toBe(-1)
  })

  it('⭐ 新文件加入列表后能被定位到 —— 修好之后应有的行为', () => {
    // 修之前：列表里根本没有这个文件
    const before = ['a.jpg', 'b.jpg']
    expect(locateInList(before, 'b_已裁剪.jpg')).toBe(-1)

    // 修之后：重列目录把新文件纳进来
    const after = ['a.jpg', 'b.jpg', 'b_已裁剪.jpg']
    expect(locateInList(after, 'b_已裁剪.jpg')).toBe(2)
  })
})
