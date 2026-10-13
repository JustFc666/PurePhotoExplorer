/**
 * 打印预览浮层（居中模态）。
 *
 * ## 存在的理由
 * 原来「打印」是**一点就送印**（直接弹系统对话框 → 立刻送打印机）。
 * 用户在裁剪/旋转之后点打印，很容易在没确认排版的情况下把纸浪费掉，
 * 而且发现印歪了也已经印完了。预览让「所见」先于「所印」。
 *
 * ## ⛔ 排版计算必须与后端 `win_print::layout()` 严格一致
 * 预览画的是**图在纸上的位置**，真正送印的是 `layout()` 算出的矩形。
 * 两边算法一旦分叉，就会出现「预览看着居中、印出来贴左上」这类
 * **最难查的不一致**（图没错、设置没错，就是位置不对）。
 *
 * 所以本文件里的 [`computePrintLayout`] 是后端 `layout()` 的 1:1 移植，
 * 两处改动**必须同步**。测试见 `printLayout.test.ts`，
 * 那边固化了后端全部 6 条单测的期望值。
 *
 * ## 为什么不用系统对话框自带的预览
 * `PD_PREVIEWFLAG` 依赖打印机驱动支持，行为不可控、样式也不统一；
 * 应用内自己画可控、可靠（详见 `MEMORY-dev.md`）。
 */

import { useEffect, useMemo, useRef, useState } from 'react'
import type { DecodedImage } from '../../ipc'
import { listPrinters } from '../../ipc'

/** 纸张尺寸（毫米，含边距的整纸） */
export interface PaperSize {
  id: string
  label: string
  /** 横向（Portrait）时的整纸宽/高，毫米 */
  widthMm: number
  heightMm: number
}

/**
 * 支持的纸张（含照片用纸）。
 *
 * ⚠️ 这里的毫米数是**整纸（纵向）**尺寸；真正能打印的区域还要扣掉页边距，
 * 见 [`MARGIN_MM`]。横向打印时宽高在预览里临时对调，但物理纸张尺寸不变
 * （方向由 `orientation` 控制，送印时写进 `DEVMODE.dmOrientation`）。
 */
export const PAPER_SIZES: readonly PaperSize[] = [
  { id: 'a4', label: 'A4', widthMm: 210, heightMm: 297 },
  { id: 'a5', label: 'A5', widthMm: 148, heightMm: 210 },
  { id: 'a6', label: 'A6', widthMm: 105, heightMm: 148 },
  { id: 'a3', label: 'A3', widthMm: 297, heightMm: 420 },
  { id: 'letter', label: 'Letter', widthMm: 215.9, heightMm: 279.4 },
  { id: 'legal', label: 'Legal', widthMm: 215.9, heightMm: 355.6 },
  { id: 'photo_3_5x5', label: '3.5×5 英寸（照片）', widthMm: 88.9, heightMm: 127.0 },
  { id: 'photo_4x6', label: '4×6 英寸（照片）', widthMm: 101.6, heightMm: 152.4 },
  { id: 'photo_5x7', label: '5×7 英寸（照片）', widthMm: 127.0, heightMm: 177.8 },
  { id: 'photo_8x10', label: '8×10 英寸（照片）', widthMm: 203.2, heightMm: 254.0 },
]

/** 打印方向 */
export type PrintOrientation = 'portrait' | 'landscape'
export const ORIENTATIONS: readonly { id: PrintOrientation; label: string }[] = [
  { id: 'portrait', label: '纵向' },
  { id: 'landscape', label: '横向' },
]

/** 打印质量（负数 = 驱动预设档：草稿/普通/高） */
export type PrintQuality = 'draft' | 'normal' | 'high'
export const QUALITIES: readonly { id: PrintQuality; label: string }[] = [
  { id: 'draft', label: '草稿' },
  { id: 'normal', label: '普通' },
  { id: 'high', label: '高（照片）' },
]

/**
 * 页边距（毫米）—— 必须与后端 `win_print::MARGIN_MM` 一致。
 *
 * 导出是因为测试要拿**同一个数值**去对账后端的期望值；
 * 测试里自己写一个 `10` 的话，两边一旦不同步，测试会绿着通过而预览是错的。
 */
export const MARGIN_MM = 10

/**
 * 参考 DPI —— 必须与后端 `win_print::ASSUMED_IMAGE_DPI` 一致。
 *
 * 图像没有物理尺寸信息（我们不读 DPI 元数据），所以「原始尺寸」按屏幕
 * 惯用的 96 DPI 换算，含义是「屏幕上多大就印多大」。
 */
const ASSUMED_IMAGE_DPI = 96

/** 预览按多少 DPI 绘制（只影响显示，不影响送印） */
const PREVIEW_DPI = 96

/**
 * 图像在纸上的位置（**归一化**，0~1，相对整纸）。
 *
 * 用归一化而不是设备像素：预览 canvas 尺寸随容器变，
 * 归一化让同一份计算在任何显示尺寸下都成立。
 */
export interface PrintLayout {
  /** 左边缘（0~1，相对纸宽） */
  x: number
  /** 上边缘（0~1，相对纸高） */
  y: number
  width: number
  height: number
  /** 可打印区域（扣掉页边距后）的宽高，毫米 */
  printableMm: { w: number; h: number }
  /** 图像实际落到纸上的尺寸，毫米 */
  imageMm: { w: number; h: number }
  /** 是否被缩小过（超出页面时） */
  shrunk: boolean
  /** 相对「屏幕上多大」的缩放倍率（1 = 原大打印） */
  scale: number
}

/**
 * ⛔ 后端 `win_print::layout()` 的 1:1 移植 —— **两边必须同步改**。
 *
 * 参数都用毫米/像素比率而非设备像素，好让预览与 DPI 解耦：
 * - `paperW/paperH`：整纸尺寸（毫米）
 * - `imgW/imgH`：图像像素
 * - `dpi`：打印机 DPI（影响「原始尺寸」模式下 1 像素等于多少毫米）
 * - `fit`：`true` = 铺满可打印区域；`false` = 按屏幕大小（超出才缩）
 *
 * 后端那边先算设备像素再换算，这里直接算毫米 —— 数学等价，
 * 但这样预览不需要知道打印机 DPI 的具体值也能给出正确的**相对**位置。
 */
export function computePrintLayout(
  paperW: number,
  paperH: number,
  imgW: number,
  imgH: number,
  dpi: number,
  fit: boolean,
  marginMm: number = MARGIN_MM,
): PrintLayout {
  const marginW = marginMm * 2
  const marginH = marginMm * 2
  const availW = Math.max(1, paperW - marginW)
  const availH = Math.max(1, paperH - marginH)
  const iw = Math.max(1, imgW)
  const ih = Math.max(1, imgH)

  // 像素 → 毫米：打印时 1 像素 = 1/dpi 英寸
  const pxToMm = 25.4 / Math.max(1, dpi)

  let scale: number
  if (fit) {
    // 适应页面：两个方向里较小的缩放比（等比，不拉变形）
    scale = Math.min(availW / (iw * pxToMm), availH / (ih * pxToMm))
  } else {
    // 原始尺寸：按屏幕 DPI 换算成「屏幕大小」，超出页面才等比缩小
    const base = ASSUMED_IMAGE_DPI / dpi // 屏幕像素 → 纸上毫米
    const wAtBase = iw * base
    const hAtBase = ih * base
    const shrink = Math.min(availW / wAtBase, availH / hAtBase, 1)
    scale = base * shrink
  }

  const imgWmm = iw * pxToMm * scale
  const imgHmm = ih * pxToMm * scale

  // 居中（后端是 `margin + (avail - dw) / 2`，等价）
  const xMm = marginMm + (availW - imgWmm) / 2
  const yMm = marginMm + (availH - imgHmm) / 2

  return {
    x: xMm / paperW,
    y: yMm / paperH,
    width: imgWmm / paperW,
    height: imgHmm / paperH,
    printableMm: { w: availW, h: availH },
    imageMm: { w: imgWmm, h: imgHmm },
    shrunk: scale < 0.999,
    scale,
  }
}

interface PrintPreviewProps {
  /** 打印时的实际像素尺寸（= 画布调整后的结果） */
  imageWidth: number
  imageHeight: number
  /** 当前图的解码像素（预览要画出真实照片，而不只是占位色块） */
  sourceImage: DecodedImage
  /** 90° 旋转步数（顺时针，0~3）—— 与送印用的 `steps` 一致 */
  rotation: number
  /** 打印进行中（禁用确认按钮，避免连点弹多个系统对话框） */
  printing: boolean
  /** 出错时的消息 */
  error?: string
  onClose: () => void
  /** 确认送印 */
  onConfirm: (opts: {
    paper: PaperSize
    fit: boolean
    orientation: PrintOrientation
    quality: PrintQuality
    /** 跳过系统对话框、直接送打印机 */
    quick: boolean
    /** 直接打印的目标打印机；`''` = 系统默认 */
    printer: string
    /** 四边页边距（毫米） */
    marginMm: number
  }) => void
}

/**
 * 把解码像素画成 dataURL，供预览 `<img>` 显示。
 *
 * ⚠️ **旋转要烘焙进位图**，不能只靠 CSS `transform: rotate`：
 * 预览图块（`.pv-print-image`）的盒子尺寸已经是**旋转后**的比例，
 * 若只转元素不转像素，图像会与盒子比例不匹配而被 `object-fit` 拉变形。
 * 这里直接在离屏 canvas 上按 90° 步数旋转后导出，导出的位图尺寸
 * 恰好等于盒子尺寸，预览里看到的就是「印出来那张图」的样子
 * （不含裁剪——裁剪区已在后端 `layout()` 以盒子尺寸体现，这里只画内容）。
 */
function buildPreviewSrc(image: DecodedImage | null | undefined, rotation: number): string | null {
  if (!image || !image.data || image.width <= 0 || image.height <= 0) return null
  const src = document.createElement('canvas')
  src.width = image.width
  src.height = image.height
  const sctx = src.getContext('2d')
  if (!sctx) return null
  try {
    sctx.putImageData(new ImageData(image.data, image.width, image.height), 0, 0)
  } catch {
    return null
  }
  const steps = (((Math.round(rotation / 90) % 4) + 4) % 4) as number
  if (steps === 0) return src.toDataURL('image/png')

  const swapped = steps % 2 === 1
  const w = swapped ? image.height : image.width
  const h = swapped ? image.width : image.height
  const out = document.createElement('canvas')
  out.width = w
  out.height = h
  const octx = out.getContext('2d')
  if (!octx) return src.toDataURL('image/png')
  octx.translate(w / 2, h / 2)
  octx.rotate((steps * 90 * Math.PI) / 180)
  octx.drawImage(src, -image.width / 2, -image.height / 2)
  return out.toDataURL('image/png')
}

export function PrintPreview({
  imageWidth,
  imageHeight,
  sourceImage,
  rotation,
  printing,
  error,
  onClose,
  onConfirm,
}: PrintPreviewProps) {
  const [paper, setPaper] = useState<PaperSize>(PAPER_SIZES[0]!)
  const [orientation, setOrientation] = useState<PrintOrientation>('portrait')
  const [quality, setQuality] = useState<PrintQuality>('high')
  const [fit, setFit] = useState(true)
  const [quick, setQuick] = useState(false)
  /** 页边距（毫米，四边同值）—— 与送印传后端的是同一个值 */
  const [marginMm, setMarginMm] = useState(MARGIN_MM)
  /** 本机打印机列表；空数组 = 枚举失败，下拉框退化为只有「系统默认」 */
  const [printers, setPrinters] = useState<string[]>([])
  /** 直接打印目标；`''` = 系统默认打印机 */
  const [printer, setPrinter] = useState('')
  const paperRef = useRef<HTMLDivElement>(null)

  // 打开浮层时枚举一次打印机（失败静默——打印服务没起也能照常打印到默认）
  useEffect(() => {
    let alive = true
    listPrinters()
      .then((list) => {
        if (alive) setPrinters(list)
      })
      .catch(() => {})
    return () => {
      alive = false
    }
  }, [])

  /**
   * 横向时把整纸宽高对调，预览显示与送印完全一致（送印时方向写进
   * `DEVMODE.dmOrientation`，物理纸张尺寸不变）。
   */
  const effW = orientation === 'landscape' ? paper.heightMm : paper.widthMm
  const effH = orientation === 'landscape' ? paper.widthMm : paper.heightMm

  /**
   * 排版用**固定的参考 DPI**（`PREVIEW_DPI`），所以同一个
   * 「纸张 + 适应模式」组合算出的相对位置是稳定的 —— 切换时不会跳。
   */
  const layout = useMemo(
    () =>
      computePrintLayout(
        effW,
        effH,
        imageWidth,
        imageHeight,
        PREVIEW_DPI,
        fit,
        marginMm,
      ),
    [effW, effH, imageWidth, imageHeight, fit, marginMm],
  )

  /**
   * 真实照片的 dataURL。随「当前图 / 旋转」变化重算；
   * 旋转烘焙进位图，盒子尺寸与之完全一致。
   */
  const previewSrc = useMemo(
    () => buildPreviewSrc(sourceImage, rotation),
    [sourceImage, rotation],
  )

  // Esc 关闭（由 App 的快捷键链触发 onClose，这里只负责挂遮罩点击）
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])

  return (
    <div
      className="pv-print-backdrop"
      onMouseDown={(e) => {
        // 点遮罩关闭；点卡片内部不关（mousedown 而非 click，避免拖拽时误关）
        if (paperRef.current && !paperRef.current.contains(e.target as Node)) onClose()
      }}
    >
      <div className="pv-print" ref={paperRef} role="dialog" aria-label="打印预览">
        <div className="pv-print-head">
          <span className="pv-print-title">打印预览</span>
          <button
            className="pv-print-close"
            onClick={onClose}
            aria-label="关闭打印预览"
            title="关闭"
          >
            ×
          </button>
        </div>

        {/* 纸张示意：白纸 + 图在纸上的实际相对位置 */}
        <div className="pv-print-stage">
          <div
            className="pv-print-paper"
            style={{
              // 保持纸张长宽比（已按方向对调），并把整纸限制在可用空间内
              aspectRatio: `${effW} / ${effH}`,
            }}
          >
            <div
              /*
               * 可打印区（扣掉页边距）。
               *
               * ⚠️ **必须用百分比，不能用 CSS 的 `inset: 10mm`** ——
               * 纸张显示高度是 `height: 300px`，它对应的实际毫米数不等于
               * 预览用的 `PREVIEW_DPI` 下 297mm（300px 在 96dpi下≈ 79mm），
               * 坐标系不同会让虚线框和图像块错位（实测图块跑到框外）。
               *
               * 百分比则与纸元素同坐标系，与图像块用的 `left/top/width/height`
               * 完全一致 —— 后端 `layout()` 的 margin 本来就是按纸张比例算的。
               */
              className="pv-print-margin"
              style={{
                left: `${(marginMm / effW) * 100}%`,
                top: `${(marginMm / effH) * 100}%`,
                right: `${(marginMm / effW) * 100}%`,
                bottom: `${(marginMm / effH) * 100}%`,
              }}
            />
            <div
              className="pv-print-image"
              style={{
                left: `${layout.x * 100}%`,
                top: `${layout.y * 100}%`,
                width: `${layout.width * 100}%`,
                height: `${layout.height * 100}%`,
              }}
            >
              {previewSrc && (
                <img className="pv-print-img" src={previewSrc} alt="打印预览" draggable={false} />
              )}
            </div>
          </div>
        </div>

        {/* 打印设置：两列网格（打印机独占一行） */}
        <div className="pv-print-options">
          <label className="pv-print-field pv-print-field-wide">
            <span className="pv-print-group-label">打印机</span>
            <select
              className="pv-print-select"
              value={printer}
              onChange={(e) => setPrinter(e.target.value)}
              title="直接打印（勾选快速打印）时送到这里选的打印机"
            >
              <option value="">系统默认打印机</option>
              {printers.map((p) => (
                <option key={p} value={p}>
                  {p}
                </option>
              ))}
            </select>
          </label>
          <label className="pv-print-field">
            <span className="pv-print-group-label">纸张</span>
            <select
              className="pv-print-select"
              value={paper.id}
              onChange={(e) => {
                const p = PAPER_SIZES.find((x) => x.id === e.target.value)
                if (p) setPaper(p)
              }}
            >
              {PAPER_SIZES.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.label}（{p.widthMm} × {p.heightMm} mm）
                </option>
              ))}
            </select>
          </label>
          <label className="pv-print-field">
            <span className="pv-print-group-label">方向</span>
            <select
              className="pv-print-select"
              value={orientation}
              onChange={(e) => setOrientation(e.target.value as PrintOrientation)}
            >
              {ORIENTATIONS.map((o) => (
                <option key={o.id} value={o.id}>
                  {o.label}
                </option>
              ))}
            </select>
          </label>
          <label className="pv-print-field">
            <span className="pv-print-group-label">质量</span>
            <select
              className="pv-print-select"
              value={quality}
              onChange={(e) => setQuality(e.target.value as PrintQuality)}
            >
              {QUALITIES.map((q) => (
                <option key={q.id} value={q.id}>
                  {q.label}
                </option>
              ))}
            </select>
          </label>
          <label className="pv-print-field">
            <span className="pv-print-group-label">页边距</span>
            <input
              className="pv-print-margin-input"
              type="number"
              min={0}
              max={50}
              step={1}
              value={marginMm}
              onChange={(e) => {
                const v = Number(e.target.value)
                if (Number.isFinite(v)) setMarginMm(Math.min(50, Math.max(0, Math.round(v))))
              }}
              title="四边页边距（毫米），0–50"
            />
            <span className="pv-print-unit">mm</span>
          </label>
          <div className="pv-print-group" role="radiogroup" aria-label="版面">
            <span className="pv-print-group-label">版面</span>
            <button
              className="pv-print-chip"
              data-active={fit}
              onClick={() => setFit(true)}
              role="radio"
              aria-checked={fit}
              title="等比缩放铺满可打印区域"
            >
              适应页面
            </button>
            <button
              className="pv-print-chip"
              data-active={!fit}
              onClick={() => setFit(false)}
              role="radio"
              aria-checked={!fit}
              title="按屏幕上的大小印，超出页面时才缩小"
            >
              原始尺寸
            </button>
          </div>
        </div>

        {/* 快速打印：跳过系统对话框，直接送所选打印机 */}
        <label className="pv-print-quick" title="勾选后点「打印…」会直接送到上方选的打印机（未选则用系统默认），不弹出系统对话框（固定 1 份、无取消）">
          <input
            type="checkbox"
            checked={quick}
            onChange={(e) => setQuick(e.target.checked)}
          />
          <span>快速打印（跳过对话框，直接送所选打印机）</span>
        </label>

        {error && (
          <p className="pv-print-error" role="alert">
            {error}
          </p>
        )}

        <div className="pv-print-actions">
          <button className="pv-print-btn" onClick={onClose} disabled={printing}>
            取消
          </button>
          <button
            className="pv-print-btn pv-print-btn-primary"
            onClick={() => onConfirm({ paper, fit, orientation, quality, quick, printer, marginMm })}
            disabled={printing}
          >
            {printing ? '送印中…' : '打印…'}
          </button>
        </div>
      </div>
    </div>
  )
}