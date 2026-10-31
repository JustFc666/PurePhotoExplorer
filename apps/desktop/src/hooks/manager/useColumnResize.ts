/**
 * 列表视图的列宽拖拽（表头那几根竖线）。
 *
 * ## 为什么要一个 hook
 * 这段是「全局 `mousemove`/`mouseup` + 一个拖拽态 + 松手落盘」的老套路，和侧栏
 * 宽度（`useSideResize`）同一个形状。埋在 2000 行的组件里时，任何一处写错都只
 * 在「拖着拖着松手」时才暴露，靠人眼读不出来。
 *
 * ## 两个必须守住的点
 * 1. 监听挂 `window` 而不是表头元素 —— 鼠标移出表头（列很窄时几乎立刻发生）就
 *    收不到 `mousemove`，宽度会卡住。
 * 2. 落盘只在松手时做 —— 拖动过程中每移 1px 就写一次 localStorage 会明显掉帧。
 *
 * 列的显隐与宽度定义见 `hooks/listColumns.ts`。
 */

import { useCallback, useEffect, useRef, useState } from 'react'
import type { MouseEvent as ReactMouseEvent } from 'react'

import { useListPrefs, type ListColumnId } from '../listColumns'

export type ColumnResizeApi = {
  /** 列定义 + 宽度读写（`gridTemplate` 也在这里，表格直接用） */
  listPrefs: ReturnType<typeof useListPrefs>
  /** 正在拖列宽（拖拽期给表头加类，避免 hover 效果打架） */
  resizing: boolean
  /** 表头竖线 `onMouseDown` */
  onGripDown: (e: ReactMouseEvent, id: ListColumnId) => void
}

export function useColumnResize(): ColumnResizeApi {
  const listPrefs = useListPrefs()
  /** 拖拽起点；`null` = 没在拖 */
  const colDrag = useRef<{ id: ListColumnId; startX: number; startW: number } | null>(null)
  const [resizing, setResizing] = useState(false)

  /*
   * ⚠️ 依赖数组必须是空 —— 用 ref 读最新的 api。不然 `listPrefs` 每换一次引用
   * 就重绑一次监听（拖一下掉一帧），而且 effect 重跑时会在拖拽中途漏掉 `mouseup`。
   */
  const listPrefsRef = useRef(listPrefs)
  listPrefsRef.current = listPrefs

  useEffect(() => {
    const onMove = (e: MouseEvent) => {
      const d = colDrag.current
      if (!d) return
      listPrefsRef.current.setWidthLocal(d.id, d.startW + (e.clientX - d.startX))
    }
    const onUp = () => {
      if (!colDrag.current) return
      colDrag.current = null
      setResizing(false)
      listPrefsRef.current.commitWidths()
    }
    window.addEventListener('mousemove', onMove)
    window.addEventListener('mouseup', onUp)
    return () => {
      window.removeEventListener('mousemove', onMove)
      window.removeEventListener('mouseup', onUp)
    }
  }, [])

  const onGripDown = useCallback((e: ReactMouseEvent, id: ListColumnId) => {
    e.preventDefault()
    e.stopPropagation()
    colDrag.current = { id, startX: e.clientX, startW: listPrefsRef.current.widthOf(id) }
    setResizing(true)
  }, [])

  return { listPrefs, resizing, onGripDown }
}
