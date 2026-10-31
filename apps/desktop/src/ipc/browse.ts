/**
 * 文件浏览器（侧栏「文件夹」）IPC 封装。
 *
 * 后端 `fs_browse` 只做**展示用的枚举**：盘符 + 用户标准文件夹 + 子目录。
 * 真正的索引要等用户点了某个目录、由 `managerScan` 去扫。
 */

import { invoke } from '@tauri-apps/api/core'

import { parsePixels, toBytes } from './protocol'
import type { DecodedImage } from './types'
import type { ThumbTier } from './manager'

/**
 * 「我的电脑」虚拟节点的路径占位。
 *
 * 它没有真实路径，但前端要拿它当展开状态的 key，也需要一个能传给后端的标识。
 */
export const FS_COMPUTER_KEY = '__computer__'

/** 浏览器节点。`kind` 保留 `string`（后端可能加新类型，不要用联合类型骗自己）。 */
export interface FsNode {
  name: string
  /** `null` = 虚拟节点（只有「我的电脑」） */
  path: string | null
  /** `computer` / `drive` / `known` / `dir` */
  kind: string
  /** 附加说明：盘符 `(C:)` */
  note: string | null
  /**
   * 字节数。**只有压缩包会给**（卡片右下角显示大小）；
   * 目录恒为 null —— 算目录大小要递归，浏览里不可接受。
   */
  size?: number | null
}

/** 侧栏文件树的**系统图标**（`data:image/bmp;base64,…`；取不到为 null）。 */
export interface TreeIcons {
  computer: string | null
  folder: string | null
  /**
   * **每个盘符各自的图标**：`[["C:\\", "data:image/png;base64,…"], …]`。
   *
   * ⛔ 不能共用一个：系统盘带 Windows 徽标，可移动盘/光驱又是另外的样式，
   * 共用一个会让所有盘长得一模一样。
   */
  drives: Array<[string, string]>
}

/**
 * 取侧栏树要用的系统图标 —— **就是资源管理器那一套**（`SHGetFileInfoW`）。
 *
 * ⚠️ 用系统图标而不是 emoji：盘符图标会区分固定盘 / 可移动盘 / 光驱 / 网络位置，
 * 这是系统才知道的信息，emoji 只有一个形状，观感也和 Win10 差很远。
 * 一次取三个，前端缓存，不要每次渲染都调。
 */
export async function fsTreeIcons(): Promise<TreeIcons> {
  return invoke<TreeIcons>('fs_tree_icons')
}

/** 顶层：我的电脑 + 用户标准文件夹（图片/桌面/文档/下载）。 */
export async function fsRoots(): Promise<FsNode[]> {
  return invoke<FsNode[]>('fs_roots')
}

/** 展开一个节点：`null` 或 `__computer__` 返回盘符，否则返回该目录的子目录。 */
export async function fsListDirs(path: string | null): Promise<FsNode[]> {
  return invoke<FsNode[]>('fs_list_dirs', { path })
}

/** 地址栏解析结果。 */
export interface ResolvedDir {
  path: string
  name: string
}

/**
 * 把地址栏里敲的路径解析成**要浏览的目录**；`null` = 路径不存在。
 *
 * 规则（都在后端 `fs_browse::resolve_dir`，有单测）：
 * 目录→它自己、**文件→它所在的目录**、`E:`→`E:\`、剥掉前后空格与成对引号、
 * 分隔符统一成 `\`。
 */
export async function fsResolveDir(path: string): Promise<ResolvedDir | null> {
  return invoke<ResolvedDir | null>('fs_resolve_dir', { path })
}

/** 浏览列表里的一个文件。 */
export interface FsImage {
  path: string
  name: string
  size: number
  /**
   * 修改时间（Unix **纳秒**）。
   *
   * 浏览列表不经过索引，所以「按修改时间排序」和「按拍摄时间的兜底」
   * 都只能靠它 —— 后端遍历目录时顺手带出来的，零额外成本。
   */
  mtime: number
  format: string
  isRaw: boolean
  /**
   * 拍摄时间（Unix 秒）。
   *
   * 列目录时**永远是 `null`**（读 EXIF 太贵），只有用户选了「按拍摄时间排序」
   * 才会由 [`fsProbeTimes`] 在后台分批补上。
   */
  takenAt: number | null
  /** 像素尺寸；探不出来为 null（后端只读文件头，成本极低） */
  width: number | null
  height: number | null
}

/** 目录浏览结果（`items` 可能被后端截断到 `MAX_BROWSE_FILES`，`total` 是真实数量）。 */
export interface DirListing {
  total: number
  items: FsImage[]
  /**
   * 这份列表来自**压缩包内部**（2026-10-31）。
   *
   * 前端据此跳过「后台补尺寸」—— 包内每条探尺寸都要先解压：
   * zip 3ms/条、rar 85ms/条（起进程），240 条一批就是 20 秒白跑。
   * 而卡片的信息量本来就在缩略图上，尺寸标签只是锦上添花。
   *
   * ⛔ 不要用「路径里有没有 `!`」自行判断：包名和普通文件名都可能带 `!`。
   */
  isArchive: boolean
}

/**
 * 列出目录里的图片 —— **单层、不写索引、不生成缩略图**（临时预览用）。
 *
 * ⚠️ 与 `managerOpenFolder` 是两条路：那个会**建索引行**（从此进入图库）。
 * 浏览只是「看一眼」，不产生任何持久化副作用。
 *
 * `probeLimit`：只探测前多少个文件的尺寸，**其余为 null**。
 * 首屏只探这一批就返回（默认由后端定，60 ≈ 三屏），
 * 剩下的用 [`fsProbeDimensions`] 在后台补 —— 别在这里传大数，
 * 那等于回到「全部探完才看见界面」。
 */
export async function fsListImages(path: string, probeLimit?: number): Promise<DirListing> {
  return invoke<DirListing>('fs_list_images', { path, probeLimit })
}

/**
 * 批量探测像素尺寸（后台补齐用）。
 *
 * 返回值与 `paths` **一一对应**：`[宽, 高]` 或 `null`（探不出 / RAW）。
 * RAW 永远是 `null` —— 探它要读整个文件，且内嵌预览尺寸不等于传感器尺寸。
 */
export async function fsProbeDimensions(
  paths: string[],
): Promise<Array<[number, number] | null>> {
  return invoke<Array<[number, number] | null>>('fs_probe_dimensions', { paths })
}

/**
 * 批量读取**拍摄时间**（Unix 秒），与 `paths` 一一对应，读不出是 `null`。
 *
 * ⚠️ 每个文件要 `open` 一次读 EXIF —— **只在用户选了「按拍摄时间排序」时才调**，
 * 并且要分批（前端 `DIM_CHUNK` 一批），否则会把首屏重新拖慢。
 */
export async function fsProbeTimes(paths: string[]): Promise<Array<number | null>> {
  return invoke<Array<number | null>>('fs_probe_times', { paths })
}

/**
 * 临时缩略图 → RGBA（PVI1 裸字节解析，与入库那条路同一条零 base64 通道）。
 *
 * 后端**只解码缩放**，不写 L1/L2、不编码 JPEG —— 所以反复浏览同一目录
 * 每次都会重新解码，这是「不保存」的代价，符合浏览模式的语义。
 */
export async function fsThumbnail(path: string, tier: ThumbTier): Promise<DecodedImage> {
  const raw = await invoke<unknown>('fs_thumbnail', { path, tier })
  return parsePixels(toBytes(raw))
}
