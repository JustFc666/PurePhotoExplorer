import { describe, expect, it } from 'vitest'

import { compareName, descForSort, makeComparator, type Sortable } from './sortItems'

function row(
  name: string,
  o: Partial<Pick<Sortable, 'size' | 'mtime' | 'format' | 'takenAt'>> = {},
): Sortable & { name: string } {
  return {
    name,
    size: o.size ?? 0,
    mtime: o.mtime ?? 0,
    format: o.format ?? 'jpeg',
    takenAt: o.takenAt ?? null,
  }
}

const sorted = (arr: Sortable[], sort: Parameters<typeof makeComparator>[0], desc: boolean) =>
  [...arr].sort(makeComparator(sort, desc)).map((x) => x.name)

describe('文件名排序', () => {
  it('数字感知：_DSC2 排在 _DSC10 前面（字节序会反过来）', () => {
    // 这是相机文件的真实形态；普通字符串比较会把 _DSC10000 排到 _DSC9999 前
    expect(compareName('_DSC2.ARW', '_DSC10.ARW')).toBeLessThan(0)
    expect(compareName('_DSC9999.ARW', '_DSC10000.ARW')).toBeLessThan(0)
  })

  it('大小写不敏感（.JPG 和 .jpg 视为同类，再按其余字符比）', () => {
    expect(compareName('IMG_1.JPG', 'img_1.jpg')).toBe(0)
  })
})

describe('排序方向', () => {
  it('文件名与文件类型升序，其余降序', () => {
    expect(descForSort('name')).toBe(false)
    expect(descForSort('format')).toBe(false)
    expect(descForSort('size')).toBe(true)
    expect(descForSort('mtime')).toBe(true)
    expect(descForSort('taken')).toBe(true)
  })

  it('大小降序：大的在前', () => {
    const arr = [row('a', { size: 10 }), row('b', { size: 300 }), row('c', { size: 50 })]
    expect(sorted(arr, 'size', true)).toEqual(['b', 'c', 'a'])
  })

  it('修改时间降序：新的在前', () => {
    const arr = [row('a', { mtime: 100 }), row('b', { mtime: 900 }), row('c', { mtime: 500 })]
    expect(sorted(arr, 'mtime', true)).toEqual(['b', 'c', 'a'])
  })

  it('文件类型升序：同类型聚在一起，类型内按名字', () => {
    const arr = [
      row('x.ARW', { format: 'raw' }),
      row('b.jpg', { format: 'jpeg' }),
      row('a.jpg', { format: 'jpeg' }),
      row('c.png', { format: 'png' }),
    ]
    expect(sorted(arr, 'format', false)).toEqual(['a.jpg', 'b.jpg', 'c.png', 'x.ARW'])
  })
})

describe('拍摄时间', () => {
  it('有 takenAt 就按它排（降序）', () => {
    const arr = [
      row('a', { takenAt: 100 }),
      row('b', { takenAt: 900 }),
      row('c', { takenAt: 500 }),
    ]
    expect(sorted(arr, 'taken', true)).toEqual(['b', 'c', 'a'])
  })

  it('缺 takenAt 时回退到 mtime，而不是当成 0 挤到最后', () => {
    // 浏览模式初始没有 EXIF，如果不回退，所有文件会挤在一起（顺序看起来随机）
    const arr = [
      row('a', { takenAt: null, mtime: 900 }),
      row('b', { takenAt: 500, mtime: 100 }),
      row('c', { takenAt: null, mtime: 100 }),
    ]
    expect(sorted(arr, 'taken', true)).toEqual(['a', 'b', 'c'])
  })
})

describe('稳定性', () => {
  it('主键相同时按文件名兜底（每次重排结果一致）', () => {
    const arr = [
      row('c', { size: 10 }),
      row('a', { size: 10 }),
      row('b', { size: 10 }),
    ]
    // 升序 / 降序都得是同一个确定性顺序（a、b、c）
    expect(sorted(arr, 'size', true)).toEqual(['a', 'b', 'c'])
    expect(sorted(arr, 'size', false)).toEqual(['a', 'b', 'c'])
  })

  it('浏览模式没有数据的排序（added/rating）→ 按文件名，不会乱', () => {
    const arr = [row('c'), row('a'), row('b')]
    expect(sorted(arr, 'added', true)).toEqual(['a', 'b', 'c'])
    expect(sorted(arr, 'rating', true)).toEqual(['a', 'b', 'c'])
  })
})
