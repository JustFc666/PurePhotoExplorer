/**
 * 图库模式左侧栏：**文件浏览器**（我的电脑 → 盘符 → 目录）+ **图库列表**。
 *
 * ## ⭐⭐ 三条核心语义（与用户逐条确认过）
 *
 * 1. **浏览 ≠ 入库**：左键点目录 = `onBrowseDir`，临时预览。
 *    只直读磁盘列文件 + 现解码缩略图，**不建索引、不写缩略图缓存**。
 * 2. **「加入图库」三选一才算入库**：用文件夹名 / 自定义新图库名 / 加入已有图库。
 *    由 `onAddToLibrary` 落地：单层扫描写索引 + 归入某个图库。
 * 3. **启动默认停在「我的电脑」**：`scope.kind === 'none'`，主区不显示任何图片；
 *    点「我的电脑」这一行就回到这个状态。
 *
 * ⚠️ 树是**懒加载**的（见 `useFsTree`）：`C:\` 下可能有几万个子目录，
 * 一次性递归枚举会在网络盘上卡死。展开哪个节点才拉哪个节点的直接子目录。
 */

import { useCallback, useEffect, useRef, useState } from 'react'
import type { MouseEvent as ReactMouseEvent } from 'react'

import { ConfirmDialog } from '../chrome/ConfirmDialog'
import { ContextMenu, useContextMenu } from '../chrome/ContextMenu'
import type { FolderNode, LibraryItem, TagItem } from '../../ipc/manager'
import { FS_COMPUTER_KEY, fsResolveDir, type FsNode, type TreeIcons } from '../../ipc/browse'
import type { FsTreeApi } from '../../hooks/useFsTree'
import { useShellIcons } from '../../hooks/useShellIcons'
import type { LibChoice, Scope } from '../../hooks/useManager'
import { ArchiveIcon } from '../icons/ArchiveIcon'
import { FolderIcon } from '../icons/FolderIcon'
import { GalleryIcon } from '../icons/GalleryIcon'

function fmt(n: number): string {
  return n.toLocaleString('zh-CN')
}

/** 文件夹类节点（`dir` = 普通目录，`known` = 图片/文档/下载）。 */
function isFolderNode(node: FsNode): boolean {
  return node.kind === 'dir' || node.kind === 'known'
}

/**
 * 节点图标（**回退用**）：系统图标取不到时（非 Windows / 取图标失败）才用它。
 *
 * ⚠️ emoji 只是兜底。正常路径是 `iconSrc` 拿到的系统图标 —— 盘符图标能区分
 * 固定盘/可移动盘/光驱，emoji 只有一个「光盘」形状，观感也和资源管理器差很远。
 *
 * 文件夹不在这里兜底 —— 它走矢量 `FolderIcon`，不需要系统图标。
 */
function iconOf(node: FsNode): string {
  switch (node.kind) {
    case 'computer':
      return '🖥'
    case 'drive':
      return '💽'
    default:
      return ''
  }
}

/**
 * 节点图标（**首选**）：资源管理器同源的系统图标。
 *
 * ⭐ 文件夹返回 `null`：它用矢量 `FolderIcon` 画（见 `isFolderNode` 分支），
 * 不走系统图标、也不走位图 —— 位图在 16px 的树里和上百 px 的卡片里没法两头兼顾。
 */
function iconSrc(node: FsNode, icons: TreeIcons | null): string | null {
  if (node.kind === 'computer') return icons?.computer ?? null
  if (node.kind === 'drive') {
    // 盘符各用各的图标（系统盘/U 盘/光驱不一样），查不到就不显示
    return icons?.drives.find(([p]) => p === node.path)?.[1] ?? null
  }
  return null
}

/** 单个浏览器节点（递归渲染子节点）。 */
function FsRow({
  node,
  depth,
  tree,
  scope,
  onGoHome,
  onBrowseDir,
  onContextMenu,
  icons,
}: {
  node: FsNode
  depth: number
  tree: FsTreeApi
  scope: Scope
  onGoHome: () => void
  onBrowseDir: (path: string, name: string) => void
  onContextMenu: (e: ReactMouseEvent, node: FsNode) => void
  icons: TreeIcons | null
}) {
  const kids = tree.childrenOf(node)
  const expanded = tree.isExpanded(node)
  const loading = tree.isLoading(node)
  const isComputer = node.path === null
  /**
   * 有没有箭头：没加载过（`kids === null`）就先当作有 ——
   * 否则用户得点两次（第一次点开才发现其实有子目录）才看得到内容。
   * 加载完是空的才把箭头收起来。
   */
  /*
   * 压缩包**永远不给展开箭头**：我们不在包里模拟目录层级（进去就是扁平一层图），
   * 而 `kids === null`（还没加载过）那条「先当作有箭头」的规则会让它显示成可展开，
   * 点开一次才发现是空的。直接在这里排除掉。
   */
  const isArchive = node.kind === 'archive'
  const hasChevron = !isArchive && (isComputer || kids === null || kids.length > 0)
  // 「停在我的电脑」「已入库」「正在浏览」都算选中，用户才不会觉得点了没反应
  const onThisRow = isComputer
    ? scope.kind === 'none'
    : (scope.kind === 'folder' || scope.kind === 'browse') && scope.path === node.path

  const activate = () => {
    if (isComputer) {
      // 点「我的电脑」= 回主页（清空主区），顺带确保它展开着能看见盘符
      onGoHome()
      if (!expanded) tree.expand(node)
      return
    }
    onBrowseDir(node.path as string, node.name)
  }

  return (
    <>
      <div
        className={`pv-mg-row${onThisRow ? ' is-on' : ''}`}
        style={{ paddingLeft: 4 + depth * 12 }}
        role="button"
        tabIndex={0}
        title={node.path ?? '我的电脑'}
        onClick={activate}
        onContextMenu={(e) => onContextMenu(e, node)}
        // 目录可以**拖到图库行上**直接入库（「我的电脑」没有路径，不可拖）
        draggable={!isComputer}
        onDragStart={(e) => {
          if (!node.path) return
          e.dataTransfer.setData('text/plain', node.path)
          e.dataTransfer.effectAllowed = 'copy'
        }}
        onKeyDown={(e) => {
          if (e.key === 'Enter') activate()
        }}
      >
        <button
          type="button"
          className={`pv-mg-chev${hasChevron ? '' : ' is-leaf'}`}
          onClick={(e) => {
            // 别让点击冒泡到行上（否则展开的同时还会触发一次浏览）
            e.stopPropagation()
            if (hasChevron) tree.toggle(node)
          }}
          tabIndex={-1}
          aria-label={expanded ? '折叠' : '展开'}
        >
          {loading ? '·' : hasChevron ? (expanded ? '▾' : '▸') : ''}
        </button>
        <span className="pv-mg-ico-glyph" aria-hidden>
          {isFolderNode(node) ? (
            // 矢量文件夹图标：16px 的树里和大卡片里共用同一份形状
            <FolderIcon size={16} />
          ) : isArchive ? (
            // 压缩包：和网格卡片同一份矢量图，一眼和文件夹区分开
            <ArchiveIcon size={16} />
          ) : iconSrc(node, icons) ? (
            <img className="pv-mg-ico-img" src={iconSrc(node, icons)!} alt="" draggable={false} />
          ) : (
            iconOf(node)
          )}
        </span>
        <span className="pv-mg-rowname">{node.name}</span>
        {node.note && <span className="pv-mg-rownote">{node.note}</span>}
      </div>
      {expanded &&
        kids?.map((c) => (
          <FsRow
            key={c.path ?? c.name}
            node={c}
            depth={depth + 1}
            tree={tree}
            scope={scope}
            onGoHome={onGoHome}
            onBrowseDir={onBrowseDir}
            onContextMenu={onContextMenu}
            icons={icons}
          />
        ))}
    </>
  )
}

/**
 * 图库行 + 它的**二级**成员目录（用户 2026-10-18）：
 * 点**小三角** = 展开/折叠成员目录；点图库行本身 = 只选中图库。
 * 成员目录行悬停 `title` 显示**绝对路径**，点击直接打开该目录。
 */
function LibraryRow({
  lib,
  members,
  selected,
  expanded,
  dropTarget,
  onToggle,
  onSelect,
  onSelectFolder,
  onFolderMenu,
  onMenu,
  onDropPath,
}: {
  lib: LibraryItem
  members: FolderNode[]
  selected: boolean
  expanded: boolean
  /** 正被拖拽悬停（外部拖放进来的高亮） */
  dropTarget: boolean
  onToggle: () => void
  onSelect: () => void
  onSelectFolder: (f: FolderNode) => void
  onFolderMenu: (e: ReactMouseEvent, f: FolderNode) => void
  onMenu: (e: ReactMouseEvent, lib: LibraryItem) => void
  /** 把一个路径（内部拖来的目录 / 外部拖进来的文件夹）加进这个图库 */
  onDropPath: (path: string, lib: LibraryItem) => void
}) {
  // 成员目录按名字排，跟文件树一个观感（后端给的是 folder_id 序）
  const kids = [...members].sort((a, b) => a.name.localeCompare(b.name, 'zh-Hans-CN'))
  const hasKids = kids.length > 0
  /** 应用内部拖拽（文件树节点）悬停中 —— 与外部拖放共用一个高亮样式 */
  const [over, setOver] = useState(false)
  return (
    <>
      <div
        // ⭐ `data-pv-lib`：外部拖放（原生通道）靠它做命中测试，见 `hooks/libDrop.ts`
        data-pv-lib={lib.id}
        className={`pv-mg-row${selected ? ' is-on' : ''}${
          dropTarget || over ? ' is-drop' : ''
        }`}
        style={{ paddingLeft: 4 }}
        role="button"
        tabIndex={0}
        onClick={onSelect}
        onContextMenu={(e) => onMenu(e, lib)}
        onDragOver={(e) => {
          // 只接「纯文本路径」（内部拖的是树节点），外部文件拖放走原生通道
          if (!e.dataTransfer.types.includes('text/plain')) return
          e.preventDefault()
          e.dataTransfer.dropEffect = 'copy'
          setOver(true)
        }}
        onDragLeave={() => setOver(false)}
        onDrop={(e) => {
          setOver(false)
          const path = e.dataTransfer.getData('text/plain')
          if (!path) return
          e.preventDefault()
          onDropPath(path, lib)
        }}
        onKeyDown={(e) => {
          if (e.key === 'Enter') onSelect()
        }}
        title={lib.rule ?? lib.name}
      >
        <button
          type="button"
          className={`pv-mg-chev${hasKids ? '' : ' is-leaf'}`}
          onClick={(e) => {
            e.stopPropagation()
            if (hasKids) onToggle()
          }}
          tabIndex={-1}
          aria-label={expanded ? '折叠' : '展开'}
        >
          {hasKids ? (expanded ? '▾' : '▸') : ''}
        </button>
        <span className="pv-mg-ico-glyph" aria-hidden>
          {lib.kind === 'smart' ? (
            '✦'
          ) : (
            /* 普通图库用八瓣花图标（用户 2026-10-18 指定），替代原来的 📚 */
            <GalleryIcon size={16} />
          )}
        </span>
        <span className="pv-mg-rowname">{lib.name}</span>
        <span className="pv-mg-rowcount">{fmt(lib.count)}</span>
      </div>
      {expanded &&
        kids.map((f) => (
          <div
            key={f.id}
            className="pv-mg-row"
            style={{ paddingLeft: 16 }}
            role="button"
            tabIndex={0}
            onClick={() => onSelectFolder(f)}
            onContextMenu={(e) => onFolderMenu(e, f)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') onSelectFolder(f)
            }}
            title={f.mountPath ? `${f.mountPath}${f.relPath}` : f.relPath}
          >
            <span className="pv-mg-chev is-leaf" />
            <span className="pv-mg-ico-glyph" aria-hidden>
              <FolderIcon size={16} />
            </span>
            <span className="pv-mg-rowname">{f.name}</span>
            <span className="pv-mg-rowcount">{fmt(f.imageCount)}</span>
          </div>
        ))}
    </>
  )
}


/**
 * 「文件夹 / 图库」上下分区的比例（0..1，`tree` 占多少）。
 *
 * ⭐ 存**比例**不存像素：窗口一改大小，像素值就失效了（要么把图库区挤成 0，
 * 要么留一大片空白）。比例天然自适应。
 */
const TREE_PCT_KEY = 'pv.mg.sideTreePct'
const TREE_MIN = 0.2
const TREE_MAX = 0.8
const TREE_DEFAULT = 0.58

function readTreePct(): number {
  try {
    const v = Number(localStorage.getItem(TREE_PCT_KEY))
    if (Number.isFinite(v) && v >= TREE_MIN && v <= TREE_MAX) return v
  } catch {
    /* 隐私模式等读不到就用默认 */
  }
  return TREE_DEFAULT
}

interface Props {
  /**
   * 树状态 —— 由 [`ManagerView`] 持有并传下来。
   *
   * 为什么不在侧栏内部 `useFsTree()`：主区网格双击子文件夹进入时，
   * 也要能把树展开到那里（`revealTo`），树的状态必须两边共享。
   */
  tree: FsTreeApi
  /** 侧栏宽度（px），由主视图的拖拽条控制 */
  width: number
  libraries: LibraryItem[]
  /** 图库 → 成员目录（图库行的二级展开） */
  libMembers: Record<number, FolderNode[]>
  scope: Scope
  totalCount: number
  onGoHome: () => void
  onSelectAll: () => void
  onSelectLibrary: (id: number, name: string) => void
  onSelectAddedFolder: (f: FolderNode) => void
  /** 左键：临时预览（不入库） */
  onBrowseDir: (path: string, name: string) => void
  /** 加入图库；`lib` 决定归到哪 */
  onAddToLibrary: (path: string, folderName: string, lib: LibChoice) => void
  /** 「自定义新图库」：由上层弹输入框收名字 */
  onAskNewLibraryName: (path: string, folderName: string) => void
  onRemoveFromLibrary: (f: FolderNode) => void
  /** 地址栏跳转（已入库的目录会自动走索引那条路） */
  onGotoDir: (path: string, name: string) => void
  /**
   * 点「图库」标题右边的 `+`：建一个**空的**图库。
   *
   * ⭐ 名字由上层弹 `PromptDialog` 收（本组件不持有弹窗状态 ——
   * 弹窗要 portal 到 body，放这里会让侧栏多出一个 `:last-child`，
   * 把 `pv-mg-sec-lib` 的 flex 布局顶掉，见 `sidebarLayout.test.ts`）。
   */
  onCreateLibrary: () => void
  /** 图库行右键「添加文件夹…」：由上层弹系统选目录对话框 */
  onAddFolderToLibrary: (lib: LibraryItem) => void
  /** 图库行右键「重命名」：由上层弹输入框 */
  onRenameLibrary: (lib: LibraryItem) => void
  /** 图库行右键「删除图库」：由上层弹确认框（只删分组，不删照片） */
  onDeleteLibrary: (lib: LibraryItem) => void
  /**
   * 新建**智能图库**（`+` 右键的预设）：`rule` 是 `pv-lib` 的 DSL 文本。
   *
   * 为什么挂在 `+` 的右键上：侧栏只有这一个「新建」入口，再放一个按钮会挤；
   * 而智能图库是进阶功能，藏在右键里正合适（tooltip 里有提示）。
   */
  onCreateSmartLibrary: (name: string, rule: string) => void
  /** 正被外部拖拽悬停的图库 id（原生拖放通道算出来的，见 `hooks/libDrop.ts`） */
  dropLibId: number | null
  /** 把拖来的目录加进这个图库 */
  onDropPathOnLibrary: (path: string, lib: LibraryItem) => void
  /** 全部标签 + 使用次数（侧栏「标签」分区） */
  tags: TagItem[]
  /** 点标签 = 只看这个标签的图 */
  onSelectTag: (name: string) => void
  /** 右键标签：改名 */
  onRenameTag: (t: TagItem) => void
  /** 右键标签：删除整个标签 */
  onDeleteTag: (t: TagItem) => void
}

/** 地址栏的悬停说明 —— 右键菜单是隐形的，得有个地方告诉用户它能干什么 */
const ADDR_HINT =
  '输入或粘贴文件夹路径后回车即可到达。\n' +
  '· 支持 C:\\照片、E:/照片，也可以只写盘符 E:\n' +
  '· 粘贴一张图片的路径会落到它所在的文件夹\n' +
  '· 已加入图库的目录会直接打开（有缩略图缓存）\n' +
  '· 想入库：在下面的目录上右键 →「加进图库」'

/** 右键菜单的作用对象：文件浏览器里的目录 /「已加入的目录」里的目录 / 图库行本身。 */
type CtxTarget =
  | { kind: 'fs'; node: FsNode }
  | { kind: 'folder'; node: FolderNode }
  | { kind: 'library'; lib: LibraryItem }
  /** 标签行的右键（改名 / 删除标签） */
  | { kind: 'tag'; tag: TagItem }
  /** 「图库 +」右键：新建智能图库的预设菜单 */
  | { kind: 'newsmart' }

export function ManagerSidebar({
  tree,
  width,
  libraries,
  libMembers,
  scope,
  totalCount,
  onGoHome,
  onSelectAll,
  onSelectLibrary,
  onSelectAddedFolder,
  onBrowseDir,
  onAddToLibrary,
  onAskNewLibraryName,
  onRemoveFromLibrary,
  onGotoDir,
  onCreateLibrary,
  onAddFolderToLibrary,
  onRenameLibrary,
  onDeleteLibrary,
  onCreateSmartLibrary,
  dropLibId,
  onDropPathOnLibrary,
  tags,
  onSelectTag,
  onRenameTag,
  onDeleteTag,
}: Props) {
  const { expand } = tree
  const computer = tree.roots.find((n) => n.kind === 'computer')
  // 系统图标（资源管理器那一套）。模块级缓存，树里每行共用，不会重复取。
  const icons = useShellIcons()

  /**
   * 地址栏。焦点外始终显示**当前所在目录**；点树 / 切范围时同步过去。
   *
   * ⚠️ `currentDir` 是派生值，不能用它当初始 state 后就不管 ——
   * 用户点树换了目录，输入框得跟着走（否则地址栏会一直停在打开时的那个路径）。
   */
  const currentDir = scope.kind === 'folder' || scope.kind === 'browse' ? scope.path : ''
  const [addr, setAddr] = useState(currentDir)
  const [addrErr, setAddrErr] = useState<string | null>(null)
  /** 展开（显示成员目录）的图库 id 集合 */
  const [openLibs, setOpenLibs] = useState<ReadonlySet<number>>(new Set())

  useEffect(() => {
    setAddr(currentDir)
    setAddrErr(null)
  }, [currentDir])

  /** 回车：解析路径 → 展开树 → 切过去 */
  const gotoAddr = async () => {
    const raw = addr.trim()
    if (!raw) return
    setAddrErr(null)
    try {
      const r = await fsResolveDir(raw)
      if (!r) {
        setAddrErr('找不到这个路径')
        return
      }
      // 先把树展开到那里，用户能看见自己在哪一级
      await tree.revealTo(r.path)
      onGotoDir(r.path, r.name)
    } catch (e) {
      setAddrErr(e instanceof Error ? e.message : String(e))
    }
  }

  /** 默认把「我的电脑」展开，直接看到盘符 —— 点开「此电脑」该有的样子。 */
  useEffect(() => {
    if (computer) expand(computer)
  }, [computer, expand])

  /** 右键菜单：记下右键的是哪个节点（菜单项要按节点类型生成） */
  const ctx = useContextMenu()
  /**
   * 「文件夹 / 图库」分区的上下拖拽（2026-10-28 用户要求可调）。
   *
   * 存比例、不存像素：窗口缩放时比例天然自适应，不会把某一边挤成 0。
   * 拖动中直接 setState（每帧一次重渲染，两个分区都很轻，不需要 rAF 节流）。
   */
  const asideRef = useRef<HTMLElement>(null)
  const [treePct, setTreePct] = useState<number>(readTreePct)
  const [vResizing, setVResizing] = useState(false)
  const vDrag = useRef({ startY: 0, startPct: TREE_DEFAULT })
  const treePctRef = useRef(treePct)
  treePctRef.current = treePct

  const onHSplitDown = useCallback((e: ReactMouseEvent) => {
    e.preventDefault()
    vDrag.current = { startY: e.clientY, startPct: treePctRef.current }
    setVResizing(true)
  }, [])

  useEffect(() => {
    if (!vResizing) return
    const onMove = (e: MouseEvent) => {
      const h = asideRef.current?.clientHeight ?? 0
      if (h <= 0) return
      const next = vDrag.current.startPct + (e.clientY - vDrag.current.startY) / h
      setTreePct(Math.min(TREE_MAX, Math.max(TREE_MIN, next)))
    }
    const onUp = () => {
      setVResizing(false)
      try {
        localStorage.setItem(TREE_PCT_KEY, String(treePctRef.current))
      } catch {
        /* 存不进去就只对本会话生效 */
      }
    }
    window.addEventListener('mousemove', onMove)
    window.addEventListener('mouseup', onUp)
    return () => {
      window.removeEventListener('mousemove', onMove)
      window.removeEventListener('mouseup', onUp)
    }
  }, [vResizing])

  const [ctxTarget, setCtxTarget] = useState<CtxTarget | null>(null)
  /** 待确认「从图库移除」的目录。非 null 时显示自绘确认框。 */
  const [pendingRemove, setPendingRemove] = useState<FolderNode | null>(null)
  const openFsMenu = (e: ReactMouseEvent, node: FsNode) => {
    setCtxTarget({ kind: 'fs', node })
    ctx.open(e)
  }
  const openFolderMenu = (e: ReactMouseEvent, node: FolderNode) => {
    setCtxTarget({ kind: 'folder', node })
    ctx.open(e)
  }
  const openLibMenu = (e: ReactMouseEvent, lib: LibraryItem) => {
    setCtxTarget({ kind: 'library', lib })
    ctx.open(e)
  }
  const openTagMenu = (e: ReactMouseEvent, tag: TagItem) => {
    setCtxTarget({ kind: 'tag', tag })
    ctx.open(e)
  }
  const closeMenu = () => {
    setCtxTarget(null)
    ctx.close()
  }

  /**
   * 「从图库移除」的确认。
   *
   * ⚠️ 必须说清「只删索引与缓存、不删原图」—— 否则用户不敢点，
   * 或者反过来误以为照片被删了（这两个误解都很难受）。
   *
   * ⛔ 别再改回 `@tauri-apps/plugin-dialog` 的 `confirm`：它底层是
   * `message` + 自定义按钮，靠「返回的按钮 label === okLabel」判断，任何一环对不上
   * 就**静默返回 false**，而且 Promise 的 rejection 被 `void` 吞掉 —— 用户点了菜单
   * 什么都不会发生、也没有任何报错。2026-10-27「从图库移除一直不生效」正是这个原因
   * （改用自绘 `ConfirmDialog`）。
   */
  const askRemove = (f: FolderNode) => setPendingRemove(f)

  const ctxPath = ctxTarget?.kind === 'fs' ? ctxTarget.node.path : null

  return (
    <aside
      ref={asideRef}
      className="pv-mg-side"
      // 宽度由主视图的拖拽条控制（拖动时实时变，松手记进 localStorage）
      style={{ flex: `0 0 ${width}px` }}
    >
      <div className="pv-mg-sec pv-mg-sec-tree" style={{ flex: `0 0 ${treePct * 100}%` }}>
        {/*
          地址栏（取代原来的「文件夹」分区标题）。照资源管理器那样：
          直接敲路径回车就到。原来的「右键可加入图库」提示移到 title 里 ——
          用户明确不要那行字，但右键菜单是隐形的，总得有地方说明。
        */}
        <div className="pv-mg-addrbox">
          <input
            className={`pv-mg-addr${addrErr ? ' is-bad' : ''}`}
            value={addr}
            onChange={(e) => {
              setAddr(e.target.value)
              if (addrErr) setAddrErr(null)
            }}
            onKeyDown={(e) => {
              if (e.key === 'Enter') {
                e.preventDefault()
                void gotoAddr()
              } else if (e.key === 'Escape') {
                // 放弃编辑，回到当前所在目录（别让它冒泡去退图库模式）
                setAddr(currentDir)
                setAddrErr(null)
                e.currentTarget.blur()
              }
            }}
            placeholder="输入文件夹路径，回车到达"
            title={ADDR_HINT}
            spellCheck={false}
            aria-label="文件夹地址栏"
          />
        </div>
        {addrErr && <div className="pv-mg-addrerr">{addrErr}</div>}
        <div className="pv-mg-treelist">
          {tree.loadingRoots ? (
            <div className="pv-mg-hint">正在读取驱动器…</div>
          ) : tree.roots.length === 0 ? (
            <div className="pv-mg-hint">{tree.error ?? '读不到驱动器列表'}</div>
          ) : (
            tree.roots.map((n) => (
              <FsRow
                key={n.path ?? FS_COMPUTER_KEY}
                node={n}
                depth={0}
                tree={tree}
                scope={scope}
                onGoHome={onGoHome}
                onBrowseDir={onBrowseDir}
                onContextMenu={openFsMenu}
                icons={icons}
              />
            ))
          )}
        </div>
      </div>

      {/* 上下分区的拖拽条：平时几乎隐形，悬停/拖动时亮起来 */}
      <div
        className="pv-mg-hsplit"
        data-dragging={vResizing}
        onMouseDown={onHSplitDown}
        role="separator"
        aria-orientation="horizontal"
        aria-label="调整文件夹与图库的高度"
        title="拖动调整上下比例"
      />

      <div className="pv-mg-sec pv-mg-sec-lib">
        <div className="pv-mg-sech">
          {/*
            标题字号与树行一致（12.5px）；`+` 靠右 —— 正对下面每行「张数」那一列
            （用户 2026-10-18：加号放在下面数字的上方）。
          */}
          <span className="pv-mg-sectitle">图库</span>
          {/*
            新建空图库。`+` 常年可见（不是 hover 才出现）—— 侧栏没有别的入口，
            藏起来用户根本找不到；误点也无害，弹窗可以直接取消。
          */}
          <button
            type="button"
            className="pv-mg-secadd"
            title="左键：新建空图库\n右键：新建智能图库（今年 / 本月 / 四星以上 / 相机 RAW / 按年份）"
            aria-label="新建图库"
            onClick={onCreateLibrary}
            onContextMenu={(e) => {
              setCtxTarget({ kind: 'newsmart' })
              ctx.open(e)
            }}
          >
            ＋
          </button>
        </div>
        <div className="pv-mg-treelist">
          {/* ⭐ 图库在前，「全部照片」沉底（用户 2026-10-18）。不再有「自定义图库」分组标题。 */}
          {libraries.length === 0 && (
            <div className="pv-mg-hint">
              还没有图库。点右边的 ＋ 建一个空的；
              也可以在「文件夹」里右键目录 →「加进新图库…」，一步建库加照片。
            </div>
          )}
          {libraries.map((l) => {
            const on = scope.kind === 'library' && scope.id === l.id
            const kids = libMembers[l.id] ?? []
            const expanded = openLibs.has(l.id)
            return (
              <LibraryRow
                key={l.id}
                lib={l}
                members={kids}
                selected={on}
                expanded={expanded}
                dropTarget={dropLibId === l.id}
                onToggle={() =>
                  setOpenLibs((s) => {
                    const next = new Set(s)
                    if (next.has(l.id)) next.delete(l.id)
                    else next.add(l.id)
                    return next
                  })
                }
                onSelect={() => onSelectLibrary(l.id, l.name)}
                onSelectFolder={onSelectAddedFolder}
                onMenu={openLibMenu}
                onDropPath={onDropPathOnLibrary}
                onFolderMenu={openFolderMenu}
              />
            )
          })}

          {/*
            标签分区（2026-10-19）：放在「图库」和「全部照片」之间。
            点一下 = 只看这个标签的图；右键可改名/删除整个标签。
          */}
          <div className="pv-mg-taghead">
            <span className="pv-mg-tagtitle">标签</span>
            {tags.length > 0 && <span className="pv-mg-tagcount">{tags.length}</span>}
          </div>
          {tags.length === 0 ? (
            <div className="pv-mg-hint">选中照片后按 L 打标签</div>
          ) : (
            tags.map((t) => (
              <div
                key={t.id}
                className={`pv-mg-row${
                  scope.kind === 'tag' && scope.name === t.name ? ' is-on' : ''
                }`}
                style={{ paddingLeft: 4 }}
                role="button"
                tabIndex={0}
                title={`只看「${t.name}」的照片`}
                onClick={() => onSelectTag(t.name)}
                onContextMenu={(e) => openTagMenu(e, t)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') onSelectTag(t.name)
                }}
              >
                <span className="pv-mg-chev is-leaf" />
                <span className="pv-mg-ico-glyph" aria-hidden>
                  🏷
                </span>
                <span className="pv-mg-rowname">{t.name}</span>
                <span className="pv-mg-rowcount">{fmt(t.count)}</span>
              </div>
            ))
          )}

          {/* 内置的「全部照片」不是数据库里的行，是「不加任何条件」这个语义 */}
          <div
            className={`pv-mg-row${scope.kind === 'all' ? ' is-on' : ''}`}
            style={{ paddingLeft: 4 }}
            role="button"
            tabIndex={0}
            onClick={onSelectAll}
            onKeyDown={(e) => {
              if (e.key === 'Enter') onSelectAll()
            }}
          >
            <span className="pv-mg-chev is-leaf" />
            <span className="pv-mg-ico-glyph" aria-hidden>
              🖼
            </span>
            <span className="pv-mg-rowname">全部照片</span>
            <span className="pv-mg-rowcount">{fmt(totalCount)}</span>
          </div>
        </div>
      </div>

      {/*
        右键菜单。三个位置共用：
        - 文件浏览器里的目录 → 「加入图库」三选一 + 仅浏览
        - 「已加入的目录」里的目录 → 浏览 / 从图库移除
        - 图库行 → 添加文件夹 / 重命名 / 删除图库
      */}
      {ctx.state && ctxTarget && (
        <ContextMenu
          key={`${ctx.state.x},${ctx.state.y}`}
          position={{ x: ctx.state.x, y: ctx.state.y }}
          items={
            ctxTarget.kind === 'fs'
              ? [
                  {
                    id: 'add-by-name',
                    label: '加进图库：用文件夹名',
                    hint: `「${ctxTarget.node.name}」`,
                    disabled: ctxPath == null,
                    onSelect: () => {
                      if (ctxPath) {
                        onAddToLibrary(ctxPath, ctxTarget.node.name, { kind: 'byFolderName' })
                      }
                    },
                  },
                  {
                    id: 'add-new',
                    label: '加进新图库…',
                    hint: '自定义名称',
                    disabled: ctxPath == null,
                    onSelect: () => {
                      if (ctxPath) onAskNewLibraryName(ctxPath, ctxTarget.node.name)
                    },
                  },
                  {
                    id: 'add-existing',
                    label: '加进已有图库',
                    disabled: ctxPath == null,
                    // 子菜单：列出全部图库。一条都没有时给一条禁用的说明项，
                    // 而不是把父项禁用 —— 父项禁用了悬停也展不开，用户看不到原因。
                    children:
                      libraries.length > 0
                        ? libraries.map((l) => ({
                            id: `lib-${l.id}`,
                            label: l.name,
                            hint: `${fmt(l.count)} 张`,
                            onSelect: () => {
                              if (ctxPath) {
                                onAddToLibrary(ctxPath, ctxTarget.node.name, {
                                  kind: 'existing',
                                  id: l.id,
                                  name: l.name,
                                })
                              }
                            },
                          }))
                        : [{ id: 'none', label: '（还没有其他图库）', disabled: true }],
                  },
                  {
                    id: 'browse',
                    label: '仅浏览此目录',
                    hint: '不入库',
                    separatorBefore: true,
                    disabled: ctxPath == null,
                    onSelect: () => {
                      if (ctxPath) onBrowseDir(ctxPath, ctxTarget.node.name)
                    },
                  },
                ]
              : ctxTarget.kind === 'folder'
                ? [
                    {
                      id: 'open',
                      label: '浏览此目录',
                      onSelect: () => onSelectAddedFolder(ctxTarget.node),
                    },
                    {
                      id: 'remove',
                      label: '从图库移除…',
                      hint: '不删原图',
                      separatorBefore: true,
                      onSelect: () => askRemove(ctxTarget.node),
                    },
                  ]
                : ctxTarget.kind === 'library'
                  ? [
                    {
                      id: 'lib-add',
                      label: '添加文件夹…',
                      hint: '选一个目录加进来',
                      onSelect: () => onAddFolderToLibrary(ctxTarget.lib),
                    },
                    {
                      id: 'lib-rename',
                      label: '重命名图库…',
                      separatorBefore: true,
                      onSelect: () => onRenameLibrary(ctxTarget.lib),
                    },
                    {
                      id: 'lib-delete',
                      label: '删除图库…',
                      hint: `照片保留在「全部照片」里`,
                      onSelect: () => onDeleteLibrary(ctxTarget.lib),
                    },
                  ]
                : ctxTarget.kind === 'tag'
                  ? [
                      {
                        id: 'tag-rename',
                        label: '重命名标签…',
                        onSelect: () => onRenameTag(ctxTarget.tag),
                      },
                      {
                        id: 'tag-delete',
                        label: '删除标签…',
                        hint: '照片不动，只去掉标签',
                        separatorBefore: true,
                        onSelect: () => onDeleteTag(ctxTarget.tag),
                      },
                    ]
                  : [
                    // 「图库 +」右键：智能图库预设。规则由 `pv-lib` 的 DSL 表达，
                    // 建完是一个**每次打开都重新求值**的图库（照片加了自动进）。
                    {
                      id: 'smart-year',
                      label: `今年（${new Date().getFullYear()}）`,
                      hint: '智能图库',
                      onSelect: () =>
                        onCreateSmartLibrary(
                          `${new Date().getFullYear()} 年`,
                          `date:${new Date().getFullYear()}`,
                        ),
                    },
                    {
                      id: 'smart-month',
                      label: `本月（${new Date().getMonth() + 1} 月）`,
                      onSelect: () => {
                        const d = new Date()
                        const y = d.getFullYear()
                        const m = String(d.getMonth() + 1).padStart(2, '0')
                        onCreateSmartLibrary(`${y} 年 ${m} 月`, `date:${y}-${m}`)
                      },
                    },
                    {
                      id: 'smart-rating',
                      label: '四星以上',
                      hint: 'rating:>=4',
                      separatorBefore: true,
                      onSelect: () => onCreateSmartLibrary('四星以上', 'rating:>=4'),
                    },
                    {
                      id: 'smart-raw',
                      label: '相机 RAW',
                      hint: 'type:raw',
                      onSelect: () => onCreateSmartLibrary('相机 RAW', 'type:raw'),
                    },
                    {
                      id: 'smart-years',
                      label: '按年份各建一个',
                      hint: '近 5 年',
                      separatorBefore: true,
                      onSelect: () => {
                        const y = new Date().getFullYear()
                        for (let i = 0; i < 5; i += 1) {
                          const year = y - i
                          const name = `${year} 年`
                          // 已存在同名图库就跳过（后端也会挡，但没必要让它报错）
                          if (libraries.some((l) => l.name === name)) continue
                          onCreateSmartLibrary(name, `date:${year}`)
                        }
                      },
                    },
                  ]
          }
          onClose={closeMenu}
        />
      )}

      {/* 「从图库移除」确认框（自绘）。⛔ 别换回 plugin-dialog 的 confirm，见 askRemove 的注释 */}
      {pendingRemove && (
        <ConfirmDialog
          title="从图库移除"
          message={
            `从图库移除「${pendingRemove.name}」？\n\n` +
            '· 删除它在索引里的记录与对应的缩略图缓存\n' +
            '· 如果它下面还有子目录的记录，会一起移除\n' +
            '· 不会删除任何原始图片文件（磁盘上的照片原封不动）\n\n' +
            '以后随时可以右键「加入图库」再加回来。'
          }
          okLabel="移除"
          cancelLabel="取消"
          danger
          onOk={() => {
            const target = pendingRemove
            setPendingRemove(null)
            onRemoveFromLibrary(target)
          }}
          onCancel={() => setPendingRemove(null)}
        />
      )}
    </aside>
  )
}
