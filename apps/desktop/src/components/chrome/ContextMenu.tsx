/**
 * 右键菜单（主界面 + 图库侧栏共用）。
 *
 * ## 定位
 * 主要是「对**当前这张图**的操作」—— 另存为、打印、视图相关的几项；
 * 另外还有**全屏切换**（它不依赖图片，但全屏时工具栏会隐藏，
 * 右键菜单是全屏下唯一还在的入口）。图库侧栏也复用它做目录操作。
 *
 * 凡是工具栏/快捷键已经有的入口，这里给的是**就近的等价物**：
 * 用户不必把视线移到顶部，也不用记快捷键。
 *
 * ## 子菜单
 * `item.children` 非空即渲染成「▸」，**悬停**展开（不用点击）——
 * 这是 Windows 菜单的通用行为，点一下才展开会让人觉得菜单坏了。
 * 当前只支持**一层**子菜单（够用即可，不做无限嵌套）。
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
import { createPortal } from 'react-dom'

/** 菜单里的一项 */
export interface ContextMenuItem {
  /** 稳定的唯一 id —— React key 用它 */
  id: string
  label: string
  /** 右侧灰字提示，如快捷键 */
  hint?: string
  /** 分组分隔线：该项**之前**画一条线 */
  separatorBefore?: boolean
  disabled?: boolean
  /**
   * 勾选态：显示一个方框（多选筛选用）。
   * 不传 = 不画方框（普通菜单项）；`false` = 空框；`true` = 打勾。
   */
  checked?: boolean
  /** 叶子项的动作；有 `children` 时忽略（父项只负责展开子菜单） */
  onSelect?: () => void
  /** 子菜单项（**只支持一层**） */
  children?: ContextMenuItem[]
}

interface ContextMenuProps {
  items: ContextMenuItem[]
  /** 触发右键时的鼠标视口坐标（`clientX/clientY`）。菜单据此定位在鼠标处，并贴边翻转。 */
  position: { x: number; y: number }
  /** 点菜单外 / Esc / 执行后关闭 */
  onClose: () => void
  /**
   * 固定宽度（px）。不传 = 最小 [`MENU_WIDTH`]、由内容撑开。
   *
   * 短菜单（排序这种「2–4 个字一列」）用 196 会显得很空，
   * 传 120 既紧凑、又保证每一项等宽（不会因文字长短而参差）。
   */
  width?: number
}

/** 菜单最小宽度，与 CSS 一致 */
const MENU_WIDTH = 196
/** 距视口边缘的留白：贴边时向内收，否则菜单一半在窗口外点不到 */
const EDGE_PAD = 8

/**
 * 菜单项的行（根菜单与子菜单共用同一套 markup，避免两处走样）。
 *
 * ⚠️ `onHover` **只在根菜单传**。
 *
 * 曾经的 bug（2026-10-18 用户报告「鼠标移到右边子菜单，菜单就消失」）：
 * 子菜单项也绑了 `onMouseEnter → hover()`，而 `hover` 对「没有 children 的项」
 * 会 `setSub(null)` —— 于是鼠标刚进子菜单，就被判成「离开了父项」，子菜单立刻关掉。
 * 子菜单只支持一层，里面的项必然没有 children，所以**不需要 hover 切换**
 * （传 `undefined` 就不挂监听）。
 */
function MenuRow({
  item,
  onRun,
  onHover,
}: {
  item: ContextMenuItem
  onRun: (item: ContextMenuItem) => void
  /** 省略 = 这一行不参与「悬停换子菜单」（子菜单内部就是这样） */
  onHover?: (item: ContextMenuItem, el: HTMLElement) => void
}) {
  const hasSub = (item.children?.length ?? 0) > 0
  return (
    <div className="pv-ctx-row">
      {item.separatorBefore && <div className="pv-ctx-sep" role="separator" />}
      <button
        className={`pv-ctx-item${hasSub ? ' has-sub' : ''}`}
        role="menuitem"
        disabled={item.disabled}
        aria-haspopup={hasSub ? 'menu' : undefined}
        title={item.hint}
        onMouseEnter={onHover ? (e) => onHover(item, e.currentTarget) : undefined}
        // 父项点了不关菜单（子菜单靠悬停展开），点它会误以为「没反应」
        onClick={() => {
          if (!hasSub) onRun(item)
        }}
      >
        {/* 勾选方框：筛选类菜单用它表达「这一项已选」 */}
        {item.checked !== undefined && (
          <span className={`pv-ctx-check${item.checked ? ' is-on' : ''}`} aria-hidden />
        )}
        <span className="pv-ctx-label">{item.label}</span>
        {hasSub ? (
          <span className="pv-ctx-hint" aria-hidden>
            ▸
          </span>
        ) : (
          item.hint && <span className="pv-ctx-hint">{item.hint}</span>
        )}
      </button>
    </div>
  )
}

export function ContextMenu({ items, position, onClose, width }: ContextMenuProps) {
  const menuRef = useRef<HTMLDivElement>(null)
  /** 包裹根面板 + 子菜单：「点菜单外关闭」要按它判断，否则点子菜单会整片关掉 */
  const wrapRef = useRef<HTMLDivElement>(null)
  // 初始位置 = 鼠标点击处。组件仅在 state 非空时挂载，所以这里拿到的就是当次右键坐标；
  // 换位置重新右键时由 App 用 key 强制重挂载，pos 重新取新坐标（翻转逻辑在 useLayoutEffect 里修正贴边）。
  const [pos, setPos] = useState(position)
  /**
   * 首次渲染时先隐藏，等 `useLayoutEffect` 量完真实尺寸并做贴边翻转后
   * 再显示。否则菜单会先闪一下错误位置（用户会感觉到“跳一下”）。
   */
  const [ready, setReady] = useState(false)
  const [sub, setSub] = useState<{ items: ContextMenuItem[]; x: number; y: number } | null>(null)
  const [subReady, setSubReady] = useState(false)
  const subRef = useRef<HTMLDivElement>(null)

  /**
   * 右键不同位置时**由调用方用坐标当 key 强制重挂载**，`pos` 从 `useState` 拿到新坐标。
   *
   * ⛔ 别改成「保留实例 + `useEffect([position.x, position.y])` 同步坐标」：
   * 那个 effect **首次挂载时也会跑**，会把 `useLayoutEffect` 刚设好的 `ready` 重置成
   * `false`，而 `setPos(position)` 又是同一个值、`useLayoutEffect` 不会再跑 →
   * 菜单永远停在 `visibility: hidden`，表现为「菜单一闪就消失」（2026-10-18 实测）。
   */

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
      setPos((p) => {
        // 翻到鼠标上方后如果还超出（菜单比可视区还高），就贴住上边
        const y = flipY
          ? Math.max(EDGE_PAD, Math.min(p.y - rect.height, vh - EDGE_PAD - rect.height))
          : p.y
        const x = flipX
          ? Math.max(EDGE_PAD, Math.min(p.x - rect.width, vw - EDGE_PAD - rect.width))
          : p.x
        return x === p.x && y === p.y ? p : { x, y }
      })
    }
    setReady(true)
    // ⚠️ 依赖里必须有 `items.length`：标签菜单是**先弹 1 行"读取中"再换成 N 行**的，
    // 高度在挂载后才变化，只依赖坐标的话永远不会重新贴边 → 底部被窗口裁掉（2026-10-22 用户报的 bug）。
  }, [pos.x, pos.y, items.length])

  /** 子菜单的纵向贴边修正（横向在展开时已算过） */
  useLayoutEffect(() => {
    const el = subRef.current
    if (!el || !sub) return
    const vh = window.innerHeight
    const rect = el.getBoundingClientRect()
    if (sub.y + rect.height > vh - EDGE_PAD) {
      setSub((s) =>
        s ? { ...s, y: Math.max(EDGE_PAD, vh - EDGE_PAD - rect.height) } : s,
      )
    }
    setSubReady(true)
  }, [sub])

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
      if (wrapRef.current && !wrapRef.current.contains(e.target as Node)) onClose()
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
      if (item.disabled || !item.onSelect) return
      item.onSelect()
      onClose()
    },
    [onClose],
  )

  /**
   * 子菜单延迟关闭的计时器。
   *
   * 鼠标从父项斜向移到子菜单时，可能先掠过下面一行（没有 children）；
   * 立刻 `setSub(null)` 会让子菜单「刚出现就消失」。给 240ms 缓冲，
   * 期间进入子菜单就把计时器取消。
   */
  const subTimer = useRef<number | null>(null)
  const cancelSubTimer = useCallback(() => {
    if (subTimer.current != null) {
      window.clearTimeout(subTimer.current)
      subTimer.current = null
    }
  }, [])
  // 卸载时别留下悬空的计时器（触发时组件已经没了）
  useEffect(() => cancelSubTimer, [cancelSubTimer])

  /** 悬停到某项：有子项就展开到它右侧（右边放不下就翻到左侧） */
  const hover = useCallback(
    (item: ContextMenuItem, el: HTMLElement) => {
      // 进到任何一行都先取消「等待关闭」
      cancelSubTimer()
      if (!item.children?.length) {
        // 无子项的项：延迟关闭，避免斜移路径穿过它时把子菜单打掉
        subTimer.current = window.setTimeout(() => {
          setSub(null)
          setSubReady(false)
          subTimer.current = null
        }, 240)
        return
      }
      const r = el.getBoundingClientRect()
      const flipX = r.right + MENU_WIDTH > window.innerWidth - EDGE_PAD
      setSubReady(false)
      setSub({
        items: item.children,
        x: flipX ? Math.max(EDGE_PAD, r.left - MENU_WIDTH + 4) : r.right - 4,
        y: r.top - 4,
      })
    },
    [cancelSubTimer],
  )

  const stopCtx = (e: React.MouseEvent) => {
    // 右键菜单本身不该再弹一个系统菜单
    e.preventDefault()
    e.stopPropagation()
  }

  /**
   * ⭐ 用 Portal 挂到 `document.body`，**不要**作为调用方（如侧栏 `<aside>`）的子元素。
   *
   * 曾经的坑：菜单挂在侧栏里时，它的 wrapper 会变成 `<aside>` 的 `:last-child`，
   * 把图库分区的 `.pv-mg-sec:last-child { flex: 1 1 42% }` 顶掉 → 侧栏布局瞬间重算，
   * 用户看到「跳一下」。挂到 body 后菜单完全不参与调用方的布局。
   */
  return createPortal(
    <div ref={wrapRef}>
      <div
        className="pv-ctx"
        ref={menuRef}
        role="menu"
        aria-label="菜单"
        style={{ left: pos.x, top: pos.y, minWidth: width ?? MENU_WIDTH, width, visibility: ready ? 'visible' : 'hidden' }}
        onContextMenu={stopCtx}
      >
        {items.map((item) => (
          <MenuRow key={item.id} item={item} onRun={run} onHover={hover} />
        ))}
      </div>

      {sub && (
        <div
          className="pv-ctx"
          ref={subRef}
          role="menu"
          aria-label="子菜单"
          style={{ left: sub.x, top: sub.y, minWidth: MENU_WIDTH, visibility: subReady ? 'visible' : 'hidden' }}
          onContextMenu={stopCtx}
          // 鼠标进到子菜单就取消「等待关闭」——这正是之前子菜单一碰就没的地方
          onMouseEnter={cancelSubTimer}
        >
          {/*
            ⚠️ 子菜单项**不传 `onHover`**：它们必然没有 children，
            传了会被 `hover` 判成「离开父项」而立刻 `setSub(null)`。
          */}
          {sub.items.map((item) => (
            <MenuRow key={item.id} item={item} onRun={run} />
          ))}
        </div>
      )}
    </div>,
    document.body,
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
