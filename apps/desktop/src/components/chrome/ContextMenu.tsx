/**
 * 右键菜单（主界面）。
 *
 * ## 定位
 * 主要是「对**当前这张图**的操作」—— 另存为、打印、视图相关的几项；
 * 另外还有**全屏切换**（它不依赖图片，但全屏时工具栏会隐藏，
 * 右键菜单是全屏下唯一还在的入口）。
 *
 * 凡是工具栏/快捷键已经有的入口，这里给的是**就近的等价物**：
 * 用户不必把视线移到顶部，也不用记快捷键。
 *
 * ## 关闭方式（四种，覆盖所有常见预期）
 * - 点菜单外任意处
 * - `Esc`
 * - 右键换一个位置（直接重定位，不闪一下）
 * - 执行任意一项
 *
 * ⚠️ **右键不能关掉菜单**（再次右键同位置 → 关闭，这是 Windows 通用行为），
 * 否则用户想「换个位置重新右键」时会莫名关掉。
 */

import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'

/** 菜单里的一项（由 App 组装好文案与回调后传入） */
export interface ContextMenuItem {
  /** 稳定的唯一 id —— React key 与「刚执行过哪项」高亮都用它 */
  id: string
  label: string
  /** 右侧灰字提示，如快捷键 */
  hint?: string
  /** 分组分隔线：该项**之前**画一条线 */
  separatorBefore?: boolean
  disabled?: boolean
  onSelect: () => void
}

interface ContextMenuProps {
  items: ContextMenuItem[]
  /** 触发右键时的鼠标视口坐标（`clientX/clientY`）。菜单据此定位在鼠标处，并贴边翻转。 */
  position: { x: number; y: number }
  /** 点菜单外 / Esc / 执行后关闭 */
  onClose: () => void
}

/** 菜单最小宽度，与 CSS 一致 */
const MENU_WIDTH = 196
/** 距视口边缘的留白：贴边时向内收，否则菜单一半在窗口外点不到 */
const EDGE_PAD = 8

export function ContextMenu({ items, position, onClose }: ContextMenuProps) {
  const menuRef = useRef<HTMLDivElement>(null)
  // 初始位置 = 鼠标点击处。组件仅在 state 非空时挂载，所以这里拿到的就是当次右键坐标；
  // 换位置重新右键时由 App 用 key 强制重挂载，pos 重新取新坐标（翻转逻辑在 useLayoutEffect 里修正贴边）。
  const [pos, setPos] = useState(position)

  /**
   * 贴边翻转：先用估算高度定位置，再用实际高度修正一次。
   *
   * 为什么要两段：第一段渲染前拿不到真实尺寸（DOM 还没生成），
   * 但必须先定一个位置才能渲染；渲染后立刻量一次做修正，
   * 避免菜单在窗口底部/右侧被裁掉。
   */
  useLayoutEffect(() => {
    const el = menuRef.current
    if (!el) return
    const vw = window.innerWidth
    const vh = window.innerHeight
    const rect = el.getBoundingClientRect()
    const flipX = pos.x + rect.width > vw - EDGE_PAD
    const flipY = pos.y + rect.height > vh - EDGE_PAD
    if (flipX || flipY) {
      setPos((p) => ({
        x: flipX ? Math.max(EDGE_PAD, p.x - rect.width) : p.x,
        y: flipY ? Math.max(EDGE_PAD, p.y - rect.height) : p.y,
      }))
    }
  }, [pos.x, pos.y])

  // Esc 关闭；执行任意项也关闭（包在 handle 里）
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation()
        onClose()
      }
    }
    // 用 capture 抢在 App 的快捷键之前 —— App 的 Esc 还管着「退出全屏」，
    // 菜单开着时按 Esc 应该只关菜单，不该顺手把全屏也退了。
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [onClose])

  /** 点菜单外关闭。用 mousedown 而不是 click：菜单项自己的点击还没处理完就关掉会让闪烁 */
  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) onClose()
    }
    window.addEventListener('mousedown', onDown)
    window.addEventListener('resize', onClose)
    window.addEventListener('blur', onClose)
    return () => {
      window.removeEventListener('mousedown', onDown)
      window.removeEventListener('resize', onClose)
      window.removeEventListener('blur', onClose)
    }
  }, [onClose])

  const run = useCallback(
    (item: ContextMenuItem) => {
      if (item.disabled) return
      item.onSelect()
      onClose()
    },
    [onClose],
  )

  return (
    <div
      className="pv-ctx"
      ref={menuRef}
      role="menu"
      aria-label="图片操作"
      style={{ left: pos.x, top: pos.y, minWidth: MENU_WIDTH }}
      onContextMenu={(e) => {
        // 右键菜单本身不该再弹一个系统菜单
        e.preventDefault()
        e.stopPropagation()
      }}
    >
      {items.map((item) => (
        <div key={item.id} className="pv-ctx-row">
          {item.separatorBefore && <div className="pv-ctx-sep" role="separator" />}
          <button
            className="pv-ctx-item"
            role="menuitem"
            disabled={item.disabled}
            title={item.hint}
            onClick={() => run(item)}
          >
            <span className="pv-ctx-label">{item.label}</span>
            {item.hint && <span className="pv-ctx-hint">{item.hint}</span>}
          </button>
        </div>
      ))}
    </div>
  )
}

/** 菜单状态：位置 + 是否打开。关闭时位置清空，避免下次先闪一下再移动。 */
export interface ContextMenuState {
  x: number
  y: number
  /** 触发右键时鼠标落在哪（用来判断"同位置再右键 = 关闭"） */
  atX: number
  atY: number
}

export function useContextMenu() {
  const [state, setState] = useState<ContextMenuState | null>(null)
  /**
   * 当前位置的 ref 副本 —— 只为让 `open` 不依赖 `state`。
   *
   * ⛔ 如果直接 `useCallback([state])`：`open` 每次菜单开关都会变，
   * 于是挂在它上面的 `onContextMenu` 也跟着变 → 画布元素每次重渲染，
   * 白白触发 React 更新。ref 是判断「是否同位置再右键」的干净做法。
   */
  const stateRef = useRef<ContextMenuState | null>(null)

  /**
   * 在鼠标位置打开菜单。
   *
   * ⚠️ **必须 `preventDefault()`**：不阻止的话 WebView2 会同时弹出
   * Chromium 自己的默认菜单（重新加载 / 检查元素 / 查看源代码），
   * 两层菜单叠在一起 —— 这是 Tauri 上做右键菜单最常见的坑。
   */
  const open = useCallback((e: React.MouseEvent) => {
    e.preventDefault()
    e.stopPropagation()
    const cur = stateRef.current
    const sameSpot =
      cur !== null && Math.abs(cur.atX - e.clientX) < 4 && Math.abs(cur.atY - e.clientY) < 4
    if (sameSpot) {
      stateRef.current = null
      setState(null) // 同位置再右键 = 关闭（Windows 通用行为）
      return
    }
    const next: ContextMenuState = {
      x: e.clientX,
      y: e.clientY,
      atX: e.clientX,
      atY: e.clientY,
    }
    stateRef.current = next
    setState(next)
  }, [])

  const close = useCallback(() => {
    stateRef.current = null
    setState(null)
  }, [])

  /** 换图 / 退出全屏等场景要主动关掉，否则菜单会挂在旧图上 */
  const reset = close

  return { state, open, close, reset }
}