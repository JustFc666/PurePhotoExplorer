/**
 * 图库的**列表视图**（从 `ManagerView.tsx` 拆出，2026-11）。
 *
 * 表头两件事要注意（都是踩过的）：
 * - 拖动条 9px 宽、`translateX(50%)` **骑在列边界上** —— 太窄按不准，太宽会盖住相邻列；
 * - 三角只用来看「当前按哪列排」，非当前列淡显 ▲，提示「这列也可以点」。
 *
 * ⛔ 别在这里读 `mgr`：行数据的双击/单击语义和网格视图**必须一致**，
 * 所以全部由调用方传进来，两个视图才不会各自漂移。
 */

import type { MouseEvent as ReactMouseEvent } from 'react'

import type { ManagerSort } from '../../ipc/manager'
import type { GridItem } from '../../hooks/useManager'
import type { ListColumnId, ListPrefsApi } from '../../hooks/listColumns'
import { renderListCell } from './ListCells'

export type ManagerListProps = {
  items: GridItem[]
  /** 选中的行 key（不是索引 id —— 浏览模式下 id 恒为 null） */
  selected: ReadonlySet<string>
  canSelect: boolean
  sort: ManagerSort
  sortDesc: boolean
  onToggleSort: (key: ManagerSort) => void
  onToggleRow: (item: GridItem, e: ReactMouseEvent) => void
  onOpenRow: (item: GridItem) => void
  onRowContextMenu: (item: GridItem, e: ReactMouseEvent) => void
  /** 空白处右键 = 刷新/全选 */
  onBlankContextMenu: (e: ReactMouseEvent) => void
  listPrefs: ListPrefsApi
  /** 正在拖列宽（挂 `is-resizing` 让全局光标变 col-resize） */
  colResizing: boolean
  onGripDown: (e: ReactMouseEvent, id: ListColumnId) => void
}

export function ManagerList(props: ManagerListProps) {
  const {
    items,
    selected,
    canSelect,
    sort,
    sortDesc,
    onToggleSort,
    onToggleRow,
    onOpenRow,
    onRowContextMenu,
    onBlankContextMenu,
    listPrefs,
    colResizing,
    onGripDown,
  } = props

  return (
    <div
      className={`pv-mg-list${colResizing ? ' is-resizing' : ''}`}
      style={{ ['--pv-list-cols' as string]: listPrefs.gridTemplate }}
      onContextMenu={onBlankContextMenu}
    >
      {/* 表头（2026-10-31）：列名 + 右侧拖动条 */}
      <div className="pv-mg-listhead">
        <span />
        {listPrefs.visible.map((c) => {
          const sorted = sort === c.sortKey
          return (
            <span key={c.id} className={`pv-mg-listhcell is-${c.align}${sorted ? ' is-sorted' : ''}`}>
              <button
                type="button"
                className="pv-mg-listhbtn"
                onClick={() => onToggleSort(c.sortKey)}
                title={`${c.label}${c.hint ? ` — ${c.hint}` : ''}\n点击按这一列排序${
                  sorted ? '（再点一次反向）' : ''
                }`}
                aria-sort={sorted ? (sortDesc ? 'descending' : 'ascending') : 'none'}
              >
                <span className="pv-mg-listhtxt">{c.label}</span>
                {/* 三角：当前排序列显示实心方向，其它列淡显（提示「可以点」） */}
                <span className="pv-mg-listsort" aria-hidden>
                  {sorted ? (sortDesc ? '▼' : '▲') : '▲'}
                </span>
              </button>
              <span
                className="pv-mg-listgrip"
                role="separator"
                aria-orientation="vertical"
                onMouseDown={(e) => onGripDown(e, c.id)}
                onDoubleClick={() => listPrefs.resetWidth(c.id)}
                title="拖动调整列宽，双击恢复默认"
              />
            </span>
          )
        })}
      </div>

      {items.map((item) => {
        const on = selected.has(item.key)
        return (
          <div
            key={item.key}
            className={`pv-mg-listrow${on ? ' is-selected' : ''}`}
            role="button"
            tabIndex={0}
            onClick={(e) => onToggleRow(item, e)}
            onDoubleClick={() => onOpenRow(item)}
            onContextMenu={(e) => onRowContextMenu(item, e)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') onOpenRow(item)
            }}
            title={item.path ?? item.name}
          >
            {/*
              不可选（浏览模式下文件）就留个空格子占位 —— 空 `span` 也占 26px，
              否则这一行整块的列会往左挪一列，和别的行对不齐。
            */}
            {canSelect || item.kind !== 'file' ? (
              <span className={`pv-mg-listcheck${on ? ' is-on' : ''}`} aria-hidden>
                {on ? '✓' : ''}
              </span>
            ) : (
              <span />
            )}
            {listPrefs.visible.map((c) => (
              <span
                key={c.id}
                className={`pv-mg-listmeta is-${c.align}${c.id === 'name' ? ' is-name' : ''}`}
              >
                {renderListCell(item, c.id)}
              </span>
            ))}
          </div>
        )
      })}
    </div>
  )
}
