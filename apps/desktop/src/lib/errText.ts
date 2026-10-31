/**
 * 把「未知异常」变成一句能给用户看的话。
 *
 * ## 为什么必须统一走这里
 *
 * Tauri 命令失败时，`invoke()` 的 rejection 值是 **Rust 端序列化后的 `ErrorDto`**，
 * 也就是一个**普通对象** `{ code, message, user_visible }` —— 它**不是** `Error` 实例。
 *
 * 于是这几种种写法都会翻车：
 *
 * ```ts
 * // ❌ 后端错误走到 String(e) → 界面上是 "[object Object]"
 * e instanceof Error ? e.message : String(e)
 *
 * // ❌ 同样拿不到 message
 * String(e)
 * ```
 *
 * 尤其是 AI 打标这类「失败是常态」的功能（本机没装 Python 就直接报错），
 * 弹窗里只显示 `[object Object]`，用户完全不知道该怎么办。
 *
 * ## 用法
 * 所有 `catch` 一律 `setErr(errText(e))` / `detail: errText(e)`，⛔ 别各写各的。
 */

export function errText(e: unknown): string {
  if (typeof e === 'string') return e
  if (e && typeof e === 'object') {
    // 后端 ErrorDto 的形态：{ code, message, user_visible }
    const m = (e as { message?: unknown }).message
    if (typeof m === 'string' && m) return m
    // 真正的 JS Error（前端自己 throw 的）
    if (e instanceof Error && e.message) return e.message
  }
  return String(e)
}
