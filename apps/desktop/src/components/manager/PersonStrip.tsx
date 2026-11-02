/**
 * 人像条 —— 图库标题行右侧（标签条右边）的「人像识别」入口。
 *
 * ## 形态（2026-10-25 用户定稿：方案 B）
 * `人像 | 圆角方形头像 ×6 | +N | 39 人 · 517 脸 | 重新识别`
 *
 * ⭐ 头像是**圆角方形**不是圆形（用户明确要求）：小尺寸下圆形容器会切掉
 * 太多脸部信息，圆角方形更接近照片本身。
 *
 * ## 数据来源
 * - 计数：`manager_faces_stats`（人脸总数 / 已归属 / 人物数）
 * - 头像：`manager_people(3)` 的封面脸 —— 目前只取"谁"用于占位显示，
 *   **真正的脸裁剪还没接**（要用缩略图 + 归一化框在 canvas 里裁），
 *   所以现在显示人名首字或序号，接上后替换 `avatarOf()` 即可。
 *
 * ⛔ 这里不放"设置/齿轮"：AI 设置唯一入口是顶栏的「设置」（用户 2026-10-24 定）。
 */

import { useEffect, useMemo, useState } from 'react'
import { managerPeople, managerRenamePerson, type PersonItem } from '../../ipc/people'
import { notifyPeopleChanged, onPeopleChanged } from '../../lib/peopleEvents'
import { ContextMenu, useContextMenu, type ContextMenuItem } from '../chrome/ContextMenu'
import { PromptDialog } from '../chrome/PromptDialog'
import { FaceAvatar } from './FaceAvatar'

interface Props {
  /** 点「更多面部」：把锚点交给调用方弹浮层（右对齐到方块右缘） */
  onOpenWall: (rect: { left: number; right: number; bottom: number }) => void
  /** 点某个头像：**按这个人筛主区网格**（与选 tag 同一效果，不开新页面） */
  onOpenPerson: (id: number, name: string) => void
  /** 只有入库模式才显示（浏览模式没有 image_id，人脸挂不上） */
  visible: boolean
  /** 当前图库 id（有值就只显示"这个库里有人脸"的人物；目录/全部照片时传 undefined） */
  libraryId?: number
}

/** 未命名的显示 ①②…；命名的显示首字。 */
function avatarText(p: PersonItem, index: number): string {
  const n = p.name?.trim()
  if (n) return n.slice(0, 1)
  return String(index + 1)
}

export function PersonStrip({ onOpenWall, onOpenPerson, visible, libraryId }: Props) {
  const [people, setPeople] = useState<PersonItem[]>([])
  /** 右键点中的头像 → 菜单的「作用对象」（菜单项里要用它） */
  const [ctxPerson, setCtxPerson] = useState<PersonItem | null>(null)
  /** 正在命名的人物。非 null 时弹输入框。 */
  const [renaming, setRenaming] = useState<PersonItem | null>(null)
  const ctx = useContextMenu()
  /**
   * 一屏能放几个头像：按窗口宽度估（每格 80 + 8 间距）。
   * ⛔ 不用 ResizeObserver 量自身宽度 —— 自身宽度又取决于显示几个头像，会互相牵制。
   */
  const [fit, setFit] = useState(3)
  useEffect(() => {
    const calc = () => {
      const w = window.innerWidth
      /* 宽屏多放几个，别让标签和人像之间空一大片（用户 2026-10-25 反馈） */
      setFit(
        w >= 2000 ? 10
        : w >= 1900 ? 9
        : w >= 1800 ? 8
        : w >= 1700 ? 7
        : w >= 1600 ? 6
        : w >= 1500 ? 5
        : w >= 1400 ? 4
        : 3,
      )
    }
    calc()
    window.addEventListener('resize', calc)
    return () => window.removeEventListener('resize', calc)
  }, [])

  useEffect(() => {
    if (!visible) return
    let alive = true
    /* 两个请求并行：计数与头像互不依赖，串行会多等一个来回 */
    void Promise.all([
      /* 拉全部人物（≥1 张脸）：浮层里要能列出所有，只取 3 个会让「…」没内容 */
      managerPeople(1, libraryId).catch(() => [] as PersonItem[]),
    ]).then(([p]) => {
      if (alive) setPeople(p)
    })
    return () => {
      alive = false
    }
  }, [visible, libraryId])

  /*
   * 人物数据变了就重拉：改名、换封面、合并/拆分/删人物，以及**删照片 / 移除目录**
   * （脸会跟着图一起没）。2026-10-27 用户报「删除后『更多图像 共31人』没更新」——
   * 就是删目录这条路径漏了广播，数字停在旧值上。
   */
  useEffect(() => {
    if (!visible) return
    return onPeopleChanged(() => {
      void managerPeople(1, libraryId)
        .then(setPeople)
        .catch(() => undefined)
    })
  }, [visible, libraryId])

  /**
   * 右键头像的菜单项 —— **和照片墙里右键头像完全一致**（命名… / 取消命名）。
   *
   * ⛔ 别改回「右键就打开浮层」：原来就是这么做的，用户 2026-10-27 明确要求
   * 两处行为一致（浮层里能改名、人像条里也能，不必绕一层）。
   *
   * ⚠️ 这个 `useMemo` **必须留在下面所有提前 `return null` 之前**。
   * 曾经放在 `if (!visible) return null` 后面，`visible` 由 false 变 true 时
   * hook 数量变了 → React 抛 "Rendered more hooks than during the previous render"
   * → **整个应用白屏**（2026-10-27 踩过，差点去查渲染链路）。
   */
  const personMenuItems = useMemo<ContextMenuItem[]>(() => {
    const p = ctxPerson
    if (!p) return []
    return [
      {
        id: 'rename',
        label: '命名…',
        onSelect: () => setRenaming(p),
      },
      {
        id: 'unname',
        label: '取消命名',
        separatorBefore: true,
        onSelect: () => {
          /* 本地先改，别让用户等一个来回才看到效果 */
          setPeople((prev) => prev.map((x) => (x.id === p.id ? { ...x, name: null } : x)))
          void managerRenamePerson(p.id, null)
            .then(notifyPeopleChanged)
            .catch(() => undefined)
        },
      },
    ]
  }, [ctxPerson])

  if (!visible) return null

  /*
   * ⚠️ 人数必须用 `people.length`（已按图库过滤），
   * 不能用 `manager_faces_stats().persons` —— 那个是全库统计，
   * 会在「二次元」这种没人像的图库里也显示 32 人（2026-10-25 用户报的）。
   */
  const persons = people.length
  /* 留一格给「…」：放不下时少显示一个头像，避免出现"最后一个被省略号挤掉" */
  const overflow = people.length > fit
  const shown = people.slice(0, overflow ? fit - 1 : fit)
  const rest = Math.max(0, persons - shown.length)


  const closePersonMenu = () => {
    setCtxPerson(null)
    ctx.close()
  }

  /** 提交命名。名字没变或为空就什么都不做。 */
  const commitName = (p: PersonItem, raw: string) => {
    const name = raw.trim()
    setRenaming(null)
    if (!name || name === (p.name ?? '')) return
    setPeople((prev) => prev.map((x) => (x.id === p.id ? { ...x, name } : x)))
    void managerRenamePerson(p.id, name)
      .then(notifyPeopleChanged)
      .catch(() => undefined)
  }

  /* 本图库一个人物都没有 → **整块不渲染**（连"开始识别"也不要，用户 2026-10-25 定） */
  if (persons === 0) return null


  return (
    <div className="pv-ps">
      <div className="pv-ps-faces">
        {shown.map((p, i) => (
          <button
            key={p.id}
            type="button"
            className="pv-ps-face"
            title={`${p.name ?? '未命名'} · ${p.faceCount} 张脸 · 源 ${Math.round(
              p.cover?.quality ?? 0,
            )}px`}
            onClick={() => onOpenPerson(p.id, p.name ?? '未命名')}
            onContextMenu={(e) => {
              /* 右键 = 命名 / 取消命名，与照片墙里右键头像完全一致 */
              setCtxPerson(p)
              ctx.open(e)
            }}
          >
            <FaceAvatar face={p.cover} size={80} fallback={avatarText(p, i)} />
          </button>
        ))}
        {/* 「…」点开浮层列出剩余人物（用户 2026-10-25 要求） */}
        {rest > 0 && (
          <button
            type="button"
            className="pv-ps-more"
            title={`还有 ${rest} 个人物，点击展开`}
            onClick={(e) => {
              const r = (e.currentTarget as HTMLElement).getBoundingClientRect()
              onOpenWall({ left: r.left, right: r.right, bottom: r.bottom })
            }}
          >
            <span className="pv-ps-more-l">更多图像</span>
            <span className="pv-ps-more-s">共{persons}人</span>
          </button>
        )}
      </div>

      {/* 右键头像的菜单（命名… / 取消命名），和照片墙那套一致 */}
      {ctx.state && ctxPerson && (
        <ContextMenu
          key={`${ctx.state.x},${ctx.state.y}`}
          position={{ x: ctx.state.x, y: ctx.state.y }}
          items={personMenuItems}
          onClose={closePersonMenu}
        />
      )}

      {/* 命名输入框（PromptDialog 内部已 portal 到 body，不会被标题区的 overflow 裁掉） */}
      {renaming && (
        <PromptDialog
          title="命名人物"
          label={`这个人有 ${renaming.faceCount} 张脸。命名只写索引，不动任何照片。`}
          placeholder="人物名称"
          initial={renaming.name ?? ''}
          okLabel="保存"
          onOk={(name) => commitName(renaming, name)}
          onCancel={() => setRenaming(null)}
        />
      )}
    </div>
  )
}
