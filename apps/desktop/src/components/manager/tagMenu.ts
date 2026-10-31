/**
 * 卡片「标签」按钮弹出的菜单 **内容构建**（纯函数）。
 *
 * 为什么单独成文件：这段约 90 行的「一级/二级归组 + 按分数排序 + 缩进 + EXIF 补两行」
 * 原先整个塞在 `ManagerView` 的 `showTags` 闭包里，夹在 `await` 与 `setState` 之间，
 * 既不能单测也没法一眼核对版式。2026-11 拆出，见 `tagMenu.test.ts`。
 *
 * ## 版式（用户 2026-10-22 定稿，别随手改）
 * ```
 * 标签 · 9 个
 * 自然            0.45      ← 一级：带自己的置信度
 * 　峡谷          0.42      ← 二级：缩进一格，另一个一级之前画分隔线
 * 　河流          0.41
 * ──
 * 相机            ASUS_I001DA
 * 地点            上海
 * ```
 *
 * ## 三条隐含规则
 * 1. ⛔ `source === 'exif'` 的标签不参与列表 —— 它们由末尾「相机 / 地点」两行代表
 * 2. ⛔ 一级行**只按自身分数**排序（没有分数的排最后），不是按组内二级的分数
 * 3. ⛔ 有一级标签却没有二级的组，也要单独占一行（否则那张图的标签会凭空少一半）
 */

import type { ContextMenuItem } from '../chrome/ContextMenu'
import type { ImageTagInfo, ImageTagsResult, VocabTerm } from '../../ipc/manager'

/** 词表索引：一次算好，供标签条与弹层共用。 */
export type VocabIndex = {
  /** 词条 → 所属一级/二级 */
  map: Map<string, { l1: string; l2: string }>
  /** 全部一级名（用来区分「这条标签是一级还是二级」） */
  l1Names: Set<string>
  /** 全部一级，**按词表出现顺序** —— 标签条要铺满，没有图的显示 0 */
  allL1: string[]
}

/** 从词表建索引。纯函数，词表变了重算一次即可。 */
export function buildVocabIndex(terms: VocabTerm[]): VocabIndex {
  const map = new Map<string, { l1: string; l2: string }>()
  const l1Names = new Set<string>()
  const allL1: string[] = []
  for (const t of terms) {
    map.set(t.term, { l1: t.l1, l2: t.l2 })
    l1Names.add(t.l1)
    if (!allL1.includes(t.l1)) allL1.push(t.l1)
  }
  return { map, l1Names, allL1 }
}

/** 没有一级归属的标签落到哪个组（OCR 单独一组，其余都进「其他」）。 */
function fallbackGroup(src: string): string {
  return src === 'ocr' ? '文字' : '其他'
}

/** 二级行的右列：有分数给分数，没有就写明来源。 */
function leafHint(t: { conf: number | null; src: string }): string {
  if (t.conf != null) return t.conf.toFixed(2)
  if (t.src === 'ocr') return '文字'
  if (t.src === 'face') return '人脸'
  return '手动'
}

/**
 * 把一张图的标签组织成菜单项。
 *
 * `close` 在叶子项被点时调用（菜单收起）；一级行与表头是 `disabled`，不响应点击。
 * 结果为空时返回一行提示，交给调用方照常弹层 —— 不做「不弹」判断。
 */
export function buildTagMenu(
  res: ImageTagsResult | ImageTagInfo[],
  vocab: VocabIndex,
  close: () => void,
): ContextMenuItem[] {
  // 老版本只有数组形态，兼容一下（后端已统一返回对象）
  const all = Array.isArray(res) ? res : res.tags
  const camera = Array.isArray(res) ? null : res.camera
  const place = Array.isArray(res) ? null : res.place

  const shown = all.filter((t) => t.source !== 'exif')
  if (shown.length === 0 && !camera && !place) {
    return [{ id: 'none', label: '这张图没有匹配到任何标签', disabled: true }]
  }

  // 拆成「一级标签」和「二级标签」：本身是一级名、又不是任何词条 → 当一级行
  const l1Tags = new Map<string, number | null>()
  const leaves: { name: string; l1: string; conf: number | null; src: string }[] = []
  for (const t of shown) {
    const v = vocab.map.get(t.name)
    if (vocab.l1Names.has(t.name) && !v) {
      l1Tags.set(t.name, t.confidence)
    } else {
      leaves.push({
        name: t.name,
        l1: v?.l1 ?? fallbackGroup(t.source),
        conf: t.confidence,
        src: t.source,
      })
    }
  }

  // 按一级归组，组按一级自身分数倒序；没有分数的一级排最后
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

  const total = shown.length + (camera ? 1 : 0) + (place ? 1 : 0)
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
        // 全角空格缩进一格：二级比一级低一级
        label: `　${vocab.map.get(t.name)?.l2 ?? t.name}`,
        hint: leafHint(t),
        onSelect: close,
      })
    }
  }

  // EXIF：相机 / 地点，右列是值（不是分数）
  //
  // ⚠️ 分隔线看的是「**实际渲染出来的**第一行 EXIF」，不是相机槽位的下标 ——
  // 没有相机机型时（截图、手机直出无机型）地点才是第一行，此时它前面必须画线，
  // 否则地点看起来像上一条标签的二级项。
  let firstExif = true
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
      separatorBefore: firstExif,
      onSelect: close,
    })
    firstExif = false
  }

  return items
}
