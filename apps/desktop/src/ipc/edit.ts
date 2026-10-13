/**
 * 画布调整的落盘（覆盖原文件 / 另存为）。
 *
 * ⚠️ 覆盖保存是**唯一会修改用户原始文件**的调用。成功后必须让所有
 * 依赖该文件的缓存失效，否则界面会继续显示旧像素。
 *
 * ## 两种模式
 * | `targetPath` | 行为 | 源格式限制 |
 * |---|---|---|
 * | `undefined` | **覆盖原文件** | 仅 JPEG / PNG / BMP |
 * | 有值 | **另存为**新文件 | 任何能解码的格式（含 RAW / HEIC / WebP）|
 *
 * RAW 只能走另存为：它的像素来自相机内嵌 JPEG 预览，
 * 覆盖原文件等于用预览替换传感器数据，画质永久降级。
 */

import { invoke } from '@tauri-apps/api/core'
import { invalidateImage } from './image'
import type { SaveResult } from './types'

/** 自由框选的裁剪区（归一化坐标，0~1，相对旋转后的图像） */
export interface CropRectInput {
  x: number
  y: number
  w: number
  h: number
}

/**
 * 把当前画布调整（90° 旋转 + 裁剪）写盘。
 *
 * **注意**：角度微调（±45°）不参与写回。任意角度重采样会导致
 * 「预览所见 ≠ 文件所得」，所以只在预览层生效。
 *
 * 裁剪有两种表达，后端**优先看 `crop`**：
 * - `crop` 非空 → 按用户框选的归一化矩形裁（「自由」模式的鼠标框选）；
 * - 否则按 `aspect` 做居中比例裁剪。
 * 两者同时传是前端不该出现的情况，但后端已定义为「框选优先」。
 *
 * @param path          **源**文件（原文件本身）
 * @param rotationSteps 顺时针 90° 的次数（可为负 = 逆时针）
 * @param aspect        `free` / `1:1` / `3:4` / `4:3` / `16:9`
 * @param crop          自由框选区域；未框选时传 `null`
 * @param targetPath    另存为的目标路径；**留空 = 覆盖原文件**。
 *                      目标扩展名（.jpg/.png/.bmp）决定编码格式，
 *                      与源文件格式无关 —— .cr3 导出成 .jpg 就编 JPEG。
 */
export async function saveEditedImage(
  path: string,
  rotationSteps: number,
  aspect: string,
  crop: CropRectInput | null = null,
  targetPath?: string,
): Promise<SaveResult> {
  const result = await invoke<SaveResult>('save_edited_image', {
    path,
    rotationSteps,
    aspect,
    crop,
    targetPath: targetPath ?? null,
  })

  // ⚠️ 只有覆盖保存才需要失效**源**文件的缓存 —— 另存为时原文件的字节没变，
  // 刷新它等于白刷一次，还会把用户刚做的裁剪框冲掉。
  if (!result.exported) {
    invalidateImage(path)
  } else if (targetPath) {
    // ⚠️ 但**目标**文件的缓存必须失效：用户完全可能把图导出到同目录里
    // 一个**已存在**的文件上（那个路径很可能已被预取）。不失效的话，
    // 之后翻到它显示的还是导出前的旧像素（2026-10-11 补）。
    invalidateImage(targetPath)
  }

  return result
}