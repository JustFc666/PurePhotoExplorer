/**
 * 卡片「标签」按钮的弹层 + AI 词表。
 *
 * ## 为什么自己管一份位置/项，不复用 `useContextMenu`
 * 标签数据是**异步**取的。先弹一行「读取标签…」再换成真列表，比「点了没反应
 * 200ms」好得多 —— `useContextMenu` 只能一次性给出 `items`。
 *
 * ## 词表为什么等 `ready`
 * ⚠️ 必须在 `manager_open` 完成后再拉。先挂载就调会报「管理器尚未打开」，
 * 词表拿不到 → 所有标签都归到「EXIF / 其他」，标签条与弹层的分组全乱。
 *
 * 内容构建（一级/二级归组、排序、缩进）是纯函数，见 `components/manager/tagMenu.ts`。
 */

import { useCallback, useEffect, useMemo, useState } from 'react'
import type { MouseEvent as ReactMouseEvent } from 'react'

import type { ContextMenuItem } from '../../components/chrome/ContextMenu'
import {
  buildTagMenu,
  buildVocabIndex,
  type VocabIndex,
} from '../../components/manager/tagMenu'
import { aiVocab, managerImageTags, type VocabTerm } from '../../ipc/manager'

/** 已弹出的标签菜单：位置 + 菜单项。`null` = 关着。 */
export type TagPop = { x: number; y: number; items: ContextMenuItem[] } | null

export type TagPopoverApi = {
  tagPop: TagPop
  /** 浮层关闭时置 `null`（`ManagerView` 的菜单渲染直接透传） */
  setTagPop: (v: TagPop) => void
  /** 词表索引：标签条与弹层共用 */
  vocab: VocabIndex
  /** 原始词表（标签条要按一级逐个列出词条） */
  terms: VocabTerm[]
  /** 卡片「标签」按钮：读这张图的标签并在鼠标处弹层 */
  showTags: (id: number, e: ReactMouseEvent | MouseEvent) => Promise<void>
}

export function useTagPopover(ready: boolean): TagPopoverApi {
  const [tagPop, setTagPop] = useState<TagPop>(null)
  const [terms, setTerms] = useState<VocabTerm[]>([])

  useEffect(() => {
    if (!ready) return
    let alive = true
    aiVocab()
      .then((v) => {
        if (alive) setTerms(v)
      })
      .catch((e) => {
        if (alive) console.warn('词表读取失败', e)
      })
    return () => {
      alive = false
    }
  }, [ready])

  const vocab = useMemo(() => buildVocabIndex(terms), [terms])

  const showTags = useCallback(
    async (id: number, e: ReactMouseEvent | MouseEvent) => {
      e.preventDefault()
      e.stopPropagation()
      const x = e.clientX
      const y = e.clientY
      setTagPop({ x, y, items: [{ id: 'loading', label: '读取标签…', disabled: true }] })
      try {
        const res = await managerImageTags(id)
        setTagPop({ x, y, items: buildTagMenu(res, vocab, () => setTagPop(null)) })
      } catch {
        setTagPop({ x, y, items: [{ id: 'err', label: '读取标签失败', disabled: true }] })
      }
    },
    [vocab],
  )

  return { tagPop, setTagPop, vocab, terms, showTags }
}
