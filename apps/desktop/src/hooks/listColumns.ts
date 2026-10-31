/**
 * 列表视图的**列定义**与**偏好持久化**（2026-10-31）。
 *
 * ## 为什么单独一个模块
 * 三处都要用同一份定义，写在组件里必然漂移：
 * - 列表表头（列名、顺序、默认宽度）
 * - 行渲染（每个单元格显示什么）
 * - 设置窗口（哪些列能关、关掉后剩什么）
 *
 * 偏好存 localStorage 而不是后端 SQLite：这是**纯显示偏好**，
 * 跟图库数据无关，也不需要在多端同步。跟 `sort` / `slide` 那些 pref 同一套路。
 */

import { useCallback, useEffect, useMemo, useState } from 'react'

import type { ManagerSort } from '../ipc/manager'

/** 列 id。新增列时**只能往后加**（localStorage 里存的是 id，删了会丢配置）。 */
export type ListColumnId =
  | 'name'
  | 'format'
  | 'size'
  | 'mtime'
  | 'takenAt'
  | 'dims'
  | 'flag'
  | 'rating'

export interface ListColumnDef {
  id: ListColumnId
  label: string
  /** 默认宽度（px） */
  width: number
  /** 拖动时的下限 —— 再窄就只剩省略号，没有意义 */
  min: number
  align: 'left' | 'right' | 'center'
  /**
   * 设置里**不允许关掉**。
   *
   * 用户 2026-10-31：「格式、大小是必须的」。文件名同理 ——
   * 一个没有名字的列表没有意义。
   */
  locked?: boolean
  /** 表头的悬停说明（为什么这一列有时候是空的） */
  hint?: string
  /**
   * 点这一列的表头时按什么排。
   *
   * ⚠️ 目前每列都有；留 `null` 的口子是给「不属于数据、只做展示」的未来列用
   * （比如缩略图预览）—— 那种列不该有三角。
   */
  sortKey: ManagerSort
}

/**
 * 全部列，**顺序即默认显示顺序**。
 *
 * 宽度按「内容常见长度」定：文件名最长、格式最短。
 * 合计 1046px（1200 宽的主窗口去掉侧栏还剩 ~980，会横向滚动）——
 * 但用户本来就会按需关掉几列，默认给出完整信息比默认藏一半更好。
 */
export const LIST_COLUMNS: ListColumnDef[] = [
  {
    id: 'name',
    label: '文件名',
    width: 340,
    min: 120,
    align: 'left',
    locked: true,
    sortKey: 'name',
  },
  {
    id: 'format',
    label: '格式',
    width: 72,
    min: 48,
    align: 'left',
    locked: true,
    hint: '按文件内容判定，与扩展名无关',
    sortKey: 'format',
  },
  { id: 'size', label: '大小', width: 92, min: 60, align: 'right', locked: true, sortKey: 'size' },
  { id: 'mtime', label: '修改日期', width: 148, min: 108, align: 'left', sortKey: 'mtime' },
  {
    id: 'takenAt',
    label: '拍摄日期',
    width: 148,
    min: 108,
    align: 'left',
    hint: '来自 EXIF；浏览模式下要等后台读完才有',
    sortKey: 'taken',
  },
  { id: 'dims', label: '分辨率', width: 116, min: 84, align: 'left', sortKey: 'dims' },
  { id: 'flag', label: '标记', width: 56, min: 44, align: 'center', sortKey: 'flag' },
  { id: 'rating', label: '分级', width: 84, min: 56, align: 'left', sortKey: 'rating' },
]

export const COLUMN_BY_ID: Record<ListColumnId, ListColumnDef> = Object.fromEntries(
  LIST_COLUMNS.map((c) => [c.id, c]),
) as Record<ListColumnId, ListColumnDef>

// ── 持久化 ─────────────────────────────────────────────────────

/** 存**隐藏**的列。存「显示列表」的话，以后新增列默认就是隐藏的。 */
const HIDDEN_KEY = 'pv.list.hidden'
const WIDTH_KEY = 'pv.list.widths'

function readJSON<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(key)
    return raw ? (JSON.parse(raw) as T) : fallback
  } catch {
    return fallback
  }
}

export function readHiddenColumns(): ListColumnId[] {
  const v = readJSON<unknown>(HIDDEN_KEY, [])
  if (!Array.isArray(v)) return []
  // 过滤掉已经不存在的 id（老版本存下的），否则设置页会渲染出幽灵项
  return v.filter((x): x is ListColumnId => typeof x === 'string' && x in COLUMN_BY_ID)
}

export function writeHiddenColumns(ids: ListColumnId[]): void {
  try {
    localStorage.setItem(HIDDEN_KEY, JSON.stringify(ids))
  } catch {
    /* 隐私模式写不进去就算了，当次仍然生效 */
  }
  emit()
}

export function readColumnWidths(): Partial<Record<ListColumnId, number>> {
  const v = readJSON<Record<string, number>>(WIDTH_KEY, {})
  const out: Partial<Record<ListColumnId, number>> = {}
  for (const [k, w] of Object.entries(v ?? {})) {
    const id = k as ListColumnId
    if (!(id in COLUMN_BY_ID) || typeof w !== 'number' || !Number.isFinite(w)) continue
    out[id] = clampWidth(id, w)
  }
  return out
}

export function writeColumnWidths(w: Partial<Record<ListColumnId, number>>): void {
  try {
    localStorage.setItem(WIDTH_KEY, JSON.stringify(w))
  } catch {
    /* 同上 */
  }
  emit()
}

/** 夹到 `[min, 640]`。上限是防手滑拖出几万像素把其它列挤没。 */
export function clampWidth(id: ListColumnId, w: number): number {
  const min = COLUMN_BY_ID[id]?.min ?? 48
  return Math.max(min, Math.min(640, Math.round(w)))
}

// ── 跨组件通知（设置窗口改了，列表要立刻跟着变）─────────────────────
//
// ⚠️ 不能用 `storage` 事件：它只在**其它窗口**改时触发，同一个窗口里
// localStorage.setItem 不会派发。所以自己维护一份订阅表。

const listeners = new Set<() => void>()

export function subscribeListPrefs(fn: () => void): () => void {
  listeners.add(fn)
  return () => {
    listeners.delete(fn)
  }
}

function emit(): void {
  for (const fn of listeners) {
    try {
      fn()
    } catch {
      /* 单个订阅者出错不能影响别人 */
    }
  }
}

// ── 单元格取值 ─────────────────────────────────────────────────

/** `2026-10-31 09:06`；没有值给 `—`（列表里不能留空白，看着像漏了一行）。 */
export function formatDateTime(sec: number | null | undefined): string {
  if (sec == null || !Number.isFinite(sec) || sec <= 0) return '—'
  const d = new Date(sec * 1000)
  if (Number.isNaN(d.getTime())) return '—'
  const p = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`
}

/** 文件修改时间是**纳秒**（后端 `mtime_ns`），换算成秒再格式化。 */
export function formatMtime(mtimeNs: number | null | undefined): string {
  if (mtimeNs == null || !Number.isFinite(mtimeNs) || mtimeNs <= 0) return '—'
  return formatDateTime(mtimeNs / 1e9)
}

/** 星级：`★★★☆☆` 太吵，列表里只用实心星 + 数量；0 星给 `—`。 */
export function formatRating(rating: number): string {
  if (!rating || rating <= 0) return '—'
  return '★'.repeat(Math.min(5, rating))
}

// ── React 侧：读偏好 + 拖动时改宽度 ─────────────────────────────

export interface ListPrefsApi {
  /** 当前要显示的列（已按 `LIST_COLUMNS` 顺序排好，隐藏的已剔除） */
  visible: ListColumnDef[]
  /** 某列当前宽度（用户拖过就用用户的，否则默认） */
  widthOf: (id: ListColumnId) => number
  /** 拖动过程中只改内存（要跟手），松手才落盘 */
  setWidthLocal: (id: ListColumnId, w: number) => void
  /** 松手时调用：把当前宽度写进 localStorage */
  commitWidths: () => void
  /** 双击表头分隔线：恢复这一列的默认宽度 */
  resetWidth: (id: ListColumnId) => void
  /** 网格列模板（含最前面的勾选列） */
  gridTemplate: string
}

/** 勾选列宽度：和行内 `.pv-mg-listcheck` 一致 */
export const CHECK_COL_WIDTH = 26

export function useListPrefs(): ListPrefsApi {
  const [hidden, setHidden] = useState<ListColumnId[]>(readHiddenColumns)
  const [widths, setWidths] = useState<Partial<Record<ListColumnId, number>>>(readColumnWidths)

  // 设置窗口改了列显隐 → 列表要立刻跟着变（同窗口内 localStorage 不派发 storage 事件）
  useEffect(
    () =>
      subscribeListPrefs(() => {
        setHidden(readHiddenColumns())
        setWidths(readColumnWidths())
      }),
    [],
  )

  const visible = useMemo(
    () =>
      // `locked` 的列无条件显示：就算 localStorage 里被误写成隐藏也照显，
      // 否则「文件名/格式/大小」消失会让人以为列表坏了。
      LIST_COLUMNS.filter((c) => c.locked || !hidden.includes(c.id)),
    [hidden],
  )

  const widthOf = useCallback(
    (id: ListColumnId) => widths[id] ?? COLUMN_BY_ID[id]?.width ?? 100,
    [widths],
  )

  const setWidthLocal = useCallback((id: ListColumnId, w: number) => {
    setWidths((prev) => ({ ...prev, [id]: clampWidth(id, w) }))
  }, [])

  // 用 ref 拿最新值，避免 `commitWidths` 每次拖都换引用
  const widthsRef = useMemo(() => widths, [widths])
  const commitWidths = useCallback(() => {
    writeColumnWidths(widthsRef)
  }, [widthsRef])

  const resetWidth = useCallback((id: ListColumnId) => {
    setWidths((prev) => {
      const next = { ...prev }
      delete next[id]
      writeColumnWidths(next)
      return next
    })
  }, [])

  const gridTemplate = useMemo(
    () => `${CHECK_COL_WIDTH}px ` + visible.map((c) => `${widthOf(c.id)}px`).join(' '),
    [visible, widthOf],
  )

  return { visible, widthOf, setWidthLocal, commitWidths, resetWidth, gridTemplate }
}
