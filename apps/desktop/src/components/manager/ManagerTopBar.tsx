/**
 * 图库模式顶栏 —— 兼作**窗口标题栏**（`decorations: false` 自绘方案）。
 *
 * 左 → 右：logo / 标题 · 菜单 · 搜索 · 视图模式 · 排序 · 窗口按钮。
 * 空白处按下交给 `useWindowDrag`（它会自己忽略落在控件上的按下）。
 */

import { useEffect, useRef, useState } from 'react'
import type { MouseEvent as ReactMouseEvent } from 'react'

import { WindowControls } from '../chrome/WindowControls'
import { LogoTile } from '../chrome/Toolbar'
import { ContextMenu } from '../chrome/ContextMenu'
import type { ManagerSort, ThumbTier } from '../../ipc/manager'
import type { ViewMode } from '../../hooks/useManager'
import { SORT_OPTIONS } from '../../constants/sort'

/** 下拉菜单的一项。`onSelect` 缺失即视为未接入（置灰并显示提示）。 */
export interface MenuItem {
  id: string
  label: string
  hint?: string
  disabled?: boolean
  /** 已选中态（用于「网格/列表」这类单选） */
  checked?: boolean
  onSelect?: () => void
  separatorBefore?: boolean
}

/**
 * 顶栏下拉菜单。
 *
 * ⚠️ `Esc` 要**吞掉**：图库模式下 `Esc` = 退出图库（见 `ManagerView`），
 * 而窗口级监听器是**子组件的 effect 先注册**（React 自下而上跑 effect），
 * 所以这里 `preventDefault()` 之后，`ManagerView` 的处理器只要检查
 * `e.defaultPrevented` 就会自动让路。否则「想关个菜单」会把人踢回看图模式。
 */
function MenuButton({ label, items }: { label: string; items: MenuItem[] }) {
  const [open, setOpen] = useState(false)
  const ref = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false)
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return
      e.preventDefault()
      setOpen(false)
    }
    window.addEventListener('mousedown', onDown)
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('mousedown', onDown)
      window.removeEventListener('keydown', onKey)
    }
  }, [open])

  return (
    <div className="pv-mg-menu" ref={ref}>
      <button
        type="button"
        className={`pv-mg-menubtn${open ? ' is-open' : ''}`}
        onClick={() => setOpen((v) => !v)}
        aria-haspopup="menu"
        aria-expanded={open}
      >
        {label}
      </button>
      {open && (
        <div className="pv-mg-dropdown" role="menu">
          {items.map((it) => (
            <div key={it.id}>
              {it.separatorBefore && <div className="pv-mg-sep" />}
              <button
                type="button"
                role="menuitem"
                className="pv-mg-menuitem"
                disabled={it.disabled || !it.onSelect}
                onClick={() => {
                  setOpen(false)
                  it.onSelect?.()
                }}
                title={!it.onSelect ? '待接入' : undefined}
              >
                <span className="pv-mg-menutick">{it.checked ? '✓' : ''}</span>
                <span className="pv-mg-menulabel">{it.label}</span>
                {it.hint && <span className="pv-mg-menuhint">{it.hint}</span>}
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}

const TIERS: { key: ThumbTier; label: string }[] = [
  { key: 'sm', label: '小' },
  { key: 'md', label: '中' },
  { key: 'lg', label: '大' },
  { key: 'xl', label: '特大' },
]

interface Props {
  searchInput: string
  onSearch: (v: string) => void
  /** 搜索只对已入库的内容有效；浏览模式（未入库）下置灰 */
  searchEnabled: boolean
  viewMode: ViewMode
  onViewMode: (v: ViewMode) => void
  sort: ManagerSort
  onSort: (s: ManagerSort) => void
  /**
   * 当前是不是「已入库」的范围。
   *
   * 浏览（未入库）模式下没有索引，`入库时间`/`评分` 这两项没有数据可排 ——
   * 置灰而不是让它静默失效（选了没反应比看不见更糟）。
   */
  libraryMode: boolean
  tier: ThumbTier
  onTier: (t: ThumbTier) => void
  busy: boolean
  /** 当前范围能不能重扫（只有「某个目录」才有意义） */
  canRescan: boolean
  /** 能不能多选（浏览模式不可） */
  canSelect: boolean
  hasSelection: boolean
  /** 有可另存的目标（选中了图片）才启用「文件 → 另存为…」 */
  canSaveAs: boolean
  onSaveAs: () => void
  onAddFolder: () => void
  onRescan: () => void
  onSelectAll: () => void
  onClearSelection: () => void
  onExit: () => void
  onDragMouseDown: (e: ReactMouseEvent) => void
  isMaximized: boolean
  onMinimize: () => void
  onToggleMaximize: () => void
  onClose: () => void
  /** 打开设置窗口（菜单栏「设置」按钮；2026-10-24 与浏览模式共用同一个窗口） */
  onOpenSettings: () => void
}

export function ManagerTopBar(props: Props) {
  const {
    searchInput,
    onSearch,
    searchEnabled,
    viewMode,
    onViewMode,
    sort,
    onSort,
    libraryMode,
    tier,
    onTier,
    busy,
    canRescan,
    canSelect,
    hasSelection,
    canSaveAs,
    onSaveAs,
    onAddFolder,
    onRescan,
    onSelectAll,
    onClearSelection,
    onExit,
    onOpenSettings,
    onDragMouseDown,
  } = props

  /** 排序菜单的视口坐标（`null` = 关着）。点在按钮下方展开。 */
  const [sortOpen, setSortOpen] = useState<{ x: number; y: number } | null>(null)
  /** 按钮上显示的当前排序（短名） */
  const sortLabel = SORT_OPTIONS.find((o) => o.value === sort)?.label ?? '排序'

  const menus: { label: string; items: MenuItem[] }[] = [
    {
      label: '文件',
      items: [
        { id: 'add', label: '添加文件夹…', hint: 'Ctrl+O', onSelect: onAddFolder },
        {
          id: 'saveas',
          // 没选中时禁用：这条作用于「选中的那几张」，不是整个目录
          label: '另存为…',
          disabled: !canSaveAs,
          onSelect: onSaveAs,
        },
        {
          id: 'rescan',
          label: '重新扫描当前目录',
          hint: 'F5',
          disabled: !canRescan || busy,
          onSelect: onRescan,
        },
        { id: 'exit', label: '返回看图', hint: 'Esc', separatorBefore: true, onSelect: onExit },
      ],
    },
    {
      label: '编辑',
      items: [
        { id: 'all', label: '全选', hint: 'Ctrl+A', disabled: !canSelect, onSelect: onSelectAll },
        {
          id: 'none',
          label: '取消选择',
          disabled: !hasSelection,
          onSelect: onClearSelection,
        },
      ],
    },
    {
      label: '查看',
      items: [
        { id: 'grid', label: '网格视图', checked: viewMode === 'grid', onSelect: () => onViewMode('grid') },
        { id: 'list', label: '列表视图', checked: viewMode === 'list', onSelect: () => onViewMode('list') },
        ...TIERS.map((t, i) => ({
          id: `tier-${t.key}`,
          label: `缩略图：${t.label}`,
          checked: tier === t.key,
          separatorBefore: i === 0,
          onSelect: () => onTier(t.key),
        })),
      ],
    },
    {
      label: '图像',
      items: [
        { id: 'print', label: '打印…' },
        { id: 'saveas', label: '另存为…' },
        { id: 'rotate', label: '旋转 / 裁剪' },
      ],
    },
    /*
     * ⛔ 「工具」「帮助」两个菜单已删除（2026-10-29 用户要求：没啥用）。
     *
     * 里面全是**没有 onSelect 的死条目** —— `MenuButton` 只调 `it.onSelect?.()`，
     * 点了完全没反应：
     *   工具 → AI 关键词与语义搜索 / 加密图库 / 首选项…（首选项有直接按钮了）
     *   帮助 → 快捷键速查 / 关于 PurePhotoExplorer
     *
     * 唯一有实现的「首选项…」也不需要：工具栏右侧已有直接的「设置」按钮。
     * 「快捷键速查」在看图模式的「帮助」按钮 + H/F1 快捷键里。
     * 将来真要做这些功能，直接加回菜单对象即可。
     */
  ]

  return (
    <header className="pv-mg-bar" onMouseDown={onDragMouseDown}>
      <LogoTile />
      <span className="pv-mg-brand">PurePhotoExplorer</span>

      <nav className="pv-mg-menubar">
        {menus.map((m) => (
          <MenuButton key={m.label} label={m.label} items={m.items} />
        ))}
        {/*
          「设置」是**直接按钮**而不是下拉：它只有一个动作（开设置窗口），
          做成一行下拉再点一次是多余的。紧跟在菜单栏后面。
        */}
        <button
          type="button"
          className="pv-mg-menubtn"
          /*
           * ⛔ 必须包一层 `() => ...`：直接写 `onClick={onOpenSettings}` 会把
           * MouseEvent 当成 `page` 传进去（签名是 `(page?: SettingsPage) => void`），
           * 于是设置窗口拿到的 page 是个对象 → 右栏一个页面都不匹配 → **整片空白**，
           * 左栏也没有任何一项高亮（2026-10-28 踩过）。
           */
          onClick={() => onOpenSettings()}
          title="设置（Ctrl+,）"
          aria-label="设置"
        >
          设置
        </button>
      </nav>

      <input
        className="pv-mg-search"
        placeholder={searchEnabled ? '搜索文件名 / 标签 / 地点 / 机型…' : '浏览模式不能搜索'}
        value={searchInput}
        onChange={(e) => onSearch(e.target.value)}
        disabled={!searchEnabled}
        title={
          searchEnabled
            ? '按文件名、标签名、地点名、相机品牌/机型/镜头筛选；空格分隔多个词 = 都要命中'
            : '先把目录右键「加入图库」才能搜索'
        }
        aria-label="搜索文件名、标签、地点与机型"
      />

      <span className="pv-mg-seg" role="group" aria-label="视图模式">
        {(
          [
            ['grid', '▦', '网格视图'],
            ['list', '☰', '列表视图'],
          ] as const
        ).map(([mode, glyph, tip]) => (
          <button
            type="button"
            key={mode}
            className={`pv-mg-icobtn${viewMode === mode ? ' is-on' : ''}`}
            onClick={() => onViewMode(mode)}
            title={tip}
            aria-pressed={viewMode === mode}
            aria-label={tip}
          >
            {glyph}
          </button>
        ))}
        
      </span>

      <span className="pv-mg-sortwrap">
        <span className="pv-mg-sortlabel">排序</span>
        <button
          type="button"
          className="pv-mg-sortbtn"
          aria-haspopup="menu"
          aria-expanded={sortOpen != null}
          aria-label="排序方式"
          // ⚠️ 菜单开着时再点按钮：window 的 mousedown 会先关掉菜单，
          // 紧接着 click 又把它打开 → 看起来「闪一下」。拦在这里最省事。
          onMouseDown={(e) => e.stopPropagation()}
          onClick={(e) => {
            const r = e.currentTarget.getBoundingClientRect()
            setSortOpen({ x: r.left, y: r.bottom + 4 })
          }}
        >
          <span>{sortLabel}</span>
          <span className="pv-mg-caret" aria-hidden>
            ▾
          </span>
        </button>
      </span>
      {sortOpen && (
        <ContextMenu
          key={`${sortOpen.x},${sortOpen.y}`}
          items={SORT_OPTIONS.map((o) => ({
            id: o.value,
            label: o.label,
            disabled: o.libraryOnly === true && !libraryMode,
            onSelect: () => onSort(o.value),
          }))}
          position={sortOpen}
          onClose={() => setSortOpen(null)}
          width={120}
        />
      )}

      <div className="pv-spacer" />

      <WindowControls
        isMaximized={props.isMaximized}
        onMinimize={props.onMinimize}
        onToggleMaximize={props.onToggleMaximize}
        onClose={props.onClose}
      />
    </header>
  )
}
