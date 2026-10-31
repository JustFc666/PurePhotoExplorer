/**
 * 把两种数据来源归一成网格的**同一行模型**（[`GridItem`]）。
 *
 * 两种来源：
 * - **入库模式**：后端查询结果 `ImageRow[]`（有 `id`、有评分/旗标/标签）；
 * - **浏览模式**（未入库）：直读磁盘的 `DirListing` + 子文件夹 `FsNode[]`
 *   （每行 `id` 恒 `null`、`transient: true`，缩略图只解码不落缓存）。
 *
 * 为什么单独成文件：这层映射原先在 `useManager.ts` 的 `useMemo` 里，混合了
 * "排序在内存里做"、"文件夹永远排前面"、"压缩包也走文件夹分支" 三条规则，
 * 而组件只认这层产出的 `GridItem` —— 想改卡片显示什么，得先读完整 hook。
 * 搬出来之后它是纯函数，可以直接喂假数据断言顺序与字段（见 `gridItems.test.ts`）。
 *
 * ## 两条不能破的规则
 * 1. ⛔ **浏览模式的排序在前端做**：列表是直读磁盘的一次性结果，切排序不可能
 *    重新扫一遍目录（冷盘 1–10 秒）；一页最多 2 万条，内存排序是毫秒级的。
 * 2. ⛔ **文件夹/压缩包永远排在最前**，且内部只按名字排 —— 它们没有大小、
 *    没有分辨率，参与「按大小排」会变成一堆 0 混在一起。
 */

import type { ImageRow, ManagerSort } from '../../ipc/manager'
import type { DirListing, FsNode } from '../../ipc/browse'
import { compareName, makeComparator } from '../sortItems'
import type { GridItem, Scope } from './types'

/** 行模型的来源（正好是 `useManager` 里 `items` 那段 `useMemo` 读的全部东西）。 */
export type GridItemSource = {
  scopeKind: Scope['kind']
  /** 浏览模式的目录列表（临时，不落库） */
  listing: DirListing | null
  /** 浏览模式的**子文件夹**（临时；双击进入用） */
  subdirs: FsNode[]
  /** 入库模式的查询结果 */
  rows: ImageRow[]
  sort: ManagerSort
  /** 实际方向（含用户在同一列上的翻转） */
  desc: boolean
}

/**
 * 两种来源归一成同一行模型。
 *
 * `scopeKind === 'none'` 时返回空数组：启动默认态不该显示任何图片
 * （`rows` 里可能还留着上一次查询的结果）。
 */
export function buildGridItems(src: GridItemSource): GridItem[] {
  const { scopeKind, listing, subdirs, rows, sort, desc } = src

  if (scopeKind === 'none') return []

  if (scopeKind === 'browse') {
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

    files.sort(makeComparator<GridItem>(sort, desc))
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
}
