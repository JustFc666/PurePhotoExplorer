/**
 * 压缩包图标 —— 内联 SVG 矢量（2026-10-31）。
 *
 * 用在两处，尺寸都跟容器走：
 * - 图库网格里的压缩包卡片（`.pv-mg-diricon`，最大 110px）
 * - 侧栏文件夹树的行内小图标（16px）
 *
 * 为什么不用系统图标：虽然盘符能拿到资源管理器的图标，但 `.zip` 的系统图标
 * 是「带拉链的文件夹」，和旁边的黄色文件夹卡片太像 —— 用户一眼分不清
 * 「这是文件夹还是压缩包」。这里画成**独立的压缩包盒子**，一眼可辨。
 *
 * 画布 256 × 218，与 `FolderIcon` / `DriveIcon` 一致，
 * 同一网格里高度不参差。
 *
 * 三个形状：
 *   ① 盒身（浅紫 `#8E8AA8` 底 + 亮面 `#A5A1BF`）
 *   ② 竖向拉链槽 + 齿
 *   ③ 右侧折角（暗示「压扁了」）
 */

interface Props {
  /** 定宽定高（正方形，图形居中）；不传则靠 `className` 由 CSS 定宽 */
  size?: number
  className?: string
  title?: string
}

const VB_W = 256
const VB_H = 218

export function ArchiveIcon({ size, className, title }: Props) {
  const box = size != null ? { width: size, height: size } : undefined
  return (
    <svg
      className={className}
      viewBox={`0 0 ${VB_W} ${VB_H}`}
      preserveAspectRatio="xMidYMid meet"
      role={title ? 'img' : undefined}
      aria-hidden={title ? undefined : true}
      aria-label={title}
      {...box}
    >
      {title && <title>{title}</title>}

      {/* ① 盒身：底 + 亮面（左上偏亮，制造厚度） */}
      <rect x="40" y="26" width="176" height="166" rx="16" fill="#7C7793" />
      <rect x="40" y="26" width="176" height="96" rx="16" fill="#9994B3" />
      <rect x="40" y="102" width="176" height="20" fill="#9994B3" />

      {/* ② 拉链槽：中间一条竖缝，两侧齿交错 */}
      <rect x="118" y="26" width="20" height="166" fill="#6A6580" />
      {[0, 1, 2, 3, 4, 5, 6].map((i) => (
        <g key={i}>
          <rect x="113" y={40 + i * 22} width="13" height="9" rx="2" fill="#D8D4E8" />
          <rect x="130" y={51 + i * 22} width="13" height="9" rx="2" fill="#D8D4E8" />
        </g>
      ))}

      {/* ③ 右侧折角：右下角斜切一块更暗的面，暗示「被压扁」 */}
      <path d="M216 152 L216 176 A16 16 0 0 1 200 192 L176 192 Z" fill="#6A6580" />
    </svg>
  )
}
