/**
 * 网格排序 —— **浏览模式在内存里排**（图库模式由后端 SQL 排）。
 *
 * ## 为什么浏览模式要自己排
 * 浏览列表是「直读磁盘的一次性结果」，后端 `walk_dir*` 固定按文件名排。
 * 用户切排序时不可能重新扫一遍目录（1–10 秒），所以在内存里排 ——
 * 一页最多 `MAX_BROWSE_FILES` 条，排序是毫秒级的。
 *
 * ## 排序方向
 * - **文件名 / 文件类型 → 升序**（A→Z、jpeg→png，符合「找某个东西」的直觉）
 * - **时间 / 大小 → 降序**（新的、大的在前）
 *
 * 与图库模式保持一致（`useManager` 里 `desc: sort !== 'name'` 是同一条规则）。
 *
 * ## ⭐ 文件名要「数字感知」
 * 相机的文件名是 `_DSC9999` → `_DSC10000`。按字节序排的话 `_DSC10000` 会排在
 * `_DSC9999` **前面**（因为 `'1' < '9'`），看起来像「顺序乱了」。
 * `localeCompare(…, { numeric: true })` 把连续数字当整数比，才是人预期的顺序。
 */

import type { ManagerSort } from '../ipc/manager'

/** 参与排序的最小字段集（结构类型，`GridItem` 直接满足）。 */
export interface Sortable {
  name: string
  size: number | null
  mtime: number | null
  format: string | null
  /** 拍摄时间（Unix 毫秒/秒都行，只要同单位）。缺 = null，比较时会回退到 mtime */
  takenAt: number | null
  /** 分辨率排序要用（缺省按 0 算，排在最前） */
  width?: number | null
  height?: number | null
  /** 星级 / 旗标排序要用 */
  rating?: number
  flag?: number
}

/**
 * 这个排序的**默认**方向是不是降序。
 *
 * - 名字 / 格式：升序（A→Z 才符合「找某个东西」的直觉）
 * - 时间 / 大小 / 分辨率：降序（新的、大的在前）
 * - 星级 / 旗标：降序（高分的在前）
 *
 * ⚠️ 这只是「切到这一列时的初始方向」。用户在同一列上再点一次会翻转，
 * 那个覆盖值存在 `useManager` 的 `sortDesc` 里，不改变这里的默认。
 */
export function descForSort(sort: ManagerSort): boolean {
  return sort !== 'name' && sort !== 'format'
}

/** 文件名比较：数字感知 + 中文/大小写不敏感。 */
export function compareName(a: string, b: string): number {
  return a.localeCompare(b, 'zh-Hans-CN', { numeric: true, sensitivity: 'base' })
}

/**
 * 造一个比较函数。
 *
 * 所有分支在值相等时**回落到文件名**，保证排序稳定 —— 否则大小相同的文件
 * 每次重排都可能换位置，滚动时看起来像在抖。
 */
export function makeComparator<T extends Sortable>(
  sort: ManagerSort,
  desc: boolean,
): (a: T, b: T) => number {
  const dir = desc ? -1 : 1
  const cmp = (a: T, b: T): number => {
    let r = 0
    switch (sort) {
      case 'name':
        r = compareName(a.name, b.name)
        break
      case 'format':
        // 同一类型的排一起；同类内按文件名（下面回落）
        r = compareName(a.format ?? '', b.format ?? '')
        break
      case 'size':
        r = (a.size ?? 0) - (b.size ?? 0)
        break
      case 'dims':
        // 像素**总数**：竖图 4000×3000 和横图 3000×4000 是同一档，
        // 单比宽或单比高会把它们分成两类，很反直觉。
        r =
          (a.width ?? 0) * (a.height ?? 0) - (b.width ?? 0) * (b.height ?? 0)
        break
      case 'rating':
        r = (a.rating ?? 0) - (b.rating ?? 0)
        break
      case 'flag':
        r = (a.flag ?? 0) - (b.flag ?? 0)
        break
      case 'mtime':
        r = (a.mtime ?? 0) - (b.mtime ?? 0)
        break
      case 'taken':
        // 没有拍摄时间就退到修改时间（相机拷出来的文件 mtime 通常就是拍摄时间附近），
        // 总比当成 0（全挤到最前面）好
        r = (a.takenAt ?? a.mtime ?? 0) - (b.takenAt ?? b.mtime ?? 0)
        break
      default:
        // added / rating / random 在浏览模式没有对应数据 → 按文件名，至少不乱
        r = 0
        break
    }
    if (r !== 0) return r * dir
    // 主键相同 → 文件名兜底（始终升序，只为了确定性）
    return compareName(a.name, b.name)
  }
  return cmp
}
