/**
 * 查询参数组装的**规则测试**（不是文案测试）。
 *
 * 这些用例一条条对应真实踩过的坑 —— 改规则会让它们红，改字段顺序/写法不会。
 * 一旦某条挂了，先看它上面那行注释里的日期，别直接把期望值改掉。
 */

import { describe, expect, it } from 'vitest'
import {
  baseKeyOf,
  baseScopeOf,
  buildManagerQuery,
  scopeKeyOf,
  type BuildQueryInput,
} from './query'
import { PAGE_LIMIT } from './constants'
import type { Scope } from './types'

/** 一份"什么都没筛"的基线；每个用例只改它关心的那一两项。 */
function input(over: Partial<BuildQueryInput> = {}): BuildQueryInput {
  const scope: Scope = over.scope ?? { kind: 'all' }
  return {
    scope,
    baseScope: over.baseScope ?? scope,
    formats: [],
    camera: [],
    years: [],
    placeFilter: [],
    personId: null,
    tagFilter: null,
    searchTerm: '',
    sort: 'taken',
    desc: true,
    ...over,
  }
}

describe('buildManagerQuery · 什么时候不该查索引', () => {
  it('浏览模式返回 null（否则索引结果会盖掉磁盘列表）', () => {
    expect(buildManagerQuery(input({ scope: { kind: 'browse', path: 'C:/p', name: 'p' } }))).toBeNull()
  })

  it('启动默认态返回 null', () => {
    expect(buildManagerQuery(input({ scope: { kind: 'none' } }))).toBeNull()
  })

  it('入库范围正常返回，且带上排序与分页', () => {
    const q = buildManagerQuery(input({ sort: 'size', desc: false }))
    expect(q).toMatchObject({ sort: 'size', desc: false, limit: PAGE_LIMIT, offset: 0 })
  })
})

describe('buildManagerQuery · 范围', () => {
  it('目录范围只给 dirIds（精确匹配，不含子目录）', () => {
    const scope: Scope = { kind: 'folder', id: 7, path: 'C:/p', name: 'p' }
    expect(buildManagerQuery(input({ scope }))).toMatchObject({ dirIds: [7] })
    // 目录范围不该带图库，否则会并集出别处的图
    expect(buildManagerQuery(input({ scope }))?.libraryId).toBeUndefined()
  })

  it('图库范围只给 libraryId', () => {
    const scope: Scope = { kind: 'library', id: 3, name: '测试图库' }
    expect(buildManagerQuery(input({ scope }))).toMatchObject({ libraryId: 3 })
    expect(buildManagerQuery(input({ scope }))?.dirIds).toBeUndefined()
  })
})

describe('buildManagerQuery · 标签', () => {
  it('标签范围带 tagsAny；从图库里点进来的要一起带 libraryId', () => {
    // 2026-10-29 用户报的「rednote → 人像」查出全库人像
    // （用「半身」当样本：2026-11-02 起「人像/单人/合照」都改按人脸算，见下面的用例）
    const scoped: Scope = { kind: 'tag', name: '半身', libraryId: 9 }
    expect(buildManagerQuery(input({ scope: scoped }))).toMatchObject({
      tagsAny: ['半身'],
      libraryId: 9,
    })
  })

  it('标签范围没有图库上下文时保持全局', () => {
    const scope: Scope = { kind: 'tag', name: '半身' }
    expect(buildManagerQuery(input({ scope }))?.libraryId).toBeUndefined()
  })

  it('「人像」按检出人脸筛，但仍要守住图库范围', () => {
    const scoped = buildManagerQuery(
      input({ scope: { kind: 'tag', name: '人像', libraryId: 9 } }),
    )
    expect(scoped).toMatchObject({ hasFaces: true, libraryId: 9 })
    expect(scoped?.tagsAny).toBeUndefined()

    const global = buildManagerQuery(input({ scope: { kind: 'tag', name: '人像' } }))
    expect(global?.hasFaces).toBe(true)
    expect(global?.libraryId).toBeUndefined()
  })

  it('标签组：范围要从 baseScope 取（tagGroup 自己不带 id）', () => {
    /*
     * 2026-10-23：点标签组后 `scope.kind === 'tagGroup'`，直接按它取 lib/dir
     * 会得到 undefined → 静悄悄变成全库。必须回落到筛选前的那个图库。
     */
    const scope: Scope = { kind: 'tagGroup', label: '人像', names: ['单人', '全身'] }
    const q = buildManagerQuery(
      input({ scope, baseScope: { kind: 'library', id: 4, name: '测试图库' } }),
    )
    /*
     * 「人像」这一组自 2026-11-02 起**按检出人脸筛**（用户要求「有脸就是人像」），
     * 所以不再是 tagsAny；但「图库范围必须带上」这条老规则仍然成立。
     */
    expect(q).toMatchObject({ hasFaces: true, libraryId: 4 })
    expect(q?.tagsAny).toBeUndefined()
  })

  it('「人像」按人脸筛；它的「单人 / 合照」按脸数筛，普通二级词照旧按标签', () => {
    const group = buildManagerQuery(
      input({ scope: { kind: 'tagGroup', label: '人像', names: ['单人', '合照'] } }),
    )
    expect(group?.hasFaces).toBe(true)
    expect(group?.tagsAny).toBeUndefined()

    /*
     * ⚠️ 二级 chip 传的是**组合名**（`人像 · 合照`），不是裸的「合照」。
     * 2026-11-02 就是只判裸名，导致数字写 52、主区只筛出 5 张（退回了标签口径）。
     * 这几条用真实形态钉住，别再改回去。
     */
    const single = buildManagerQuery(
      input({ scope: { kind: 'tagGroup', label: '人像 · 单人', names: ['单人'] } }),
    )
    expect(single?.faceCountEq).toBe(1)
    expect(single?.tagsAny).toBeUndefined()

    const multi = buildManagerQuery(
      input({ scope: { kind: 'tagGroup', label: '人像 · 合照', names: ['合照'] } }),
    )
    expect(multi?.faceCountMin).toBe(2)
    expect(multi?.tagsAny).toBeUndefined()

    /* 其余二级词（半身/全身/儿童…）没有人脸口径，仍然按标签名筛 */
    const leaf = buildManagerQuery(
      input({ scope: { kind: 'tagGroup', label: '人像 · 半身', names: ['半身'] } }),
    )
    expect(leaf?.hasFaces).toBeUndefined()
    expect(leaf?.faceCountEq).toBeUndefined()
    expect(leaf?.tagsAny).toEqual(['半身'])
  })

  it('「无标签」+「按标签筛」不再拼出恒空集', () => {
    // 2026-10-31：既要求有「半身」又要求没有任何标签 → 一张不显示
    const scope: Scope = { kind: 'tag', name: '半身' }
    expect(buildManagerQuery(input({ scope, tagFilter: 'none' }))?.hasTags).toBeUndefined()
  })

  it('只开「无标签」时正常生效', () => {
    expect(buildManagerQuery(input({ tagFilter: 'none' }))?.hasTags).toBe(false)
    expect(buildManagerQuery(input({ tagFilter: 'has' }))?.hasTags).toBe(true)
    expect(buildManagerQuery(input({ tagFilter: null }))?.hasTags).toBeUndefined()
  })
})

describe('buildManagerQuery · 筛选条', () => {
  it('空数组一律不拼（否则后端会当成"筛成空"）', () => {
    const q = buildManagerQuery(input())
    expect(q?.formats).toBeUndefined()
    expect(q?.camera).toBeUndefined()
    expect(q?.years).toBeUndefined()
    expect(q?.places).toBeUndefined()
    expect(q?.personId).toBeUndefined()
    expect(q?.nameLike).toBeUndefined()
  })

  it('有值时逐个拼上', () => {
    const q = buildManagerQuery(
      input({
        formats: ['jpg'],
        camera: ['X-T5'],
        years: [2024],
        placeFilter: ['上海'],
        personId: 12,
        searchTerm: '日落',
      }),
    )
    expect(q).toMatchObject({
      formats: ['jpg'],
      camera: ['X-T5'],
      years: [2024],
      places: ['上海'],
      personId: 12,
      nameLike: '日落',
    })
  })

  it('地点范围也走 places（城市名精确匹配）', () => {
    const scope: Scope = { kind: 'place', name: '上海' }
    expect(buildManagerQuery(input({ scope }))?.places).toEqual(['上海'])
  })
})

describe('层级辅助', () => {
  it('baseScopeOf：标签组回落到筛选前的范围，其他原样', () => {
    const lib: Scope = { kind: 'library', id: 1, name: 'A' }
    const group: Scope = { kind: 'tagGroup', label: '人像', names: ['单人'] }
    expect(baseScopeOf(group, lib)).toBe(lib)
    expect(baseScopeOf(group, null)).toEqual({ kind: 'none' })
    expect(baseScopeOf(lib, group)).toBe(lib)
  })

  it('scopeKeyOf：标签组内词条顺序不影响指纹', () => {
    const a: Scope = { kind: 'tagGroup', label: '人像', names: ['单人', '全身'] }
    const b: Scope = { kind: 'tagGroup', label: '人像', names: ['全身', '单人'] }
    expect(scopeKeyOf(a)).toBe(scopeKeyOf(b))
    // 顺序不同但拼出来的 tagsAny 也不同 —— 指纹相同才不会来回重查
  })

  it('scopeKeyOf：不同范围不会撞键', () => {
    const keys = [
      scopeKeyOf({ kind: 'all' }),
      scopeKeyOf({ kind: 'none' }),
      scopeKeyOf({ kind: 'browse', path: 'C:/p', name: 'p' }),
      scopeKeyOf({ kind: 'tag', name: '人像' }),
      scopeKeyOf({ kind: 'place', name: '上海' }),
      scopeKeyOf({ kind: 'library', id: 1, name: 'A' }),
      scopeKeyOf({ kind: 'folder', id: 1, path: 'C:/p', name: 'p' }),
      scopeKeyOf({ kind: 'tagGroup', label: '人像', names: ['单人'] }),
    ]
    expect(new Set(keys).size).toBe(keys.length)
    // 图库 1 和目录 1 同名 id 也不能撞
    expect(keys[5]).not.toBe(keys[6])
  })

  it('baseKeyOf：图库/目录/浏览带上 id 或路径，其余用 kind', () => {
    expect(baseKeyOf({ kind: 'library', id: 3, name: 'A' })).toBe('library:3')
    expect(baseKeyOf({ kind: 'folder', id: 5, path: 'C:/p', name: 'p' })).toBe('folder:5')
    expect(baseKeyOf({ kind: 'browse', path: 'C:/p', name: 'p' })).toBe('browse:C:/p')
    expect(baseKeyOf({ kind: 'none' })).toBe('none')
    expect(baseKeyOf({ kind: 'all' })).toBe('all')
  })
})
