/**
 * 自绘窗口控制按钮（最小化 / 最大化·还原 / 关闭）。
 *
 * 仅在 `decorations: false` 时使用 —— 系统标题栏没了，这三个按钮
 * 得自己画在工具栏右端。视觉尺寸对齐 Windows 原生：
 * 按钮宽 46 物理 px ÷ 1.25 ≈ 36.8 CSS px，高度撑满工具栏（50px）。
 *
 * 关闭按钮沿用 Windows 惯例：hover 时底色变红（`#c42b1c`）。
 */

interface WindowControlsProps {
  isMaximized: boolean
  onMinimize: () => void
  onToggleMaximize: () => void
  onClose: () => void
}

export function WindowControls({
  isMaximized,
  onMinimize,
  onToggleMaximize,
  onClose,
}: WindowControlsProps) {
  return (
    <div className="pv-winctl" data-no-drag>
      <button className="pv-winctl-btn" onClick={onMinimize} title="最小化" aria-label="最小化">
        {/* 最小化：一条水平线 */}
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden>
          <path d="M0 5.5h10" stroke="currentColor" strokeWidth="1" />
        </svg>
      </button>

      <button
        className="pv-winctl-btn"
        onClick={onToggleMaximize}
        title={isMaximized ? '向下还原' : '最大化'}
        aria-label={isMaximized ? '向下还原' : '最大化'}
      >
        {isMaximized ? (
          // 还原：两个错开的方框
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden>
            <path
              d="M2.5 2.5V0.5h7v7h-2"
              fill="none"
              stroke="currentColor"
              strokeWidth="1"
            />
            <rect
              x="0.5"
              y="2.5"
              width="7"
              height="7"
              fill="none"
              stroke="currentColor"
              strokeWidth="1"
            />
          </svg>
        ) : (
          // 最大化：单个方框（必须包在 <svg> 里，裸 <rect> 不会渲染）
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden>
            <rect
              x="0.5"
              y="0.5"
              width="9"
              height="9"
              fill="none"
              stroke="currentColor"
              strokeWidth="1"
            />
          </svg>
        )}
      </button>

      <button
        className="pv-winctl-btn pv-winctl-close"
        onClick={onClose}
        title="关闭"
        aria-label="关闭"
      >
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden>
          <path
            d="M0.5 0.5l9 9M9.5 0.5l-9 9"
            stroke="currentColor"
            strokeWidth="1"
          />
        </svg>
      </button>
    </div>
  )
}
