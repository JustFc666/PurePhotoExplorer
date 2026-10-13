/**
 * 打印（Windows 系统打印对话框 + GDI）。
 *
 * 与 [`saveEditedImage`](./edit.ts) **共用后端的同一套几何变换** ——
 * 传同样的 `rotationSteps` / `aspect` / `crop`，所以打印出来的就是
 * 画布上看到的那张（含裁剪与旋转）。
 *
 * 打印**不会**修改任何文件，也不进解码缓存。
 */

import { invoke } from '@tauri-apps/api/core'
import type { CropRectInput } from './edit'
import type { PrintResult } from './types'

/**
 * 枚举本机可用打印机（预览浮层「打印机」下拉框数据源）。
 * 打印服务异常时后端返回空数组，不抛错。
 */
export async function listPrinters(): Promise<string[]> {
  return invoke<string[]>('list_printers')
}

/**
 * 打印当前图像。
 *
 * @param path          源文件（用于按 Export 档重新解码全尺寸像素）
 * @param rotationSteps 顺时针 90° 的次数（与保存共用）
 * @param aspect        `free` / `1:1` / `3:4` / …
 * @param crop          自由框选区域；未框选传 `null`
 * @param fit           `true`（默认）= 适应页面；`false` = 原始尺寸（96dpi 逻辑尺寸）
 * @param orientation   `portrait` / `landscape` —— 经后端写进打印机 DEVMODE
 * @param quality       `draft` / `normal` / `high` —— 打印质量
 * @param paperWMm      整纸物理宽（毫米，纵向）
 * @param paperHMm      整纸物理高（毫米，纵向）
 * @param marginMm      四边页边距（毫米）—— 与预览同一个值，所见即所印
 * @param printerName   直接打印的目标打印机；`null`/空 = 系统默认
 * @param quick         `true` = 跳过系统对话框直接送印（1 份、无取消）
 *
 * 默认（`quick=false`）后端会弹出**系统打印对话框**（选打印机 / 份数）；
 * 方向 / 质量 / 纸张已按预览预设下发。用户取消时返回 `cancelled: true`，不抛错。
 */
export async function printImage(
  path: string,
  rotationSteps: number,
  aspect: string,
  crop: CropRectInput | null = null,
  fit = true,
  orientation: 'portrait' | 'landscape' = 'portrait',
  quality: 'draft' | 'normal' | 'high' = 'high',
  paperWMm = 210,
  paperHMm = 297,
  marginMm = 10,
  printerName: string | null = null,
  quick = false,
): Promise<PrintResult> {
  return invoke<PrintResult>('print_image', {
    path,
    rotationSteps,
    aspect,
    crop,
    fit,
    orientation,
    quality,
    paperWMm,
    paperHMm,
    marginMm,
    printer: printerName,
    quick,
  })
}
