/**
 * 标准下拉框（筛选栏：格式 / 年份 / 相机 / 地点）。
 *
 * ## 为什么不用 ContextMenu
 * 2026-10-24 用户要求「用标准下拉框，不要悬浮」。原来的做法是把
 * `ContextMenu` 用 portal 挂到 body、按**鼠标坐标**弹出一个悬浮面板：
 * ① 位置跟按钮没关系，点上面和点下面弹出来的地方不一样；
 * ② 面板浮在网格上，视觉上是「飘」的。
 * 这里改成**锚定在按钮正下方**的内联下拉：面板是按钮父容器的子元素，
 * 随布局走、关得上、看得见来源。
 *
 * ## 交互约定
 * - 点按钮开/关；点面板外任意处或 `Esc` 关。
 * - ⭐ 多选筛选（格式/相机/地点/年份都是多选）**选中后不关面板**，
 *   这样连勾几项不用反复开关；想关就点外面或按 Esc。
 * - 最多显示 13 行，超出滚动（见 `.pv-fdd-panel` 的 max-height）。
 *
 * ⛔ 不要改成原生 `<select>`：自绘无边框窗口里原生下拉的弹层
 *    不跟随深色配色、位置还会被裁（项目既定禁令）。
 */

import { useEffect, useRef, useState, type ReactNode } from 'react'

export interface FilterDropdownProps {
  /** 触发器宽度（px）：格式/年份 200、相机 240、地点 280 */
  width: number
  /** 是否有选中项（决定描边高亮） */
  active: boolean
  title: string
  /** 按钮左边的分类名 */
  label: string
  children: ReactNode
}

export function FilterDropdown({
  width,
  active,
  title,
  label,
  children,
}: FilterDropdownProps) {
  const [open, setOpen] = useState(false)
  const boxRef = useRef<HTMLDivElement | null>(null)

  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent) => {
      if (boxRef.current && !boxRef.current.contains(e.target as Node)) setOpen(false)
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false)
    }
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('keydown', onKey)
    }
  }, [open])

  return (
    <div className="pv-fdd" ref={boxRef} style={{ width }}>
      <button
        type="button"
        className={`pv-mg-fbtn${active ? ' is-on' : ''}`}
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
        title={title}
      >
        {label}
        <span className="pv-mg-caret">▾</span>
      </button>
      {open && (
        <div className="pv-fdd-panel" role="group" aria-label={`${label}筛选`}>
          {children}
        </div>
      )}
    </div>
  )
}

export interface FilterRowProps {
  checked: boolean
  label: string
  /** 右侧「N 张」；传 null 不显示 */
  count: number | null
  onSelect: () => void
}

/** 下拉里的一行：左勾选框 + 名字，右侧右对齐张数（复用右键菜单的行样式） */
export function FilterRow({ checked, label, count, onSelect }: FilterRowProps) {
  return (
    /*
      `title={label}`：相机名/地点名经常比下拉宽（`iPhone 15 Pro Max back triple camera`
      这种），文本会被省略成 `iPhone 15 …`。**悬停要能看到全名** ——
      没有这条，用户选的时候只能靠猜（2026-10-25 用户报的）。
    */
    <button type="button" className="pv-ctx-item" onClick={onSelect} title={label}>
      <span className="pv-fdd-name">
        <span className={`pv-ctx-check${checked ? ' is-on' : ''}`} aria-hidden />
        <span className="pv-fdd-text">{label}</span>
      </span>
      {count !== null && <span className="pv-ctx-hint">{count} 张</span>}
    </button>
  )
}
