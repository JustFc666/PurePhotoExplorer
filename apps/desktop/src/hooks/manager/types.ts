/**
 * 图库模式的**类型契约**：hook 与视图之间只通过这里的形状对话。
 *
 * 单独成文件的原因：`useManager` 的本体有上千行，类型声明塞在中间会让
 * 「界面能拿到什么」和「怎么实现的」混在一起 —— 想加个字段得先翻遍逻辑。
 * 现在改契约只看这一个文件，实现改了也不影响它。
 *
 * ⛔ 这里**只放类型**，不放逻辑、不放常量（常量在 `constants.ts`）。
 */

import type {
  FolderNode,
  LibraryItem,
  ManagerCamera,
  ManagerSort,
  ManagerYear,
  PlaceItem,
  Stats,
  TagHit,
  FaceCounts,
  TagItem,
  ThumbTier,
} from '../../ipc/manager'

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
 * `useManager()` 的返回值 —— **视图层唯一依赖的接口**。
 *
 * 字段按「状态 → 范围 → 筛选 → 排序/视图 → 选择 → 动作」分组；
 * 加新动作前先想清楚它属于哪一组，别顺手插到末尾。
 */
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
   * 扫目录并加入图库。
   *
   * ⚠️ 先扫描再挂图库：目录不在 `folders` 表里就挂不上。
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
  renameLibrary: (id: number, name: string) => Promise<void>
  /**
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
  /** 人脸分布：有脸的图 / 单人（恰好 1 张脸）/ 合照（≥2 张脸）；null = 还没拉到 */
  faceCounts: FaceCounts | null
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
   * 重拉**所有元数据**：侧栏（图库 / 目录 / 成员）、统计、标签条、地点 / 相机 / 年份，
   * 顺带 `queryVersion + 1` 让主区重查。
   *
   * ⭐ AI 打标 / 人脸跑完**必须**用它，不能用 `reload()`：
   * 那一次变的不是图片，而是标签和人物。只 `reload` 的话库里明明写进了 22 个人物，
   * 界面上人像条还是一片空白（2026-11-01 用户报「人脸识别没完成」就是这么来的）。
   */
  refreshMeta: () => Promise<void>
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
  /**
   * 按人物筛选（点人像头像）：与标签筛选同层语义，只改网格内容与标题。
   * 再点同一个人 = 取消。
   */
  personFilter: { id: number; name: string } | null
  setPersonFilter: (p: { id: number; name: string } | null) => void
  /** 当前范围的显示名（主区标题用） */
  scopeTitle: string
  /**
   * 当前生效的**叠加条件**描述（人物 / 标签 / 地点 / 搜索），空数组 = 没有。
   *
   * 空态要写清「为什么 0 张」。只写「没有符合当前筛选的照片」时，
   * 用户完全看不出是哪个条件在卡着 —— 2026-11-01 就是这么被绕进去的。
   */
  activeFilters: string[]
}
