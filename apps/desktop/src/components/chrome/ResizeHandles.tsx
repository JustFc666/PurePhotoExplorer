/**
 * 窗口边缘 resize 热区（8 个：四边 + 四角）。
 *
 * `decorations: false` 后 Windows 剥掉了 `WS_THICKFRAME`，
 * 鼠标移到边框不再变成缩放光标、也不响应拖拽，必须自己铺一层透明热区，
 * 按下时调 `startResizeDragging(direction)` 把控制权交还系统。
 *
 * 热区全部 `position: fixed` 贴在视口边缘，`z-index` 高于内容但不挡按钮
 * （按钮在工具栏内、距边缘超过热区宽度，天然不冲突）。
 * 最大化状态下不渲染 —— 此时拖边框没有意义。
 */

import type { CSSProperties, MouseEvent as ReactMouseEvent } from 'react'
import { RESIZE_BORDER, RESIZE_CORNER, type ResizeDirection } from '../../hooks/useWindowDrag'

interface ResizeHandlesProps {
  /** 最大化时隐藏全部热区 */
  enabled: boolean
  onResize: (direction: ResizeDirection, e: ReactMouseEvent) => void
}

/** [方向, 样式] —— 四边用边框宽，四角用角宽且叠在边上更靠外 */
const HANDLES: ReadonlyArray<readonly [ResizeDirection, CSSProperties]> = [
  // 四条边
  ['North', { top: 0, left: RESIZE_CORNER, right: RESIZE_CORNER, height: RESIZE_BORDER, cursor: 'ns-resize' }],
  ['South', { bottom: 0, left: RESIZE_CORNER, right: RESIZE_CORNER, height: RESIZE_BORDER, cursor: 'ns-resize' }],
  ['West', { left: 0, top: RESIZE_CORNER, bottom: RESIZE_CORNER, width: RESIZE_BORDER, cursor: 'ew-resize' }],
  ['East', { right: 0, top: RESIZE_CORNER, bottom: RESIZE_CORNER, width: RESIZE_BORDER, cursor: 'ew-resize' }],
  // 四个角：尺寸用 RESIZE_CORNER，压在边的外侧
  ['NorthWest', { top: 0, left: 0, width: RESIZE_CORNER, height: RESIZE_CORNER, cursor: 'nwse-resize' }],
  ['NorthEast', { top: 0, right: 0, width: RESIZE_CORNER, height: RESIZE_CORNER, cursor: 'nesw-resize' }],
  ['SouthWest', { bottom: 0, left: 0, width: RESIZE_CORNER, height: RESIZE_CORNER, cursor: 'nesw-resize' }],
  ['SouthEast', { bottom: 0, right: 0, width: RESIZE_CORNER, height: RESIZE_CORNER, cursor: 'nwse-resize' }],
]

export function ResizeHandles({ enabled, onResize }: ResizeHandlesProps) {
  if (!enabled) return null

  return (
    <>
      {HANDLES.map(([direction, style]) => (
        <div
          key={direction}
          className="pv-resize-handle"
          style={style}
          onMouseDown={(e) => onResize(direction, e)}
          aria-hidden
        />
      ))}
    </>
  )
}
