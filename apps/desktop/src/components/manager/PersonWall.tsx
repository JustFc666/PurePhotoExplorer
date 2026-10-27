/**
 * 人像墙**浮层**（2026-10-25 用户定稿：不再用整页，改成锚定浮层）。
 *
 * ## 形态
 * `人像 · 37 人` + 6 列头像网格（80×80 圆角，默认露三行）+ **只显示张数** → 点一个人即筛主区并关闭。
 * - 每行**固定 6 个**，多了用滚轮滚动；
 * - **不显示名字**（卡片太挤），鼠标悬停用浮层显示「名字 · N 张」；
 * - **右键**弹菜单：命名… / 取消命名（命名是内联输入，Enter 提交、Esc 取消）；
 * - **无搜索框**（37 人一屏基本看得完）。
 *
 * ## 定位
 * 用 `position: fixed` + 调用方给的锚点（「更多面部」方块的 `getBoundingClientRect`），
 * 因为标题区有 `overflow: hidden`，`absolute` 会被裁掉。
 */

import { useEffect, useRef, useState } from 'react'
import { managerPeople, managerRenamePerson, type PersonItem } from '../../ipc/people'
import { notifyPeopleChanged } from '../../lib/peopleEvents'
import { FaceAvatar } from './FaceAvatar'

export interface Anchor {
  x: number
  y: number
}

interface Props {
  /** 浮层左上角（由「更多面部」方块算出来，右对齐） */
  anchor: Anchor
  /** 当前正在筛的人物（高亮） */
  currentId?: number
  onClose: () => void
  /** 选中某人：调用方负责按这个人筛主区并关闭浮层 */
  onSelect: (id: number, name: string) => void
}

/** 浮层宽度（要与 CSS 的 `.pv-pwp` 保持一致，锚点右对齐时要用） */
export const PERSON_POP_W = 560

export function PersonWall({ anchor, currentId, onClose, onSelect }: Props) {
  const [people, setPeople] = useState<PersonItem[]>([])
  const [tip, setTip] = useState<{ x: number; y: number; text: string } | null>(null)
  const [menu, setMenu] = useState<{ x: number; y: number; p: PersonItem } | null>(null)
  /** 正在内联改名的人物 id */
  const [renaming, setRenaming] = useState<number | null>(null)
  const [draft, setDraft] = useState('')
  const inputRef = useRef<HTMLInputElement | null>(null)

  useEffect(() => {
    let alive = true
    managerPeople(1)
      .then((list) => {
        if (alive) setPeople(list)
      })
      .catch(() => undefined)
    return () => {
      alive = false
    }
  }, [])

  /* 进入改名：输入框挂上就全选（与卡片重命名同一套手感） */
  useEffect(() => {
    if (renaming == null) return
    const el = inputRef.current
    if (el) {
      el.focus()
      el.select()
    }
  }, [renaming])

  /* Esc：改名中先退改名，菜单开则关菜单，否则关浮层 */
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return
      if (renaming != null) setRenaming(null)
      else if (menu) setMenu(null)
      else onClose()
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [renaming, menu, onClose])

  const commitRename = (p: PersonItem) => {
    const name = draft.trim()
    setRenaming(null)
    if (!name) return
    /* 本地先更新，避免等一次往返才看到新名字 */
    setPeople((prev) => prev.map((x) => (x.id === p.id ? { ...x, name } : x)))
    void managerRenamePerson(p.id, name)
      .then(notifyPeopleChanged)
      .catch(() => undefined)
  }

  return (
    <>
      <div
        className="pv-pop-mask"
        onClick={onClose}
        onContextMenu={(e) => e.preventDefault()}
      />
      <div
        className="pv-pwp"
        style={{ left: anchor.x, top: anchor.y }}
        onContextMenu={(e) => e.preventDefault()}
      >
        <div className="pv-pwp-head">
          <span className="pv-pwp-title">人像</span>
          <span className="pv-pwp-sub">{people.length} 人</span>
          <div style={{ flex: 1 }} />
          <button type="button" className="pv-pwp-x" onClick={onClose} aria-label="关闭">
            ✕
          </button>
        </div>

        <div className="pv-pwp-grid">
          {people.map((p, i) => (
            <div
              key={p.id}
              role="button"
              tabIndex={0}
              className={`pv-pwp-card${currentId === p.id ? ' is-on' : ''}`}
              onClick={() => {
                if (renaming === p.id) return
                onSelect(p.id, p.name ?? '未命名')
              }}
              onKeyDown={(e) => {
                if (e.key === 'Enter') onSelect(p.id, p.name ?? '未命名')
              }}
              onContextMenu={(e) => {
                e.preventDefault()
                e.stopPropagation()
                setMenu({ x: e.clientX, y: e.clientY, p })
              }}
              onMouseEnter={(e) =>
                setTip({
                  x: e.clientX,
                  y: e.clientY,
                  text: `${p.name ?? '未命名'} · ${p.faceCount} 张 · 源 ${Math.round(
                    p.cover?.quality ?? 0,
                  )}px`,
                })
              }
              onMouseMove={(e) => setTip((t) => (t ? { ...t, x: e.clientX, y: e.clientY } : t))}
              onMouseLeave={() => setTip(null)}
            >
              {renaming === p.id ? (
                <input
                  ref={inputRef}
                  className="pv-pwp-input"
                  value={draft}
                  onChange={(e) => setDraft(e.target.value)}
                  onBlur={() => commitRename(p)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') commitRename(p)
                    if (e.key === 'Escape') setRenaming(null)
                  }}
                  onClick={(e) => e.stopPropagation()}
                />
              ) : (
                <FaceAvatar
                  face={p.cover}
                  size={80}
                  fallback={p.name ? p.name.slice(0, 1) : String(i + 1)}
                />
              )}
              <span className="pv-pwp-n">{p.faceCount}</span>
            </div>
          ))}
          {people.length === 0 && <div className="pv-pwp-empty">还没有归纳出人物</div>}
        </div>
      </div>

      {menu && (
        <>
          <div
            className="pv-pop-mask"
            onClick={() => setMenu(null)}
            onContextMenu={(e) => e.preventDefault()}
          />
          <div className="pv-pop" style={{ left: menu.x, top: menu.y }} role="menu">
            <button
              type="button"
              className="pv-pop-item"
              onClick={() => {
                setDraft(menu.p.name ?? '')
                setRenaming(menu.p.id)
                setMenu(null)
              }}
            >
              命名…
            </button>
            <button
              type="button"
              className="pv-pop-item"
              onClick={() => {
                const p = menu.p
                setMenu(null)
                setPeople((prev) => prev.map((x) => (x.id === p.id ? { ...x, name: null } : x)))
                void managerRenamePerson(p.id, null)
                  .then(notifyPeopleChanged)
                  .catch(() => undefined)
              }}
            >
              取消命名
            </button>
          </div>
        </>
      )}

      {tip && (
        <div className="pv-fdd-tip" style={{ left: tip.x + 14, top: tip.y + 16 }}>
          {tip.text}
        </div>
      )}
    </>
  )
}
