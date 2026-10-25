/**
 * 设置窗口：左分类树 + 右内容（形态对齐 SumatraPDF 的设置窗口）。
 *
 * ## 为什么是窗口而不是菜单
 * 设置项里既有布尔开关，也有连续量（阈值 0.30–0.60、停留 0.5–5 秒），
 * 菜单只能塞离散项；而且**词表 111 条**根本没有菜单形态能容纳。
 *
 * ## 状态来源（不新建第二份真相）
 * - 打开方式：`App` 传进来的 `associations` + 回调（写注册表）
 * - 排序 / 幻灯片：`useManager`、`useSlideshow` 里已有的 pref 读写函数（localStorage）
 * - 缩略图尺寸：`useManager` 的 `readThumbTier/saveThumbTier`（2026-10-24 新增持久化）
 * - AI 阈值/张数/人脸/词表：后端 `ai.*` 键（`ipc/manager.ts`），与图库齿轮共用
 *
 * ## 交互约定
 * **改动即时生效并自动保存**，没有「确定/取消」—— 底栏写明。
 * 危险操作（清空人脸）用**行内两段确认**（按钮变「确认清空」，4 秒后复原），
 * 不弹对话框：设置窗口里再叠一层模态，容易点错。
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { ASSOCIATION_FORMATS } from '../../constants/formats'
import type { AssociationState } from '../../ipc'
import { save as saveDialog } from '@tauri-apps/plugin-dialog'
import {
  aiConfig,
  aiSaveConfig,
  aiVocab,
  aiVocabExport,
  aiVocabToggle,
  managerLibraries,
  appDataDir,
  managerCacheStats,
  managerClearCache,
  type LibraryItem,
  revealInExplorer,
  type AiConfig,
  type CacheStats,
  type ManagerSort,
  type VocabTerm,
} from '../../ipc/manager'
import { managerFacesClear, managerFacesStats, type FacesStats } from '../../ipc/people'
import {
  FX_MS_MAX,
  FX_MS_MIN,
  FX_MS_STEP,
  SLIDE_FX_OPTIONS,
  applyFxMs,
  applySlideFx,
  readFxMs,
  readSlideFx,
  readSlideSeconds,
  saveSlideSeconds,
  type SlideFx,
} from '../../hooks/useSlideshow'
import {
  applySortPref,
  readSortPref,
  readThumbTier,
  saveThumbTier,
  THUMB_TIERS,
} from '../../hooks/useManager'
import type { ThumbTier } from '../../ipc/manager'

/* ── 页面表：组分隔，组内页按顺序 ─────────────────────────── */
const PAGES = [
  { id: 'data', group: '常规', label: '数据位置' },
  { id: 'thumb', group: '外观', label: '缩略图' },
  { id: 'sort', group: '浏览', label: '排序' },
  { id: 'slide', group: '浏览', label: '幻灯片' },
  { id: 'assoc', group: '文件类型', label: '打开方式' },
  { id: 'ai', group: 'AI 分析', label: '打标与人脸' },
  { id: 'vocab', group: 'AI 分析', label: '词表' },
  { id: 'faces', group: '隐私与数据', label: '人脸数据' },
  { id: 'cache', group: '隐私与数据', label: '缓存与目录' },
  { id: 'about', group: '关于', label: '关于' },
] as const

type PageId = (typeof PAGES)[number]['id']
/** 页面 id（`App` 用它决定从别处跳进来落到哪一页） */
export type SettingsPage = PageId

const SORT_CHOICES: Array<{ value: ManagerSort; label: string }> = [
  { value: 'taken', label: '拍摄时间' },
  { value: 'name', label: '文件名' },
  { value: 'size', label: '文件大小' },
  { value: 'mtime', label: '修改时间' },
]

export interface SettingsWindowProps {
  onClose: () => void
  associations: AssociationState | null
  onToggleAssociation: (ext: string, label: string, enable: boolean) => void
  onToggleAllAssociations: (enable: boolean) => void
  associationBusy: boolean
  associationError: string | null
  /** 直接打开某页（图库标题右侧的齿轮用它跳到 AI 页） */
  initialPage?: SettingsPage
}

export function SettingsWindow({
  onClose,
  associations,
  onToggleAssociation,
  onToggleAllAssociations,
  associationBusy,
  associationError,
  initialPage = 'data',
}: SettingsWindowProps) {
  const [page, setPage] = useState<PageId>(initialPage)
  const [query, setQuery] = useState('')

  /**
   * 搜索：不是"过滤右栏"，而是**过滤左栏 + 点结果跳页**。
   * 设置项分布在不同页里，直接把命中的页列出来比改右栏更容易找到东西。
   */
  const visible = useMemo(() => {
    const q = query.trim().toLowerCase()
    if (!q) return PAGES
    return PAGES.filter(
      (p) => p.label.toLowerCase().includes(q) || p.group.toLowerCase().includes(q),
    )
  }, [query])

  /* Esc 关闭（App 的快捷键链也会关，这里兜一层：焦点在输入框里时也能关） */
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation()
        onClose()
      }
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [onClose])

  return (
    <div
      className="pv-set-overlay"
      onClick={onClose}
      role="dialog"
      aria-modal="true"
      aria-label="设置"
    >
      <div className="pv-set-window" onClick={(e) => e.stopPropagation()}>
        <div className="pv-set-head">
          <span className="pv-set-head-title">设置</span>
          <span className="pv-set-head-spacer" />
          <button className="pv-set-close" onClick={onClose} aria-label="关闭设置" title="关闭 (Esc)">
            ✕
          </button>
        </div>

        <div className="pv-set-body">
          <nav className="pv-set-nav" aria-label="设置分类">
            <input
              className="pv-set-search"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="搜索设置"
              aria-label="搜索设置"
            />
            {visible.map((p, i) => {
              const first = i === 0 || visible[i - 1]?.group !== p.group
              return (
                <div key={p.id}>
                  {first && <div className="pv-set-group">{p.group}</div>}
                  <button
                    className={`pv-set-item${page === p.id ? ' is-on' : ''}`}
                    onClick={() => setPage(p.id)}
                    aria-current={page === p.id}
                  >
                    {p.label}
                  </button>
                </div>
              )
            })}
            {visible.length === 0 && <div className="pv-set-hint">没有匹配的设置项</div>}
          </nav>

          <div className="pv-set-main">
            {page === 'data' && <DataPage />}
            {page === 'thumb' && <ThumbPage />}
            {page === 'sort' && <SortPage />}
            {page === 'slide' && <SlidePage />}
            {page === 'assoc' && (
              <AssocPage
                associations={associations}
                onToggle={onToggleAssociation}
                onToggleAll={onToggleAllAssociations}
                busy={associationBusy}
                error={associationError}
              />
            )}
            {page === 'ai' && <AiPage />}
            {page === 'vocab' && <VocabPage />}
            {page === 'faces' && <FacesPage />}
            {page === 'cache' && <CachePage />}
            {page === 'about' && <AboutPage />}
          </div>
        </div>

        <div className="pv-set-foot">
          改动即时生效并自动保存
          <span className="pv-set-head-spacer" />
          <span>Esc 关闭</span>
        </div>
      </div>
    </div>
  )
}

/* ════════════════════════════════════════════════════════════
   通用控件（全部自绘：不用原生 select / checkbox）
   ════════════════════════════════════════════════════════════ */

function Row({ label, hint, children }: { label: string; hint?: string; children: React.ReactNode }) {
  return (
    <div className="pv-set-row">
      <div className="pv-set-label">{label}</div>
      {children}
      {hint && <div className="pv-set-hint">{hint}</div>}
    </div>
  )
}

/** 单选行：左侧 ✓ 表示当前值（视觉上与菜单/筛选下拉一致） */
function Choice({ on, label, onClick }: { on: boolean; label: string; onClick: () => void }) {
  return (
    <button
      className={`pv-set-item${on ? ' is-on' : ''}`}
      style={{ paddingLeft: 6, width: '100%' }}
      onClick={onClick}
      role="radio"
      aria-checked={on}
    >
      <span style={{ width: 14, fontSize: 11, color: on ? 'var(--pv-accent)' : 'transparent' }}>
        ✓
      </span>
      <span style={{ marginLeft: 2 }}>{label}</span>
    </button>
  )
}

function Slider({
  value,
  min,
  max,
  step,
  onInput,
  onCommit,
  format,
}: {
  value: number
  min: number
  max: number
  step: number
  onInput: (v: number) => void
  onCommit: (v: number) => void
  format: (v: number) => string
}) {
  return (
    <div className="pv-set-slider-wrap">
      <input
        className="pv-set-slider"
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(e) => onInput(Number(e.target.value))}
        onPointerUp={(e) => onCommit(Number((e.target as HTMLInputElement).value))}
        onKeyUp={(e) => onCommit(Number((e.target as HTMLInputElement).value))}
      />
      <span className="pv-set-value">{format(value)}</span>
    </div>
  )
}

function Switch({ on, onChange, label }: { on: boolean; onChange: (v: boolean) => void; label?: string }) {
  return (
    <div className="pv-set-switch-row">
      <button
        className={`pv-set-switch${on ? ' is-on' : ''}`}
        onClick={() => onChange(!on)}
        role="switch"
        aria-checked={on}
        aria-label={label ?? '开关'}
      />
      <span style={{ fontSize: 12, color: 'var(--pv-text-primary)' }}>{on ? '开' : '关'}</span>
    </div>
  )
}

/* ════════════════════════════════════════════════════════════
   各页
   ════════════════════════════════════════════════════════════ */

function DataPage() {
  const [dir, setDir] = useState<string>('')
  useEffect(() => {
    void appDataDir()
      .then(setDir)
      .catch(() => setDir(''))
  }, [])
  return (
    <>
      <h2 className="pv-set-h1">数据位置</h2>
      <Row label="图库索引与缓存" hint="人脸向量、缩略图缓存都在这个目录里；删掉等于重置图库（原图不受影响）">
        <div className="pv-set-info">
          <span style={{ flex: 1, wordBreak: 'break-all' }}>{dir || '读取中…'}</span>
        </div>
      </Row>
      <div className="pv-set-btnrow">
        <button
          className="pv-set-btn"
          disabled={!dir}
          onClick={() => void revealInExplorer(dir).catch(() => undefined)}
        >
          在资源管理器中打开
        </button>
      </div>
      <Row
        label="日志级别"
        hint="默认 warn。排查问题时用环境变量 PHOTOVAULT_LOG=info 启动，日志在 stderr。"
      >
        <div className="pv-set-info">warn（默认）</div>
      </Row>
    </>
  )
}

function ThumbPage() {
  const [tier, setTier] = useState<ThumbTier>(readThumbTier)
  const pick = (t: ThumbTier) => {
    setTier(t)
    saveThumbTier(t)
  }
  return (
    <>
      <h2 className="pv-set-h1">缩略图</h2>
      <Row label="尺寸" hint="只影响图库网格的显示；改完立即生效并记住">
        {THUMB_TIERS.map((t) => (
          <Choice key={t.value} on={tier === t.value} label={`${t.label}（${t.px}px）`} onClick={() => pick(t.value)} />
        ))}
      </Row>
    </>
  )
}

function SortPage() {
  const [sort, setSort] = useState<ManagerSort>(readSortPref)
  const pick = (s: ManagerSort) => {
    setSort(s)
    applySortPref(s)
  }
  return (
    <>
      <h2 className="pv-set-h1">排序</h2>
      <Row label="默认排序" hint="图库网格与单图翻页都按这个顺序">
        {SORT_CHOICES.map((c) => (
          <Choice key={c.value} on={sort === c.value} label={c.label} onClick={() => pick(c.value)} />
        ))}
      </Row>
    </>
  )
}

function SlidePage() {
  const [sec, setSec] = useState<number>(readSlideSeconds)
  const [fx, setFx] = useState<SlideFx>(readSlideFx)
  const [ms, setMs] = useState<number>(readFxMs)
  return (
    <>
      <h2 className="pv-set-h1">幻灯片</h2>
      <Row label="每张停留" hint="播放幻灯片时每张图停留的秒数">
        <Slider
          value={sec}
          min={0.5}
          max={5}
          step={0.5}
          onInput={setSec}
          onCommit={saveSlideSeconds}
          format={(v) => `${v.toFixed(1)}s`}
        />
      </Row>
      <Row label="切换特效">
        {SLIDE_FX_OPTIONS.map((o) => (
          <Choice
            key={o.value}
            on={fx === o.value}
            label={o.label}
            onClick={() => {
              setFx(o.value)
              applySlideFx(o.value)
            }}
          />
        ))}
      </Row>
      <Row label="特效时长" hint={`${FX_MS_MIN}–${FX_MS_MAX} ms，两种特效共用`}>
        <Slider
          value={ms}
          min={FX_MS_MIN}
          max={FX_MS_MAX}
          step={FX_MS_STEP}
          onInput={setMs}
          onCommit={applyFxMs}
          format={(v) => `${v}ms`}
        />
      </Row>
    </>
  )
}

function AssocPage({
  associations,
  onToggle,
  onToggleAll,
  busy,
  error,
}: {
  associations: AssociationState | null
  onToggle: (ext: string, label: string, enable: boolean) => void
  onToggleAll: (enable: boolean) => void
  busy: boolean
  error: string | null
}) {
  const registered = useMemo(() => new Set(associations?.registered ?? []), [associations])
  const total = ASSOCIATION_FORMATS.length
  const checked = ASSOCIATION_FORMATS.filter(([ext]) => registered.has(ext)).length
  const disabled = busy || associations === null
  return (
    <>
      <h2 className="pv-set-h1">打开方式</h2>
      <Row
        label={`已加入 ${checked} / ${total} 个格式`}
        hint="勾选后双击这些格式的文件会用本程序打开；这只是把本程序加进「打开方式」列表，不是把系统默认程序改掉。"
      >
        <div className="pv-set-grid2">
          {ASSOCIATION_FORMATS.map(([ext, label]) => {
            const on = registered.has(ext)
            return (
              <div className={`pv-set-check-row${on ? '' : ' is-off'}`} key={ext}>
                <button
                  className={`pv-set-check${on ? ' is-on' : ''}`}
                  disabled={disabled}
                  onClick={() => onToggle(ext, label, !on)}
                  role="checkbox"
                  aria-checked={on}
                  aria-label={label}
                >
                  ✓
                </button>
                <span>{label}</span>
              </div>
            )
          })}
        </div>
      </Row>
      <div className="pv-set-btnrow">
        <button className="pv-set-btn" disabled={disabled} onClick={() => onToggleAll(true)}>
          全选
        </button>
        <button className="pv-set-btn" disabled={disabled} onClick={() => onToggleAll(false)}>
          全不选
        </button>
        {busy && <span className="pv-set-hint">正在写入注册表…</span>}
      </div>
      {error && <div className="pv-set-hint" style={{ color: '#f09595' }}>{error}</div>}
      {associations?.note && <div className="pv-set-hint">{associations.note}</div>}
    </>
  )
}

function AiPage() {
  const [cfg, setCfg] = useState<AiConfig | null>(null)
  const [threshold, setThreshold] = useState(0.4)
  const [maxTags, setMaxTags] = useState(6)
  const timer = useRef<number | undefined>(undefined)

  useEffect(() => {
    void aiConfig()
      .then((c) => {
        setCfg(c)
        setThreshold(c.threshold)
        setMaxTags(c.maxTags)
      })
      .catch(() => undefined)
  }, [])

  /** 滑块拖动结束才写库：拖动过程中每帧都写会把 SQLite 打满 */
  const commitThreshold = (v: number) => {
    setThreshold(v)
    void aiSaveConfig({ threshold: v }).catch(() => undefined)
  }
  const pickMax = (n: number) => {
    setMaxTags(n)
    void aiSaveConfig({ maxTags: n }).catch(() => undefined)
  }
  const setFaces = (on: boolean) => {
    setCfg((c) => (c ? { ...c, faces: on } : c))
    void aiSaveConfig({ faces: on }).catch(() => undefined)
  }

  useEffect(() => () => window.clearTimeout(timer.current), [])

  return (
    <>
      <h2 className="pv-set-h1">打标与人脸</h2>
      {!cfg ? (
        <div className="pv-set-hint">读取中…</div>
      ) : (
        <>
          <Row label="置信度阈值" hint="调高更严：只保留把握大的标签，宁少勿错。改完需要「重新分析」才会应用到已打的标签。">
            <Slider
              value={threshold}
              min={0.3}
              max={0.6}
              step={0.01}
              onInput={setThreshold}
              onCommit={commitThreshold}
              format={(v) => v.toFixed(2)}
            />
          </Row>
          <Row label="每张最多标签">
            {[3, 5, 8].map((n) => (
              <Choice key={n} on={maxTags === n} label={`${n} 个`} onClick={() => pickMax(n)} />
            ))}
          </Row>
          <Row label="人脸识别" hint="只在本机运行，不上传；embedding 属于生物特征，可随时在「人脸数据」里清空">
            <Switch on={cfg.faces} onChange={setFaces} label="人脸识别" />
          </Row>
          <div className="pv-set-info">
            模型 {cfg.model} · 词表 {cfg.vocabCount} 条（停用 {cfg.disabledCount}）
          </div>
          <FaceRerun />
        </>
      )}
    </>
  )
}

/**
 * 人像重新识别（2026-10-25 用户要求：从图库标题行搬进设置，且能选图库）。
 *
 * ⚠️ 识别由本地 Python 脚本执行（`tools/ai/face_probe.py` + `face_cluster.py`），
 * 这里只负责"选目标图库 + 生成可直接粘贴的命令"，不假装能一键跑完 ——
 * 一句"已重新识别"然后什么都没发生，比给命令更糟。
 */
function FaceRerun() {
  const [libs, setLibs] = useState<LibraryItem[]>([])
  const [picked, setPicked] = useState<string>('')

  useEffect(() => {
    void managerLibraries()
      .then((list) => {
        setLibs(list)
        const first = list[0]
        if (first) setPicked(first.name)
      })
      .catch(() => undefined)
  }, [])

  const cmd =
    `J:/ai/tts/.venv/Scripts/python.exe tools/ai/face_probe.py --library "${picked}" ` +
    `--long-side 2048 --min-face 56 --only-person-tags --write-db
` +
    `J:/ai/tts/.venv/Scripts/python.exe tools/ai/face_cluster.py --reset`

  return (
    <div className="pv-set-row" style={{ marginTop: 18 }}>
      <div className="pv-set-label">重新识别人像</div>
      <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
        <div style={{ minWidth: 220 }}>
          {libs.map((l) => (
            <Choice key={l.id} on={picked === l.name} label={`${l.name}（${l.count} 张）`} onClick={() => setPicked(l.name)} />
          ))}
          {libs.length === 0 && <div className="pv-set-hint">还没有图库</div>}
        </div>
        <button
          className="pv-set-btn"
          disabled={!picked}
          onClick={() => void navigator.clipboard.writeText(cmd).catch(() => undefined)}
        >
          复制识别命令
        </button>
      </div>
      <div className="pv-set-hint">
        识别在本机脚本里跑（不进主进程）：复制上面的命令到项目目录执行即可。
        只处理带「人像 / 证件」标签的图，同一张图里同一个人只算一次。
      </div>
    </div>
  )
}

function VocabPage() {
  const [terms, setTerms] = useState<VocabTerm[] | null>(null)
  const [q, setQ] = useState('')
  useEffect(() => {
    void aiVocab()
      .then(setTerms)
      .catch(() => setTerms([]))
  }, [])

  const list = useMemo(() => {
    if (!terms) return []
    const kw = q.trim()
    if (!kw) return terms
    return terms.filter((t) => t.term.includes(kw) || t.l2.includes(kw) || t.syn.includes(kw))
  }, [terms, q])

  const onCount = terms?.filter((t) => t.enabled).length ?? 0
  const [exportMsg, setExportMsg] = useState<string | null>(null)

  /**
   * 导出词表：走**系统保存对话框**（不能自己拼路径 —— 用户要能选目录）。
   * 旧的那个 AI 弹层里有这个入口，搬到设置窗口后必须补上，否则功能丢一块。
   */
  const exportVocab = async () => {
    try {
      const path = await saveDialog({
        title: '导出主题词表',
        defaultPath: 'photo_tags.csv',
        filters: [{ name: 'CSV', extensions: ['csv'] }],
      })
      if (!path) return
      await aiVocabExport(path)
      setExportMsg(`已导出：${path}`)
    } catch (e) {
      setExportMsg(`导出失败：${e instanceof Error ? e.message : String(e)}`)
    }
  }

  const toggle = (t: VocabTerm) => {
    const next = !t.enabled
    setTerms((prev) => prev?.map((x) => (x.term === t.term ? { ...x, enabled: next } : x)) ?? prev)
    void aiVocabToggle(t.term, next).catch(() => undefined)
  }

  return (
    <>
      <h2 className="pv-set-h1">词表</h2>
      <Row
        label={`${terms?.length ?? 0} 条 · 启用 ${onCount}`}
        hint="停用的词不会参与打标；改完同样要重新分析才会影响已打的标签。"
      >
        <input
          className="pv-set-search"
          style={{ width: '100%', marginBottom: 8 }}
          value={q}
          onChange={(e) => setQ(e.target.value)}
          placeholder="搜索词条 / 同义词"
        />
        <div className="pv-set-vocab">
          {list.map((t) => (
            <div className={`pv-set-vocab-row${t.enabled ? '' : ' is-off'}`} key={t.term}>
              <button
                className={`pv-set-check${t.enabled ? ' is-on' : ''}`}
                onClick={() => toggle(t)}
                role="checkbox"
                aria-checked={t.enabled}
                aria-label={t.term}
              >
                ✓
              </button>
              <span className="pv-set-vocab-l1">{t.l1}</span>
              <span style={{ flex: 1 }}>{t.term}</span>
            </div>
          ))}
          {list.length === 0 && <div className="pv-set-hint">没有匹配的词条</div>}
        </div>
      </Row>
      <div className="pv-set-btnrow">
        <button className="pv-set-btn" onClick={() => void exportVocab()}>
          导出词表…
        </button>
        <span className="pv-set-hint">导出为 CSV，可在 Excel 里改完再导回来</span>
      </div>
      {exportMsg && <div className="pv-set-hint">{exportMsg}</div>}
    </>
  )
}

function FacesPage() {
  const [stats, setStats] = useState<FacesStats | null>(null)
  const [confirming, setConfirming] = useState(false)
  const [msg, setMsg] = useState<string | null>(null)

  const load = useCallback(() => {
    void managerFacesStats()
      .then(setStats)
      .catch(() => setStats(null))
  }, [])
  useEffect(load, [load])

  /* 两段确认：点了「清空」先变成「确认清空」，4 秒不点就复原 */
  useEffect(() => {
    if (!confirming) return
    const t = window.setTimeout(() => setConfirming(false), 4000)
    return () => window.clearTimeout(t)
  }, [confirming])

  const clear = () => {
    if (!confirming) {
      setConfirming(true)
      return
    }
    setConfirming(false)
    void managerFacesClear()
      .then((n) => {
        setMsg(`已删除 ${n} 张人脸与全部人物`)
        load()
      })
      .catch((e: unknown) => setMsg(String(e)))
  }

  return (
    <>
      <h2 className="pv-set-h1">人脸数据</h2>
      <Row label="当前状态" hint="这些是人脸识别的**结果**，不在照片文件里，也不会被导出">
        <div className="pv-set-info">
          <span style={{ flex: 1 }}>
            已分析 <strong>{stats?.faces ?? 0}</strong> 张脸 · 已归属{' '}
            <strong>{stats?.assigned ?? 0}</strong> · <strong>{stats?.persons ?? 0}</strong> 个人物（
            {stats?.namedPersons ?? 0} 个已命名）
          </span>
        </div>
      </Row>
      <div className="pv-set-btnrow">
        <button className="pv-set-btn pv-set-btn--danger" onClick={clear}>
          {confirming ? '确认清空人脸数据' : '清空人脸数据…'}
        </button>
        <span className="pv-set-hint">只删除人脸与人物，照片、标签、缓存都不受影响</span>
      </div>
      {msg && <div className="pv-set-hint">{msg}</div>}
    </>
  )
}

function CachePage() {
  const [stats, setStats] = useState<CacheStats | null>(null)
  const [busy, setBusy] = useState(false)
  const [msg, setMsg] = useState<string | null>(null)
  const [dir, setDir] = useState('')

  useEffect(() => {
    void managerCacheStats()
      .then(setStats)
      .catch(() => undefined)
    void appDataDir()
      .then(setDir)
      .catch(() => undefined)
  }, [])

  const clear = () => {
    setBusy(true)
    setMsg(null)
    void managerClearCache()
      .then((s) => {
        setStats(s)
        setMsg('缓存已清理，下次浏览会重新生成')
      })
      .catch((e: unknown) => setMsg(String(e)))
      .finally(() => setBusy(false))
  }

  return (
    <>
      <h2 className="pv-set-h1">缓存与目录</h2>
      <Row label="缩略图缓存" hint="清理后不会丢任何照片，只是下次打开要重新生成缩略图">
        <div className="pv-set-info">
          <span style={{ flex: 1 }}>
            {stats
              ? `${stats.files.toLocaleString()} 个文件 · ${(stats.bytes / 1024 / 1024).toFixed(1)} MB`
              : '读取中…'}
          </span>
        </div>
      </Row>
      <div className="pv-set-btnrow">
        <button className="pv-set-btn" disabled={busy} onClick={clear}>
          {busy ? '清理中…' : '清理缓存…'}
        </button>
        <button
          className="pv-set-btn"
          disabled={!dir}
          onClick={() => void revealInExplorer(dir).catch(() => undefined)}
        >
          打开数据目录
        </button>
      </div>
      {msg && <div className="pv-set-hint">{msg}</div>}
      <Row label="目录" hint="索引库 library.db 与人脸向量都在这里">
        <div className="pv-set-info">
          <span style={{ flex: 1, wordBreak: 'break-all' }}>{dir || '…'}</span>
        </div>
      </Row>
    </>
  )
}

function AboutPage() {
  return (
    <>
      <h2 className="pv-set-h1">关于</h2>
      <Row label="软件">
        <div className="pv-set-info">PurePhotoExplorer · 图库模式（S2）· 内部代号 PhotoVault</div>
      </Row>
      <Row label="本地 AI 模型">
        <div className="pv-set-info" style={{ display: 'block', lineHeight: 1.7 }}>
          打标：CN-CLIP ViT-B/16（中文原生）
          <br />
          人脸：YuNet（MIT）+ SFace（Apache-2.0）
          <br />
          全部在本机推理，**不联网、不上传**
        </div>
      </Row>
      <Row label="提示">
        <div className="pv-set-info" style={{ display: 'block', lineHeight: 1.7 }}>
          按 H 或 F1 打开快捷键速查；右键缩略图可批量操作；F5 重新扫描当前范围。
        </div>
      </Row>
    </>
  )
}
