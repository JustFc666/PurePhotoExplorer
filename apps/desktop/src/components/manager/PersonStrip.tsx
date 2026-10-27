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

import { useEffect, useState } from 'react'
import { managerPeople, type PersonItem } from '../../ipc/people'
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

  /* 浮层里改了名 → 这里跟着重拉（否则筛选后的标题还是旧名字） */
  useEffect(() => {
    const onRenamed = () => {
      if (!visible) return
      void managerPeople(1, libraryId)
        .then(setPeople)
        .catch(() => undefined)
    }
    window.addEventListener('pv:person-renamed', onRenamed)
    return () => window.removeEventListener('pv:person-renamed', onRenamed)
  }, [visible, libraryId])

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
              /* 右键 = 打开人像墙浮层（命名/取消命名都在那里面，两处菜单保持一致） */
              e.preventDefault()
              const r = (e.currentTarget as HTMLElement).getBoundingClientRect()
              onOpenWall({ left: r.left, right: r.right, bottom: r.bottom })
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

    </div>
  )
}
