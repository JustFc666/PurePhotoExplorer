/**
 * Magic Kernel Sharp 2013 —— 高质量降采样。
 *
 * ## 为什么不用 canvas 的 `imageSmoothingQuality = 'high'`
 * Skia 的 `high` 是**双三次插值，缩小时不做预滤波**，3:1 这种幅度会产生明显锯齿
 * （Skia issue #500117356：CanvasKit 降采样不用 mipmap，Flutter 团队只能手动逐级减半绕过）。
 *
 * ## MKS 是什么
 * Facebook 自 2013、Instagram 自 2015 用来做图片缩放。作者 John Costella 2021 年证明了
 * Magic Kernel 的傅里叶变换就是 sinc³，在采样频率整数倍处有三阶零点，因此能强力压制
 * 低频混叠；而 Lanczos 没有这个性质，缩小时会有 beat（拍频纹）。
 *
 * 核 = 三次 B 样条（box ⊗ box ⊗ box）⊗ Sharp 2013 {−1, +6, −1}/4。
 *
 * ## 三个必须照做的细节
 * 1. **在线性光空间缩放**：sRGB → linear → 缩放 → sRGB。直接在 sRGB 上插值会让暗部偏亮、
 *    局部对比度被压平（FB/IG 的流水线都是这么做的）。
 * 2. **缩小时把核半径乘以 scale**（反混叠），否则等于没做低通。
 * 3. **Sharp 卷积作用在输出分辨率上**，一次就够；再叠加 unsharp 会过度。
 *
 * 只做缩小。放大直接双线性——那属于另一个问题（超分），不在本模块职责内。
 */

export interface RgbaImage {
  data: Uint8ClampedArray
  width: number
  height: number
}

/** sRGB 8bit → 线性 [0,1] 的 256 项查找表。 */
const SRGB_TO_LIN = (() => {
  const t = new Float32Array(256)
  for (let i = 0; i < 256; i++) {
    const c = i / 255
    t[i] = c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4)
  }
  return t
})()

/**
 * 线性 [0,1] → sRGB 8bit 的 4096 段查找表。
 * 逐像素调 `Math.pow(c, 1/2.4)` 在 10 万像素上有几百毫秒，查表后是几毫秒。
 */
const LIN_LUT_N = 4096
const LIN_TO_SRGB = (() => {
  const t = new Uint8ClampedArray(LIN_LUT_N + 1)
  for (let i = 0; i <= LIN_LUT_N; i++) {
    const c = i / LIN_LUT_N
    const s = c <= 0.0031308 ? c * 12.92 : 1.055 * Math.pow(c, 1 / 2.4) - 0.055
    t[i] = Math.round(Math.min(1, Math.max(0, s)) * 255)
  }
  return t
})()

function encodeLinear(v: number): number {
  if (v <= 0) return 0
  if (v >= 1) return 255
  return LIN_TO_SRGB[(v * LIN_LUT_N) | 0]!
}

/**
 * 三次 B 样条（cardinal B-spline of order 3）—— Magic Kernel 的核。
 * 支撑半径 2，端点处连续到一阶导数，频谱是 sinc³。
 */
function cubicBSpline(x: number): number {
  const a = x < 0 ? -x : x
  if (a < 1) return (4 - 6 * a * a + 3 * a * a * a) / 6
  if (a < 2) {
    const t = 2 - a
    return (t * t * t) / 6
  }
  return 0
}

interface Taps {
  /** 第一个源像素的下标（可能为负，取值时 clamp） */
  start: number
  /** 归一化后的权重，长度 = taps * 2 */
  w: Float32Array
}

/**
 * 预计算一维权重。
 *
 * ⭐ 关键：`dist` 要除以 `scale`，即缩小时把核"拉宽"到覆盖更多源像素。
 *    漏掉这一步就等于没做低通，会退化成最近邻那种锯齿。
 */
function buildTaps(srcLen: number, dstLen: number): Taps[] {
  const scale = srcLen / dstLen
  const radius = 2 * Math.max(scale, 1)
  const taps = Math.ceil(radius)
  const n = taps * 2
  const out: Taps[] = []

  for (let i = 0; i < dstLen; i++) {
    const center = (i + 0.5) * scale - 0.5
    const start = Math.floor(center) - taps + 1
    const w = new Float32Array(n)
    let sum = 0
    for (let k = 0; k < n; k++) {
      const dist = Math.abs((center - (start + k)) / scale)
      const v = cubicBSpline(dist)
      w[k] = v
      sum += v
    }
    if (sum > 1e-8) {
      for (let k = 0; k < n; k++) w[k] = w[k]! / sum
    }
    out.push({ start, w })
  }
  return out
}

/**
 * 单通道一维重采样。
 * `stride` = 相邻样本在数组里的间隔（水平 1 / 垂直 width），
 * `base` = 这一行（或这一列）的起始偏移。
 */
function resampleLine(
  src: Float32Array,
  srcLen: number,
  dst: Float32Array,
  taps: Taps[],
  stride: number,
  base: number,
): void {
  const last = srcLen - 1
  for (let i = 0; i < taps.length; i++) {
    const { start, w } = taps[i]!
    let acc = 0
    for (let k = 0; k < w.length; k++) {
      let idx = start + k
      if (idx < 0) idx = 0
      else if (idx > last) idx = last
      acc += src[base + idx * stride]! * w[k]!
    }
    dst[i] = acc
  }
}

/**
 * Sharp 卷积：`{−s, 4+2s, −s}/4`（可分离，水平和垂直各做一次）。边走 clamp，避免暗环。
 *
 * `s = 1` 就是标准 Sharp 2013 `{−1, +6, −1}/4`；
 * `s > 1` 是 Costella 说的 "Magic Kernel Sharp+"，Instagram 用的就是这个（`s` 官方未公布）。
 * 往大调锐度上升，但过大会出现振铃（眼白/牙齿边缘的白圈）。
 */
function sharpenLine(
  src: Float32Array,
  len: number,
  dst: Float32Array,
  s: number,
): void {
  const last = len - 1
  const mid = 4 + 2 * s
  for (let i = 0; i < len; i++) {
    const cur = src[i]!
    const prev = src[i > 0 ? i - 1 : 0]!
    const next = src[i < last ? i + 1 : last]!
    dst[i] = (mid * cur - s * prev - s * next) / 4
  }
}

function clamp01(v: number): number {
  return v < 0 ? 0 : v > 1 ? 1 : v
}

/**
 * Magic Kernel Sharp 2013 降采样。
 *
 * @param src    源图（RGBA8，来自 canvas 的 `getImageData`）
 * @param dstW   目标宽
 * @param dstH   目标高
 * @param sharp  是否应用 Sharp 卷积（默认 true；纯低通时传 false）
 * @param strength Sharp 强度 `s`：1 = 标准 MKS2013，>1 = MKS+（更锐，过头会振铃）
 * @returns      目标尺寸的新 RGBA8 图
 */
export function resampleMks(
  src: RgbaImage,
  dstW: number,
  dstH: number,
  sharp = true,
  strength = 1,
): RgbaImage {
  const sw = src.width
  const sh = src.height
  const dw = Math.max(1, Math.round(dstW))
  const dh = Math.max(1, Math.round(dstH))

  // 没缩小（或尺寸非法）就别做滤波，直接双线性交给 canvas 更快
  if (sw <= dw && sh <= dh) {
    return bilinear(src, dw, dh)
  }

  const tapsX = buildTaps(sw, dw)
  const tapsY = buildTaps(sh, dh)

  const out = new Uint8ClampedArray(dw * dh * 4)
  // 三个颜色通道各用一个平面，逐通道处理（alpha 最后统一置 255：头像源是 JPEG）
  const planeTmpH = new Float32Array(dw * sh)
  const planeTmpV = new Float32Array(dw * dh)
  const planeTmpH2 = new Float32Array(dw * dh)

  for (let ch = 0; ch < 3; ch++) {
    // ── 1. 解码到线性空间 ──
    const lin = new Float32Array(sw * sh)
    for (let p = 0, i = 0; p < lin.length; p++, i += 4) {
      lin[p] = SRGB_TO_LIN[src.data[i + ch]!]!
    }

    // ── 2. 水平重采样：sw → dw，每个源行一次 ──
    for (let y = 0; y < sh; y++) {
      resampleLine(lin, sw, planeTmpH.subarray(y * dw, y * dw + dw), tapsX, 1, y * sw)
    }

    // ── 3. 垂直重采样：sh → dh ──
    for (let x = 0; x < dw; x++) {
      resampleLine(planeTmpH, sh, planeTmpV.subarray(x * dh, x * dh + dh), tapsY, dw, x)
    }

    // ⚠️ planeTmpV 目前是「按列存」的（x 主序），转到行主序再做水平卷积
    const rowMajor = new Float32Array(dw * dh)
    for (let y = 0; y < dh; y++) {
      for (let x = 0; x < dw; x++) rowMajor[y * dw + x] = planeTmpV[x * dh + y]!
    }

    // ── 4. Sharp 2013（水平 + 垂直各一次）──
    if (sharp) {
      for (let y = 0; y < dh; y++) {
        const start = y * dw
        sharpenLine(
          rowMajor.subarray(start, start + dw),
          dw,
          planeTmpH2.subarray(start, start + dw),
          strength,
        )
      }
      const col = new Float32Array(dh)
      const colOut = new Float32Array(dh)
      for (let x = 0; x < dw; x++) {
        for (let y = 0; y < dh; y++) col[y] = planeTmpH2[y * dw + x]!
        sharpenLine(col, dh, colOut, strength)
        for (let y = 0; y < dh; y++) {
          const v = colOut[y]!
          planeTmpH2[y * dw + x] = v
        }
      }
    } else {
      planeTmpH2.set(rowMajor)
    }

    // ── 5. 编码回 sRGB ──
    for (let p = 0, i = ch; p < dh * dw; p++, i += 4) {
      out[i] = encodeLinear(clamp01(planeTmpH2[p]!))
    }
  }

  for (let i = 3; i < out.length; i += 4) out[i] = 255
  return { data: out, width: dw, height: dh }
}

/** 放大用的朴素双线性（不在 MKS 职责内，仅作兜底）。 */
function bilinear(src: RgbaImage, dw: number, dh: number): RgbaImage {
  const out = new Uint8ClampedArray(dw * dh * 4)
  const sw = src.width
  const sh = src.height
  for (let y = 0; y < dh; y++) {
    const fy = ((y + 0.5) * sh) / dh - 0.5
    const y0 = Math.max(0, Math.min(sh - 1, Math.floor(fy)))
    const y1 = Math.min(sh - 1, y0 + 1)
    const wy = Math.max(0, Math.min(1, fy - y0))
    for (let x = 0; x < dw; x++) {
      const fx = ((x + 0.5) * sw) / dw - 0.5
      const x0 = Math.max(0, Math.min(sw - 1, Math.floor(fx)))
      const x1 = Math.min(sw - 1, x0 + 1)
      const wx = Math.max(0, Math.min(1, fx - x0))
      const o = (y * dw + x) * 4
      for (let c = 0; c < 3; c++) {
        const a = src.data[(y0 * sw + x0) * 4 + c]!
        const b = src.data[(y0 * sw + x1) * 4 + c]!
        const cc = src.data[(y1 * sw + x0) * 4 + c]!
        const d = src.data[(y1 * sw + x1) * 4 + c]!
        out[o + c] = (a * (1 - wx) + b * wx) * (1 - wy) + (cc * (1 - wx) + d * wx) * wy
      }
      out[o + 3] = 255
    }
  }
  return { data: out, width: dw, height: dh }
}
