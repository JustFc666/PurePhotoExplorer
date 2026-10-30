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
  /**
   * 二级子菜单（**只支持一层**，和 `ContextMenu` 保持一致）。
   * 有 `children` 的项不响应点击，悬停向右展开。
   */
  children?: MenuItem[]
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
              {it.children ? (
                // 有子菜单的项：悬停向右展开（CSS `:hover` 控制显隐，见 manager.css）
                <div className="pv-mg-subwrap">
                  <button
                    type="button"
                    role="menuitem"
                    className="pv-mg-menuitem"
                    disabled={it.disabled}
                    aria-haspopup="menu"
                  >
                    <span className="pv-mg-menutick" />
                    <span className="pv-mg-menulabel">{it.label}</span>
                    {it.hint && <span className="pv-mg-menuhint">{it.hint}</span>}
                    <span className="pv-mg-subcaret">›</span>
                  </button>
                  <div className="pv-mg-submenu" role="menu">
                    {it.children.map((c) => (
                      <button
                        key={c.id}
                        type="button"
                        role="menuitem"
                        className="pv-mg-menuitem"
                        disabled={c.disabled || !c.onSelect}
                        onClick={() => {
                          setOpen(false)
                          c.onSelect?.()
                        }}
                      >
                        <span className="pv-mg-menutick">{c.checked ? '✓' : ''}</span>
                        <span className="pv-mg-menulabel">{c.label}</span>
                        {c.hint && <span className="pv-mg-menuhint">{c.hint}</span>}
                      </button>
                    ))}
                  </div>
                </div>
              ) : (
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
              )}
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
  /** 打开文件夹（只导航，不改变图库成员） */
  onOpenFolder: () => void
  /**
   * 当前正在看的目录（`null` = 不在某个目录里，比如「全部照片」/图库范围）。
   * 「新建图库 / 加入图库」作用于它 —— 没目录时这两项置灰。
   */
  dirPath: string | null
  dirName: string
  /** 用当前目录新建一个图库（对话框默认名字 = 目录名） */
  onNewLibraryFromDir: () => void
  /** 把当前目录加进指定的已有图库 */
  onAddDirToLibrary: (id: number, name: string) => void
  /** 已有图库（「加入图库 ▸」的子菜单） */
  libraries: { id: number; name: string }[]
  onRescan: () => void
  /** 打印选中的图（复用单图模式的版面预览） */
  onPrint: () => void
  /** 复制选中的图到文件夹（字节复制） */
  onCopyTo: () => void
  /** 移动选中的图到文件夹 */
  onMoveTo: () => void
  /** 删除选中的图（进回收站） */
  onDeleteFile: () => void
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
    onOpenFolder,
    dirPath,
    dirName,
    onNewLibraryFromDir,
    onAddDirToLibrary,
    libraries,
    onRescan,
    onPrint,
    onCopyTo,
    onMoveTo,
    onDeleteFile,
    onSelectAll,
    onClearSelection,
    onExit,
    onOpenSettings,
    onClose,
    onDragMouseDown,
  } = props

  /** 排序菜单的视口坐标（`null` = 关着）。点在按钮下方展开。 */
  const [sortOpen, setSortOpen] = useState<{ x: number; y: number } | null>(null)
  /** 按钮上显示的当前排序（短名） */
  const sortLabel = SORT_OPTIONS.find((o) => o.value === sort)?.label ?? '排序'

  const menus: { label: string; items: MenuItem[] }[] = [
    {
      /*
       * 文件菜单按「正常软件」的顺序排（用户 2026-10-30）：
       *   打开 / 加入 → 打印 → 复制·移动·删除 → 另存为 / 重扫 → 退出
       *
       * ⚠️ 现在的「打开文件夹…」是**入库入口**（用户 2026-10-30 定稿）：
       *   选完目录才弹出「新建图库 / 加入图库 ▸」，不再在菜单里摊一串
       *   「加进已有图库 · xxx」（图库一多就把菜单撑爆，还要每次改文案）。
       *   单纯浏览目录走侧栏「文件夹」树或地址栏。
       */
      label: '文件',
      items: [
        { id: 'open-folder', label: '打开文件夹…', hint: 'Ctrl+O', onSelect: onOpenFolder },
        /*
         * 「新建图库 / 加入图库」作用于**当前正在看的这个目录**（用户 2026-10-30）。
         * 这两条原来散在别处：新建图库在侧栏「图库 +」，加入图库在浏览模式标题栏
         * 那个紫色按钮 —— 都在视线之外。收进文件菜单，和"打开文件夹"挨着。
         */
        {
          id: 'new-lib',
          label: '新建图库',
          hint: dirPath ? dirName : '先打开一个文件夹',
          disabled: !dirPath,
          onSelect: onNewLibraryFromDir,
        },
        {
          id: 'add-to-lib',
          label: '加入图库',
          disabled: !dirPath,
          // 二级菜单只列图库名，不带张数（用户 2026-10-30 明确要求）
          children:
            libraries.length > 0
              ? libraries.map((l) => ({
                  id: `add-lib-${l.id}`,
                  label: l.name,
                  onSelect: () => onAddDirToLibrary(l.id, l.name),
                }))
              : [{ id: 'add-lib-none', label: '（还没有图库）', disabled: true }],
        },
        {
          id: 'print',
          label: '打印…',
          hint: 'Ctrl+P',
          // 打印的是**选中的那几张**（预览里可翻页），一张没选就无从下手
          disabled: !hasSelection,
          separatorBefore: true,
          onSelect: onPrint,
        },
        {
          id: 'copy-to',
          label: '复制到文件夹…',
          disabled: !hasSelection,
          onSelect: onCopyTo,
        },
        {
          id: 'move-to',
          label: '移动到文件夹…',
          disabled: !hasSelection,
          onSelect: onMoveTo,
        },
        {
          id: 'delete',
          label: '删除…',
          hint: 'Delete',
          disabled: !hasSelection,
          onSelect: onDeleteFile,
        },
        {
          id: 'saveas',
          // 没选中时禁用：这条作用于「选中的那几张」，不是整个目录
          label: '另存为…',
          disabled: !canSaveAs,
          separatorBefore: true,
          onSelect: onSaveAs,
        },
        {
          id: 'rescan',
          label: '重新扫描当前目录',
          hint: 'F5',
          disabled: !canRescan || busy,
          onSelect: onRescan,
        },
        // 「退出」= 关程序（原来这里只有「返回看图」，模式切换不该占文件菜单末尾）
        { id: 'quit', label: '退出', hint: 'Alt+F4', separatorBefore: true, onSelect: onClose },
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
        // 「返回看图」从「文件」搬到这里：它不是文件操作，是**切回单图模式**
        { id: 'exit', label: '返回看图', hint: 'Esc', separatorBefore: true, onSelect: onExit },
      ],
    },
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
