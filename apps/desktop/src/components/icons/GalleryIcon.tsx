/**
 * 图库图标 —— 八瓣彩色花，**内联 SVG 矢量**（2026-10-18）。
 *
 * 重建自用户给的参考图（736×736 位图）：
 * 8 个「楔形 + 圆弧顶」的花瓣绕中心每 45° 一片，相邻花瓣互相搭接，
 * 搭接处呈**正片叠底**（multiply）变暗 —— 与原图逐色比对确认过
 * （如 青×蓝 → `#0079a8`，正是两色相乘的值）。
 *
 * ⚠️ 混合模式必须在 `isolation: isolate` 的分组里用：不隔离的话
 * multiply 会和底下的**窗口深色背景**相乘，整朵花直接糊成黑团；
 * 隔离后透明底不受影响，只有花瓣彼此相乘，任意背景色都安全。
 *
 * 八瓣颜色（从正上方起顺时针）：
 *   橙 `#FFB74B` → 黄绿 `#BDC531` → 绿 `#8CC24A` → 青 `#00A9BC` →
 *   蓝 `#63B1EF` → 紫 `#B565C6` → 粉 `#EF8EAD` → 红 `#FF5152`
 *
 * 用法：`<GalleryIcon size={16} />` 定尺寸，或 `<GalleryIcon className=… />`
 * 交给 CSS 定尺寸（同 `FolderIcon` 的约定）。
 */

interface Props {
  /** 定宽定高（正方形）。给了 `size` 就不需要 CSS 再定尺寸 */
  size?: number
  /** 不传 `size` 时靠这个类定尺寸 */
  className?: string
  /** 需要无障碍语义时才传；默认 `aria-hidden` */
  title?: string
}

/** 八瓣颜色，从正上方（0°）起顺时针，每 45° 一片 */
const PETALS = ['#FFB74B', '#BDC531', '#8CC24A', '#00A9BC', '#63B1EF', '#B565C6', '#EF8EAD', '#FF5152']

/** 单片花瓣（指向正上方，绕 128,128 旋转复用）：
 * 从中心附近的尖点起，两条略外凸的边到 y=68 处展开成 72 宽，
 * 顶上用一段圆弧（R≈36）收成圆头。 */
const PETAL_PATH = 'M128 122 Q104 94 92 68 A36.2 36.2 0 0 1 164 68 Q152 94 128 122 Z'

export function GalleryIcon({ size, className, title }: Props) {
  const box = size != null ? { width: size, height: size } : undefined
  return (
    <svg
      className={className}
      viewBox="0 0 256 256"
      preserveAspectRatio="xMidYMid meet"
      aria-hidden={title ? undefined : true}
      role={title ? 'img' : undefined}
      {...box}
    >
      <defs>
        <path id="pv-gallery-petal" d={PETAL_PATH} />
      </defs>
      {/* isolate：把 multiply 限制在这朵花内部，不与页面背景相乘 */}
      <g style={{ isolation: 'isolate' }}>
        {PETALS.map((color, i) => (
          <use
            key={color}
            href="#pv-gallery-petal"
            fill={color}
            style={{ mixBlendMode: 'multiply' }}
            transform={i === 0 ? undefined : `rotate(${i * 45} 128 128)`}
          />
        ))}
      </g>
    </svg>
  )
}
