/**
 * 文件夹图标 —— **内联 SVG 矢量**（2026-10-17）。
 *
 * 为什么不用位图：
 * 1. 侧栏树里是 16px，网格里最大 320px 宽的卡片要用到 110px 以上，
 *    一张 64px 的 PNG 放大就糊；矢量任意尺寸都锐利。
 * 2. 之前用「白色 → 透明」处理位图背景，会把**黄色文件夹里的白色纸张**
 *    一起抠掉（用户明确指出「黄色当中的白色要保留」）。矢量是三个形状
 *    直接画出来的，不存在抠图一说，白纸永远是白纸。
 *
 * 图形按 256 × 218 的画布重建自用户给的参考图，三层自下而上：
 *   ① 后板 + 左上标签（深黄 `#E4993A`）
 *   ② 白色纸张（`#FFFFFF`，夹在两层之间，右端斜角由前板斜坡自然裁出）
 *   ③ 前板（亮黄 `#FFBB32`，右侧顶边更高，中部有一段斜坡过渡）
 *
 * 用法：
 * - 固定尺寸：`<FolderIcon size={16} />` → 画成 16×16（居中留白，与系统图标对齐）
 * - 跟着容器缩放：`<FolderIcon className="pv-mg-diricon" />`，由 CSS 定宽，
 *   高度按 `256 / 218` 的比例自动得出（见 `manager.css`）。
 */

interface Props {
  /** 定宽定高（正方形，图形居中）。给了 `size` 就不需要 CSS 再定尺寸 */
  size?: number
  /** 不传 `size` 时靠这个类定尺寸 */
  className?: string
  /** 需要无障碍语义时才传；默认 `aria-hidden` */
  title?: string
}

/** 画布宽高（= `viewBox`），形状坐标都以它为基准 */
const VB_W = 256
const VB_H = 218

export function FolderIcon({ size, className, title }: Props) {
  const box = size != null ? { width: size, height: size } : undefined
  return (
    <svg
      className={className}
      viewBox={`0 0 ${VB_W} ${VB_H}`}
      // 定死尺寸时图形居中缩放，四周留白 —— 和系统图标的 16×16 对齐
      preserveAspectRatio="xMidYMid meet"
      aria-hidden={title ? undefined : true}
      role={title ? 'img' : undefined}
      {...box}
    >
      {/* ① 后板 + 左上标签 */}
      <path
        fill="#E4993A"
        d="M13 190 L13 24 Q13 15 22 15 L86 15 Q95 15 99.5 21.5 L104.5 29 Q106.5 31.5 110 31.5 L229 31.5 Q243 31.5 243 45.5 L243 190 Q243 204 229 204 L27 204 Q13 204 13 190 Z"
      />
      {/* ② 白色纸张（下半截被前板盖住，露出的是夹在中间的那一条） */}
      <rect x="27" y="39" width="140" height="9" fill="#FFFFFF" />
      {/* ③ 前板 */}
      <path
        fill="#FFBB32"
        d="M13 190 L13 61 Q13 47 27 47 L146 47 Q153 47 156.5 42.5 L163 34.5 Q166.5 31 171 31 L229 31 Q243 31 243 45 L243 190 Q243 204 229 204 L27 204 Q13 204 13 190 Z"
      />
    </svg>
  )
}
