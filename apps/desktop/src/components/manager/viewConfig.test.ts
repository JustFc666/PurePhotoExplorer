/**
 * `viewConfig` 里**纯规则**的测试：侧栏宽度夹紧、千分位、路径提取、格式显示名。
 *
 * 这些原先要么是内联表达式、要么夹在 `ManagerView` 的模块顶层，只在拖拽/渲染
 * 时被顺带执行到。提成具名函数后可以直接喂边界值。
 */

import { describe, expect, it } from 'vitest'

import {
  SIDE_DEFAULT,
  SIDE_MAX,
  SIDE_MIN,
  clampSideWidth,
  filePathsOf,
  fmtCount,
  formatLabelOf,
} from './viewConfig'
import type { GridItem } from '../../hooks/useManager'

describe('clampSideWidth', () => {
  it('区间内原样返回', () => {
    expect(clampSideWidth(300)).toBe(300)
  })

  it('拖过头就停在两端', () => {
    expect(clampSideWidth(-500)).toBe(SIDE_MIN)
    expect(clampSideWidth(99999)).toBe(SIDE_MAX)
  })

  it('边界值本身算合法（不是「必须大于」）', () => {
    expect(clampSideWidth(SIDE_MIN)).toBe(SIDE_MIN)
    expect(clampSideWidth(SIDE_MAX)).toBe(SIDE_MAX)
    expect(clampSideWidth(SIDE_DEFAULT)).toBe(SIDE_DEFAULT)
  })

  it('小数不取整 —— 拖拽是亚像素的，取整会让条一卡一卡', () => {
    expect(clampSideWidth(240.6)).toBe(240.6)
  })
})

describe('fmtCount', () => {
  it('按中文习惯加千分位', () => {
    expect(fmtCount(16682)).toBe('16,682')
    expect(fmtCount(0)).toBe('0')
  })
})

describe('filePathsOf', () => {
  const row = (kind: GridItem['kind'], path: string | null) =>
    ({ kind, path }) as unknown as GridItem

  it('只留拿得到路径的行，顺序不动', () => {
    const items = [row('file', 'C:\\a\\1.jpg'), row('dir', 'C:\\a\\sub'), row('file', 'C:\\a\\2.jpg')]
    expect(filePathsOf(items)).toEqual(['C:\\a\\1.jpg', 'C:\\a\\sub', 'C:\\a\\2.jpg'])
  })

  /**
   * 这条是**有意为之**：`filePathsOf` 不按 `kind` 过滤（过滤由调用方先做，
   * 例如 `selectedFileItems`）。⛔ 别在这里偷偷加上 `kind !== 'dir'` ——
   * 侧栏「加入图库」正是靠它取**目录**路径。
   */
  it('目录也留着 —— 过滤是调用方的事', () => {
    expect(filePathsOf([row('dir', 'C:\\a\\sub')])).toEqual(['C:\\a\\sub'])
  })

  it('没路径的行跳过（离线卷 / 未挂载）', () => {
    expect(filePathsOf([row('file', null), row('file', 'D:\\b.jpg')])).toEqual(['D:\\b.jpg'])
  })
})

describe('formatLabelOf', () => {
  it('认识的格式给短名（jpeg 显示成 JPG）', () => {
    expect(formatLabelOf('jpeg')).toBe('JPG')
    expect(formatLabelOf('webp')).toBe('WebP')
  })

  it('认不出的格式退化成大写原名，不给 undefined', () => {
    expect(formatLabelOf('heif2')).toBe('HEIF2')
  })

  it('`?` 是「认不出容器」的占位，显示成「未知」', () => {
    expect(formatLabelOf('?')).toBe('未知')
  })
})
