/**
 * 主题偏好（2026-11-03）。
 *
 * ## 为什么存 localStorage 而不是索引库
 * 这是**本机显示偏好**，与图库数据无关 —— 换个盘、换台机器不该跟着搬。
 * 更要紧的是启动时序：存索引库就得等 Rust 侧 `Store` 打开才能读到，
 * 首屏会先按默认渲染一帧再跳色（白闪/黑闪），而 localStorage 在
 * `index.html` 的内联脚本里就能读到，第一帧就是对的。
 *
 * ## 谁是唯一真相
 * CSS 只认 `<html data-theme="light|dark">`；`system` 是偏好而不是主题，
 * 落地时会被解析成 light/dark 再写进属性（`color-scheme` 也跟着走）。
 */

export type ThemePref = 'system' | 'light' | 'dark'

const KEY = 'pv.theme'

export function readThemePref(): ThemePref {
  try {
    const v = localStorage.getItem(KEY)
    if (v === 'light' || v === 'dark' || v === 'system') return v
  } catch {
    /* 隐私模式 / 存储被禁用时读写会抛，按默认走 */
  }
  return 'system'
}

function systemPrefersDark(): boolean {
  return (
    typeof window !== 'undefined' &&
    typeof window.matchMedia === 'function' &&
    window.matchMedia('(prefers-color-scheme: dark)').matches
  )
}

/** 把偏好落到 `<html data-theme>` 上。 */
export function applyTheme(pref: ThemePref = readThemePref()): void {
  const dark = pref === 'dark' || (pref === 'system' && systemPrefersDark())
  document.documentElement.dataset.theme = dark ? 'dark' : 'light'
}

export function setThemePref(pref: ThemePref): void {
  try {
    localStorage.setItem(KEY, pref)
  } catch {
    /* 存不下也要让当前这次生效 */
  }
  applyTheme(pref)
}

/**
 * 「跟随系统」时响应当前系统的深浅切换。
 *
 * @returns 取消函数（组件卸载时调）
 */
export function watchSystemTheme(): () => void {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return () => {}
  const mq = window.matchMedia('(prefers-color-scheme: dark)')
  const onChange = () => {
    if (readThemePref() === 'system') applyTheme('system')
  }
  mq.addEventListener('change', onChange)
  return () => mq.removeEventListener('change', onChange)
}
