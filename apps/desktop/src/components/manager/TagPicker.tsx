/**
 * 标签选择浮层（批量打标签）。
 *
 * ## 交互
 * - 输入即过滤已有标签；输入一个不存在的词 → 出现「新建「xxx」」，回车即建并应用
 * - 已打满的标签显示 ✓，点一下**整批去掉**；部分有的显示 ◐，点一下**整批加上**
 * - ↑↓ 选择、Enter 提交第一项、Esc 关闭
 *
 * ## 为什么要把「这批里有多少张带这个标签」拉回来
 * 没有这个数就只能显示"有/没有"，用户批量操作时不知道自己会不会把已有的又加一遍，
 * 也不知道点一下到底会加还是减。后端 `manager_tag_counts` 就是这么来的。
 */

import { useEffect, useMemo, useRef, useState } from 'react'
import { managerTagCounts, type TagItem } from '../../ipc/manager'

interface TagPickerProps {
  /** 作用对象（已入库的图；浏览模式的临时项没有 id，不能打标签） */
  ids: number[]
  /** 全部标签（侧栏那份，按使用次数排） */
  tags: TagItem[]
  /** 加标签（不存在则新建） */
  onAdd: (names: string[]) => Promise<unknown>
  /** 按 id 去掉整批的某个标签 */
  onRemove: (tagId: number) => Promise<unknown>
  onClose: () => void
}

/** 下拉里最多显示多少条候选（再多键盘也选不过来） */
const MAX_SUGGEST = 8

export function TagPicker({ ids, tags, onAdd, onRemove, onClose }: TagPickerProps) {
  const [text, setText] = useState('')
  const [hits, setHits] = useState<Record<string, number>>({})
  const [cursor, setCursor] = useState(0)
  const [busy, setBusy] = useState(false)
  const inputRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    inputRef.current?.focus()
  }, [])

  // 拉「这批图各自已有哪些标签」
  useEffect(() => {
    let alive = true
    void managerTagCounts(ids)
      .then((list) => {
        if (!alive) return
        const m: Record<string, number> = {}
        for (const h of list) m[h.name] = h.count
        setHits(m)
      })
      .catch(() => {
        /* 读不到就当作"都没有"，不影响继续打标签 */
      })
    return () => {
      alive = false
    }
  }, [ids])

  const q = text.trim()

  /** 候选列表：匹配的已有标签（前缀优先）+ 可能的"新建"项 */
  const suggest = useMemo(() => {
    const nameHit = new Map(tags.map((t) => [t.name, t]))
    const lower = q.toLowerCase()
    const matched = q
      ? tags
          .filter((t) => t.name.toLowerCase().includes(lower))
          .sort((a, b) => {
            // 前缀命中排前面，其余按使用次数（`tags` 已按次数排好）
            const ap = a.name.toLowerCase().startsWith(lower) ? 0 : 1
            const bp = b.name.toLowerCase().startsWith(lower) ? 0 : 1
            return ap - bp
          })
      : tags
    const list = matched.slice(0, MAX_SUGGEST)
    const canCreate = q.length > 0 && !nameHit.has(q)
    return { list, canCreate }
  }, [tags, q])

  // 候选变了 → 光标回到第一项
  useEffect(() => setCursor(0), [q])

  const total = ids.length
  const rows = suggest.list.length + (suggest.canCreate ? 1 : 0)

  const apply = async (name: string, tagId?: number) => {
    if (busy) return
    const have = hits[name] ?? 0
    const all = tagId != null && have >= total
    setBusy(true)
    try {
      if (all && tagId != null) {
        await onRemove(tagId)
        setHits((h) => ({ ...h, [name]: 0 }))
      } else {
        await onAdd([name])
        setHits((h) => ({ ...h, [name]: total }))
      }
      setText('')
      inputRef.current?.focus()
    } finally {
      setBusy(false)
    }
  }

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Escape') {
      e.preventDefault()
      onClose()
      return
    }
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      setCursor((c) => (rows === 0 ? 0 : (c + 1) % rows))
      return
    }
    if (e.key === 'ArrowUp') {
      e.preventDefault()
      setCursor((c) => (rows === 0 ? 0 : (c - 1 + rows) % rows))
      return
    }
    if (e.key === 'Enter') {
      e.preventDefault()
      if (rows === 0) return
      // 光标落在「新建」项上，或者没有候选但有输入 → 新建
      if (cursor >= suggest.list.length) {
        if (q) void apply(q)
        return
      }
      const t = suggest.list[cursor]
      if (t) void apply(t.name, t.id)
    }
  }

  return (
    <div className="pv-tagmask" onMouseDown={onClose}>
      <div className="pv-tagbox" onMouseDown={(e) => e.stopPropagation()}>
        <div className="pv-tagtitle">
          添加到 {total} 张照片
          <button type="button" className="pv-tagclose" onClick={onClose} aria-label="关闭">
            ×
          </button>
        </div>
        <input
          ref={inputRef}
          className="pv-taginput"
          value={text}
          placeholder="输入标签名，回车即建"
          onChange={(e) => setText(e.target.value)}
          onKeyDown={onKeyDown}
        />
        <div className="pv-taglist">
          {suggest.list.map((t, i) => {
            const have = hits[t.name] ?? 0
            const mark = have >= total ? '✓' : have > 0 ? '◐' : ''
            return (
              <button
                key={t.id}
                type="button"
                className={`pv-tagrow${i === cursor ? ' is-cur' : ''}`}
                onMouseEnter={() => setCursor(i)}
                onClick={() => void apply(t.name, t.id)}
              >
                <span className="pv-tagmark">{mark}</span>
                <span className="pv-tagname">{t.name}</span>
                {have > 0 && have < total && (
                  <span className="pv-tagmeta">
                    {have}/{total}
                  </span>
                )}
              </button>
            )
          })}
          {suggest.canCreate && (
            <button
              type="button"
              className={`pv-tagrow is-new${cursor === suggest.list.length ? ' is-cur' : ''}`}
              onMouseEnter={() => setCursor(suggest.list.length)}
              onClick={() => void apply(q)}
            >
              <span className="pv-tagmark">＋</span>
              <span className="pv-tagname">新建「{q}」</span>
            </button>
          )}
          {rows === 0 && <div className="pv-taghint">没有匹配的标签，输入新名字回车即可新建</div>}
        </div>
        {busy && <div className="pv-tagbusy">处理中…</div>}
      </div>
    </div>
  )
}
