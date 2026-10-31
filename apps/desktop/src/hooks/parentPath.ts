/**
 * 浏览模式「返回上一级」的父路径推导。
 *
 * 单独抽成纯函数是因为它的边界比看起来多：盘根（`E:\`）、UNC（`\\nas\share`）、
 * 压缩包（`E:\a\b.cbz` 的上一级是 `E:\a` 而不是它自己）、以及混杂的正反斜杠。
 * 写错一个的表现是「点了返回没反应」或「跳到别处」，都很容易被当成偶发。
 */

/** 路径的显示名（最后一段）；盘根用盘符 `E:`。 */
export function baseName(path: string): string {
  const p = path.replace(/[\\/]+$/, '')
  const i = Math.max(p.lastIndexOf('\\'), p.lastIndexOf('/'))
  return i >= 0 ? p.slice(i + 1) : p
}

/**
 * 上一级目录；已经是根（盘根 / UNC 根）返回 `null`。
 *
 * - `E:\a\b`   → `E:\a`
 * - `E:\a`     → `E:\`
 * - `E:\`      → `null`（到顶了，别再往上）
 * - `\\nas\share\a` → `\\nas\share`
 * - `\\nas\share`   → `null`
 */
export function parentPath(path: string): string | null {
  const raw = path.replace(/[\\/]+$/, '')
  if (!raw) return null

  // 盘根：`E:` 或 `E:\` → 没有上一级
  if (/^[a-zA-Z]:$/.test(raw)) return null

  const i = Math.max(raw.lastIndexOf('\\'), raw.lastIndexOf('/'))
  if (i < 0) return null

  const head = raw.slice(0, i)
  // `E:\a` → head 是 `E:` → 补回斜杠变成盘根
  if (/^[a-zA-Z]:$/.test(head)) return `${head}\\`
  // UNC：`\\nas\share` 已经到共享根，再往上就是 `\\nas`，那不是可浏览的东西
  if (/^\\\\[^\\]+$/.test(head)) return null
  if (!head) return null
  return head
}
