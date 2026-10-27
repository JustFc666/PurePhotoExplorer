/**
 * 输入名称的小弹窗（「自定义新图库」用）。
 *
 * ## 为什么不用 `window.prompt`
 * Tauri 的 WebView2 里 `window.prompt` 不可用（返回 null，拿不到输入）。
 * `@tauri-apps/plugin-dialog` 也**没有** prompt —— 它只有 message / ask / confirm /
 * open / save 五种，全都不收文本输入。所以只能自己做一个。
 *
 * ## 键盘
 * - 打开即聚焦并全选（用户多半想直接覆盖输入）
 * - `Enter` = 确定，`Esc` = 取消
 * - ⚠️ `Esc` 用**捕获阶段**拦下并 `stopPropagation`：图库模式下全局 `Esc`
 *   是「退回单图浏览」，不拦的话按 Esc 想取消输入，结果整个模式都退掉了。
 */

import { useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'

interface Props {
  title: string
  /** 输入框上方的说明 */
  label?: string
  placeholder?: string
  initial?: string
  okLabel?: string
  /** 上一次提交失败的原因（红字）。有值时弹窗**保持打开**，用户改完直接再回车。 */
  error?: string
  onOk: (value: string) => void
  onCancel: () => void
}

export function PromptDialog({
  title,
  label,
  placeholder,
  initial = '',
  okLabel = '确定',
  error,
  onOk,
  onCancel,
}: Props) {
  const [value, setValue] = useState(initial)
  const inputRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    const el = inputRef.current
    if (!el) return
    el.focus()
    el.select()
  }, [])

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return
      e.stopPropagation()
      onCancel()
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [onCancel])

  const trimmed = value.trim()
  const submit = () => {
    if (trimmed) onOk(trimmed)
  }

  return createPortal(
    <div
      className="pv-modal-mask"
      // 点遮罩 = 取消（点卡片本身不关）
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onCancel()
      }}
    >
      <div className="pv-modal" role="dialog" aria-modal="true" aria-label={title}>
        <div className="pv-modal-title">{title}</div>
        {label && <div className="pv-modal-label">{label}</div>}
        <input
          ref={inputRef}
          className="pv-modal-input"
          value={value}
          placeholder={placeholder}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault()
              submit()
            }
          }}
        />
        {error && (
          <div className="pv-modal-err" role="alert">
            {error}
          </div>
        )}
        <div className="pv-modal-actions">
          <button type="button" className="pv-btn" onClick={onCancel}>
            取消
          </button>
          <button
            type="button"
            className="pv-btn pv-btn-primary"
            disabled={trimmed.length === 0}
            onClick={submit}
          >
            {okLabel}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  )
}
