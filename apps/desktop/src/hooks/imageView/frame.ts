/**
 * 裁剪框几何（纯函数，无 React 依赖）。
 *
 * 框有两套坐标，别混：
 * - [`FrameRect`]：归一化**视口**坐标（相对容器 0~1）—— 决定框画在哪；
 * - [`CropRect`]：归一化**图像**坐标（相对旋转后的图）—— 决定裁哪块。
 *
 * 两者之间的换算只发生在 [`rectToCrop`] / [`cropToScreenRect`] 里。
 */

import type { DecodedImage } from '../../ipc'
import { ASPECT_VALUES, MIN_FRAME_SCALE } from './types'
import type { AspectRatio, CropRect, FrameRect, ViewState } from './types'
import { cropToScreenRect, rotatedBounds, rotatedImageSize } from './geometry'

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
