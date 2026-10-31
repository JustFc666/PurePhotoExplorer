/**
 * 列表视图的单元格渲染（2026-10-31）。
 *
 * 抽出来是因为它要跟列定义（`hooks/listColumns.ts`）严格对齐：
 * 列定义说「有这一列」，这里就必须给出这一列显示什么。
 * 写在 `ManagerView` 里的话，两者隔了几百行，加列时很容易只改一半。
 */

import { formatFileSize } from '../../ipc'
import type { GridItem } from '../../hooks/useManager'
import {
  formatDateTime,
  formatMtime,
  formatRating,
  type ListColumnId,
} from '../../hooks/listColumns'
import { ArchiveIcon } from '../icons/ArchiveIcon'
import { FolderIcon } from '../icons/FolderIcon'

/** 压缩包本身没有 `format`（它是容器不是图片），用扩展名顶上。 */
function archiveFormat(name: string): string {
  const i = name.lastIndexOf('.')
  return i > 0 ? name.slice(i + 1).toUpperCase() : '压缩包'
}

export function renderListCell(item: GridItem, col: ListColumnId): React.ReactNode {
  switch (col) {
    case 'name':
      return (
        <span className="pv-mg-listname">
          {item.kind === 'dir' && <FolderIcon className="pv-mg-listico" />}
          {item.kind === 'archive' && <ArchiveIcon className="pv-mg-listico" />}
          <span className="pv-mg-listtext">{item.name}</span>
        </span>
      )
    case 'format':
      if (item.kind === 'dir') return '—'
      if (item.kind === 'archive') return archiveFormat(item.name)
      return item.format?.toUpperCase() ?? '—'
    case 'size':
      return item.size ? formatFileSize(item.size) : '—'
    case 'mtime':
      return formatMtime(item.mtime)
    case 'takenAt':
      return formatDateTime(item.takenAt)
    case 'dims':
      return item.width && item.height ? `${item.width} × ${item.height}` : '—'
    case 'flag':
      // 只显示「有旗标」，空的不画空框 —— 一列全是框线太吵
      return item.flag > 0 ? <span className="pv-mg-listflag">⚑</span> : '—'
    case 'rating':
      return formatRating(item.rating)
    default:
      return '—'
  }
}
