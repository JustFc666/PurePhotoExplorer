/**
 * 换范围 / 换图库时，**哪些筛选要跟着清掉**。
 *
 * ## 为什么单独抽出来
 * 2026-11-01 用户报：在一个图库点过人像头像（正常浏览），再点侧栏另一个图库，
 * 主区一张图都没有，标题还写着「未命名」。
 *
 * 原因：`personFilter` 是**叠在 `scope` 上的额外条件**，不在 `scope` 里，
 * 所以 `setScope` 一换，条件就变成了「新图库 + 那个人」——
 * 那个人没在新库里拍过，结果恒为 0；而标题读的是 `personFilter.name`，
 * 于是整个图库看起来像坏了。
 *
 * 把它做成纯函数，是为了能用单测锁住「加新筛选时必须顺手清」——
 * 靠人记注释是记不住的（这个坑就是这么来的）。
 */

/** 会被「换范围」清掉的筛选维度。加新筛选时**必须**在这里补一个字段。 */
export interface ScopeBoundFilters {
  /** 人物筛选（点人像头像进入） */
  person: boolean
  /** 标签筛选（侧栏某个 tag） */
  tag: boolean
  /** 地点筛选（侧栏某个地点） */
  place: boolean
  /** 搜索词 */
  search: boolean
}

/** 全部清空 —— 换范围时的默认行为。 */
export const CLEARED_FILTERS: ScopeBoundFilters = {
  person: true,
  tag: true,
  place: true,
  search: true,
}

/**
 * 换范围时实际要清哪些。
 *
 * ⭐ 目前**一律全清**：任何叠加条件都绑定在原来的范围上，
 * 换了范围还留着，用户看到的就是「明明这个库有 500 张，却显示 0 张」。
 *
 * 之所以还留成函数而不是直接返回常量：将来若要支持「点侧栏标签 → 再点图库
 * 保持标签」，只要在这里按 `from`/`to` 判断即可，调用方不用动。
 */
export function filtersToClearOnScopeChange(): ScopeBoundFilters {
  return CLEARED_FILTERS
}

/**
 * 空态文案里要写出的「当前生效条件」。
 *
 * ⭐ 只在**真的有值**时才列：全空的数组交给调用方走原来的通用文案，
 * 免得写出一句「没有同时满足 的照片」这种半截话。
 */
export function describeActiveFilters(f: {
  personName?: string | null
  tagFilter?: 'has' | 'none' | null
  places?: readonly string[]
  search?: string | null
}): string[] {
  const out: string[] = []
  if (f.personName) out.push(`人物「${f.personName}」`)
  if (f.tagFilter === 'has') out.push('有标签')
  else if (f.tagFilter === 'none') out.push('无标签')
  const places = f.places ?? []
  if (places.length) out.push(`地点「${places.join('、')}」`)
  const s = (f.search ?? '').trim()
  if (s) out.push(`搜索「${s}」`)
  return out
}
