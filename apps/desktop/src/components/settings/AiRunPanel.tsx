/**
 * AI 任务一键执行面板（打标 / 人像识别共用）。
 *
 * ## 2026-10-29 改造（用户要求）
 * 1. 图库选择从「一排按钮」改成**下拉框**，并且多一项「全部图库」
 * 2. 按钮文案随状态变：`打标` / `重新打标`、`识别人像` / `重新识别人像`
 * 3. 已经跑过 → **二次确认**再动手
 * 4. 识别人像时如果这个范围**还没打过标** → 提示，并可先打标再识别人像
 *
 * ## 为什么按钮文案要从后端查
 * 「有没有跑过」在前端不可信：换个图库就得重算，而且用户可能刚在别处跑过。
 * 所以每次换范围都问一次 `ai_library_status`，按 `tagged` / `faced` 决定。
 */

import { useCallback, useEffect, useRef, useState } from 'react'

import { ConfirmDialog } from '../chrome/ConfirmDialog'
import { ContextMenu, type ContextMenuItem } from '../chrome/ContextMenu'
import {
  aiLibraryStatus,
  aiRunTask,
  aiTaskStatus,
  aiTaskStop,
  managerLibraries,
  type AiTaskStatus,
  type LibraryAiStatus,
  type LibraryItem,
} from '../../ipc/manager'

interface Props {
  task: 'tag' | 'face'
  /** 行标题，如「打标」 */
  title: string
  /** 底部灰字说明 */
  hint: string
}

export function AiRunPanel({ task, title, hint }: Props) {
  const [libs, setLibs] = useState<LibraryItem[]>([])
  /** `''` = 全部图库 */
  const [picked, setPicked] = useState('')
  const [cov, setCov] = useState<LibraryAiStatus | null>(null)
  const [busy, setBusy] = useState(false)
  const [status, setStatus] = useState<AiTaskStatus | null>(null)
  const [err, setErr] = useState<string | null>(null)
  /** 下拉菜单位置（不为 null 表示展开） */
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null)
  /** 待确认的动作 */
  const [ask, setAsk] = useState<null | 'rerun' | 'pretag'>(null)

  const pickRef = useRef<HTMLButtonElement>(null)
  const logRef = useRef<HTMLPreElement>(null)

  /* 图库列表 */
  useEffect(() => {
    void managerLibraries()
      .then((list) => {
        setLibs(list)
        setPicked((cur) => cur || list[0]?.name || '')
      })
      .catch(() => undefined)
  }, [])

  /* 换范围就重查「跑过没有」—— 按钮文案与二次确认都靠它 */
  const refreshCov = useCallback((lib: string) => {
    void aiLibraryStatus(lib)
      .then(setCov)
      .catch(() => setCov(null))
  }, [])
  useEffect(() => {
    if (picked) refreshCov(picked)
  }, [picked, refreshCov])

  /* 进页面时如果后台已经有任务在跑（切走再切回来），直接接上轮询 */
  useEffect(() => {
    void aiTaskStatus()
      .then((st) => {
        setStatus(st)
        if (st.running) setBusy(true)
      })
      .catch(() => undefined)
  }, [])

  /* 轮询：500ms 一次，只在 busy 时跑（空闲时不停请求会一直抢后端那把任务锁） */
  useEffect(() => {
    if (!busy) return
    let alive = true
    const tick = async () => {
      try {
        const st = await aiTaskStatus()
        if (!alive) return
        setStatus(st)
        if (st.done) {
          setBusy(false)
          refreshCov(picked)
          return
        }
        window.setTimeout(tick, 500)
      } catch (e) {
        if (alive) {
          setErr(e instanceof Error ? e.message : String(e))
          setBusy(false)
        }
      }
    }
    void tick()
    return () => {
      alive = false
    }
  }, [busy, picked, refreshCov])

  /* 新日志出来就滚到底 */
  useEffect(() => {
    const el = logRef.current
    if (el) el.scrollTop = el.scrollHeight
  }, [status?.lines])

  /** 等后台任务跑完（串行「先打标再识别人像」要用）。 */
  const waitDone = async () => {
    for (let i = 0; i < 4500; i++) {
      await new Promise((r) => window.setTimeout(r, 800))
      try {
        const st = await aiTaskStatus()
        setStatus(st)
        if (st.done) return
      } catch {
        return
      }
    }
  }

  /** 跑任务；`preTag` = 先打标再识别人像（串行）。 */
  const run = async (preTag: boolean) => {
    setErr(null)
    try {
      if (preTag) {
        await aiRunTask('tag', picked)
        setBusy(true)
        await waitDone()
      }
      await aiRunTask(task, picked)
      setBusy(true)
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
      setBusy(false)
    }
  }

  /**
   * 点主按钮：先判断要不要确认。
   *
   * ⭐ **必须现查**，不能用 state 里的 `cov`。
   * 那次踩的坑：换到「rednote」（373 张、从没打过标）后立刻点「识别人像」，
   * `picked` 已经变了但 `aiLibraryStatus` 的请求还没回来，`cov` 还是上一个库的
   * （tagged=8）→ 判定「已打标」→ 既不提示也不先打标，直接开跑。
   * 现查一次只多几十毫秒，但判断一定基于当前范围。
   */
  const onStart = async () => {
    let st: LibraryAiStatus | null = cov
    try {
      st = await aiLibraryStatus(picked)
      setCov(st)
    } catch {
      /* 查不到就沿用旧值（null 也不阻塞：走「直接跑」） */
    }

    if (task === 'face' && st && st.tagged === 0 && st.total > 0) {
      setAsk('pretag')
      return
    }
    const doneBefore = task === 'tag' ? (st?.tagged ?? 0) > 0 : (st?.faced ?? 0) > 0
    if (doneBefore) {
      setAsk('rerun')
      return
    }
    void run(false)
  }

  const stop = async () => {
    try {
      await aiTaskStop()
    } catch {
      /* 停止失败不阻塞用户：下一次轮询会发现任务已不在 */
    }
    setBusy(false)
  }

  const openMenu = () => {
    const r = pickRef.current?.getBoundingClientRect()
    if (!r) return
    setMenu({ x: r.left, y: r.bottom + 4 })
  }

  const items: ContextMenuItem[] = [
    { id: '__all__', label: '全部图库', checked: picked === '', onSelect: () => setPicked('') },
    ...libs.map((l) => ({
      id: String(l.id),
      label: `${l.name}（${l.count} 张）`,
      checked: picked === l.name,
      onSelect: () => setPicked(l.name),
    })),
  ]

  const scopeLabel = picked || '全部图库'
  const doneBefore = task === 'tag' ? (cov?.tagged ?? 0) > 0 : (cov?.faced ?? 0) > 0
  const startLabel = doneBefore
    ? task === 'tag'
      ? '重新打标'
      : '重新识别人像'
    : task === 'tag'
      ? '打标'
      : '识别人像'

  const stateText = busy
    ? '运行中…'
    : status?.done
      ? status.ok
        ? '已完成'
        : '失败（看下面的日志）'
      : ''

  return (
    <div className="pv-set-row" style={{ marginTop: 18 }}>
      <div className="pv-set-label">{title}</div>

      {/* 范围下拉 */}
      <div style={{ display: 'flex', gap: 8, alignItems: 'center', marginBottom: 8 }}>
        <button
          ref={pickRef}
          type="button"
          className="pv-set-btn"
          disabled={busy}
          onClick={openMenu}
          style={{ minWidth: 180, display: 'flex', justifyContent: 'space-between', gap: 10 }}
        >
          <span>{scopeLabel}</span>
          <span style={{ opacity: 0.6 }}>▾</span>
        </button>
        {cov && (
          <span className="pv-set-hint">
            {cov.total} 张
            {task === 'tag'
              ? cov.tagged > 0
                ? ` · 已打标 ${cov.tagged}`
                : ' · 未打标'
              : cov.faced > 0
                ? ` · 已识别 ${cov.faced}`
                : ' · 未识别'}
          </span>
        )}
        {libs.length === 0 && <span className="pv-set-hint">还没有图库</span>}
      </div>

      <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
        <button
          className="pv-set-btn"
          disabled={!picked || busy || (cov?.total ?? 0) === 0}
          onClick={() => void onStart()}
        >
          {startLabel}
        </button>
        <button className="pv-set-btn" disabled={!busy} onClick={() => void stop()}>
          停止
        </button>
        {stateText && (
          <span
            className="pv-set-hint"
            style={{ color: busy ? undefined : status?.ok ? '#8fd694' : '#f09595' }}
          >
            {stateText}
          </span>
        )}
      </div>

      {status && status.lines.length > 0 && (
        <pre className="pv-set-log" ref={logRef}>
          {status.lines.join('\n')}
        </pre>
      )}
      {err && (
        <div className="pv-set-hint" style={{ color: '#f09595' }}>
          {err}
        </div>
      )}
      <div className="pv-set-hint">{hint}</div>

      {menu && (
        <ContextMenu
          key={`${menu.x},${menu.y}`}
          items={items}
          position={menu}
          width={240}
          onClose={() => setMenu(null)}
        />
      )}

      {ask === 'rerun' && (
        <ConfirmDialog
          title={task === 'tag' ? '重新打标' : '重新识别人像'}
          message={
            task === 'tag'
              ? `「${scopeLabel}」已经打过标（${cov?.tagged} 张）。重新打标会先清掉这些图的 AI 标签再写新的，不会越跑越多。`
              : `「${scopeLabel}」已经识别过人像（${cov?.faced} 张）。重新识别会覆盖这些人脸并重建人物，已命名的人物名字会丢。`
          }
          okLabel="继续"
          danger
          onOk={() => {
            setAsk(null)
            void run(false)
          }}
          onCancel={() => setAsk(null)}
        />
      )}

      {ask === 'pretag' && (
        <ConfirmDialog
          title="还没打标"
          message={`「${scopeLabel}」还没有 AI 标签。识别人像本身用不到标签，但先打标能把「截图 / 文档」这类明显不是人像的图筛掉，少做无用功。是否先打标，再识别人像？`}
          okLabel="先打标再识别"
          cancelLabel="直接识别"
          onOk={() => {
            setAsk(null)
            void run(true)
          }}
          onCancel={() => {
            setAsk(null)
            void (async () => {
              try {
                await aiRunTask('face', picked)
                setBusy(true)
              } catch (e) {
                setErr(e instanceof Error ? e.message : String(e))
              }
            })()
          }}
        />
      )}
    </div>
  )
}
