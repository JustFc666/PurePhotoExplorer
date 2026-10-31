/**
 * 图库模式（S2 管理器）的状态与动作。
 *
 * 与后端的分工：**后端负责扫描/查询/统计/缩略图**，这里只维护界面状态
 * （当前范围、筛选、排序、档位、视图模式、多选）。
 *
 * ## ⭐⭐ 两条核心规则（2026-10-16 与用户确认的产品语义）
 *
 * ### 1. 浏览 ≠ 入库
 * 文件浏览器里**点目录只做临时预览**（`browseDir` / `scope.kind === 'browse'`）：
 * 直读磁盘列文件（`fs_list_images`）、缩略图只解码不落缓存（`fs_thumbnail`），
 * **不写 `library.db`、不写 L1/L2**。
 * 只有用户**右键 →「加入图库」**才真正入库（`openDir` → `manager_open_folder`）。
 *
 * ### 2. 入库的目录**单层**扫、**只看本目录**
 * - 扫描走 `manager_open_folder`（**不递归**）—— 用递归版点一下盘符就会遍历整块盘；
 * - 查询用 `dirIds: [folderId]` 精确匹配，**不含子目录**。
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { notifyPeopleChanged } from '../lib/peopleEvents'
import {
  managerAddToLibrary,
  managerCount,
  managerCreateSmartLibrary,
  managerDeleteLibrary,
  managerDeleteTag,
  managerFolders,
  managerLibraries,
  managerLibraryMembers,
  managerAddTags,
  managerOpen,
  managerOpenFolder,
  managerCameras,
  managerYears,
  managerPlaces,
  managerQuery,
  managerRemoveFolder,
  managerRemoveTag,
  managerRenameLibrary,
  managerRenameTag,
  managerSetFlag,
  managerSetRating,
  managerStats,
  managerTagCounts,
  managerTagGroups,
  managerTags,
  type FolderNode,
  type ManagerCamera,
  type ManagerYear,
  type ImageRow,
  type LibraryItem,
  type ManagerQuery,
  type ManagerSort,
  type PlaceItem,
  type Stats,
  type TagHit,
  type TagItem,
  type ThumbTier,
} from '../ipc/manager'
import {
  fsListDirs,
  fsListImages,
  fsProbeDimensions,
  fsProbeTimes,
  type DirListing,
  type FsImage,
  type FsNode,
} from '../ipc/browse'
import { mergeDimensions, mergeTakenAt } from './mergeDimensions'
import { compareName, descForSort, makeComparator } from './sortItems'

/**
 * 单页拉取条数。
 *
 * ⚠️ 从 500 提到 20000（≈ 不截断）：卡片有 `content-visibility: auto`，
 * 离屏卡片不参与 layout/paint，所以「多挂几百张」的代价主要在
 * React 建 fiber + 节点内存，而不是渲染。实测 1224 张可接受。
 * ⛔ 再往上（几万）要换**按行开窗** —— 只有把离屏节点删出 DOM 才能让内存与 N 脱钩。
 */
const PAGE_LIMIT = 20_000

/**
 * 浏览大目录时，**首屏只要求后端探这么多个文件的尺寸**。
 *
 * 一个 1224 个文件的目录如果等全部尺寸探完才返回，界面要空白几秒到十几秒
 * （机械盘上每个文件头探测要 10–30ms）。用户其实只需要先看到第一屏。
 * 60 ≈ 三屏，够滚动时一直有尺寸可看；剩下的在后台分批补。
 */
const FIRST_SCREEN_PROBE = 60

/** 后台补尺寸的批大小。一批 240 个 ≈ 2–5 秒，批之间让出一帧保证滚动不卡。 */
const DIM_CHUNK = 240

/**
 * 压缩包扩展名（与后端 `pv_archive::ARCHIVE_EXTS` 保持一致）。
 *
 * 只用于「进这个路径之前先判断要不要拉子目录」这种**省一次 IPC** 的小事。
 * ⛔ 不要拿它做业务判断（能不能打开、是不是图片），那些一律看后端返回。
 */
const ARCHIVE_EXT_RE = /\.(zip|cbz|rar|cbr|7z|cb7)$/i

/** 文件名搜索的防抖时长（ms）——敲每个字符都查一次会白跑很多次 SQL。 */
const SEARCH_DEBOUNCE_MS = 250

/** 当前浏览范围。把**显示名**一起带上，标题就不用回头去查表。 */
export type Scope =
  /** 还没选任何目录（**启动默认态**）：主区空着，等用户去「我的电脑」里点 */
  | { kind: 'none' }
  | { kind: 'all' }
  | { kind: 'folder'; id: number; path: string; name: string }
  | { kind: 'library'; id: number; name: string }
  | { kind: 'browse'; path: string; name: string }
  /**
   * 按标签筛选（侧栏「标签」分区 / 标签条点一级类目）。
   *
   * `libraryId` = **在哪个图库的语境下筛的**。人像条要用它把人像限制在当前图库，
   * 否则从「rednote」点进「人像」会看到全库 34 个人（2026-10-29 用户报的混合问题）。
   */
  | { kind: 'tag'; name: string; libraryId?: number }
  /** 按地点筛选（侧栏「地点」分区点进来，值是离线城市名） */
  | { kind: 'place'; name: string }
  /**
   * 按**标签组**筛选：AI 标签条点二级类目时用。
   * 一个二级类目（如「花」）下有多个词条（樱花/荷花/玫瑰…），
   * 命中任一即可 —— 复用后端的 `tags_any`。
   */
  | { kind: 'tagGroup'; label: string; names: string[]; libraryId?: number }

/**
 * 「加入图库」时把目录归到哪里。
 *
 * - `byFolderName`：用**文件夹名**当图库名（同名图库已存在就挂上去）
 * - `new`：用户输入的新图库名（同名同样复用）
 * - `existing`：挂到指定的已有图库
 */
export type LibChoice =
  | { kind: 'byFolderName' }
  | { kind: 'new'; name: string }
  | { kind: 'existing'; id: number; name: string }

/** 主区视图形态。 */
export type ViewMode = 'grid' | 'list'

/**
 * 网格 / 胶片条的**统一行模型**。
 *
 * 两种来源（已入库的索引行、浏览模式的临时文件）都归一到它，
 * 卡片与胶片条就只认这一种形状，不必各自分支。
 */
export interface GridItem {
  /** React key（索引行 = id，浏览项 = 绝对路径） */
  key: string
  /** 索引 id；浏览模式为 `null` —— 于是不可选中、没有索引类操作 */
  id: number | null
  /** 绝对路径；已入库但卷未挂载时为 `null` */
  path: string | null
  name: string
  size: number | null
  /**
   * 文件修改时间（Unix 纳秒）。
   *
   * 浏览模式靠它排「修改时间」；图库模式由后端 SQL 排，
   * 前端拿不到也不需要（所以是 null）。
   */
  mtime: number | null
  /**
   * 拍摄时间（Unix 秒）。
   *
   * 入库行来自索引 `taken_at`；浏览项在用户选「按拍摄时间」时才由
   * 后台批量读 EXIF 补上，之前是 null（排序会回退到 mtime）。
   */
  takenAt: number | null
  /** 像素尺寸（入库行来自索引；浏览项来自后端探测文件头）；缺为 null */
  width: number | null
  height: number | null
  format: string | null
  /** 星级 0–5（只有入库行有；浏览项恒为 0，也不能打星） */
  rating: number
  /** 旗标 0/1（同上） */
  flag: number
  /**
   * 有没有标签（入库行由列表 SQL 的 `EXISTS(image_tags)` 算出）。
   * 卡片用它决定显示「标签」还是「无匹配」；浏览项恒为 false。
   */
  hasTags: boolean
  /** true = 浏览模式的临时项（未入库） */
  transient: boolean
  /**
   * `dir` = 浏览模式里的**子文件夹**（双击进入，无缩略图）。
   * `archive` = 压缩包（zip/cbz/rar/cbr/7z/cb7），同样双击进入，
   *   进去看到的是**包内图片**（虚拟路径 `包!内部`）。
   * 两者都排在文件前面（照资源管理器的习惯）。
   */
  kind: 'file' | 'dir' | 'archive'
}

/**
 * 索引里的文件夹 → 绝对路径。
 *
 * `folders` 存的是「卷挂载点 + 相对路径」，拼起来才能得到绝对路径
 * （**索引里不存盘符**是这个项目的既定设计，手册 §5.5）。
 * 重扫一个已入库目录时要用它还原路径。
 */
function absOfFolder(f: FolderNode): string | null {
  if (!f.mountPath) return null
  const base = f.mountPath.replace(/\\/g, '/').replace(/\/+$/, '')
  return f.relPath ? `${base}/${f.relPath}` : base
}

/** 路径归一化：`\`→`/`、去掉尾部斜杠、转小写（Windows 路径不区分大小写）。 */
function normPath(p: string): string {
  return p.replace(/\\/g, '/').replace(/\/+$/, '').toLowerCase()
}

export interface ManagerApi {
  /** 后端已打开（建库完成） */
  ready: boolean
  busy: boolean
  error: string | null
  /** 最近打开过（入库）的目录，状态栏显示用 */
  root: string | null
  scanInfo: string | null

  /** 主区要渲染的行（两种来源归一后的结果） */
  items: GridItem[]
  /**
   * 命中总数：入库模式 = 查询命中数；
   * 浏览模式 = 目录里的图片数 **+ 子文件夹数**（网格里两者都显示，
   * 只算图片的话状态栏会出现「1 / 0 个项目」这种怪数字）。
   */
  total: number
  /** 浏览模式下被截断（目录里比显示的多） */
  browseTruncated: boolean

  /** 已加入图库的目录 */
  addedFolders: FolderNode[]
  libraries: LibraryItem[]
  /** 图库 → 成员目录（图库树二级展开用） */
  libMembers: Record<number, FolderNode[]>
  stats: Stats | null

  scope: Scope
  /**
   * 「基础范围」的 key（剥掉标签筛选后的 图库/目录/浏览路径）。
   * 标签条靠它判断范围是否切换；⛔ 别用标签列表指纹（会被莫名触发）。
   */
  scopeBaseKey: string
  /** 当前是否处于「浏览（未入库）」模式 */
  isBrowsing: boolean
  /** 还没选任何目录（启动默认态）—— 主区要显示引导文案 */
  isIdle: boolean
  /** 是否允许多选（浏览 / 未选 都不可 —— 没有可做的索引操作） */
  canSelect: boolean
  /** 选中的格式（空 = 不限） */
  formats: string[]
  sort: ManagerSort
  tier: ThumbTier
  viewMode: ViewMode
  /** 搜索框的即时输入值（防抖前） */
  searchInput: string
  /**
   * 当前选中的**行 key**（`GridItem.key`）。
   *
   * ⚠️ 用 key 而不是索引 id：**浏览模式（未入库）每一行 `id` 都是 `null`**，
   * 按 id 存根本选不中。key 在两种模式下都唯一（入库 = `String(id)`，浏览 = 绝对路径）。
   */
  selectedKeys: string[]
  /** 当前范围能不能重扫（只有已入库的目录才有意义） */
  canRescan: boolean

  /** **浏览**一个目录（点侧栏树）：临时预览，**不索引、不出缓存** */
  browseDir: (path: string, name: string) => Promise<void>
  /**
   * 把一个目录**加入图库**：单层扫描 → 写索引 → 归入某个图库 → 切到它。
   *
   * ⚠️ 先扫描再挂图库：目录不在 `folders` 表里就挂不上。
   */
  /**
   * 扫目录并加入图库。
   *
   * `recursive = true` → 连同**全部子目录**一起入库（用户在「是否包含子文件夹」
   * 里答了「包含」）。默认 false：只加这一层。
   */
  addFolderToLibrary: (
    path: string,
    folderName: string,
    lib: LibChoice,
    recursive?: boolean,
  ) => Promise<void>
  /** 把目录加进**指定 id** 的图库（拖放 / 「添加文件夹…」用） */
  addFolderToLibraryById: (
    path: string,
    folderName: string,
    libId: number,
    libName: string,
  ) => Promise<void>
  /** 改图库名（重名时抛错，调用方负责留在弹窗里让用户改） */
  renameLibrary: (id: number, name: string) => Promise<void>  /**
   * 新建**智能图库**（规则文本，语法见 `pv-lib::rule`）。
   *
   * 建完自动切过去 —— 用户刚建完就想看里面有什么；
   * 规则命中 0 张时主区会显示空态，也正好说明规则太窄。
   */
  createSmartLibrary: (name: string, rule: string) => Promise<number>
  /** 删除图库（只删分组；删的是当前图库时范围退回「全部照片」） */
  deleteLibrary: (id: number) => Promise<void>
  /** 全部标签 + 使用次数（侧栏「标签」分区） */
  tags: TagItem[]
  /** 一级类目 → 去重后的图片张数（AI 标签条上一级chip的数字） */
  tagGroups: Record<string, number>
  /** 切到某个标签：主区只显示带这个标签的图 */
  selectTag: (name: string) => void
  /** 给一批图加标签（不存在则新建）。返回建立的挂载数 */
  addTags: (ids: number[], names: string[]) => Promise<number>
  /** 从一批图移除某个标签（按 id） */
  removeTag: (ids: number[], tagId: number) => Promise<number>
  /** 这批图各自已有哪些标签（标签选择器判断全选/半选） */
  tagCountsFor: (ids: number[]) => Promise<TagHit[]>
  /** 标签改名（重名抛错，调用方留在输入框里让用户改） */
  renameTag: (id: number, name: string) => Promise<void>
  /** 删除标签（拆挂载 + 删标签行，照片不动） */
  deleteTag: (id: number) => Promise<void>
  /** 全部地点 + 张数（侧栏「地点」分区，离线城市名） */
  places: PlaceItem[]
  /** 切到某个地点：主区只显示在那里拍的照片 */
  selectPlace: (name: string) => void
  /** 重扫当前已入库的目录（**不改图库归属**） */
  rescan: () => Promise<void>
  /** 回到「我的电脑」：清空主区（启动默认态） */
  goHome: () => void
  selectAllScope: () => void
  selectLibrary: (id: number, name: string) => void
  /** 切到一个已入库的目录（侧栏「已加入图库」列表点击） */
  selectAddedFolder: (f: FolderNode) => void
  /** 地址栏跳转：已入库走索引，未入库走临时预览 */
  gotoDir: (path: string, name: string) => Promise<void>
  /**
   * 从图库移除一个目录**及其整棵子树**（删索引 + 删缩略图缓存，**不碰原图**）。
   *
   * ⚠️ 破坏性操作，调用方必须先弹确认框。
   */
  removeAddedFolder: (f: FolderNode) => Promise<void>
  toggleFormat: (fmt: string) => void
  clearFormats: () => void
  /** 库里出现过的相机机型（顶部「相机」下拉） */
  cameras: ManagerCamera[]
  /** 已选相机（空 = 不限） */
  camera: string[]
  toggleCamera: (model: string) => void
  clearCameras: () => void
  /** 按一组标签筛选（标签条点一级/二级） */
  selectTagGroup: (label: string, names: string[]) => void
  /** 取消标签组筛选，回到进入前的范围（标签条的「全部」） */
  clearTagGroup: () => void
  /** 拍摄年份（顶部「年份」下拉） */
  years: ManagerYear[]
  yearFilter: number[]
  toggleYear: (y: number) => void
  clearYears: () => void
  /** 已选地点（顶部「地点」下拉，多选） */
  placeFilter: string[]
  togglePlace: (name: string) => void
  clearPlaces: () => void
  /** 标签有无：null=不限 / 'has'=有标签 / 'none'=无标签 */
  tagFilter: 'has' | 'none' | null
  setTagFilter: (v: 'has' | 'none' | null) => void
  setSearchInput: (v: string) => void
  setSort: (s: ManagerSort) => void
  /** 当前方向的**实际**值（已含用户在同一列上翻转的覆盖） */
  sortDesc: boolean
  /**
   * 列头点击用：换列 → 用该列默认方向；**同一列再点 → 翻转**。
   *
   * 与 `setSort` 分开是因为顶栏「排序」菜单里再点同一项不该翻转方向
   * （菜单没有方向指示，翻了用户看不出来）。
   */
  toggleSort: (s: ManagerSort) => void
  setTier: (t: ThumbTier) => void
  setViewMode: (v: ViewMode) => void
  toggleSelect: (key: string) => void
  /** 只选这一张（普通单击的语义） */
  selectOnly: (key: string) => void
  selectAll: () => void
  clearSelection: () => void
  /** 区间选择（Shift + 单击）：从 anchorKey 一路选到 key（追加，不清空已有选择） */
  selectRange: (anchorKey: string, key: string) => void
  /** 批量打星（0–5）。只改索引；改完自动重查（星级筛选要立刻生效）。 */
  setRating: (ids: number[], rating: number) => Promise<number>
  /** 批量打旗标（0/1）。只改索引；改完自动重查。 */
  setFlag: (ids: number[], flag: number) => Promise<number>
  /** 强制重查当前范围（批量删除/重命名后调用，索引已经变了）。 */
  reload: () => void
  /**
   * 只重拉**侧栏元数据**（图库列表 / 已加入目录 / 成员 / 统计），不重查主区。
   *
   * ⭐ 新建图库后必须调它 —— 侧栏的 `libraries` 是挂载时拉的一份快照，
   * 不刷新就看不到刚建的那个。它顺带 `queryVersion + 1`，所以主区不会错。
   */
  refreshLibraries: () => Promise<void>
  /**
   * 让**指定文件**的缩略图重取一次（原地改写后调用）。
   *
   * 只标记这几张，不是全量：刷新时让 500 张卡片同时重取既浪费又会闪烁。
   */
  bumpThumbs: (paths: string[]) => void
  /**
   * 取某个文件的缩略图版本号（0 = 从没被原地改写过）。
   *
   * 文件被**原地改写**（旋转/翻转保存）时 `imageId` / `path` / 行 key 都没变，
   * 只有 mtime 变了 —— 没有版本号的话 `ThumbCanvas` 的缓存键不变，
   * 界面会一直显示**旧画面**。
   */
  thumbRevOf: (path: string | null) => number
  /** 当前范围的显示名（主区标题用） */
  /**
   * 按人物筛选（点人像头像）：与标签筛选同层语义，只改网格内容与标题。
   * 再点同一个人 = 取消。
   */
  personFilter: { id: number; name: string } | null
  setPersonFilter: (p: { id: number; name: string } | null) => void
  scopeTitle: string
}

/** 网格排序的全部取值（设置面板的「默认排序」也用它校验 localStorage 里的历史值）。 */
export const MANAGER_SORTS = [
  'taken',
  'name',
  'size',
  'mtime',
  'format',
  'rating',
  'dims',
  'flag',
  'added',
  'random',
] as const

/** 默认排序的持久化键 */
const SORT_KEY = 'pv.mg.sort'
/**
 * 「设置面板改了默认排序」的事件名。
 *
 * 为什么不用 React 状态往上提：`useManager` 在 `ManagerView` 里，而设置面板在
 * `App` 里 —— 中间隔着好几层组件。一个 window 事件比一路 prop 回调干净，
 * 而且 `ManagerView` 是常驻挂载的，事件是唯一能让它**立刻**反应过来的办法。
 */
export const EVENT_SET_SORT = 'pv:set-sort'

/** 读「默认排序」偏好（没存过 / 存的值不认识 → 拍摄时间）。 */
export function readSortPref(): ManagerSort {
  try {
    const v = localStorage.getItem(SORT_KEY)
    if (v && (MANAGER_SORTS as readonly string[]).includes(v)) return v as ManagerSort
  } catch {
    /* localStorage 不可用 → 用默认值 */
  }
  return 'taken'
}

/** 记住排序偏好，并通知常驻挂载的图库视图立刻跟着变。 */
export function applySortPref(s: ManagerSort): void {
  try {
    localStorage.setItem(SORT_KEY, s)
  } catch {
    /* 忽略：只影响下次启动的默认值 */
  }
  window.dispatchEvent(new CustomEvent(EVENT_SET_SORT, { detail: s }))
}

/* ── 缩略图档位偏好（2026-10-24 新增：以前只在内存里，重启就回默认）─────
 * 档位与像素值的对应关系来自 `pv_index::ThumbTier`，这里只做展示与记忆。
 * 改完不需要事件通知：设置窗口只管写，图库视图下次进入时读它。 */
const TIER_KEY = 'pv.mg.tier'

export const THUMB_TIERS: Array<{ value: ThumbTier; label: string; px: number }> = [
  { value: 'sm', label: '小', px: 116 },
  { value: 'md', label: '中', px: 168 },
  { value: 'lg', label: '大', px: 236 },
  { value: 'xl', label: '特大', px: 320 },
]

const TIER_VALUES = THUMB_TIERS.map((t) => t.value)

/** 读缩略图档位偏好（没存过 / 值不认识 → 中）。 */
export function readThumbTier(): ThumbTier {
  try {
    const v = localStorage.getItem(TIER_KEY)
    if (v && (TIER_VALUES as string[]).includes(v)) return v as ThumbTier
  } catch {
    /* localStorage 不可用 → 默认值 */
  }
  return 'md'
}

/** 记住缩略图档位偏好。 */
export function saveThumbTier(t: ThumbTier): void {
  try {
    localStorage.setItem(TIER_KEY, t)
  } catch {
    /* 忽略：只影响下次启动 */
  }
}

/**
 * 把未知异常变成一句人话。
 *
 * ⚠️ 后端命令失败时抛的是**序列化后的 `ErrorDto` 对象**（有 `message` 字段），
 * 直接 `String(e)` 只会得到 `[object Object]`；这里优先取 `message`。
 * 也导出给视图层用（别在各处各写一遍）。
 */
export function errText(e: unknown): string {
  if (typeof e === 'string') return e
  if (e && typeof e === 'object' && 'message' in e) {
    const m = (e as { message?: unknown }).message
    if (typeof m === 'string') return m
  }
  return String(e)
}

export function useManager(): ManagerApi {
  const [ready, setReady] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [root, setRoot] = useState<string | null>(null)
  const [scanInfo, setScanInfo] = useState<string | null>(null)

  const [rows, setRows] = useState<ImageRow[]>([])
  const [total, setTotal] = useState(0)
  /** 浏览模式的目录列表（临时，不落库） */
  const [listing, setListing] = useState<DirListing | null>(null)
  /** 浏览模式的**子文件夹**（临时；双击进入用） */
  const [subdirs, setSubdirs] = useState<FsNode[]>([])
  /**
   * 浏览会话号。每进一个目录就 +1。
   *
   * 后台补尺寸是**跨多次 await** 的长任务：用户可能在补完之前就切走了，
   * 没有这个号码它会把旧目录的尺寸合并进新目录的列表（串图）。
   */
  const browseToken = useRef(0)
  /**
   * 哪个目录的拍摄时间已经读过了。
   *
   * 读 EXIF 是「每张一次 open」的重活，**同一个目录只该读一遍**：
   * 用户在「拍摄时间 / 文件名」之间来回切时不该重读。
   */
  const timesLoadedFor = useRef<string | null>(null)

  const [addedFolders, setAddedFolders] = useState<FolderNode[]>([])
  const [libraries, setLibraries] = useState<LibraryItem[]>([])
  /** 图库 → 成员目录（图库树二级；refreshMeta 时与目录表拼装） */
  const [libMembers, setLibMembers] = useState<Record<number, FolderNode[]>>({})
  /** 全部标签 + 使用次数（侧栏「标签」分区） */
  /**
   * 进入标签组筛选（tagGroup）**之前**的范围 —— 点「全部」时回到这里。
   * ⚠️ 必须声明在所有 effect 之前：`baseScope` 要用它。
   */
  const scopeBeforeTag = useRef<Scope | null>(null)
  const [tagList, setTagList] = useState<TagItem[]>([])
  /**
   * 一级类目 → **去重**后的图片张数。
   *
   * ⚠️ 不能拿 `tagList` 里二级的张数相加：一张图常同时挂「单人」「全身」，
   * 相加会重复计数（测试图库人像组相加 89，实际只有 52 张）。
   */
  const [tagGroups, setTagGroups] = useState<Record<string, number>>({})
  /** 全部地点 + 张数（侧栏「地点」分区；由后端离线地名库解析） */
  const [placeList, setPlaceList] = useState<PlaceItem[]>([])
  const [stats, setStats] = useState<Stats | null>(null)

  const [scope, setScope] = useState<Scope>({ kind: 'none' })
  const [formats, setFormats] = useState<string[]>([])
  const [cameras, setCameras] = useState<ManagerCamera[]>([])
  const [years, setYears] = useState<ManagerYear[]>([])
  const [yearFilter, setYearFilter] = useState<number[]>([])
  const [placeFilter, setPlaceFilter] = useState<string[]>([])
  const [tagFilter, setTagFilter] = useState<'has' | 'none' | null>(null)
  /**
   * 按**人物**筛选（2026-10-25）：与标签筛选同一层语义 —— 主区还在，
   * 只是把网格筛成"这个人出现过的照片"，标题显示人名。
   * ⛔ 不要打开独立页面：用户要的是"效果和选 tag 一样"。
   */
  const [personFilter, setPersonFilterState] = useState<{ id: number; name: string } | null>(null)
  const [camera, setCamera] = useState<string[]>([])
  const [searchInput, setSearchInput] = useState('')
  const [searchTerm, setSearchTerm] = useState('')
  /**
   * 默认按**拍摄时间**排（新的在前）—— 相册的直觉顺序。
   *
   * 能不能这么默认，取决于 `images.taken_at` 到底有没有数据：
   * - 入库时 `fill_folder_times` 会读 EXIF `DateTimeOriginal` 落库；
   * - 老库启动时 `backfill_times(1500)` 分批补齐。
   * 排序 SQL 是 `COALESCE(taken_at, added_ts)`，所以**没有 EXIF 的图不会消失**，
   * 只是回退到入库时间。护栏 = `pv-store` 的「入库时填上拍摄时间」。
   *
   * ⚠️ 浏览模式（未入库）没有索引，`takenAt` 由 `fsProbeTimes` 按需读，
   * 排序在内存里做（`sortItems.ts`），同一套方向规则。
   *
   * 用户在设置面板改过之后以 `pv.mg.sort` 为准（`readSortPref`）。
   */
  const [sort, setSortState] = useState<ManagerSort>(readSortPref)
  /**
   * 改排序：**顺便记住**（下次启动沿用）。
   *
   * 直接暴露 `useState` 的 setter 也能用，但每次切换都写一次 `localStorage`
   * 正是「记住了」的实现 —— 放在这里，调用点（顶栏/状态栏）不用管持久化。
   */
  /**
   * 用户在同一列上**手动翻转**的方向；`null` = 用该列的默认方向（`descForSort`）。
   *
   * 为什么方向要单独存：`ManagerSort` 只是「按哪一列」，不含方向。
   * 列头点第二次要能升/降互换，这个覆盖值就是那次的翻转结果。
   */
  const [sortDesc, setSortDesc] = useState<boolean | null>(null)
  /** 实际生效的方向（后端 SQL 与浏览模式的内存排序都读它） */
  const descending = sortDesc ?? descForSort(sort)

  const setSort = useCallback((s: ManagerSort) => {
    setSortState(s)
    // 换列 → 回到该列的默认方向（否则「按大小降序」的覆盖会被带到「按文件名」）
    setSortDesc(null)
    try {
      localStorage.setItem(SORT_KEY, s)
    } catch {
      /* 忽略 */
    }
  }, [])

  const toggleSort = useCallback(
    (s: ManagerSort) => {
      if (s !== sort) {
        setSort(s)
        return
      }
      setSortDesc(!descending)
    },
    [sort, descending, setSort],
  )
  /**
   * 设置面板改「默认排序」时**立刻生效**。
   *
   * ManagerView 是**常驻挂载**的（切到单图只是 `display:none`），所以设置面板
   * 改了 localStorage 之后，这个 hook 里的 state 不会自己变 ——
   * 必须靠事件把新值送进来（重新挂载才会重读初值）。
   */
  useEffect(() => {
    const onSet = (e: Event) => {
      const v = (e as CustomEvent<string>).detail
      if ((MANAGER_SORTS as readonly string[]).includes(v)) setSortState(v as ManagerSort)
    }
    window.addEventListener(EVENT_SET_SORT, onSet)
    return () => window.removeEventListener(EVENT_SET_SORT, onSet)
  }, [])
  const [tier, setTierState] = useState<ThumbTier>(readThumbTier)
  /** 换档位顺带记住（设置窗口也写同一个键，两边口径一致） */
  const setTier = useCallback((t: ThumbTier) => {
    saveThumbTier(t)
    setTierState(t)
  }, [])
  const [viewMode, setViewMode] = useState<ViewMode>('grid')
  const [selected, setSelected] = useState<Set<string>>(new Set())
  /** 每次「该重查了」自增，作为查询 effect 的触发器 */
  const [queryVersion, setQueryVersion] = useState(0)
  /** path → 缩略图版本号（只记被原地改写过的文件；见 `thumbRevOf`） */
  const [thumbRevMap, setThumbRevMap] = useState<Map<string, number>>(() => new Map())

  /** 打开过一次就不重复开（`manager_open` 会替换 Store） */
  const opened = useRef(false)
  /**
   * 启动引导的**共享 Promise**。
   *
   * ⚠️ 这里刻意**不用布尔守卫 + `alive` 早退**（最直觉的写法，但它是错的）：
   * React 18 的 StrictMode 会把 effect 跑两遍（mount → cleanup → mount）。
   * 布尔守卫会让第二遍直接早退，而第一遍的异步体在 `await ensureOpen()` 之后
   * 撞上 cleanup 设的 `alive = false` → 直接 return ——
   * **结果是元数据一个都没加载**（现象：格式筛选条不见了、侧栏「全部照片」显示 0）。
   * 存 Promise 则两遍共享同一份工作，且不受 cleanup 影响。
   */
  const bootRef = useRef<Promise<void> | null>(null)

  const ensureOpen = useCallback(async () => {
    if (opened.current) return
    await managerOpen()
    opened.current = true
    setReady(true)
  }, [])

  /** 重载侧栏 / 状态栏数据（已入库目录 + 图库列表 + 成员关系 + 全库统计）。 */
  const refreshMeta = useCallback(async () => {
    const [folders, libs, members, st, tagList, placeList, cams, yrs] = await Promise.all([
      managerFolders(),
      managerLibraries(),
      managerLibraryMembers(),
      managerStats(),
      managerTags(),
      managerPlaces(),
      managerCameras().catch(() => [] as ManagerCamera[]),
      managerYears().catch(() => [] as ManagerYear[]),
    ])
    // 只列「显式加入过」的目录：`open_dir` 会把它标成 is_root
    setAddedFolders(folders.filter((f) => f.isRoot))
    // 图库树的**二级**：图库 → 成员目录。成员可能不是 is_root（保险起见
    // 和**全量**目录表拼装，别只拿已加入的根目录）。
    const byId = new Map(folders.map((f) => [f.id, f]))
    const grouped: Record<number, FolderNode[]> = {}
    for (const m of members) {
      const f = byId.get(m.folderId)
      if (f) (grouped[m.libraryId] ??= []).push(f)
    }
    setLibMembers(grouped)
    setLibraries(libs)
    setTagList(tagList)
    setPlaceList(placeList)
    setStats(st)
    setCameras(cams)
    setYears(yrs)
    setQueryVersion((v) => v + 1)
  }, [])

  /**
   * 后台把剩余文件的**尺寸**补进浏览列表（[`browseDir`] 首屏渲染之后才跑）。
   *
   * 逐批 `await`，批之间让出一帧：探测是实打实的同步 I/O，
   * 一次丢 1200 个进去会把主线程占满几秒（滚动直接卡）。
   *
   * `token` 是浏览会话号 —— 用户切走后必须立刻停手，否则会串图。
   * 探测失败就**静默停掉**：尺寸只是锦上添花，不该弹错。
   */
  const fillRestDimensions = useCallback(async (first: DirListing, token: number) => {
    const items = first.items
    for (let start = FIRST_SCREEN_PROBE; start < items.length; start += DIM_CHUNK) {
      const slice = items.slice(start, start + DIM_CHUNK)
      let dims: Array<[number, number] | null>
      try {
        dims = await fsProbeDimensions(slice.map((s) => s.path))
      } catch {
        return
      }
      if (token !== browseToken.current) return
      // 原地合并：只换 width/height，不重建整个 items（避免整表重渲染）
      setListing((prev) => (prev ? { ...prev, items: mergeDimensions(prev.items, start, dims) } : prev))
      await new Promise<void>((resolve) => setTimeout(resolve, 0))
    }
  }, [])

  /**
   * 后台把**拍摄时间**读进浏览列表。
   *
   * 只在用户选了「按拍摄时间排序」时调用 —— 每个文件要 `open` 一次读 EXIF，
   * 1200 张就是几秒。分批 + 批间让一帧，界面能看到列表逐步归位到正确的时间顺序。
   */
  const fillTakenTimes = useCallback(async (items: FsImage[], token: number) => {
    for (let start = 0; start < items.length; start += DIM_CHUNK) {
      const slice = items.slice(start, start + DIM_CHUNK)
      let times: Array<number | null>
      try {
        times = await fsProbeTimes(slice.map((s) => s.path))
      } catch {
        return // 读不出来就保持现状（排序仍按 mtime 兜底），不打扰用户
      }
      if (token !== browseToken.current) return
      setListing((prev) =>
        prev ? { ...prev, items: mergeTakenAt(prev.items, start, times) } : prev,
      )
      await new Promise<void>((resolve) => setTimeout(resolve, 0))
    }
  }, [])

  /**
   * **浏览**一个目录（侧栏树里点）：临时预览，**绝不碰索引与缓存**。
   *
   * 先切 scope 再取列表 —— 界面立刻切过去并显示加载态，
   * 而不是让用户盯着旧内容等。
   *
   * 同时拉一份**子文件夹**（复用树的 `fs_list_dirs`：已按名排序、跳系统目录、
   * 上限 400）—— 浏览一个目录时主区要能看到里面还有哪些文件夹，双击进入。
   * 两者并行，谁先回谁先上；子文件夹拉失败**不算错**（只少了文件夹入口）。
   *
   * ## ⭐ 首屏优先（2026-10-17）
   * 只要求后端探前 `FIRST_SCREEN_PROBE` 个的尺寸，**拿到就立刻渲染并解除忙碌态**；
   * 剩下的交给 [`fillRestDimensions`] 在后台补。卡片会先显示 `JPEG · 6.4 MB`，
   * 几秒后补成 `2920 × 4524 · JPEG · 6.4 MB`。
   *
   * 不做这件事的话，1224 个文件的目录要等全部探测完（机械盘上十几秒）
   * 才能看到第一张图 —— 而用户其实只关心第一屏。
   */
  const browseDir = useCallback(
    async (path: string, name: string) => {
      const token = ++browseToken.current
      // 新的一次浏览：之前读过的拍摄时间不算数了
      timesLoadedFor.current = null
      setScope({ kind: 'browse', path, name })
      setBusy(true)
      setError(null)
      try {
        /*
         * ⚠️ 压缩包**不拉子目录**：我们不在包里模拟目录层级（进去就是扁平一层图），
         * 而 `fs_list_dirs(包路径)` 会把它当普通目录读 —— 得到空数组，
         * 白白多一次 IPC 和一次 read_dir（在移动盘上还能卡一下）。
         */
        const isArchive = ARCHIVE_EXT_RE.test(path)
        const [first, dirs] = await Promise.all([
          fsListImages(path, FIRST_SCREEN_PROBE),
          isArchive
            ? Promise.resolve([] as FsNode[])
            : fsListDirs(path).catch(() => [] as FsNode[]),
        ])
        if (token !== browseToken.current) return
        setListing(first)
        setSubdirs(dirs)
        // ⭐ 先让界面活过来，剩下的事后台做
        setBusy(false)
        // 包内条目探尺寸要先解压（见 DirListing.isArchive 的注释），一律跳过
        if (!first.isArchive) void fillRestDimensions(first, token)
      } catch (e) {
        if (token !== browseToken.current) return
        setListing(null)
        setSubdirs([])
        setError(errText(e))
      } finally {
        if (token === browseToken.current) setBusy(false)
      }
    },
    [fillRestDimensions],
  )

  /**
   * 浏览模式 + 用户选了「按拍摄时间」→ 后台读 EXIF。
   *
   * 为什么是 effect 而不是在 `setSort` 里做：用户可能在**任何**时候切排序
   * （包括刚从图库切到浏览），统一的触发点只有「当前状态是浏览 + 排序是 taken」。
   *
   * `timesLoadedFor` 保证同一个目录只读一遍 —— 在「拍摄时间 / 文件名」之间
   * 来回切时不该反复读 1200 个文件的 EXIF。
   */
  useEffect(() => {
    if (scope.kind !== 'browse') return
    if (sort !== 'taken') return
    const items = listing?.items
    if (!items || items.length === 0) return
    if (timesLoadedFor.current === scope.path) return
    timesLoadedFor.current = scope.path
    void fillTakenTimes(items, browseToken.current)
  }, [scope, sort, listing, fillTakenTimes])

  /**
   * 把一个目录**加入图库**：单层扫描（不递归）→ 写索引 → 归入图库 → 切到它。
   *
   * `manager_open_folder` 直接返回 `folderId`，不需要拿绝对路径去反查 ——
   * 索引里不存盘符，反查要自己拼「挂载点 + 相对路径」再归一化比对。
   *
   * ⚠️ **顺序**：必须先扫描（目录才会进 `folders` 表），再挂图库。
   */
  const addFolderToLibrary = useCallback(
    async (path: string, folderName: string, lib: LibChoice, recursive = false) => {
      setBusy(true)
      setError(null)
      try {
        await ensureOpen()
        // `recursive` 由用户在「是否包含子文件夹」里选（见 ManagerView::addDirToLibrary）
        const r = await managerOpenFolder(path, recursive)
        // `byFolderName` / `new` 都传 libraryId=null，让后端按名字**同名复用**；
        // 只有 `existing` 才显式给 id。
        //
        // ⚠️ `recursive` 要**再传一次**给挂库：`open_folder` 只负责「把目录扫进索引」，
        // 「挂哪些目录进图库」是另一件事 —— 漏传的话图库只含根目录那一层。
        const libName = lib.kind === 'new' ? lib.name : folderName
        if (lib.kind === 'existing') {
          await managerAddToLibrary(r.folderId, lib.name, lib.id, recursive)
        } else {
          await managerAddToLibrary(r.folderId, libName, null, recursive)
        }
        const target = lib.kind === 'existing' ? lib.name : libName
        setRoot(path)
        const scope = r.folders > 1 ? `（含 ${r.folders} 个目录）` : ''
        setScanInfo(
          `已加入「${target}」${scope}：新增 ${r.added} · 更新 ${r.changed} · 移除 ${r.removed}`,
        )
        setScope({ kind: 'folder', id: r.folderId, path, name: folderName })
        setQueryVersion((v) => v + 1)
        await refreshMeta()
      } catch (e) {
        setError(errText(e))
      } finally {
        setBusy(false)
      }
    },
    [ensureOpen, refreshMeta],
  )

  /**
   * 把一个目录加进**指定 id 的图库**（侧栏拖放 / 右键「添加文件夹…」用）。
   *
   * 与 `addFolderToLibrary` 完全同路（先扫描再挂），只是 lib 固定为 `existing`。
   */
  const addFolderToLibraryById = useCallback(
    (path: string, folderName: string, libId: number, libName: string) =>
      addFolderToLibrary(path, folderName, { kind: 'existing', id: libId, name: libName }),
    [addFolderToLibrary],
  )

  /** 改图库名。只改索引里的名字；改完刷新侧栏（标题/列表都要跟着变）。 */
  const renameLibrary = useCallback(
    async (id: number, name: string) => {
      setBusy(true)
      setError(null)
      try {
        await ensureOpen()
        await managerRenameLibrary(id, name)
        await refreshMeta()
      } catch (e) {
        setError(errText(e))
        throw e
      } finally {
        setBusy(false)
      }
    },
    [ensureOpen, refreshMeta],
  )

  /**
   * 删除图库：**连它独占的目录一起从索引里清掉**（2026-10-31 改）。
   *
   * 旧语义是「只删分组」，但那样删完图库后照片还挂在「全部照片」里 ——
   * 用户反馈要一起清。
   *
   * ⛔ 仍然**绝不碰磁盘上的原图**，只删索引行 + 缩略图缓存；
   *    还被别的图库引用的目录也不会删（后端 `delete_library` 判）。
   *
   * 删的是当前选中的图库时，范围退回「全部照片」，否则用户会盯着一个已不存在的库。
   */
  const deleteLibrary = useCallback(
    async (id: number) => {
      setBusy(true)
      setError(null)
      try {
        await ensureOpen()
        const r = await managerDeleteLibrary(id)
        if (r) {
          setScanInfo(
            `已删除图库：清理 ${r.images} 张索引 · ${r.folders} 个目录 · ${r.thumbs} 个缩略图（原图都没动）`,
          )
        }
        setScope((s) => (s.kind === 'library' && s.id === id ? { kind: 'all' } : s))
        setQueryVersion((v) => v + 1)
        await refreshMeta()
      } catch (e) {
        setError(errText(e))
        throw e
      } finally {
        setBusy(false)
      }
    },
    [ensureOpen, refreshMeta],
  )

  /* ── 标签（2026-10-19）─────────────────────────────────────────
   * 标签只存在索引里：改完 `refreshMeta()` 重拉侧栏计数，主区自动重查。
   * ⚠️ 标签名是**用户随手敲的**，后端 `ensure` 会去空格并同名复用。
   */

  const selectTag = useCallback((name: string) => {
    setScope((prev) => ({
      kind: 'tag',
      name,
      // 只在「已进入某个图库」时把图库带过去；从「全部」点标签仍是全局
      libraryId: prev.kind === 'library' ? prev.id : undefined,
    }))
    setQueryVersion((v) => v + 1)
  }, [])

  const addTags = useCallback(
    async (ids: number[], names: string[]) => {
      if (ids.length === 0 || names.length === 0) return 0
      const n = await managerAddTags(ids, names)
      await refreshMeta()
      return n
    },
    [refreshMeta],
  )

  const removeTag = useCallback(
    async (ids: number[], tagId: number) => {
      if (ids.length === 0) return 0
      const n = await managerRemoveTag(ids, tagId)
      await refreshMeta()
      return n
    },
    [refreshMeta],
  )

  const tagCountsFor = useCallback((ids: number[]) => managerTagCounts(ids), [])

  const renameTag = useCallback(
    async (id: number, name: string) => {
      await managerRenameTag(id, name)
      await refreshMeta()
    },
    [refreshMeta],
  )

  const deleteTag = useCallback(
    async (id: number) => {
      await managerDeleteTag(id)
      // 删的正是当前在看的标签 → 范围退回「全部照片」，别盯着一个不存在的标签
      setScope((s) => (s.kind === 'tag' ? { kind: 'all' } : s))
      await refreshMeta()
    },
    [refreshMeta],
  )

  /* ── 地点（2026-10-19）─────────────────────────────────────────
   * 城市名是**后端离线表**从 GPS 算出来的，前端不参与计算、也不联网。
   * 侧栏列表来自 `manager_places`，回填在 `manager_open` 里已经跑过一轮；
   * 入库新目录后要 `refreshMeta()` 才能看到新地点。
   */

  const selectPlace = useCallback((name: string) => {
    setScope({ kind: 'place', name })
    setQueryVersion((v) => v + 1)
  }, [])

  /**
   * 新建智能图库。规则由调用方拼好（侧栏的预设）；语法错会抛错。
   *
   * 建完 `refreshMeta()` 再 `selectLibrary()` —— **顺序不能反**：
   * 先切范围时侧栏还没有这条图库，标题会退化成空字符串。
   */
  const createSmartLibrary = useCallback(
    async (name: string, rule: string) => {
      setBusy(true)
      setError(null)
      try {
        await ensureOpen()
        const id = await managerCreateSmartLibrary(name, rule)
        await refreshMeta()
        setScope({ kind: 'library', id, name })
        setQueryVersion((v) => v + 1)
        return id
      } catch (e) {
        setError(errText(e))
        throw e
      } finally {
        setBusy(false)
      }
    },
    [ensureOpen, refreshMeta],
  )

  /**
   * 重扫当前范围（**不改图库归属**）。
   *
   * 外部往目录里放了新图之后，索引里还没有它 —— 必须真的扫一遍磁盘才会出现。
   * 四种范围各有各的扫法：
   * - `browse`：重新列目录（**不写索引**，浏览模式本来就不入库）；
   * - `folder`：只重扫这一个已入库目录；
   * - `library`：重扫该图库包含的**每个成员目录**；
   * - `all`：重扫所有已入库的根目录。
   *
   * ⚠️ 卷未挂载（`mountPath == null`）的目录直接跳过 —— 拿不到绝对路径。
   */
  const rescan = useCallback(async () => {
    // 标签范围没有磁盘可扫（标签只存在索引里）
    if (scope.kind === 'none' || scope.kind === 'tag') return
    setBusy(true)
    setError(null)
    try {
      await ensureOpen()

      if (scope.kind === 'browse') {
        // 浏览模式的数据来自 fs_list_images，重列一次即可（不碰索引）
        await browseDir(scope.path, scope.name)
        return
      }

      if (scope.kind === 'folder') {
        const r = await managerOpenFolder(scope.path)
        setRoot(scope.path)
        setScanInfo(`本目录：新增 ${r.added} · 更新 ${r.changed} · 移除 ${r.removed}`)
        setQueryVersion((v) => v + 1)
        await refreshMeta()
        return
      }

      // library / all：逐个重扫成员目录，单目录失败不影响其他
      const targets: FolderNode[] =
        scope.kind === 'library' ? (libMembers[scope.id] ?? []) : addedFolders
      let added = 0
      let changed = 0
      let removed = 0
      let scanned = 0
      for (const f of targets) {
        if (!f.mountPath) continue
        // relPath 可能为空（根目录），拼的时候别多插一个分隔符
        const abs = f.relPath ? f.mountPath + f.relPath : f.mountPath
        try {
          const r = await managerOpenFolder(abs)
          added += r.added
          changed += r.changed
          removed += r.removed
          scanned += 1
        } catch {
          /* 单个目录读不到（权限/掉线）就跳过，别让整批失败 */
        }
      }
      setScanInfo(
        `重扫 ${scanned} 个目录：新增 ${added} · 更新 ${changed} · 移除 ${removed}`,
      )
      setQueryVersion((v) => v + 1)
      await refreshMeta()
    } catch (e) {
      setError(errText(e))
    } finally {
      setBusy(false)
    }
  }, [addedFolders, browseDir, ensureOpen, libMembers, refreshMeta, scope])

  /** 回到「我的电脑」：清空主区（= 启动默认态）。 */
  const goHome = useCallback(() => setScope({ kind: 'none' }), [])

  const selectAllScope = useCallback(() => setScope({ kind: 'all' }), [])
  const selectLibrary = useCallback(
    (id: number, name: string) => setScope({ kind: 'library', id, name }),
    [],
  )
  /** 点侧栏「已加入图库」里的目录：**不再扫描**，直接切过去看。 */
  const selectAddedFolder = useCallback((f: FolderNode) => {
    const path = absOfFolder(f)
    if (!path) return
    setRoot(path)
    setScope({ kind: 'folder', id: f.id, path, name: f.name })
  }, [])

  /**
   * 地址栏跳转。
   *
   * ⭐ **已经入库的目录走索引那条路**（有缩略图缓存、有筛选条），
   * 否则才做临时预览 —— 否则用户敲一个自己明明加过的目录，
   * 却看到「未加入图库」徽标、缩略图还要现解码，像是白加了。
   */
  const gotoDir = useCallback(
    async (path: string, name: string) => {
      const target = normPath(path)
      const hit = addedFolders.find((f) => {
        const abs = absOfFolder(f)
        return abs != null && normPath(abs) === target
      })
      if (hit) selectAddedFolder(hit)
      else await browseDir(path, name)
    },
    [addedFolders, browseDir, selectAddedFolder],
  )

  /**
   * 从图库移除一个目录**及其整棵子树**（删索引 + 删缩略图缓存，**不碰原图**）。
   *
   * ⚠️ 破坏性：确认框由调用方（侧栏右键菜单）负责弹，这里只管执行。
   */
  const removeAddedFolder = useCallback(
    async (f: FolderNode) => {
      setBusy(true)
      setError(null)
      try {
        await ensureOpen()
        const r = await managerRemoveFolder(f.id)
        const extra = r.thumbsOrphan > 0 ? ` · ${r.thumbsOrphan} 张离线未能清缓存` : ''
        setScanInfo(
          `已移除「${f.name}」：目录 ${r.folders} · 照片 ${r.images} · 缓存 ${r.thumbs}${extra}`,
        )
        // 正看着被移除的目录就退回「全部照片」，否则会停在一个已经不存在的范围上
        if (scope.kind === 'folder' && scope.id === f.id) setScope({ kind: 'all' })
        await refreshMeta()
        /*
         * 目录里的照片连同**人脸**一起被删了（`persons` 会少人、`face_count` 会变），
         * 人像条和人像墙各自持有一份人物列表，必须通知它们重拉 ——
         * 否则会一直显示旧的「共 N 人」。
         */
        notifyPeopleChanged()
      } catch (e) {
        setError(errText(e))
      } finally {
        setBusy(false)
      }
    },
    [ensureOpen, refreshMeta, scope],
  )

  /**
   * 启动引导：**打开库 → 读已索引的元数据**（已入库目录、图库列表、格式筛选条、状态栏总量）。
   *
   * ⚠️ 不这么做的话，进入图库模式会看到一片空白：库（`library.db`）是
   * **跨会话持久**的，而 `ensureOpen`/`refreshMeta` 原来只挂在「添加文件夹」上，
   * 于是老用户每次进来都得重新「添加文件夹」一次才看得见自己的照片。
   *
   * ⚠️ 刻意**不自动扫描任何目录**：范围默认停留在「全部照片」，
   * 由用户在侧栏「我的电脑」里自己往下展开选中（产品决策，2026-10-15）。
   */
  useEffect(() => {
    bootRef.current ??= (async () => {
      setBusy(true)
      try {
        await ensureOpen()
        await refreshMeta()
      } catch (e) {
        setError(errText(e))
      } finally {
        setBusy(false)
      }
    })()
  }, [ensureOpen, refreshMeta])

  // 搜索防抖：敲字过程中不发请求
  useEffect(() => {
    const t = window.setTimeout(() => setSearchTerm(searchInput.trim()), SEARCH_DEBOUNCE_MS)
    return () => window.clearTimeout(t)
  }, [searchInput])

  /**
   * 「有效范围」= 剥掉标签筛选那一层之后的范围。
   *
   * ⛔⛔ 点标签条后 `scope.kind` 变成 `tagGroup`，它**自己不带图库/目录**；
   * 直接按 `scope` 推导就得到 undefined → 查询与计数都静悄悄变成**全库**。
   * 现象（2026-10-23 用户报的）：
   * - 标签条写「人像 89」（其实把二级张数相加，同一张图被数了两次）；
   * - 点进去只有 52 张（这个才是真数字）；
   * - 点「截图」标题 149 张、chip 写 139（多出的 10 张来自另一个图库）。
   */
  const baseScope: Scope =
    scope.kind === 'tagGroup' ? (scopeBeforeTag.current ?? { kind: 'none' }) : scope

  const scopeKey =
    scope.kind === 'all'
      ? 'all'
      : scope.kind === 'browse'
        ? `browse:${scope.path}`
        : scope.kind === 'none'
          ? 'none'
          : scope.kind === 'tag'
            ? `tag:${scope.name}`
            : scope.kind === 'place'
              ? `place:${scope.name}`
              : scope.kind === 'tagGroup'
                ? `tagGroup:${scope.label}:${[...scope.names].sort().join('|')}`
              : `${scope.kind}:${scope.id}`
  /**
   * ⭐ 标签计数跟随当前范围：图库 N 张就只数这 N 张里的标签。
   * 之前用的是全库计数，导致「二次元 11 张」和「测试图库 286 张」显示一模一样的数字。
   *
   * ⛔⛔ 点标签筛选后 `scope.kind` 变成 `tagGroup`，这里**不能**直接按它取 lib/dir：
   * 那会得到 `undefined` → 静悄悄退回全库统计。现象是「点进人像后，截图/艺术/动物
   * 的张数各自涨了一点」（涨的正好是别的图库那部分）。要么就回到筛选前那个范围。
   */
  useEffect(() => {
    if (!ready) return
    let alive = true
    const lib = baseScope.kind === 'library' ? baseScope.id : undefined
    const dir = baseScope.kind === 'folder' ? baseScope.id : undefined
    managerTags(lib, dir)
      .then((list) => {
        if (alive) setTagList(list)
      })
      .catch(() => {})
    /*
     * 地点同理：标签会跟着图库切换、地点却一直是全库数字，看着就是「切了图库没反应」。
     * ⭐ 必须传**同一对** (lib, dir)，否则两份计数口径不一致。
     */
    managerPlaces(lib, dir)
      .then((list) => {
        if (alive) setPlaceList(list)
      })
      .catch(() => {})
    // 一级类目的去重张数（后端 SQL 算，前端加不出来）
    managerTagGroups(lib, dir)
      .then((list) => {
        if (!alive) return
        const m: Record<string, number> = {}
        for (const g of list) m[g.l1] = g.count
        setTagGroups(m)
      })
      .catch(() => {})
    return () => {
      alive = false
    }
    /*
     * ⚠️ `queryVersion` 必须在依赖里：`refreshMeta`（移除目录、重扫之后）用的是
     * **无 scope** 的 `managerTags()` / `managerPlaces()`，会把侧栏数字写成全库值。
     * 那之后 `scopeKey` 往往没变，不重跑这个 effect 的话数字就**永远停在全库**上
     * （2026-10-28「切图库后地点不变」的另一半原因）。
     * `refreshMeta` 末尾递增 `queryVersion`，正好当作「重算一次」的信号。
     */
  }, [ready, scopeKey, queryVersion])

  const formatKey = useMemo(() => [...formats].sort().join(','), [formats])
  const cameraKey = useMemo(() => [...camera].sort().join(','), [camera])
  const yearKey = useMemo(() => [...yearFilter].sort().join(','), [yearFilter])
  const placeKey = useMemo(() => [...placeFilter].sort().join(','), [placeFilter])

  /**
   * 查询 effect —— 入库模式的**唯一取数入口**。
   *
   * ⚠️ `browse` 与 `none` 必须**提前返回**：
   * - `browse` 的数据来自 `fs_list_images`，查索引会把它覆盖掉；
   * - `none` 是启动默认态（还没选目录），**不该显示任何图片**。
   */
  useEffect(() => {
    if (!ready || scope.kind === 'browse' || scope.kind === 'none') return
    let alive = true
    setBusy(true)

    const q: ManagerQuery = {
      sort,
      // 实际方向（含用户在同一列上的翻转），与浏览模式的内存排序同一份口径
      desc: descending,
      limit: PAGE_LIMIT,
      offset: 0,
    }
    // ⭐ 只看这个目录**本身**，不含子目录（`dirIds` 是精确匹配）
    if (baseScope.kind === 'folder') q.dirIds = [baseScope.id]
    if (baseScope.kind === 'library') q.libraryId = baseScope.id
    // 标签筛选：后端按**标签名**过滤（`tags_any` = 命中任一即可）
    // ⭐ 如果这个标签是**从某个图库点进去的**，图库范围要一起带上，
    //    否则「rednote → 人像」会查出全库的人像图（2026-10-29 用户报的混合）。
    if (scope.kind === 'tag') {
      q.tagsAny = [scope.name]
      if (scope.libraryId != null) q.libraryId = scope.libraryId
    }
    // 地点筛选：城市名是后端离线解析好的，精确匹配
    if (scope.kind === 'place') q.places = [scope.name]
    if (scope.kind === 'tagGroup') {
      q.tagsAny = scope.names
      if (scope.libraryId != null) q.libraryId = scope.libraryId
    }
    if (formats.length > 0) q.formats = formats
    if (camera.length > 0) q.camera = camera
    if (yearFilter.length > 0) q.years = yearFilter
    // 顶栏「地点」筛选与侧栏 scope=place 互不干扰：这里只作为附加过滤
    if (placeFilter.length > 0) q.places = placeFilter
    if (personFilter) q.personId = personFilter.id
    if (tagFilter) q.hasTags = tagFilter === 'has'
    if (searchTerm) q.nameLike = searchTerm

    Promise.all([managerQuery(q), managerCount(q)])
      .then(([list, n]) => {
        if (!alive) return
        setRows(list)
        setTotal(n)
        setError(null)
      })
      .catch((e) => {
        if (alive) setError(errText(e))
      })
      .finally(() => {
        if (alive) setBusy(false)
      })
    return () => {
      alive = false
    }
    // scopeKey / formatKey 用字符串而不是对象：scope 每次都是新对象，
    // 直接进依赖会无限查询
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ready, queryVersion, scopeKey, formatKey, cameraKey, yearKey, placeKey, tagFilter, personFilter, searchTerm, sort, descending])

  // 换范围 / 换筛选就清空多选 —— 留着上一批的选中项，之后点「导出」会导出看不见的图
  useEffect(() => {
    setSelected(new Set())
  }, [scopeKey, formatKey, cameraKey, yearKey, placeKey, tagFilter, personFilter, searchTerm])

  const isBrowsing = scope.kind === 'browse'
  const isIdle = scope.kind === 'none'

  /** 两种来源归一成同一行模型（见 [`GridItem`]）。 */
  const items = useMemo<GridItem[]>(() => {
    // 启动默认态：不显示任何图片（`rows` 里可能还留着上一次查询的结果）
    if (scope.kind === 'none') return []
    if (scope.kind === 'browse') {
      // 子文件夹排前面（照资源管理器的习惯），然后是图片
      const dirs: GridItem[] = subdirs.map((d) => ({
        // 压缩包也走这条分支（无缩略图、无索引行），只是 kind 不同 ——
        // 卡片据此换图标，双击行为完全一样（进去看）
        key: `${d.kind === 'archive' ? 'arc' : 'dir'}:${d.path ?? d.name}`,
        id: null,
        path: d.path,
        name: d.name,
        // 压缩包有大小（卡片右下角显示），文件夹没有
        size: d.size ?? null,
        mtime: null,
        takenAt: null,
        width: null,
        height: null,
        format: null,
        rating: 0,
        flag: 0,
        hasTags: false,
        transient: true,
        kind: d.kind === 'archive' ? 'archive' : 'dir',
      }))
      // ⭐ 浏览模式的排序在**前端**做：列表是直读磁盘的一次性结果，
      // 切排序不可能重新扫一遍目录（冷盘 1–10 秒）。一页最多 2 万条，
      // 内存排序是毫秒级的。
      const files: GridItem[] = (listing?.items ?? []).map((f) => ({
        key: f.path,
        id: null,
        path: f.path,
        name: f.name,
        size: f.size,
        mtime: f.mtime,
        takenAt: f.takenAt, // 选「按拍摄时间」时由后台补，否则恒为 null
        width: f.width,
        height: f.height,
        format: f.format,
        rating: 0,
        flag: 0,
        hasTags: false,
        transient: true,
        kind: 'file',
      }))
      files.sort(makeComparator<GridItem>(sort, descending))
      // 文件夹没有大小/时间可排，永远按名字（且永远在前）
      dirs.sort((a, b) => compareName(a.name, b.name))
      return [...dirs, ...files]
    }
    return rows.map((r) => ({
      key: String(r.id),
      id: r.id,
      path: r.absPath,
      name: r.relName ?? `#${r.id}`,
      size: r.size,
      mtime: null,
      takenAt: r.takenAt,
      width: r.width,
      height: r.height,
      format: r.format,
      rating: r.rating,
      flag: r.flag,
      hasTags: r.hasTags,
      transient: false,
      kind: 'file',
    }))
  }, [scope.kind, listing, subdirs, rows, sort])

  // 浏览模式：图片数 + 子文件夹数（网格里两者都显示）
  const shownTotal = isIdle ? 0 : isBrowsing ? (listing?.total ?? 0) + subdirs.length : total

  const toggleFormat = useCallback((fmt: string) => {
    setFormats((prev) => (prev.includes(fmt) ? prev.filter((f) => f !== fmt) : [...prev, fmt]))
  }, [])
  const clearFormats = useCallback(() => setFormats([]), [])
  const toggleCamera = useCallback((model: string) => {
    setCamera((prev) => (prev.includes(model) ? prev.filter((m) => m !== model) : [...prev, model]))
  }, [])
  const clearCameras = useCallback(() => setCamera([]), [])
  const toggleYear = useCallback((y: number) => {
    setYearFilter((prev) => (prev.includes(y) ? prev.filter((v) => v !== y) : [...prev, y]))
  }, [])
  const clearYears = useCallback(() => setYearFilter([]), [])
  const togglePlace = useCallback((name: string) => {
    setPlaceFilter((prev) => (prev.includes(name) ? prev.filter((v) => v !== name) : [...prev, name]))
  }, [])
  const clearPlaces = useCallback(() => setPlaceFilter([]), [])
  /** 标签条点二级类目：按这一组标签筛选（命中任一） */
  const selectTagGroup = useCallback((label: string, names: string[]) => {
    if (names.length === 0) return
    setScope((prev) => {
      if (prev.kind !== 'tagGroup') scopeBeforeTag.current = prev
      // 图库上下文：当前就在图库 → 用它；已经在标签筛选里 → 继承原来的；否则全局
      const libId =
        prev.kind === 'library'
          ? prev.id
          : prev.kind === 'tagGroup'
            ? prev.libraryId
            : scopeBeforeTag.current?.kind === 'library'
              ? scopeBeforeTag.current.id
              : undefined
      return { kind: 'tagGroup', label, names, libraryId: libId }
    })
  }, [])
  const clearTagGroup = useCallback(() => {
    // ⛔ 2026-10-24 bug：原来无条件恢复 `scopeBeforeTag.current ?? { kind: 'none' }`，
    // 而那个 ref 只在**进入标签筛选时**才被赋值。用户路径是
    // 「我的电脑 → 测试图库 → 点全部」，此时 scope 还是 library、ref 还是 null，
    // 于是直接被踢回「我的电脑」。必须**只在标签筛选态下**才恢复，
    // 否则保持当前范围（点全部 = 看这个图库的全部照片）。
    setScope((prev) =>
      prev.kind === 'tagGroup' ? (scopeBeforeTag.current ?? { kind: 'none' }) : prev,
    )
  }, [])

  const toggleSelect = useCallback((key: string) => {
    setSelected((prev) => {
      const next = new Set(prev)
      if (next.has(key)) next.delete(key)
      else next.add(key)
      return next
    })
  }, [])
  const selectOnly = useCallback((key: string) => setSelected(new Set([key])), [])
  /**
   * 全选**当前可见的图片**（不含文件夹行 —— 目录没有文件级操作可做）。
   * 浏览模式同样适用：`items` 里就是磁盘上的这批文件。
   */
  const selectAll = useCallback(
    () => setSelected(new Set(items.filter((i) => i.kind !== 'dir').map((i) => i.key))),
    [items],
  )
  /**
   * 区间选择（`Shift + 单击`）：把 anchor 到 key 之间的**全部项**加进已选。
   *
   * ## 为什么是「追加」而不是「替换」
   * 资源管理器的 Shift 是替换语义，但那是**单段选择**的思维。这里按住 Shift
   * 连点几下可以拼出好几段（先选 1–20，再选 50–80），对「大量选取」更实用，
   * 而且不会一不小心把前面选的清掉。
   *
   * 找不到 anchor（比如切过范围、那项已经不在列表里）就退化成「只选这一项」——
   * 总比什么都不发生好。
   */
  const selectRange = useCallback(
    (anchorKey: string, key: string) => {
      setSelected((prev) => {
        const next = new Set(prev)
        const a = items.findIndex((i) => i.key === anchorKey)
        const b = items.findIndex((i) => i.key === key)
        if (a < 0 || b < 0) {
          next.add(key)
          return next
        }
        const [lo, hi] = a <= b ? [a, b] : [b, a]
        for (let i = lo; i <= hi; i++) {
          const it = items[i]
          if (it) next.add(it.key)
        }
        return next
      })
    },
    [items],
  )

  const clearSelection = useCallback(() => setSelected(new Set()), [])

  /**
   * 批量打星 / 打旗标。
   *
   * ⭐ 改完**必须 `bumpQuery()` 重查**：如果当前开着「按星级筛选」，
   * 一张被降到 3 星的照片应该立刻从列表里消失 —— 只就地改 `items` 里的字段
   * 做不到这一点（行还在，只是星星变了）。
   */
  const setRating = useCallback(
    async (ids: number[], rating: number) => {
      if (ids.length === 0) return 0
      const n = await managerSetRating(ids, rating)
      setQueryVersion((v) => v + 1)
      return n
    },
    [],
  )

  const setFlag = useCallback(async (ids: number[], flag: number) => {
    if (ids.length === 0) return 0
    const n = await managerSetFlag(ids, flag)
    setQueryVersion((v) => v + 1)
    return n
  }, [])

  /**
   * 强制重查。
   *
   * ⚠️ 批量删除 / 重命名**改的是索引本身**，不能只就地改 `items` 里的字段 ——
   * 已删的行必须真的从结果里消失，所以要再查一次。
   */
  const reload = useCallback(() => setQueryVersion((v) => v + 1), [])

  const bumpThumbs = useCallback((paths: string[]) => {
    if (paths.length === 0) return
    setThumbRevMap((prev) => {
      const next = new Map(prev)
      for (const p of paths) next.set(p, (next.get(p) ?? 0) + 1)
      return next
    })
  }, [])

  const thumbRevOf = useCallback(
    (path: string | null) => (path ? (thumbRevMap.get(path) ?? 0) : 0),
    [thumbRevMap],
  )

  const selectedKeys = useMemo(() => [...selected], [selected])

  return {
    ready,
    busy,
    error,
    root,
    scanInfo,
    items,
    total: shownTotal,
    browseTruncated: isBrowsing && (listing?.items.length ?? 0) < (listing?.total ?? 0),
    addedFolders,
    libraries,
    libMembers,
    stats,
    scope,
    /**
     * 「基础范围」的 key（剥掉标签筛选那一层之后的 图库/目录/浏览路径）。
     * 标签条用它判断"范围是不是换了"——换了才退回一级列表。
     * ⛔ 不要用标签列表的指纹判断：实测它会被莫名触发（2026-10-24 计数器跑到 6 次），
     *    导致用户点了一级芯片后二级视图被反复弹回一级。
     */
    scopeBaseKey:
      baseScope.kind === 'library'
        ? `library:${baseScope.id}`
        : baseScope.kind === 'folder'
          ? `folder:${baseScope.id}`
          : baseScope.kind === 'browse'
            ? `browse:${baseScope.path}`
            : baseScope.kind,
    isBrowsing,
    isIdle,
    // 浏览模式也能选：导出/转格式/摆正/打印都是**文件级**操作，不需要索引行
    canSelect: !isIdle,
    formats,
    sort,
    tier,
    viewMode,
    searchInput,
    selectedKeys,
    canRescan: scope.kind === 'folder',
    browseDir,
    addFolderToLibrary,
    addFolderToLibraryById,
    renameLibrary,
    deleteLibrary,
    createSmartLibrary,
    tags: tagList,
    tagGroups,
    selectTag,
    addTags,
    removeTag,
    tagCountsFor,
    renameTag,
    deleteTag,
    places: placeList,
    selectPlace,
    rescan,
    goHome,
    selectAllScope,
    selectLibrary,
    selectAddedFolder,
    gotoDir,
    removeAddedFolder,
    toggleFormat,
    clearFormats,
    cameras,
    camera,
    toggleCamera,
    clearCameras,
    years,
    yearFilter,
    toggleYear,
    clearYears,
    placeFilter,
    togglePlace,
    clearPlaces,
    tagFilter,
    personFilter,
    /** 点人脸 = 按这个人筛（再点同一个人 = 取消） */
    setPersonFilter: (p: { id: number; name: string } | null) => {
      setPersonFilterState((prev) => (prev?.id === p?.id ? null : p))
      setQueryVersion((v) => v + 1)
    },
    setTagFilter,
    selectTagGroup,
    clearTagGroup,
    setSearchInput,
    setSort,
    sortDesc: descending,
    toggleSort,
    setTier,
    setViewMode,
    toggleSelect,
    selectOnly,
    selectAll,
    clearSelection,
    selectRange,
    setRating,
    setFlag,
    reload,
    refreshLibraries: refreshMeta,
    bumpThumbs,
    thumbRevOf,
    scopeTitle:
      personFilter
        ? personFilter.name
        : scope.kind === 'all'
        ? '全部照片'
        : scope.kind === 'none'
          ? '我的电脑'
          : scope.kind === 'tagGroup'
            ? scope.label
            : scope.name,
  }
}
