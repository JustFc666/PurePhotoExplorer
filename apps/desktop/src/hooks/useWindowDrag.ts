/**
 * 自绘标题栏的窗口交互（配合 `decorations: false`）。
 *
 * `decorations: false` 让 Windows 剥掉 `WS_CAPTION | WS_THICKFRAME`，
 * 系统标题栏消失、内容铺满整窗，但**边框交互同时失效**，需逐项补回：
 *
 * | 能力         | 系统原生     | 本模块实现                        |
 * |--------------|--------------|-----------------------------------|
 * | 拖动窗口     | 标题栏       | `startDragging()`                 |
 * | 双击最大化   | 标题栏       | 自己计时 + `toggleMaximize()`     |
 * | 拖拽调大小   | 边框/四角    | 8 热区 → `startResizeDragging()`  |
 *
 * ## 最大化为什么不用系统的
 *
 * 根因：`decorations: false` 之后 **`WS_EX_WINDOWEDGE` 扩展样式依然存在**，
 * 且**无法清除** —— tao 的 `window_state::apply_diff()` 在窗口任何 flag
 * 变化时都会用 `to_window_styles()` 完整重写 `GWL_STYLE/GWL_EXSTYLE`，
 * 而该函数无条件写回 `WS_EX_WINDOWEDGE`（tao-0.37.1 源码 239/413 行）。
 * 实测清掉后瞬间被写回。
 *
 * 该残留边框会在**外框内侧**留一圈不可见的非客户区（本机 DPI 120 实测
 * 左 9 / 上 0 / 右 9 / 下 9 物理 px）。若只把「外框」摆到工作区，
 * 客户区就会四面内缩，肉眼表现为「最大化了但没铺满」。
 *
 * 因此最大化改由 Rust 侧实现：查 `SPI_GETWORKAREA` 取工作区矩形，
 * **按实测偏移把外框向外扩**后再 `SetWindowPos`（`maximize_to_work_area`
 * 命令），这样客户区恰好等于工作区。全程不经过 `SW_MAXIMIZE`，
 * 也就不会被系统的最大化布局覆盖。还原态由本模块记账。
 *
 * ## 度量约定
 *
 * 三个命令（`window_rect_physical` / `maximize_to_work_area` /
 * `restore_window_bounds`）**统一使用「客户区」物理像素矩形**。
 * 前端只负责原样存取，不做任何像素换算 —— 外框/客户区的换算全在
 * Rust 侧一处完成，避免两边口径不一致导致「最大化→还原→再最大化」漂移。
 *
 * ## 已知取舍
 *
 * 窗口不再带系统 `IsZoomed` 标记，且拖到屏幕边缘的 Aero Snap 吸附
 * 依赖 `WM_NCHITTEST` 协商、在 WebView 层无法复刻 —— 这是
 * `decorations: false` 自绘方案固有的代价。
 */

import { useCallback, useRef, useState } from 'react'
import type { MouseEvent as ReactMouseEvent } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'

/** 双击判定窗口（ms），与系统双击速度大致对齐 */
const DOUBLE_CLICK_MS = 300

/** 四边热区宽度（CSS px）—— 与原生边框手感对齐 */
export const RESIZE_BORDER = 6
/** 四角热区边长，比边框宽一些便于抓取 */
export const RESIZE_CORNER = 12

/** Rust 侧返回的窗口**客户区**矩形（物理像素，屏幕坐标） */
type PhysicalRect = [number, number, number, number]

/**
 * 拖拽调整大小的方向。
 * `@tauri-apps/api/window` 的 `ResizeDirection` 是内部类型别名、未导出，
 * 这里按 `startResizeDragging` 的签名本地复刻一份。
 */
export type ResizeDirection =
  | 'East'
  | 'North'
  | 'NorthEast'
  | 'NorthWest'
  | 'South'
  | 'SouthEast'
  | 'SouthWest'
  | 'West'

export interface UseWindowDrag {
  /** 绑定到标题栏（工具栏空白处）的按下处理器：拖动 / 双击最大化 */
  onDragHandleMouseDown: (e: ReactMouseEvent) => void
  /** 当前是否最大化（决定按钮图标是「最大化」还是「还原」） */
  isMaximized: boolean
  /** 最小化窗口 */
  minimize: () => void
  /** 最大化 / 还原切换 */
  toggleMaximize: () => void
  /** 关闭窗口 */
  close: () => void
  /** 开始拖拽调整大小（由四边四角热区调用） */
  startResize: (direction: ResizeDirection, e: ReactMouseEvent) => void
}

/** 最大化前的客户区矩形（物理像素），供「还原」回到原处 */
type RestoreBounds = { x: number; y: number; width: number; height: number }

export function useWindowDrag(): UseWindowDrag {
  /** 上一次按下时间，用于自己判定双击（`onDoubleClick` 在拖动后被吞掉） */
  const lastDownRef = useRef(0)
  /** 最大化前的客户区；null 表示当前不处于最大化态 */
  const restoreRef = useRef<RestoreBounds | null>(null)
  const [isMaximized, setIsMaximized] = useState(false)

  /** 最小化 */
  const minimize = useCallback(() => {
    getCurrentWindow()
      .minimize()
      .catch(() => {})
  }, [])

  /** 关闭窗口 */
  const close = useCallback(() => {
    getCurrentWindow()
      .close()
      .catch(() => {})
  }, [])

  /**
   * 最大化 / 还原。
   *
   * 最大化：记住当前客户区 → 交给 Rust 铺满工作区。
   * 还原：把记住的客户区交回 Rust。
   * 两个方向都绕开 `SW_MAXIMIZE`，避免 `WS_EX_WINDOWEDGE` 造成的内容内缩。
   */
  const toggleMaximize = useCallback(async () => {
    try {
      const saved = restoreRef.current
      if (saved) {
        // ── 还原 ──
        restoreRef.current = null
        await invoke<PhysicalRect | null>('restore_window_bounds', {
          x: Math.round(saved.x),
          y: Math.round(saved.y),
          width: Math.round(saved.width),
          height: Math.round(saved.height),
        })
        setIsMaximized(false)
        return
      }

      // ── 最大化 ──
      const rect = await invoke<PhysicalRect | null>('window_rect_physical')
      if (!rect) return
      const filled = await invoke<PhysicalRect | null>('maximize_to_work_area')
      if (!filled) return
      restoreRef.current = { x: rect[0], y: rect[1], width: rect[2], height: rect[3] }
      setIsMaximized(true)
    } catch {
      // 非 Tauri 环境或权限不足时静默失败
    }
  }, [])

  /** 标题栏空白处按下：拖动窗口 / 双击最大化 */
  const onDragHandleMouseDown = useCallback(
    (e: ReactMouseEvent) => {
      // 只响应鼠标左键
      if (e.button !== 0) return

      // 点在按钮/交互元素上时不拖窗口，也把双击计时重置，避免"点按钮算双击"
      const target = e.target as HTMLElement | null
      if (target?.closest('button, input, a, [data-no-drag]')) {
        lastDownRef.current = 0
        return
      }

      const now = Date.now()
      const isDoubleClick = now - lastDownRef.current < DOUBLE_CLICK_MS
      lastDownRef.current = now

      // 必须 preventDefault：否则系统会在 mousedown 后自行进入拖拽，与
      // 随后的 startDragging() 争抢，表现为窗口抖动或拖不动。
      e.preventDefault()

      if (isDoubleClick) {
        void toggleMaximize()
        return
      }

      getCurrentWindow()
        .startDragging()
        .catch(() => {})
    },
    [toggleMaximize],
  )

  /** 四边四角热区按下 → 交还系统做 resize */
  const startResize = useCallback((direction: ResizeDirection, e: ReactMouseEvent) => {
    if (e.button !== 0) return
    e.preventDefault()
    // 手动改尺寸后就不再是「最大化」态了
    restoreRef.current = null
    setIsMaximized(false)
    getCurrentWindow()
      .startResizeDragging(direction)
      .catch(() => {})
  }, [])

  // 注意：**不要**监听 window 的 resize 事件来清空 restoreRef。
  // 我们自己调用的 `SetWindowPos`（最大化/还原）同样会触发 resize，
  // 会把刚记录下来的还原态立刻抹掉，导致「还原」失效（踩坑记录）。

  return { onDragHandleMouseDown, isMaximized, minimize, toggleMaximize, close, startResize }
}
