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
 *
 * ## 文件已按域拆开（2026-11）
 * - `manager/types.ts`    → `Scope` / `LibChoice` / `ViewMode` / `GridItem` / `ManagerApi`
 * - `manager/constants.ts`→ 页码、探测批大小、防抖时长等调参常量
 * - `manager/prefs.ts`    → 排序与档位的 localStorage 读写（设置窗口也用）
 * - `manager/utils.ts`    → `absOfFolder` / `normPath` / `errText`
 *
 * 本文件只留 hook 本体。⛔ 别把类型再搬回来 —— 上千行逻辑里夹类型声明，
 * 想知道「界面能拿到什么」就得翻遍全文。
 *
 * ⚠️ 对外**路径不变**：下方 `export ... from` 把上面那些原样再导出，
 * 调用方继续 `from 'hooks/useManager'` 即可，不必跟着改。
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
  type ManagerSort,
  type PlaceItem,
  type Stats,
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
import { descForSort } from './sortItems'
import { ARCHIVE_EXT_RE, DIM_CHUNK, FIRST_SCREEN_PROBE, SEARCH_DEBOUNCE_MS } from './manager/constants'
import { baseKeyOf, baseScopeOf, buildManagerQuery, scopeKeyOf } from './manager/query'
import { buildGridItems } from './manager/gridItems'
import { absOfFolder, errText, normPath } from './manager/utils'
import { EVENT_SET_SORT, MANAGER_SORTS, readSortPref, readThumbTier, saveThumbTier, writeSortPref } from './manager/prefs'
import type { GridItem, LibChoice, ManagerApi, Scope, ViewMode } from './manager/types'
import { filtersToClearOnScopeChange, describeActiveFilters } from '../lib/scopeSwitch'

/* ── 再导出：对外 API 面与拆分前完全一致 ─────────────────────────── */
export type { GridItem, LibChoice, ManagerApi, Scope, ViewMode } from './manager/types'
export {
  EVENT_SET_SORT,
  MANAGER_SORTS,
  THUMB_TIERS,
  applySortPref,
  readSortPref,
  readThumbTier,
  saveThumbTier,
} from './manager/prefs'
export { errText } from './manager/utils'

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

  /**
   * 换范围 —— **所有切换范围的入口都必须走这里**，别直接调 `setScope`。
   *
   * ⛔⛔ 人物筛选不是 `scope` 的一部分，它是**叠在 scope 上的额外条件**。
   * 2026-11-01 用户报「点过图像、正常浏览后切回别的图库，显示不了图片」：
   * 点过人像头像 → `personFilter` 有值；再点侧栏另一个图库，条件就成了
   * 「新图库 + 那个人」—— 那人没在这个库里拍过，结果恒为 0 张。
   * 更迷惑的是标题读的是 `personFilter.name`，于是写着「未命名」，
   * 看着像整个图库的数据都坏了。
   */
  const changeScope = useCallback((next: Scope | ((prev: Scope) => Scope)) => {
    const clear = filtersToClearOnScopeChange()
    if (clear.person) setPersonFilterState(null)
    if (clear.tag) setTagFilter(null)
    if (clear.place) setPlaceFilter([])
    if (clear.search) {
      setSearchTerm('')
      setSearchInput('')
    }
    setScope(next)
  }, [])

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
    // 只写不广播：本 hook 就在 `EVENT_SET_SORT` 的监听端，广播等于自激一次
    writeSortPref(s)
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
      changeScope({ kind: 'browse', path, name })
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
        changeScope({ kind: 'folder', id: r.folderId, path, name: folderName })
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
        changeScope((s) => (s.kind === 'library' && s.id === id ? { kind: 'all' } : s))
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
    changeScope((prev) => ({
      kind: 'tag',
      name,
      // 只在「已进入某个图库」时把图库带过去；从「全部」点标签仍是全局
      libraryId: prev.kind === 'library' ? prev.id : undefined,
    }))
    // 用户已经指名道姓要某个标签了，标签条上的「无标签 / 有标签」开关就该失效
    // （留着的话会拼出自相矛盾的条件，主区恒空）
    setTagFilter(null)
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
      changeScope((s) => (s.kind === 'tag' ? { kind: 'all' } : s))
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
    changeScope({ kind: 'place', name })
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
        changeScope({ kind: 'library', id, name })
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
  const goHome = useCallback(() => changeScope({ kind: 'none' }), [])

  const selectAllScope = useCallback(() => changeScope({ kind: 'all' }), [])
  const selectLibrary = useCallback(
    (id: number, name: string) => changeScope({ kind: 'library', id, name }),
    [],
  )
  /** 点侧栏「已加入图库」里的目录：**不再扫描**，直接切过去看。 */
  const selectAddedFolder = useCallback((f: FolderNode) => {
    const path = absOfFolder(f)
    if (!path) return
    setRoot(path)
    changeScope({ kind: 'folder', id: f.id, path, name: f.name })
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
        if (scope.kind === 'folder' && scope.id === f.id) changeScope({ kind: 'all' })
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
  const baseScope: Scope = baseScopeOf(scope, scopeBeforeTag.current)
  const scopeKey = scopeKeyOf(scope)
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
   * ⚠️ `browse` 与 `none` 由 `buildManagerQuery` 返回 `null` 提前挡掉：
   * - `browse` 的数据来自 `fs_list_images`，查索引会把它覆盖掉；
   * - `none` 是启动默认态（还没选目录），**不该显示任何图片**。
   *
   * 拼参数的规则全在 `manager/query.ts`（那里有单测），这里只管发请求与状态。
   */
  useEffect(() => {
    if (!ready) return
    const q = buildManagerQuery({
      scope,
      baseScope,
      formats,
      camera,
      years: yearFilter,
      placeFilter,
      personId: personFilter?.id ?? null,
      tagFilter,
      searchTerm,
      sort,
      desc: descending,
    })
    if (!q) return
    let alive = true
    setBusy(true)

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

  /** 两种来源归一成同一行模型（规则见 `manager/gridItems.ts`）。 */
  const items = useMemo<GridItem[]>(
    () =>
      buildGridItems({
        scopeKind: scope.kind,
        listing,
        subdirs,
        rows,
        sort,
        desc: descending,
      }),
    // ⚠️ `descending` 必须在依赖里：浏览模式的排序是在这里做的（不走后端查询），
    // 漏了它 → 同一列上点第二次（只翻方向、`sort` 不变）列表纹丝不动。
    [scope.kind, listing, subdirs, rows, sort, descending],
  )

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
    changeScope((prev) => {
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
    // 同 `selectTag`：已经按具体标签筛了，「无标签」开关必须让路
    setTagFilter(null)
  }, [])
  const clearTagGroup = useCallback(() => {
    // ⛔ 2026-10-24 bug：原来无条件恢复 `scopeBeforeTag.current ?? { kind: 'none' }`，
    // 而那个 ref 只在**进入标签筛选时**才被赋值。用户路径是
    // 「我的电脑 → 测试图库 → 点全部」，此时 scope 还是 library、ref 还是 null，
    // 于是直接被踢回「我的电脑」。必须**只在标签筛选态下**才恢复，
    // 否则保持当前范围（点全部 = 看这个图库的全部照片）。
    changeScope((prev) =>
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
     * ⛔ 不要用标签列表的指纹判断（原因见 `manager/query.ts::baseKeyOf`）。
     */
    scopeBaseKey: baseKeyOf(baseScope),
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
    /*
     * `reload` 只把图片列表重查一遍；带标签 / 筛选条 / 侧栏计数的**元数据**
     * 全在这个里。AI 跑完必须用这个 —— 只 reload 的话，新标签和新人物
     * 在界面上一个都不会出现（2026-11-01 用户报「人脸识别没完成」的真因）。
     */
    refreshMeta,
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
    /*
     * 空态用来解释「为什么一张都没有」。只列真正在生效的条件，
     * 顺序按用户点选的先后直觉：人 → 标签 → 地点 → 搜索。
     */
    activeFilters: describeActiveFilters({
      personName: personFilter?.name,
      tagFilter,
      places: placeFilter,
      search: searchTerm,
    }),
  }
}
