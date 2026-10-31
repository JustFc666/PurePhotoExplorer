/**
 * 单图浏览的**类型与常量** —— `hooks/imageView/` 这一层的地基。
 *
 * ## 目录职责
 * - `types.ts`（本文件）：类型、比例常量、缩放上下限 —— 没有逻辑，谁都可以引
 * - `geometry.ts`：屏幕 ↔ 图像坐标换算（旋转外包框、归一化裁剪区）
 * - `frame.ts`：裁剪框几何（预置框、图像可见区、拖拽约束）
 * - `fit.ts`：「适应窗口」与「能否拖拽平移」的判据
 * - `../useImageView.ts`：hook 本体（状态 + 事件），也是对外唯一入口
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

export const MIN_SCALE = 0.05
export const MAX_SCALE = 32

/** 裁剪框缩放系数的下限（再小就没有裁剪意义了） */
export const MIN_FRAME_SCALE = 0.2
