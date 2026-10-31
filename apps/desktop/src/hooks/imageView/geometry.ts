/**
 * 屏幕坐标 ↔ 图像坐标的换算（纯函数，无 React 依赖）。
 *
 * 两套坐标系必须分清 —— 这一层历史上所有 bug 都是混用它们造成的：
 * - **屏幕 / 容器坐标**：CSS 像素，`tx/ty` 指图像**中心**；
 * - **图像归一化坐标**：`0~1`，相对**旋转后的图**（见 [`CropRect`]）。
 *
 * ⛔ 别用原图宽高去换算 `crop` 的宽高：90°/270° 时两者是交叉的。
 */

import { MIN_CROP_SIZE } from './types'
import type { CropRect } from './types'

/**
 * 旋转任意角度后的**外接矩形**尺寸。
 *
 * 0°/90°/180°/270° 时正好等于原尺寸（浮点误差会被抹掉）；
 * 任意角度时按 `w·|cos| + h·|sin|` 计算。
 */
export function rotatedBounds(
  w: number,
  h: number,
  deg: number,
): { w: number; h: number } {
  const rad = (deg * Math.PI) / 180
  const c = Math.abs(Math.cos(rad))
  const s = Math.abs(Math.sin(rad))
  // 抹掉 ±1e-9 级别误差，避免 90° 被算成 90.0000001
  const bw = Math.round((w * c + h * s) * 1e6) / 1e6
  const bh = Math.round((w * s + h * c) * 1e6) / 1e6
  return { w: bw, h: bh }
}

/**
 * 旋转 90° 整数倍之后，**那张图**的像素尺寸。
 *
 * ## 为什么需要它
 * `crop` 的归一化坐标系是「**旋转之后的图**」—— 后端 `apply_edits` 先按
 * `rotation_steps` 旋转，再用 `crop_normalized(&rotated, …)` 裁剪，坐标就是
 * 相对那张图的（见 `commands/edit.rs::apply_edits` 的文档）。
 *
 * 而屏幕上显示的就是那张图（画布把原图旋转了 `rotation + fineAngle`），
 * 所以屏幕 ↔ `crop` 的换算只需要这一个尺寸 + 残余的细调角。
 *
 * @param rotationSteps 顺时针 90° 的次数（可为负）
 */
export function rotatedImageSize(
  imgW: number,
  imgH: number,
  rotationSteps: number,
): { w: number; h: number } {
  const odd = (((Math.round(rotationSteps) % 4) + 4) % 4) % 2 === 1
  return odd ? { w: imgH, h: imgW } : { w: imgW, h: imgH }
}

/**
 * 屏幕坐标 → **裁剪坐标系**归一化（0~1）。框选的数学核心。
 *
 * ## 坐标系约定（2026-10-11 统一，改之前必读）
 * 归一化的基准是**旋转之后的那张图**（`rotW × rotH`），不是原图 ——
 * 理由见 [`rotatedImageSize`]。
 *
 * 于是映射极其简单：屏幕上看到的就是那张图，只多一个 `fineAngle`
 * （细调角，不参与写回）的残余旋转：
 *
 * ```
 * 屏幕 → 反向旋转 fineAngle → 除以 scale → 按 (rotW, rotH) 归一化
 * ```
 *
 * ⚠️ **历史坑**（2026-10-11 修）：旧版按「**原图**」归一化（除以原图 W/H、
 * 反旋转整个 `deg`），与后端差一个 90° 的**宽高互换**。后果有两个：
 * ① 预置框看着是正方形、存出来却是 0.44 的扁图（框在骗人）；
 * ② 90° 旋转后选固定比例再拖框会**跳变**（当时靠 `screenRatioFor` 硬凑，
 *    那个函数已随本次修正删除）。
 *
 * @param sx,sy      相对容器左上角的屏幕坐标（CSS 像素）
 * @param tx,ty      图像中心在容器中的位置
 * @param scale      当前缩放
 * @param fineAngle  细调角（**不含** 90° 整数倍旋转）
 * @param rotW,rotH  旋转后的图像像素尺寸，见 [`rotatedImageSize`]
 */
export function screenToImageNorm(
  sx: number,
  sy: number,
  tx: number,
  ty: number,
  scale: number,
  fineAngle: number,
  rotW: number,
  rotH: number,
): { x: number; y: number } {
  const rad = (-fineAngle * Math.PI) / 180
  const cos = Math.cos(rad)
  const sin = Math.sin(rad)
  // 相对图像中心的向量
  const dx = sx - tx
  const dy = sy - ty
  // 反向旋转（旋转矩阵的转置）—— 只抵消细调角
  const rx = dx * cos - dy * sin
  const ry = dx * sin + dy * cos
  // 除以缩放得到图像局部坐标，加回半宽高归一化
  return {
    x: rx / scale / rotW + 0.5,
    y: ry / scale / rotH + 0.5,
  }
}

/**
 * 由屏幕上的拖拽矩形算出**裁剪坐标系**的归一化区域（见 [`screenToImageNorm`] 的约定）。
 *
 * **取四角反变换后的外接矩形**：有细调角（`fineAngle`）时，屏幕上的轴对齐矩形
 * 映射回裁剪坐标系后是斜的；取 AABB 是保守近似（可能多含一点边角），
 * 但实现简单、结果稳定，用户也更容易理解「我框住的范围大致就是它」。
 * 90° 整数倍（`fineAngle = 0`）时没有斜角，是**精确**的。
 *
 * 结果会被夹到 `[0, 1]` 并保证 `x+w ≤ 1`、`y+h ≤ 1`。
 * 尺寸小于 [`MIN_CROP_SIZE`] 时返回 `null`（视为误触）。
 */
export function rectToCrop(
  x1: number,
  y1: number,
  x2: number,
  y2: number,
  tx: number,
  ty: number,
  scale: number,
  fineAngle: number,
  rotW: number,
  rotH: number,
): CropRect | null {
  const xs = [x1, x2]
  const ys = [y1, y2]
  let minX = Infinity
  let minY = Infinity
  let maxX = -Infinity
  let maxY = -Infinity
  for (const sx of xs) {
    for (const sy of ys) {
      const p = screenToImageNorm(sx, sy, tx, ty, scale, fineAngle, rotW, rotH)
      if (p.x < minX) minX = p.x
      if (p.y < minY) minY = p.y
      if (p.x > maxX) maxX = p.x
      if (p.y > maxY) maxY = p.y
    }
  }
  // 夹到图像范围内
  minX = Math.max(0, minX)
  minY = Math.max(0, minY)
  maxX = Math.min(1, maxX)
  maxY = Math.min(1, maxY)

  const cw = maxX - minX
  const ch = maxY - minY
  if (cw < MIN_CROP_SIZE || ch < MIN_CROP_SIZE) return null

  return { x: minX, y: minY, w: cw, h: ch }
}

/**
 * 两个归一化裁剪区是否（近似）相同。
 *
 * 供 `crop` 派生 effect 用：视图每动一下都重算 crop，但结果常常没变，
 * 这时必须返回**原对象**（`return v`）而不是无脑 `{ ...v, crop }`，
 * 否则每次 setView 都产生新 state → 多一轮重渲染。
 */
export function sameCrop(a: CropRect | null, b: CropRect | null): boolean {
  if (a === null || b === null) return a === b
  return (
    Math.abs(a.x - b.x) < 1e-6 &&
    Math.abs(a.y - b.y) < 1e-6 &&
    Math.abs(a.w - b.w) < 1e-6 &&
    Math.abs(a.h - b.h) < 1e-6
  )
}

/**
 * 归一化裁剪区域 → 屏幕矩形（[`rectToCrop`] 的逆运算）。
 *
 * 基准是**旋转后的图**（`rotW × rotH`），它在屏幕上是轴对齐的（除 `fineAngle`
 * 这点残余旋转），所以映射就是「乘尺寸 → 转 fineAngle → 平移到 (tx,ty)」。
 * `fineAngle = 0` 时与 [`rectToCrop`] **精确互逆**。
 */
export function cropToScreenRect(
  crop: CropRect,
  tx: number,
  ty: number,
  scale: number,
  fineAngle: number,
  rotW: number,
  rotH: number,
): { left: number; top: number; width: number; height: number } {
  const rad = (fineAngle * Math.PI) / 180
  const cos = Math.cos(rad)
  const sin = Math.sin(rad)
  const corners: Array<[number, number]> = [
    [crop.x, crop.y],
    [crop.x + crop.w, crop.y],
    [crop.x, crop.y + crop.h],
    [crop.x + crop.w, crop.y + crop.h],
  ]
  let minX = Infinity
  let minY = Infinity
  let maxX = -Infinity
  let maxY = -Infinity
  for (const [nx, ny] of corners) {
    const lx = (nx - 0.5) * rotW * scale
    const ly = (ny - 0.5) * rotH * scale
    const rx = lx * cos - ly * sin
    const ry = lx * sin + ly * cos
    const px = tx + rx
    const py = ty + ry
    if (px < minX) minX = px
    if (py < minY) minY = py
    if (px > maxX) maxX = px
    if (py > maxY) maxY = py
  }
  return { left: minX, top: minY, width: maxX - minX, height: maxY - minY }
}
