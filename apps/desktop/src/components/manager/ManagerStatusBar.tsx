/**
 * 图库底部的状态栏（从 `ManagerView.tsx` 拆出，2026-11）。
 *
 * 这里只渲染、不算业务：所有数字都由调用方算好传进来。
 * 唯一的规则（「已选」的措辞）在 `viewConfig.selectionSummary`，那是纯函数、有单测。
 *
 * ⛔ 别在这里读 `mgr`：状态栏是「看着像摆件、实际最容易和别处数字对不上」的那类 UI，
 * 一旦它自己去捞数据，改一个字段就会悄悄裂成两种口径。
 */

import { formatFileSize } from '../../ipc/format'
import { fmtCount, selectionSummary } from './viewConfig'

export type ManagerStatusBarProps = {
  /** 还没选任何范围（只看到「图库共 N 张」） */
  idle: boolean
  /** 全库总张数 —— 只在 `idle` 时显示 */
  statsTotal: number
  /** 当前列出的条数（浏览模式可能被截断） */
  shown: number
  /** 当前筛选命中的总数 */
  total: number
  /** 浏览模式（未入库） */
  browsing: boolean
  /** 当前筛选结果的字节总数 */
  totalBytes: number
  /** 浏览目录里还有更多，只列出了前 N 个 */
  browseTruncated: boolean
  /** 已列出的图片条数（`browseTruncated` 的提示里要报这个数） */
  fileCount: number
  /** 还在解码的缩略图数量 */
  pendingThumbs: number
  /** 入库扫描的进度文案 */
  scanInfo: string | null
  /** 当前图库根目录或浏览目录 */
  path: string | null
  /** 后端报错 */
  error: string | null
  /** 选中条数 */
  selectedCount: number
  /** 选中的是否**全是**目录（决定「N 个文件夹」还是「N 张 · 大小」） */
  selectedAllDirs: boolean
  /** 选中项的字节总数 */
  selectedBytes: number
}

export function ManagerStatusBar(props: ManagerStatusBarProps) {
  const {
    idle,
    statsTotal,
    shown,
    total,
    browsing,
    totalBytes,
    browseTruncated,
    fileCount,
    pendingThumbs,
    scanInfo,
    path,
    error,
    selectedCount,
    selectedAllDirs,
    selectedBytes,
  } = props

  return (
    <footer className="pv-mg-status">
      <span>
        {idle ? (
          `图库共 ${fmtCount(statsTotal)} 张`
        ) : (
          <>
            {fmtCount(shown)} / {fmtCount(total)} 个项目
            {!browsing && totalBytes > 0 && ` · ${formatFileSize(totalBytes)}`}
            {browsing && ' · 浏览模式（未入库）'}
          </>
        )}
      </span>
      {browseTruncated && (
        <span className="pv-mg-dim">
          · 目录里还有更多，只列出前 {fileCount} 张图片
        </span>
      )}
      {pendingThumbs > 0 && (
        <span className="pv-mg-dim" title="首次看某个目录时要逐张解码，看完不会留在缓存里">
          · 正在解码缩略图，剩余 {pendingThumbs}
        </span>
      )}
      {!browsing && scanInfo && <span className="pv-mg-dim">· {scanInfo}</span>}
      {!idle && path && (
        <span className="pv-mg-dim pv-mg-statuspath" title={path}>
          · {path}
        </span>
      )}
      {error && <span className="pv-mg-err">· {error}</span>}
      {/*
        ⭐ 2026-10-24 用户定稿：选中信息（「已选 N 张 · 大小」）放到**状态栏右下角**，
        并且**去掉「导出 / 标记」两个按钮** —— 这两个动作在右键菜单里本来就有，
        挂在状态栏上既挤又容易误点。
      */}
      {selectedCount > 0 && (
        <span className="pv-mg-dim pv-mg-selinfo">
          {selectionSummary(selectedCount, selectedAllDirs, selectedBytes)}
        </span>
      )}
    </footer>
  )
}
