/**
 * 同目录列表的**缓存决策**（纯函数，便于单测）。
 *
 * ## 为什么单独抽出来
 * 「同目录里多了一张新图也要能翻到」这条曾经出过回归：另存为在当前文件夹
 * 写出一个新文件后，翻页序列里没有它，必须关掉重开才认出来。
 * 根因是列表缓存**只在目录路径变化时**才失效。
 *
 * 决策本身很小（就是几个布尔），但它一旦写错就是「文件凭空消失」这种
 * 极难自查的问题，所以抽成纯函数钉住判据 —— 与 `useImageView` 把缩放/
 * 框选数学抽出来单测是同一个思路。
 */

/** 目录列表缓存的记忆项 */
export interface SiblingsCache {
  /** 上次列过的目录（`null` = 从没列过） */
  dir: string | null
  /** 上次列到的文件列表 */
  list: string[]
}

/** 空缓存（首次进入） */
export function emptySiblingsCache(): SiblingsCache {
  return { dir: null, list: [] }
}

/**
 * 是否需要真正调用 `list_siblings`（读磁盘）。
 *
 * 两种情况需要：
 * 1. **目录变了** —— 换文件夹当然要重列；
 * 2. **强制刷新**（`force = true`）—— 目录没变，但内容可能变了：
 *    另存为新增了文件、或用户从别处拷了图进来。
 *
 * ⚠️ 目录没变就跳过，是**为了省一次 IPC**，代价是"同目录内容变化"看不见。
 * 任何会往目录里**写新文件**的入口都必须传 `force = true`，
 * 否则用户会遇到「新文件要重开才认出来」。
 *
 * @param cached 上次的缓存
 * @param dir    本次要打开的文件所在目录
 * @param force  是否强制重列（保存后刷新、手动刷新都传 true）
 */
export function shouldRelist(cached: SiblingsCache, dir: string, force: boolean): boolean {
  return force || cached.dir !== dir
}

/**
 * 应用一份新列表，算出当前图在其中的位置。
 *
 * 找不到时返回 `-1` —— 调用方据此显示「未知」而不是错误的序号
 * （列表是异步的，当前图可能已被移出目录）。
 */
export function locateInList(list: string[], currentPath: string): number {
  return list.indexOf(currentPath)
}
