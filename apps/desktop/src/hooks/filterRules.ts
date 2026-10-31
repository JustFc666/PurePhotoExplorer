/**
 * 检索条件的**互斥规则**（2026-10-31）。
 *
 * 起因：用户点侧栏标签「半身」（明明有 32 张），主区一张都不显示。
 * 后端查得出 32 张（`--example query_tag 半身` 验证过），问题在前端拼条件：
 *
 * - 标签条上的「无匹配」把 `tagFilter` 设成 `'none'` → 拼出 `has_tags = false`
 * - 再点侧栏某个标签 → 又拼出 `tags_any = ['半身']`
 *
 * 两条一起下去就是「既要有『半身』标签，又要求这张图**没有任何标签**」——
 * 恒为空集。而且界面上完全看不出来：标签高亮了、计数也写着 32，
 * 只是图没了。
 *
 * ⛔ 所以**「按标签筛」一旦有值，「有无标签」必须让路**。
 */

/**
 * 算出 `has_tags` 该不该拼。
 *
 * @param tagFilter 标签条的「有标签 / 无标签」开关（`null` = 不限）
 * @param tagsAny 当前是否在按具体标签筛（`tags_any`）
 * @returns `true` / `false` / `undefined`（不拼这个条件）
 */
export function resolveHasTags(
  tagFilter: 'has' | 'none' | null,
  tagsAny: string[] | undefined,
): boolean | undefined {
  // 已经在按具体标签筛了，「无标签」就是个自相矛盾的条件，一律忽略
  if (tagsAny && tagsAny.length > 0) return undefined
  if (tagFilter === 'has') return true
  if (tagFilter === 'none') return false
  return undefined
}
