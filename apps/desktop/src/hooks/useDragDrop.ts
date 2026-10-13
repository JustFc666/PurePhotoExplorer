/**
 * 文件拖放接入。
 *
 * **为什么用 Tauri 原生 API 而不是 HTML5 的 onDrop**：
 * `tauri.conf.json` 里 `dragDropEnabled: true` 启用了 WebView2 的**原生**文件拖放。
 * 此时浏览器收不到 HTML5 的 drag/drop 事件——拖放被系统在原生层拦截，
 * 交给 Tauri 的 webview 通道。所以必须用 `getCurrentWebview().onDragDropEvent()`。
 *
 * HTML5 分支仅作为浏览器调试时的降级路径保留。
 */

import { useCallback, useEffect, useRef, useState } from 'react'
import { getCurrentWebview } from '@tauri-apps/api/webview'

export interface UseDragDropOptions {
  /** 拖入文件（已解析出真实路径） */
  onDropFile: (path: string) => void
  /** 无法识别文件等异常 */
  onError: (message: string) => void
}

export interface UseDragDrop {
  /** 拖拽悬停中（用于视觉反馈） */
  dragging: boolean
  /** 绑定到根元素的 HTML5 降级拖放属性（浏览器调试用） */
  html5DropProps: {
    onDrop: (e: React.DragEvent) => void
    onDragOver: (e: React.DragEvent) => void
  }
}

export function useDragDrop({ onDropFile, onError }: UseDragDropOptions): UseDragDrop {
  const [dragging, setDragging] = useState(false)
  /** 悬停状态的镜像，用于高频事件去重（`over` 每秒可触发数十次） */
  const draggingRef = useRef(false)
  /** 保证回调始终是最新引用，避免因依赖变化重复注册原生监听器 */
  const handlers = useRef({ onDropFile, onError })
  handlers.current = { onDropFile, onError }

  useEffect(() => {
    let unlisten: (() => void) | null = null
    let cancelled = false

    getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type === 'drop') {
          // noUncheckedIndexedAccess：paths[0] 类型是 string | undefined
          const first = event.payload.paths[0]
          if (first !== undefined) {
            handlers.current.onDropFile(first)
          } else {
            handlers.current.onError('未识别到文件')
          }
        }

        // 悬停状态用 ref 去重再 setState（`over` 事件触发极频繁）
        const hovering = event.payload.type === 'enter' || event.payload.type === 'over'
        if (hovering !== draggingRef.current) {
          draggingRef.current = hovering
          setDragging(hovering)
        }
      })
      .then((fn) => {
        // 若在监听器就绪前已卸载（StrictMode 假卸载），立即注销，避免重复注册
        if (cancelled) fn()
        else unlisten = fn
      })
      .catch(() => {
        // 非 Tauri 环境（如浏览器调试）忽略
      })

    // 清理函数里绝不调 setState（否则会引发「重渲染 → 重跑 → 死循环」）
    return () => {
      cancelled = true
      unlisten?.()
    }
  }, [])

  const onDrop = useCallback((e: React.DragEvent) => {
    e.preventDefault()
    setDragging(false)
    const file = e.dataTransfer.files[0]
    if (!file) return
    const path = (file as File & { path?: string }).path
    if (path) handlers.current.onDropFile(path)
    else handlers.current.onError('请使用「打开」按钮选择文件')
  }, [])

  const onDragOver = useCallback((e: React.DragEvent) => e.preventDefault(), [])

  return { dragging, html5DropProps: { onDrop, onDragOver } }
}
