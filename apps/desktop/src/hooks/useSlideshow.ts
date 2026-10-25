/**
 * 幻灯片放映 —— 定时自动翻页。
 *
 * ## 为什么自己抢键盘
 * 放映时 `useShortcuts` 被整个关掉（`enabled: mode === 'viewer' && !slideshow`）。
 * 理由是两边的键位直接冲突：空格在查看器里是「下一张」，在放映里是「暂停」；
 * Esc 在查看器里是一串「关浮层 → 退全屏 → 适应」的多级回退，在放映里只该
 * 「退出放映」。让两套逻辑同时活着，就得在里面塞一堆 `if (slideshow)` ——
 * 关掉一套、这里独立处理反而更清楚。
 *
 * ⚠️ 监听必须用**捕获阶段**：万一哪天 `useShortcuts` 又开着，
 * 也要保证放映先拿到按键。
 *
 * ## 计时为什么要一个 `tick`
 * 计时器的依赖里有「当前图」。翻到下一张时图变了 → effect 重跑 → 重新计时，
 * 这是对的。但**到达最后一张再回到第一张**时，两张图路径不同，同样会重跑，
 * 所以 `tick` 不是必需的……
 * 真正需要它的场景是「调用方这次推进什么都没变」（只有一张图 / 索引未知）：
 * 没有 `tick`，effect 不会重跑，计时就永远停了，表现为「放映卡住」。
 * 加一个自增计数，保证每次推进后都会重新挂一个计时器。
 */

import { useCallback, useEffect, useRef, useState } from 'react'

/** 幻灯片间隔（秒）的持久化键 */
const SECONDS_KEY = 'pv.slide.seconds'
/**
 * 默认间隔：**1 秒**（用户 2026-10-19 指定）。
 *
 * 换照片快一点才像在"放幻灯片"；想看清楚就去设置面板调，
 * 范围 0.5–10 秒、步长 0.5（见 `SLIDE_MIN` / `SLIDE_MAX` / `SLIDE_STEP`）。
 */
const DEFAULT_SECONDS = 1

/** 间隔的可调范围与步长（设置面板与持久化校验共用这一份，别写两遍） */
export const SLIDE_MIN = 0.5
export const SLIDE_MAX = 10
export const SLIDE_STEP = 0.5

/** 把任意数字收进合法区间并对齐到 0.5 的整数倍（0.30000000000000004 这类浮点尾巴也削掉） */
export function clampSlideSeconds(sec: number): number {
  const v = Math.min(SLIDE_MAX, Math.max(SLIDE_MIN, sec))
  return Math.round(v / SLIDE_STEP) * SLIDE_STEP
}

export function readSlideSeconds(): number {
  try {
    const raw = Number(localStorage.getItem(SECONDS_KEY))
    if (Number.isFinite(raw)) return clampSlideSeconds(raw)
  } catch {
    /* localStorage 不可用 → 用默认值 */
  }
  return DEFAULT_SECONDS
}

export function saveSlideSeconds(sec: number): void {
  try {
    localStorage.setItem(SECONDS_KEY, String(clampSlideSeconds(sec)))
  } catch {
    /* 忽略：设置只影响下次启动的默认值 */
  }
}

/** 「设置面板改了放映间隔」的事件名（App 里的 `slideSeconds` 靠它更新） */
export const EVENT_SET_SLIDE_SECONDS = 'pv:set-slide-seconds'

/** 放映切换特效 */
export type SlideFx = 'none' | 'fade' | 'slide'

const FX_KEY = 'pv.slide.fx'
/** 「设置面板改了放映特效」的事件名 */
export const EVENT_SET_SLIDE_FX = 'pv:set-slide-fx'

/** 设置面板里显示的名字（顺序即展示顺序） */
export const SLIDE_FX_OPTIONS: Array<{ value: SlideFx; label: string }> = [
  { value: 'none', label: '无特效' },
  { value: 'fade', label: '淡入淡出' },
  { value: 'slide', label: '滑动' },
]

export function readSlideFx(): SlideFx {
  try {
    const v = localStorage.getItem(FX_KEY)
    if (v === 'none' || v === 'fade' || v === 'slide') return v
  } catch {
    /* localStorage 不可用 → 用默认值 */
  }
  return 'fade'
}

/** 记住放映特效，并通知正在运行的 App 立刻切换 */
export function applySlideFx(fx: SlideFx): void {
  try {
    localStorage.setItem(FX_KEY, fx)
  } catch {
    /* 忽略 */
  }
  window.dispatchEvent(new CustomEvent(EVENT_SET_SLIDE_FX, { detail: fx }))
}

/* ── 特效时长（方案 A：两种特效共用一个值） ──────────────────────
 * 100–2000ms、步长 50。默认 300ms：比原来淡入的 260 稍慢一点，
 * 但比滑动的 340 收一点，两种特效共用一个值时不至于哪边都不对。
 * 上限 2 秒是用户 2026-10-19 指定的（想要"慢镜头"式过渡）。 */
const FX_MS_KEY = 'pv.slide.fxms'
export const FX_MS_MIN = 100
export const FX_MS_MAX = 2000
export const FX_MS_STEP = 50
const DEFAULT_FX_MS = 300

/** 「设置面板改了特效时长」的事件名 */
export const EVENT_SET_SLIDE_FX_MS = 'pv:set-slide-fx-ms'

export function clampFxMs(ms: number): number {
  const v = Math.min(FX_MS_MAX, Math.max(FX_MS_MIN, ms))
  return Math.round(v / FX_MS_STEP) * FX_MS_STEP
}

export function readFxMs(): number {
  try {
    const raw = Number(localStorage.getItem(FX_MS_KEY))
    if (Number.isFinite(raw) && raw > 0) return clampFxMs(raw)
  } catch {
    /* localStorage 不可用 → 用默认值 */
  }
  return DEFAULT_FX_MS
}

/** 记住特效时长，并通知正在运行的 App 立刻生效 */
export function applyFxMs(ms: number): void {
  const v = clampFxMs(ms)
  try {
    localStorage.setItem(FX_MS_KEY, String(v))
  } catch {
    /* 忽略 */
  }
  window.dispatchEvent(new CustomEvent(EVENT_SET_SLIDE_FX_MS, { detail: v }))
}

/**
 * 改放映间隔：持久化 + 通知正在运行的 App。
 *
 * 间隔是 `useState` 的初值，只在挂载时读一次 —— 不通知的话，
 * 用户在设置里改了「每 8 秒」，当前这次运行还是 4 秒，得重启才生效。
 */
export function applySlideSeconds(sec: number): void {
  saveSlideSeconds(sec)
  window.dispatchEvent(new CustomEvent(EVENT_SET_SLIDE_SECONDS, { detail: sec }))
}

export interface UseSlideshowOptions {
  /** 是否正在放映 */
  active: boolean
  /** 每张停留时间（毫秒） */
  intervalMs: number
  /** 当前图的标识（换图 → 重新计时）。一般传图片路径 */
  imageKey: string | null
  /** 有图才走计时（空状态/解码失败时不该空转） */
  hasImage: boolean
  /** 推进到下一张。**末尾回到第一张由调用方负责**（这里不知道图片列表） */
  onAdvance: () => void
  /** 手动后退一张（←/PageUp）。不给就不处理该按键 */
  onBack?: () => void
  /** 用户按 Esc 请求退出 */
  onExit: () => void
}

export interface UseSlideshow {
  paused: boolean
  togglePause: () => void
}

export function useSlideshow({
  active,
  intervalMs,
  imageKey,
  hasImage,
  onAdvance,
  onBack,
  onExit,
}: UseSlideshowOptions): UseSlideshow {
  const [paused, setPaused] = useState(false)
  /** 每次推进 +1，保证「推进了但图没变」也能重新计时 */
  const [tick, setTick] = useState(0)

  /* 回调放 ref：换一份回调不该重置计时器（父组件每次渲染都会给新函数） */
  const advance = useRef(onAdvance)
  advance.current = onAdvance
  const exit = useRef(onExit)
  exit.current = onExit
  const back = useRef(onBack)
  back.current = onBack

  /** 每次开始放映都从「播放中」起（上一轮退出时可能停在暂停态） */
  useEffect(() => {
    if (active) setPaused(false)
  }, [active])

  useEffect(() => {
    if (!active || paused || !hasImage) return
    const t = window.setTimeout(() => {
      advance.current()
      setTick((v) => v + 1)
    }, intervalMs)
    return () => window.clearTimeout(t)
  }, [active, paused, hasImage, intervalMs, imageKey, tick])

  useEffect(() => {
    if (!active) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault()
        e.stopPropagation()
        exit.current()
        return
      }
      if (e.key === ' ') {
        // 空格 = 暂停/继续；别让它冒泡去触发「下一张」
        e.preventDefault()
        e.stopPropagation()
        setPaused((p) => !p)
        return
      }
      if (e.key === 'ArrowRight' || e.key === 'PageDown') {
        e.preventDefault()
        e.stopPropagation()
        advance.current()
        setTick((v) => v + 1)
        return
      }
      if (e.key === 'ArrowLeft' || e.key === 'PageUp') {
        e.preventDefault()
        e.stopPropagation()
        back.current?.()
        setTick((v) => v + 1)
        return
      }
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [active])

  const togglePause = useCallback(() => setPaused((p) => !p), [])

  return { paused, togglePause }
}
