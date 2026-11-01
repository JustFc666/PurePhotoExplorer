import { describe, expect, test } from 'vitest'
import {
  CLEARED_FILTERS,
  describeActiveFilters,
  filtersToClearOnScopeChange,
} from './scopeSwitch'

describe('换范围清筛选', () => {
  test('人物筛选要清 —— 2026-11-01「切库后一张图都没有」的直接原因', () => {
    expect(filtersToClearOnScopeChange().person).toBe(true)
  })

  test('四个维度全清，不留任何叠加条件', () => {
    expect(filtersToClearOnScopeChange()).toEqual(CLEARED_FILTERS)
    expect(Object.values(CLEARED_FILTERS).every(Boolean)).toBe(true)
  })

  test('返回的是共享常量，调用方改不动它', () => {
    const a = filtersToClearOnScopeChange()
    expect(a).toBe(CLEARED_FILTERS)
  })

  test('维度清单锁定：加新筛选必须显式改这个常量，免得又漏清', () => {
    expect(Object.keys(CLEARED_FILTERS).sort()).toEqual(['person', 'place', 'search', 'tag'])
  })
})

describe('空态写出生效条件', () => {
  test('全空 → 空数组（交给通用文案，不写出半截话）', () => {
    expect(describeActiveFilters({})).toEqual([])
    expect(describeActiveFilters({ places: [], search: '   ', tagFilter: null })).toEqual([])
  })

  test('人物条件单独出现时，点名到人', () => {
    expect(describeActiveFilters({ personName: '张三' })).toEqual(['人物「张三」'])
  })

  test('多条件按 人 → 标签 → 地点 → 搜索 排列', () => {
    expect(
      describeActiveFilters({
        personName: '张三',
        tagFilter: 'has',
        places: ['上海', '北京'],
        search: '  日落 ',
      }),
    ).toEqual(['人物「张三」', '有标签', '地点「上海、北京」', '搜索「日落」'])
  })

  test('无标签与有标签互斥，不会同时出现', () => {
    const out = describeActiveFilters({ tagFilter: 'none' })
    expect(out).toEqual(['无标签'])
    expect(out).not.toContain('有标签')
  })
})
