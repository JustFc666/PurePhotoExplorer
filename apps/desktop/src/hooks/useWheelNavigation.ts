/**
 * 普通滚轮 → 同目录翻页（**窗口级**监听）。
 *
 * ## 为什么不能挂在画布上
 * 滚轮翻页原先写在 `<ImageCanvas>` 的 `onWheel` 里。但画布**只在有图时才渲染**：
 * 一旦当前文件解码失败，`App` 会改渲染 `<EmptyState>`（错误详情页），
 * 画布及其监听**一起被卸载** —— 于是用户在坏图上滚轮毫无反应，
 * 只能去点工具栏按钮，直接卡死在那里。
 *
 * 翻页是「查看器」这个整体的行为，不该依赖画布在不在。
 * （键盘翻页早就这么处理了：`useShortcuts` 里把翻页放在 `hasImage` 早退**之前**，
 * 见该文件顶部注释。这里是同一个坑的另一半。）
 *
 * ## 与画布上 Ctrl+滚轮的分工
 * 缩放仍留在画布：它以**光标在画布内的坐标**为锚点，而且没图时缩放也没有意义。
 * 所以那边只处理 Ctrl/⌘+滚轮，普通滚轮直接放行冒泡到这里。
 */

import { useEffect, useRef } from 'react'

/**
 * 翻页冷却（毫秒）。
 *
 * 一格滚轮 ≈ 100px 的 delta；触控板一次轻扫会连发几十个事件，
 * 没有冷却会一口气翻掉十几张图。
 */
const WHEEL_NAV_COOLDOWN = 220

/**
 * 内部自己要滚动的浮层 —— 落在它们里面的滚轮不抢。
 *
 * 主要是帮助 / 设置两个模态（内容长、里面是可滚动区域）；抢过来翻页会让用户
 * 看着帮助却翻起了图。
 *
 * ⚠️ 用的是**类名**而非组件引用，所以类名改名时要同步这里（`grep pv-modal`）。
 * 漏一个的症状很隐蔽：面板能打开，但滚轮翻的是图，面板内容滚不动。
 */
const SELF_SCROLLING_OVERLAYS = ['.pv-modal']

export interface UseWheelNavigationOptions {
  /** 滚轮向下 = +1（下一张），向上 = -1（上一张） */
  onNavigate: (direction: 1 | -1) => void
  /** 关掉监听（例如当前目录只有一张图，没得翻） */
  enabled?: boolean
}

export function useWheelNavigation({
  onNavigate,
  enabled = true,
}: UseWheelNavigationOptions): void {
  // 回调放 ref：调用方每次渲染都会传新函数，直接进依赖会让监听反复解绑/重挂
  const navRef = useRef(onNavigate)
  navRef.current = onNavigate
  const lastNavAt = useRef(0)

  useEffect(() => {
    if (!enabled) return

    const onWheel = (e: WheelEvent) => {
      // Ctrl/⌘ + 滚轮是缩放，交给画布那张自己的监听
      if (e.ctrlKey || e.metaKey) return
      // 浮层内部要自己滚（帮助浮层内容长）
      // 先把 target 取出来：在 `.some()` 的回调里 `instanceof` 的收窄会失效
      const target = e.target
      if (target instanceof Element && SELF_SCROLLING_OVERLAYS.some((s) => target.closest(s))) {
        return
      }
      // 触控板的水平滚动不带 deltaY，别当成翻页
      if (Math.abs(e.deltaY) < 1) return

      const now = performance.now()
      if (now - lastNavAt.current < WHEEL_NAV_COOLDOWN) return
      lastNavAt.current = now

      navRef.current(e.deltaY > 0 ? 1 : -1)
    }

    // passive：这里不需要 preventDefault（页面本身不可滚动），
    // 声明成 passive 可以让浏览器不必等回调就能滚动，滚得更跟手
    window.addEventListener('wheel', onWheel, { passive: true })
    return () => window.removeEventListener('wheel', onWheel)
  }, [enabled])
}
