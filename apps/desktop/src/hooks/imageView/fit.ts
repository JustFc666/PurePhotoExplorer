/**
 * 「适应窗口」与「能不能拖」的判据（纯函数，无 React 依赖）。
 *
 * ⚠️ [`computeFitFor`] 是整个画布缩放的地基：2026-10-07 出过
 * 「任意角度旋转后自由比例塌缩成 1:1」的严重回归。改之前先读它的注释，
 * 再跑 `useImageView.test.ts`。
 */

import type { DecodedImage } from '../../ipc'
import { ASPECT_VALUES, MAX_SCALE, MIN_SCALE } from './types'
import type { AspectRatio, CropRect, FitMode, FrameRect } from './types'
import { rotatedBounds, rotatedImageSize } from './geometry'
import { effectiveFrameRect, frameRectToPixels, targetViewport } from './frame'

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

/**
 * 图片在容器里是否**超出**（任一方向有内容被裁掉）—— 决定能不能拖拽平移。
 *
 * 只在超出时才允许拖，和 Windows 照片 / FastStone 一致：适应窗口时图片完整可见，
 * 能拖反而会把它拖偏，看着像「图跑偏了」。
 *
 * @param rotation 90° 粗旋转角（度，会被四舍五入到整 90）
 */
export function canPanImage(
  imageW: number,
  imageH: number,
  rotation: number,
  scale: number,
  containerW: number,
  containerH: number,
): boolean {
  if (containerW <= 0 || containerH <= 0) return false
  const rs = rotatedImageSize(imageW, imageH, Math.round(rotation / 90))
  /* 1px 容差：浮点算下来 0.9999 倍不算超出 */
  return rs.w * scale > containerW + 1 || rs.h * scale > containerH + 1
}
