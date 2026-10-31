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
import { confirm, open as openDialog } from '@tauri-apps/plugin-dialog'

import { formatFileSize } from '../../ipc'
import { managerCreateLibrary, type TagItem } from '../../ipc/manager'
import { subscribeThumbPending } from '../../ipc/thumbQueue'
import { notifyPeopleChanged } from '../../lib/peopleEvents'
import { useFsTree } from '../../hooks/useFsTree'
import { useGridWindow } from '../../hooks/useGridWindow'
import { errText, useManager, type GridItem } from '../../hooks/useManager'
import { useManagerBatch } from '../../hooks/manager/batchActions'
import { useLibDrop } from '../../hooks/libDrop'
import { useTagPopover } from '../../hooks/manager/useTagPopover'
import { useSideResize } from '../../hooks/manager/useSideResize'
import { useColumnResize } from '../../hooks/manager/useColumnResize'
import { ContextMenu, useContextMenu } from '../chrome/ContextMenu'
import { FilterDropdown, FilterRow } from './FilterDropdown'
import { ConfirmDialog } from '../chrome/ConfirmDialog'
import { PromptDialog } from '../chrome/PromptDialog'
import { PrintPreview } from '../viewer/PrintPreview'
import { DriveGrid } from './DriveGrid'
import { useShellIcons } from '../../hooks/useShellIcons'
import { fsListDirs } from '../../ipc/browse'
import { baseName, parentPath } from '../../hooks/parentPath'
import { ManagerSidebar } from './ManagerSidebar'
import { ManagerStatusBar } from './ManagerStatusBar'
import { ManagerList } from './ManagerList'
import { AiTagStrip } from './AiTagStrip'
import { TagPicker } from './TagPicker'
import { ManagerTopBar } from './ManagerTopBar'
import { PersonStrip } from './PersonStrip'
import { AiPipelineDialog } from './AiPipelineDialog'
import { PersonWall, PERSON_POP_W } from './PersonWall'
import { managerSetPersonCover } from '../../ipc/people'
import { PhotoCard } from './PhotoCard'
import {
  DEFAULT_LIB_NAME,
  PREFETCH_AHEAD,
  filePathsOf,
  fmtCount,
  formatLabelOf,
} from './viewConfig'
import { thumbWidthOf } from '../../hooks/manager/prefs'
import { buildBlankMenu, buildCardMenu } from './cardMenu'
import '../../styles/manager.css'

/** 缩略图档位 → 网格列宽（px）。CSS 用 `auto-fill + minmax` 排布。 */
/* 常量与纯函数见 `./viewConfig`（2026-11 拆出）：预取量、路径提取、格式显示名、
 * 千分位、侧栏宽度记忆、新建图库默认名。⛔ 别再往这个文件里加模块级常量。 */

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
   * `page = 'tag'` 时直接落到「AI 分析 · 打标」页 —— 标签条右侧那个齿轮用它，
   * 2026-10-24 起齿轮不再弹自己的小面板（设置统一到设置窗口，避免两份）。
   */
  onOpenSettings: (page?: 'tag') => void
  /**
   * 双击缩略图：用单图浏览打开该绝对路径。
   *
   * `siblings` 是**主区当前显示的图片路径列表**（顺序一致）。交给单图浏览后，
   * 「下一张」就在这份列表里翻 —— 按标签/人像筛过之后也只在筛出来的图里走。
   * 不传的话单图模式会退回「按文件所在目录翻页」，一翻就跳出筛选结果。
   */
  onOpenImage: (path: string, siblings: string[]) => void
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
  /** 一键 AI 识别弹层（先打标再识别人像） */
  const [pipeOpen, setPipeOpen] = useState(false)
  const [pendingThumbs, setPendingThumbs] = useState(0)

  useEffect(() => subscribeThumbPending(setPendingThumbs), [])

  /** 多选集合的**唯一真源在 `useManager`**（它会随范围/筛选变化清空）。 */
  const selected = useMemo(() => new Set(mgr.selectedKeys), [mgr.selectedKeys])

  /**
   * 右键作用的对象：右键那一刻的选中集合
   *（菜单打开后即使选区变了，也不影响已经弹出的那些项）。
   *
   * 批量动作（删除 / 重命名）作用在它上面而不是「当前选中」——
   * 右键后用户可能顺手改了选区，菜单里的「删除」不该跟着变。
   */
  const [cardCtxItems, setCardCtxItems] = useState<GridItem[]>([])

  /**
   * 文件级批量操作：导出 / 转格式 / 另存为 / 摆正 / 打印 / 复制·移动 / 删除 /
   * 重命名，外加统一的结果提示。
   *
   * 实现见 `hooks/manager/batchActions.ts`。这里只解构出本组件（顶栏、右键菜单、
   * 预览浮层）真正用到的那部分。
   */
  const {
    exporting,
    printing,
    printError,
    printBatch,
    setPrintBatch,
    printSrc,
    exportNotice,
    setExportNotice,
    renameFor,
    setRenameFor,
    inlineRename,
    setInlineRename,
    printOpenRef,
    doExport,
    copyPaths,
    doConvert,
    doSaveAs,
    doAutoOrient,
    doPrint,
    confirmBatchPrint,
    doCopyTo,
    doMoveTo,
    doDelete,
    doTransform,
    doRename,
    doInlineRename,
  } = useManagerBatch(mgr, cardCtxItems)

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
  const [newLibFor, setNewLibFor] = useState<{
    path: string
    folderName: string
    /** 用户答过「包含子文件夹」才为 true */
    recursive: boolean
  } | null>(null)
  /**
   * 「是否包含子文件夹」的三选一（2026-10-31）。
   *
   * 用 Promise 包一层 state：调用方 `await` 它拿到结果再继续，
   * 这样 `addDirToLibrary` 不必拆成「先弹框、再回调里接着做」两段。
   */
  const [askTree, setAskTree] = useState<{
    path: string
    name: string
    /** 第一层子目录数（后端一次最多列 400 个，到顶就是「400 以上」） */
    subdirs: number
    more: boolean
    resolve: (v: 'tree' | 'flat' | 'cancel') => void
  } | null>(null)

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

  /**
   * 图库行右键「删除图库…」。
   *
   * ⭐ 2026-10-31 语义变了：现在**连带清掉它独占的目录索引**，
   * 这些照片会从「全部照片」里消失（用户要求）。所以确认框必须把代价说清楚 ——
   * 评分/标签/人脸都挂在 `image_id` 上，删了索引就一起没了，重扫也恢复不了。
   */
  const doDeleteLibrary = async (id: number, name: string) => {
    const ok = await confirm(
      `删除图库「${name}」？\n\n` +
        '· 这个图库**独占**的目录会连同索引一起清掉 —— 这些照片将从「全部照片」里消失\n' +
        '· ⚠️ 它们的评分、标签、人脸标记会一起丢失，重扫也恢复不了\n' +
        '· 同一个目录如果还挂在**别的图库**上，那边不受影响\n' +
        '· **不会删除任何原始图片文件**（磁盘上的照片原封不动）',
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

  // ── 侧栏宽度：拖中间那条缝（2026-11 拆到 useSideResize）──
  const { sideW, resetSide, resizing, onSplitDown } = useSideResize()

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

  /**
   * 网格共用的「打开」：文件夹 → 进入；图片 → 单图查看。
   *
   * ⭐ 图片必须把 `playPaths`（当前筛选结果的路径列表）一起传出去，
   * 否则单图模式的「下一张」只在**当前文件所在目录**里翻 ——
   * 按人像/标签筛出来的图散落在各目录，翻到第 2 张就跳出筛选结果了。
   */
  const openItem = useCallback(
    (item: GridItem) => {
      if (item.kind === 'dir' && item.path) {
        gotoWithReveal(item.path, item.name)
        return
      }
      /*
       * 压缩包：进入后主区显示**包内图片**（走 `fs_list_images(包路径)`）。
       *
       * ⛔ 不走 `gotoWithReveal`：它会给侧栏树做 `revealTo(路径)`，
       *    而树里根本没有「包」这个节点（我们不在包里模拟目录层级），
       *    那次调用必然是空转 + 一次多余的失败。
       */
      if (item.kind === 'archive' && item.path) {
        void mgr.gotoDir(item.path, item.name)
        return
      }
      if (item.path) onOpenImage(item.path, playPaths)
    },
    [gotoWithReveal, onOpenImage, playPaths],
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

  /**
   * 「加进图库」三种方式（2026-10-30 用户要求，和侧栏目录右键菜单一致）：
   *
   * - `name`     用文件夹名当图库名（同名复用，一步到位）
   * - `new`      弹输入框自定义图库名
   * - `existing` 直接挂到指定的已有图库
   *
   * ⭐ 目录卡片右键、侧栏目录右键都走这里 —— **同一个函数**，行为不会两处漂移。
   * 文件菜单不进这里：它先让你选目录，再问「新建 / 加入已有」，见 `folderMenu`。
   */
  const addDirToLibrary = useCallback(
    async (
      path: string,
      mode: 'name' | 'new' | 'existing',
      lib?: { id: number; name: string },
    ) => {
      const trimmed = path.replace(/[\\/]+$/, '')
      const folderName = trimmed.split(/[\\/]/).pop() || path

      /*
       * ① 先问「是否包含子文件夹」（用户 2026-10-31）——**只在有子目录时才问**。
       *
       * 为什么先查一次子目录而不是无脑弹框：绝大多数收录目录都是叶子目录，
       * 每次入库都弹一下纯属打扰。`fs_list_dirs` 是单层 read_dir，几十毫秒。
       *
       * ⚠️ 查询失败（无权限 / 盘掉了）**不算错** → 按单层走。真正的错误
       * 会在后面的入库里报出来，这里拦一下只会让用户莫名其妙加不进去。
       */
      let recursive = false
      try {
        const subs = await fsListDirs(path)
        const n = subs.filter((d) => d.kind === 'dir').length
        // 后端单层最多列 400 个，到顶说明「至少 400」
        const more = n >= 400
        if (n > 0) {
          const ans = await new Promise<'tree' | 'flat' | 'cancel'>((resolve) => {
            setAskTree({
              path,
              name: folderName,
              subdirs: n,
              more,
              resolve: (v) => {
                setAskTree(null)
                resolve(v)
              },
            })
          })
          if (ans === 'cancel') return
          recursive = ans === 'tree'
        }
      } catch {
        /* 查不到子目录就按单层走 */
      }

      if (mode === 'new') {
        // 收名字 → 确认后走 `newLibFor` 对话框里的 `addFolderToLibrary`
        setNewLibFor({ path, folderName, recursive })
        return
      }
      if (mode === 'existing') {
        if (!lib) return
        void mgr.addFolderToLibrary(
          path,
          folderName,
          { kind: 'existing', id: lib.id, name: lib.name },
          recursive,
        )
        return
      }
      void mgr.addFolderToLibrary(path, folderName, { kind: 'byFolderName' }, recursive)
    },
    [mgr],
  )

  /**
   * 文件 →「打开文件夹…」：选一个目录导航过去看。
   *
   * ⛔ 不要复用 `pickFolder` —— 那个是「加入图库」（扫索引 + 生成缩略图缓存）。
   * 两个动作以前挤在一个「添加文件夹」里，看着像打开、实际在入库。
   */
  const pickOpenFolder = useCallback(async () => {
    const picked = await openDialog({
      directory: true,
      multiple: false,
      title: '打开文件夹',
    })
    if (typeof picked !== 'string') return
    const trimmed = picked.replace(/[\\/]+$/, '')
    gotoWithReveal(trimmed, trimmed.split(/[\\/]/).pop() || picked)
  }, [gotoWithReveal])

  /**
   * 单击选择（用户 2026-10-24 定稿）：
   * - **普通单击 = 加选**（连续点缩略图累加多选，不再需要按住 Ctrl）；
   * - **单击已选中的 = 取消本次全部选择**（比逐个移除更常用 —— 用户点错了
   *   想一键清空，而不是费劲把某一张摘出去；摘单张仍可用 Ctrl+单击）；
   * - Ctrl / Shift / Cmd 保持原来的切换语义。
   * 浏览模式不参与选择。
   */
  /**
   * 单击选择。
   *
   * ⭐ 2026-10-30：**文件夹行也参与选择**（用户要求「选中目录再右键操作」）。
   * 以前 `kind === 'dir'` 直接 return，于是目录卡片点一下毫无反应 ——
   * 而「把这个目录加进图库」恰恰是最常做的事。
   *
   * ⚠️ 浏览模式（未入库）整体 `canSelect === false`（照片不参与批量操作），
   * 但**目录必须例外**，否则浏览模式里选不了文件夹，右键只能作用于单张。
   */
  /**
   * 区间选择的锚点（上一次「普通单击 / Ctrl 单击」落在哪一项）。
   *
   * `Shift + 单击` 从它一路选到当前项。**Shift 本身不移动锚点** ——
   * 这样连点几下就能反复扩大/缩小同一段范围，跟资源管理器一致。
   */
  const selAnchor = useRef<string | null>(null)
  // 选择被清空（点空白 / Esc / 切范围）后锚点就失效了，留着会选出一段莫名其妙的东西
  if (mgr.selectedKeys.length === 0) selAnchor.current = null

  const toggle = useCallback(
    (item: GridItem, e: ReactMouseEvent) => {
      // 文件夹与压缩包都是「容器」：浏览模式下也要能选中（右键对它们操作）
      const container = item.kind === 'dir' || item.kind === 'archive'
      if (!container && !mgr.canSelect) return

      // Shift：从锚点一路选过来（「大量选取」）
      if (e.shiftKey) {
        mgr.selectRange(selAnchor.current ?? item.key, item.key)
        return
      }
      if (e.ctrlKey || e.metaKey) {
        mgr.toggleSelect(item.key)
        selAnchor.current = item.key
        return
      }
      if (selected.has(item.key)) {
        mgr.clearSelection()
        selAnchor.current = null
      } else {
        mgr.toggleSelect(item.key)
        selAnchor.current = item.key
      }
    },
    [mgr, selected],
  )

  const selectedItems = mgr.items.filter((i) => selected.has(i.key))
  const selectedBytes = selectedItems.reduce((sum, i) => sum + (i.size ?? 0), 0)
  const selectedCount = selectedItems.length
  /**
   * 选中里的**文件**：目录和压缩包都不参与导出/打印/删除这些文件级动作
   * —— 把压缩包路径喂给 `doExport` 只会得到「已复制 0 个」这种莫名提示。
   */
  const selectedFileItems = useMemo(
    () => selectedItems.filter((i) => i.kind === 'file'),
    [selectedItems],
  )
  const selectedFileCount = selectedFileItems.length
  /** 选中项是否**全都**已打旗标（决定顶栏按钮是「标记」还是「取消标记」） */


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
  // `cardCtxItems`（右键作用对象）已上移到组件开头 —— `useManagerBatch`
  // 需要它（删除 / 重命名作用于"右键那一刻"的集合），而 hook 必须在
  // 所有用到它的 useCallback 之前调用。这里只留菜单自身的开关。
  const cardCtx = useContextMenu()

  /* ── 卡片「标签」弹层 + AI 词表（2026-11 拆到 useTagPopover）──
     词表只读一次（几 KB）；弹层的分组/排序见 `./tagMenu`（纯函数，有单测）。 */
  const { tagPop, setTagPop, vocab, terms: vocabTerms, showTags } = useTagPopover(mgr.ready)
  /** 词表索引的三份视图（标签条与弹层共用同一份，别在这里另建） */
  const { map: vocabMap, l1Names, allL1 } = vocab

  /** 人像墙浮层的锚点（null = 关闭）。2026-10-25 用户定稿：用浮层，不新开页面 */
  const [personPop, setPersonPop] = useState<{ x: number; y: number } | null>(null)


  /** 顶栏按钮用：当前**选中**的行 */
  const selectedPaths = useMemo(() => filePathsOf(selectedItems), [selectedItems])
  /** 只含文件路径 —— 「导出」这类动作不能吃到目录 */
  const selectedFilePaths = useMemo(() => filePathsOf(selectedFileItems), [selectedFileItems])


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
        // 目录在浏览模式也允许被设成唯一选中（`canSelect` 那边是给照片用的）
        if (mgr.canSelect || item.kind === 'dir') mgr.selectOnly(item.key)
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

  /**
   * 把当前人物的头像换成「这张图里他的脸」。
   *
   * 成功后广播人物变更事件 —— 人像条与人像墙都在监听它重拉列表。
   * 头像位图不用手动清：`cropCache` 的 key 含 `face.id`，换封面后 face.id 变了，
   * 旧缓存天然不会被命中。
   */
  const doSetPersonCover = useCallback(async (personId: number, imageId: number) => {
    const cover = await managerSetPersonCover(personId, imageId)
    if (!cover) {
      /* 理论上不该发生：按人筛选出来的图必然有这个人的脸 */
      console.warn('[person] 这张图里没有该人物的脸', { personId, imageId })
      return
    }
    notifyPeopleChanged()
  }, [])

  /**
   * 卡片右键菜单的内容。菜单项本身全在 `cardMenu.ts`（纯函数，可单测），
   * 这里只负责把当前渲染周期里的数据和回调递进去。
   *
   * ⚠️ 依赖数组必须列全 —— `buildCardMenu` 只是把原文搬进函数，读的仍是这些
   * 闭包变量，少一个就会出现「改了评分、菜单不刷新」这类看着像玄学的问题。
   * 四个 setter（`setTagFor` / `setRenameFor` / `setInlineRename` / `setExportNotice`）
   * 是 state setter，引用恒定，按 React 惯例不进依赖。
   */
  const cardMenuItems = useMemo(
    () =>
      buildCardMenu({
        cardCtxItems,
        mgr,
        exporting,
        addDirToLibrary,
        gotoWithReveal,
        toggleTag: doToggleTag,
        setPersonCover: doSetPersonCover,
        openTagPicker: setTagFor,
        setRenameFor,
        setInlineRename,
        setExportNotice,
        copyPaths,
        doExport,
        doSaveAs,
        doCopyTo,
        doMoveTo,
        doTransform,
        doConvert,
        doAutoOrient,
        doPrint,
        doDelete,
        playCount: playPaths.length,
        playSlideshow,
      }),
    [
      cardCtxItems,
      mgr,
      exporting,
      addDirToLibrary,
      gotoWithReveal,
      doToggleTag,
      doSetPersonCover,
      copyPaths,
      doExport,
      doSaveAs,
      doCopyTo,
      doMoveTo,
      doTransform,
      doConvert,
      doAutoOrient,
      doPrint,
      doDelete,
      playPaths.length,
      playSlideshow,
    ],
  )

  // 关掉菜单时清掉作用对象，避免下次打开前闪一下旧菜单项
  useEffect(() => {
    if (!cardCtx.state) setCardCtxItems([])
  }, [cardCtx.state])

  /**
   * 当前正在看的**目录**（浏览模式 / 已入库目录），没有就是 `null`。
   *
   * 「文件 → 新建图库 / 加入图库」和空白处右键的同名两项都作用在它身上。
   * ⚠️ 必须定义在 `blankMenuItems` **之前** —— 那个 useMemo 会读它，
   * 声明在后面就是 TDZ（渲染时 `Cannot access before initialization`）。
   */
  const curDir =
    mgr.scope.kind === 'browse' || mgr.scope.kind === 'folder'
      ? { path: mgr.scope.path, name: mgr.scope.name }
      : null

  const blankCtx = useContextMenu()

  /** 空白处右键的菜单项（菜单内容见 `cardMenu.ts::buildBlankMenu`）。 */
  const blankMenuItems = useMemo(
    () =>
      buildBlankMenu({
        mgr,
        curDir,
        selectedItems,
        addDirToLibrary,
        selectedCount,
        playCount: playPaths.length,
        playSlideshow,
      }),
    [mgr, curDir, selectedItems, addDirToLibrary, selectedCount, playPaths.length, playSlideshow],
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

  /**
   * 浏览模式「返回上一级」的落脚点（`null` = 已经在盘根，没得退）。
   *
   * ⚠️ 用 `parentPath` 而不是自己切字符串：盘根（`E:\`）、UNC（`\\nas\share`）、
   * 压缩包路径各有各的边界，全在 `hooks/parentPath.ts` 里单测过了。
   */
  const browseParent = useMemo(
    () => (browseScope ? parentPath(browseScope.path) : null),
    [browseScope],
  )
  const goUp = useCallback(() => {
    if (!browseParent) return
    void mgr.gotoDir(browseParent, baseName(browseParent))
  }, [browseParent, mgr])

  /*
   * 列表视图的列（2026-10-31）：显示哪些列、各列多宽，都在
   * `hooks/listColumns.ts` 里定义并持久化；设置窗口改完这里立刻跟着变。
   * 列宽拖拽搬到 `useColumnResize`（2026-11）：全局监听 + 松手落盘那套自己一处。
   */
  const { listPrefs, resizing: colResizing, onGripDown: onColGripDown } = useColumnResize()

  /**
   * 「我的电脑」主区直接铺盘符（用户 2026-10-30）。
   *
   * 盘符本来只有展开左侧树的第一层才有 —— 选中「我的电脑」时主区是空的，
   * 用户还得回侧栏再展开一次。这里直接把 `computer` 节点的子节点（磁盘）
   * 拿来渲染，进来看图库就能点盘。
   */
  const computerNode = tree.roots.find((n) => n.kind === 'computer') ?? null
  const driveNodes = computerNode ? tree.childrenOf(computerNode) : null
  /*
   * 盘符卡片用**系统图标**（和侧栏树同一份数据，见 `useShellIcons`）：
   * 用户 2026-10-31「这里磁盘 ui 要和侧栏统一」—— 自绘 SVG 的深色盘体和
   * 资源管理器那套白色盘体并排看像两个软件。
   * 后端给的是 256px 的 Jumbo 图标，缩到 112px 依然清晰；取不到才回退 SVG。
   */
  const shellIcons = useShellIcons()
  /**
   * 「我的电脑」主区里被单击的盘符（**双击才进入**，用户 2026-10-31 要求）。
   *
   * 单独一份 state 而不复用 `mgr.selection` —— 盘符不是 `mgr.items` 里的一项
   * （那个列表是当前目录/图库的照片），塞进去会让「已选 N 张」和批量操作都乱掉。
   * 单击只负责高亮，和文件夹卡片的手感保持一致。
   */
  const [driveSel, setDriveSel] = useState<string | null>(null)
  const expandNode = tree.expand
  useEffect(() => {
    if (mgr.scope.kind !== 'none' || !computerNode || driveNodes !== null) return
    void expandNode(computerNode)
  }, [mgr.scope.kind, computerNode, driveNodes, expandNode])

  /**
   * 文件菜单的快捷键：`Ctrl+O` 打开文件夹、`Ctrl+P` 打印、`Delete` 删除。
   *
   * ⚠️ 菜单上写了 hint 就必须真的绑上 —— F5 那次就是「菜单写着 F5、实际没绑」，
   * 用户按了没反应以为是刷新坏了。三个都作用在**当前选中**（打印/删除）。
   * 用 ref 转一层，避免选中一变就重挂监听。
   */
  const fileKeysRef = useRef({
    open: () => {},
    print: () => {},
    del: () => {},
  })
  fileKeysRef.current = {
    open: () => void pickOpenFolder(),
    // 空选中时**静默**（不要去弹「这些图片没有可用的本地路径」——
    // 用户只是随手按了下快捷键，不是要执行操作）
    print: () => {
      if (selectedFilePaths.length > 0) doPrint(selectedFilePaths)
    },
    del: () => {
      if (selectedFileItems.length > 0) void doDelete(selectedFileItems)
    },
  }
  useEffect(() => {
    if (!props.active) return
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null
      const typing =
        !!t && (['INPUT', 'TEXTAREA', 'SELECT'].includes(t.tagName) || t.isContentEditable)
      if (typing) return
      if ((e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === 'o') {
        e.preventDefault()
        fileKeysRef.current.open()
        return
      }
      if ((e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === 'p') {
        e.preventDefault()
        fileKeysRef.current.print()
        return
      }
      // 不带修饰键的 Delete：选中为空时什么都不做（别误删）
      if (e.key === 'Delete' && !e.ctrlKey && !e.metaKey) {
        e.preventDefault()
        fileKeysRef.current.del()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [props.active])
  /**
   * 能否跑「一键 AI 识别」。
   *
   * ⭐ 2026-10-29 从标题右侧挪到筛选行（`格式` 之前）。放在标题右侧时，
   * 它被 `margin-left:auto` 顶到窗口最右边、离图库名隔着一大片空白，
   * 既不像"这个图库的操作"，也在窄窗口下离筛选器很远。
   * 挪到筛选行第一个位置后，视线从「图库名 → 筛选 → AI 识别」是一条线。
   *
   * 只对**已入库的图库**显示：标签条和人像条都按 image_id 存，
   * 浏览模式的临时预览没有索引行，跑了也没地方显示。
   */
  const aiEligible = !mgr.isBrowsing && !mgr.isIdle && mgr.scope.kind === 'library'

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
        hasFileSelection={selectedFileCount > 0}
        onOpenFolder={() => void pickOpenFolder()}
        dirPath={curDir?.path ?? null}
        dirName={curDir?.name ?? ''}
        onNewLibraryFromDir={() => {
          // 统一入口：先问「是否包含子文件夹」（直接 setNewLibFor 会绕过那一问）
          if (curDir) void addDirToLibrary(curDir.path, 'new')
        }}
        onAddDirToLibrary={(id, name) => {
          if (curDir) addDirToLibrary(curDir.path, 'existing', { id, name })
        }}
        libraries={mgr.libraries.map((l) => ({ id: l.id, name: l.name }))}
        // 文件级操作都作用在**当前选中**上（右键走各自的 cardCtxItems）
        onPrint={() => doPrint(selectedFilePaths)}
        onCopyTo={() => void doCopyTo(selectedFilePaths)}
        onMoveTo={() => void doMoveTo(selectedFileItems)}
        onDeleteFile={() => void doDelete(selectedFileItems)}
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
          onAddToLibrary={(_path, _folderName, lib) => {
            // 三处侧栏入口统一走 `addDirToLibrary`，才能都经过「问子文件夹」那一步
            if (lib.kind === 'existing') {
              void addDirToLibrary(_path, 'existing', { id: lib.id, name: lib.name })
            } else {
              void addDirToLibrary(_path, 'name')
            }
          }}
          onAskNewLibraryName={(path) => void addDirToLibrary(path, 'new')}
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
          onDoubleClick={resetSide}
        />

        <section className="pv-mg-main">
          {/* 人像墙浮层：点「更多面部」弹出，选一个人就筛主区并关闭 */}
          {personPop && (
            <PersonWall
              anchor={personPop}
              currentId={mgr.personFilter?.id}
              libraryId={
                mgr.scope.kind === 'library'
                  ? mgr.scope.id
                  : mgr.scope.kind === 'tag' || mgr.scope.kind === 'tagGroup'
                    ? mgr.scope.libraryId
                    : undefined
              }
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
          {pipeOpen && (
            <AiPipelineDialog
              library={mgr.scopeTitle}
              onClose={() => setPipeOpen(false)}
              onDone={() => {
                setPipeOpen(false)
                mgr.reload()
              }}
            />
          )}
          <div className="pv-mg-head">
            {/*
              返回上一级（用户 2026-10-31）——**只在浏览模式出现**：
              图库/标签/地点这些范围是「筛出来的集合」，没有「上一级」这个概念，
              给个按钮退回哪里都说不通。
            */}
            {browseScope && (
              <button
                type="button"
                className="pv-mg-back"
                onClick={goUp}
                disabled={!browseParent}
                title={
                  browseParent
                    ? `返回上一级（${baseName(browseParent)}）`
                    : '已经在最上层了'
                }
                aria-label="返回上一级"
              >
                <svg width="16" height="16" viewBox="0 0 16 16" fill="none" aria-hidden>
                  <path
                    d="M10.2 3.2 5.4 8l4.8 4.8"
                    stroke="currentColor"
                    strokeWidth="1.7"
                    strokeLinecap="round"
                    strokeLinejoin="round"
                  />
                </svg>
              </button>
            )}
            <div className={`pv-mg-headl${browseScope ? ' is-browse' : ''}`}>
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
                  {/*
                    只统计**文件**：目录现在也能被选中（用于「加进图库」），
                    但它不能导出 —— 把目录路径塞进 `doExport` 会得到
                    「已复制 0 个」这种莫名其妙的提示。
                  */}
                  {selectedFileCount > 0 && (
                    <>
                      <span className="pv-mg-dim">
                        已选 {selectedFileCount} 张 · {formatFileSize(selectedBytes)}
                      </span>
                      <button
                        type="button"
                        className="pv-btn"
                        disabled={exporting}
                        onClick={() => void doExport(selectedFilePaths)}
                        title="把选中的原图复制到你指定的文件夹（只复制，不改动原图）"
                      >
                        {exporting ? '导出中…' : '导出'}
                      </button>
                    </>
                  )}
                  {/*
                    ⛔ 这里原来有个紫色「加入图库…」按钮（2026-10-30 用户要求删掉）。
                    同一个动作已经收进「文件 → 新建图库 / 加入图库」——
                    放在标题栏既抢眼又和菜单重复，而且只能"新建"，没法选已有图库。
                  */}
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
          (aiEligible ||
            mgr.stats?.formats.length ||
            mgr.cameras.length ||
            mgr.places.length ||
            mgr.years.length) ? (
            <div className="pv-mg-filters">
              {/*
                一键 AI 识别（2026-10-29 用户要求：放在「格式」前面）。
                空图库（没标签没人像）时用户最自然的动作是就地跑一遍，
                而不是「打开设置 → AI 分析 → 选库 → 点两个按钮」。
              */}
              {aiEligible && (
                <button
                  type="button"
                  // ⛔ 不用全局 `.pv-btn`：它高 30px，而筛选行的下拉是 26px，
                  //    混在一起会把整行撑高 4px、人像条跟着变位。
                  //    `.pv-mg-ai` 与 `.pv-mg-fbtn` 同尺寸，只换配色（主色）。
                  className="pv-mg-ai"
                  title="先打标，再识别人像（两步自动串行）"
                  onClick={() => setPipeOpen(true)}
                >
                  AI 识别
                </button>
              )}
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
                    label={formatLabelOf(f.format)}
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
              libraryId={
                mgr.scope.kind === 'library'
                  ? mgr.scope.id
                  : mgr.scope.kind === 'tag' || mgr.scope.kind === 'tagGroup'
                    ? mgr.scope.libraryId
                    : undefined
              }
              onOpenWall={(r) => setPersonPop({ x: Math.max(12, r.right - PERSON_POP_W), y: r.bottom + 6 })}
              onOpenPerson={(id, name) => mgr.setPersonFilter({ id, name })}
            />
          </div>

          {/*
            「我的电脑」：直接铺盘符卡片。单击进入该盘（浏览模式），
            和侧栏点盘符走同一条 `gotoWithReveal`（会顺便把树展开定位过去）。
          */}
          {mgr.scope.kind === 'none' && driveNodes && driveNodes.length > 0 ? (
            <DriveGrid
              nodes={driveNodes}
              icons={shellIcons}
              selected={driveSel}
              onSelect={setDriveSel}
              onOpen={gotoWithReveal}
              tier={mgr.tier}
            />
          ) : mgr.items.length === 0 ? (
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
              style={{ ['--pv-card' as string]: `${thumbWidthOf(mgr.tier)}px` }}
            >
              {mgr.items.map((item, i) => (
                <PhotoCard
                  key={item.key}
                  item={item}
                  tier={mgr.tier}
                  index={i}
                  selected={selected.has(item.key)}
                  // 目录即便在浏览模式也可选（见 `toggle` 的注释）
                  selectable={mgr.canSelect || item.kind === 'dir'}
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
            <ManagerList
              items={mgr.items}
              selected={selected}
              canSelect={mgr.canSelect}
              sort={mgr.sort}
              sortDesc={mgr.sortDesc}
              onToggleSort={mgr.toggleSort}
              onToggleRow={toggle}
              onOpenRow={openItem}
              onRowContextMenu={onCardContextMenu}
              onBlankContextMenu={openBlankMenu}
              listPrefs={listPrefs}
              colResizing={colResizing}
              onGripDown={onColGripDown}
            />
          )}

        </section>
      </div>

      {/* 状态栏：只摆数字，口径全在这里算好（2026-11 拆到 ManagerStatusBar） */}
      <ManagerStatusBar
        idle={mgr.isIdle}
        statsTotal={mgr.stats?.totalCount ?? 0}
        shown={mgr.items.length}
        total={mgr.total}
        browsing={mgr.isBrowsing}
        totalBytes={totalBytes}
        browseTruncated={mgr.browseTruncated}
        fileCount={fileItems.length}
        pendingThumbs={pendingThumbs}
        scanInfo={mgr.scanInfo}
        path={mgr.root ?? browseScope?.path ?? null}
        error={mgr.error}
        selectedCount={selectedCount}
        selectedAllDirs={selectedItems.every((i) => i.kind === 'dir')}
        selectedBytes={selectedBytes}
      />

      {/*
        「是否包含子文件夹」——只在目录有子文件夹时出现（见 `addDirToLibrary`）。
        三个选项是**平级**的，所以用 ConfirmDialog 的三按钮版本：
        Esc / 点遮罩 = 取消（什么都不做），不会像两按钮那样把「取消」当成「只加本层」。
      */}
      {askTree && (
        <ConfirmDialog
          title="是否包含子文件夹？"
          message={
            `「${askTree.name}」下面还有 ${
              askTree.more ? '400 个以上' : `${askTree.subdirs} 个`
            }子文件夹。\n\n` +
            `包含：连同所有层级一起加入图库，可能是几百上千张，会慢一些。\n` +
            `只加本层：只收这个目录里的图片，子文件夹稍后可以单独加。`
          }
          // ⚠️ 主按钮（Enter 默认）给**轻**的那个：递归入库可能很重，不该是默认动作
          okLabel="只加本层"
          extraLabel={`包含子文件夹（${askTree.more ? '400+' : askTree.subdirs}）`}
          cancelLabel="取消"
          onOk={() => askTree.resolve('flat')}
          onExtra={() => askTree.resolve('tree')}
          onCancel={() => askTree.resolve('cancel')}
        />
      )}

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
            void mgr.addFolderToLibrary(
              target.path,
              target.folderName,
              { kind: 'new', name },
              target.recursive,
            )
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
