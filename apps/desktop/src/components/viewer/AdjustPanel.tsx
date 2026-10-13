/**
 * 画布调整浮层（右上角）。
 *
 * 三组控件：
 * 1. **角度微调**（-45° ~ +45°）：滚动条 + 数值，实时作用于画布；
 * 2. **画布比例**：自由 / 1:1 / 3:4 / 4:3 / 16:9 / 9:16 / 2.35:1 / 2:3（两行 4 列）。
 *    画面上**始终有一个裁剪框**（2026-10-08 统一两种模式）：
 *    自由 → 预置框 = 图像可见矩形（初始即整图）；固定比例 → 预置框 = 视口内
 *    该比例的最大矩形。**拖四角改大小**（固定比例锁死宽高比）、**拖框内部平移**，
 *    全程画面不动；切比例会立刻重算刷新画面。
 * 3. **旋转**：顺时针 / 逆时针 90°；
 *
 * 底部是「复位」与「覆盖原图」。「复位」会把**旋转**、角度、比例与框
 * 一并清掉并回到整图适应 —— 因此它的可用性判断必须包含 `rotation`。
 *
 * ⚠️ **没拖过框时 `crop` 为 `null`**（= 不裁剪）。所以「只放大看细节、没动过框」
 * 不会把可见区域误存成裁剪区；固定比例下后端拿到 `null` 会按 `aspect` 居中裁剪，
 * 与居中的预置框一致。
 *
 * ## 两个角度维度的分工
 * - `rotation`（本面板的 90° 按钮、工具栏的左转/右转）：**会**写回原文件
 * - `fineAngle`（滑块）：**只在预览生效**，不写回
 *
 * 原因是任意角度的重采样会让「预览所见 ≠ 文件所得」，且叠加二次采样损失。
 * 保存时若 `fineAngle ≠ 0`，面板会给出一句提示。
 */

import type { AspectRatio, CropRect } from '../../hooks/useImageView'
import { MAX_FINE_ANGLE, MIN_FINE_ANGLE } from '../../hooks/useImageView'

/** 比例选项（顺序即展示顺序；8 项由 CSS 排成两行 4 列） */
const ASPECT_OPTIONS: Array<{ value: AspectRatio; label: string }> = [
  { value: 'free', label: '自由' },
  { value: '1:1', label: '1:1' },
  { value: '3:4', label: '3:4' },
  { value: '4:3', label: '4:3' },
  { value: '16:9', label: '16:9' },
  { value: '9:16', label: '9:16' },
  { value: '2.35:1', label: '2.35:1' },
  { value: '2:3', label: '2:3' },
]

/** 保存状态：三态由父组件驱动 */
export type SaveState =
  | { kind: 'idle' }
  | { kind: 'saving' }
  | { kind: 'ok'; message: string }
  | { kind: 'error'; message: string }

interface AdjustPanelProps {
  /** 当前 90° 粗旋转角度（0/90/180/270） */
  rotation: number
  /** 当前细调角度（度） */
  fineAngle: number
  /** 当前比例 */
  aspect: AspectRatio
  /** 当前自由框选区域；未框选时为 null */
  crop: CropRect | null
  onFineAngle: (deg: number) => void
  onAspect: (a: AspectRatio) => void
  /** 清掉自由框选（保留角度与比例） */
  onClearCrop: () => void
  /** 顺时针 90° */
  onRotateCw: () => void
  /** 逆时针 90° */
  onRotateCcw: () => void
  onReset: () => void
  /** 覆盖写回原文件（仅可覆盖的格式可用） */
  onSave: () => void
  /** 另存为新文件（RAW 等不可覆盖的格式走这条） */
  onSaveAs: () => void
  /** 保存状态（由父组件持有，便于页面上其他位置也能感知） */
  saveState: SaveState
  /** 当前文件是否支持**覆盖**写回原文件（RAW 等为false） */
  writable: boolean
  /** 不支持覆盖时的原因（鼠标悬停可见） */
  notWritableReason?: string
  onClose: () => void
}

export function AdjustPanel({
  rotation,
  fineAngle,
  aspect,
  crop,
  onFineAngle,
  onAspect,
  onClearCrop,
  onRotateCw,
  onRotateCcw,
  onReset,
  onSave,
  onSaveAs,
  saveState,
  writable,
  notWritableReason,
  onClose,
}: AdjustPanelProps) {
  // ⚠️ 必须把 rotation 也算进来：否则「顺时针 90°」之后按钮仍是禁用态，
  // 用户看到的画面明明转了却点不动复位（2026-10-07 用户报的 bug）。
  const isDefault = rotation === 0 && fineAngle === 0 && aspect === 'free' && crop === null
  const saving = saveState.kind === 'saving'

  return (
    <aside className="pv-adjust" aria-label="画布调整">
      <div className="pv-adjust-head">
        <span className="pv-adjust-title">画布调整</span>
        <button
          className="pv-adjust-close"
          onClick={onClose}
          aria-label="关闭画布调整"
          title="关闭"
        >
          ×
        </button>
      </div>

      {/* ── 角度 ── */}
      <div className="pv-adjust-group">
        <div className="pv-adjust-label-row">
          <span className="pv-adjust-label">角度</span>
          <span className="pv-adjust-value" data-zero={fineAngle === 0}>
            {fineAngle > 0 ? `+${fineAngle}` : fineAngle}°
          </span>
        </div>
        <input
          className="pv-adjust-slider"
          type="range"
          min={MIN_FINE_ANGLE}
          max={MAX_FINE_ANGLE}
          step={1}
          value={fineAngle}
          onChange={(e) => onFineAngle(Number(e.target.value))}
          aria-label="角度微调"
          aria-valuemin={MIN_FINE_ANGLE}
          aria-valuemax={MAX_FINE_ANGLE}
          aria-valuenow={fineAngle}
        />
        <div className="pv-adjust-ticks" aria-hidden>
          <span>{MIN_FINE_ANGLE}°</span>
          <button className="pv-adjust-zero" onClick={() => onFineAngle(0)} title="归零">
            0
          </button>
          <span>+{MAX_FINE_ANGLE}°</span>
        </div>
      </div>

      {/* ── 旋转 90° ── */}
      <div className="pv-adjust-group">
        <div className="pv-adjust-label-row">
          <span className="pv-adjust-label">旋转</span>
        </div>
        <div className="pv-adjust-rotates">
          <button
            className="pv-rotate-btn"
            onClick={onRotateCcw}
            title="逆时针旋转 90°（可保存）"
          >
            <span aria-hidden>↺</span> 逆时针 90°
          </button>
          <button
            className="pv-rotate-btn"
            onClick={onRotateCw}
            title="顺时针旋转 90°（可保存）"
          >
            顺时针 90° <span aria-hidden>↻</span>
          </button>
        </div>
      </div>

      {/* ── 比例 ── */}
      <div className="pv-adjust-group">
        <div className="pv-adjust-label-row">
          <span className="pv-adjust-label">画布比例</span>
        </div>
        <div className="pv-adjust-ratios" role="radiogroup" aria-label="画布比例">
          {ASPECT_OPTIONS.map((opt) => (
            <button
              key={opt.value}
              className="pv-ratio-btn"
              data-active={aspect === opt.value}
              onClick={() => onAspect(opt.value)}
              role="radio"
              aria-checked={aspect === opt.value}
              title={
                opt.value === 'free'
                  ? '自由：画面上有一个框，拖四角改大小、拖框内移动'
                  : `裁剪为 ${opt.label}`
              }
            >
              {opt.label}
            </button>
          ))}
        </div>

        {/*
          状态行：**两种模式共用一句话**（2026-10-08 统一）——
          画面上始终有一个框，拖四角改大小、拖框内平移；
          区别只在固定比例会把宽高比锁死，所以额外标注一下比例。
          没动过框时 `crop` 为 `null`（= 不裁剪），此时只显示操作提示。
        */}
        <div className="pv-adjust-crop">
          {crop ? (
            <>
              <span className="pv-adjust-crop-info">
                已框选 {Math.round(crop.w * 100)}% × {Math.round(crop.h * 100)}%
              </span>
              <button className="pv-adjust-crop-clear" onClick={onClearCrop} title="清除框选">
                清除
              </button>
            </>
          ) : (
            <span className="pv-adjust-crop-hint">
              {aspect === 'free'
                ? '拖框四角改大小、拖框内移动'
                : `拖框四角改大小、拖框内移动（比例锁为 ${aspect}）`}
            </span>
          )}
        </div>
      </div>

      <button
        className="pv-adjust-reset"
        onClick={onReset}
        disabled={isDefault}
        title="旋转、角度与比例一并复位"
      >
        复位
      </button>

      {/*
        保存按钮分两种形态（2026-10-10）：
        - 可覆盖的格式（JPEG/PNG/BMP）→ 「覆盖原图」，直接写回原文件；
        - 不可覆盖的格式（**RAW** 等）→ 「另存为…」，弹保存框写新文件。
        ⛔ 不要把不可覆盖的格式做成「置灰」—— 那等于告诉用户这条路走不通，
        而RAW 其实有出口（导出 JPEG/PNG，原文件不动）。
      */}
      {writable ? (
        <button
          className="pv-adjust-save"
          onClick={onSave}
          disabled={saving}
          title="把旋转与裁剪写回原文件（原文件将被覆盖）"
        >
          {saving ? '保存中…' : '覆盖原图'}
        </button>
      ) : (
        <button
          className="pv-adjust-save"
          data-kind="export"
          onClick={onSaveAs}
          disabled={saving}
          title="导出为 JPEG 或 PNG，写到新文件（原文件不变）"
        >
          {saving ? '导出中…' : '另存为…'}
        </button>
      )}

      {/* ── 状态 / 提示 ── */}
      {saveState.kind === 'ok' && (
        <p className="pv-adjust-status" data-kind="ok" role="status">
          {saveState.message}
        </p>
      )}
      {saveState.kind === 'error' && (
        <p className="pv-adjust-status" data-kind="error" role="alert">
          {saveState.message}
        </p>
      )}
      {/* 角度微调不写回 —— 有值时明确告知，避免用户以为存进去了 */}
      {saveState.kind !== 'saving' && fineAngle !== 0 && (
        <p className="pv-adjust-note">
          角度微调（{fineAngle > 0 ? `+${fineAngle}` : fineAngle}°）仅用于预览，
          保存时只应用 90° 旋转与比例裁剪。
        </p>
      )}
      {/*
        不可覆盖时的说明。⚠️ 措辞要说清「有出口」——
        只说「不能覆盖」而不给出路，用户会以为整个保存功能对 RAW 不可用。
      */}
      {!writable && notWritableReason && (
        <p className="pv-adjust-note" data-kind="warn">
          {notWritableReason}
        </p>
      )}
    </aside>
  )
}
