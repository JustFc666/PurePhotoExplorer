/**
 * 缩放控件：显示当前缩放百分比 + 可拖拽的滑块（对数刻度，5%–3200%）。
 *
 * 内联在底部信息栏。采用对数刻度是因为线性刻度下小比例段（10%–100%）
 * 几乎无法精细调节。
 */

import { MAX_SCALE, MIN_SCALE } from '../../hooks/useImageView'

const LOG_MIN = Math.log(MIN_SCALE)
const LOG_MAX = Math.log(MAX_SCALE)
const SLIDER_STEPS = 1000

function scaleToSlider(scale: number): number {
  const t = (Math.log(scale) - LOG_MIN) / (LOG_MAX - LOG_MIN)
  return Math.round(Math.min(1, Math.max(0, t)) * SLIDER_STEPS)
}

function sliderToScale(v: number): number {
  return Math.exp(LOG_MIN + (v / SLIDER_STEPS) * (LOG_MAX - LOG_MIN))
}

interface ZoomControlProps {
  scale: number
  onScale: (scale: number) => void
}

export function ZoomControl({ scale, onScale }: ZoomControlProps) {
  return (
    <div className="pv-zoom">
      <input
        className="pv-zoom-slider"
        type="range"
        min={0}
        max={SLIDER_STEPS}
        value={scaleToSlider(scale)}
        onChange={(e) => onScale(sliderToScale(Number(e.target.value)))}
        aria-label="缩放比例"
        title="拖动缩放（Ctrl+滚轮）"
      />
      <span className="pv-zoom-value">{Math.round(scale * 100)}%</span>
    </div>
  )
}
