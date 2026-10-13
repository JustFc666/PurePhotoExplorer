/**
 * 键盘快捷键 —— 看图软件的效率关键。
 *
 * ## 键位约定（2026-10-07 定稿）
 *
 * | 键 | 功能 |
 * |---|---|
 * | 空格 / ArrowRight / PageDown | 下一张 |
 * | ArrowLeft / PageUp / Shift+空格 | 上一张 |
 * | （滚轮上下） | 同目录翻页 |
 * | I / F / M / A | 信息浮层 / 全屏 / 最大化 / 调整浮层 |
 * | Ctrl+O / Ctrl+P | 打开 / **打印预览**（确认版面后才送印）|
 * | F5 | 刷新（重读磁盘上的当前图片）|
 * | H / F1 | 帮助（快捷键速查 + 关于） |
 * | Esc | 关右键菜单 → 关打印预览 → 关帮助 → 取消裁剪 → 退出全屏 → 适应窗口 |
 * | + − 0 1 [ ] | 缩放 / 适应 / 原大小 / 旋转 |
 * | ArrowUp / ArrowDown | 竖直平移 |
 *
 * **方向键的左右已被翻页占用** —— 看图场景下「翻下一张」的频率远高于
 * 「左右平移」，而放大后平移可以用鼠标拖拽（`useDragDrop`）。
 *
 * ## 翻页必须早于 `hasImage` 判定
 *
 * 读不出来的文件（损坏 / 格式不支持 / 空 / Git LFS 指针）也要能翻走，
 * 否则用户卡在坏图上只能用鼠标点工具栏。所以翻页分支放在
 * `if (!hasImage) return` **之前** —— 别把它挪到下面。
 */

import { useEffect } from 'react'

interface ShortcutsProps {
  onOpen: () => void
  /**
   * 打印（Ctrl+P）。
   *
   * ⚠️ 2026-10-10 起**只是打开打印预览**，不直接送印 ——
   * 真正送印由预览浮层的「打印…」触发。回调名保持 `onPrint` 不变，
   * 但语义已变成「请求打印」。
   */
  onPrint: () => void
  onAction: (action: string) => void
  hasImage: boolean
  /** 下一张（空格 / ArrowRight / PageDown） */
  onNext: () => void
  /** 上一张（ArrowLeft / PageUp） */
  onPrev: () => void
  /** 全屏切换（F） */
  onToggleFullscreen: () => void
  /** 最大化 / 还原（M） */
  onToggleMaximize: () => void
  /** 图片信息浮层（I） */
  onToggleInfo: () => void
  /** 画布调整浮层（A） */
  onToggleAdjust: () => void
  /** 帮助浮层（H / F1） */
  onToggleHelp: () => void
  /**
   * 刷新当前图片（F5）—— 失效缓存 + 重读磁盘。
   *
   * 复用 App 的 `handleRefresh`：它内部已对「没有图」做了早退，
   * 所以这里不必再判 `hasImage`。
   */
  onRefresh: () => void
  /**
   * **`Esc` 的第一优先级处理** —— 关浮层 / 取消裁剪，返回 `true` 表示已消费。
   *
   * 顺序要求：浮层（帮助/信息/调整）→ 裁剪框 → 全屏 → 适应窗口。
   * 没有可处理的东西时**必须返回 `false`**，让快捷键层继续往下走，
   * 否则 `Esc` 会变成哑键。
   */
  onEscape: () => boolean
  /** 当前是否全屏（决定 Esc 是退出全屏还是适应窗口） */
  isFullscreen: boolean
}

export function useShortcuts({
  onOpen,
  onPrint,
  onAction,
  hasImage,
  onNext,
  onPrev,
  onToggleFullscreen,
  onToggleMaximize,
  onToggleInfo,
  onToggleAdjust,
  onToggleHelp,
  onEscape,
  isFullscreen,
  onRefresh,
}: ShortcutsProps) {
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      // 输入框内不拦截
      const target = e.target as HTMLElement | null
      if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA')) {
        return
      }

      // Ctrl+O 打开 —— 无论有没有图都生效
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'o') {
        e.preventDefault()
        onOpen()
        return
      }

      // Ctrl+P 打印 —— 系统的标准快捷键，用户会下意识去按。
      // 没有图时直接放过（让浏览器/WebView 的默认行为也别做，所以仍然 preventDefault）
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'p') {
        e.preventDefault()
        onPrint()
        return
      }

      // F 全屏切换（不带修饰键，避免与 Ctrl+F 冲突）
      if (!e.ctrlKey && !e.metaKey && e.key.toLowerCase() === 'f') {
        e.preventDefault()
        onToggleFullscreen()
        return
      }

      // M 最大化 / 还原（不带修饰键）
      if (!e.ctrlKey && !e.metaKey && e.key.toLowerCase() === 'm') {
        e.preventDefault()
        onToggleMaximize()
        return
      }

      // I 图片信息浮层（不带修饰键）
      if (!e.ctrlKey && !e.metaKey && e.key.toLowerCase() === 'i') {
        e.preventDefault()
        onToggleInfo()
        return
      }

      // A 画布调整浮层（不带修饰键）
      if (!e.ctrlKey && !e.metaKey && e.key.toLowerCase() === 'a') {
        e.preventDefault()
        onToggleAdjust()
        return
      }

      // H / F1 帮助浮层（H 不带修饰键，避免吃掉 Ctrl+H；F1 是系统惯例）
      if (
        (e.key === 'F1' || (!e.ctrlKey && !e.metaKey && e.key.toLowerCase() === 'h'))
      ) {
        e.preventDefault()
        onToggleHelp()
        return
      }

      // F5 刷新当前图片（重读磁盘）—— 文件被外部改过 / 坏文件修复后即时生效。
      // 与浏览器刷新惯例一致；preventDefault 阻止 WebView 可能的默认行为。
      if (e.key === 'F5') {
        e.preventDefault()
        onRefresh()
        return
      }

      // ── Esc 的优先级（2026-10-08 更新）──
      // ① 浮层打开中 → 关掉浮层 / 裁剪框已定 → 取消裁剪
      // ② 全屏 → 退出全屏
      // ③ 其余情况落到下面的 switch：适应窗口
      // 必须把 ① 放在最前，否则用户在「框选放大」里按 Esc 会先去动全屏，
      // 画面反而更乱，而他真正想要的是「取消这次裁剪」。
      if (e.key === 'Escape' && onEscape()) {
        e.preventDefault()
        return
      }

      if (e.key === 'Escape' && isFullscreen) {
        e.preventDefault()
        onToggleFullscreen()
        return
      }

      // ── 翻页：必须在 `hasImage` 早退之前处理 ──
      // 读不出来的图片（损坏 / 不支持的格式 / 空文件）也要能翻走，
      // 否则用户卡在一张坏图上，只能用鼠标点工具栏。
      // 键位：空格 / ArrowRight / PageDown = 下一张，ArrowLeft / PageUp = 上一张。
      if (e.code === 'Space' || e.key === ' ') {
        e.preventDefault()
        if (e.shiftKey) onPrev()
        else onNext()
        return
      }
      if (e.key === 'ArrowRight' || e.key === 'PageDown') {
        e.preventDefault()
        onNext()
        return
      }
      if (e.key === 'ArrowLeft' || e.key === 'PageUp') {
        e.preventDefault()
        onPrev()
        return
      }

      if (!hasImage) return

      switch (e.key) {
        // 缩放
        case '+':
        case '=':
          e.preventDefault()
          onAction('zoom-in')
          break
        case '-':
          e.preventDefault()
          onAction('zoom-out')
          break
        case '0':
          e.preventDefault()
          onAction('fit')
          break
        case '1':
          e.preventDefault()
          onAction('actual')
          break
        // 旋转
        case '[':
          e.preventDefault()
          onAction('rotate-left')
          break
        case ']':
          e.preventDefault()
          onAction('rotate-right')
          break
        // ArrowUp / ArrowDown 保留为竖直平移（左右已让给翻页）
        case 'ArrowUp':
          e.preventDefault()
          onAction('pan-up')
          break
        case 'ArrowDown':
          e.preventDefault()
          onAction('pan-down')
          break
        case 'Escape':
          onAction('fit')
          break
      }
    }

    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [onOpen, onPrint, onAction, hasImage, onNext, onPrev, onToggleFullscreen, onToggleMaximize, onToggleInfo, onToggleAdjust, onToggleHelp, onEscape, isFullscreen, onRefresh])
}
