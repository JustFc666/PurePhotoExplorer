/**
 * 图库模式（S2 管理器）主视图 —— 组合根。
 *
 * 布局（对标设计稿的三栏 + 双底栏）：
 * ```
 * ┌ 顶栏：logo/标题 · 文件 编辑 查看 图像 工具 帮助 · 搜索 · 视图模式 · 排序 · − ☐ ✕ ┐
 * ├───────────┬───────────────────────────────────────────────────────────────────────┤
 * │ 左栏      │ 全部照片                                           已选 3 张 · 48.7 MB │
 * │ 文件夹    │ 16,682 张 · 按文件名排序                             [导出] [标记]     │
 * │  └ 树     │ [全部 16682][JPG 5393][RAW 6]…                                        │
 * │ 图库      │ ┌────┐┌────┐┌────┐┌────┐   ← 卡片网格（缩略图+勾选+文件名+元信息）   │
 * │  └ 列表   │ └────┘└────┘└────┘└────┘                                             │
 * ├───────────┴───────────────────────────────────────────────────────────────────────┤
 * │ 16,682 个项目 · 1.04 GB · C:\…                                                     │
 * └───────────────────────────────────────────────────────────────────────────────────┘
 * ```
 *
 * ## 两种模式的外观差异（有意为之，用户要能一眼看出「这还没入库」）
 * - **浏览模式**（左键点侧栏目录）：标题旁挂一个「未加入图库」徽标 + 一个加入按钮，
 *   **不显示四联筛选条**（未入库没有全库统计可比）；多选与导出**可用**
 *   （导出/转格式/摆正/打印是文件级操作，不需要索引行），评分/标记/删除/重命名不给。
 * - **入库模式**（右键 → 加入图库）：完整的筛选条 + 多选 + 已选统计 + 全部批量操作。
 *
 * ⚠️ 窗口是 `decorations: false` 自绘的，所以顶栏还要负责拖动窗口并放窗口按钮。
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { MouseEvent as ReactMouseEvent } from 'react'
import { confirm, open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog'

import { formatFileSize } from '../../ipc'
import type { DecodedImage } from '../../ipc'
import { fsThumbnail } from '../../ipc/browse'
import {
  managerAutoOrient,
  managerConvertImages,
  managerCopyToFolder,
  managerCreateLibrary,
  managerDeleteImages,
  managerExportImages,
  managerPrintImages,
  managerRenameImages,
  managerMoveToFolder,
  managerSaveAs,
  managerTransformImages,
  revealInExplorer,
  type BatchSummary,
  type TagItem,
  type ThumbTier,
} from '../../ipc/manager'
import { subscribeThumbPending } from '../../ipc/thumbQueue'
import { useFsTree } from '../../hooks/useFsTree'
import { useGridWindow } from '../../hooks/useGridWindow'
import { errText, useManager, type GridItem } from '../../hooks/useManager'
import { useLibDrop } from '../../hooks/libDrop'
import { ContextMenu, useContextMenu } from '../chrome/ContextMenu'
import type { ContextMenuItem } from '../chrome/ContextMenu'
import { FilterDropdown, FilterRow } from './FilterDropdown'
import { PromptDialog } from '../chrome/PromptDialog'
import { PrintPreview } from '../viewer/PrintPreview'
import { FolderIcon } from '../icons/FolderIcon'
import { ManagerSidebar } from './ManagerSidebar'
import { AiTagStrip } from './AiTagStrip'
import { TagPicker } from './TagPicker'
import { ManagerTopBar } from './ManagerTopBar'
import { PersonStrip } from './PersonStrip'
import { PersonWall, PERSON_POP_W } from './PersonWall'
import {
  managerImageTags,
  aiVocab,
  type VocabTerm,
} from '../../ipc/manager'
import { PhotoCard } from './PhotoCard'
import '../../styles/manager.css'

/** 缩略图档位 → 网格列宽（px）。CSS 用 `auto-fill + minmax` 排布。 */
const CARD_WIDTH: Record<ThumbTier, number> = {
  sm: 116,
  md: 168,
  lg: 236,
  xl: 320,
}

/**
 * 视口之后预取多少张。
 *
 * 30 是「大约两屏」的量：卡片最小档（116px）一屏约 40 张，最大档（320px）约 12 张，
 * 30 张在任何档位都够铺满下一屏。再多就是纯浪费 —— 用户根本没在滚的时候，
 * 多取的每一张都在和真正要看的图抢那 4 个槽位。
 */
const PREFETCH_AHEAD = 30

/**
 * 从一组行里取**可操作的绝对路径**（离线/卷未挂载的没有 `path`）。
 *
 * ⚠️ 不能只从「选中集合」取：**浏览模式（未入库）的每一行 `id` 都是 `null`**，
 * 而 `selectedItems` 的条件是 `id != null` —— 那集合在浏览模式永远是空的。
 * 之前导出/转格式全走它，于是文件夹浏览模式下右键一律报
 * 「这些图片没有可用的本地路径」，可实际上路径就在 `item.path` 上。
 */
function filePathsOf(items: GridItem[]): string[] {
  const out: string[] = []
  for (const it of items) {
    if (it.path) out.push(it.path)
  }
  return out
}

/** 后端格式名 → 筛选条上的显示名。 */
const FORMAT_LABEL: Record<string, string> = {
  jpeg: 'JPG',
  png: 'PNG',
  gif: 'GIF',
  webp: 'WebP',
  heic: 'HEIC',
  heif: 'HEIF',
  psd: 'PSD',
  bmp: 'BMP',
  tiff: 'TIFF',
  avif: 'AVIF',
  '?': '未知',
}

function fmtCount(n: number): string {
  return n.toLocaleString('zh-CN')
}

/* ── 侧栏宽度（可拖拽，记忆在 localStorage）────────────────── */
const SIDE_WIDTH_KEY = 'pv.mg.sideWidth'
const SIDE_MIN = 160
const SIDE_MAX = 480
const SIDE_DEFAULT = 208

/**
 * 侧栏「图库 +」弹出的新建图库**默认名**（用户 2026-10-18 指定）。
 *
 * ⭐ 打开即全选（见 `PromptDialog` 的 `el.select()`），直接敲字就覆盖，
 * 所以默认值只起「提示格式」的作用，不用怕它挡路。
 */
const DEFAULT_LIB_NAME = '新的图库'

function readSideWidth(): number {
  try {
    const raw = Number(localStorage.getItem(SIDE_WIDTH_KEY))
    if (Number.isFinite(raw) && raw >= SIDE_MIN && raw <= SIDE_MAX) return raw
  } catch {
    /* localStorage 不可用（隐私模式）就给默认值 */
  }
  return SIDE_DEFAULT
}

interface Props {
  /**
   * 当前是否处于图库模式。
   *
   * ⚠️ 图库视图**始终挂载**（切到单图查看时只 `display:none`）——
   * 卸载会把「当前在哪个文件夹 / 选了哪些图 / 滚到第几屏」全丢掉，
   * 用户从单图返回时会看到一个空白主页（原来的 bug）。
   * 代价是它的事件监听还挂着，所以要用这个开关把**键盘处理关掉**，
   * 否则单图查看时按 Esc 会顺手把图库模式也退了。
   */
  active: boolean
  /** 返回单图浏览模式 */
  onExit: () => void
  /**
   * 打开设置窗口。
   *
   * `page = 'ai'` 时直接落到「AI 分析」页 —— 标签条右侧那个齿轮用它，
   * 2026-10-24 起齿轮不再弹自己的小面板（设置统一到设置窗口，避免两份）。
   */
  onOpenSettings: (page?: 'ai') => void
  /** 双击缩略图：用单图浏览打开该绝对路径 */
  onOpenImage: (path: string) => void
  /**
   * 用**当前范围的图片列表**放幻灯片（右键「幻灯片放映」）。
   *
   * 列表顺序 = 主区当前的显示顺序；`startPath` 是右键点中的那一张。
   * 由 `App` 交给 `useImageLoader`，让「下一张」在图库范围内翻而不是只翻同目录。
   */
  onPlaySlideshow: (paths: string[], startPath?: string) => void
  onDragMouseDown: (e: ReactMouseEvent) => void
  isMaximized: boolean
  onMinimize: () => void
  onToggleMaximize: () => void
  onClose: () => void
}

export function ManagerView(props: Props) {
  const mgr = useManager()
  const { onDragMouseDown, onExit, onOpenImage, onPlaySlideshow } = props

  /**
   * 还在排队 / 正在生成的缩略图数量。
   *
   * 存在的意义是**让等待可解释**：首次访问某个目录时缩略图要一张张现解码，
   * 没有这个计数，界面就只是「一片空白」——与「卡死了」在观感上没有任何区别。
   */
  const [pendingThumbs, setPendingThumbs] = useState(0)

  useEffect(() => subscribeThumbPending(setPendingThumbs), [])

  /** 多选集合的**唯一真源在 `useManager`**（它会随范围/筛选变化清空）。 */
  const selected = useMemo(() => new Set(mgr.selectedKeys), [mgr.selectedKeys])

  /**
   * 选中项的**索引 id**（浏览模式的 key 是路径 → `Number()` 得 NaN → 被过滤）。
   * 标签只能打在索引里的行上。
   */
  const selectedIds = useMemo(
    () => [...selected].map((k) => Number(k)).filter((n) => Number.isInteger(n)),
    [selected],
  )
  /** `L` 键的处理函数读它（避免为了一个回调把监听器反复重挂） */
  const tagForRef = useRef<number[]>([])
  /** 正在打标签的对象（`null` = 浮层关着） */
  const [tagFor, setTagFor] = useState<number[] | null>(null)
  /** 正在改名的标签 */
  const [tagRenameFor, setTagRenameFor] = useState<{
    id: number
    name: string
    error?: string
  } | null>(null)

  /**
   * 当前范围的**图片路径列表**（放映用），顺序 = 主区显示顺序。
   *
   * ⚠️ 必须在这里先算好（`fileItems` 在文件后半段才声明，菜单的 useMemo
   * 依赖数组在渲染期求值，引用后面的 const 会踩 TDZ）。
   */
  const playPaths = useMemo(
    () =>
      mgr.items
        .filter((i) => i.kind === 'file' && !!i.path)
        .map((i) => i.path as string),
    [mgr.items],
  )

  /** 从当前范围放幻灯片；带 `startPath` 就从那一张开始 */
  const playSlideshow = useCallback(
    (startPath?: string) => {
      if (playPaths.length === 0) return
      onPlaySlideshow(playPaths, startPath)
    },
    [playPaths, onPlaySlideshow],
  )

  /**
   * 「加进新图库…」要收名字 —— 弹输入框。
   * Tauri 的 dialog 插件没有 prompt（只有 message/ask/confirm/open/save），
   * 所以用自己的 [`PromptDialog`]。
   */
  const [newLibFor, setNewLibFor] = useState<{ path: string; folderName: string } | null>(null)

  /**
   * 侧栏「图库 +」：建一个**空的**图库。
   *
   * ⭐ 弹窗**不随提交关闭** —— 建失败（最常见是重名）要留在输入框里让用户改名，
   * 关掉再点一次 `+` 很烦。只有成功才 `setCreateLib(null)`。
   * `name` 记的是「用户最后输入的名字」，失败后重开也用它当默认值。
   */
  const [createLib, setCreateLib] = useState<{ name: string; error?: string } | null>(null)

  const doCreateLibrary = async (name: string) => {
    try {
      await managerCreateLibrary(name)
      setCreateLib(null)
      // 侧栏的图库列表是 `mgr.libraries`，不刷新就看不到新建的那个
      await mgr.refreshLibraries()
    } catch (e) {
      setCreateLib({ name, error: errText(e) })
    }
  }

  /**
   * 图库行右键「重命名」。
   *
   * ⭐ 与「新建」同一个套路：**失败不关弹窗**（重名时留在输入框里改名）。
   */
  const [renameLib, setRenameLib] = useState<{
    id: number
    name: string
    error?: string
  } | null>(null)

  const doRenameLibrary = async (id: number, name: string) => {
    try {
      await mgr.renameLibrary(id, name)
      setRenameLib(null)
    } catch (e) {
      setRenameLib({ id, name, error: errText(e) })
    }
  }

  /** 图库行右键「添加文件夹…」：系统选目录对话框 → 加进这个图库。 */
  const doAddFolderToLibrary = async (id: number, libName: string) => {
    const picked = await openDialog({
      directory: true,
      multiple: false,
      title: `选择要加进「${libName}」的文件夹`,
    })
    if (typeof picked !== 'string') return
    const trimmed = picked.replace(/[\\/]+$/, '')
    const name = trimmed.split(/[\\/]/).pop() || picked
    await mgr.addFolderToLibraryById(picked, name, id, libName)
  }

  /** 图库行右键「删除图库…」：只删分组，照片留在「全部照片」里。 */
  const doDeleteLibrary = async (id: number, name: string) => {
    const ok = await confirm(
      `删除图库「${name}」？\n\n` +
        '· 只删这个分组本身（图库 + 它的成员关系）\n' +
        '· 里面的照片**仍然在「全部照片」里**，随时可以再挂到别的图库\n' +
        '· 不会删除任何原始图片文件（磁盘上的照片原封不动）',
      { title: '删除图库', kind: 'warning', okLabel: '删除', cancelLabel: '取消' },
    )
    if (!ok) return
    try {
      await mgr.deleteLibrary(id)
    } catch {
      /* 错误已经进 `mgr.error`，状态栏会显示 */
    }
  }

  /**
   * 树状态**提到这一层**（原来在侧栏里）。
   *
   * 为什么：从**主区网格**双击子文件夹进入时，侧栏树也得跟着展开/高亮 ——
   * 树的状态在侧栏内部的话，主区根本碰不到它。
   */
  const tree = useFsTree()

  // ── 缩略图预取：可见窗口之后的 30 张 ──
  const gridRef = useRef<HTMLDivElement>(null)
  /**
   * 「这是同一批图吗」的标识。
   *
   * 换目录 / 换图库 / 换排序后，旧的可见索引指向的根本不是同一批图 ——
   * 不重置的话，在 5000 张的图库里滚到底再切到只有 20 张的目录，
   * 那 20 张会被当成「窗口内」全部预取。
   */
  const gridDataKey = useMemo(() => {
    const s = mgr.scope
    const scope =
      s.kind === 'browse'
        ? `b:${s.path}`
        : s.kind === 'folder'
          ? `f:${s.id}`
          : s.kind === 'library'
            ? `l:${s.id}`
            : s.kind
    return `${scope}|${mgr.sort}`
  }, [mgr.scope, mgr.sort])
  const prefetchUntil = useGridWindow(
    gridRef,
    mgr.items.length,
    gridDataKey,
    PREFETCH_AHEAD,
  )

  // ── 侧栏宽度：拖拽中间那条缝，松手记到 localStorage，下次打开还是这个宽度 ──
  const [sideW, setSideW] = useState<number>(readSideWidth)
  const [resizing, setResizing] = useState(false)
  const drag = useRef({ startX: 0, startW: SIDE_DEFAULT })
  const sideWRef = useRef(sideW)
  sideWRef.current = sideW

  const onSplitDown = useCallback((e: ReactMouseEvent) => {
    e.preventDefault()
    drag.current = { startX: e.clientX, startW: sideWRef.current }
    setResizing(true)
  }, [])

  useEffect(() => {
    if (!resizing) return
    const { startX, startW } = drag.current
    const onMove = (e: MouseEvent) => {
      setSideW(Math.min(SIDE_MAX, Math.max(SIDE_MIN, startW + (e.clientX - startX))))
    }
    const onUp = () => {
      setResizing(false)
      try {
        localStorage.setItem(SIDE_WIDTH_KEY, String(sideWRef.current))
      } catch {
        /* 写不进去（隐私模式等）就只对本会话生效 */
      }
    }
    window.addEventListener('mousemove', onMove)
    window.addEventListener('mouseup', onUp)
    document.body.classList.add('is-col-resizing')
    return () => {
      window.removeEventListener('mousemove', onMove)
      window.removeEventListener('mouseup', onUp)
      document.body.classList.remove('is-col-resizing')
    }
  }, [resizing])

  /** 浏览 + 把树展开到那里（树展开失败不算错，主区照样切） */
  const browseWithReveal = useCallback(
    (path: string, name: string) => {
      void tree.revealTo(path).catch(() => {})
      void mgr.browseDir(path, name)
    },
    [mgr, tree],
  )

  /** 跳转（已入库走索引）+ 把树展开到那里 */
  const gotoWithReveal = useCallback(
    (path: string, name: string) => {
      void tree.revealTo(path).catch(() => {})
      void mgr.gotoDir(path, name)
    },
    [mgr, tree],
  )

  /** 网格共用的「打开」：文件夹 → 进入；图片 → 单图查看 */
  const openItem = useCallback(
    (item: GridItem) => {
      if (item.kind === 'dir' && item.path) {
        gotoWithReveal(item.path, item.name)
        return
      }
      if (item.path) onOpenImage(item.path)
    },
    [gotoWithReveal, onOpenImage],
  )

  /**
   * `Esc` 退出图库模式。
   *
   * ⚠️ **必须在这里单独处理**：单图浏览的 `useShortcuts` 在图库模式下是
   * **整体禁用**的（`enabled: mode === 'viewer'`），所以那条链上的 `Esc`
   * 不会执行 —— 而顶栏「返回看图」的提示又向用户承诺了 `Esc`。
   *
   * ⚠️ `defaultPrevented` 检查不能删：顶栏下拉菜单 / 目录右键菜单的 `Esc`
   * 会先 `preventDefault()`（子组件 effect 先注册 → 先执行），那是「关菜单」
   * 而不是「离开图库」。不检查的话想关个菜单就会把人踢回看图模式。
   */
  const onExitRef = useRef(onExit)
  onExitRef.current = onExit
  /**
   * 打印预览 / 送印进行中时，`Esc` 归预览自己处理。
   *
   * 本组件的 keydown 比 `PrintPreview` 注册得**更早**，会先执行；
   * 不拦的话按 `Esc` 会「关预览」+「退出图库模式」同时发生。
   * 用 ref 读，避免把这个 effect 的依赖搞成每渲染一换。
   */
  const printOpenRef = useRef(false)
  useEffect(() => {
    // ⚠️ 非激活时**不注册**：图库视图是常驻挂载的（见 Props.active 的说明）
    if (!props.active) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape' || e.defaultPrevented) return
      if (printOpenRef.current) return
      const t = e.target as HTMLElement | null
      const tag = t?.tagName
      if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || t?.isContentEditable) return
      e.preventDefault()
      onExitRef.current()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [props.active])

  /**
   * `Ctrl/⌘ + A` 全选（当前结果集里的图片）。
   *
   * 顶栏「编辑 → 全选」上写着这个提示，但之前**没实现**——和 `F5` 是同一类问题：
   * 菜单承诺了快捷键，实际按下没反应。
   *
   * ⚠️ 输入框内不拦：搜索框里 `Ctrl+A` 是「选中刚打的字」。
   */
  const selectAllRef = useRef(mgr.selectAll)
  selectAllRef.current = mgr.selectAll
  useEffect(() => {
    if (!props.active) return
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || e.key.toLowerCase() !== 'a') return
      const t = e.target as HTMLElement | null
      const tag = t?.tagName
      if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || t?.isContentEditable) return
      e.preventDefault()
      selectAllRef.current()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [props.active])

  /**
   * `L` 给选中的照片打标签。
   *
   * 只作用在**已入库**的行（浏览模式的临时项 `id` 是 null，没有索引行可挂标签）。
   */
  tagForRef.current = selectedIds
  useEffect(() => {
    if (!props.active) return
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey || e.metaKey || e.altKey) return
      if (e.key.toLowerCase() !== 'l') return
      const t = e.target as HTMLElement | null
      if (t && ['INPUT', 'TEXTAREA', 'SELECT'].includes(t.tagName)) return
      if (t?.isContentEditable) return
      if (tagForRef.current.length === 0) return
      e.preventDefault()
      setTagFor(tagForRef.current)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [props.active])

  /**
   * `F5` 重扫当前范围。
   *
   * ⚠️ **图库模式原本根本没绑 F5**（`useShortcuts` 只在 `mode === 'viewer'` 生效），
   * 而顶栏菜单的「重新扫描当前目录」上写着「F5」—— 承诺了却没实现。
   * 外部往目录里放了新图按 F5 没反应，看起来就像「不刷新」。
   */
  const rescanRef = useRef(mgr.rescan)
  rescanRef.current = mgr.rescan
  useEffect(() => {
    if (!props.active) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'F5') return
      // 输入框里按 F5 是浏览器语义，不拦
      const t = e.target as HTMLElement | null
      if (t && ['INPUT', 'TEXTAREA', 'SELECT'].includes(t.tagName)) return
      e.preventDefault()
      void rescanRef.current()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [props.active])

  /**
   * **窗口重新获得焦点时自动重扫当前范围**。
   *
   * 这是「外部存了一张图，切回来看不见」的正解：用户从资源管理器复制回来，
   * 窗口一激活就自动扫一遍。不做文件系统监听 —— 那要常驻句柄，
   * 对移动硬盘/网络盘也不友好，而「切回来刷一次」已经覆盖绝大多数场景。
   *
   * ⚠️ 三条限制：
   * - `all`（全部照片）不自动扫：可能是几十个目录，切一次窗口就卡一下；
   * - 2 秒内不重复（焦点事件容易连发：点任务栏、点窗口、从对话框返回）；
   * - 卷未挂载的目录由 `rescan` 内部跳过。
   */
  const lastAutoScan = useRef(0)
  useEffect(() => {
    if (!props.active) return
    const onFocus = () => {
      if (mgr.scope.kind === 'none' || mgr.scope.kind === 'all') return
      const now = Date.now()
      if (now - lastAutoScan.current < 2000) return
      lastAutoScan.current = now
      void rescanRef.current()
    }
    window.addEventListener('focus', onFocus)
    return () => window.removeEventListener('focus', onFocus)
  }, [props.active, mgr.scope.kind])

  /** 对话框方式把某个目录加入图库（顶栏「文件 → 添加文件夹…」）。等价于「用文件夹名」那一项。 */
  const pickFolder = useCallback(async () => {
    const picked = await openDialog({
      directory: true,
      multiple: false,
      title: '选择要加入图库的文件夹',
    })
    if (typeof picked !== 'string') return
    const trimmed = picked.replace(/[\\/]+$/, '')
    const name = trimmed.split(/[\\/]/).pop() || picked
    await mgr.addFolderToLibrary(picked, name, { kind: 'byFolderName' })
  }, [mgr])

  /**
   * 单击选择（用户 2026-10-24 定稿）：
   * - **普通单击 = 加选**（连续点缩略图累加多选，不再需要按住 Ctrl）；
   * - **单击已选中的 = 取消本次全部选择**（比逐个移除更常用 —— 用户点错了
   *   想一键清空，而不是费劲把某一张摘出去；摘单张仍可用 Ctrl+单击）；
   * - Ctrl / Shift / Cmd 保持原来的切换语义。
   * 浏览模式不参与选择。
   */
  const toggle = useCallback(
    (item: GridItem, e: ReactMouseEvent) => {
      // 文件夹行不参与选择（它没有文件级操作）
      if (!mgr.canSelect || item.kind === 'dir') return
      if (e.ctrlKey || e.metaKey || e.shiftKey) mgr.toggleSelect(item.key)
      else if (selected.has(item.key)) mgr.clearSelection()
      else mgr.toggleSelect(item.key)
    },
    [mgr, selected],
  )

  const selectedItems = mgr.items.filter((i) => selected.has(i.key))
  const selectedBytes = selectedItems.reduce((sum, i) => sum + (i.size ?? 0), 0)
  const selectedCount = selectedItems.length
  /** 选中项是否**全都**已打旗标（决定顶栏按钮是「标记」还是「取消标记」） */

  // ── 批量导出 ──
  const [exporting, setExporting] = useState(false)
  /** 送印中（禁用预览里的确认按钮，避免连点重复送印） */
  const [printing, setPrinting] = useState(false)
  /** 送印错误（显示在预览浮层里，不关浮层，用户可以改版面重试） */
  const [printError, setPrintError] = useState<string | undefined>(undefined)
  /**
   * 批量打印的待印队列 + 当前预览到第几张。
   *
   * `null` = 预览浮层关着。**版面参数对整批共用一套** —— 预览换图只换样本，
   * 不换设置（否则「预览的是这张的设置，印的是另一套」）。
   */
  const [printBatch, setPrintBatch] = useState<{ paths: string[]; index: number } | null>(null)
  /** 当前预览那张的解码图（走 fs_thumbnail 的 2048 档，够看清版面、又不至于解原图） */
  const [printSrc, setPrintSrc] = useState<DecodedImage | null>(null)

  // 预览/送印开着时，`Esc` 交给预览自己处理（见 `printOpenRef` 的说明）
  useEffect(() => {
    printOpenRef.current = printBatch !== null || printing
  }, [printBatch, printing])
  /**
   * 导出结果提示。`null` = 不显示。
   *
   * 用一条贴在主区底部的浮层，而不是 `alert`：批量导出后用户接着就要看结果，
   * 弹窗还得再点一次「确定」打断节奏。
   */
  const [exportNotice, setExportNotice] = useState<
    { tone: 'ok' | 'warn' | 'err'; text: string; detail?: string } | null
  >(null)
  /** 待重命名的目标集合（`null` = 对话框关着） */
  const [renameFor, setRenameFor] = useState<GridItem[] | null>(null)
  /**
   * 单张重命名的**原地编辑**（用户 2026-10-24：不弹对话框，直接在缩略图的
   * 文件名上改，文本全选）。只有一张且当前是网格视图时走这里；
   * 多张 / 列表视图仍走 `renameFor` 的模板对话框。
   * 提交回调 `doInlineRename` 在 `doRename` 旁边（要用 runBatch，声明顺序）。
   */
  const [inlineRename, setInlineRename] = useState<GridItem | null>(null)

  /**
   * 把文件夹**拖到图库行上** → 直接入库。
   *
   * 两个来源都走它：应用内部（文件浏览器树节点，HTML5 拖拽）和
   * 从资源管理器拖进来的（原生通道，见 `hooks/libDrop.ts`）。
   *
   * ⚠️ 拖进来的是**单个图片文件**时不入库 —— 那是「打开看图」的语义，
   * 别静默地把一个文件当目录扫。
   */
  const onDropPathOnLibrary = useCallback(
    (path: string, lib: { id: number; name: string }) => {
      if (/\.[A-Za-z0-9]{1,8}$/.test(path)) {
        setExportNotice({
          tone: 'warn',
          text: '请把文件夹拖到图库上（单张图片拖进来是「打开看图」）',
        })
        return
      }
      const trimmed = path.replace(/[\\/]+$/, '')
      const name = trimmed.split(/[\\/]/).pop() || path
      void mgr.addFolderToLibraryById(path, name, lib.id, lib.name)
    },
    [mgr, setExportNotice],
  )

  /** 原生拖放通道的命中测试（从资源管理器拖进来的文件夹） */
  const { dropLibId } = useLibDrop({
    active: props.active,
    onDropPath: (path, libId) => {
      const lib = mgr.libraries.find((l) => l.id === libId)
      if (lib) onDropPathOnLibrary(path, lib)
    },
  })

  // ── 照片卡片右键菜单 ──
  // ⚠️ 这两个声明必须放在**所有使用它们的 useCallback 之前**：
  // `doDelete` / `doRename` 的依赖数组里引用了 `cardCtxItems`，
  // 放到后面会触发 TS2448「used before declaration」。
  const cardCtx = useContextMenu()

  /**
   * 卡片「标签」按钮弹出的小菜单：标签名 + 置信度。
   *
   * 自己管一份位置/项，不复用 `useContextMenu` —— 因为数据是**异步**取的，
   * 先弹「读取中…」再替换成真列表，体验上比"点了没反应 200ms"好。
   */
  const [tagPop, setTagPop] = useState<{ x: number; y: number; items: ContextMenuItem[] } | null>(
    null,
  )



  /* ── AI 打标配置与词表（2026-10-21）──
     词表只读一次（385 条，几 KB）；配置在打开面板时刷一次。 */
  const [vocabTerms, setVocabTerms] = useState<VocabTerm[]>([])
  /** 人像墙浮层的锚点（null = 关闭）。2026-10-25 用户定稿：用浮层，不新开页面 */
  const [personPop, setPersonPop] = useState<{ x: number; y: number } | null>(null)

  // ⚠️ 必须等 `mgr.ready`（= manager_open 完成）再拉：先挂载就调会报
  //    「管理器尚未打开」，词表拿不到 → 标签全归到「EXIF / 其他」。
  useEffect(() => {
    if (!mgr.ready) return
    let alive = true
    aiVocab()
      .then((v) => {
        if (!alive) return
        setVocabTerms(v)
      })
      .catch((e) => {
        if (alive) console.warn('词表读取失败', e)
      })
    return () => {
      alive = false
    }
  }, [mgr.ready])

  /** 词条 → 类目映射（标签条两级钻取用） */
  const vocabMap = useMemo(
    () => new Map(vocabTerms.map((t) => [t.term, { l1: t.l1, l2: t.l2 }])),
    [vocabTerms],
  )

  /** 全部一级名（用来区分"这条标签是一级还是二级"） */
  const l1Names = useMemo(() => new Set(vocabTerms.map((t) => t.l1)), [vocabTerms])
  /** 全部一级（按词表出现顺序）—— 标签条要铺满，没有图的显示 0 */
  const allL1 = useMemo(() => {
    const out: string[] = []
    for (const v of vocabTerms) if (!out.includes(v.l1)) out.push(v.l1)
    return out
  }, [vocabTerms])

  /**
   * 卡片「标签」按钮：弹出这张图的标签。
   *
   * 版式（用户 2026-10-22 定）：
   * ```
   * 标签 · 9 个
   * 自然            0.45      ← 一级：带自己的置信度
   * 　峡谷          0.42      ← 二级：缩进
   * 　河流          0.41
   * ──
   * 相机            ASUS_I001DA
   * 地点            上海
   * ```
   * 一级行现在有分数了（打标时把一级也当标签写库，分数取「一级自身」与「组内最高二级」的较大者）。
   */
  const showTags = useCallback(
    async (id: number, e: React.MouseEvent) => {
      e.preventDefault()
      e.stopPropagation()
      const x = e.clientX
      const y = e.clientY
      setTagPop({ x, y, items: [{ id: 'loading', label: '读取标签…', disabled: true }] })
      try {
        const res = await managerImageTags(id)
        const all = Array.isArray(res) ? res : res.tags
        const camera = Array.isArray(res) ? null : res.camera
        const place = Array.isArray(res) ? null : res.place

        if (all.length === 0 && !camera && !place) {
          setTagPop({
            x,
            y,
            items: [{ id: 'none', label: '这张图没有匹配到任何标签', disabled: true }],
          })
          return
        }

        // 拆成「一级标签」和「二级标签」
        const l1Tags = new Map<string, number | null>()
        const leaves: { name: string; l1: string; conf: number | null; src: string }[] = []
        for (const t of all) {
          if (t.source === 'exif') continue
          const v = vocabMap.get(t.name)
          if (l1Names.has(t.name) && !v) {
            l1Tags.set(t.name, t.confidence)
          } else {
            leaves.push({
              name: t.name,
              l1: v?.l1 ?? (t.source === 'ocr' ? '文字' : '其他'),
              conf: t.confidence,
              src: t.source,
            })
          }
        }

        // 按一级归组；没有一级标签的组补一个"无分数"的一级行
        const groups = new Map<string, typeof leaves>()
        for (const t of leaves) {
          const arr = groups.get(t.l1) ?? []
          arr.push(t)
          groups.set(t.l1, arr)
        }
        const order = [...groups.keys()].sort(
          (a, b) => (l1Tags.get(b) ?? -1) - (l1Tags.get(a) ?? -1),
        )
        // 有一级标签但没有任何二级的，也要显示
        for (const name of l1Tags.keys()) if (!groups.has(name)) order.push(name)

        const total = all.filter((t) => t.source !== 'exif').length + (camera ? 1 : 0) + (place ? 1 : 0)
        const items: ContextMenuItem[] = [{ id: 'hdr', label: `标签 · ${total} 个`, disabled: true }]

        let first = true
        for (const l1 of order) {
          const score = l1Tags.get(l1)
          items.push({
            id: `l1-${l1}`,
            label: l1,
            hint: score != null ? score.toFixed(2) : '',
            separatorBefore: !first,
            disabled: true,
          })
          first = false
          for (const [i, t] of (groups.get(l1) ?? []).entries()) {
            items.push({
              id: `t-${l1}-${i}`,
              label: `　${vocabMap.get(t.name)?.l2 ?? t.name}`,
              hint:
                t.conf != null
                  ? t.conf.toFixed(2)
                  : t.src === 'ocr'
                    ? '文字'
                    : t.src === 'face'
                      ? '人脸'
                      : '手动',
              onSelect: () => setTagPop(null),
            })
          }
        }

        // EXIF：相机 / 地点，右列是值（不是分数）
        for (const [k, [label, value]] of (
          [
            ['相机', camera],
            ['地点', place],
          ] as const
        ).entries()) {
          if (!value) continue
          items.push({
            id: `exif-${k}`,
            label,
            hint: value,
            separatorBefore: k === 0,
            onSelect: () => setTagPop(null),
          })
        }

        setTagPop({ x, y, items })
      } catch {
        setTagPop({ x, y, items: [{ id: 'err', label: '读取标签失败', disabled: true }] })
      }
    },
    [vocabMap, l1Names],
  )
  /** 右键作用的对象：右键那一刻的选中集合（菜单打开后即使选区变了也不影响已弹出的项） */
  const [cardCtxItems, setCardCtxItems] = useState<GridItem[]>([])

  /** 提示自动消失（错误多留一会儿，够看清） */
  useEffect(() => {
    if (!exportNotice) return
    const ms = exportNotice.tone === 'ok' ? 4000 : 9000
    const t = window.setTimeout(() => setExportNotice(null), ms)
    return () => window.clearTimeout(t)
  }, [exportNotice])

  /** 顶栏按钮用：当前**选中**的行 */
  const selectedPaths = useMemo(() => filePathsOf(selectedItems), [selectedItems])

  const doExport = useCallback(async (paths: string[]) => {
    if (exporting) return
    if (paths.length === 0) {
      setExportNotice({ tone: 'warn', text: '这些图片没有可用的本地路径（卷可能未挂载）' })
      return
    }
    const picked = await openDialog({
      directory: true,
      multiple: false,
      title: `导出 ${paths.length} 张图片到文件夹`,
    })
    if (typeof picked !== 'string') return

    setExporting(true)
    try {
      const r = await managerExportImages(paths, picked)
      const tone = r.failed.length > 0 ? 'warn' : r.skipped > 0 ? 'warn' : 'ok'
      let text = `已导出 ${r.copied} 张 → ${r.destDir}`
      if (r.skipped > 0) text += `（${r.skipped} 张因目标就是源目录已跳过）`
      if (r.failed.length > 0) text += `（${r.failed.length} 张失败）`
      setExportNotice({
        tone,
        text,
        detail:
          r.failed.length > 0
            ? r.failed
                .slice(0, 4)
                .map((f) => `${f.name}：${f.reason}`)
                .join('\n') +
              (r.failed.length > 4 ? `\n…另有 ${r.failed.length - 4} 张` : '')
            : undefined,
      })
    } catch (e) {
      setExportNotice({
        tone: 'err',
        text: '导出失败',
        detail: e instanceof Error ? e.message : String(e),
      })
    } finally {
      setExporting(false)
    }
  }, [exporting])

  /** 把某个路径复制到剪贴板（多选时每行一个） */
  const copyPaths = useCallback(async (paths: string[]) => {
    try {
      await navigator.clipboard.writeText(paths.join('\r\n'))
      setExportNotice({
        tone: 'ok',
        text: paths.length > 1 ? `已复制 ${paths.length} 个路径` : '已复制路径',
      })
    } catch {
      setExportNotice({ tone: 'err', text: '复制到剪贴板失败' })
    }
  }, [])

  /** 把批量结果统一成一条提示（转格式 / 摆正 / 打印共用）。 */
  const showBatchResult = useCallback((title: string, r: BatchSummary) => {
    const tone = r.failed.length > 0 ? 'warn' : 'ok'
    const parts = [`${title}：完成 ${r.done} 张`]
    if (r.skipped > 0) parts.push(`跳过 ${r.skipped} 张`)
    if (r.failed.length > 0) parts.push(`失败 ${r.failed.length} 张`)
    if (r.destDir) parts.push(`→ ${r.destDir}`)
    setExportNotice({
      tone,
      text: parts.join(' · '),
      detail:
        r.failed.length > 0
          ? r.failed
              .slice(0, 4)
              .map((f) => `${f.name}：${f.reason}`)
              .join('\n') + (r.failed.length > 4 ? `\n…另有 ${r.failed.length - 4} 张` : '')
          : undefined,
    })
  }, [])

  /** 跑一个批量任务：统一锁按钮、统一捕错。 */
  const runBatch = useCallback(
    async (title: string, task: () => Promise<BatchSummary>) => {
      if (exporting) return
      setExporting(true)
      try {
        showBatchResult(title, await task())
      } catch (e) {
        setExportNotice({
          tone: 'err',
          text: `${title}失败`,
          detail: e instanceof Error ? e.message : String(e),
        })
      } finally {
        setExporting(false)
      }
    },
    [exporting, showBatchResult],
  )

  /**
   * 批量转格式：先选输出目录，再按目标格式重编码。
   *
   * ⚠️ 与「导出」并列而不是合并：导出是字节复制（无损、瞬间），
   * 转格式要整幅解码再编码（慢、JPEG 有损），两者代价差一个数量级。
   */
  const doConvert = useCallback(
    async (format: 'jpg' | 'png' | 'bmp', paths: string[]) => {
      if (paths.length === 0) {
        setExportNotice({ tone: 'warn', text: '这些图片没有可用的本地路径' })
        return
      }
      const picked = await openDialog({
        directory: true,
        multiple: false,
        title: `把 ${paths.length} 张转换为 ${format.toUpperCase()}，选择输出文件夹`,
      })
      if (typeof picked !== 'string') return
      await runBatch(`转换为 ${format.toUpperCase()}`, () =>
        managerConvertImages(paths, picked, format),
      )
    },
    [runBatch],
  )

  /**
   * 批量「另存为」：用**系统原生的保存对话框**（用户更熟、能直接改目录和文件名）。
   *
   * ## 系统对话框只有一个文件名，多张时怎么命名
   * 单张 → 就用用户输入的名字。
   * 多张 → 把它当作**基名**，其余按 `基名_001`、`基名_002` 排下去
   * （序号位数按总数补零）。标题里写明这条规则，避免用户以为会覆盖。
   *
   * ## 格式由扩展名决定
   * 用户把后缀改成 `.png` 就转 PNG；保持原后缀就按原格式（后端**字节复制**，不重编码）。
   * 不在 jpg/png/bmp 白名单里的后缀（如 RAW 的 `.CR3`）→ 按「保持原格式」处理。
   */
  const doSaveAs = useCallback(
    async (paths: string[]) => {
      if (paths.length === 0) {
        setExportNotice({ tone: 'warn', text: '这些图片没有可用的本地路径' })
        return
      }
      const first = paths[0]!
      const picked = await saveDialog({
        title:
          paths.length > 1
            ? `另存为（共 ${paths.length} 张，其余按「名称_序号」命名）`
            : '另存为',
        // 预填第一张的原名与所在目录，用户不改就是「复制一份同名文件」
        defaultPath: first,
        filters: [
          { name: 'JPEG', extensions: ['jpg', 'jpeg'] },
          { name: 'PNG', extensions: ['png'] },
          { name: 'BMP', extensions: ['bmp'] },
        ],
      })
      if (typeof picked !== 'string') return

      // 拆出目录 / 文件名 / 扩展名（Windows 与 POSIX 分隔符都认）
      const sep = Math.max(picked.lastIndexOf('\\'), picked.lastIndexOf('/'))
      const dir = sep >= 0 ? picked.slice(0, sep) : ''
      if (!dir) {
        setExportNotice({ tone: 'err', text: '无法从保存路径确定目标文件夹' })
        return
      }
      const full = sep >= 0 ? picked.slice(sep + 1) : picked
      const dot = full.lastIndexOf('.')
      const base = dot > 0 ? full.slice(0, dot) : full
      const ext = dot > 0 ? full.slice(dot + 1).toLowerCase() : ''
      const fmt: 'jpg' | 'png' | 'bmp' | null =
        ext === 'jpg' || ext === 'jpeg' ? 'jpg' : ext === 'png' ? 'png' : ext === 'bmp' ? 'bmp' : null

      await runBatch('另存为', () =>
        managerSaveAs(paths, {
          destDir: dir,
          format: fmt,
          // 多张：基名 + 序号；单张：用用户给的完整文件名
          nameTemplate: paths.length > 1 ? `${base}_{n}` : null,
          fileName: paths.length === 1 ? full : null,
        }),
      )
    },
    [runBatch],
  )

  /**
   * 按 EXIF 方向摆正。
   *
   * `overwrite = true` 会**改写原文件** —— 必须二次确认，且只在用户明确选择时走这条路。
   */
  const doAutoOrient = useCallback(
    async (overwrite: boolean, paths: string[]) => {
      if (paths.length === 0) {
        setExportNotice({ tone: 'warn', text: '这些图片没有可用的本地路径' })
        return
      }
      let dest: string | null = null
      if (overwrite) {
        const ok = await confirm(
          `按 EXIF 方向摆正并**写回原文件**？\n\n` +
            `· 共 ${paths.length} 张，只处理方向需要旋转（90°/180°）的\n` +
            `· 会重新编码并覆盖原文件，**不可撤销**\n` +
            `· 需要镜像的方向（2/4/5/7）会被跳过\n\n` +
            `建议先在副本上试一次。`,
          { title: '写回原文件', kind: 'warning', okLabel: '写回', cancelLabel: '取消' },
        )
        if (!ok) return
      } else {
        const picked = await openDialog({
          directory: true,
          multiple: false,
          title: '选择摆正后的输出文件夹',
        })
        if (typeof picked !== 'string') return
        dest = picked
      }
      await runBatch(overwrite ? '摆正并写回' : '摆正另存', () =>
        managerAutoOrient(paths, dest, overwrite),
      )
    },
    [runBatch],
  )

  /**
   * 批量打印：直接送印、不弹系统对话框。
   *
   * ⚠️ 没有取消机会（`quick = true`），所以必须先确认张数。
   */
  /**
   * 打印：**先开版面预览**（与单图模式同一套 `PrintPreview`），确认后才送印。
   *
   * 单张 = 预览只有一页；多选 = 预览可翻页、确认后逐张按同一版面送印。
   * ⛔ 不要退回「点一下就直接印」——用户没法确认版面，印歪了纸已经浪费了。
   */
  const doPrint = useCallback((paths: string[]) => {
    if (paths.length === 0) {
      setExportNotice({ tone: 'warn', text: '这些图片没有可用的本地路径' })
      return
    }
    setPrintError(undefined)
    setPrintSrc(null)
    setPrintBatch({ paths, index: 0 })
  }, [])

  /** 预览样本：换一张就重新取一张缩略图（2048 档够看清版面，不必解原图） */
  useEffect(() => {
    if (!printBatch) {
      setPrintSrc(null)
      return
    }
    let alive = true
    const p = printBatch.paths[printBatch.index]
    if (!p) return
    setPrintSrc(null)
    fsThumbnail(p, 'xl')
      .then((img) => {
        if (alive) setPrintSrc(img)
      })
      .catch(() => {
        if (alive) setPrintSrc(null)
      })
    return () => {
      alive = false
    }
  }, [printBatch])

  /**
   * 确认送印：把预览里选的版面原样交给后端，逐张打印。
   *
   * ⚠️ `quick` 由预览强制为 true（批量时每张弹一次系统对话框没法用），
   * 这里不再从参数里取。
   */
  const confirmBatchPrint = useCallback(
    async (opts: {
      paper: { widthMm: number; heightMm: number }
      fit: boolean
      orientation: string
      quality: string
      marginMm: number
      printer: string
    }) => {
      if (!printBatch || printing) return
      setPrinting(true)
      setPrintError(undefined)
      try {
        const r = await managerPrintImages(printBatch.paths, {
          fit: opts.fit,
          orientation: opts.orientation,
          quality: opts.quality,
          paperWmm: opts.paper.widthMm,
          paperHmm: opts.paper.heightMm,
          marginMm: opts.marginMm,
          printer: opts.printer || null,
        })
        showBatchResult('打印', r)
        setPrintBatch(null)
      } catch (e) {
        // 不关浮层：用户改个版面就能重试，不必从头再点一遍
        setPrintError(e instanceof Error ? e.message : String(e))
      } finally {
        setPrinting(false)
      }
    },
    [printBatch, printing, showBatchResult],
  )

  /**
   * 文件被改过（删除/重命名）之后刷新列表。
   *
   * ⚠️ 两种模式的数据来源不同，不能都只 `reload()`：
   * - 入库模式 → 重查索引（`queryVersion` 触发）；
   * - **浏览模式 → 数据来自 `fs_list_images`，`reload()` 在那儿是空操作**
   *   （查询 effect 对 browse 提前 return），必须重新列目录。
   */
  const refreshAfterMutation = useCallback(async () => {
    // 删除/重命名：行会消失或 key 会变，缩略图自然重取，**不需要**动版本号
    if (mgr.isBrowsing) await mgr.rescan()
    else mgr.reload()
  }, [mgr])

  /** 删除整个标签（照片不动，只拆挂载）。 */
  const doDeleteTag = async (t: TagItem) => {
    const ok = await confirm(
      `删除标签「${t.name}」？\n\n` +
        `· 只去掉这条标签（${t.count} 张照片会失去它）\n` +
        '· **照片本身不动**，也不会被移出图库\n' +
        '· 想只去掉某几张的标签，选中后用标签浮层点掉即可',
      { title: '删除标签', kind: 'warning', okLabel: '删除', cancelLabel: '取消' },
    )
    if (!ok) return
    try {
      await mgr.deleteTag(t.id)
    } catch {
      /* 错误已进 mgr.error */
    }
  }

  /** 标签改名（重名失败时留在输入框里让用户改）。 */
  const doRenameTag = async (id: number, name: string) => {
    try {
      await mgr.renameTag(id, name)
      setTagRenameFor(null)
    } catch (e) {
      setTagRenameFor({ id, name, error: errText(e) })
    }
  }

  /**
   * **复制**到文件夹：字节复制（不重新编码、保留原格式与 EXIF）。
   *
   * 与「导出」是同一条后端实现，区别只在于入口在这里、目标目录当场选。
   */
  const doCopyTo = useCallback(
    async (paths: string[]) => {
      if (paths.length === 0) {
        setExportNotice({ tone: 'warn', text: '这些图片没有可用的本地路径' })
        return
      }
      const picked = await openDialog({
        directory: true,
        multiple: false,
        title: `把 ${paths.length} 张复制到文件夹`,
      })
      if (typeof picked !== 'string') return
      await runBatch('复制到文件夹', () => managerCopyToFolder(paths, picked))
    },
    [runBatch],
  )

  /**
   * **移动**到文件夹：文件真的从原位置搬走。
   *
   * ⚠️ 比复制危险，必须二次确认，并说清「会从当前图库索引里移除」——
   * 用户看到照片从列表里消失时才知道是自己干的那件事。
   */
  const doMoveTo = useCallback(
    async (targets: GridItem[]) => {
      const withPath = targets.filter((i) => !!i.path)
      if (withPath.length === 0) {
        setExportNotice({ tone: 'warn', text: '这些图片没有可用的本地路径（卷可能未挂载）' })
        return
      }
      const picked = await openDialog({
        directory: true,
        multiple: false,
        title: `把 ${withPath.length} 张移动到文件夹`,
      })
      if (typeof picked !== 'string') return
      const ok = await confirm(
        `把 ${withPath.length} 张移动到：\n${picked}\n\n` +
          '· 文件会**真的从原位置搬走**（不是复制）\n' +
          '· 已入库的会同时从当前图库的索引与缩略图缓存里移除\n' +
          '· 目标目录里已有同名文件时自动加序号，不会覆盖',
        { title: '移动到文件夹', kind: 'warning', okLabel: '移动', cancelLabel: '取消' },
      )
      if (!ok) return
      await runBatch('移动到文件夹', () =>
        managerMoveToFolder(
          withPath.map((i) => ({ id: i.id, path: i.path })),
          picked,
        ),
      )
      // 源目录少了一批图 —— 必须刷新，否则列表里还留着已经不存在的行
      await refreshAfterMutation()
    },
    [runBatch, refreshAfterMutation],
  )

  /**
   * 批量删除：**移入回收站**（可撤销），再清索引与缓存。
   *
   * ⚠️ 必须说清三件事，否则用户不敢点或者误以为文件彻底没了：
   * 能还原、会清索引、卷未挂载的只能清索引。
   */
  const doDelete = useCallback(async () => {
    if (exporting) return
    // 浏览模式的项没有 id，但有 path —— 照样能删（只删文件，没有索引要清）
    const targets = cardCtxItems.filter((i) => i.path != null || i.id != null)
    if (targets.length === 0) {
      setExportNotice({ tone: 'warn', text: '这些项目既没有本地路径也没有索引记录' })
      return
    }
    const offline = targets.filter((i) => !i.path).length
    const withIndex = targets.filter((i) => i.id != null).length
    const ok = await confirm(
      `把 ${targets.length} 张移到回收站？\n\n` +
        `· 文件会**进入回收站**，之后可以从回收站还原\n` +
        (withIndex > 0
          ? `· 其中 ${withIndex} 张已入库，会同时从图库索引移除并清缩略图缓存\n`
          : '· 这些是文件夹里的文件（未入库），只删文件\n') +
        (offline > 0
          ? `· 其中 ${offline} 张所在卷未挂载，只能清索引、删不到文件\n`
          : '') +
        `\n⚠️ 清空回收站之后就无法恢复了。`,
      { title: '删除到回收站', kind: 'warning', okLabel: '移到回收站', cancelLabel: '取消' },
    )
    if (!ok) return

    setExporting(true)
    try {
      const r = await managerDeleteImages(
        targets.map((i) => ({ id: i.id, path: i.path })),
      )
      // 索引已经变了：清空选择 + 重查（已删的行必须真的消失）
      mgr.clearSelection()
      await refreshAfterMutation()
      const parts = [`已移入回收站 ${r.deletedFiles} 个文件`]
      if (r.removedIndex > 0) parts.push(`清理索引 ${r.removedIndex} 条`)
      if (r.offlineIndexOnly > 0) parts.push(`${r.offlineIndexOnly} 张仅清索引`)
      if (r.failed.length > 0) parts.push(`失败 ${r.failed.length} 张`)
      setExportNotice({
        tone: r.failed.length > 0 ? 'warn' : 'ok',
        text: parts.join(' · '),
        detail:
          r.failed.length > 0
            ? r.failed.map((f) => `${f.name}：${f.reason}`).join('\n')
            : '可以到系统回收站里还原',
      })
    } catch (e) {
      setExportNotice({
        tone: 'err',
        text: '删除失败',
        detail: e instanceof Error ? e.message : String(e),
      })
    } finally {
      setExporting(false)
    }
  }, [cardCtxItems, exporting, mgr])

  /**
   * 旋转 / 翻转并**写回原文件**。
   *
   * ## 为什么必须二次确认
   * 这是本页面唯一「直接改写用户原始文件」的操作，且不可撤销。
   * 另外 RAW / HEIC / WebP / TIFF 本来就写不回去，确认框里先说明白，
   * 免得用户以为「跳过」是程序坏了。
   *
   * ## 顺序
   * 后端是**先旋转、后翻转**；这里每种组合只发一次调用，所以顺序天然一致。
   */
  const doTransform = useCallback(
    async (
      paths: string[],
      opts: { rotate?: number; flipH?: boolean; flipV?: boolean },
      label: string,
    ) => {
      if (paths.length === 0) {
        setExportNotice({ tone: 'warn', text: '这些图片没有可用的本地路径' })
        return
      }
      const ok = await confirm(
        `把 ${paths.length} 张「${label}」并**写回原文件**？\n\n` +
          `· 会重新编码并覆盖原文件，**不可撤销**\n` +
          `· 只有 JPEG / PNG / BMP 能覆盖；RAW、HEIC、WebP、TIFF 会被跳过\n` +
          `· 想保留原图请先「另存为」一份\n\n` +
          `建议先在副本上试一次。`,
        { title: '旋转 / 翻转并保存', kind: 'warning', okLabel: '写回', cancelLabel: '取消' },
      )
      if (!ok) return
      await runBatch(label, () => managerTransformImages(paths, opts))
      // 文件被真的改写了（size / mtime 都变）：
      // 1) 只给**这几张**打版本号 —— 全量刷新会让 500 张卡片一起重取并闪烁
      // 2) 等重扫落地（索引/目录列表拿到新的 size、mtime）
      mgr.bumpThumbs(paths)
      await mgr.rescan()
    },
    [mgr, runBatch],
  )

  /**
   * 批量重命名：按模板展开新文件名，交给后端逐张执行。
   *
   * ## 模板规则
   * - `{name}` = 原文件名（不含扩展名）
   * - `{n}` = 序号（从 1 开始，位数按总数自动补零：9 张 → `1..9`，100 张 → `001..100`）
   * - **扩展名永远保留**（用户在模板里写破天也不会把 .jpg 弄丢）
   * - 模板里没有 `{n}` 时**自动补 `_{n}`**：否则 N 张会全叫同一个名字，
   *   除第一张外全部失败 —— 那不是用户想要的「批量重命名」
   */
  const doRename = useCallback(
    async (template: string) => {
      const items = renameFor
      setRenameFor(null)
      if (!items || items.length === 0) return
      // 浏览模式的行没有 id，但照样能改名（纯文件重命名）
      const rows = items
      if (rows.length === 0) return

      const hasIndex = /\{n\}/i.test(template)
      const tpl = hasIndex ? template : `${template}_{n}`
      const width = String(rows.length).length < 3 ? 3 : String(rows.length).length

      const payload = rows.map((it, i) => {
        const dot = it.name.lastIndexOf('.')
        const stem = dot > 0 ? it.name.slice(0, dot) : it.name
        const ext = dot > 0 ? it.name.slice(dot) : ''
        const n = String(i + 1).padStart(width, '0')
        // 非法字符换成下划线：Windows 上会让 rename 直接失败
        const body = tpl
          .replace(/\{name\}/gi, stem)
          .replace(/\{n\}/gi, n)
          .replace(/[\\/:*?"<>|]/g, '_')
          .trim()
        return { id: it.id, path: it.path, newName: (body || stem) + ext }
      })

      await runBatch('重命名', () => managerRenameImages(payload))
      // 文件名变了：入库模式改的是索引 rel_name，浏览模式改的是磁盘 —— 两边都要刷
      await refreshAfterMutation()
    },
    [mgr, renameFor, runBatch],
  )

  /**
   * 原地重命名提交（单张，见 `inlineRename`）：输入的是**去掉扩展名的主体**，
   * 扩展名始终保留；非法字符换成下划线（与批量重命名同一套规则）。
   */
  const doInlineRename = useCallback(
    async (item: GridItem, input: string) => {
      setInlineRename(null)
      const body = input.trim().replace(/[\\/:*?"<>|]/g, '_')
      if (!body) return
      const dot = item.name.lastIndexOf('.')
      const stem = dot > 0 ? item.name.slice(0, dot) : item.name
      const ext = dot > 0 ? item.name.slice(dot) : ''
      if (body === stem) return
      await runBatch('重命名', () =>
        managerRenameImages([{ id: item.id, path: item.path, newName: body + ext }]),
      )
      await refreshAfterMutation()
    },
    // 与 doRename 保持同一组依赖（refreshAfterMutation 随 mgr 稳定）
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [runBatch],
  )


  /**
   * 右键一张卡片。
   *
   * Windows 习惯：右键**未选中**的项 → 先把它变成唯一选中再弹菜单；
   * 右键**已选中**的项 → 保留整个多选（这样「导出 20 张」不会因为右键某一张就散掉）。
   */
  const onCardContextMenu = useCallback(
    (item: GridItem, e: ReactMouseEvent) => {
      const already = selected.has(item.key)
      let targets: GridItem[]
      if (already) {
        targets = mgr.items.filter((i) => selected.has(i.key))
      } else {
        if (mgr.canSelect) mgr.selectOnly(item.key)
        targets = [item]
      }
      setCardCtxItems(targets)
      cardCtx.open(e)
    },
    [cardCtx, mgr, selected],
  )

  /**
   * 右键菜单里点某个标签：**整批切换**（全有 → 去掉；否则 → 全部加上）。
   *
   * 为什么要先查一遍 `tagCountsFor`：菜单一打开就点，本地不知道这批里有几张
   * 已经带着这个标签 —— 猜错的话用户会以为"点了没反应"（其实是又加了一遍）。
   */
  const doToggleTag = useCallback(
    async (ids: number[], tag: TagItem) => {
      if (ids.length === 0) return
      const hits = await mgr.tagCountsFor(ids)
      const have = hits.find((h) => h.name === tag.name)?.count ?? 0
      if (have >= ids.length) await mgr.removeTag(ids, tag.id)
      else await mgr.addTags(ids, [tag.name])
    },
    [mgr],
  )

  const cardMenuItems = useMemo<ContextMenuItem[]>(() => {
    const withPath = cardCtxItems.filter((i) => !!i.path)
    /**
     * ⚠️ 右键菜单的批量操作必须用**右键作用对象**的路径，不能用「选中集合」：
     * 浏览模式（未入库）根本没有选中集合可用（`id` 全是 `null`）。
     */
    const paths = filePathsOf(cardCtxItems)
    const n = cardCtxItems.length
    /** 能打星的行（浏览模式的临时项没有索引 id） */
    const rateable = cardCtxItems.map((i) => i.id).filter((id): id is number => id != null)
    /** 多选时星数为「各不相同」就显示「混合」，避免谎报成第一张的值 */
    const ratings = new Set(cardCtxItems.map((i) => i.rating))
    const oneRating = ratings.size === 1 ? ([...ratings][0] ?? 0) : -1
    const ratingHint =
      oneRating < 0 ? '混合' : oneRating === 0 ? undefined : '★'.repeat(oneRating)
    const allFlagged = cardCtxItems.length > 0 && cardCtxItems.every((i) => i.flag > 0)

    return [
      {
        id: 'rating',
        label: n > 1 ? `评分（${n} 张）` : '评分',
        hint: ratingHint,
        disabled: rateable.length === 0,
        // 子菜单只支持一层（ContextMenu 的既定约束）
        children: [0, 1, 2, 3, 4, 5].map((v) => ({
          id: `rating-${v}`,
          label: v === 0 ? '清除星级' : '★'.repeat(v),
          hint: oneRating === v ? '当前' : undefined,
          onSelect: () => void mgr.setRating(rateable, v),
        })),
      },
      {
        id: 'flag',
        label: allFlagged ? '取消标记' : '标记为待处理',
        hint: allFlagged ? '●' : undefined,
        disabled: rateable.length === 0,
        onSelect: () => void mgr.setFlag(rateable, allFlagged ? 0 : 1),
      },
      {
        id: 'tag',
        label: n > 1 ? `标签（${n} 张）` : '标签',
        hint: 'L',
        disabled: rateable.length === 0,
        // 子菜单只支持一层：常用标签直接平铺，最后一项进浮层（新建 / 全部标签）
        children: [
          ...mgr.tags.slice(0, 8).map((t) => ({
            id: `tag-${t.id}`,
            label: t.name,
            hint: `${t.count} 张`,
            onSelect: () => void doToggleTag(rateable, t),
          })),
          {
            id: 'tag-more',
            label: mgr.tags.length > 0 ? '更多 / 新建标签…' : '新建标签…',
            separatorBefore: mgr.tags.length > 0,
            onSelect: () => setTagFor(rateable),
          },
        ],
      },
      {
        id: 'open',
        label: '在资源管理器中显示',
        separatorBefore: true,
        disabled: withPath.length === 0,
        onSelect: () => {
          const p = withPath[0]?.path
          if (p) {
            void revealInExplorer(p).catch((e: unknown) =>
              setExportNotice({
                tone: 'err',
                text: '定位失败',
                detail: e instanceof Error ? e.message : String(e),
              }),
            )
          }
        },
      },
      {
        id: 'copy-path',
        label: n > 1 ? `复制 ${n} 个文件路径` : '复制文件路径',
        disabled: withPath.length === 0,
        onSelect: () => void copyPaths(withPath.map((i) => i.path as string)),
      },
      {
        id: 'play',
        label: '幻灯片放映',
        hint: '从这张开始',
        separatorBefore: true,
        disabled: playPaths.length === 0,
        onSelect: () => playSlideshow(cardCtxItems[0]?.path ?? undefined),
      },
      {
        id: 'export',
        label: n > 1 ? `导出所选 ${n} 张…` : '导出到文件夹…',
        separatorBefore: true,
        disabled: exporting || withPath.length === 0,
        hint: exporting ? '处理中…' : undefined,
        onSelect: () => void doExport(paths),
      },
      {
        id: 'saveas',
        label: n > 1 ? `另存为…（${n} 张）` : '另存为…',
        disabled: exporting || withPath.length === 0,
        onSelect: () => void doSaveAs(paths),
      },
      {
        id: 'copy-to',
        label: '复制到文件夹…',
        disabled: exporting || withPath.length === 0,
        onSelect: () => void doCopyTo(paths),
      },
      {
        id: 'move-to',
        label: '移动到文件夹…',
        // ⚠️ 移动会真的把文件从原位置搬走，破坏性比复制大 —— 菜单里排在复制后面
        disabled: exporting || withPath.length === 0,
        onSelect: () => void doMoveTo(cardCtxItems),
      },
      {
        id: 'transform',
        label: '旋转 / 翻转并保存',
        disabled: exporting || withPath.length === 0,
        // 第二层只能是叶子项（ContextMenu 只支持一层子菜单）
        children: [
          {
            id: 'tf-cw',
            label: '顺时针 90°',
            onSelect: () => void doTransform(paths, { rotate: 1 }, '顺时针旋转 90°'),
          },
          {
            id: 'tf-ccw',
            label: '逆时针 90°',
            onSelect: () => void doTransform(paths, { rotate: 3 }, '逆时针旋转 90°'),
          },
          {
            id: 'tf-180',
            label: '旋转 180°',
            onSelect: () => void doTransform(paths, { rotate: 2 }, '旋转 180°'),
          },
          {
            id: 'tf-fh',
            label: '水平翻转（左右）',
            onSelect: () => void doTransform(paths, { flipH: true }, '水平翻转'),
          },
          {
            id: 'tf-fv',
            label: '垂直翻转（上下）',
            onSelect: () => void doTransform(paths, { flipV: true }, '垂直翻转'),
          },
        ],
      },
      {
        id: 'convert',
        label: '转换格式',
        disabled: exporting || withPath.length === 0,
        // ⚠️ 与「导出」的区别要在文案里点明：导出是原样复制，这里是重新编码
        hint: '重新编码',
        children: [
          { id: 'conv-jpg', label: '转为 JPEG（有损）', onSelect: () => void doConvert('jpg', paths) },
          { id: 'conv-png', label: '转为 PNG（无损）', onSelect: () => void doConvert('png', paths) },
          { id: 'conv-bmp', label: '转为 BMP（无压缩）', onSelect: () => void doConvert('bmp', paths) },
        ],
      },
      {
        id: 'orient',
        label: '按 EXIF 方向摆正',
        disabled: exporting || withPath.length === 0,
        children: [
          {
            id: 'orient-copy',
            label: '另存到文件夹…',
            onSelect: () => void doAutoOrient(false, paths),
          },
          {
            id: 'orient-write',
            label: '写回原文件…（不可撤销）',
            onSelect: () => void doAutoOrient(true, paths),
          },
        ],
      },
      {
        id: 'print',
        label: n > 1 ? `打印 ${n} 张…` : '打印…',
        disabled: exporting || withPath.length === 0,
        onSelect: () => void doPrint(paths),
      },
      {
        id: 'rename',
        label: n > 1 ? `批量重命名 ${n} 张…` : '重命名…',
        disabled: exporting || withPath.length === 0,
        onSelect: () => {
          // 单张 + 网格视图 → 直接在缩略图文件名上原地编辑（2026-10-24）；
          // 多张 / 列表视图仍走模板对话框
          if (n === 1 && mgr.viewMode === 'grid') setInlineRename(cardCtxItems[0] ?? null)
          else setRenameFor(cardCtxItems)
        },
      },
      {
        id: 'delete',
        label: n > 1 ? `删除 ${n} 张到回收站…` : '删除到回收站…',
        separatorBefore: true,
        disabled: exporting || withPath.length === 0,
        onSelect: () => void doDelete(),
      },
    ]
  }, [
    cardCtxItems,
    copyPaths,
    doAutoOrient,
    doConvert,
    doCopyTo,
    doDelete,
    doExport,
    doMoveTo,
    doSaveAs,
    doPrint,
    doTransform,
    doToggleTag,
    exporting,
    mgr,
    playPaths.length,
    playSlideshow,
  ])

  // 关掉菜单时清掉作用对象，避免下次打开前闪一下旧菜单项
  useEffect(() => {
    if (!cardCtx.state) setCardCtxItems([])
  }, [cardCtx.state])

  // ── 空白处右键（刷新 / 全选 / 取消选择）──
  /**
   * 网格或列表的**空白区域**右键。
   *
   * 与卡片右键是两个独立菜单：卡片菜单是对「某几张文件」的操作，
   * 空白菜单是对「当前这个文件夹 / 图库」的操作。
   *
   * ⭐ 核心是「刷新」：外部往目录里放了新图（或删了文件）之后，
   * 就地重扫当前范围并留在原地 —— **不切换目录、不弹回「我的电脑」**。
   * 走的是同一个 `mgr.rescan()`（`F5` 也是它）：浏览模式重列目录、
   * 入库模式重扫索引，两条路都不动 `scope`。
   */
  const blankCtx = useContextMenu()
  const blankMenuItems = useMemo<ContextMenuItem[]>(
    () => [
      {
        id: 'refresh',
        label: '刷新',
        hint: 'F5',
        disabled: mgr.isIdle || mgr.busy,
        onSelect: () => void mgr.rescan(),
      },
      {
        id: 'sel-all',
        label: '全选',
        hint: 'Ctrl+A',
        separatorBefore: true,
        disabled: mgr.isIdle || mgr.total === 0,
        onSelect: mgr.selectAll,
      },
      {
        id: 'sel-none',
        label: '取消选择',
        disabled: selectedCount === 0,
        onSelect: mgr.clearSelection,
      },
      {
        id: 'play-all',
        label: '幻灯片放映',
        hint: '播放本范围',
        separatorBefore: true,
        disabled: playPaths.length === 0,
        onSelect: () => playSlideshow(),
      },
    ],
    [mgr, selectedCount, playPaths.length, playSlideshow],
  )

  /** 打开空白菜单；「我的电脑」启动态不弹（那里没有可刷新的东西）。 */
  const openBlankMenu = useCallback(
    (e: ReactMouseEvent) => {
      if (mgr.isIdle) return
      blankCtx.open(e)
    },
    [blankCtx, mgr.isIdle],
  )


  /** 只放图片的行（状态栏「只列出前 N 张」用它计数） */
  const fileItems = useMemo(() => mgr.items.filter((i) => i.kind !== 'dir'), [mgr.items])

  const totalBytes = mgr.stats?.totalBytes ?? 0
  const scopeIsPath = mgr.scope.kind === 'folder' || mgr.scope.kind === 'browse'
  /**
   * 浏览范围的对象本身（`null` = 不在浏览模式）。
   *
   * ⚠️ 不要用 `mgr.isBrowsing` 做类型收窄 —— 它是个独立布尔，TS 收窄不了
   * `mgr.scope` 的联合类型，访问 `.path` 会报 TS2339。
   */
  const browseScope = mgr.scope.kind === 'browse' ? mgr.scope : null

  return (
    <div className={`pv-mg${props.active ? '' : ' is-hidden'}`}>
      <ManagerTopBar
        searchInput={mgr.searchInput}
        onSearch={mgr.setSearchInput}
        searchEnabled={!mgr.isBrowsing}
        canSelect={mgr.canSelect}        viewMode={mgr.viewMode}
        onViewMode={mgr.setViewMode}
        sort={mgr.sort}
        onSort={mgr.setSort}
        libraryMode={!mgr.isBrowsing}
        tier={mgr.tier}
        onTier={mgr.setTier}
        busy={mgr.busy}
        canRescan={mgr.canRescan}
        hasSelection={selectedCount > 0}
        onAddFolder={() => void pickFolder()}
        onRescan={() => void mgr.rescan()}
        onSelectAll={mgr.selectAll}
        onClearSelection={mgr.clearSelection}
        canSaveAs={selectedCount > 0}
        onSaveAs={() => void doSaveAs(selectedPaths)}
        onExit={onExit}
        onOpenSettings={props.onOpenSettings}
        onDragMouseDown={onDragMouseDown}
        isMaximized={props.isMaximized}
        onMinimize={props.onMinimize}
        onToggleMaximize={props.onToggleMaximize}
        onClose={props.onClose}
      />

      <div className="pv-mg-body">
        <ManagerSidebar
          tree={tree}
          width={sideW}
          libraries={mgr.libraries}
          libMembers={mgr.libMembers}
          scope={mgr.scope}
          totalCount={mgr.stats?.totalCount ?? 0}
          onSelectAll={mgr.selectAllScope}
          onSelectLibrary={mgr.selectLibrary}
          onSelectAddedFolder={mgr.selectAddedFolder}
          onGoHome={mgr.goHome}
          onBrowseDir={browseWithReveal}
          onAddToLibrary={(path, folderName, lib) =>
            void mgr.addFolderToLibrary(path, folderName, lib)
          }
          onAskNewLibraryName={(path, folderName) => setNewLibFor({ path, folderName })}
          onRemoveFromLibrary={(f) => void mgr.removeAddedFolder(f)}
          onGotoDir={gotoWithReveal}
          // 侧栏「图库 +」：只负责开口，弹窗在下面（portal 到 body，别塞进侧栏）
          onCreateLibrary={() => setCreateLib({ name: DEFAULT_LIB_NAME })}
          onAddFolderToLibrary={(l) => void doAddFolderToLibrary(l.id, l.name)}
          onRenameLibrary={(l) => setRenameLib({ id: l.id, name: l.name })}
          onDeleteLibrary={(l) => void doDeleteLibrary(l.id, l.name)}
          onCreateSmartLibrary={(name, rule) => void mgr.createSmartLibrary(name, rule)}
          tags={mgr.tags}
          onSelectTag={mgr.selectTag}
          onRenameTag={(t) => setTagRenameFor({ id: t.id, name: t.name })}
          onDeleteTag={(t) => void doDeleteTag(t)}
          places={mgr.places}
          onSelectPlace={mgr.selectPlace}
          dropLibId={dropLibId}
          onDropPathOnLibrary={onDropPathOnLibrary}
        />

        {/* 侧栏与主区之间的拖拽条：拖动调宽，双击恢复默认 */}
        <div
          className={`pv-mg-split${resizing ? ' is-active' : ''}`}
          role="separator"
          aria-orientation="vertical"
          title="拖动调整侧栏宽度（双击恢复默认）"
          onMouseDown={onSplitDown}
          onDoubleClick={() => setSideW(SIDE_DEFAULT)}
        />

        <section className="pv-mg-main">
          {/* 人像墙浮层：点「更多面部」弹出，选一个人就筛主区并关闭 */}
          {personPop && (
            <PersonWall
              anchor={personPop}
              currentId={mgr.personFilter?.id}
              onClose={() => setPersonPop(null)}
              onSelect={(id, name) => {
                mgr.setPersonFilter({ id, name })
                setPersonPop(null)
              }}
            />
          )}
          {/*
            ⭐ 2026-10-25：人像块要**跨「标题 + 两行标签 + 筛选行」三行的总高**，
            所以把标题行与筛选行收进 `.pv-mg-top-main`，人像条作为它的兄弟节点
            垂直拉伸（`align-items: stretch`）。
            以前人像条在 `.pv-mg-head` 内部，高度只能撑到标题行那么高。
          */}
          <div className="pv-mg-top">
            <div className="pv-mg-top-main">
          <div className="pv-mg-head">
            <div className="pv-mg-headl">
              <h2 className="pv-mg-title">
                {mgr.scopeTitle}
                {mgr.isBrowsing && (
                  <span className="pv-mg-badge" title="只读预览：没有写入索引，也没有生成缩略图缓存">
                    未加入图库
                  </span>
                )}
              </h2>
              <div className="pv-mg-sub">
                {mgr.isIdle ? (
                  <span>在左边展开「我的电脑」，点一个文件夹看照片</span>
                ) : (
                  <>
                    {/* 浏览模式可能是「图片 + 文件夹」混合，用「项」；入库只有照片，用「张」 */}
                    {fmtCount(mgr.total)} {mgr.isBrowsing ? '项' : '张'}
                    {/* ⛔ 不要在这里加「· 按评分排序」：用户 2026-10-24 明确不要，
                        排序状态看工具栏的「排序」按钮就够了，副标题一行放不下会折行。 */}
                    {mgr.isBrowsing && ' · 仅此目录'}
                  </>
                )}
                {mgr.busy && <span className="pv-mg-dim"> · 正在处理…</span>}
              </div>
            </div>
            {/*
              AI 标签条（2026-10-20）：只放在「已入库」的语境里 —— 浏览模式没有索引行，
              AI 结果按 image_id 存，拿不到就没得显示。
              数据来自 `mgr.tags`（`tags` + `image_tags`，source = ai / exif）。
            */}
            {!mgr.isBrowsing && !mgr.isIdle && (
              <AiTagStrip
                // ⛔ 只传词表里的二级标签：
                //   ① 一级（自然/活动…）是汇总行，混进来会重复计数；
                //   ② 相机/地点等 EXIF 标签已有下拉筛选，不该再进标签条。
                tags={mgr.tags.filter((t) => vocabMap.has(t.name) && !l1Names.has(t.name))}
                vocab={vocabMap}
                allL1={allL1}
                baseKey={mgr.scopeBaseKey}
                noTagActive={mgr.tagFilter === 'none'}
                onToggleNoTag={() => mgr.setTagFilter(mgr.tagFilter === 'none' ? null : 'none')}
                groupCounts={mgr.tagGroups}
                allActive={mgr.scope.kind === 'tagGroup'}
                onShowAll={mgr.clearTagGroup}
                onPickGroup={(label, names) => mgr.selectTagGroup(label, names)}
                termsOfL1={(l1) =>
                  // ⭐ 一级名**自己**也要算进去：有些图只过了一级门槛、没挂上任何二级
                  //（测试图库「自然」有 11 张、「建筑」有 16 张是这样），
                  // 只用二级词去筛会少掉它们，和标签条上的张数对不上。
                  [l1, ...vocabTerms.filter((v) => v.l1 === l1).map((v) => v.term)]
                }
              />
            )}
            <div className="pv-mg-headr">
              {browseScope ? (
                <>
                  {/*
                    浏览模式也显示已选统计 + 导出：导出/转格式/摆正/打印都是**文件级**操作，
                    不需要索引行。评分/标记/删除/重命名要索引 id，这里不给。
                  */}
                  {selectedCount > 0 && (
                    <>
                      <span className="pv-mg-dim">
                        已选 {selectedCount} 张 · {formatFileSize(selectedBytes)}
                      </span>
                      <button
                        type="button"
                        className="pv-btn"
                        disabled={exporting}
                        onClick={() => void doExport(selectedPaths)}
                        title="把选中的原图复制到你指定的文件夹（只复制，不改动原图）"
                      >
                        {exporting ? '导出中…' : '导出'}
                      </button>
                    </>
                  )}
                  <button
                    type="button"
                    className="pv-btn pv-btn-primary"
                    onClick={() => setNewLibFor({ path: browseScope.path, folderName: browseScope.name })}
                    title="扫描这个目录并加入图库（生成缩略图缓存，之后打开更快）"
                  >
                    加入图库…
                  </button>
                </>
              ) : null}
            </div>
          </div>

          {/*
            筛选改成**下拉**（2026-10-21 用户要求）：
            格式可能有十几种、相机型号更多，横铺一排 chip 会把标题区挤成两行。
            只对「已入库且已选目录」有意义：浏览模式没有全库统计可比。
          */}
          {!mgr.isBrowsing &&
          !mgr.isIdle &&
          (mgr.stats?.formats.length || mgr.cameras.length || mgr.places.length || mgr.years.length) ? (
            <div className="pv-mg-filters">
              {/*
                2026-10-24：改成**标准下拉框**（锚定按钮正下方、定宽、最多 13 行可滚动），
                顺序按用户要求：格式 → 年份 → 相机 → 地点。
              */}
              <FilterDropdown
                width={130}
                active={mgr.formats.length > 0}
                title="按文件格式筛选"
                label="格式"
              >
                <FilterRow
                  checked={mgr.formats.length === 0}
                  label="全部"
                  count={mgr.stats?.totalCount ?? 0}
                  onSelect={mgr.clearFormats}
                />
                {(mgr.stats?.formats ?? []).map((f) => (
                  <FilterRow
                    key={f.format}
                    checked={mgr.formats.includes(f.format)}
                    label={FORMAT_LABEL[f.format] ?? f.format.toUpperCase()}
                    count={f.count}
                    onSelect={() => mgr.toggleFormat(f.format)}
                  />
                ))}
              </FilterDropdown>

              <FilterDropdown
                width={130}
                active={mgr.yearFilter.length > 0}
                title="按拍摄年份筛选（EXIF 拍摄时间，没有的按入库时间）"
                label="年份"
              >
                <FilterRow
                  checked={mgr.yearFilter.length === 0}
                  label="全部"
                  count={mgr.years.reduce((a, y) => a + y.count, 0)}
                  onSelect={mgr.clearYears}
                />
                {mgr.years.map((y) => (
                  <FilterRow
                    key={y.year}
                    checked={mgr.yearFilter.includes(y.year)}
                    label={String(y.year)}
                    count={y.count}
                    onSelect={() => mgr.toggleYear(y.year)}
                  />
                ))}
              </FilterDropdown>

              <FilterDropdown
                width={160}
                active={mgr.camera.length > 0}
                title="按相机机型筛选（来自 EXIF）"
                label="相机"
              >
                <FilterRow
                  checked={mgr.camera.length === 0}
                  label="全部"
                  count={mgr.cameras.reduce((a, c) => a + c.count, 0)}
                  onSelect={mgr.clearCameras}
                />
                {mgr.cameras.map((c) => (
                  <FilterRow
                    key={c.model}
                    checked={mgr.camera.includes(c.model)}
                    label={c.model}
                    count={c.count}
                    onSelect={() => mgr.toggleCamera(c.model)}
                  />
                ))}
              </FilterDropdown>

              <FilterDropdown
                width={160}
                active={mgr.placeFilter.length > 0}
                title="按拍摄地点筛选（离线地名库，来自 EXIF GPS）"
                label="地点"
              >
                <FilterRow
                  checked={mgr.placeFilter.length === 0}
                  label="全部"
                  count={mgr.places.reduce((a, p) => a + p.count, 0)}
                  onSelect={mgr.clearPlaces}
                />
                {mgr.places.map((p) => (
                  <FilterRow
                    key={p.name}
                    checked={mgr.placeFilter.includes(p.name)}
                    label={p.name}
                    count={p.count}
                    onSelect={() => mgr.togglePlace(p.name)}
                  />
                ))}
              </FilterDropdown>
            </div>
          ) : null}
            </div>
            {/* 人像条：跨三行总高（标题 + 标签两行 + 筛选行） */}
            <PersonStrip
              visible={!mgr.isBrowsing && !mgr.isIdle}
              libraryId={mgr.scope.kind === 'library' ? mgr.scope.id : undefined}
              onOpenWall={(r) => setPersonPop({ x: Math.max(12, r.right - PERSON_POP_W), y: r.bottom + 6 })}
              onOpenPerson={(id, name) => mgr.setPersonFilter({ id, name })}
            />
          </div>

          {mgr.items.length === 0 ? (
            <div className="pv-mg-empty" onContextMenu={openBlankMenu}>
              {mgr.busy
                ? '正在处理…'
                : mgr.isIdle
                  ? '左边「我的电脑」里展开选一个文件夹：点一下是预览，右键可以加入图库'
                  : scopeIsPath
                    ? '这个目录里没有可浏览的图片'
                    : mgr.total === 0 && !mgr.searchInput && mgr.formats.length === 0
                      ? '图库里还没有照片。去左边右键一个目录，选「加入图库」'
                      : '没有符合当前筛选的照片'}
            </div>
          ) : mgr.viewMode === 'grid' ? (
            <div
              className="pv-mg-grid"
              ref={gridRef}
              // 空白处右键 → 刷新/全选；卡片自己的 onContextMenu 会 stopPropagation
              onContextMenu={openBlankMenu}
              // 点空白处（没落在卡片上）= 取消全部选择（用户 2026-10-24）
              onClick={(e) => {
                if (e.target === e.currentTarget && mgr.canSelect) mgr.clearSelection()
              }}
              style={{ ['--pv-card' as string]: `${CARD_WIDTH[mgr.tier]}px` }}
            >
              {mgr.items.map((item, i) => (
                <PhotoCard
                  key={item.key}
                  item={item}
                  tier={mgr.tier}
                  index={i}
                  selected={selected.has(item.key)}
                  selectable={mgr.canSelect}
                  // 只有被原地改写过的文件才有非零版本号 → 其余卡片完全不动
                  rev={mgr.thumbRevOf(item.path)}
                  // 窗口内的直接预取；`ThumbCanvas` 自己还会按「真的可见」提权
                  prefetch={i <= prefetchUntil}
                  onToggle={(e) => toggle(item, e)}
                  onOpen={() => openItem(item)}
                  onContextMenu={(e) => onCardContextMenu(item, e)}
                  onShowTags={(id, e) => void showTags(id, e)}
                  renaming={inlineRename?.key === item.key}
                  onRenameCommit={(name) => void doInlineRename(item, name)}
                  onRenameCancel={() => setInlineRename(null)}
                />
              ))}
            </div>
          ) : (
            <div className="pv-mg-list" onContextMenu={openBlankMenu}>
              {mgr.items.map((item) => {
                const on = selected.has(item.key)
                return (
                  <div
                    key={item.key}
                    className={`pv-mg-listrow${on ? ' is-selected' : ''}`}
                    role="button"
                    tabIndex={0}
                    onClick={(e) => toggle(item, e)}
                    onDoubleClick={() => openItem(item)}
                    onContextMenu={(e) => onCardContextMenu(item, e)}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter') openItem(item)
                    }}
                    title={item.path ?? item.name}
                  >
                    {mgr.canSelect ? (
                      <span className={`pv-mg-listcheck${on ? ' is-on' : ''}`} aria-hidden>
                        {on ? '✓' : ''}
                      </span>
                    ) : (
                      <span />
                    )}
                    <span className="pv-mg-listname">
                      {/* 文件夹行：与网格卡片、侧栏树用同一个矢量图标 */}
                      {item.kind === 'dir' && <FolderIcon className="pv-mg-listico" />}
                      <span className="pv-mg-listtext">{item.name}</span>
                    </span>
                    <span className="pv-mg-listmeta">{item.format?.toUpperCase() ?? '—'}</span>
                    <span className="pv-mg-listmeta">
                      {item.size ? formatFileSize(item.size) : '—'}
                    </span>
                  </div>
                )
              })}
            </div>
          )}

        </section>
      </div>

      <footer className="pv-mg-status">
        <span>
          {mgr.isIdle ? (
            `图库共 ${fmtCount(mgr.stats?.totalCount ?? 0)} 张`
          ) : (
            <>
              {fmtCount(mgr.items.length)} / {fmtCount(mgr.total)} 个项目
              {!mgr.isBrowsing && totalBytes > 0 && ` · ${formatFileSize(totalBytes)}`}
              {mgr.isBrowsing && ' · 浏览模式（未入库）'}
            </>
          )}
        </span>
        {mgr.browseTruncated && (
          <span className="pv-mg-dim">
            · 目录里还有更多，只列出前 {fileItems.length} 张图片
          </span>
        )}
        {pendingThumbs > 0 && (
          <span className="pv-mg-dim" title="首次看某个目录时要逐张解码，看完不会留在缓存里">
            · 正在解码缩略图，剩余 {pendingThumbs}
          </span>
        )}
        {!mgr.isBrowsing && mgr.scanInfo && <span className="pv-mg-dim">· {mgr.scanInfo}</span>}
        {!mgr.isIdle && (mgr.root ?? browseScope?.path) && (
          <span className="pv-mg-dim pv-mg-statuspath" title={mgr.root ?? browseScope?.path}>
            · {mgr.root ?? browseScope?.path}
          </span>
        )}
        {mgr.error && <span className="pv-mg-err">· {mgr.error}</span>}
        {/*
          ⭐ 2026-10-24 用户定稿：选中信息（「已选 N 张 · 大小」）放到**状态栏右下角**，
          并且**去掉「导出 / 标记」两个按钮** —— 这两个动作在右键菜单里本来就有，
          挂在状态栏上既挤又容易误点。
        */}
        {selectedCount > 0 && (
          <span className="pv-mg-dim pv-mg-selinfo">
            已选 {selectedCount} 张 · {formatFileSize(selectedBytes)}
          </span>
        )}
      </footer>

      {/* 「加进新图库…」的名字输入框 */}
      {newLibFor && (
        <PromptDialog
          title="加进新图库"
          label={`给「${newLibFor.folderName}」建一个图库；如果已经有同名图库，会直接加进去。`}
          placeholder="图库名称"
          initial={newLibFor.folderName}
          okLabel="加入"
          onOk={(name) => {
            const target = newLibFor
            setNewLibFor(null)
            void mgr.addFolderToLibrary(target.path, target.folderName, { kind: 'new', name })
          }}
          onCancel={() => setNewLibFor(null)}
        />
      )}

      {/*
        侧栏「图库 +」：新建**空的**图库。
        ⭐ 失败（重名/空名）不关弹窗 —— `createLib` 留着，`error` 变成红字，
        用户改完直接回车重试。只有成功那条路才 `setCreateLib(null)`。
      */}
      {createLib && (
        <PromptDialog
          title="新建图库"
          label="先建一个空图库，之后在「文件夹」里右键目录 →「加进已有图库」就能填照片。"
          placeholder="图库名称"
          initial={createLib.name}
          okLabel="创建"
          error={createLib.error}
          onOk={(name) => void doCreateLibrary(name)}
          onCancel={() => setCreateLib(null)}
        />
      )}

      {/* 批量重命名的模板输入框 */}
      {renameFor && (
        <PromptDialog
          title="批量重命名"
          label={
            `共 ${renameFor.length} 张。模板里可用 {name}（原文件名）和 {n}（序号，自动补零）。` +
            `不写 {n} 会自动补上，扩展名始终保留。`
          }
          placeholder="{name}_{n}"
          initial="{name}_{n}"
          okLabel="重命名"
          onOk={(tpl) => void doRename(tpl)}
          onCancel={() => setRenameFor(null)}
        />
      )}

      {/*
        标签浮层（L 键 / 右键「更多 / 新建标签…」）。
      */}
      {tagFor && (
        <TagPicker
          ids={tagFor}
          tags={mgr.tags}
          onAdd={(names) => mgr.addTags(tagFor, names)}
          onRemove={(tagId) => mgr.removeTag(tagFor, tagId)}
          onClose={() => setTagFor(null)}
        />
      )}

      {/* 标签改名（失败不关弹窗，重名时留在输入框里改） */}
      {tagRenameFor && (
        <PromptDialog
          title="重命名标签"
          label="只改标签名，照片与其标签关系都不动。"
          placeholder="标签名"
          initial={tagRenameFor.name}
          okLabel="重命名"
          error={tagRenameFor.error}
          onOk={(name) => void doRenameTag(tagRenameFor.id, name)}
          onCancel={() => setTagRenameFor(null)}
        />
      )}

      {/*
        图库行右键「重命名」。同样是**失败不关弹窗**（重名时留在输入框里改名）。
      */}
      {renameLib && (
        <PromptDialog
          title="重命名图库"
          label="只改图库的名字，里面的目录与照片都不动。"
          placeholder="图库名称"
          initial={renameLib.name}
          okLabel="重命名"
          error={renameLib.error}
          onOk={(name) => void doRenameLibrary(renameLib.id, name)}
          onCancel={() => setRenameLib(null)}
        />
      )}

      {/*
        照片卡片右键菜单：在资源管理器中显示 / 复制路径 / 导出所选。
        `key` 用坐标强制重挂载（换位置重新右键 = 新实例）；
        ⛔ 不要改成「保留实例 + 监听 position 的 effect」——那会让菜单永远 hidden。
      */}
      {cardCtx.state && (
        <ContextMenu
          key={`${cardCtx.state.x},${cardCtx.state.y}`}
          position={{ x: cardCtx.state.x, y: cardCtx.state.y }}
          items={cardMenuItems}
          onClose={cardCtx.close}
        />
      )}

      {/* 卡片「标签」按钮：这张图的标签 + 置信度 */}
      {tagPop && (
        <ContextMenu
          key={`tags-${tagPop.x},${tagPop.y}`}
          position={{ x: tagPop.x, y: tagPop.y }}
          items={tagPop.items}
          width={220}
          onClose={() => setTagPop(null)}
        />
      )}

      {/* 主题词管理（启停 / 搜索 / 导出） */}

      {/* 空白处右键：刷新当前文件夹 / 图库（留在原地，不切换范围） */}
      {blankCtx.state && (
        <ContextMenu
          key={`blank-${blankCtx.state.x},${blankCtx.state.y}`}
          position={{ x: blankCtx.state.x, y: blankCtx.state.y }}
          items={blankMenuItems}
          onClose={blankCtx.close}
        />
      )}

      {/*
        打印预览（与单图模式**同一个组件**）。
        - 单张：只有一页，确认后印 1 张；
        - 多选：顶部有 ‹ 1 / N ›，只换预览样本，版面参数对整批共用一套。
        预览样本还没取回来时先不渲染（避免拿 0×0 去算版面）。
      */}
      {printBatch && printSrc && (
        <PrintPreview
          imageWidth={printSrc.width}
          imageHeight={printSrc.height}
          sourceImage={printSrc}
          // 批量不做画布旋转/裁剪：按文件原始像素印，与后端 `rotation_steps = 0` 对齐
          rotation={0}
          printing={printing}
          error={printError}
          forceQuick
          page={
            printBatch.paths.length > 1
              ? {
                  index: printBatch.index,
                  total: printBatch.paths.length,
                  onPrev: () =>
                    setPrintBatch((p) =>
                      p ? { ...p, index: Math.max(0, p.index - 1) } : p,
                    ),
                  onNext: () =>
                    setPrintBatch((p) =>
                      p ? { ...p, index: Math.min(p.paths.length - 1, p.index + 1) } : p,
                    ),
                }
              : undefined
          }
          onClose={() => setPrintBatch(null)}
          onConfirm={(o) => void confirmBatchPrint(o)}
        />
      )}

      {/*
        导出结果浮层：不打断操作（不用 alert），几秒后自己消失。
        失败明细最多列 4 条，多了折成「…另有 N 张」。
      */}
      {exportNotice && (
        <div
          className={`pv-mg-toast is-${exportNotice.tone}`}
          role="status"
          onClick={() => setExportNotice(null)}
          title="点击关闭"
        >
          <div className="pv-mg-toasttext">{exportNotice.text}</div>
          {exportNotice.detail && (
            <pre className="pv-mg-toastdetail">{exportNotice.detail}</pre>
          )}
        </div>
      )}
    </div>
  )
}
