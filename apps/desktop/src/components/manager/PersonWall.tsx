/**
 * 人像墙**浮层**（2026-10-25 用户定稿：不再用整页，改成锚定浮层）。
 *
 * ## 形态
 * `人像 · 392 张脸` + 6 列头像网格（80×80 圆角，默认露三行）→ 点一张即筛主区并关闭。
 * - 每行**固定 6 个**，多了用滚轮滚动；
 * - **不显示名字**（卡片太挤），鼠标悬停用浮层显示「名字 · 源 N px」；
 * - 右下角小标：未归类的脸标「未」，已归类的留「·」；
 * - **右键**弹菜单：命名… / 取消命名（仅限已归类的脸，未归类没有人物可命名）；
 * - **无搜索框**（一屏基本看得完）。
 *
 * ## 数据来源（2026-11-02 改为 `faces`）
 * 用户要求「只要检出人脸就算人像」，所以这里铺的是**检测到的脸**而不是归纳后的人物：
 * 覆盖 100%（含那 30% 没聚进人物的），代价是同一人会重复出现多次。
 * 点已归类的脸 → 按那个人筛；点未归类的 → 直接开那张照片。
 *
 * ## 定位
 * 用 `position: fixed` + 调用方给的锚点（「更多面部」方块的 `getBoundingClientRect`），
 * 因为标题区有 `overflow: hidden`，`absolute` 会被裁掉。
 */

import { useEffect, useRef, useState } from 'react'
import { managerFaces, managerRenamePerson, type FaceItem } from '../../ipc/people'
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
   * ⚠️ 不传的话浮层列出的是**全库**人脸 —— 在「rednote」里点开人像墙
   * 会看到「测试图库」的人（2026-10-29 用户报的混合问题）。
   */
  libraryId?: number
  onClose: () => void
  /** 选中某人：调用方负责按这个人筛主区并关闭浮层 */
  onSelect: (id: number, name: string) => void
  /** 点未归类的脸：打开这张脸所在的原图 */
  onOpenFace: (path: string) => void
}

/**
 * 浮层宽度（**必须与 CSS `.pv-pwp` 的 `width` 一致**）。
 *
 * 锚点右对齐时用它算 `x = right - 宽度`，两边不一致浮层就偏出去（右边被窗口裁掉）。
 * ⛔ 改列数/卡片尺寸时，这里和 `manager.css` 的 `.pv-pwp { width }` 要一起改。
 */
export const PERSON_POP_W = 494

/**
 * 浮层里最多铺几张脸。
 *
 * ⚠️ 每格都要拉一次缩略图（`FaceAvatar` 有缓存，但首屏是实打实的 N 次解码，
 * 走 `thumbQueue` 的 4 并发）。60 格 = 十行，够看出覆盖度又不至于开面板就等两秒。
 */
const WALL_LIMIT = 60

export function PersonWall({ libraryId, anchor, currentId, onClose, onSelect, onOpenFace }: Props) {
  const [faces, setFaces] = useState<FaceItem[]>([])
  const [tip, setTip] = useState<{ x: number; y: number; text: string } | null>(null)
  const [menu, setMenu] = useState<{ x: number; y: number; f: FaceItem } | null>(null)
  /** 正在内联改名的人物 id */
  const [renaming, setRenaming] = useState<number | null>(null)
  const [draft, setDraft] = useState('')
  const inputRef = useRef<HTMLInputElement | null>(null)

  useEffect(() => {
    let alive = true
    managerFaces({ limit: WALL_LIMIT, libraryId })
      .then((list) => {
        if (alive) setFaces(list)
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

  const commitRename = (pid: number) => {
    const name = draft.trim()
    setRenaming(null)
    if (!name) return
    void managerRenamePerson(pid, name)
      .then(notifyPeopleChanged)
      .catch(() => undefined)
  }

  /** 点一张脸：归了人就按人筛并关闭，没归类就开那张照片 */
  const openFace = (f: FaceItem) => {
    if (f.personId != null) {
      onSelect(f.personId, f.personName ?? '未命名')
      return
    }
    if (f.path) {
      onOpenFace(f.path)
      onClose()
    }
  }

  return (
    <>
      <div className="pv-pop-mask" onClick={onClose} onContextMenu={(e) => e.preventDefault()} />
      <div
        className="pv-pwp"
        style={{ left: anchor.x, top: anchor.y }}
        onContextMenu={(e) => e.preventDefault()}
      >
        <div className="pv-pwp-head">
          <span className="pv-pwp-title">人像</span>
          <span className="pv-pwp-sub">最大的 {faces.length} 张脸</span>
          <div style={{ flex: 1 }} />
          <button type="button" className="pv-pwp-x" onClick={onClose} aria-label="关闭">
            ✕
          </button>
        </div>

        <div className="pv-pwp-grid">
          {faces.map((f, i) => (
            <div
              key={f.id}
              role="button"
              tabIndex={0}
              className={`pv-pwp-card${f.personId != null && currentId === f.personId ? ' is-on' : ''}`}
              onClick={() => {
                if (renaming != null) return
                openFace(f)
              }}
              onKeyDown={(e) => {
                if (e.key === 'Enter') openFace(f)
              }}
              onContextMenu={(e) => {
                e.preventDefault()
                e.stopPropagation()
                /* 未归类的脸没有人物可命名 */
                if (f.personId == null) return
                setMenu({ x: e.clientX, y: e.clientY, f })
              }}
              onMouseEnter={(e) =>
                setTip({
                  x: e.clientX,
                  y: e.clientY,
                  text: `${f.personName ?? '未归类'} · 源 ${Math.round(f.quality ?? 0)}px`,
                })
              }
              onMouseMove={(e) => setTip((t) => (t ? { ...t, x: e.clientX, y: e.clientY } : t))}
              onMouseLeave={() => setTip(null)}
            >
              {renaming != null && renaming === f.personId ? (
                <input
                  ref={inputRef}
                  className="pv-pwp-input"
                  value={draft}
                  onChange={(e) => setDraft(e.target.value)}
                  onBlur={() => commitRename(renaming)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') commitRename(renaming)
                    if (e.key === 'Escape') setRenaming(null)
                  }}
                  onClick={(e) => e.stopPropagation()}
                />
              ) : (
                <FaceAvatar
                  face={f}
                  size={80}
                  fallback={f.personName ? f.personName.slice(0, 1) : String(i + 1)}
                />
              )}
              <span className="pv-pwp-n">{f.personId == null ? '未' : '·'}</span>
            </div>
          ))}
          {faces.length === 0 && <div className="pv-pwp-empty">还没有检测到人脸</div>}
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
                setDraft(menu.f.personName ?? '')
                setRenaming(menu.f.personId)
                setMenu(null)
              }}
            >
              命名…
            </button>
            <button
              type="button"
              className="pv-pop-item"
              onClick={() => {
                const pid = menu.f.personId
                setMenu(null)
                if (pid == null) return
                void managerRenamePerson(pid, null)
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
