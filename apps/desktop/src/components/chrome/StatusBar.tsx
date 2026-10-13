/** 底部信息栏：文件名与完整路径（左下）、格式/尺寸/大小、浏览位置、缩放控件 */

import { formatFileSize, type ImageInfo } from '../../ipc'
import type { Status } from '../../types/viewer'
import { ZoomControl } from '../viewer/ZoomControl'

interface StatusBarProps {
  info: ImageInfo | null
  /**
   * 一次性操作提示（如「已送印 1240 × 1754」）。
   *
   * 与 `info` 里的持久信息不同：它是**短暂**的，由 `App` 在几秒后清掉。
   * 放在状态栏而不是弹窗：打印/保存这类操作的结果不需要用户点击确认，
   * 弹窗反而打断操作节奏。
   */
  notice: string | null
  status: Status
  /** 当前项在同目录列表中的索引（0 起，-1 = 未知） */
  index: number
  /** 同目录图片总数 */
  total: number
  /** 当前缩放倍率 */
  zoomScale: number
  /** 拖动缩放滑块 */
  onZoomScale: (scale: number) => void
  /**
   * 动图信息；非动图为 `null`。
   *
   * `truncated` 为真时说明帧数/内存超限、只解出了前若干帧 ——
   * 这种情况必须显式告诉用户，否则「动图播到一半就循环回开头」会被当成 bug。
   */
  animation: {
    playing: boolean
    frameIndex: number
    frameCount: number
    truncated: boolean
  } | null
}

export function StatusBar({
  info,
  notice,
  status,
  index,
  total,
  zoomScale,
  onZoomScale,
  animation,
}: StatusBarProps) {
  return (
    <footer className="pv-statusbar">
      {info ? (
        <>
          <span className="pv-status-name" title={info.fileName}>
            {info.fileName}
          </span>
          <span className="pv-status-sep" />
          {/* 完整路径放左下角；过长时省略号截断，悬停看全文 */}
          <span className="pv-status-path" title={info.path}>
            {info.path}
          </span>
          <span className="pv-status-sep" />
          <span>{info.formatLabel}</span>
          <span className="pv-status-sep" />
          <span>
            {info.width} × {info.height}
          </span>
          <span className="pv-status-sep" />
          <span>{formatFileSize(info.fileSize)}</span>
          {index >= 0 && total > 0 && (
            <>
              <span className="pv-status-sep" />
              <span className="pv-status-muted">
                {index + 1} / {total}
              </span>
            </>
          )}
          {info.isRaw && (
            <>
              <span className="pv-status-sep" />
              <span className="pv-badge">RAW</span>
            </>
          )}
          {notice && (
            <>
              <span className="pv-status-sep" />
              <span className="pv-status-notice">{notice}</span>
            </>
          )}
          {animation && (
            <>
              <span className="pv-status-sep" />
              <span className="pv-status-muted">
                {animation.playing ? '动图' : '动图（已暂停）'} {animation.frameIndex + 1}/
                {animation.frameCount} 帧
              </span>
              {animation.truncated && (
                <>
                  <span className="pv-status-sep" />
                  <span className="pv-badge pv-badge-warn" title="帧数或内存超限，只解出了前面的帧">
                    已截断
                  </span>
                </>
              )}
            </>
          )}
        </>
      ) : (
        <span className="pv-status-muted">
          {status.kind === 'error' ? status.message : '就绪'}
        </span>
      )}

      <span className="pv-spacer" />

      <span className="pv-status-muted">滚轮/空格翻页</span>
      <span className="pv-status-sep" />
      <ZoomControl scale={zoomScale} onScale={onZoomScale} />
    </footer>
  )
}
