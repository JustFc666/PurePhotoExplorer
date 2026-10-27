/**
 * 自绘确认框（替代 `@tauri-apps/plugin-dialog` 的 `confirm`）。
 *
 * ## 为什么不用插件的 confirm
 * 1. **行为不可控**：插件的 `confirm` 底层是 `message` + 自定义按钮，返回值靠
 *    「按钮 label 字符串 === okLabel」判断；任何一环对不上（按钮格式、返回标签、
 *    权限）都会静默失败 —— `void confirmRemove(...)` 把 Promise 的 rejection 吞掉，
 *    用户点了菜单**什么都不会发生**，也没有任何错误提示（2026-10-27 踩过：
 *    「从图库移除」一直"未生效"，查了半天是确认框压根没弹出来）。
 * 2. **样式不一致**：这是自绘无边框窗口，原生 MessageBox 的观感与全站 CSS 割裂。
 * 3. 项目里其他确认（改名、建图库）都走自绘 `PromptDialog`，这里补齐同一套。
 *
 * ## 键盘
 * - `Enter` = 确认，`Esc` = 取消
 * - ⚠️ `Esc` 用**捕获阶段**拦下并 `stopPropagation`：图库模式全局 `Esc` 是
 *   「退回单图浏览」，不拦的话按 Esc 想取消，结果整个模式都退掉了。
 *
 * ## ⛔ 必须挂到 `document.body`
 * 调用方常把它写在侧栏（`.pv-mg-side`）里，而侧栏有 `overflow: hidden` +
 * `flex` 容器。直接作子元素渲染时，遮罩虽然 `position: fixed`，仍会被祖先的
 * 层叠/裁剪吃掉 —— 表现是**点了菜单、状态也改了，但屏幕上什么都没有**，
 * 和「功能没生效」一模一样（2026-10-27 踩过：换成自绘框后依然"没反应"）。
 * 和 `ContextMenu` 同一套理由，统一用 `createPortal` 到 `body`。
 */

import { useEffect, useRef } from 'react'
import { createPortal } from 'react-dom'

interface Props {
  title: string
  /** 正文。含 `\n` 时按行渲染（保持和原来 `confirm` 的多行文案一致） */
  message: string
  okLabel?: string
  cancelLabel?: string
  /** 破坏性操作：确认按钮标红 */
  danger?: boolean
  onOk: () => void
  onCancel: () => void
}

export function ConfirmDialog({
  title,
  message,
  okLabel = '确定',
  cancelLabel = '取消',
  danger,
  onOk,
  onCancel,
}: Props) {
  const okRef = useRef<HTMLButtonElement>(null)

  /* 打开即聚焦确认按钮，直接回车就能继续 */
  useEffect(() => {
    okRef.current?.focus()
  }, [])

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation()
        onCancel()
      } else if (e.key === 'Enter') {
        e.stopPropagation()
        e.preventDefault()
        onOk()
      }
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [onOk, onCancel])

  return createPortal(
    <div
      className="pv-modal-mask"
      /* 点遮罩 = 取消（点卡片本身不关） */
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onCancel()
      }}
    >
      <div className="pv-modal" role="dialog" aria-modal="true" aria-label={title}>
        <div className="pv-modal-title">{title}</div>
        {/* 整段交给 CSS 的 `white-space: pre-line` 处理换行，
            不拆成一堆 div —— 拆了每行都会吃一份间距，文案越长越散 */}
        <div className="pv-modal-text">{message}</div>
        <div className="pv-modal-actions">
          <button type="button" className="pv-btn" onClick={onCancel}>
            {cancelLabel}
          </button>
          <button
            ref={okRef}
            type="button"
            className={`pv-btn ${danger ? 'pv-btn-danger' : 'pv-btn-primary'}`}
            onClick={onOk}
          >
            {okLabel}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  )
}
