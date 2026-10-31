/**
 * 卡片「标签」弹层的**版式规则**测试（`tagMenu.ts`）。
 *
 * 这段逻辑原先整段埋在 `ManagerView` 的 `showTags` 闭包里 —— 一级/二级归组、
 * 按分数排序、全角空格缩进、EXIF 补两行，全靠人眼守。提成纯函数后可以直接
 * 喂输入看输出。测的是**规则**，不是文案措辞。
 */

import { describe, expect, it } from 'vitest'

import { buildTagMenu, buildVocabIndex, type VocabIndex } from './tagMenu'
import type { ContextMenuItem } from '../chrome/ContextMenu'
import type { ImageTagInfo, VocabTerm } from '../../ipc/manager'

/** 造词表：`[一级, [二级…]]` 展开成 `VocabTerm[]`。 */
function vocab(rows: [string, string[]][]): VocabTerm[] {
  const out: VocabTerm[] = []
  for (const [l1, l2s] of rows) {
    for (const l2 of l2s) {
      out.push({ term: l2, l1, l2, syn: '', en: '', enabled: true })
    }
  }
  return out
}

/** 造一条标签。 */
function tag(name: string, confidence: number | null, source = 'ai'): ImageTagInfo {
  return { name, source, confidence }
}

const CLOSED = { closed: 0 }

function menu(
  tags: ImageTagInfo[],
  v: VocabIndex,
  camera: string | null = null,
  place: string | null = null,
): ContextMenuItem[] {
  return buildTagMenu({ tags, camera, place }, v, () => {
    CLOSED.closed += 1
  })
}

/** 只取「非表头」的行，便于按顺序断言。 */
function body(items: ContextMenuItem[]): ContextMenuItem[] {
  return items.filter((i) => i.id !== 'hdr')
}

const V = buildVocabIndex(
  vocab([
    ['自然', ['峡谷', '河流', '雪山']],
    ['人像', ['单人', '合照']],
  ]),
)

describe('buildVocabIndex', () => {
  it('一级按词表出现顺序去重，不按字母排', () => {
    expect(V.allL1).toEqual(['自然', '人像'])
  })

  it('一级名单独收一份（用来区分「这条标签是一级还是二级」）', () => {
    expect(V.l1Names.has('自然')).toBe(true)
    expect(V.l1Names.has('峡谷')).toBe(false)
  })

  it('词条能查到所属一级与二级', () => {
    expect(V.map.get('峡谷')).toEqual({ l1: '自然', l2: '峡谷' })
    expect(V.map.get('不存在的词')).toBeUndefined()
  })

  it('空词表不出错', () => {
    const empty = buildVocabIndex([])
    expect(empty.allL1).toEqual([])
    expect(empty.l1Names.size).toBe(0)
  })
})

describe('buildTagMenu 版式', () => {
  it('表头写总数：EXIF 不算标签，但相机/地点各占一项', () => {
    const items = menu([tag('峡谷', 0.4)], V, 'iQOO 10', '上海')
    expect(items[0]!.label).toBe('标签 · 3 个')
  })

  it('一级行带自己的分数，二级缩进一格（全角空格）', () => {
    // 一级「自然」自己也被写进库里（打标时一级同样落库）
    const items = body(menu([tag('自然', 0.45), tag('峡谷', 0.42)], V))
    expect(items[0]).toMatchObject({ id: 'l1-自然', label: '自然', hint: '0.45', disabled: true })
    expect(items[1]!).toMatchObject({ id: 't-自然-0', label: '　峡谷', hint: '0.42' })
    expect(items[1]!.label.startsWith('　')).toBe(true)
  })

  it('一级按自身分数倒序；没分数的排最后', () => {
    const items = body(
      menu([tag('自然', 0.2), tag('峡谷', 0.2), tag('人像', 0.9), tag('单人', 0.9)], V),
    )
    expect(items.filter((i) => i.id.startsWith('l1-')).map((i) => i.label)).toEqual([
      '人像',
      '自然',
    ])
  })

  it('有一级标签却没有任何二级的组也要占一行', () => {
    const items = body(menu([tag('人像', 0.8)], V))
    expect(items).toHaveLength(1)
    expect(items[0]).toMatchObject({ id: 'l1-人像', hint: '0.80' })
  })

  it('第一个一级之前不画分隔线，之后每个一级都画', () => {
    const items = body(
      menu([tag('自然', 0.9), tag('峡谷', 0.9), tag('人像', 0.1), tag('单人', 0.1)], V),
    )
    const l1s = items.filter((i) => i.id.startsWith('l1-'))
    expect(l1s[0]!.separatorBefore).toBe(false)
    expect(l1s[1]!.separatorBefore).toBe(true)
  })

  it('词表里没有的标签落进「其他」，OCR 落进「文字」', () => {
    const items = body(menu([tag('路边随手拍', 0.3), tag('发票号', null, 'ocr')], V))
    const groups = items.filter((i) => i.id.startsWith('l1-')).map((i) => i.label)
    expect(groups).toEqual(['其他', '文字'])
  })

  it('没有分数的标签右列写明来源，不显示 0.00', () => {
    const items = body(
      menu([tag('峡谷', null, 'user'), tag('人脸A', null, 'face')], buildVocabIndex([])),
    )
    const hints = items.filter((i) => i.id.startsWith('t-')).map((i) => i.hint)
    expect(hints).toEqual(['手动', '人脸'])
  })

  it('EXIF 标签不参与列表 —— 它们由结尾「相机/地点」两行代表', () => {
    const items = body(menu([tag('iQOO 10', null, 'exif'), tag('峡谷', 0.4)], V))
    expect(items.some((i) => i.label.includes('iQOO'))).toBe(false)
    expect(items.some((i) => i.id.startsWith('l1-'))).toBe(true)
  })

  it('相机/地点排在最后，右列是值不是分数，且前面画分隔线', () => {
    const items = body(menu([tag('峡谷', 0.4)], V, 'iQOO 10', '上海'))
    expect(items.slice(-2).map((i) => [i.label, i.hint])).toEqual([
      ['相机', 'iQOO 10'],
      ['地点', '上海'],
    ])
    expect(items[items.length - 2]!.separatorBefore).toBe(true)
  })

  it('只有地点没有相机时，地点自己就是那一组的第一项', () => {
    const items = body(menu([tag('峡谷', 0.4)], V, null, '上海'))
    expect(items[items.length - 1]!).toMatchObject({ label: '地点', separatorBefore: true })
  })

  it('一个标签都没有时给一行提示，不弹空菜单', () => {
    const items = menu([], V)
    expect(items).toHaveLength(1)
    expect(items[0]!.disabled).toBe(true)
  })

  it('只剩 EXIF 标签（相机/地点都为空）也算空', () => {
    const items = menu([tag('iQOO 10', null, 'exif')], V)
    expect(items).toHaveLength(1)
    expect(items[0]!.disabled).toBe(true)
  })

  it('点叶子项会收起菜单，点一级行什么都不发生', () => {
    CLOSED.closed = 0
    const items = body(menu([tag('自然', 0.45), tag('峡谷', 0.42)], V))
    const leaf = items.find((i) => i.id.startsWith('t-'))!
    const l1 = items.find((i) => i.id.startsWith('l1-'))!
    expect(l1.onSelect).toBeUndefined()
    leaf.onSelect?.()
    expect(CLOSED.closed).toBe(1)
  })

  it('兼容后端的老形态（只给一个标签数组）', () => {
    const items = buildTagMenu([tag('峡谷', 0.42)], V, () => {})
    expect(items[0]!.label).toBe('标签 · 1 个')
    expect(items.some((i) => i.id.startsWith('exif-'))).toBe(false)
  })
})
