/**
 * 单图浏览的**视图状态机**。
 *
 * 把「适应窗口 / 缩放 / 平移 / 旋转 / 拖拽」的交互逻辑从组件里剥离出来，
 * 组件只负责渲染与事件绑定。
 *
 * ## 坐标语义（关键）
 * `tx, ty` = **图像中心在屏幕（容器）中的位置**，而非左上角偏移。
 * 画布变换为 `translate(tx,ty) rotate(r) scale(s) translate(-w/2,-h/2)`，
 * 因此缩放/旋转天然绕图像中心进行，旋转后不会跑出画面。
 *
 * ## 性能约束
 * 缩放/平移/旋转全部走 CSS `transform`（GPU 合成），不重绘像素；
 * 拖拽平移用 `requestAnimationFrame` 节流。
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { DecodedImage } from '../ipc'

/** 适应模式 */
export type FitMode = 'none' | 'contain' | 'width' | 'height'

/**
 * 画布比例（裁剪到目标长宽比）。
 *
 * `free` = 保持图片原始比例，不做裁剪；其余把可见区域裁成对应比例。
 * 比例值统一用「宽 / 高」表示，便于直接参与计算。
 */
export type AspectRatio =
  | 'free'
  | '1:1'
  | '3:4'
  | '4:3'
  | '16:9'
  | '9:16'
  | '2.35:1'
  | '2:3'

/** 各比例的宽高比数值（`free` 不在表内，按图片自身比例处理） */
export const ASPECT_VALUES: Record<Exclude<AspectRatio, 'free'>, number> = {
  '1:1': 1,
  '3:4': 3 / 4,
  '4:3': 4 / 3,
  '16:9': 16 / 9,
  '9:16': 9 / 16,
  '2.35:1': 2.35,
  '2:3': 2 / 3,
}

/** 细调角度的取值范围（度） */
export const MIN_FINE_ANGLE = -45
export const MAX_FINE_ANGLE = 45

/**
 * 自由框选的裁剪区域，用**图像归一化坐标**表示（0~1）。
 *
 * ## 为什么用归一化坐标而不是屏幕像素
 * 用户框选时是在**屏幕**上拖，但缩放、平移、旋转、窗口 resize 都会改变
 * 屏幕坐标与图像像素的对应关系。若直接存屏幕像素，用户框完再滚一下滚轮，
 * 框的位置就和「他当时选中的图像内容」对不上了。
 *
 * 归一化坐标锚定的是**图像内容本身**（`0.25` 永远是图像宽度的 1/4 处），
 * 因此任意变换下都稳定，保存时也能直接乘回像素尺寸。
 */
export interface CropRect {
  /** 左边界（0~1，相对图像宽度） */
  x: number
  /** 上边界（0~1，相对图像高度） */
  y: number
  /** 宽度（0~1） */
  w: number
  /** 高度（0~1） */
  h: number
}

/** 框选区域的最小边长（归一化），小于这个值视为误触 */
export const MIN_CROP_SIZE = 0.02

/** 视图状态：缩放倍率 + 图像中心位置 + 旋转角度 + 适应模式 */
export interface ViewState {
  /** 缩放倍率 */
  scale: number
  /** 图像中心在屏幕中的位置（CSS 像素） */
  tx: number
  ty: number
  /**
   * 90° 步进的粗旋转（左转/右转按钮、`[` `]` 键）。
   * 与 [`fineAngle`] 叠加后才是最终视觉角度 ——
   * 分开存是为了让「左转 90°」不会抹掉用户拖的角度滑块。
   */
  rotation: number
  /**
   * 角度微调（-45° ~ +45°），来自调整面板的滚动条。
   * 视觉总角度 = `rotation + fineAngle`。
   */
  fineAngle: number
  /** 裁剪比例 */
  aspect: AspectRatio
  /**
   * 固定比例下**裁剪框的位置与大小**（归一化「视口坐标」）。
   *
   * ## 语义（2026-10-07 第三次定稿，勿再改）
   * 固定比例下画面上始终有一个框：选完比例就是居中的「预置框」（最大），
   * 用户可以
   * - 拖**四角**改大小（宽高比锁死为所选比例，尺寸**限制在视口内**）；
   * - 拖**框内部**平移（整体移动，尺寸不变）。
   * 全程**画面不动**（用户诉求「画面不变，鼠标框变」）。
   *
   * ## 坐标含义
   * `x/y/w/h` 都是**归一化到容器视口**的（0~1，相对容器左上角），
   * 不是相对图像的 —— 因为框的大小由视口决定（预置框 = 视口内该比例最大矩形），
   * 与图像内容解耦。落盘时再用 `rectToCrop` 反变换成图像归一化 `crop`。
   *
   * `null` 表示「用居中的最大预置框」，由 `frameScale` 之外的位置/尺寸默认值决定。
   */
  frameRect: FrameRect | null
  /**
   * 固定比例下**预置框的缩放系数**（0.2 ~ 1），仅在 `frameRect === null` 时生效。
   *
   * `frameScale = 1` 表示铺满容器内该比例的最大矩形。
   * 用户一旦拖过框就写 `frameRect`，此后本字段不再参与。
   *
   * 与 `view.scale`（画面缩放）是**两套独立的东西**：
   * 拖框改的是「取哪一块」，画面缩放改的是「那块内容显示多大」。
   */
  frameScale: number
  /**
   * 鼠标框选出的裁剪区域（图像归一化坐标）；未框选时为 `null`。
   *
   * **自由与固定比例两种模式都会产生它**：
   * - 自由 → 在空白处拖出任意比例的框（松手落成 `crop` 并放大）；
   * - 固定比例 → 由 `frameRect` 经 `rectToCrop` 反变换得来 —— **松手时**落一次，
   *   拖动过程中不更新（画面全程不动，框内高亮那片就是裁剪范围）。
   *
   * ⚠️ 后端 `save_edited_image` **优先用 `crop`、忽略 `aspect`**，
   * 所以这里的 `crop` 一旦存在就是真正的裁剪依据；`aspect` 仅用于面板高亮。
   */
  crop: CropRect | null
  fit: FitMode
}

/** 归一化到容器视口的矩形（0~1），固定比例下框的位置与大小 */
export interface FrameRect {
  x: number
  y: number
  w: number
  h: number
}

/** 固定比例下框的拖拽模式 */
export type FrameDragMode =
  /** 拖四角改大小（比例锁死） */
  | { kind: 'resize'; corner: 'nw' | 'ne' | 'sw' | 'se' }
  /** 拖框内部平移（尺寸不变） */
  | { kind: 'move' }

/** 拖拽状态：只需记录上一次鼠标位置，用增量计算位移 */
interface DragState {
  lastX: number
  lastY: number
}

export const MIN_SCALE = 0.05
export const MAX_SCALE = 32

/** 按钮缩放步进 */
const BUTTON_ZOOM_STEP = 1.25
/** 平移步进（方向键/工具按钮用），CSS 像素 */
const PAN_STEP = 60
/** 滚轮缩放系数 */
const WHEEL_ZOOM_FACTOR = 1.12

const INITIAL_VIEW: ViewState = {
  scale: 1,
  tx: 0,
  ty: 0,
  rotation: 0,
  fineAngle: 0,
  aspect: 'free',
  frameScale: 1,
  frameRect: null,
  crop: null,
  fit: 'contain',
}

/** 裁剪框缩放系数的下限（再小就没有裁剪意义了） */
export const MIN_FRAME_SCALE = 0.2

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
function sameCrop(a: CropRect | null, b: CropRect | null): boolean {
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

/**
 * 在容器内取「最大可达的目标比例矩形」。
 *
 * 若该比例算出来的框比容器还大（例如超宽画幅 `2.35:1` 放进竖屏容器），
 * **不越界**：退回容器本身的比例。否则裁剪框会溢到画面外，
 * 用户看到的框和保存出来的结果对不上。
 */
export function targetViewport(
  containerW: number,
  containerH: number,
  target: number,
): { w: number; h: number } {
  return containerW / containerH > target
    ? { w: containerH * target, h: containerH }
    : { w: containerW, h: containerW / target }
}

/**
 * 固定比例下**默认（预置）框矩形**：容器内该比例的最大矩形，居中。
 *
 * 这是用户还没拖过框时的初始框 —— 「选完比例就看到一个框」，
 * 可以直接拖它的四角改大小、拖内部挪位置（见 [`frameRectToPixels`]）。
 *
 * 在 [`targetViewport`] 基础上乘 `frameScale`：
 * `frameScale = 1` 铺满容器，`0.5` 是一半大小，比例**始终锁定**。
 */
export function cropFrameRect(
  containerW: number,
  containerH: number,
  target: number,
  frameScale: number,
): { w: number; h: number } {
  const base = targetViewport(containerW, containerH, target)
  const s = Math.min(1, Math.max(MIN_FRAME_SCALE, frameScale))
  return { w: base.w * s, h: base.h * s }
}

/**
 * 取固定比例下**当前生效的框**（归一化**视口**坐标，0~1）。
 *
 * `frameRect` 为空时返回居中的预置框 —— 调用方（渲染与落盘）都用这一个入口，
 * 保证「看到的框」就是「将被裁的区域」。
 *
 * ⚠️ **返回的是归一化值**（与 `FrameRect` 契约一致），要像素请再过
 * [`frameRectToPixels`]。这里绝不能直接返回像素 —— 早期版本混用两套坐标，
 * 导致 `frameRectToPixels` 二次乘容器尺寸，框跑到屏幕外（已修）。
 *
 * `area`：预置框要落在哪块区域里（**像素**、容器坐标）。省略 = 整个容器
 * （保持旧行为，测试与历史调用方不受影响）。传「图像可见矩形」时会得到
 * 一个始终压在画面上的框 —— 见 [`activeFrameRect`] 的说明。
 */
export function effectiveFrameRect(
  containerW: number,
  containerH: number,
  target: number,
  frameRect: FrameRect | null,
  frameScale: number,
): FrameRect {
  if (containerW === 0 || containerH === 0) return { x: 0, y: 0, w: 0, h: 0 }
  if (frameRect) return frameRect
  const { w, h } = cropFrameRect(containerW, containerH, target, frameScale)
  return {
    x: (containerW - w) / 2 / containerW,
    y: (containerH - h) / 2 / containerH,
    w: w / containerW,
    h: h / containerH,
  }
}

/**
 * 固定比例下**后端将要裁的那块区域**（归一化，相对**旋转后的图**）。
 *
 * 与 `pv_decode::crop_to_aspect` 一一对应：在**旋转后的整图**上取该比例的
 * 最大矩形并居中。
 *
 * ## 为什么需要它
 * `frameRect === null`（用户没拖过框）时前端**不产出 `crop`**（见 `crop` 派生
 * effect），保存时由后端按这条规则裁。所以框必须画在这块区域的屏幕投影上 ——
 * 否则「看到的框」与「裁出来的」对不上（旧实现是靠**移动画面**让两者重合的，
 * 2026-10-11 用户明确不要那个副作用）。
 *
 * ⚠️ 只按 **90° 步数**算：`fineAngle`（±45° 微调）不参与写回。
 * ⛔ **不要再把 `w`/`h` 和原图宽高交叉换算**：坐标系已经是旋转后的图，
 * 尺寸就是它自己的（`cw/rotW`、`ch/rotH`）。旧版多交叉了一次，导致
 * 90° 下「框是正方形、裁出来是扁的」—— 见 `MEMORY.md` 里那条禁令。
 */
export function aspectCenterCrop(
  target: number,
  image: { width: number; height: number },
  rotationSteps: number,
): CropRect | null {
  if (image.width <= 0 || image.height <= 0 || !(target > 0)) return null
  const { w: rotW, h: rotH } = rotatedImageSize(image.width, image.height, rotationSteps)

  // 该坐标系下能容纳的最大目标比例矩形
  let cw = rotW
  let ch = rotH
  if (rotW / rotH > target) {
    cw = rotH * target
  } else {
    ch = rotW / target
  }
  const w = cw / rotW
  const h = ch / rotH
  return { x: (1 - w) / 2, y: (1 - h) / 2, w, h }
}

/**
 * 把归一化框矩形（相对容器视口）换算成**容器像素矩形**。
 */
export function frameRectToPixels(
  frame: FrameRect,
  containerW: number,
  containerH: number,
): { left: number; top: number; width: number; height: number } {
  return {
    left: frame.x * containerW,
    top: frame.y * containerH,
    width: frame.w * containerW,
    height: frame.h * containerH,
  }
}

/**
 * 图像当前在**画面上可见**的矩形（归一化「容器视口」坐标，0~1）。
 *
 * 两个用途：
 * 1. **自由模式的预置框** —— 选「自由」时框住整张可见的图（初始 = 整图 = 不裁剪）；
 * 2. 拖框时的**可行域** —— 保证框内始终有图像内容。
 *
 * 取旋转后的**外包矩形**（斜角时会略大于实际图像，可接受），再与视口求交并
 * 夹到视口内。图像比视口大 → 结果就是整个视口；图像完全在视口内 → 就是图像本身。
 */
export function imageVisibleRect(
  tx: number,
  ty: number,
  scale: number,
  deg: number,
  imgW: number,
  imgH: number,
  containerW: number,
  containerH: number,
): FrameRect {
  if (containerW === 0 || containerH === 0) return { x: 0, y: 0, w: 1, h: 1 }
  if (imgW === 0 || imgH === 0) return { x: 0, y: 0, w: 1, h: 1 }
  const b = rotatedBounds(imgW, imgH, deg)
  const halfW = (b.w * scale) / 2
  const halfH = (b.h * scale) / 2
  // 图像矩形与视口求交（顺带夹到视口内）
  const left = Math.max(0, Math.min(containerW, tx - halfW))
  const top = Math.max(0, Math.min(containerH, ty - halfH))
  const right = Math.max(0, Math.min(containerW, tx + halfW))
  const bottom = Math.max(0, Math.min(containerH, ty + halfH))
  const w = Math.max(0, right - left)
  const h = Math.max(0, bottom - top)
  return { x: left / containerW, y: top / containerH, w: w / containerW, h: h / containerH }
}

/**
 * 取**当前生效的框**（归一化视口坐标）；没有可用框时返回 `null`。
 *
 * - 用户拖过（`frameRect` 非空）→ 直接用它；
 * - 没拖过 → 用**预置框**：
 *   - 固定比例 → 视口内该比例的最大矩形（居中），见 [`effectiveFrameRect`]；
 *   - 自由 → 图像可见矩形，见 [`imageVisibleRect`]。
 *
 * 渲染（`frameBox`）与拖拽（`onFrameMouseDown`）都走这一个入口，
 * 保证「画面上那个框」与「正在拖的框」永远是同一个。
 */
export function activeFrameRect(
  frameRect: FrameRect | null,
  aspect: AspectRatio,
  containerW: number,
  containerH: number,
  view: Pick<ViewState, 'tx' | 'ty' | 'scale' | 'rotation' | 'fineAngle'>,
  image: DecodedImage | null,
): FrameRect | null {
  if (frameRect) return frameRect
  if (containerW === 0 || containerH === 0) return null
  if (!image) return null

  // 自由模式的预置框 = 整张可见的图（= 不裁剪）
  if (aspect === 'free') {
    return imageVisibleRect(
      view.tx,
      view.ty,
      view.scale,
      view.rotation + view.fineAngle,
      image.width,
      image.height,
      containerW,
      containerH,
    )
  }

  /**
   * 固定比例、用户还没拖过框：前端不产出 `crop`，保存时由后端在
   * **旋转后的整图**上按比例居中裁（`crop_to_aspect`）。
   *
   * 所以这里把**那块区域**投影到屏幕上当作框 —— 「看到的框」与「裁出来的」
   * 始终一致。旧实现是反过来做的（把画面缩放平移到框上），
   * 代价是换个比例就把用户的构图挪走；2026-10-11 用户明确要求画面不动。
   *
   * ⚠️ 形状用 **90° 步数**算（与后端一致，`fineAngle` 不参与写回），
   * 投影到屏幕才带上 `fineAngle`（画面当前确实斜着）。
   * 投影的基准尺寸是**旋转后的图**（`rotatedImageSize`），不是原图 ——
   * 这样 90° 时框的屏幕比恰好等于裁剪结果比。
   */
  const rotSteps = Math.round(view.rotation / 90)
  const crop = aspectCenterCrop(ASPECT_VALUES[aspect], image, rotSteps)
  if (!crop) return null
  const { w: rotW, h: rotH } = rotatedImageSize(image.width, image.height, rotSteps)
  const r = cropToScreenRect(crop, view.tx, view.ty, view.scale, view.fineAngle, rotW, rotH)
  return {
    x: r.left / containerW,
    y: r.top / containerH,
    w: r.width / containerW,
    h: r.height / containerH,
  }
}

/**
 * 把屏幕拖拽的起止点**约束成 `screenTarget` 比例的矩形**（屏幕坐标）。
 *
 * ## `screenTarget` 就是目标比例本身（2026-10-11 起）
 * 因为 `crop` 的坐标系现在是**旋转后的图**，而屏幕上显示的就是那张图，
 * 所以「屏幕矩形比」与「裁剪结果比」**恒等** —— 任何角度下都直接传
 * `ASPECT_VALUES[aspect]`，不需要再换算。
 *
 * ⚠️ 旧版在 90°/270° 要传 `(H/W)²/target`（`screenRatioFor`）来抵消坐标系里
 * 少做的一次宽高交叉。那个函数已随坐标系统一删除；**别把它加回来** ——
 * 加了会让 90° 旋转后拖框时比例突然跳变。
 *
 * ## 取值规则
 * 以 `start` 为锚点（拖拽起点固定在框的一角），取拖动方向为符号，
 * 用两轴位移中**较大**的那个决定尺寸（避免斜拖时抖动），
 * 另一端按比例推出来，保证「拖出来的框」严格满足 `screenTarget`。
 */
export function constrainRectToRatio(
  startX: number,
  startY: number,
  curX: number,
  curY: number,
  screenTarget: number,
): { x1: number; y1: number; x2: number; y2: number } {
  const dx = curX - startX
  const dy = curY - startY
  const signX = dx < 0 ? -1 : 1
  const signY = dy < 0 ? -1 : 1
  // 宽按 |dx|、高按 |dx|/ratio 起算；若 |dy| 更大则改由高度反推宽度
  let w = Math.abs(dx)
  let h = w / screenTarget
  if (Math.abs(dy) > h) {
    h = Math.abs(dy)
    w = h * screenTarget
  }
  return {
    x1: startX,
    y1: startY,
    x2: startX + signX * w,
    y2: startY + signY * h,
  }
}

/**
 * 计算「适应窗口」的缩放与居中位置（纯函数）。
 *
 * 返回的 `tx/ty` 是**容器中心**——即把图像中心放到窗口中心。
 *
 * ## 三种「目标视口」的取法
 *
 * 1. **固定比例**（1:1 / 16:9 / …）→ **当前框**（`frameRect`，含用户拖出来的
 *    位置与大小；未拖过则为居中的预置框），**覆盖模式**：
 *    画面必须放大到把该矩形填满（旋转后也不许露黑角）。
 * 2. **自由 + 已框选** → 目标视口就是框选区（换算成容器坐标），
 *    同样覆盖模式。这样用户框完那一刻，视图会自己放大到「只剩框里那块」，
 *    所见即所得地预览裁剪结果。
 * 3. **自由 + 未框选** → 目标视口 = 整个容器，按 `contain` 把整图放进来。
 *
 * ⚠️ 固定比例下 `tx/ty` 还要把**框的中心**对到容器中心（框可能被用户拖偏），
 * 不只是图像中心对齐 —— 否则覆盖放大后画面会跑偏。
 */
export function computeFitFor(
  rotation: number,
  fit: FitMode,
  image: DecodedImage | null,
  containerW: number,
  containerH: number,
  fineAngle = 0,
  aspect: AspectRatio = 'free',
  crop: CropRect | null = null,
  frameScale = 1,
  frameRect: FrameRect | null = null,
): { scale: number; tx: number; ty: number } | null {
  if (!image || containerW === 0 || containerH === 0) return null

  const deg = rotation + fineAngle

  // ── 目标视口 + 缩放基准尺寸 ──
  let viewW = containerW
  let viewH = containerH
  /** 覆盖模式（必须放大到填满视口，不允许留白） */
  let wantCover = false
  /**
   * 参与「填满视口」计算的**屏幕尺寸**。
   *
   * ⚠️ 关键：`scale` 作用在**旋转前**的画布上，而 `viewW/viewH` 是**屏幕**（旋转后）
   * 的视口尺寸。两者必须同处一个坐标系才能比 —— 所以这里存的是
   * 「该区域旋转之后在屏幕上占据的宽高」，正交角时即为宽高互换。
   * 早期版本直接用未旋转尺寸做 `max(viewW/fillW, ...)`，
   * 90° 时会把 `W` 和 `H` 用反，覆盖倍数差 `(W/H)²`（本机样本 2.25 倍）。
   */
  let fillW: number
  let fillH: number
  /** 该区域在**屏幕**上的宽高（= fillW/fillH 经 rot90 互换） */
  let screenW: number
  let screenH: number
  /** 固定比例下框在容器里的像素矩形（用于把框中心对到容器中心） */
  let framePx: { left: number; top: number; width: number; height: number } | null = null

  if (aspect !== 'free') {
    // 固定比例：视口 = 当前框（用户拖过就按用户的位置/大小）
    const fr = effectiveFrameRect(containerW, containerH, ASPECT_VALUES[aspect], frameRect, frameScale)
    framePx = frameRectToPixels(fr, containerW, containerH)
    viewW = framePx.width
    viewH = framePx.height
    wantCover = true
    // 90° 整数倍时用原尺寸更精确（外包矩形在正交角度下等于原尺寸）
    const rot90 = Math.abs(((deg % 180) + 180) % 180 - 90) < 1e-9
    const b = rotatedBounds(image.width, image.height, deg)
    fillW = rot90 ? image.height : b.w
    fillH = rot90 ? image.width : b.h
    screenW = fillW
    screenH = fillH
  } else if (crop) {
    // 自由框选：`crop` 的坐标系是**旋转后的图**（2026-10-11 统一，见
    // `screenToImageNorm`），所以它的物理尺寸直接由旋转后尺寸给出；
    // 而 `scale` 作用在**旋转前**的画布上，需要把两者交叉回来。
    const rotSteps = Math.round(rotation / 90)
    const odd = (((rotSteps % 4) + 4) % 4) % 2 === 1
    const rs = rotatedImageSize(image.width, image.height, rotSteps)
    // 屏幕上占的宽高（与 viewW/viewH 同坐标系）
    screenW = crop.w * rs.w
    screenH = crop.h * rs.h
    // 旋转前画布上的宽高（`scale` 作用于此）
    fillW = (odd ? crop.h : crop.w) * image.width
    fillH = (odd ? crop.w : crop.h) * image.height
    const v = targetViewport(containerW, containerH, screenW / screenH)
    viewW = v.w
    viewH = v.h
    wantCover = true
  } else {
    // 自由比例 = 「把整张图放进窗口」。
    //
    // ## 这里要按角度分两种情况（2026-10-07 二次修正）
    //
    // ① **正交角**（0/90/180/270°）：旋转后画面严格等于 `h×w` 的盒子，
    //    此时必须用**旋转后**的尺寸去做 contain —— 否则 90° 旋转会把
    //    横图变成「超出画布上下的竖图」，用户点一下「顺时针 90°」就会看到
    //    图片上下被切掉（用户原始诉求：「旋转后图片要刷新到合适窗口大小」）。
    // ② **任意角**（含细调 ±45°）：外包矩形会显著变大（5847×3898 转 45°
    //    → 6891×6891），若拿它 contain 会把画面缩到「刚好塞下那个正方形」，
    //    表现为「一旋转画面就变成 1:1」。所以这里**必须用未旋转尺寸** ——
    //    旋转只是绕图像中心的预览操作，显示尺寸不该因斜角而改变，
    //    转出去的四个角超出视口是正常的（用户可自行平移查看）。
    const rot90 = Math.abs(((deg % 180) + 180) % 180 - 90) < 1e-9
    fillW = rot90 ? image.height : image.width
    fillH = rot90 ? image.width : image.height
    screenW = fillW
    screenH = fillH
  }

  let scale: number
  switch (fit) {
    case 'contain':
      // 覆盖模式用**屏幕尺寸**比视口（两者同坐标系）；
      // 非覆盖模式沿用未旋转尺寸（自由比例下 fillW/fillH 已按角度调好）
      scale = wantCover
        ? Math.max(viewW / (crop ? screenW : fillW), viewH / (crop ? screenH : fillH))
        : Math.min(viewW / fillW, viewH / fillH, 1)
      break
    case 'width':
      scale = wantCover
        ? Math.max(viewW / (crop ? screenW : fillW), viewH / (crop ? screenH : fillH))
        : viewW / fillW
      break
    case 'height':
      scale = wantCover
        ? Math.max(viewW / (crop ? screenW : fillW), viewH / (crop ? screenH : fillH))
        : viewH / fillH
      break
    case 'none':
      return null
  }
  scale = Math.min(MAX_SCALE, Math.max(MIN_SCALE, scale))

  // ── 定位 ──
  // 目标区域在屏幕上未必居中（用户可能框在角落、或把固定比例的框拖偏），
  // 所以不能简单返回容器中心，必须让**该区域自己的中心**落到容器中心。
  //
  // ⚠️ 固定比例下「区域」是**框**（framePx，容器坐标），不随图像旋转；
  //    像素内容需要额外补偿旋转后的偏移。
  if (framePx) {
    // 框在容器里的中心；图像中心要对到它
    const fx = framePx.left + framePx.width / 2
    const fy = framePx.top + framePx.height / 2
    return { scale, tx: fx, ty: fy }
  }

  if (!crop) return { scale, tx: containerW / 2, ty: containerH / 2 }

  // 裁剪区中心在屏幕上的偏移：`crop` 的坐标系是旋转后的图，先按它自己的
  // 尺寸取局部坐标，再抵消残余的细调角（90° 整数倍已在 rotW/rotH 里）。
  const rad = (fineAngle * Math.PI) / 180
  const cos = Math.cos(rad)
  const sin = Math.sin(rad)
  const rs = rotatedImageSize(image.width, image.height, Math.round(rotation / 90))
  const lx = (crop.x + crop.w / 2 - 0.5) * rs.w * scale
  const ly = (crop.y + crop.h / 2 - 0.5) * rs.h * scale
  return {
    scale,
    tx: containerW / 2 - (lx * cos - ly * sin),
    ty: containerH / 2 - (lx * sin + ly * cos),
  }
}

export interface UseImageView {
  view: ViewState
  /** 拖拽平移中（用于切换光标） */
  dragging: boolean
  /**
   * 裁剪框的屏幕矩形（容器 CSS 像素）；无图 / 尺寸未就绪时为 `null`。
   *
   * **两种模式都有框**（2026-10-08 统一）：没拖过时显示的是**预置框**
   * （固定比例 = 视口内该比例的最大矩形；自由 = 图像可见矩形，初始即整图），
   * 拖过之后就是用户定下的那个。拖四角改大小、拖内部平移，**全程画面不动**。
   */
  frameBox: { left: number; top: number; width: number; height: number } | null
  /** 正在拖固定比例的框（改大小或平移），用于切换光标 */
  frameDragging: FrameDragMode | null
  /**
   * 开始拖固定比例的框（拖四角改大小 / 拖内部平移）。
   * `aspect === 'free'` 时是空操作。
   */
  onFrameMouseDown: (mode: FrameDragMode) => (e: React.MouseEvent) => void
  /** 放大到 150%+ 时用 pixelated 保持像素锐利 */
  isPixelated: boolean
  /** 量取容器尺寸用，绑定到画布容器元素 */
  containerRef: React.RefObject<HTMLDivElement>
  onWheel: (e: React.WheelEvent) => void
  onDoubleClick: () => void
  /** 直接设定缩放（以视口中心为锚点） */
  setZoom: (scale: number) => void
  /** 设定细调角度（度，会被夹到 ±45），并重算适应 */
  setFineAngle: (deg: number) => void
  /** 设定裁剪比例，并重算适应（会清掉自由框选、固定比例的框回到预置位置） */
  setAspect: (aspect: AspectRatio) => void
  /** 清掉自由框选的裁剪区（不动角度与比例），并把视图退回整图适应 */
  clearCrop: () => void
  /** 把旋转、角度、比例与框选一并复位，并回到整图适应 */
  resetAdjust: () => void
  /**
   * 取当前视图状态的一份**快照**（含 scale/tx/ty/角度/比例/框选）。
   *
   * 用途：打开画布调整面板时先存下来，若用户没点「覆盖原图」就关闭面板，
   * 用 [`restoreView`] 原样还原 —— 这样「退出调整」= 「放弃这次调整」
   * （2026-10-07 用户要求：「如果调整没有覆盖原图，退出调整后应恢复调整前的显示」）。
   * 是**深拷贝**，之后视图继续变化不会影响快照。
   */
  snapshotView: () => ViewState
  /** 用 [`snapshotView`] 的快照还原视图（含缩放平移，逐字段覆盖） */
  restoreView: (snap: ViewState) => void
}

/**
 * 视图状态机 hook。
 *
 * @param image 当前解码图（换图会重置旋转并按 `contain` 适应窗口）
 */
export function useImageView(image: DecodedImage | null): UseImageView {
  const containerRef = useRef<HTMLDivElement>(null)
  const [view, setView] = useState<ViewState>(INITIAL_VIEW)
  const [dragging, setDragging] = useState(false)
  const dragState = useRef<DragState | null>(null)
  // 拖拽时用 ref 累积增量，避免每个 mousemove 都触发重渲染
  const pendingDelta = useRef<{ dx: number; dy: number } | null>(null)
  const rafId = useRef<number | null>(null)
  /** 上次滚轮翻页时间，用于冷却 */
  /** 保持最新的导航回调，避免 effect 依赖抖动 */

  /** 正在拖裁剪框（改大小/平移），`null` = 没在拖 */
  const [frameDragging, setFrameDragging] = useState<FrameDragMode | null>(null)
  /** 容器尺寸镜像（给 `frameBox` 派生用；由渲染帧写入，避免额外 ResizeObserver） */
  const [containerSize, setContainerSize] = useState({ w: 0, h: 0 })
  /** 用 ref 读最新的 view，避免框选 effect 因 view 变化反复重建监听 */
  const viewRef = useRef(view)
  viewRef.current = view

  /** 计算适应窗口的缩放倍率与居中位置（依赖当前旋转角/细调角/比例/框选/框缩放/框矩形） */
  const computeFit = useCallback(
    (fit: FitMode, containerW: number, containerH: number) =>
      computeFitFor(
        view.rotation,
        fit,
        image,
        containerW,
        containerH,
        view.fineAngle,
        view.aspect,
        view.crop,
        view.frameScale,
        view.frameRect,
      ),
    [
      image,
      view.rotation,
      view.fineAngle,
      view.aspect,
      view.crop,
      view.frameScale,
      view.frameRect,
    ],
  )

  // 换图时重置旋转与调整参数，按 contain 适应窗口；容器尺寸变化时跟随重算
  useEffect(() => {
    const el = containerRef.current
    if (!el || !image) return

    const syncSize = () => {
      const r = el.getBoundingClientRect()
      setContainerSize((prev) =>
        prev.w === r.width && prev.h === r.height ? prev : { w: r.width, h: r.height },
      )
    }

    const fitTo = (fitMode: FitMode) => {
      const rect = el.getBoundingClientRect()
      const fit = computeFitFor(0, fitMode, image, rect.width, rect.height, 0, 'free', null, 1)
      // 新图：旋转、细调角、比例、框缩放、框矩形、框选一并复位
      if (fit) {
        setView((v) => ({
          ...v,
          ...fit,
          fit: fitMode,
          rotation: 0,
          fineAngle: 0,
          aspect: 'free',
          frameScale: 1,
          frameRect: null,
          crop: null,
        }))
      }
    }

    fitTo('contain')
    syncSize()

    const ro = new ResizeObserver(() => {
      syncSize()
      // 只有处于适应模式时才跟随容器变化重算
      setView((v) => {
        if (v.fit === 'none') return v
        const rect = el.getBoundingClientRect()
        const fit = computeFitFor(
          v.rotation,
          v.fit,
          image,
          rect.width,
          rect.height,
          v.fineAngle,
          v.aspect,
          v.crop,
          v.frameScale,
          v.frameRect,
        )
        return fit ? { ...v, ...fit } : v
      })
    })
    ro.observe(el)
    return () => ro.disconnect()
  }, [image])

  /**
   * 画布上的滚轮 —— **只处理 Ctrl/⌘ + 滚轮（缩放）**。
   *
   * ⚠️ 普通滚轮的「翻页」不在这里，而是窗口级的 [`useWheelNavigation`]。
   * 原因：画布在**解码失败时会被 `<EmptyState>` 替换掉**（整个组件被卸载），
   * 挂在这里的监听会一起消失 —— 用户遇到坏图就滚不动、翻不了页。
   * 翻页是查看器的整体行为，不能依赖画布是否在渲染。
   *
   * 缩放保留在画布：它以**光标在画布内的坐标**为锚点，且无图时没有意义。
   */
  const onWheel = useCallback(
    (e: React.WheelEvent) => {
      if (!image) return
      // 普通滚轮放行（冒泡到 window 的翻页监听），不 preventDefault
      if (!e.ctrlKey && !e.metaKey) return
      e.preventDefault()

      // 缩放：以光标位置为锚点（保持光标下的图像点不动）
      const el = containerRef.current
      if (!el) return
      const rect = el.getBoundingClientRect()
      const cx = e.clientX - rect.left
      const cy = e.clientY - rect.top

      setView((v) => {
        const factor = e.deltaY < 0 ? WHEEL_ZOOM_FACTOR : 1 / WHEEL_ZOOM_FACTOR
        const newScale = Math.min(MAX_SCALE, Math.max(MIN_SCALE, v.scale * factor))
        const ratio = newScale / v.scale
        return {
          ...v,
          scale: newScale,
          tx: cx - (cx - v.tx) * ratio,
          ty: cy - (cy - v.ty) * ratio,
          fit: 'none',
        }
      })
    },
    [image],
  )

  /**
   * 开始拖裁剪框：**拖四角改大小** / **拖框内部平移**（**两种模式共用**）。
   *
   * 2026-10-08 统一：自由与固定比例不再是两套交互，都是「画面上有个框，
   * 拖角改大小、拖内部平移」。区别只有一处 —— 固定比例改大小时宽高比锁死。
   *
   * 直接在事件里现场挂 window 监听（不用 effect）—— 拖拽几何走的是闭包，
   * 放 effect 里会因为派发时机不对而挂不上监听。
   *
   * 约束：
   * - 固定比例改大小时宽高比**锁死**为目标比例（`ASPECT_VALUES[aspect]`）；
   *   自由比例不锁，鼠标到哪就是哪。
   * - 框**限制在「视口 ∩ 图像可见范围」内**（用户要求不许拖出可见区；
   *   再叠加图像范围，是为保证框内始终有内容、所见即所得）；
   * - 平移时尺寸不变，只挪位置，同样不越界。
   *
   * 全程**画面不动**：只改 `frameRect`；`crop` 由派生 effect 统一维护。
   */
  const onFrameMouseDown = useCallback(
    (mode: FrameDragMode) => (e: React.MouseEvent) => {
      if (e.button !== 0) return
      // 阻止冒泡，避免画布上的其他按下逻辑插手
      e.stopPropagation()
      e.preventDefault()
      const el = containerRef.current
      const v = viewRef.current
      if (!el || !image) return

      const cw = el.clientWidth
      const ch = el.clientHeight
      if (cw === 0 || ch === 0) return

      /**
       * 自由比例不锁宽高比；固定比例锁死为所选比例。
       *
       * ⚠️ 这里传的**就是目标比例本身**（不是「屏幕比」）—— 因为 `crop` 与屏幕
       * 同处「旋转后的图」这一个坐标系，两者宽高比恒等。见
       * [`constrainRectToRatio`] 的说明（旧版 90° 时传 `(H/W)²/target`，
       * 那是坐标系没统一时的补丁，会让旋转后拖框比例跳变）。
       */
      const lockRatio = v.aspect === 'free' ? null : ASPECT_VALUES[v.aspect]

      /** 当前框：拖过就是它，没拖过就是预置框（两种模式都有） */
      const startFrame = activeFrameRect(v.frameRect, v.aspect, cw, ch, v, image)
      if (!startFrame) return
      const rect = el.getBoundingClientRect()
      const px = e.clientX - rect.left
      const py = e.clientY - rect.top

      /**
       * 框的**可行域** = 视口 ∩ 图像可见矩形（就是 [`imageVisibleRect`]）。
       *
       * ⚠️ 光限制视口不够（2026-10-08 补）：画面可能是「按预置框 cover」的，
       * 横向图选 1:1 时图像往往只占视口中间一条，两侧留背景。若允许框拖到
       * 背景区，框内就是空的；而且 `rectToCrop` 会把结果 clamp 回图像范围 ——
       * 于是「看到的框」≠「裁出来的区域」，违背所见即所得。
       * 夹到图像范围后，框内**始终有图像内容**。
       *
       * （图像比视口大时交集就是视口，与「限制在视口内」等价。）
       */
      const vr = imageVisibleRect(
        v.tx,
        v.ty,
        v.scale,
        v.rotation + v.fineAngle,
        image.width,
        image.height,
        cw,
        ch,
      )
      const bounds = {
        left: vr.x * cw,
        top: vr.y * ch,
        right: (vr.x + vr.w) * cw,
        bottom: (vr.y + vr.h) * ch,
      }

      setFrameDragging(mode)

      const onMove = (ev: MouseEvent) => {
        const r2 = containerRef.current?.getBoundingClientRect()
        if (!r2) return
        const mx = ev.clientX - r2.left
        const my = ev.clientY - r2.top
        // 屏幕像素的框（拖动期间实时算，不依赖 state）
        const box = frameRectToPixels(startFrame, cw, ch)
        let next: FrameRect

        if (mode.kind === 'move') {
          // 平移：整体挪，尺寸不变
          const nx = box.left + (mx - px)
          const ny = box.top + (my - py)
          next = {
            x: Math.min(Math.max(nx, bounds.left), bounds.right - box.width) / cw,
            y: Math.min(Math.max(ny, bounds.top), bounds.bottom - box.height) / ch,
            w: box.width / cw,
            h: box.height / ch,
          }
        } else {
          // 改大小：抓的是哪个角，就固定它**对角**那个顶点不动
          const anchors = {
            se: { ax: box.left, ay: box.top },
            sw: { ax: box.left + box.width, ay: box.top },
            ne: { ax: box.left, ay: box.top + box.height },
            nw: { ax: box.left + box.width, ay: box.top + box.height },
          } as const
          const { ax, ay } = anchors[mode.corner]
          // 鼠标作为「动的那个角」：
          // - 固定比例 → 按目标比例约束，宽高比锁死；
          // - 自由比例 → 鼠标到哪就是哪，不约束。
          let left: number
          let top: number
          let w: number
          let h: number
          if (lockRatio === null) {
            left = Math.min(ax, mx)
            top = Math.min(ay, my)
            w = Math.abs(mx - ax)
            h = Math.abs(my - ay)
          } else {
            const r = constrainRectToRatio(ax, ay, mx, my, lockRatio)
            left = Math.min(r.x1, r.x2)
            top = Math.min(r.y1, r.y2)
            w = Math.abs(r.x2 - r.x1)
            h = Math.abs(r.y2 - r.y1)
          }

          // 限制在可行域内：装不下就整体缩到能装下（保持比例，锚点就近）
          const maxW = mode.corner === 'nw' || mode.corner === 'sw' ? ax - bounds.left : bounds.right - ax
          const maxH = mode.corner === 'nw' || mode.corner === 'ne' ? ay - bounds.top : bounds.bottom - ay
          const fit = Math.max(0, Math.min(maxW / w, maxH / h, 1))
          w *= fit
          h *= fit
          if (mode.corner === 'nw' || mode.corner === 'sw') left = ax - w
          if (mode.corner === 'nw' || mode.corner === 'ne') top = ay - h
          // 夹进可行域（浮点误差兜底）
          left = Math.min(Math.max(left, bounds.left), bounds.right - w)
          top = Math.min(Math.max(top, bounds.top), bounds.bottom - h)

          next = { x: left / cw, y: top / ch, w: w / cw, h: h / ch }
        }

        // 实时更新 frameRect 供渲染；`crop` 由派生 effect 统一维护
        setView((prev) => ({ ...prev, frameRect: next }))
      }

      const onUp = () => {
        setFrameDragging(null)
        window.removeEventListener('mousemove', onMove)
        window.removeEventListener('mouseup', onUp)
        // ⚠️ 松手**什么都不做** —— 尤其别再调 `computeFitFor` 重算视图，
        //    那会让画面「一松手就缩放」（用户明确报过的 bug）。
        //    只点击、没拖动时 `frameRect` 仍是 `null`，于是「没拖过 = 不裁剪」
        //    的语义得以保持；`crop` 全权交给派生 effect 维护。
      }

      window.addEventListener('mousemove', onMove)
      window.addEventListener('mouseup', onUp)
    },
    [image],
  )

  /**
   * `crop` 由「框 + 当前视图」**派生**（不要在拖拽时手写）。
   *
   * ## 规则（2026-10-08 定稿，两种模式统一）
   * - **没拖过框（`frameRect === null`）→ `crop = null` = 不裁剪。**
   *   这样「只放大看细节、没动过框」不会把可见区域误存成裁剪区；
   *   固定比例下后端拿到 `crop = null` 会按 `aspect` 居中裁剪，
   *   而居中的预置框正好就是那个范围 —— 两者一致。
   * - **拖过框 → `crop` = 框内那片图像内容**（用当前视图反变换）。
   *   视图变化（缩放/平移/旋转/窗口 resize）会改变框内看到的内容，
   *   所以这里跟着重算，保证「框里看到的」= 「将被裁掉的」。
   *
   * ⚠️ 不要再改成「拖拽时手写 crop」：那样视图一变 crop 就过期了。
   */
  useEffect(() => {
    if (!image) return
    const el = containerRef.current
    if (!el) return
    const cw = el.clientWidth
    const ch = el.clientHeight
    if (cw === 0 || ch === 0) return

    setView((v) => {
      if (!v.frameRect) return v.crop === null ? v : { ...v, crop: null }
      const box = frameRectToPixels(v.frameRect, cw, ch)
      // 归一化基准是**旋转后的图**（后端先旋转再裁剪），所以传旋转后尺寸 +
      // 残余的细调角，而不是原图尺寸 + 总角度。详见 `screenToImageNorm`。
      const { w: rotW, h: rotH } = rotatedImageSize(
        image.width,
        image.height,
        Math.round(v.rotation / 90),
      )
      const next = rectToCrop(
        box.left,
        box.top,
        box.left + box.width,
        box.top + box.height,
        v.tx,
        v.ty,
        v.scale,
        v.fineAngle,
        rotW,
        rotH,
      )
      if (sameCrop(v.crop, next)) return v
      return { ...v, crop: next }
    })
  }, [
    image,
    view.frameRect,
    view.tx,
    view.ty,
    view.scale,
    view.rotation,
    view.fineAngle,
    containerSize,
  ])

  // 拖拽平移（rAF 节流）
  useEffect(() => {
    if (!dragging) return

    const flush = () => {
      rafId.current = null
      const d = pendingDelta.current
      pendingDelta.current = null
      if (!d) return
      setView((v) => ({ ...v, tx: v.tx + d.dx, ty: v.ty + d.dy }))
    }

    const handleMove = (e: MouseEvent) => {
      const s = dragState.current
      if (!s) return
      const dx = e.clientX - s.lastX
      const dy = e.clientY - s.lastY
      s.lastX = e.clientX
      s.lastY = e.clientY
      if (dx === 0 && dy === 0) return

      if (pendingDelta.current) {
        pendingDelta.current.dx += dx
        pendingDelta.current.dy += dy
      } else {
        pendingDelta.current = { dx, dy }
      }
      if (rafId.current === null) {
        rafId.current = requestAnimationFrame(flush)
      }
    }

    const handleUp = () => {
      setDragging(false)
      dragState.current = null
      pendingDelta.current = null
      if (rafId.current !== null) {
        cancelAnimationFrame(rafId.current)
        rafId.current = null
      }
    }

    window.addEventListener('mousemove', handleMove)
    window.addEventListener('mouseup', handleUp)
    return () => {
      window.removeEventListener('mousemove', handleMove)
      window.removeEventListener('mouseup', handleUp)
      if (rafId.current !== null) cancelAnimationFrame(rafId.current)
    }
  }, [dragging])

  /** 双击切换 100% / 适应窗口 */
  const onDoubleClick = useCallback(() => {
    const el = containerRef.current
    if (!el || !image) return
    const rect = el.getBoundingClientRect()

    setView((v) => {
      if (v.fit === 'none' && Math.abs(v.scale - 1) < 0.01) {
        const fit = computeFit('contain', rect.width, rect.height)
        return fit ? { ...v, ...fit, fit: 'contain' } : v
      }
      // 从适应状态到 100%，并居中
      return {
        ...v,
        scale: 1,
        tx: rect.width / 2,
        ty: rect.height / 2,
        fit: 'none',
      }
    })
  }, [image, computeFit])

  /** 直接设定缩放（以视口中心为锚点） */
  const setZoom = useCallback((scale: number) => {
    const el = containerRef.current
    if (!el) return
    const rect = el.getBoundingClientRect()
    const cx = rect.width / 2
    const cy = rect.height / 2
    const clamped = Math.min(MAX_SCALE, Math.max(MIN_SCALE, scale))

    setView((v) => {
      const ratio = clamped / v.scale
      return {
        ...v,
        scale: clamped,
        tx: cx - (cx - v.tx) * ratio,
        ty: cy - (cy - v.ty) * ratio,
        fit: 'none',
      }
    })
  }, [])

  /**
   * 旋转 90°。
   *
   * **总是重新按 `contain` 适应窗口**（2026-10-07 用户要求「旋转过后需要大小自适应」）：
   * 90° 会让宽高互换，若沿用原缩放，横图转竖后必然溢出容器、
   * 竖图转横后又会缩在中间很空。所以这里不看当前 `fit`，一律重算。
   *
   * **不改 `fineAngle`** —— 粗旋转与角度微调是两个独立维度，
   * 用户调好 ±10° 再点「左转」时，那 10° 应该保留。
   */
  const rotate = useCallback(
    (delta: number) => {
      setView((v) => {
        const newRotation = (((v.rotation + delta) % 360) + 360) % 360
        const el = containerRef.current
        if (!el) return { ...v, rotation: newRotation }
        const rect = el.getBoundingClientRect()
        const fit = computeFitFor(
          newRotation,
          'contain',
          image,
          rect.width,
          rect.height,
          v.fineAngle,
          v.aspect,
          v.crop,
          v.frameScale,
          v.frameRect,
        )
        return fit
          ? {
              ...v,
              rotation: newRotation,
              ...fit,
              // 适应后 fit 模式也应回到 contain，否则滚轮/窗口变化时行为不一致
              fit: 'contain',
            }
          : { ...v, rotation: newRotation }
      })
    },
    [image],
  )

  /** 设定细调角度（夹到 ±45°），适应模式下重算缩放 */
  const setFineAngle = useCallback((deg: number) => {
    const clamped = Math.min(MAX_FINE_ANGLE, Math.max(MIN_FINE_ANGLE, deg))
    setView((v) => {
      const el = containerRef.current
      if (!el) return { ...v, fineAngle: clamped }
      const rect = el.getBoundingClientRect()
      const fit = computeFitFor(
        v.rotation,
        v.fit,
        image,
        rect.width,
        rect.height,
        clamped,
        v.aspect,
        v.crop,
        v.frameScale,
        v.frameRect,
      )
      return fit
        ? { ...v, fineAngle: clamped, scale: fit.scale, tx: fit.tx, ty: fit.ty }
        : { ...v, fineAngle: clamped }
    })
  }, [image])

  /**
   * 设定裁剪比例：**只换框，画面一动不动**。
   *
   * ## 为什么不再重算视图（2026-10-11 用户要求）
   * 旧实现会跑一遍 `computeFitFor(..., aspect, ...)` 把 scale/tx/ty 推成
   * 「按新比例的框 cover 整张图」—— 效果是**选个比例画面就自己缩放/平移**。
   * 用户的原话是「选取了比例后，画面还是保持原来的大小，不需要调整适应」：
   * 他正在取景，换个比例框不该把他的构图挪走。
   *
   * 所以现在只做三件事：换 `aspect`、清掉旧框（回到预置框）、`frameScale` 归 1。
   * 预置框落在**画面上图像占的区域**内（见 [`activeFrameRect`]），
   * 因此不需要靠移动画面来让框有意义。
   *
   * ⚠️ 清 `frameRect`/`crop` 是必须的：旧框的比例与大小对新比例不再适用。
   * 清掉后 `frameRect === null` → 走预置框 → 也等于「没有裁剪」这条既有契约。
   */
  const setAspect = useCallback((aspect: AspectRatio) => {
    setView((v) => ({
      ...v,
      aspect,
      crop: null as CropRect | null,
      frameRect: null as FrameRect | null,
      frameScale: 1,
    }))
  }, [])

  /**
   * 裁剪框的屏幕矩形（容器 CSS 像素）；**两种模式都有**。
   *
   * ⚠️ 必须在**渲染前**同步算出来（`ImageCanvas` 直接拿它定位框），
   * 所以用 `containerSize`（由渲染帧写入的容器尺寸镜像）而非读 DOM。
   * 尺寸未就绪 / 无图时返回 `null`。
   *
   * 走 [`activeFrameRect`] —— 与拖拽用的是同一个入口，
   * 保证「画面上那个框」就是「正在拖的框」。
   * 注意自由模式的预置框依赖当前视图（图像可见矩形），
   * 所以依赖里必须带上 `view` 的几何字段。
   */
  const frameBox = useMemo(() => {
    const { w: cw, h: ch } = containerSize
    if (cw === 0 || ch === 0) return null
    const fr = activeFrameRect(view.frameRect, view.aspect, cw, ch, view, image)
    return fr ? frameRectToPixels(fr, cw, ch) : null
  }, [view, containerSize, image])

  /**
   * 清掉裁剪框（不动角度与比例），配合「清除」「复位」与切换比例使用。
   *
   * 清的是 `frameRect`（回到预置框）—— 两种模式都这样。
   * `crop` 不必手写：没拖过框时派生 effect 会把它置回 `null`（= 不裁剪）。
   * 清完视图退回「适应」，否则会停留在只显示框内那一块的构图里。
   */
  const clearCrop = useCallback(() => {
    const el = containerRef.current
    const rect = el?.getBoundingClientRect()
    setView((v) => {
      if (v.crop === null && v.frameRect === null) return v
      const fit = rect
        ? computeFitFor(
            v.rotation,
            v.fit,
            image,
            rect.width,
            rect.height,
            v.fineAngle,
            v.aspect,
            null,
            v.frameScale,
            null,
          )
        : null
      const cleared = { ...v, crop: null, frameRect: null }
      return fit ? { ...cleared, ...fit, fit: 'contain' } : cleared
    })
  }, [image])

  /**
   * 把旋转、角度、比例与框选一并复位。
   *
   * ⚠️ **必须清 `rotation`**（2026-10-07 修复）：此前只清 `fineAngle`，
   * 于是「顺时针 90°」之后点「复位」画面纹丝不动 —— 用户报的 bug。
   * 复位后按 `contain` 重算适应，让画面的确回到刚打开这张图的样子。
   */
  const resetAdjust = useCallback(() => {
    const el = containerRef.current
    const rect = el?.getBoundingClientRect()
    setView((v) => {
      const base = {
        ...v,
        rotation: 0,
        fineAngle: 0,
        aspect: 'free' as AspectRatio,
        frameScale: 1,
        frameRect: null,
        crop: null,
      }
      if (!rect) return base
      const fit = computeFitFor(0, 'contain', image, rect.width, rect.height, 0, 'free', null, 1, null)
      return fit ? { ...base, ...fit, fit: 'contain' } : base
    })
  }, [image])

  /**
   * 取当前视图的深拷贝快照。`crop` 是对象，必须一并拷，
   * 否则快照会与实时状态共享同一引用。
   */
  const snapshotView = useCallback((): ViewState => {
    const v = viewRef.current
    return { ...v, crop: v.crop ? { ...v.crop } : null }
  }, [])

  /**
   * 用快照还原视图。
   *
   * ⚠️ **不重算 fit**：快照里的 `scale/tx/ty` 就是当时屏幕上真实的
   * 缩放与位置，直接覆盖才是「恢复到调整前的显示」。若这里再跑一遍
   * `computeFitFor`，容器尺寸没变时结果相同，但一旦用户在此期间
   * 改过窗口大小，就会把用户当时的构图挪走，与诉求不符。
   * 选区的屏幕矩形由「跟随 crop 重算」的 effect 自动同步。
   */
  const restoreView = useCallback((snap: ViewState) => {
    setView({ ...snap, crop: snap.crop ? { ...snap.crop } : null })
  }, [])

  // 响应工具栏/快捷键派发的 `pv-action` 自定义事件
  useEffect(() => {
    const el = containerRef.current
    if (!el) return
    const handler = (e: CustomEvent) => {
      const detail = (e as CustomEvent<{ action: string }>).detail
      const rect = el.getBoundingClientRect()
      switch (detail.action) {
        case 'fit':
          setView((v) => {
            // 适应时保留当前角度与比例（用户调好的构图不该被「适应」抹掉）；
            // 有框选时按框选区域适应，与松手那一刻保持一致
            const fit = computeFitFor(
              v.rotation,
              'contain',
              image,
              rect.width,
              rect.height,
              v.fineAngle,
              v.aspect,
              v.crop,
              v.frameScale,
              v.frameRect,
            )
            return fit ? { ...v, ...fit, fit: 'contain' } : v
          })
          break
        case 'actual': // 100%
          setView((v) =>
            image
              ? { ...v, scale: 1, tx: rect.width / 2, ty: rect.height / 2, fit: 'none' }
              : v,
          )
          break
        case 'zoom-in':
          setView((v) => ({ ...v, scale: Math.min(MAX_SCALE, v.scale * BUTTON_ZOOM_STEP), fit: 'none' }))
          break
        case 'zoom-out':
          setView((v) => ({ ...v, scale: Math.max(MIN_SCALE, v.scale / BUTTON_ZOOM_STEP), fit: 'none' }))
          break
        case 'rotate-left':
          rotate(-90)
          break
        case 'rotate-right':
          rotate(90)
          break
        case 'pan-left':
          setView((v) => ({ ...v, tx: v.tx + PAN_STEP }))
          break
        case 'pan-right':
          setView((v) => ({ ...v, tx: v.tx - PAN_STEP }))
          break
        case 'pan-up':
          setView((v) => ({ ...v, ty: v.ty + PAN_STEP }))
          break
        case 'pan-down':
          setView((v) => ({ ...v, ty: v.ty - PAN_STEP }))
          break
      }
    }
    el.addEventListener('pv-action', handler as EventListener)
    return () => el.removeEventListener('pv-action', handler as EventListener)
  }, [image, computeFit, rotate])

  return {
    view,
    dragging,
    frameBox,
    frameDragging,
    onFrameMouseDown,
    isPixelated: view.scale >= 1.5,
    containerRef,
    onWheel,
    onDoubleClick,
    setZoom,
    setFineAngle,
    setAspect,
    clearCrop,
    resetAdjust,
    snapshotView,
    restoreView,
  }
}
