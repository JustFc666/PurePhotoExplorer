/**
 * AI 任务一键执行面板（打标 / 人像识别）。
 *
 * ⭐ 2026-10-28 用户要求：不要「复制命令去终端粘」，按一下就跑。
 *
 * ## 这里是过渡实现
 * 现阶段后端 spawn `tools/ai/*.py`（需要本机 Python + 项目源码）。
 * 发行版会换成 Rust + `ort`（DirectML）直接推理 —— 那时**只换后端**，
 * 这个组件的按钮、状态、日志都不用动。
 *
 * ## 为什么用轮询而不是事件
 * 后端把 stdout/stderr 直接重定向到临时文件，这里 500ms 拉一次尾部。
 * 比「开两条线程读管道 + emit 事件」简单得多，也不会有管道写满卡死子进程的问题。
 */

import { useEffect, useRef, useState } from 'react'

import {
  aiRunTask,
  aiTaskStatus,
  aiTaskStop,
  managerLibraries,
  type AiTaskStatus,
  type LibraryItem,
} from '../../ipc/manager'

interface Props {
  task: 'tag' | 'face'
  /** 行标题，如「重新打标」 */
  title: string
  /** 底部灰字说明 */
  hint: string
}

export function AiRunPanel({ task, title, hint }: Props) {
  const [libs, setLibs] = useState<LibraryItem[]>([])
  const [picked, setPicked] = useState('')
  const [busy, setBusy] = useState(false)
  const [status, setStatus] = useState<AiTaskStatus | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const logRef = useRef<HTMLPreElement>(null)

  useEffect(() => {
    void managerLibraries()
      .then((list) => {
        setLibs(list)
        const first = list[0]
        if (first) setPicked(first.name)
      })
      .catch(() => undefined)
  }, [])

  /* 进页面时如果后台已经有任务在跑（切走再切回来），直接接上轮询 */
  useEffect(() => {
    void aiTaskStatus()
      .then((st) => {
        setStatus(st)
        if (st.running) setBusy(true)
      })
      .catch(() => undefined)
  }, [])

  /*
   * 轮询：500ms 一次。
   * ⚠️ 只在 `busy` 时轮询 —— 空闲时不停请求会一直抢后端那把任务锁。
   */
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
  }, [busy])

  /* 新日志出来就滚到底 —— 跑批时用户只关心「现在在干嘛」 */
  useEffect(() => {
    const el = logRef.current
    if (el) el.scrollTop = el.scrollHeight
  }, [status?.lines])

  const start = async () => {
    setErr(null)
    try {
      // 先启动成功再开轮询：反过来的话第一次轮询可能拿到「没有任务」直接收尾
      await aiRunTask(task, picked)
      setBusy(true)
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    }
  }

  const stop = async () => {
    try {
      await aiTaskStop()
    } catch {
      /* 停止失败不阻塞用户：下一次轮询会发现任务已不在 */
    }
    setBusy(false)
  }

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

      <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap', marginBottom: 8 }}>
        {libs.map((l) => {
          const on = picked === l.name
          return (
            <button
              key={l.id}
              type="button"
              className="pv-set-btn"
              style={on ? { borderColor: 'var(--pv-accent)', color: 'var(--pv-accent)' } : undefined}
              disabled={busy}
              onClick={() => setPicked(l.name)}
            >
              {l.name}（{l.count} 张）
            </button>
          )
        })}
        {libs.length === 0 && <div className="pv-set-hint">还没有图库</div>}
      </div>

      <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
        <button className="pv-set-btn" disabled={!picked || busy} onClick={() => void start()}>
          开始
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
    </div>
  )
}
