/**
 * 管理器（S2 图库）IPC 封装。
 *
 * 缩略图与单图浏览走**同一条 PVI1 裸字节通道**（零 base64），
 * 因此直接复用 `parsePixels`。
 *
 * ⚠️ 字段名必须与 Rust 侧 `commands/manager.rs` 的 DTO 一致
 * （`camelCase`）；改 DTO 记得同步这里。
 */

import { invoke } from '@tauri-apps/api/core'
import { parsePixels, toBytes } from './protocol'
import type { DecodedImage } from './types'

/** 缩略图档位（与 `pv_index::ThumbTier::as_str` 一致）。 */
export type ThumbTier = 'sm' | 'md' | 'lg' | 'xl'

/** 排序键（与 `manager.rs::parse_sort` 一致）。 */
export type ManagerSort =
  | 'taken'
  | 'name'
  | 'size'
  | 'mtime'
  /** 按格式标签（jpeg/png/raw…），同类型聚在一起 */
  | 'format'
  /** 按分辨率（宽 × 高 的像素总数） */
  | 'dims'
  /** 按旗标（0 / 1） */
  | 'flag'
  | 'rating'
  | 'added'
  | 'random'

export interface ScanSummary {
  folders: number
  added: number
  changed: number
  removed: number
}

export interface ImageRow {
  id: number
  dirId: number
  relName: string | null
  /** 绝对路径；所在卷未挂载时为 null */
  absPath: string | null
  size: number | null
  width: number | null
  height: number | null
  format: string | null
  isRaw: boolean
  takenAt: number | null
  rating: number
  flag: number
  /** 这张图有没有标签（卡片显示「无匹配」用） */
  hasTags: boolean
}

export interface ManagerQuery {
  dirIds?: number[]
  libraryId?: number
  /** 只看某个人物的照片（人像专栏点进来；后端用 IN 子查询，合影不会重复） */
  personId?: number
  /** 文件名子串（大小写不敏感；`%`/`_` 由后端转义） */
  nameLike?: string
  formats?: string[]
  isRaw?: boolean
  minRating?: number
  flag?: number
  takenFrom?: number
  takenTo?: number
  minSize?: number
  maxSize?: number
  tagsAll?: string[]
  tagsAny?: string[]
  hasGps?: boolean
  /** 地点名（离线城市名），任一命中即可 */
  places?: string[]
  /** 相机机型，任一命中即可（顶部「相机」下拉） */
  camera?: string[]
  /** 拍摄年份，任一命中即可（顶部「年份」下拉） */
  years?: number[]
  /** 标签有无：true=有标签，false=无标签 */
  hasTags?: boolean
  sort?: ManagerSort
  desc?: boolean
  limit?: number
  offset?: number
}

/** 相机机型 + 张数（顶部「相机」下拉） */
export interface ManagerCamera {
  model: string
  count: number
}

/** 单张图的标签（含置信度） */
export interface ImageTagInfo {
  name: string
  /** ai / exif / user */
  source: string
  /** EXIF 与手动标签没有分数 → null */
  confidence: number | null
}

/** 年份 + 张数（顶部「年份」下拉） */
export interface ManagerYear {
  year: number
  count: number
}

/** 侧栏「文件夹」树节点（扁平表，`parentId` 表达层级）。 */
export interface FolderNode {
  id: number
  parentId: number | null
  name: string
  relPath: string
  /** **直属**图像数（不含子目录；递归数由前端按 `parentId` 汇总） */
  imageCount: number
  isRoot: boolean
  volumeId: number
  volumeLabel: string
  /** 卷未挂载时为 null（离线） */
  mountPath: string | null
}

/** 侧栏「图库」条目。 */
export interface LibraryItem {
  id: number
  name: string
  /** normal / smart / encrypted */
  kind: string
  rule: string | null
  count: number
}

/** 单个格式的统计。 */
export interface FormatStat {
  format: string
  count: number
  bytes: number
}

/** 全库统计（状态栏 + 筛选条）。 */
export interface Stats {
  totalCount: number
  totalBytes: number
  formats: FormatStat[]
}

/** 打开管理器（建库 + 缓存目录）；返回实际数据目录。 */
export async function managerOpen(dataDir?: string): Promise<string> {
  return invoke<string>('manager_open', { dataDir: dataDir ?? null })
}

/** 递归首扫某目录。 */
export async function managerScan(root: string): Promise<ScanSummary> {
  return invoke<ScanSummary>('manager_scan', { root })
}

/** 打开单个目录的结果。 */
export interface OpenFolderResult {
  /** 索引里的文件夹 id —— 拿它做「只看这个目录」的查询 */
  folderId: number
  added: number
  changed: number
  removed: number
}

/**
 * **打开单个目录**（不递归）：登记并扫描该目录里的图片。
 *
 * ⚠️ 与 [`managerScan`]（递归首扫）是两件事。文件浏览器里点目录必须走这个 ——
 * 用递归版点一下盘符就会遍历整块盘。
 */
export async function managerOpenFolder(path: string): Promise<OpenFolderResult> {
  return invoke<OpenFolderResult>('manager_open_folder', { path })
}

/** 从图库移除的结果。 */
export interface RemovedResult {
  /** 删掉的目录行数（整棵子树，含自身） */
  folders: number
  images: number
  /** 顺带清掉的缩略图缓存文件数 */
  thumbs: number
  /** 因为卷未挂载而没能清缓存的图片数 */
  thumbsOrphan: number
}

/**
 * 从图库移除一个目录**及其整棵子树**（删索引 + 删对应缩略图缓存）。
 *
 * ⚠️ **只动索引与缓存，绝不碰原始图片文件**。幂等：目录已不在库里时返回全 0。
 */
export async function managerRemoveFolder(folderId: number): Promise<RemovedResult> {
  return invoke<RemovedResult>('manager_remove_folder', { folderId })
}

/**
 * 把一个**已入库**的目录归入图库，返回图库 id。
 *
 * - `libraryId = null` → 按 `name` **同名复用**（已有同名图库就挂上去，没有才新建）
 * - `libraryId = 某个 id` → 挂到该图库
 *
 * ⚠️ 目录必须**先**用 `managerOpenFolder` 扫进索引，否则挂不上
 * （`library_folders.folder_id` 指着 `folders` 表）。
 */
export async function managerAddToLibrary(
  folderId: number,
  name: string,
  libraryId: number | null,
): Promise<number> {
  return invoke<number>('manager_add_to_library', { folderId, name, libraryId })
}

/** 组合查询（过滤/排序/分页）。 */
export async function managerQuery(query: ManagerQuery): Promise<ImageRow[]> {
  return invoke<ImageRow[]>('manager_query', { query })
}

/** 命中数量（"共 N 张"）。 */
export async function managerCount(query: ManagerQuery): Promise<number> {
  return invoke<number>('manager_count', { query })
}

/**
 * 侧栏「文件夹」树（扁平表）。
 *
 * ⚠️ **当前界面没有用它** —— 侧栏已经换成文件浏览器（`ipc/browse.ts` 的
 * `fs_roots`/`fs_list_dirs`），目录 → 图片的定位改由 `managerOpenFolder`
 * 直接给出 `folderId`。保留是因为后端 `Store::folders()` 是服务层的正常能力
 * （「按已索引目录管理」这类视图会需要它），删掉反而要连带砍掉服务层方法。
 */
export async function managerFolders(): Promise<FolderNode[]> {
  return invoke<FolderNode[]>('manager_folders')
}

/** 侧栏「图库」列表。 */
export async function managerLibraries(): Promise<LibraryItem[]> {
  return invoke<LibraryItem[]>('manager_libraries')
}

/** 图库的成员关系（图库树**二级**：图库 → 它包含的目录）。 */
export interface LibraryMember {
  libraryId: number
  folderId: number
}

/** 全部图库的成员关系；目录的名称/路径/张数和 `managerFolders()` 的行按 folderId 拼装。 */
export async function managerLibraryMembers(): Promise<LibraryMember[]> {
  return invoke<LibraryMember[]>('manager_library_members')
}

/** 全库统计（张数 / 总字节 / 分格式）。 */
export async function managerStats(): Promise<Stats> {
  return invoke<Stats>('manager_stats')
}

/** 取缩略图 → RGBA（PVI1 裸字节解析）。 */
export async function managerThumbnail(imageId: number, tier: ThumbTier): Promise<DecodedImage> {
  const raw = await invoke<unknown>('manager_thumbnail', { imageId, tier })
  return parsePixels(toBytes(raw))
}

/** 导出失败的一张 */
export interface ExportFailure {
  name: string
  reason: string
}

/** 批量导出的结果 */
export interface ExportSummary {
  copied: number
  skipped: number
  failed: ExportFailure[]
  destDir: string
}

/**
 * 批量导出：把选中的**原图**复制到 `destDir`。
 *
 * ⚠️ **只复制**，不移动、不删除；索引和原图都不受影响。
 * 重名自动退成 `name (1).jpg`，源目录 == 目标目录的计入 `skipped`。
 */
export async function managerExportImages(
  paths: string[],
  destDir: string,
): Promise<ExportSummary> {
  return invoke<ExportSummary>('manager_export_images', { paths, destDir })
}

/** 在系统资源管理器里选中该文件（`explorer /select`）。 */
export async function revealInExplorer(path: string): Promise<void> {
  return invoke<void>('reveal_in_explorer', { path })
}

/** 数据目录（`%LOCALAPPDATA%\PurePhotoExplorer`）：索引库、缩略图缓存、人脸向量都在这里。 */
export async function appDataDir(): Promise<string> {
  return invoke<string>('app_data_dir')
}

/** 批量设置星级（0–5）。**只改索引**，不动原图、不动缓存。返回改动行数。 */
export async function managerSetRating(ids: number[], rating: number): Promise<number> {
  return invoke<number>('manager_set_rating', { ids, rating })
}

/** 批量设置旗标（0/1）。**只改索引**。返回改动行数。 */
export async function managerSetFlag(ids: number[], flag: number): Promise<number> {
  return invoke<number>('manager_set_flag', { ids, flag })
}

/** 批量操作单条失败 */
export interface BatchFailure {
  name: string
  reason: string
}

/** 批量操作结果（转格式 / 摆正 / 打印共用） */
export interface BatchSummary {
  done: number
  skipped: number
  failed: BatchFailure[]
  destDir: string | null
}

/**
 * 批量转格式：解码原图后按目标扩展名重新编码，输出到 `destDir`。
 * ⚠️ 与「导出」不同：导出是字节级复制（无损），转格式是**重新编码**（有损）。
 */
export async function managerConvertImages(
  paths: string[],
  destDir: string,
  format: string,
): Promise<BatchSummary> {
  return invoke<BatchSummary>('manager_convert_images', { paths, destDir, format })
}

/**
 * 按 EXIF `Orientation` 批量摆正。
 * `overwrite = true` 写回原文件；否则输出到 `destDir`。
 * 方向 2/4/5/7 需要镜像，会被跳过并计入 `failed`。
 */
export async function managerAutoOrient(
  paths: string[],
  destDir: string | null,
  overwrite: boolean,
): Promise<BatchSummary> {
  return invoke<BatchSummary>('manager_auto_orient', { paths, destDir, overwrite })
}

/** 批量打印的版面选项 —— 与单图打印预览用的是同一套。 */
export interface BatchPrintOptions {
  /** 适应页面（true，默认）或原始尺寸（false） */
  fit?: boolean
  /** `portrait` / `landscape` */
  orientation?: string
  /** `draft` / `normal` / `high` */
  quality?: string
  paperWmm?: number
  paperHmm?: number
  /** 四边页边距（毫米） */
  marginMm?: number
  /** `null` = 系统默认打印机 */
  printer?: string | null
}

/**
 * 批量打印：逐张送到打印机，**固定跳过**系统打印对话框。
 *
 * ⚠️ 版面参数由调用方从预览浮层取（用户在预览里选什么就印什么）；
 * 不传则退化成 A4 / 纵向 / 普通质量 / 10mm。
 */
export async function managerPrintImages(
  paths: string[],
  opts?: BatchPrintOptions,
): Promise<BatchSummary> {
  return invoke<BatchSummary>('manager_print_images', {
    paths,
    fit: opts?.fit ?? null,
    orientation: opts?.orientation ?? null,
    quality: opts?.quality ?? null,
    paperWmm: opts?.paperWmm ?? null,
    paperHmm: opts?.paperHmm ?? null,
    marginMm: opts?.marginMm ?? null,
    printer: opts?.printer ?? null,
  })
}

/** 删除请求里的一个目标 */
export interface DeleteItem {
  /** 浏览模式（未入库）为 null —— 只删文件，没有索引行要清 */
  id: number | null
  /** 卷未挂载时为 null —— 只能删索引，删不了文件 */
  path: string | null
}

/** 批量删除结果 */
export interface DeleteSummary {
  deletedFiles: number
  removedIndex: number
  offlineIndexOnly: number
  failed: BatchFailure[]
}

/**
 * 批量删除：**移入回收站**（可撤销），再清索引与缩略图缓存。
 *
 * ⚠️ 顺序是「先删文件 → 成功才删索引」：删不掉就保留索引，
 * 否则用户会在列表里失去这张照片、磁盘上却还留着。
 */
export async function managerDeleteImages(items: DeleteItem[]): Promise<DeleteSummary> {
  return invoke<DeleteSummary>('manager_delete_images', { items })
}

/** 重命名请求里的一项 */
export interface RenameItem {
  /** 入库行的索引 id；浏览模式（未入库）为 null */
  id: number | null
  /** 当前绝对路径 —— `id == null` 时用它做纯文件重命名 */
  path: string | null
  /** 目标文件名（含扩展名） */
  newName: string
}

/**
 * 批量重命名：改文件名 + 同步索引 + 迁移缩略图缓存。
 *
 * ⚠️ **绝不覆盖**：目标已存在的那张会进 `failed`，不会顶掉别人的文件。
 */
export async function managerRenameImages(items: RenameItem[]): Promise<BatchSummary> {
  return invoke<BatchSummary>('manager_rename_images', { items })
}

/** 「另存为」的选项 */
export interface SaveAsOptions {
  /** 目标文件夹（必须先准备好，不存在会创建） */
  destDir: string
  /** `null` = 保持每张原格式；否则统一转成该格式 */
  format: 'jpg' | 'png' | 'bmp' | null
  /** 文件名模板（`{name}` / `{n}`）；`null`/空串 = 保持原名 */
  nameTemplate: string | null
  /**
   * 精确文件名（含扩展名），**只在单张时生效** —— 由系统「另存为」对话框给出。
   * 多张时后端会忽略它（否则 N 张会挤到同一个名字上），走 `nameTemplate`。
   */
  fileName?: string | null
}

/**
 * 批量「另存为」：目标目录 +（可选）格式 +（可选）文件名模板。
 *
 * 与「导出」「转换格式」的分工：
 * - 导出 = 字节复制，无选项；
 * - 转换格式 = 只换编码；
 * - **另存为 = 目录 + 格式 + 命名一次配齐**。
 *
 * 目标格式与源格式一致时后端走**字节复制**（不重编码，避免 JPEG 二次压缩掉画质）。
 */
export async function managerSaveAs(
  paths: string[],
  opts: SaveAsOptions,
): Promise<BatchSummary> {
  return invoke<BatchSummary>('manager_save_as', {
    paths,
    destDir: opts.destDir,
    format: opts.format,
    nameTemplate: opts.nameTemplate,
    fileName: opts.fileName ?? null,
  })
}

/**
 * 批量**复制**到文件夹：字节复制（不重新编码、保留原格式与 EXIF）。
 *
 * 重名自动变成 `名字 (1).jpg`，绝不覆盖已有文件；同一批次内也不会互相覆盖。
 */
export async function managerCopyToFolder(
  paths: string[],
  destDir: string,
): Promise<BatchSummary> {
  return invoke<BatchSummary>('manager_copy_to_folder', { paths, destDir })
}

/**
 * 批量**移动**到文件夹：真实搬走文件（同卷 rename，跨卷复制后删源）。
 *
 * ⚠️ 已入库的条目会顺带清掉源索引与缩略图缓存 —— 文件不在原处了，
 * 列表里就不该再有那一行。浏览模式传 `id: null` 即可。
 */
export async function managerMoveToFolder(
  items: DeleteItem[],
  destDir: string,
): Promise<BatchSummary> {
  return invoke<BatchSummary>('manager_move_to_folder', { items, destDir })
}

/** 侧栏「标签」条目：标签 + 使用次数 */
export interface TagItem {
  id: number
  name: string
  count: number
}

/** 标签选择器用：某个标签在这批图里命中了多少张 */
export interface TagHit {
  name: string
  count: number
}

/** 侧栏「地点」条目（离线城市名 + 张数）。 */
export interface PlaceItem {
  name: string
  count: number
}

/**
 * 地点 + 张数（多的在前）。
 *
 * ⭐ 和 [`managerTags`] 一样**必须带范围**：`libraryId` = 只数这个图库，
 * `dirId` = 只数这个目录，都不传 = 全库。不带范围的话标签会跟着图库切换、
 * 地点却一直显示全库数字（2026-10-28 用户报的 bug）。
 */
export async function managerPlaces(libraryId?: number, dirId?: number): Promise<PlaceItem[]> {
  return invoke<PlaceItem[]>('manager_places', { libraryId, dirId })
}

/**
 * 标签 + 张数（多的在前）。
 *
 * ⭐ 必须带范围：`libraryId` = 只数这个图库，`dirId` = 只数这个目录，
 * 都不传 = 全库。不带范围会让每个图库显示同一份数字（2026-10-23 报的 bug）。
 */
export async function managerTags(libraryId?: number, dirId?: number): Promise<TagItem[]> {
  return invoke<TagItem[]>('manager_tags', { libraryId, dirId })
}

/**
 * 一级类目 → **去重**后的图片张数。
 *
 * ⚠️ 必须由后端算：`managerTags` 给的是「每个标签命中多少张图」，
 * 而一张图常同时挂「单人」「全身」，把它们相加会重复计数
 * （测试图库人像组：相加 89 / 实际 52 张）。界面上显示 89 却只筛出 52 张，
 * 用户会以为「点一级没有显示全部」。
 */
export async function managerTagGroups(
  libraryId?: number,
  dirId?: number,
): Promise<{ l1: string; count: number }[]> {
  return invoke<{ l1: string; count: number }[]>('manager_tag_groups', { libraryId, dirId })
}

/**
 * 给一批图**加**标签（不存在的自动新建）。
 *
 * 只改索引：不碰文件、不碰缩略图缓存。同名标签会被复用（`tags.name UNIQUE`）。
 */
export async function managerAddTags(ids: number[], names: string[]): Promise<number> {
  return invoke<number>('manager_add_tags', { ids, names })
}

/** 从一批图**移除**某个标签（按标签 id）。照片不动。 */
export async function managerRemoveTag(ids: number[], tagId: number): Promise<number> {
  return invoke<number>('manager_remove_tag', { ids, tagId })
}

/**
 * 这批图各自已有哪些标签（`count` = 这批里带该标签的张数）。
 *
 * 标签选择器靠它区分「全选 / 半选 / 没有」——没有这个就只能显示"有/没有"，
 * 批量时用户不知道自己会不会把已有的又加一遍。
 */
export async function managerTagCounts(ids: number[]): Promise<TagHit[]> {
  return invoke<TagHit[]>('manager_tag_counts', { ids })
}

/** 标签改名（重名后端会报「已经有同名标签了」）。 */
export async function managerRenameTag(id: number, name: string): Promise<void> {
  return invoke<void>('manager_rename_tag', { id, name })
}

/** 删除标签：拆掉所有挂载 + 删标签行，**照片一张不动**。 */
export async function managerDeleteTag(id: number): Promise<void> {
  return invoke<void>('manager_delete_tag', { id })
}

/** 磁盘缩略图缓存占用 */
export interface CacheStats {
  bytes: number
  files: number
}

/** 读缩略图缓存占用（设置面板显示用）。 */
export async function managerCacheStats(): Promise<CacheStats> {
  return invoke<CacheStats>('manager_cache_stats')
}

/**
 * 清空磁盘缩略图缓存。
 *
 * ⛔ 只删 `thumbs/` 下的缓存文件：原图与索引都不动。
 * 代价是下次浏览要重新生成缩略图（首屏会慢一点）。
 */
export async function managerClearCache(): Promise<CacheStats> {
  return invoke<CacheStats>('manager_clear_cache')
}

/** 旋转/翻转参数（写回原文件） */
export interface TransformOptions {
  /** 顺时针 90° 的步数：1 = 顺 90，3 = 逆 90，2 = 180 */
  rotate?: number
  flipH?: boolean
  flipV?: boolean
}

/**
 * 批量旋转 / 翻转并**写回原文件**。
 *
 * ⚠️ 破坏性：只有 JPEG / PNG / BMP 能覆盖，RAW / HEIC / WebP / TIFF 会被跳过并报告。
 * ⚠️ 顺序是**先旋转、后翻转**。
 */
export async function managerTransformImages(
  paths: string[],
  opts: TransformOptions,
): Promise<BatchSummary> {
  return invoke<BatchSummary>('manager_transform_images', {
    paths,
    rotate: opts.rotate ?? null,
    flipH: opts.flipH ?? null,
    flipV: opts.flipV ?? null,
  })
}

/**
 * 新建一个**空图库**，返回新库 id。
 *
 * ⚠️ 重名由后端挡（`libraries.name` 没有 UNIQUE 约束），前端也该先自查一遍
 * 再弹一次对话框让用户改名，而不是等服务端报错。
 */
export async function managerCreateLibrary(name: string): Promise<number> {
  return invoke<number>('manager_create_library', { name })
}

/**
 * 新建一个**智能图库**（规则文本走 `pv-lib` 的 DSL），返回新库 id。
 *
 * 规则写法（空格 = AND）：`date:2024` / `date:2024-05` / `rating:>=4` /
 * `type:raw` / `ext:jpg` / `tag:风景` / 裸词（全文关键词）。
 * ⚠️ 语法错后端直接报错，不会建出一个永远空的库。
 */
export async function managerCreateSmartLibrary(name: string, rule: string): Promise<number> {
  return invoke<number>('manager_create_smart_library', { name, rule })
}

/** 改图库名（侧栏图库行右键「重命名」）。只改 `libraries.name`。 */
export async function managerRenameLibrary(id: number, name: string): Promise<void> {
  return invoke<void>('manager_rename_library', { id, name })
}

/**
 * 删除图库（侧栏图库行右键「删除图库」）。
 *
 * ⛔ 只删这个分组：图库行 + 它的成员关系。目录索引、缩略图缓存、原图都不动。
 * 返回 false 表示这个 id 已经不存在（幂等，不报错）。
 */
export async function managerDeleteLibrary(id: number): Promise<boolean> {
  return invoke<boolean>('manager_delete_library', { id })
}

/** 单张图的标签 + 相机 + 地点（卡片上的「标签」按钮用）。 */
export interface ImageTagsResult {
  tags: ImageTagInfo[]
  camera: string | null
  place: string | null
}

export function managerImageTags(imageId: number): Promise<ImageTagsResult> {
  return invoke<ImageTagsResult>('manager_image_tags', { imageId })
}

/** 拍摄年份列表（库里真实出现过的，倒序）。 */
export function managerYears(): Promise<ManagerYear[]> {
  return invoke<ManagerYear[]>('manager_years')
}

/** 相机机型列表（库里真实出现过的）。 */
export function managerCameras(): Promise<ManagerCamera[]> {
  return invoke<ManagerCamera[]>('manager_cameras')
}

/* ── AI 打标配置与词表 ─────────────────────────────────────────── */

export interface AiConfig {
  model: string
  vocabCount: number
  disabledCount: number
  /** 二级（具体词）门槛 */
  threshold: number
  /** 一级（类目）门槛，通常低于 `threshold` */
  thresholdL1: number
  maxTags: number
  faces: boolean
}

export interface VocabTerm {
  term: string
  l1: string
  l2: string
  syn: string
  en: string
  enabled: boolean
}

export function aiConfig(): Promise<AiConfig> {
  return invoke<AiConfig>('ai_config')
}

export function aiSaveConfig(p: {
  threshold?: number
  thresholdL1?: number
  maxTags?: number
  faces?: boolean
}): Promise<void> {
  return invoke<void>('ai_save_config', p)
}

export function aiVocab(): Promise<VocabTerm[]> {
  return invoke<VocabTerm[]>('ai_vocab')
}

export function aiVocabToggle(term: string, enabled: boolean): Promise<void> {
  return invoke<void>('ai_vocab_toggle', { term, enabled })
}

export function aiVocabExport(path: string): Promise<string> {
  return invoke<string>('ai_vocab_export', { path })
}

/** 导入结果：`updated` = 真的改了状态的行数，`unknown` = 词表里查不到的词条。 */
export interface VocabImportResult {
  updated: number
  unknown: string[]
}

/**
 * 从 CSV 导入词表**启用状态**（词条本身是编译期固定的，运行时改不了）。
 *
 * 和 [`aiVocabExport`] 闭环：导出 → 在 Excel 里勾「启用」列 → 导回来。
 * ⚠️ CSV 里没出现的词条保持原状，所以只交一部分也行。
 */
export function aiVocabImport(path: string): Promise<VocabImportResult> {
  return invoke<VocabImportResult>('ai_vocab_import', { path })
}

/** 一键任务的状态快照（每 500ms 轮询）。 */
export interface AiTaskStatus {
  running: boolean
  done: boolean
  ok: boolean
  /** 日志尾部（最多 200 行） */
  lines: string[]
  /** 当前阶段：`tag` / `face`（一键 AI 识别时用来高亮进度行） */
  phase: string
  tagDone: number
  tagTotal: number
  faceDone: number
  faceTotal: number
}

/**
 * 启动一键 AI 任务（`tag` = 打标，`face` = 人像识别 + 聚类）。
 *
 * ⚠️ 现阶段是**过渡实现**：后端 spawn `tools/ai/*.py`，需要本机有 Python。
 * 返回的是日志文件路径（调试用，界面上不显示）。
 */
/**
 * 启动 AI 任务。
 *
 * - `tag`      只打标
 * - `face`     只识别人像
 * - `pipeline` 一键：先打标再识别人像（后端串行，进度含两阶段）
 */
export function aiRunTask(
  task: 'tag' | 'face' | 'pipeline',
  library: string,
): Promise<string> {
  return invoke<string>('ai_run_task', { task, library })
}

/** 某个范围（空 = 全部图库）的 AI 处理进度，用来决定按钮文案与是否二次确认。 */
export interface LibraryAiStatus {
  /** 打过 AI 标签的图数 */
  tagged: number
  /** 已检测出人脸的图数 */
  faced: number
  /** 范围内总图数 */
  total: number
}

export function aiLibraryStatus(library: string): Promise<LibraryAiStatus> {
  return invoke<LibraryAiStatus>('ai_library_status', { library })
}

export function aiTaskStatus(): Promise<AiTaskStatus> {
  return invoke<AiTaskStatus>('ai_task_status')
}

export function aiTaskStop(): Promise<void> {
  return invoke<void>('ai_task_stop')
}
