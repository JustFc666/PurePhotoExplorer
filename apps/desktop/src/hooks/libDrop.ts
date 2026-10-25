/**
 * 把文件夹**拖到侧栏图库行上**直接入库。
 *
 * ## 为什么不能只用 HTML5 的 onDrop
 * `tauri.conf.json` 里 `dragDropEnabled: true` 打开了 WebView2 的**原生**文件拖放：
 * 从资源管理器拖进来的东西被系统在原生层拦下，走 Tauri 的 webview 通道，
 * **浏览器收不到 HTML5 的 drop 事件**。所以外部拖放必须用
 * `getCurrentWebview().onDragDropEvent()`，自己按坐标做命中测试。
 *
 * 应用**内部**的拖（文件浏览器树节点 → 图库行）是普通的 DOM 拖拽，
 * 不受这个开关影响，走 HTML5 那套即可（`data-pv-lib` 行上的 onDrop）。
 *
 * ## 命中测试
 * 原生事件只给「物理像素坐标」，没有元素。换算成 CSS 像素后用
 * `document.elementFromPoint` 找最近的有 `data-pv-lib` 的祖先。
 * ⚠️ 必须除以 `devicePixelRatio`（本机 DPI 125% → 1.25），
 * 否则坐标会偏出去一截，命中到旁边的行。
 *
 * ## 与全局「拖文件进来直接看图」的协调
 * `App.tsx` 也监听同一个事件（拖图片进来 = 打开看图）。两边都会收到 drop，
 * 所以这里把「当前悬停的图库 id」放在**模块级变量**里：
 * App 的处理器只要看到 `getLibDropHover() !== null`，就知道这次拖放
 * 已经被图库行接走了，自己不要再 `load(path)`（否则会拿一个目录去当图片打开）。
 */

import { useEffect, useRef, useState } from 'react'
import { getCurrentWebview } from '@tauri-apps/api/webview'

/** 当前悬停的图库 id（模块级：给 `App.tsx` 的全局拖放处理器判断用） */
let hoverId: number | null = null

export function getLibDropHover(): number | null {
  return hoverId
}

function setHover(id: number | null): void {
  hoverId = id
}

/** 把物理像素坐标换算成 CSS 像素后做命中测试，返回图库 id（没命中 = null）。 */
function libIdAt(pos: { x: number; y: number }): number | null {
  const scale = window.devicePixelRatio || 1
  const el = document.elementFromPoint(pos.x / scale, pos.y / scale)
  const row = el?.closest<HTMLElement>('[data-pv-lib]')
  if (!row) return null
  const raw = row.dataset.pvLib
  const id = Number(raw)
  return Number.isFinite(id) ? id : null
}

export interface UseLibDropOptions {
  /** 只在图库模式生效（ManagerView 常驻挂载，不活跃时是 `display:none`） */
  active: boolean
  /** 落在一个图库行上：`path` 是被拖进来的文件夹（也可能是文件，调用方自行判断） */
  onDropPath: (path: string, libId: number) => void
}

export interface UseLibDrop {
  /** 当前被拖拽悬停的图库 id（给侧栏加高亮） */
  dropLibId: number | null
}

export function useLibDrop({ active, onDropPath }: UseLibDropOptions): UseLibDrop {
  const [dropLibId, setDropLibId] = useState<number | null>(null)
  /** 高频 `over` 事件去重（`over` 每秒几十次，全 setState 会卡） */
  const hoverRef = useRef<number | null>(null)
  const handler = useRef(onDropPath)
  handler.current = onDropPath

  useEffect(() => {
    if (!active) return
    let unlisten: (() => void) | null = null
    let cancelled = false

    const apply = (id: number | null) => {
      if (hoverRef.current === id) return
      hoverRef.current = id
      setHover(id)
      setDropLibId(id)
    }

    getCurrentWebview()
      .onDragDropEvent((event) => {
        const p = event.payload
        switch (p.type) {
          case 'enter':
          case 'over':
            apply(libIdAt(p.position))
            break
          case 'drop': {
            const id = hoverRef.current ?? libIdAt(p.position)
            const first = p.paths[0]
            if (id !== null && first !== undefined) handler.current(first, id)
            /*
             * ⚠️ 清标记必须**延后一个宏任务**：`App.tsx` 的全局拖放处理器也在听
             * 同一个事件（拖图片进来 = 打开看图），它要靠「悬停标记还在」来判断
             * 这次拖放已被图库行接走。React 的 effect 是子先父后，所以**我们这个
             * 监听器先跑** —— 同步清掉的话，App 紧跟着就会拿一个目录去当图片打开。
             */
            setTimeout(() => apply(null), 0)
            break
          }
          case 'leave':
            apply(null)
            break
        }
      })
      .then((fn) => {
        if (cancelled) fn()
        else unlisten = fn
      })
      .catch(() => {
        /* 非 Tauri 环境（浏览器调试）忽略 */
      })

    return () => {
      cancelled = true
      unlisten?.()
      // 清理时绝不 setState，但要清模块级标记 —— 否则 App 会一直以为拖放在图库行上
      hoverRef.current = null
      setHover(null)
    }
  }, [active])

  return { dropLibId }
}
