/**
 * AI 打标 / 人像识别的 IPC 封装（2026-11 从 `manager.ts` 拆出）。
 *
 * 拆出来的理由：这族命令与「图库查询/文件操作」完全无关，只是碰巧同属
 * 管理侧的界面入口。放在一起时 `manager.ts` 828 行里有 120 行是 AI 配置
 * 与任务轮询，找「缩略图怎么取」要滚过一堆阈值参数。
 *
 * ⚠️ 仍然是**纯 `invoke` 包装**：这一层不做重试、不解析业务、不缓存状态，
 * 轮询与去抖都在调用方（`AiRunPanel.tsx` / `AiPipelineDialog.tsx`）。
 * ⚠️ 字段名必须与 Rust 侧 `commands/ai.rs` 的 DTO 一致（`camelCase`）。
 *
 * 对外路径不变：`manager.ts` 末尾把这里的导出再抛一次，
 * 调用方继续 `from '../../ipc/manager'` 即可。
 */

import { invoke } from '@tauri-apps/api/core'

/* ── 打标配置与词表 ───────────────────────────────────────────── */

export interface AiConfig {
  model: string
  vocabCount: number
  disabledCount: number
  /** 二级（具体词）门槛 */
  threshold: number
  /** 一级（类目）门槛，通常低于 `threshold` */
  thresholdL1: number
  maxTags: number
  faces: boolean
  /** 人脸检测置信度：低于它的不当作人脸 */
  faceScoreThresh: number
  /** 脸宽像素下限（解码后）：小脸向量不可信，直接丢 */
  faceMinFacePx: number
  /** 聚类第一阶段阈值：越高越严，人物分得越细 */
  faceTCore: number
  /** 建人物所需的最少张数 */
  faceMinCore: number
  /** 出厂默认值 —— 由后端下发，面板照它标「默认」，前端⛔ 不写字面量 */
  defaults: AiDefaults
}

export interface AiDefaults {
  faceScoreThresh: number
  faceMinFacePx: number
  faceTCore: number
  faceMinCore: number
}

export interface VocabTerm {
  term: string
  l1: string
  l2: string
  syn: string
  en: string
  enabled: boolean
}

export function aiConfig(): Promise<AiConfig> {
  return invoke<AiConfig>('ai_config')
}

export function aiSaveConfig(p: {
  threshold?: number
  thresholdL1?: number
  maxTags?: number
  faces?: boolean
  faceScoreThresh?: number
  faceMinFacePx?: number
  faceTCore?: number
  faceMinCore?: number
}): Promise<void> {
  return invoke<void>('ai_save_config', p)
}

export function aiVocab(): Promise<VocabTerm[]> {
  return invoke<VocabTerm[]>('ai_vocab')
}

export function aiVocabToggle(term: string, enabled: boolean): Promise<void> {
  return invoke<void>('ai_vocab_toggle', { term, enabled })
}

export function aiVocabExport(path: string): Promise<string> {
  return invoke<string>('ai_vocab_export', { path })
}

/** 导入结果：`updated` = 真的改了状态的行数，`unknown` = 词表里查不到的词条。 */
export interface VocabImportResult {
  updated: number
  unknown: string[]
}

/**
 * 从 CSV 导入词表**启用状态**（词条本身是编译期固定的，运行时改不了）。
 *
 * 和 [`aiVocabExport`] 闭环：导出 → 在 Excel 里勾「启用」列 → 导回来。
 * ⚠️ CSV 里没出现的词条保持原状，所以只交一部分也行。
 */
export function aiVocabImport(path: string): Promise<VocabImportResult> {
  return invoke<VocabImportResult>('ai_vocab_import', { path })
}

/* ── 一键任务 ─────────────────────────────────────────────────── */

/** 一键任务的状态快照（每 500ms 轮询）。 */
export interface AiTaskStatus {
  running: boolean
  done: boolean
  ok: boolean
  /** 日志尾部（最多 200 行） */
  lines: string[]
  /** 当前阶段：`tag` / `face`（一键 AI 识别时用来高亮进度行） */
  phase: string
  tagDone: number
  tagTotal: number
  faceDone: number
  faceTotal: number
}

/**
 * 启动 AI 任务。
 *
 * - `tag`      只打标
 * - `face`     只识别人像
 * - `pipeline` 一键：先打标再识别人像（后端串行，进度含两阶段）
 *
 * ⚠️ 现阶段后端 spawn `tools/ai/*.py`，需要本机有 Python。
 * 返回的是日志文件路径（调试用，界面上不显示）。
 */
export function aiRunTask(
  task: 'tag' | 'face' | 'pipeline',
  library: string,
): Promise<string> {
  return invoke<string>('ai_run_task', { task, library })
}

/** 某个范围（空 = 全部图库）的 AI 处理进度，用来决定按钮文案与是否二次确认。 */
export interface LibraryAiStatus {
  /** 打过 AI 标签的图数 */
  tagged: number
  /** 已检测出人脸的图数 */
  faced: number
  /** 范围内总图数 */
  total: number
}

export function aiLibraryStatus(library: string): Promise<LibraryAiStatus> {
  return invoke<LibraryAiStatus>('ai_library_status', { library })
}

export function aiTaskStatus(): Promise<AiTaskStatus> {
  return invoke<AiTaskStatus>('ai_task_status')
}

export function aiTaskStop(): Promise<void> {
  return invoke<void>('ai_task_stop')
}
