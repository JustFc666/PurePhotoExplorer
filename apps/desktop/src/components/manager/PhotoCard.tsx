/**
 * 照片卡片（网格视图的一个单元）。
 *
 * 三行结构对应设计稿：缩略图（带勾选角标）→ 文件名 → `JPG · 353 KB`。
 *
 * ⚠️ 用 `<div role="button">` 而不是 `<button>`：卡片里要放勾选角标等元素，
 * 真正的 `<button>` 里嵌交互元素是非法 HTML（浏览器会重排 DOM 导致 React
 * 报 `validateDOMNesting`），而且按钮默认样式要清一大堆。
 * 键盘可达性用 `tabIndex` + `onKeyDown` 补齐。
 */

import { useRef } from 'react'
import type { MouseEvent as ReactMouseEvent } from 'react'

import { formatFileSize } from '../../ipc'
import type { ThumbTier } from '../../ipc/manager'
import type { GridItem } from '../../hooks/useManager'
import { FolderIcon } from '../icons/FolderIcon'
import { ThumbCanvas, thumbSourceOf } from './ThumbCanvas'

/**
 * `1920 × 1080 · 353 KB`；缺哪段省哪段，不留空档。
 *
 * ⭐ 2026-10-21：**不再显示格式**（JPEG/PNG 那一段）。格式用顶部「格式」下拉筛就够了，
 * 卡片上这一行让位给「标签」按钮 —— 用户更关心这张图是什么，而不是它是什么编码。
 *
 * 尺寸放最前：它是判断「这张图能不能用在某处」的第一信息，
 * 也是用户在缩略图上看不出来的东西（缩略图都一样大）。
 */
function metaLine(item: GridItem): string {
  const parts: string[] = []
  if (item.width != null && item.height != null) parts.push(`${item.width} × ${item.height}`)
  if (item.size) parts.push(formatFileSize(item.size))
  return parts.join(' · ')
}

interface Props {
  item: GridItem
  tier: ThumbTier
  /** 在网格里的序号（0 起）。给 `useGridWindow` 用 —— 缩略图预取要知道谁排在哪 */
  index: number
  selected: boolean
  /** 浏览模式下为 false —— 未入库的行没有可做的索引操作，不显示勾选角标 */
  selectable: boolean
  /** 内容版本号（原地改写后 +1，逼缩略图重取） */
  rev?: number
  /**
   * 预取：这一张在「可见窗口」之后的 30 张里，提前把缩略图取回来。
   * 与「已经可见」不冲突 —— 可见时 `ThumbCanvas` 自己会走视口判断。
   */
  prefetch: boolean
  /** 多选切换（单击 / Ctrl+单击都走它） */
  onToggle: (e: ReactMouseEvent) => void
  /** 双击：回单图浏览打开 */
  onOpen: () => void
  /** 右键：弹卡片菜单（在资源管理器显示 / 复制路径 / 导出所选） */
  onContextMenu?: (e: ReactMouseEvent) => void
  /** 点「标签」：弹这张图的标签 + 置信度（未入库的行 `id` 为 null，不显示按钮） */
  onShowTags?: (id: number, e: ReactMouseEvent) => void
  /** 原地重命名进行中：文件名变成输入框，文本全选（2026-10-24，仅网格视图） */
  renaming?: boolean
  /** 提交（Enter / 失焦）：拿到的是**去扩展名的主体**，扩展名由上层保留 */
  onRenameCommit?: (stem: string) => void
  /** 取消（Esc） */
  onRenameCancel?: () => void
}

/** 文件名去掉扩展名（`dot > 0` 才算扩展名，`.hidden` 这种不算） */
function stemOf(name: string): string {
  const dot = name.lastIndexOf('.')
  return dot > 0 ? name.slice(0, dot) : name
}

export function PhotoCard({
  item,
  tier,
  index,
  selected,
  selectable,
  rev,
  prefetch,
  onToggle,
  onOpen,
  onContextMenu,
  onShowTags,
  renaming,
  onRenameCommit,
  onRenameCancel,
}: Props) {
  // 原地重命名：done 防止 Enter 提交后 blur 再提交一次
  const renameDone = useRef(false)

  // 子文件夹：没有缩略图（进去才有内容），画一个文件夹图标即可。
  //
  // ⭐ 2026-10-30：目录也要**能被选中**（用户：先选中再右键加进图库），
  //    而且选中效果要和照片**完全一致** —— 同样的高亮边框 + 右上角圆形勾。
  //    以前这里是个独立的小卡片：没有 onClick、没有 is-selected、没有勾，
  //    于是"单击没反应"。
  if (item.kind === 'dir') {
    return (
      <div
        className={`pv-mg-card is-dir${selected ? ' is-selected' : ''}`}
        // 文件夹卡片也带索引：否则「当前可见的最大序号」会漏掉它们，
        // 浏览一个纯目录时预取窗口就永远算不出来
        data-idx={index}
        role="button"
        tabIndex={0}
        aria-pressed={selected}
        title={`${item.path ?? item.name}\n双击进入`}
        onClick={onToggle}
        onDoubleClick={onOpen}
        onKeyDown={(e) => {
          if (e.key === 'Enter') onOpen()
          else if (e.key === ' ') {
            e.preventDefault()
            // 键盘路径没有鼠标事件，和照片卡片一样转一下复用同一套切换逻辑
            onToggle(e as unknown as ReactMouseEvent)
          }
        }}
      >
        <div className="pv-mg-cardimg">
          <div className="pv-mg-dirface">
            {/* 矢量图标：宽度跟着卡片档位（sm…xl）走，见 manager.css 的 `.pv-mg-diricon` */}
            <FolderIcon className="pv-mg-diricon" />
          </div>
          {selectable && (
            <span className="pv-mg-check" aria-hidden>
              {selected ? '✓' : ''}
            </span>
          )}
        </div>
        <div className="pv-mg-cardname">{item.name}</div>
        <div className="pv-mg-cardmeta">文件夹 · 双击进入</div>
      </div>
    )
  }

  const source = thumbSourceOf(item, rev)
  const offline = !source

  return (
    <div
      className={`pv-mg-card${selected ? ' is-selected' : ''}${offline ? ' is-offline' : ''}`}
      data-idx={index}
      role="button"
      tabIndex={0}
      aria-pressed={selectable ? selected : undefined}
      title={offline ? `${item.name}（所在卷未挂载）` : (item.path ?? item.name)}
      onClick={onToggle}
      onDoubleClick={onOpen}
      onContextMenu={onContextMenu}
      onKeyDown={(e) => {
        if (e.key === 'Enter') onOpen()
        else if (e.key === ' ') {
          e.preventDefault()
          // 键盘路径没有鼠标事件，直接复用同一套切换逻辑
          onToggle(e as unknown as ReactMouseEvent)
        }
      }}
    >
      <div className="pv-mg-cardimg">
        {source ? (
          <ThumbCanvas source={source} tier={tier} prefetch={prefetch} />
        ) : (
          // 没有可用的取图来源（索引行存在但卷没挂载）：占位保持网格对齐
          <div className="pv-mg-thumbbox">
            <span className="pv-mg-bad">离线</span>
          </div>
        )}
        {selectable && (
          <span className="pv-mg-check" aria-hidden>
            {selected ? '✓' : ''}
          </span>
        )}
        {/* 星级：只在真的打过分时显示，避免每张卡片都挂一排空星星 */}
        {item.rating > 0 && (
          <span className="pv-mg-stars" aria-label={`${item.rating} 星`}>
            {'★'.repeat(item.rating)}
          </span>
        )}
        {/* 旗标：与星标分开画，它是「待处理/保留」这类二元标记 */}
        {item.flag > 0 && (
          <span className="pv-mg-flagmark" aria-label="已标记">
            ●
          </span>
        )}
      </div>
      <div className="pv-mg-cardname">
        {renaming && onRenameCommit ? (
          <input
            className="pv-mg-namedit"
            defaultValue={stemOf(item.name)}
            autoFocus
            aria-label="重命名"
            // 挂载即全选（用户 2026-10-24：直接打字就覆盖原名）
            onFocus={(e) => e.currentTarget.select()}
            // ⛔ 别让输入框里的点击冒泡成卡片选中切换
            onClick={(e) => e.stopPropagation()}
            onDoubleClick={(e) => e.stopPropagation()}
            onKeyDown={(e) => {
              e.stopPropagation()
              if (e.key === 'Enter') {
                renameDone.current = true
                onRenameCommit(e.currentTarget.value)
              } else if (e.key === 'Escape') {
                renameDone.current = true
                onRenameCancel?.()
              }
            }}
            onBlur={(e) => {
              if (renameDone.current) return
              renameDone.current = true
              onRenameCommit(e.currentTarget.value)
            }}
          />
        ) : (
          item.name
        )}
      </div>
      <div className="pv-mg-cardmeta">
        <span className="pv-mg-metatext">{metaLine(item)}</span>
        {/* 未入库（浏览模式）没有 image_id，标签按钮没意义 */}
        {item.id != null && onShowTags && (
          <button
            type="button"
            className={`pv-mg-tagbtn${item.hasTags ? '' : ' is-empty'}`}
            title={
              item.hasTags
                ? '查看这张图的标签与置信度'
                : '这张图没有匹配到任何标签（阈值内一个都没有）'
            }
            // ⛔ 必须 stopPropagation：否则会顺带触发卡片的选中切换
            onClick={(e) => {
              e.stopPropagation()
              onShowTags(item.id as number, e)
            }}
            onDoubleClick={(e) => e.stopPropagation()}
          >
            {item.hasTags ? '标签' : '无匹配'}
          </button>
        )}
      </div>
    </div>
  )
}
