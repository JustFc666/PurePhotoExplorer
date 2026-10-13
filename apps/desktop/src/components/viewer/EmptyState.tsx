/**
 * 空状态 / 加载中 / 错误态。
 */

import type { Status } from '../../types/viewer'
import { ErrorDetail } from './ErrorDetail'

interface EmptyStateProps {
  status: Status
  onOpen: () => void
}

export function EmptyState({ status, onOpen }: EmptyStateProps) {
  return (
    <div className="pv-empty">
      {status.kind === 'loading' && (
        <>
          <div className="pv-spinner" aria-hidden />
          <p className="pv-empty-title">正在解码…</p>
        </>
      )}

      {status.kind === 'empty' && (
        <>
          <div className="pv-empty-icon" aria-hidden />
          <p className="pv-empty-title">未打开图片</p>
          <p className="pv-empty-sub">点击左上角「打开」，或把图片拖进窗口</p>
          <button className="pv-btn pv-btn-primary pv-open-btn" onClick={onOpen}>
            打开图片
          </button>
        </>
      )}

      {status.kind === 'error' && (
        <>
          <div className="pv-empty-icon pv-icon-error" aria-hidden />
          <p className="pv-empty-title">无法显示这张图片</p>
          <ErrorDetail message={status.message} hint={status.hint} raw={status.raw} />
          <button className="pv-btn pv-open-btn" onClick={onOpen}>
            换一张试试
          </button>
        </>
      )}
    </div>
  )
}
