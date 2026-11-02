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
import {
  managerOrphanFaces,
  managerPeople,
  managerRenamePerson,
  type OrphanFace,
  type PersonItem,
} from '../../ipc/people'
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
  /**
   * 限定在哪个图库。`undefined` = 全部图库。
   *
   * ⚠️ 不传的话浮层列出的是**全库**人物 —— 在「rednote」里点开人像墙
   * 会看到「测试图库」的人（2026-10-29 用户报的混合问题）。
   */
  libraryId?: number
  onClose: () => void
  /** 选中某人：调用方负责按这个人筛主区并关闭浮层 */
  onSelect: (id: number, name: string) => void
  /** 点「未归类」里的照片：打开那张原图 */
  onOpenFace: (path: string) => void
}

/**
 * 浮层宽度（**必须与 CSS `.pv-pwp` 的 `width` 一致**）。
 *
 * 锚点右对齐时用它算 `x = right - 宽度`，两边不一致浮层就偏出去（右边被窗口裁掉）。
 * ⛔ 改列数/卡片尺寸时，这里和 `manager.css` 的 `.pv-pwp { width }` 要一起改。
 */
export const PERSON_POP_W = 494

export function PersonWall({
  libraryId, anchor, currentId, onClose, onSelect, onOpenFace }: Props) {
  const [people, setPeople] = useState<PersonItem[]>([])
  /**
   * 没归入任何人物的脸（每张照片只留最大那张）。
   *
   * ⭐ 2026-11-02 用户要求「图库里每张有脸的照片都要能在人像里找到」。
   * 人物只覆盖聚成簇的那部分，剩下的补在这里，两张表合起来 = 全部有脸的图。
   */
  const [orphans, setOrphans] = useState<OrphanFace[]>([])
  const [tip, setTip] = useState<{ x: number; y: number; text: string } | null>(null)
  const [menu, setMenu] = useState<{ x: number; y: number; p: PersonItem } | null>(null)
  /** 正在内联改名的人物 id */
  const [renaming, setRenaming] = useState<number | null>(null)
  const [draft, setDraft] = useState('')
  const inputRef = useRef<HTMLInputElement | null>(null)

  useEffect(() => {
    let alive = true
    managerPeople(1, libraryId)
      .then((list) => {
        if (alive) setPeople(list)
      })
      .catch(() => undefined)
    /* 未归类与人物并行拉：两个请求互不依赖，串行会多等一个来回 */
    managerOrphanFaces({ limit: 120, libraryId })
      .then((list) => {
        if (alive) setOrphans(list)
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
          <span className="pv-pwp-sub">
            {people.length} 人
            {orphans.length > 0 ? ` · ${orphans.length} 张未归类` : ''}
          </span>
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

          {/*
            未归类：有人脸但没聚进任何人物的照片（一张照片只留最大那张脸）。
            点开 = 直接看那张原图 —— 用户 2026-11-02 要的「每张有脸的照片都能找到」。
          */}
          {orphans.length > 0 && (
            <>
              <div className="pv-pwp-sect">
                未归类 · {orphans.length} 张照片
                <span>有脸但没归入人物，点开即看原图</span>
              </div>
              {orphans.map((f) => (
                <div
                  key={`o${f.id}`}
                  role="button"
                  tabIndex={0}
                  className="pv-pwp-card is-orphan"
                  title={`未归类 · 源 ${Math.round(f.quality ?? 0)}px`}
                  onClick={() => {
                    if (!f.path) return
                    onOpenFace(f.path)
                    onClose()
                  }}
                  onKeyDown={(e) => {
                    if (e.key !== 'Enter' || !f.path) return
                    onOpenFace(f.path)
                    onClose()
                  }}
                  onMouseEnter={(e) =>
                    setTip({
                      x: e.clientX,
                      y: e.clientY,
                      text: `未归类 · 源 ${Math.round(f.quality ?? 0)}px`,
                    })
                  }
                  onMouseMove={(e) => setTip((t) => (t ? { ...t, x: e.clientX, y: e.clientY } : t))}
                  onMouseLeave={() => setTip(null)}
                >
                  <FaceAvatar face={f} size={80} fallback="?" />
                  <span className="pv-pwp-n">未</span>
                </div>
              ))}
            </>
          )}
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
