/**
 * 图库标题右侧的「AI 标签」条（2026-10-21 改为两级钻取）。
 *
 * ## 交互（用户 2026-10-21 定的）
 * 1. **先看一级类目**（人物 / 动物 / 风景…）+ 该类目下的张数；
 * 2. 点一级 → 换成它的**二级类目**（花 / 树 / 绿植），左侧给「← 全部」返回；
 * 3. 点二级 → 用这一组标签去筛（命中任一即可，后端 `tags_any`）。
 *
 * ## 为什么不在条里直接铺词条
 * 385 个词条横着排会变成一堵墙（实测侧栏 246 个标签已经很长了），
 * 两级钻取让第一屏永远是 20 个以内的稳定数量。
 *
 * 不在一级/二级词表里的标签（相机机型、地点这类 EXIF 标签）统一归到
 * **「EXIF / 其他」**，它们同样可点、可筛。
 */

import { useEffect, useMemo, useRef, useState } from 'react'
import '../../styles/aiTags.css'

export interface TagSummary {
  name: string
  count: number
}

/** 词条 → 所属类目（由后端 `ai_vocab` 提供） */
export type VocabMap = Map<string, { l1: string; l2: string }>

const OTHER = 'EXIF / 其他'
/**
 * 标签条：固定两行、**每行 6 个**，放不下的用「…」表示更多
 * （2026-10-25 用户定稿的表格布局：`全部 | tag1..tag6 | …` / `tag7..tag12 | …`）。
 *
 * ⛔ 别改成"全铺开"：右侧现在有人像条，标签行铺太长会**压到人像头像上面**
 *    （用户截图报过重叠）。6+6 是给右侧 80px 头像留出的固定空间。
 */
const PER_ROW = 6
const MAX_TAGS = PER_ROW * 2
/**
 * 每行固定几个标签（用户 2026-10-24 要求「每行 8 个」）。
 * ⛔ 不用 flex-wrap 自然折行：窗口一宽就全挤进一行，用户要的是**恒定两行**。
 */

interface Props {
  /** 当前范围内的标签（名字 + 张数），来自 `mgr.tags` */
  tags: TagSummary[]
  vocab: VocabMap
  /** 点一级/二级 = 按这一组标签筛（一级传该一级下的**全部**二级词） */
  onPickGroup: (label: string, names: string[]) => void
  /** 某个一级下有哪些二级词（点一级时整组筛） */
  termsOfL1: (l1: string) => string[]
  /** 词表里的**全部**一级（没有图的也要显示 0） */
  allL1: string[]
  /** 基础范围 key（图库/目录）：变了就退回一级列表 */
  baseKey: string
  /** 是否正按「无标签」筛选 */
  noTagActive: boolean
  /** 点「无标签」chip：切换筛选 */
  onToggleNoTag: () => void
  /**
   * 一级类目 → **去重**后的图片张数（后端 SQL 算）。
   *
   * ⛔ 不要用 `tags` 里二级的张数相加代替：一张图常同时挂「单人」「全身」，
   * 相加会把同一张图数两次（测试图库人像组 89 / 实际 52），
   * 界面上写 89 却只筛出 52 张，用户会以为「点一级没有显示全部」。
   */
  groupCounts: Record<string, number>
  /** 当前是否处于标签组筛选（决定「全部」是否高亮） */
  allActive: boolean
  /** 点「全部」= 取消标签组筛选，回到整库 */
  onShowAll: () => void
}

interface Group {
  label: string
  count: number
  names: string[]
}

function aggregate(
  tags: TagSummary[],
  vocab: VocabMap,
  level: 'l1' | 'l2',
  parent?: string,
): Group[] {
  const map = new Map<string, Group>()
  for (const t of tags) {
    const v = vocab.get(t.name)
    if (!v) {
      // EXIF 标签（机型/地点）没有类目 —— 只在**一级**里露出来，
      // 二级视图里不重复出现（否则每个类目下都挂一条「EXIF / 其他」，很怪）
      if (level === 'l2') continue
      const g = map.get(OTHER) ?? { label: OTHER, count: 0, names: [] }
      g.count += t.count
      g.names.push(t.name)
      map.set(OTHER, g)
      continue
    }
    if (level === 'l2' && v.l1 !== parent) continue
    const key = level === 'l1' ? v.l1 : v.l2
    const g = map.get(key) ?? { label: key, count: 0, names: [] }
    g.count += t.count
    g.names.push(t.name)
    map.set(key, g)
  }
  return [...map.values()].sort((a, b) => b.count - a.count)
}

export function AiTagStrip({
  tags,
  vocab,
  groupCounts,
  onPickGroup,
  termsOfL1,
  allL1,
  baseKey,
  noTagActive,
  onToggleNoTag,
  allActive,
  onShowAll,
}: Props) {
  /** null = 一级；否则是展开的一级类目名 */
  const [l1, setL1] = useState<string | null>(null)
  const [active, setActive] = useState<string | null>(null)
  /** 「…」浮层的位置（null = 关闭）。用 fixed 定位，避开父级 overflow 裁剪 */
  const [more, setMore] = useState<{ x: number; y: number } | null>(null)
  const boxRef = useRef<HTMLDivElement | null>(null)

  /**
   * 基础范围（图库/目录）换了才退回一级列表 —— 原来用的是「标签列表指纹」，
   * 2026-10-24 实测它会被莫名触发（计数器一路涨），用户点了一级芯片后
   * 二级视图立刻被弹回一级，面包屑根本点不到。范围 key 才是真正的意图。
   */
  const baseRef = useRef('')
  useEffect(() => {
    if (baseRef.current && baseRef.current !== baseKey) {
      setL1(null)
      setActive(null)
    }
    baseRef.current = baseKey
  }, [baseKey])
  // 词表变了（禁用/导入词条）导致当前一级不存在时，也要退回一级
  useEffect(() => {
    if (l1 && !allL1.includes(l1)) {
      setL1(null)
      setActive(null)
    }
  }, [l1, allL1])

  /**
   * 一级列表：**按词表的全量一级铺满**，没有图的显示 0。
   * 之前只显示"有图的类目"，用户看不到"这类一张都没有"，也点不了。
   */
  const level1 = useMemo(() => {
    const got = new Map(aggregate(tags, vocab, 'l1').map((g) => [g.label, g]))
    const full = allL1.map((label) => {
      const g = got.get(label)
      // ⭐ 张数一律用后端的去重计数；拿不到才退回"二级相加"（会偏大，仅兜底）
      return { label, count: groupCounts[label] ?? g?.count ?? 0, names: g?.names ?? [] }
    })
    return full.sort((a, b) => b.count - a.count)
  }, [tags, vocab, allL1, groupCounts])
  /**
   * 二级：**铺满该一级下的所有二级类目**（没有图的显示 0）。
   *
   * ⭐ 2026-10-24 用户要求「每个二级 tag 都要计算」：一级早就这么做了，
   * 二级原来只列"有图的类目"，用户看不到"这类一张都没有"。
   * 0 值的 chip 置灰不可点（`is-zero` + disabled），与一级一致。
   */
  const level2 = useMemo(() => {
    if (!l1) return []
    const got = new Map(aggregate(tags, vocab, 'l2', l1).map((g) => [g.label, g]))
    const all: string[] = []
    for (const v of vocab.values()) {
      if (v.l1 === l1 && !all.includes(v.l2)) all.push(v.l2)
    }
    /**
     * ⛔ 兜底：词表里找不到这个一级（或 `vocab` 还没加载好）时，
     * 退回"只列有图的类目" —— 否则整个二级列表会是空的，
     * 表现为「点了人像，下面什么都没有」。
     */
    if (all.length === 0) return aggregate(tags, vocab, 'l2', l1)

    const full = all.map((label) => {
      const g = got.get(label)
      return { label, count: g?.count ?? 0, names: g?.names ?? [] }
    })
    // 有图的按张数降序，0 的排后面（按名字稳定排序，避免每次刷新顺序乱跳）
    return full.sort((a, b) => b.count - a.count || a.label.localeCompare(b.label))
  }, [tags, vocab, l1])

  if (tags.length === 0) return null

  const list = l1 ? level2 : level1
  /** 只显示前 12 个（6×2），其余给「…」——右侧有 80px 头像，铺太宽会重叠 */
  const shown = list.slice(0, MAX_TAGS)
  const rest = list.length - shown.length
  const rows: Group[][] = []
  for (let i = 0; i < shown.length; i += PER_ROW) rows.push(shown.slice(i, i + PER_ROW))
  return (
    <div className="pv-ai-strip" ref={boxRef}>
      {/*
        ⭐ 2026-10-24：二级视图里原来放的是「← 全部」返回按钮，用户要求
        ① 不要左箭头 ② 和一级的「全部」长得一模一样 ③ 固定在 tag 栏左上第一个。
        所以这里统一渲染同一个 chip；点它 = 回一级列表 + 取消筛选。
      */}
      {/*
        ⭐ 2026-10-24 用户定稿：**「全部」单独占一列**（在标签网格左侧），
        不再混在每行 8 个的 chip 流里 —— 它是"取消筛选"的全局动作，
        和被筛的标签不是一类东西。一级/二级视图共用这一个按钮。
      */}
      {/*
        ⭐ 2026-10-25 定稿：左列是**上下两行** —— 「全部」在上、「无标签」在下，
        与右侧两行标签网格行对齐。点「无标签」= 只看没打任何标签的图。
      */}
      <div className="pv-ai-leftcol">
        <button
          type="button"
          className={`pv-ai-chip pv-ai-all${!l1 && !allActive ? ' is-on' : ''}`}
          title={l1 ? '返回一级类目，并显示这个图库的全部图片' : '显示该图库的全部图片'}
          onClick={() => {
            setL1(null)
            setActive(null)
            onShowAll()
          }}
        >
          全部
        </button>
        {!l1 && (
          <button
            type="button"
            className={`pv-ai-chip pv-ai-all${noTagActive ? ' is-on' : ''}`}
            title={
              noTagActive
                ? '取消「无标签」筛选'
                : '只看没有标签的图片（一个 AI / EXIF 标签都没打上）'
            }
            onClick={onToggleNoTag}
          >
            无标签
          </button>
        )}
      </div>
      {/*
        面包屑的一级名可点：点它 = **按整个一级筛选**，主区显示这一类的全部图片。
        用户 2026-10-24 定稿（截图为准）：`人像 → 单人 → 人像` 之后要停在**二级列表**，
        标签条显示成 `全部 | 人像 51 | 单人 33 | 全身 22 | …`，
        标题变回「人像 N 张」，而不是跳回一级类目列表。
      */}
      {l1 && (
        <button
          type="button"
          className={`pv-ai-crumb is-btn${active === l1 ? ' is-on' : ''}`}
          title={`显示「${l1}」的全部图片（${
            groupCounts[l1] ?? 0
          } 张），再点下面的二级可以继续收窄`}
          onClick={() => {
            setActive(l1)
            onPickGroup(l1, termsOfL1(l1))
          }}
        >
          {l1}
          {(groupCounts[l1] ?? 0) > 0 && <span className="pv-ai-num">{groupCounts[l1]}</span>}
        </button>
      )}

      <div className="pv-ai-chips">
        {/*
          「全部」按钮的落点（用户 2026-10-24 的 mockup）：
          标签条整体**上对齐**标题，所以第二行才挨着「N 张」。
          多行时把「全部」放到第二行行首，单行时放行首。
        */}
        {rows.map((row, ri) => (
          <div
            className="pv-ai-chiprow"
            key={ri}
          >
            {row.map((g) => (
              <button
                key={g.label}
                type="button"
                className={`pv-ai-chip${active === g.label ? ' is-on' : ''}${g.count === 0 ? ' is-zero' : ''}`}
                disabled={g.count === 0}
                title={
                  l1
                    ? `按「${g.label}」筛选：${g.names.join('、')}`
                    : `${g.label}：${g.names.length} 个标签，点击看二级`
                }
                onClick={() => {
                  if (!l1) {
                    // 点一级 = 进入该类目 + **立刻按整组筛选**主区（用户 2026-10-23 要求）
                    setL1(g.label)
                    setActive(g.label)
                    onPickGroup(g.label, termsOfL1(g.label))
                    return
                  }
                  setActive(g.label)
                  onPickGroup(`${l1} · ${g.label}`, g.names)
                }}
              >
                {g.label}
                <span className="pv-ai-num">{g.count}</span>
              </button>
            ))}
            {/*
              「无标签」是一级标签：只在**一级视图**末尾出现（用户 2026-10-24 要求，
              原来是顶部筛选栏的「标签 全部 ▾」下拉，已删掉）。进入二级后不再显示，
              因为「无标签」是一级筛选概念，二级是按一级类目钻取的具体词条。
            */}
            {/* 「…」= 还有标签没放下，点开浮层列出来（2026-10-25 用户要求） */}
            {ri === rows.length - 1 && rest > 0 && (
              <button
                type="button"
                className="pv-ai-more"
                title={`还有 ${rest} 个，点击展开`}
                onClick={(e) => {
                  const r = (e.currentTarget as HTMLElement).getBoundingClientRect()
                  setMore({ x: r.left, y: r.bottom + 4 })
                }}
              >
                …
              </button>
            )}
            {/*
              「已选 N 张 · 导出 / 标记」（用户 2026-10-24：显示在最后一个 tag 行的
              同一行右端，CSS 里 margin-left:auto 推到行尾）。
            */}
          </div>
        ))}
      </div>

      {/* 「…」展开的浮层：列出没放进两行的标签，点一个就按它筛 */}
      {more && (
        <>
          <div className="pv-pop-mask" onClick={() => setMore(null)} />
          <div className="pv-pop" style={{ left: more.x, top: more.y }} role="menu">
            {list.slice(MAX_TAGS).map((g) => (
              <button
                key={g.label}
                type="button"
                className="pv-pop-item"
                onClick={() => {
                  setActive(g.label)
                  onPickGroup(l1 ? `${l1} · ${g.label}` : g.label, g.names)
                  setMore(null)
                }}
              >
                <span>{g.label}</span>
                <span className="pv-pop-num">{g.count}</span>
              </button>
            ))}
          </div>
        </>
      )}
    </div>
  )
}
