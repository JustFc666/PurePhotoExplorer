import { describe, expect, it } from 'vitest'

import { mergeDimensions, mergeTakenAt } from './mergeDimensions'
import type { FsImage } from '../ipc/browse'

/** 造一个只有名字和尺寸的最小行（其余字段对合并逻辑无关） */
function row(name: string, width: number | null = null, height: number | null = null): FsImage {
  return {
    path: `C:/x/${name}`,
    name,
    size: 1,
    mtime: 0,
    format: 'jpeg',
    isRaw: false,
    takenAt: null,
    width,
    height,
  }
}

describe('后台补尺寸的合并', () => {
  it('按全局索引对齐，不会把尺寸贴到别的照片上', () => {
    const items = [row('a'), row('b'), row('c'), row('d')]
    // 从索引 2 开始补两张
    const out = mergeDimensions(items, 2, [
      [100, 50],
      [200, 60],
    ])
    expect(out[0]).toMatchObject({ name: 'a', width: null })
    expect(out[1]).toMatchObject({ name: 'b', width: null })
    expect(out[2]).toMatchObject({ name: 'c', width: 100, height: 50 })
    expect(out[3]).toMatchObject({ name: 'd', width: 200, height: 60 })
  })

  it('null（RAW / 探不出）不写 0，保留缺省', () => {
    const out = mergeDimensions([row('a'), row('b')], 0, [[10, 20], null])
    expect(out[0]).toMatchObject({ width: 10, height: 20 })
    expect(out[1]).toMatchObject({ width: null, height: null })
  })

  it('越界不 panic，也不扩容', () => {
    const items = [row('a')]
    const out = mergeDimensions(items, 0, [[1, 2], [3, 4], [5, 6]])
    expect(out).toHaveLength(1)
    expect(out[0]).toMatchObject({ width: 1, height: 2 })
  })

  it('start 超出长度 → 原样返回', () => {
    const items = [row('a')]
    const out = mergeDimensions(items, 5, [[1, 2]])
    expect(out).toHaveLength(1)
    expect(out[0]).toMatchObject({ width: null })
  })

  it('返回新数组但不动入参（React 靠引用变化重渲染）', () => {
    const items = [row('a')]
    const out = mergeDimensions(items, 0, [[7, 8]])
    expect(out).not.toBe(items)
    expect(out[0]).not.toBe(items[0])
    expect(items[0]?.width).toBeNull()
  })
})

describe('后台补拍摄时间的合并', () => {
  it('按全局索引对齐，null 不会覆盖已有值', () => {
    const items = [row('a'), row('b'), row('c')]
    const out = mergeTakenAt(items, 1, [1700000000, null])
    expect(out[0]).toMatchObject({ name: 'a', takenAt: null })
    expect(out[1]).toMatchObject({ name: 'b', takenAt: 1700000000 })
    // 第三张没有对应的时间（数组短了）→ 保持 null，不写成 0
    expect(out[2]).toMatchObject({ name: 'c', takenAt: null })
  })

  it('越界与空数组都安全', () => {
    const items = [row('a')]
    expect(mergeTakenAt(items, 5, [1])).toHaveLength(1)
    expect(mergeTakenAt(items, 0, [])[0]).toMatchObject({ takenAt: null })
  })
})
