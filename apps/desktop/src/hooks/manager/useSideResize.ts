/**
 * 侧栏宽度：拖主区左边那条缝调宽，松手记到 localStorage。
 *
 * ## 为什么监听挂在 window 上
 * 拖拽条只有 6px 宽，鼠标一快就划到主区里了 —— 监听挂在元素上会在移出的那一瞬
 * 丢掉 `mousemove`，宽度卡住不动（列表列宽那条也是同一个坑）。
 *
 * ## 为什么松手才落盘
 * 拖动过程中每移 1px 就写一次 localStorage 会明显掉帧。拖动期间只改内存。
 *
 * 宽度区间与持久化见 `components/manager/viewConfig.ts`。
 */

import { useCallback, useEffect, useRef, useState } from 'react'
import type { MouseEvent as ReactMouseEvent } from 'react'

import {
  SIDE_DEFAULT,
  clampSideWidth,
  readSideWidth,
  saveSideWidth,
} from '../../components/manager/viewConfig'

export type SideResizeApi = {
  /** 当前宽度（px），直接喂给侧栏 */
  sideW: number
  /** 双击拖拽条：回默认宽度，**并且记下来** */
  resetSide: () => void
  /** 正在拖（拖拽条要高亮） */
  resizing: boolean
  /** 拖拽条 `onMouseDown` */
  onSplitDown: (e: ReactMouseEvent) => void
}

export function useSideResize(): SideResizeApi {
  const [sideW, setSideW] = useState<number>(readSideWidth)
  const [resizing, setResizing] = useState(false)
  /** 按下那一刻的起点：`clientX` + 当时的宽度 */
  const drag = useRef({ startX: 0, startW: SIDE_DEFAULT })
  /**
   * 宽度的镜像。
   *
   * ⚠️ 松手时要写盘，但 `onUp` 是在 effect 里注册的（依赖数组只有 `resizing`），
   * 拿不到最新的 `sideW` —— 读 ref 才不会写进上一次渲染的旧值。
   */
  const sideWRef = useRef(sideW)
  sideWRef.current = sideW

  const onSplitDown = useCallback((e: ReactMouseEvent) => {
    e.preventDefault()
    drag.current = { startX: e.clientX, startW: sideWRef.current }
    setResizing(true)
  }, [])

  /**
   * 双击回默认宽度。
   *
   * ⚠️ 必须**一起落盘**：双击只是「不拖了也要改宽度」，和拖拽是同一种意图。
   * 早先只 `setSideW(SIDE_DEFAULT)` 不写盘，于是拖过之后再双击，图标看着回到
   * 默认，重启又冒出上次拖出来的宽度 —— 和自己给用户的提示（「双击恢复默认」）
   * 对不上。
   */
  const resetSide = useCallback(() => {
    setSideW(SIDE_DEFAULT)
    saveSideWidth(SIDE_DEFAULT)
  }, [])

  useEffect(() => {
    if (!resizing) return
    const { startX, startW } = drag.current
    const onMove = (e: MouseEvent) => {
      setSideW(clampSideWidth(startW + (e.clientX - startX)))
    }
    const onUp = () => {
      setResizing(false)
      // 写不进去（隐私模式等）就只对本会话生效 —— `saveSideWidth` 内部已吞异常
      saveSideWidth(sideWRef.current)
    }
    window.addEventListener('mousemove', onMove)
    window.addEventListener('mouseup', onUp)
    document.body.classList.add('is-col-resizing')
    return () => {
      window.removeEventListener('mousemove', onMove)
      window.removeEventListener('mouseup', onUp)
      document.body.classList.remove('is-col-resizing')
    }
  }, [resizing])

  return { sideW, resetSide, resizing, onSplitDown }
}
