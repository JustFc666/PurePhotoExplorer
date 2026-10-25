/**
 * 单图模式的**左侧胶片条**：同目录图片的竖直缩略图列。
 *
 * ## 为什么从图库模式搬过来
 * 图库模式（S2）原来是「底部横向胶片条」，但它和网格表达的是同一件事
 * （都是当前结果集的缩略图），在网格下面再挂一条纯属重复。
 * 单图模式才是真正需要它的人：看着一张图时想跳到同目录的其它张。
 *
 * ## 虚拟滚动（只渲染可视区附近）
 * 同目录动辄上千张。全量渲染意味着上千个 `ThumbCanvas`，每个内部都会建一个
 * `IntersectionObserver` —— 滚动时明显掉帧。这里按固定行高只渲染可视区
 * ± 缓冲，上下用占位高度撑住滚动条，DOM 数量恒定在几十个。
 *
 * ## 缩略图来源
 * 复用 `ThumbCanvas` 的 `{ kind: 'path' }` 分支（`fs_thumbnail`，只解码不落盘）——
 * 单图模式看的是磁盘上的文件，未必入过库，没有索引 id 可用。
 */

import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'

import { ThumbCanvas } from '../manager/ThumbCanvas'

/** 单元格高度（含上下间距，必须与 CSS 一致，虚拟滚动靠它算偏移）。 */
const CELL_H = 96
/** 可视区上下各多渲染几行，快速滚动时不留白。 */
const OVERSCAN = 8

interface Props {
  /** 同目录的绝对路径（顺序 = 翻页顺序） */
  paths: string[]
  /** 当前图在列表里的下标（-1 = 未知，比如刚打开还没定位） */
  index: number
  /** 点某一张 → 切过去 */
  onPick: (path: string) => void
}

/** 取文件名（不带目录）。 */
function baseName(p: string): string {
  const i = Math.max(p.lastIndexOf('/'), p.lastIndexOf('\\'))
  return i >= 0 ? p.slice(i + 1) : p
}

/** 文件名去掉扩展名 —— 胶片条空间窄，扩名信息量最低。 */
function stemOf(name: string): string {
  const i = name.lastIndexOf('.')
  return i > 0 ? name.slice(0, i) : name
}

export function ThumbRail({ paths, index, onPick }: Props) {
  const listRef = useRef<HTMLDivElement>(null)
  /** 渲染窗口 [start, end) */
  const [win, setWin] = useState({ start: 0, end: 0 })

  /** 按当前滚动位置算渲染窗口 */
  const recompute = useCallback(() => {
    const el = listRef.current
    if (!el) return
    const viewH = el.clientHeight || 1
    const first = Math.floor(el.scrollTop / CELL_H)
    const count = Math.ceil(viewH / CELL_H)
    const start = Math.max(0, first - OVERSCAN)
    const end = Math.min(paths.length, first + count + OVERSCAN)
    setWin((w) => (w.start === start && w.end === end ? w : { start, end }))
  }, [paths.length])

  // 列表变化 / 首次挂载时算一次
  useLayoutEffect(() => {
    recompute()
  }, [recompute])

  /**
   * 当前项变化 → 滚到可视区。
   *
   * ⚠️ 不用 `scrollIntoView`：那会连带滚动所有可滚动祖先（包括未来的外层容器），
   * 这里只该动胶片条自己。用 `scrollTop` 直接算，副作用最小。
   */
  useEffect(() => {
    const el = listRef.current
    if (!el || index < 0) return
    const top = index * CELL_H
    const bottom = top + CELL_H
    const viewTop = el.scrollTop
    const viewBottom = viewTop + el.clientHeight
    if (top < viewTop + CELL_H) {
      // 往上出界（留一格余量，避免紧贴边缘）
      el.scrollTop = Math.max(0, top - CELL_H)
    } else if (bottom > viewBottom - CELL_H) {
      el.scrollTop = bottom - el.clientHeight + CELL_H
    }
    recompute()
  }, [index, recompute])

  if (paths.length === 0) {
    return <div className="pv-rail pv-rail-empty">同目录没有可翻的图片</div>
  }

  const head = win.start * CELL_H
  const tail = Math.max(0, (paths.length - win.end) * CELL_H)

  return (
    <div
      className="pv-rail"
      ref={listRef}
      onScroll={recompute}
      // ⚠️ 不在这里拦 `wheel`：React 的 `stopPropagation` 管不到 window 上的原生监听。
      // 真正让「滚胶片条不翻页」生效的是 `useWheelNavigation` 的白名单（`.pv-rail`）。
      aria-label="同目录胶片条"
    >
      <div className="pv-rail-pad" style={{ height: head }} aria-hidden />
      {paths.slice(win.start, win.end).map((p, i) => {
        const idx = win.start + i
        const cur = idx === index
        const name = baseName(p)
        return (
          <button
            type="button"
            key={p}
            className={`pv-rail-cell${cur ? ' is-current' : ''}`}
            style={{ height: CELL_H }}
            onClick={() => onPick(p)}
            title={p}
            aria-current={cur ? 'true' : undefined}
          >
            <span className="pv-rail-thumb">
              {/* 只解码不落盘：单图模式看的是磁盘文件，未必入过库 */}
              <ThumbCanvas source={{ kind: 'path', path: p }} tier="sm" />
            </span>
            <span className="pv-rail-name">{stemOf(name)}</span>
            {cur && <span className="pv-rail-dot" aria-hidden />}
          </button>
        )
      })}
      <div className="pv-rail-pad" style={{ height: tail }} aria-hidden />
    </div>
  )
}
