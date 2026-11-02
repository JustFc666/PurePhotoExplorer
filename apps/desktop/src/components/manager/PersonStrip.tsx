/**
 * 人像条 —— 图库标题行右侧（标签条右边）的「人像识别」入口。
 *
 * ## 形态（2026-10-25 用户定稿：方案 B）
 * `人像 | 圆角方形头像 ×6 | +N | 39 人 · 517 脸 | 重新识别`
 *
 * ⭐ 头像是**圆角方形**不是圆形（用户明确要求）：小尺寸下圆形容器会切掉
 * 太多脸部信息，圆角方形更接近照片本身。
 *
 * ## 数据来源（2026-11-02 改为 `faces`）
 * 用户提出「只要识别出人脸就算人像」，所以这里直接铺**检测到的脸**，
 * 而不再是归纳后的 `persons`。差别：
 * - `persons`：30 个身份，一人一张头像，但只覆盖 ≥3 张脸且像真人的簇（约 70%）
 * - `faces`：392 张脸，全覆盖，但同一个人会出现多次
 *
 * 点了会分流：
 * - 这张脸已归属人物 → 按那个人筛主区（和以前一致）
 * - 未归类 → 直接打开这张脸所在的照片（`onOpenFace`）
 *
 * ⛔ 这里不放"设置/齿轮"：AI 设置唯一入口是顶栏的「设置」（用户 2026-10-24 定）。
 */

import { useEffect, useMemo, useState } from 'react'
import { managerFaces, managerRenamePerson, type FaceItem } from '../../ipc/people'
import { notifyPeopleChanged, onPeopleChanged } from '../../lib/peopleEvents'
import { ContextMenu, useContextMenu, type ContextMenuItem } from '../chrome/ContextMenu'
import { PromptDialog } from '../chrome/PromptDialog'
import { FaceAvatar } from './FaceAvatar'

interface Props {
  /** 点「更多面部」：把锚点交给调用方弹浮层（右对齐到方块右缘） */
  onOpenWall: (rect: { left: number; right: number; bottom: number }) => void
  /** 点已归类的脸：**按这个人筛主区网格**（与选 tag 同一效果，不开新页面） */
  onOpenPerson: (id: number, name: string) => void
  /** 点未归类的脸：直接打开那张照片 */
  onOpenFace: (path: string) => void
  /** 只有入库模式才显示（浏览模式没有 image_id，人脸挂不上） */
  visible: boolean
  /** 当前图库 id（有值就只显示"这个库里有人脸"的人物；目录/全部照片时传 undefined） */
  libraryId?: number
}

/** 条上最多取几张脸（约等于「全部」；用户库里 392 张，留够余量）。 */
const STRIP_LIMIT = 600

export function PersonStrip({ onOpenWall, onOpenPerson, onOpenFace, visible, libraryId }: Props) {
  const [faces, setFaces] = useState<FaceItem[]>([])
  /** 右键点中的脸 → 菜单的「作用对象」（菜单项里要用它） */
  const [ctxFace, setCtxFace] = useState<FaceItem | null>(null)
  /** 正在命名的人物。非 null 时弹输入框。 */
  const [renaming, setRenaming] = useState<FaceItem | null>(null)
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
    void managerFaces({ limit: STRIP_LIMIT, libraryId, onePerImage: true })
      .then((list) => {
        if (alive) setFaces(list)
      })
      .catch(() => undefined)
    return () => {
      alive = false
    }
  }, [visible, libraryId])

  /*
   * 人脸数据变了就重拉：改名、换封面、合并/拆分/删人物，以及**删照片 / 移除目录**
   * （脸会跟着图一起没）。2026-10-27 用户报「删除后『更多图像 共31人』没更新」——
   * 就是删目录这条路径漏了广播，数字停在旧值上。
   */
  useEffect(() => {
    if (!visible) return
    return onPeopleChanged(() => {
      void managerFaces({ limit: STRIP_LIMIT, libraryId, onePerImage: true })
        .then(setFaces)
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
  const faceMenuItems = useMemo<ContextMenuItem[]>(() => {
    const f = ctxFace
    /* 未归类的脸没有人物可命名 —— 菜单直接空着，右键不弹 */
    if (!f || f.personId == null) return []
    const pid = f.personId
    return [
      {
        id: 'rename',
        label: '命名…',
        onSelect: () => setRenaming(f),
      },
      {
        id: 'unname',
        label: '取消命名',
        separatorBefore: true,
        onSelect: () => {
          void managerRenamePerson(pid, null)
            .then(notifyPeopleChanged)
            .catch(() => undefined)
        },
      },
    ]
  }, [ctxFace])

  if (!visible) return null

  /*
   * ⚠️ 计数必须用 `faces.length`（已按图库过滤），
   * 不能用 `manager_faces_stats().faces` —— 那个是全库统计，
   * 会在「二次元」这种没人像的图库里也显示几百张脸（2026-10-25 同类问题）。
   */
  const total = faces.length
  /* 留一格给「…」：放不下时少显示一个头像，避免出现"最后一个被省略号挤掉" */
  const overflow = total > fit
  const shown = overflow ? faces.slice(0, fit - 1) : faces.slice(0, fit)
  const rest = Math.max(0, total - shown.length)

  const closeFaceMenu = () => {
    setCtxFace(null)
    ctx.close()
  }

  /** 提交命名。名字没变或为空就什么都不做。 */
  const commitName = (f: FaceItem, raw: string) => {
    const name = raw.trim()
    setRenaming(null)
    const pid = f.personId
    if (!pid || !name || name === (f.personName ?? '')) return
    void managerRenamePerson(pid, name)
      .then(notifyPeopleChanged)
      .catch(() => undefined)
  }

  /** 点一张脸：归了人就按人筛，没归类就开那张照片 */
  const openFace = (f: FaceItem) => {
    if (f.personId != null) onOpenPerson(f.personId, f.personName ?? '未命名')
    else if (f.path) onOpenFace(f.path)
  }

  /* 本图库一张脸都没有 → **整块不渲染**（连"开始识别"也不要，用户 2026-10-25 定） */
  if (total === 0) return null

  return (
    <div className="pv-ps">
      <div className="pv-ps-faces">
        {shown.map((f, i) => (
          <button
            key={f.id}
            type="button"
            className="pv-ps-face"
            title={`${f.personName ?? '未归类'} · 源 ${Math.round(f.quality ?? 0)}px`}
            onClick={() => openFace(f)}
            onContextMenu={(e) => {
              /* 右键 = 命名 / 取消命名，与照片墙里右键头像完全一致（仅限已归类） */
              if (f.personId == null) return
              setCtxFace(f)
              ctx.open(e)
            }}
          >
            <FaceAvatar face={f} size={80} fallback={String(i + 1)} />
          </button>
        ))}
        {/* 「…」点开浮层列出剩余人脸（用户 2026-10-25 要求） */}
        {rest > 0 && (
          <button
            type="button"
            className="pv-ps-more"
            title={`还有 ${rest} 张脸，点击展开`}
            onClick={(e) => {
              const r = (e.currentTarget as HTMLElement).getBoundingClientRect()
              onOpenWall({ left: r.left, right: r.right, bottom: r.bottom })
            }}
          >
            <span className="pv-ps-more-l">更多图像</span>
            <span className="pv-ps-more-s">共{total}脸</span>
          </button>
        )}
      </div>

      {/* 右键头像的菜单（命名… / 取消命名），和照片墙那套一致 */}
      {ctx.state && ctxFace && (
        <ContextMenu
          key={`${ctx.state.x},${ctx.state.y}`}
          position={{ x: ctx.state.x, y: ctx.state.y }}
          items={faceMenuItems}
          onClose={closeFaceMenu}
        />
      )}

      {/* 命名输入框（PromptDialog 内部已 portal 到 body，不会被标题区的 overflow 裁掉） */}
      {renaming && (
        <PromptDialog
          title="命名人物"
          label="这张脸属于一个人物。命名只写索引，不动任何照片。"
          placeholder="人物名称"
          initial={renaming.personName ?? ''}
          okLabel="保存"
          onOk={(name) => commitName(renaming, name)}
          onCancel={() => setRenaming(null)}
        />
      )}
    </div>
  )
}
