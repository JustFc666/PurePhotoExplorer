/**
 * 图库模式的**用户偏好**（localStorage 读写 + 跨窗口通知）。
 *
 * 全部是「读一次、写一次」的纯函数，不依赖 React，也不碰后端 ——
 * 所以设置窗口（`SettingsWindow`）能直接用，不必把 hook 拉起来。
 *
 * ⛔ 存进去的值**必须能校验**：`localStorage` 里的历史值可能是旧版本写的
 * （排序键改过名、档位加过新项），读到不认识的就回落到默认值，⛔ 别 `as` 硬转。
 */

import type { ManagerSort, ThumbTier } from '../../ipc/manager'

/** 网格排序的全部取值（设置面板的「默认排序」也用它校验 localStorage 里的历史值）。 */
export const MANAGER_SORTS = [
  'taken',
  'name',
  'size',
  'mtime',
  'format',
  'rating',
  'dims',
  'flag',
  'added',
  'random',
] as const

/** 默认排序的持久化键 */
const SORT_KEY = 'pv.mg.sort'

/**
 * 「设置面板改了默认排序」的事件名。
 *
 * 为什么不用 React 状态往上提：`useManager` 在 `ManagerView` 里，而设置面板在
 * `App` 里 —— 中间隔着好几层组件。一个 window 事件比一路 prop 回调干净，
 * 而且 `ManagerView` 是常驻挂载的，事件是唯一能让它**立刻**反应过来的办法。
 */
export const EVENT_SET_SORT = 'pv:set-sort'

/** 读「默认排序」偏好（没存过 / 存的值不认识 → 拍摄时间）。 */
export function readSortPref(): ManagerSort {
  try {
    const v = localStorage.getItem(SORT_KEY)
    if (v && (MANAGER_SORTS as readonly string[]).includes(v)) return v as ManagerSort
  } catch {
    /* localStorage 不可用 → 用默认值 */
  }
  return 'taken'
}

/** 只写偏好，不通知（图库自己的 `setSort` 用 —— 它就在监听端，别再自激一次）。 */
export function writeSortPref(s: ManagerSort): void {
  try {
    localStorage.setItem(SORT_KEY, s)
  } catch {
    /* 忽略：只影响下次启动的默认值 */
  }
}

/** 写偏好**并**通知常驻挂载的图库视图立刻跟着变（设置面板用这个）。 */
export function applySortPref(s: ManagerSort): void {
  writeSortPref(s)
  window.dispatchEvent(new CustomEvent(EVENT_SET_SORT, { detail: s }))
}

/* ── 缩略图档位偏好（2026-10-24 新增：以前只在内存里，重启就回默认）─────
 * 档位与像素值的对应关系来自 `pv_index::ThumbTier`，这里只做展示与记忆。
 * 改完不需要事件通知：设置窗口只管写，图库视图下次进入时读它。 */
const TIER_KEY = 'pv.mg.tier'

export const THUMB_TIERS: Array<{ value: ThumbTier; label: string; px: number }> = [
  { value: 'sm', label: '小', px: 116 },
  { value: 'md', label: '中', px: 168 },
  { value: 'lg', label: '大', px: 236 },
  { value: 'xl', label: '特大', px: 320 },
]

const TIER_VALUES = THUMB_TIERS.map((t) => t.value)

/** 读缩略图档位偏好（没存过 / 值不认识 → 中）。 */
export function readThumbTier(): ThumbTier {
  try {
    const v = localStorage.getItem(TIER_KEY)
    if (v && (TIER_VALUES as string[]).includes(v)) return v as ThumbTier
  } catch {
    /* localStorage 不可用 → 默认值 */
  }
  return 'md'
}

/** 记住缩略图档位偏好。 */
export function saveThumbTier(t: ThumbTier): void {
  try {
    localStorage.setItem(TIER_KEY, t)
  } catch {
    /* 忽略：只影响下次启动 */
  }
}
