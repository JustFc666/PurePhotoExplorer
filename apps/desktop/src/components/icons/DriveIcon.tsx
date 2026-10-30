/**
 * 磁盘图标 —— 内联 SVG（2026-10-30）。
 *
 * 「我的电脑」主区要直接铺一排盘符卡片（用户要求），系统抽屉图标取不到
 * 也没必要取：一个盘位盒子 + 一颗指示灯就够了，矢量任意尺寸都锐利。
 *
 * 画布 256 × 218，和 [`FolderIcon`](./FolderIcon.tsx) 一致 ——
 * 这样两者放在同一个网格里时，`width` 相同、视觉高度也接近，不会参差。
 */

interface Props {
  /** 定宽定高（正方形，图形居中）；不传则靠 `className` 由 CSS 定宽 */
  size?: number
  className?: string
  title?: string
}

const VB_W = 256
const VB_H = 218

export function DriveIcon({ size, className, title }: Props) {
  const box = size != null ? { width: size, height: size } : undefined
  return (
    <svg
      className={className}
      viewBox={`0 0 ${VB_W} ${VB_H}`}
      style={box}
      role={title ? 'img' : undefined}
      aria-hidden={title ? undefined : true}
      aria-label={title}
    >
      {title && <title>{title}</title>}
      {/* 外壳 */}
      <rect x="18" y="62" width="220" height="94" rx="14" fill="#5C6472" />
      {/* 上面板（略亮，制造一点厚度感） */}
      <rect x="18" y="62" width="220" height="58" rx="14" fill="#6E7788" />
      <rect x="18" y="106" width="220" height="14" fill="#6E7788" />
      {/* 盘位两条缝 */}
      <rect x="42" y="82" width="112" height="7" rx="3.5" fill="#3F4653" />
      <rect x="42" y="97" width="80" height="7" rx="3.5" fill="#3F4653" />
      {/* 指示灯 */}
      <circle cx="205" cy="131" r="7" fill="#8FE3A0" />
    </svg>
  )
}
