/**
 * 人类可读的展示格式化工具。
 */

/** 字节数 → 人类可读的文件大小 */
export function formatFileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`
  return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`
}

/**
 * 可**覆盖写回原文件**的格式（与 Rust 侧 `pv_decode::can_overwrite_original` 保持一致）
 *
 * ⚠️ 值是 `ImageFormat` 的**枚举变体名小写**（后端 `format!("{:?}").to_lowercase()`），
 * 所以 JPEG 是 `jpeg` 而不是 `jpg`。`jpg` 也留着当兜底 ——
 * 万一将来后端改用扩展名当id，白名单漏一个就表现为「保存按钮莫名置灰」。
 */
const WRITABLE_FORMATS = new Set(['jpeg', 'jpg', 'png', 'bmp'])

/**
 * 该格式能否**覆盖写回原文件**。
 *
 * 与后端 `can_overwrite_original` 是同一份规则的两次实现 —— 这里只为「提前置灰按钮」，
 * 真正的判据仍在后端（点了也会再校验一次）。
 */
export function isWritableFormat(format: string): boolean {
  return WRITABLE_FORMATS.has(format.toLowerCase())
}

/** 「另存为」可选的目标格式（与后端 `format_from_extension` 保持一致） */
export const EXPORT_FORMATS: ReadonlyArray<{
  /** 不含点的扩展名 */
  ext: string
  /** `accept` 里用的筛选名 */
  label: string
}> = [
  { ext: 'jpg', label: 'JPEG 图片' },
  { ext: 'png', label: 'PNG 图片' },
  { ext: 'bmp', label: 'BMP 图片' },
]

/**
 * 不能覆盖保存时的原因文案。
 *
 * 与后端 `not_writable_reason` 措辞对齐：后端也会返回同样一句话，
 * 这里是**未点按钮时**就能悬停看到的版本。
 *
 * ⚠️ 措辞里不要出现「不提供保存」——RAW 现在**有**出口（另存为），
 * 说成不能保存会让用户以为整条路走不通。
 */
export function notWritableReason(formatLabel: string, isRaw: boolean): string {
  if (isRaw) {
    return `${formatLabel} 是相机 RAW 格式，覆盖原文件会破坏相机原始数据。可用「另存为」导出 JPEG 或 PNG，原文件保持不变。`
  }
  return `暂支持覆盖保存 JPEG / PNG / BMP；${formatLabel} 可用「另存为」导出。`
}

/**
 * RAW 导出时的画质提示。
 *
 * ⚠️ 这条提示不能省：RAW 画布上的像素来自**相机内嵌的 JPEG 预览**，
 * 导出得到的文件就是那个分辨率，不是传感器全尺寸。
 * 用户看到"导出后变模糊"却没人解释，会以为是 bug。
 */
export function rawExportHint(
  exportedWidth: number,
  exportedHeight: number,
  sourceWidth?: number,
  sourceHeight?: number,
): string {
  const base = `已导出 ${exportedWidth} × ${exportedHeight}`
  // 只有在确实拿到更大的传感器尺寸、且预览明显小于它时才提示，
  // 否则「预览 = 全尺寸」的机型会被误报。
  if (!sourceWidth || !sourceHeight) return base
  if (exportedWidth >= sourceWidth && exportedHeight >= sourceHeight) return base
  return `${base}（来自内嵌预览，原图为 ${sourceWidth} × ${sourceHeight}，非全分辨率）`
}
