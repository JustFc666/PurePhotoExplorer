/**
 * 从界面状态算出**查询参数**与**范围指纹**。
 *
 * 为什么单独成文件：这三件事原先是 `useManager.ts` 里一个 60 行的 `useEffect`
 * 加两段嵌套三元，夹在 state 与动作中间。而它们恰好是历史上出错最多的地方 ——
 * 2026-10-23「点标签后计数变全库」、10-28「切图库后地点不变」、
 * 10-29「rednote → 人像查出全库人像」、10-31「无标签 + 有标签 = 恒空」，
 * 全部是这里拼错一个字段。
 *
 * 搬成纯函数之后，**每条规则都能被单测钉住**（见 `query.test.ts`），
 * 不用再靠人眼在千行 hook 里守。
 *
 * ## 三条容易踩的规则
 * 1. ⛔ **`browse` / `none` 不发查询**（返回 `null`）：浏览模式的数据来自
 *    `fs_list_images`，查索引会把它覆盖掉；`none` 是启动默认态，不该有图。
 * 2. ⛔ **范围要用 `baseScope` 而不是 `scope`**：`tagGroup` 自己不带图库/目录，
 *    直接按它取 `id` 会得到 `undefined` → 静悄悄退回全库。
 * 3. ⛔ **`hasTags` 要过 `resolveHasTags`**：直接按 `tagFilter` 拼会拼出
 *    「既要有 X 标签、又要求这张图没有任何标签」的恒空集。
 */

import type { ManagerQuery, ManagerSort } from '../../ipc/manager'
import { PAGE_LIMIT } from './constants'
import { resolveHasTags } from '../filterRules'
import type { Scope } from './types'

/**
 * 剥掉「标签筛选」那一层之后的范围。
 *
 * `tagGroup` 是叠在某个图库/目录之上的一层筛选，自己不带范围；
 * 点进去之前的范围存在 `useManager` 的 `scopeBeforeTag` ref 里，由调用方传进来。
 */
export function baseScopeOf(scope: Scope, beforeTag: Scope | null): Scope {
  return scope.kind === 'tagGroup' ? (beforeTag ?? { kind: 'none' }) : scope
}

/**
 * 范围的**指纹**（含标签筛选那一层）。
 *
 * 用途：作为查询 effect 的依赖项。`scope` 每次都是新对象，直接进依赖会无限查询，
 * 所以归一成字符串。
 */
export function scopeKeyOf(scope: Scope): string {
  switch (scope.kind) {
    case 'all':
      return 'all'
    case 'browse':
      return `browse:${scope.path}`
    case 'none':
      return 'none'
    case 'tag':
      return `tag:${scope.name}`
    case 'place':
      return `place:${scope.name}`
    case 'tagGroup':
      // 组内词条顺序不该影响指纹（同一个组，换个顺序还是同一批图）
      return `tagGroup:${scope.label}:${[...scope.names].sort().join('|')}`
    default:
      return `${scope.kind}:${scope.id}`
  }
}

/**
 * 「基础范围」的指纹（剥掉标签筛选那一层之后的 图库/目录/浏览路径）。
 *
 * 标签条用它判断「范围是不是换了」——换了才退回一级列表。
 * ⛔ 不要用标签列表的指纹判断：实测它会被莫名触发（2026-10-24 计数器跑到 6 次），
 * 导致用户点了一级芯片后二级视图被反复弹回一级。
 */
export function baseKeyOf(base: Scope): string {
  switch (base.kind) {
    case 'library':
      return `library:${base.id}`
    case 'folder':
      return `folder:${base.id}`
    case 'browse':
      return `browse:${base.path}`
    default:
      return base.kind
  }
}

/** 组装查询参数所需的全部界面状态。 */
export type BuildQueryInput = {
  scope: Scope
  /** `baseScopeOf(scope, scopeBeforeTag)` 的结果 */
  baseScope: Scope
  formats: string[]
  camera: string[]
  years: number[]
  /** 顶栏「地点」筛选（与侧栏 `scope.kind === 'place'` 互不干扰，这里是附加过滤） */
  placeFilter: string[]
  personId: number | null
  tagFilter: 'has' | 'none' | null
  searchTerm: string
  sort: ManagerSort
  /** 实际方向（含用户在同一列上的翻转），与浏览模式的内存排序同一口径 */
  desc: boolean
  limit?: number
}

/**
 * 「人像」这个一级类目**按检出人脸算**，不看 AI 标签。
 *
 * 用户 2026-11-02：「图库里每张有脸的照片，都应该在人像里」。
 * 背景：CN-CLIP 只给极少数图打上「人像/单人/合照」（他库里 546 张只有 10 张），
 * 而人脸识别检出 168 张 —— 标签条上写着 10、人脸识别说 168，用户只会觉得「漏了」。
 *
 * ⛔ 这个特例在 `crates/pv-index/src/repo/tags.rs::group_counts` 里还有一份，
 *    两处必须同时改，否则会出现「标签条写 168、点进去只有 10 张」。
 */
const FACE_GROUP = '人像'

/** 这一组是不是按「检出人脸」筛（而不是按标签名）。 */
export function isFaceGroup(label: string): boolean {
  return label === FACE_GROUP
}

/**
 * 「单人」「合照」也走人脸口径：单人 = 恰好 1 张脸，合照 = ≥2 张脸。
 *
 * 用户 2026-11-02：「单人和合照的定义，也和人像一样，和普通 tag 不一样，
 * 使得 单人 + 合照 = 人像」。所以三个数字必须来自同一个来源（`faces` 表），
 * 否则又会出现「人像 168、单人 21、合照 5，加起来对不上」。
 *
 * ⚠️ 这两个词在词表里只挂在「人像」下面，所以按名字判不会误伤别的类目。
 */
export const FACE_SINGLE = '单人'
export const FACE_MULTI = '合照'

/** 命中「单人」「合照」时给的是脸数条件，不是标签名。 */
function faceCountOf(label: string): { eq?: number; min?: number } | null {
  if (label === FACE_SINGLE) return { eq: 1 }
  if (label === FACE_MULTI) return { min: 2 }
  return null
}

/**
 * 入库模式的**唯一取数入口**用的查询参数。
 *
 * @returns `null` = 这个范围不该查索引（`browse` 走磁盘列表、`none` 是默认空态）
 */
export function buildManagerQuery(input: BuildQueryInput): ManagerQuery | null {
  const { scope, baseScope } = input
  if (scope.kind === 'browse' || scope.kind === 'none') return null

  const q: ManagerQuery = {
    sort: input.sort,
    desc: input.desc,
    limit: input.limit ?? PAGE_LIMIT,
    offset: 0,
  }

  // ⭐ 只看这个目录**本身**，不含子目录（`dirIds` 是精确匹配）
  if (baseScope.kind === 'folder') q.dirIds = [baseScope.id]
  if (baseScope.kind === 'library') q.libraryId = baseScope.id

  // 标签筛选：后端按**标签名**过滤（`tags_any` = 命中任一即可）
  // ⭐ 如果这个标签是**从某个图库点进去的**，图库范围要一起带上，
  //    否则「rednote → 人像」会查出全库的人像图（2026-10-29 用户报的混合）。
  if (scope.kind === 'tag') {
    /* 「人像」= 检出过人脸；「单人 / 合照」= 按脸数 —— 都不走标签匹配 */
    const fc = faceCountOf(scope.name)
    if (isFaceGroup(scope.name)) q.hasFaces = true
    else if (fc?.eq != null) q.faceCountEq = fc.eq
    else if (fc?.min != null) q.faceCountMin = fc.min
    else q.tagsAny = [scope.name]
    if (scope.libraryId != null) q.libraryId = scope.libraryId
  }
  // 地点筛选：城市名是后端离线解析好的，精确匹配
  if (scope.kind === 'place') q.places = [scope.name]
  if (scope.kind === 'tagGroup') {
    /* ⭐ 「人像」按人脸筛：有脸就算，不看它下挂的「单人/合照」标签 */
    const fc = faceCountOf(scope.label)
    if (isFaceGroup(scope.label)) q.hasFaces = true
    else if (fc?.eq != null) q.faceCountEq = fc.eq
    else if (fc?.min != null) q.faceCountMin = fc.min
    else q.tagsAny = scope.names
    if (scope.libraryId != null) q.libraryId = scope.libraryId
  }

  if (input.formats.length > 0) q.formats = input.formats
  if (input.camera.length > 0) q.camera = input.camera
  if (input.years.length > 0) q.years = input.years
  if (input.placeFilter.length > 0) q.places = input.placeFilter
  if (input.personId != null) q.personId = input.personId

  // ⛔ 不能直接 `if (tagFilter) q.hasTags = tagFilter === 'has'`：
  // 标签条点了「无匹配」再点侧栏某个标签，会拼出「既要有 X 标签、
  // 又要求这张图没有任何标签」—— 恒空集，而且界面上看不出来
  // （标签高亮、计数 32，就是没图）。规则见 `filterRules.ts`。
  const hasTags = resolveHasTags(input.tagFilter, q.tagsAny)
  if (hasTags !== undefined) q.hasTags = hasTags

  if (input.searchTerm) q.nameLike = input.searchTerm

  return q
}
