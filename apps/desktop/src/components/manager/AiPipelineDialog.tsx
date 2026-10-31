/**
 * 一键 AI 识别：先打标、再识别人像，后端串行跑完，这里只显示两行进度。
 *
 * ## 为什么做成一个弹层而不是塞进设置页
 * 用户在图库页看到「这个库还是空的（没标签、没人像）」，最自然的动作是
 * **就地点一下**，而不是「打开设置 → 找到 AI 分析 → 选图库 → 点两个按钮」。
 *
 * ## 进度从哪来
 * 后端 `run_pipeline_blocking` 把两阶段的计数写进同一个 `TagProgress`
 * （`tag_done/tag_total`、`face_done/face_total`），`ai_task_status` 一次返回。
 * 前端只轮询一个接口，不用自己编排两段任务。
 */

import { useEffect, useRef, useState } from 'react'

import { aiRunTask, aiTaskStatus, aiTaskStop, type AiTaskStatus } from '../../ipc/manager'
import { errText } from '../../lib/errText'

interface Props {
  /** 图库名（后端按名字匹配；空字符串表示全部图库） */
  library: string
  /** 关掉弹层（任务继续在后台跑） */
  onClose: () => void
  /** 整体结束（成功或失败）→ 调用方刷新图库 */
  onDone: () => void
}

function Bar({
  label,
  done,
  total,
  active,
  failed,
}: {
  label: string
  done: number
  total: number
  active: boolean
  failed: boolean
}) {
  const pct = total > 0 ? Math.min(100, (done / total) * 100) : 0
  return (
    <div style={{ marginBottom: 10 }}>
      <div
        style={{
          display: 'flex',
          justifyContent: 'space-between',
          fontSize: 12,
          marginBottom: 4,
          color: active ? 'var(--pv-accent)' : undefined,
          fontWeight: active ? 600 : 400,
        }}
      >
        <span>{label}</span>
        <span style={{ opacity: active ? 1 : 0.6 }}>
          {total > 0 ? `${done} / ${total}` : active ? '准备中…' : '等待中'}
        </span>
      </div>
      <div
        style={{
          height: 6,
          borderRadius: 3,
          background: 'var(--pv-border)',
          overflow: 'hidden',
        }}
      >
        <div
          style={{
            height: '100%',
            width: `${pct}%`,
            background: failed ? '#f09595' : active ? 'var(--pv-accent)' : 'var(--pv-text-dim)',
            transition: 'width .3s ease-out',
          }}
        />
      </div>
    </div>
  )
}

export function AiPipelineDialog({ library, onClose, onDone }: Props) {
  const [st, setSt] = useState<AiTaskStatus | null>(null)
  const [busy, setBusy] = useState(true)
  const [err, setErr] = useState<string | null>(null)
  const logRef = useRef<HTMLPreElement>(null)

  /* 挂上就开始跑 */
  useEffect(() => {
    let alive = true
    void aiRunTask('pipeline', library).catch((e: unknown) => {
      if (alive) {
        setErr(errText(e))
        setBusy(false)
      }
    })
    return () => {
      alive = false
    }
  }, [library])

  /* 轮询状态 */
  useEffect(() => {
    if (!busy) return
    let alive = true
    const tick = async () => {
      try {
        const s = await aiTaskStatus()
        if (!alive) return
        setSt(s)
        if (s.done) {
          setBusy(false)
          onDone()
          return
        }
        window.setTimeout(tick, 500)
      } catch (e) {
        if (alive) {
          setErr(errText(e))
          setBusy(false)
        }
      }
    }
    void tick()
    return () => {
      alive = false
    }
  }, [busy, onDone])

  /* 日志跟到底 */
  useEffect(() => {
    const el = logRef.current
    if (el) el.scrollTop = el.scrollHeight
  }, [st?.lines])

  const phase = st?.phase ?? 'tag'
  const finished = !busy
  const failed = finished && st !== null && !st.ok

  return (
    <div className="pv-modal-backdrop" role="dialog" aria-modal="true" aria-label="AI 识别">
      <div className="pv-modal" style={{ width: 460 }}>
        <div className="pv-modal-head">
          <div className="pv-modal-title">
            {busy ? 'AI 识别中…' : failed ? 'AI 识别失败' : 'AI 识别完成'}
          </div>
          <button type="button" className="pv-btn" onClick={onClose}>
            后台运行
          </button>
        </div>

        <div style={{ padding: '4px 0 10px' }}>
          <div className="pv-set-hint" style={{ marginBottom: 10 }}>
            范围：{library || '全部图库'}
          </div>

          <Bar
            label="① 打标（CN-CLIP）"
            done={st?.tagDone ?? 0}
            total={st?.tagTotal ?? 0}
            active={busy && phase === 'tag'}
            failed={failed && phase === 'tag'}
          />
          <Bar
            label="② 识别人像（YuNet + SFace）"
            done={st?.faceDone ?? 0}
            total={st?.faceTotal ?? 0}
            active={busy && phase === 'face'}
            failed={failed && phase === 'face'}
          />
        </div>

        {st && st.lines.length > 0 && (
          <pre className="pv-set-log" ref={logRef} style={{ maxHeight: 160 }}>
            {st.lines.join('\n')}
          </pre>
        )}
        {err && (
          <div className="pv-set-hint" style={{ color: '#f09595' }}>
            {err}
          </div>
        )}

        <div className="pv-set-btnrow" style={{ marginTop: 10 }}>
          {busy ? (
            <button
              className="pv-set-btn pv-set-btn--danger"
              onClick={() => void aiTaskStop().catch(() => undefined)}
            >
              停止
            </button>
          ) : (
            <button className="pv-set-btn" onClick={onClose}>
              关闭
            </button>
          )}
          <span className="pv-set-hint">
            两阶段自动串行，关掉这个窗口也会继续跑（进度看设置 → AI 分析）
          </span>
        </div>
      </div>
    </div>
  )
}
