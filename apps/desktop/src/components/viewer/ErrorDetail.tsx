/**
 * 错误详情 —— 可复制的诊断信息。
 *
 * **为什么需要**：错误提示只是给人看的，用户向开发者反馈时
 * 需要能复制完整信息（含技术细节、错误码、原始堆栈）。
 * 一层层截图转述既慢又容易丢信息。
 */

import { useCallback, useState } from 'react'
import { decodeError } from '../../ipc'

interface ErrorDetailProps {
  /** 面向用户的简短提示 */
  message: string
  /** 补充说明（如"需在 S3 阶段接入"） */
  hint?: string
  /** 原始错误对象，用于提取技术细节 */
  raw?: unknown
}

export function ErrorDetail({ message, hint, raw }: ErrorDetailProps) {
  const [copied, setCopied] = useState(false)
  const [expanded, setExpanded] = useState(false)

  const detail = decodeError(raw, message)

  const handleCopy = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(detail.full)
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    } catch {
      // 剪贴板不可用时降级为选中文本（CSS 已设 user-select: text）
      setCopied(false)
    }
  }, [detail.full])

  return (
    <div className="pv-error-detail">
      {/* 用户可读的主提示 */}
      <p className="pv-empty-sub pv-error-msg">{message}</p>
      {hint && <p className="pv-empty-sub pv-error-hint">{hint}</p>}

      {/* 展开技术细节 */}
      <div className="pv-error-actions">
        <button
          className="pv-btn pv-btn-sm"
          onClick={() => setExpanded((v) => !v)}
          aria-expanded={expanded}
        >
          {expanded ? '收起详情' : '技术详情'}
        </button>
        <button className="pv-btn pv-btn-sm" onClick={handleCopy}>
          {copied ? '已复制' : '复制诊断信息'}
        </button>
      </div>

      {expanded && (
        <div className="pv-error-panel">
          <table className="pv-error-table">
            <tbody>
              <tr>
                <th>错误码</th>
                <td>{detail.code}</td>
              </tr>
              <tr>
                <th>来源</th>
                <td>{detail.source}</td>
              </tr>
              {detail.path && (
                <tr>
                  <th>文件</th>
                  <td title={detail.path}>{detail.path}</td>
                </tr>
              )}
              <tr>
                <th>时间</th>
                <td>{detail.time}</td>
              </tr>
              <tr>
                <th>用户提示</th>
                <td>{detail.userVisible ? detail.message : '（已隐藏技术细节）'}</td>
              </tr>
            </tbody>
          </table>

          {detail.technical && (
            <pre className="pv-error-raw" onClick={(e) => selectAll(e)}>
              {detail.technical}
            </pre>
          )}

          <p className="pv-error-tip">点击上方文本区域可全选，诊断信息已包含时间戳与错误码。</p>
        </div>
      )}
    </div>
  )
}

/** 点击代码块时全选，方便手动复制 */
function selectAll(e: React.MouseEvent<HTMLPreElement>) {
  const range = document.createRange()
  range.selectNodeContents(e.currentTarget)
  const sel = window.getSelection()
  sel?.removeAllRanges()
  sel?.addRange(range)
}
