/**
 * `useImageView` 里几个**纯函数**的单元测试。
 *
 * 这些函数是整个画布交互的数学地基：缩放/平移/旋转/框选的正确性全靠它们。
 * 尤其是 `computeFitFor` —— 2026-10-07 出过一次严重回归：
 * 「任意角度旋转后，自由比例的画面塌缩成 1:1」，根因就是拿 `rotatedBounds`
 * 去 contain 一个方形外包矩形。这里把验收标准钉死，防止再犯。
 */

import { describe, expect, it } from 'vitest'
import {
  ASPECT_VALUES,
  MIN_FRAME_SCALE,
  activeFrameRect,
  aspectCenterCrop,
  computeFitFor,
  constrainRectToRatio,
  cropFrameRect,
  cropToScreenRect,
  effectiveFrameRect,
  frameRectToPixels,
  imageVisibleRect,
  rotatedBounds,
  rotatedImageSize,
  screenToImageNorm,
  targetViewport,
  rectToCrop,
  MAX_SCALE,
  MIN_CROP_SIZE,
} from './useImageView'
import type { DecodedImage } from '../ipc'

/** 真实样本尺寸（test-image/DSC9515.JPG）：横图、宽高比 1.5 */
const IMG = { width: 5847, height: 3898 } as DecodedImage
/** 画布容器（DPI 120，1600×1000 客户区扣掉工具栏 30 + 状态栏 28 后的近似值） */
const CW = 1600
const CH = 970

/**
 * 裁剪区在**屏幕**上占据的宽高。
 *
 * 归一化 `crop` 的坐标系是**旋转后的图**（后端先旋转再裁剪），而屏幕上显示的
 * 就是那张图 —— 所以尺寸就是 `归一化值 × 旋转后尺寸 × scale`，
 * **不需要任何宽高互换的特殊照顾**（互换已由 [`rotatedImageSize`] 表达）。
 *
 * ⚠️ 参数是 90° 的**步数**，不是角度 —— 任意角度只可能是 `fineAngle`
 * （细调），而细调要按外接矩形另算，本助手不处理。
 */
const screenSizeOf = (crop: { w: number; h: number }, rotSteps: number, scale: number) => {
  const { w: rotW, h: rotH } = rotatedImageSize(IMG.width, IMG.height, rotSteps)
  return { w: crop.w * rotW * scale, h: crop.h * rotH * scale }
}

describe('rotatedBounds', () => {
  it('0/90/180/270 度时等于原尺寸或宽高互换', () => {
    expect(rotatedBounds(100, 60, 0)).toEqual({ w: 100, h: 60 })
    expect(rotatedBounds(100, 60, 90)).toEqual({ w: 60, h: 100 })
    expect(rotatedBounds(100, 60, 180)).toEqual({ w: 100, h: 60 })
    expect(rotatedBounds(100, 60, 270)).toEqual({ w: 60, h: 100 })
  })

  it('抹掉浮点误差（90 度不应算成 90.0000001）', () => {
    const b = rotatedBounds(5847, 3898, 90)
    expect(b.w).toBe(3898)
    expect(b.h).toBe(5847)
  })

  it('45 度时外包矩形是正方形，边长 = (w+h)/√2', () => {
    const b = rotatedBounds(100, 100, 45)
    expect(b.w).toBeCloseTo(141.421356, 4)
    expect(b.h).toBeCloseTo(141.421356, 4)
  })
})

describe('targetViewport', () => {
  it('在容器内取最大的目标比例矩形', () => {
    expect(targetViewport(1600, 970, 1)).toEqual({ w: 970, h: 970 })
    const wide = targetViewport(1600, 970, 16 / 9)
    expect(wide.w).toBe(1600)
    expect(wide.h).toBeCloseTo(900, 6)
  })

  it('比例比容器还极端时不越界（退回容器比例）', () => {
    // 2.35:1 放进 1600x970：宽够、高算出来 681 < 970 → 正常
    const p = targetViewport(1600, 970, 2.35)
    expect(p.w).toBe(1600)
    expect(p.h).toBeCloseTo(680.85, 1)
    // 竖屏容器里放超宽比例：应退回容器比例而不是溢出
    const tall = targetViewport(400, 1000, 2.35)
    expect(tall.w).toBeLessThanOrEqual(400)
    expect(tall.h).toBeLessThanOrEqual(1000)
  })

  it('8 个比例的视口宽高比都精确', () => {
    for (const [key, ratio] of Object.entries(ASPECT_VALUES)) {
      const v = targetViewport(CW, CH, ratio)
      expect(v.w / v.h, `${key} 宽高比`).toBeCloseTo(ratio, 6)
      expect(v.w).toBeLessThanOrEqual(CW + 1e-6)
      expect(v.h).toBeLessThanOrEqual(CH + 1e-6)
    }
  })
})

describe('computeFitFor —— 自由比例（问题 1 回归防护）', () => {
  it('斜角下缩放完全一致 —— 画面尺寸不随斜角旋转变化', () => {
    const base = computeFitFor(0, 'contain', IMG, CW, CH)
    expect(base).not.toBeNull()
    for (const deg of [10, 25, 45, 135, 225, 315]) {
      const f = computeFitFor(deg, 'contain', IMG, CW, CH)
      expect(f, `${deg}° 应有结果`).not.toBeNull()
      expect(f!.scale, `${deg}° 的 scale 不应变`).toBeCloseTo(base!.scale, 9)
    }
  })

  it('90/270° 正交旋转要**重新适应窗口**（问题 4 回归防护）', () => {
    // 用户诉求：「调整模式顺逆 90 度旋转，图片要刷新到合适窗口大小」。
    // 横图转 90° 后视觉盒子变成 h×w，必须按旋转后的尺寸 contain，
    // 否则图片上下会被切掉。
    const f90 = computeFitFor(90, 'contain', IMG, CW, CH)!
    const f270 = computeFitFor(270, 'contain', IMG, CW, CH)!
    const f180 = computeFitFor(180, 'contain', IMG, CW, CH)!

    // 90°：视觉盒子 = IMG.height × IMG.width
    const boxW90 = IMG.height * f90.scale
    const boxH90 = IMG.width * f90.scale
    expect(boxW90).toBeLessThanOrEqual(CW + 1e-9)
    expect(boxH90).toBeCloseTo(CH, 6) // 高度恰好铺满
    expect(Math.max(boxW90 / CW, boxH90 / CH)).toBeCloseTo(1, 9)

    // 270° 与 90° 等价
    expect(f270.scale).toBeCloseTo(f90.scale, 9)

    // 180° 不改变宽高，缩放应与 0° 相同
    expect(f180.scale).toBeCloseTo(computeFitFor(0, 'contain', IMG, CW, CH)!.scale, 9)

    // 关键：90° 的 scale 必须**小于** 0°，否则就是没重新适应（会被切掉）
    expect(f90.scale).toBeLessThan(computeFitFor(0, 'contain', IMG, CW, CH)!.scale)
  })

  it('90° 旋转后图片完整可见，不被画布上下裁切', () => {
    const f = computeFitFor(90, 'contain', IMG, CW, CH)!
    const boxW = IMG.height * f.scale
    const boxH = IMG.width * f.scale
    // 两个方向都必须放得下（含浮点余量）
    expect(boxW).toBeLessThanOrEqual(CW + 1e-6)
    expect(boxH).toBeLessThanOrEqual(CH + 1e-6)
  })

  it('绝不塌缩成 1:1 —— 45 度时画面宽度必须远大于容器高度', () => {
    const f = computeFitFor(45, 'contain', IMG, CW, CH)!
    const onScreenW = IMG.width * f.scale
    // 1:1 塌缩的症状 = 画面宽度正好等于容器高度；这里必须显著大于它
    expect(onScreenW).toBeGreaterThan(CH * 1.3)
    // 且应当铺满容器的一条边（contain 语义 = 恰好放得下）
    const onScreenH = IMG.height * f.scale
    expect(Math.max(onScreenW / CW, onScreenH / CH)).toBeCloseTo(1, 9)
  })

  it('未旋转时按 contain 放下，且不放大小图', () => {
    const small = { width: 200, height: 100 } as DecodedImage
    const f = computeFitFor(0, 'contain', small, CW, CH)!
    expect(f.scale).toBe(1)
  })

  it('细调角度(fineAngle)同样不改变缩放', () => {
    const base = computeFitFor(0, 'contain', IMG, CW, CH, 0)
    for (const fine of [-45, -20, 0, 20, 45]) {
      const f = computeFitFor(0, 'contain', IMG, CW, CH, fine)
      expect(f!.scale, `fine=${fine}°`).toBeCloseTo(base!.scale, 9)
    }
  })
})

describe('computeFitFor —— 固定比例（覆盖模式）', () => {
  it('旋转后仍能填满裁剪框，不露黑角', () => {
    for (const aspect of ['1:1', '3:4', '4:3', '16:9', '9:16', '2.35:1', '2:3'] as const) {
      const ratio = ASPECT_VALUES[aspect]
      const v = targetViewport(CW, CH, ratio)
      for (const deg of [0, 25, 45, 90]) {
        const f = computeFitFor(deg, 'contain', IMG, CW, CH, 0, aspect)!
        // 旋转后的外包矩形在屏幕上的尺寸
        const rot90 = Math.abs((((deg % 180) + 180) % 180) - 90) < 1e-9
        const b = rotatedBounds(IMG.width, IMG.height, deg)
        const fw = rot90 ? IMG.height : b.w
        const fh = rot90 ? IMG.width : b.h
        expect(fw * f.scale, `${aspect} @${deg}° 宽覆盖`).toBeGreaterThanOrEqual(v.w - 1e-6)
        expect(fh * f.scale, `${aspect} @${deg}° 高覆盖`).toBeGreaterThanOrEqual(v.h - 1e-6)
      }
    }
  })
})

describe('cropFrameRect —— 固定比例下拖框边角缩放（问题 3）', () => {
  it('frameScale = 1 时等于容器内该比例的最大矩形', () => {
    for (const [key, ratio] of Object.entries(ASPECT_VALUES)) {
      const full = cropFrameRect(CW, CH, ratio, 1)
      const base = targetViewport(CW, CH, ratio)
      expect(full.w, `${key} 宽`).toBeCloseTo(base.w, 9)
      expect(full.h, `${key} 高`).toBeCloseTo(base.h, 9)
    }
  })

  it('比例**始终锁死** —— 缩到任意倍数宽高比都不变', () => {
    for (const [key, ratio] of Object.entries(ASPECT_VALUES)) {
      for (const fs of [1, 0.8, 0.5, 0.3, MIN_FRAME_SCALE]) {
        const r = cropFrameRect(CW, CH, ratio, fs)
        expect(r.w / r.h, `${key} @frameScale=${fs} 宽高比`).toBeCloseTo(ratio, 9)
      }
    }
  })

  it('frameScale 线性生效：0.5 就是正好一半', () => {
    const full = cropFrameRect(CW, CH, 16 / 9, 1)
    const half = cropFrameRect(CW, CH, 16 / 9, 0.5)
    expect(half.w).toBeCloseTo(full.w / 2, 9)
    expect(half.h).toBeCloseTo(full.h / 2, 9)
  })

  it('系数被夹到 [MIN_FRAME_SCALE, 1]，不会缩到 0 也不会超过容器', () => {
    const tooSmall = cropFrameRect(CW, CH, 1, 0.01)
    const min = cropFrameRect(CW, CH, 1, MIN_FRAME_SCALE)
    expect(tooSmall.w).toBeCloseTo(min.w, 9)
    const tooBig = cropFrameRect(CW, CH, 1, 5)
    const full = cropFrameRect(CW, CH, 1, 1)
    expect(tooBig.w).toBeCloseTo(full.w, 9)
  })

  it('框变小后画面仍填满新框（不露黑角）—— 覆盖视口必须跟着缩', () => {
    for (const aspect of ['1:1', '16:9', '9:16'] as const) {
      const ratio = ASPECT_VALUES[aspect]
      for (const fs of [1, 0.6, 0.35]) {
        const v = cropFrameRect(CW, CH, ratio, fs)
        for (const deg of [0, 45, 90]) {
          const f = computeFitFor(deg, 'contain', IMG, CW, CH, 0, aspect, null, fs)!
          const rot90 = Math.abs((((deg % 180) + 180) % 180) - 90) < 1e-9
          const b = rotatedBounds(IMG.width, IMG.height, deg)
          const fw = rot90 ? IMG.height : b.w
          const fh = rot90 ? IMG.width : b.h
          expect(fw * f.scale, `${aspect} @fs=${fs} @${deg}° 宽覆盖`).toBeGreaterThanOrEqual(v.w - 1e-6)
          expect(fh * f.scale, `${aspect} @fs=${fs} @${deg}° 高覆盖`).toBeGreaterThanOrEqual(v.h - 1e-6)
        }
      }
    }
  })

  it('frameScale 只影响框，不影响自由比例的整图适应', () => {
    const a = computeFitFor(0, 'contain', IMG, CW, CH, 0, 'free', null, 1)
    const b = computeFitFor(0, 'contain', IMG, CW, CH, 0, 'free', null, 0.3)
    expect(a!.scale).toBeCloseTo(b!.scale, 12)
  })
})

describe('rotatedImageSize —— 裁剪坐标系的基准尺寸（2026-10-11 统一坐标系后新增）', () => {
  it('偶数步不换宽高，奇数步互换', () => {
    expect(rotatedImageSize(5847, 3898, 0)).toEqual({ w: 5847, h: 3898 })
    expect(rotatedImageSize(5847, 3898, 2)).toEqual({ w: 5847, h: 3898 })
    expect(rotatedImageSize(5847, 3898, 1)).toEqual({ w: 3898, h: 5847 })
    expect(rotatedImageSize(5847, 3898, 3)).toEqual({ w: 3898, h: 5847 })
  })

  it('负角度与超过 4 步都归一化正确', () => {
    expect(rotatedImageSize(100, 50, -1)).toEqual({ w: 50, h: 100 })
    expect(rotatedImageSize(100, 50, -2)).toEqual({ w: 100, h: 50 })
    expect(rotatedImageSize(100, 50, 5)).toEqual({ w: 50, h: 100 })
    expect(rotatedImageSize(100, 50, 8)).toEqual({ w: 100, h: 50 })
  })

  it('正方形图无论转几步都是它自己', () => {
    for (const k of [0, 1, 2, 3]) expect(rotatedImageSize(500, 500, k)).toEqual({ w: 500, h: 500 })
  })
})

describe('constrainRectToRatio —— 拖出的框锁死等比（问题 1）', () => {
  it('右下方向：宽高严格等于屏幕目标比', () => {
    const r = constrainRectToRatio(100, 100, 400, 300, 16 / 9)
    const w = Math.abs(r.x2 - r.x1)
    const h = Math.abs(r.y2 - r.y1)
    expect(w / h).toBeCloseTo(16 / 9, 12)
  })

  it('四个拖拽方向都成立（框跟着鼠标往对应象限长）', () => {
    const targets = [1, 3 / 4, 16 / 9, 9 / 16, 2.35]
    // 位移要足够大，否则「另一轴反推」会把框推过起点（例如 1:1 时
    // 横向只拖 1px 而纵向拖 200px，宽度会按高度算成 200 → 越过起点）。
    const corners = [
      { cx: 450, cy: 350, sx: 1, sy: 1 },
      { cx: 50, cy: 350, sx: -1, sy: 1 },
      { cx: 450, cy: 50, sx: 1, sy: -1 },
      { cx: 50, cy: 50, sx: -1, sy: -1 },
    ]
    for (const t of targets) {
      for (const c of corners) {
        const r = constrainRectToRatio(100, 100, c.cx, c.cy, t)
        const w = Math.abs(r.x2 - r.x1)
        const h = Math.abs(r.y2 - r.y1)
        expect(w / h, `ratio=${t} dir=(${c.sx},${c.sy})`).toBeCloseTo(t, 9)
        // 起点不动，终点落在鼠标所在象限
        expect(r.x1).toBe(100)
        expect(r.y1).toBe(100)
        expect(Math.sign(r.x2 - 100), `ratio=${t} x 方向`).toBe(c.sx)
        expect(Math.sign(r.y2 - 100), `ratio=${t} y 方向`).toBe(c.sy)
      }
    }
  })

  it('以「变化更大的那一轴」为准 —— 斜拖也跟手且不抖动', () => {
    // 横向拖得远 → 由宽度定尺寸
    const wide = constrainRectToRatio(0, 0, 800, 10, 1)
    expect(Math.abs(wide.x2 - wide.x1)).toBeCloseTo(800, 9)
    // 纵向拖得远 → 由高度反推宽度
    const tall = constrainRectToRatio(0, 0, 10, 800, 1)
    expect(Math.abs(tall.y2 - tall.y1)).toBeCloseTo(800, 9)
    expect(Math.abs(tall.x2 - tall.x1)).toBeCloseTo(800, 9)
  })

  it('零位移时退化成零尺寸（不会算出 NaN/无限大）', () => {
    const r = constrainRectToRatio(50, 50, 50, 50, 16 / 9)
    expect(Math.abs(r.x2 - r.x1)).toBeCloseTo(0, 12)
    expect(Math.abs(r.y2 - r.y1)).toBeCloseTo(0, 12)
  })

  /**
   * 端到端：屏幕框 → crop → **输出像素比**。
   *
   * 用「整图 contain 进容器且居中」的真实视图参数（scale 由 fit 算出），
   * 在画面中心附近拖框 —— 避免框超出图像被 `rectToCrop` 夹边而失真。
   *
   * ⭐ `screenTarget` 就是**目标比本身**，任何旋转角都一样（2026-10-11 统一
   * 坐标系后）。旧版 90° 要传 `(H/W)²/target`（`screenRatioFor`），那是
   * 「crop 按原图归一化、后端按旋转后图归一化」的补丁；现在两者同一个坐标系，
   * 屏幕比与输出比恒等。**旋转后拖框会不会跳变，就看这条**。
   */
  const fitScale = Math.min(CW / IMG.width, CH / IMG.height)
  const viewTx = CW / 2
  const viewTy = CH / 2
  /** 在中心拖一个宽高为 (w,h) 的框，返回输出像素比 */
  const outputRatioOf = (rotSteps: number, target: number, boxW: number, boxH: number) => {
    // 按目标比把框规整成 (boxW, boxW/target)，两者取小以免出界
    const w = Math.min(boxW, boxH * target)
    const h = w / target
    const r = constrainRectToRatio(
      viewTx - w / 2, viewTy - h / 2,
      viewTx + w / 2, viewTy + h / 2,
      target,
    )
    const { w: rotW, h: rotH } = rotatedImageSize(IMG.width, IMG.height, rotSteps)
    const crop = rectToCrop(
      r.x1, r.y1, r.x2, r.y2,
      viewTx, viewTy, fitScale, 0,
      rotW, rotH,
    )!
    expect(crop, '框应在图像范围内，不该被裁成 null').not.toBeNull()
    // 输出尺寸 = 归一化尺寸 × **旋转后图**尺寸（后端就是这么裁的）
    return (crop.w * rotW) / (crop.h * rotH)
  }

  it('0° 场景端到端：crop 输出比 == 目标比', () => {
    for (const target of [1, 3 / 4, 16 / 9, 9 / 16, 2.35]) {
      expect(outputRatioOf(0, target, 300, 260), `target=${target}`).toBeCloseTo(target, 6)
    }
  })

  it('90°（顺、逆）场景端到端：直接按目标比锁比即可，输出比 == 目标比', () => {
    const boxes: Array<[number, number]> = [
      [300, 260],
      [200, 200],
      [420, 120],
    ]
    for (const rot of [1, 3]) {
      for (const target of [1, 3 / 4, 16 / 9, 9 / 16, 2.35]) {
        for (const [bw, bh] of boxes) {
          expect(
            outputRatioOf(rot, target, bw, bh),
            `rot=${rot * 90}° target=${target} box=${bw}x${bh}`,
          ).toBeCloseTo(target, 6)
        }
      }
    }
  })

  it('180° 与 0° 等价、270° 与 90° 等价', () => {
    for (const target of [1, 16 / 9, 3 / 4]) {
      expect(outputRatioOf(2, target, 300, 260)).toBeCloseTo(outputRatioOf(0, target, 300, 260), 9)
      expect(outputRatioOf(3, target, 300, 260)).toBeCloseTo(outputRatioOf(1, target, 300, 260), 9)
    }
  })

  /**
   * 回归防护（2026-10-07 / 2026-10-11）：`computeFitFor` 的 **crop 分支**必须
   * 用「旋转后图」的尺寸算视口比例。两侧都不能出偏差：
   * - 漏了旋转（取 `(crop.w·W)/(crop.h·H)`）→ 90° 时差 `(W/H)²` = 2.25 倍；
   * - 多算一次旋转（crop 已是旋转后坐标，又交叉一遍）→ 同样差 2.25 倍。
   */
  it('各旋转角框选后自适应：视口比例与屏幕尺寸一致（覆盖且不超框）', () => {
    const crop = { x: 0.3, y: 0.3, w: 0.3, h: 0.2 }
    for (const rot of [0, 1, 2, 3]) {
      const { w: rotW, h: rotH } = rotatedImageSize(IMG.width, IMG.height, rot)
      const expectVp = targetViewport(CW, CH, (crop.w * rotW) / (crop.h * rotH))

      const f = computeFitFor(rot * 90, 'contain', IMG, CW, CH, 0, 'free', crop)!
      const got = screenSizeOf(crop, rot, f.scale)
      expect(got.w, `${rot * 90}° 宽覆盖`).toBeGreaterThanOrEqual(expectVp.w - 1e-6)
      expect(got.h, `${rot * 90}° 高覆盖`).toBeGreaterThanOrEqual(expectVp.h - 1e-6)
      // 且恰好贴满一条边，不做多余放大
      expect(Math.max(got.w / CW, got.h / CH), `${rot * 90}° 不该多留空`).toBeCloseTo(1, 6)
    }
  })
})

describe('computeFitFor —— 自由框选后刷新自适应（问题 2 回归防护）', () => {
  const cases = [
    { x: 0.25, y: 0.3, w: 0.3, h: 0.35 },
    { x: 0.05, y: 0.05, w: 0.9, h: 0.9 },
    { x: 0.6, y: 0.55, w: 0.35, h: 0.25 },
  ]

  /**
   * 框选区在**屏幕**上占据的宽高（见文件顶部的 `screenSizeOf`）。
   */

  it('框选区不变形（屏幕上宽高比 = 裁剪区的真实像素比）', () => {
    for (const crop of cases) {
      for (const rot of [0, 1, 2, 3]) {
        const { w: rotW, h: rotH } = rotatedImageSize(IMG.width, IMG.height, rot)
        // 裁剪结果的像素比 —— 直接由「旋转后图」算，不再手工做「取倒数」
        const expectRatio = (crop.w * rotW) / (crop.h * rotH)
        const f = computeFitFor(rot * 90, 'contain', IMG, CW, CH, 0, 'free', crop)!
        const { w, h } = screenSizeOf(crop, rot, f.scale)
        expect(w / h, `crop ${crop.w}x${crop.h} @${rot * 90}°`).toBeCloseTo(expectRatio, 9)
      }
    }
  })

  it('框选区最大化展示：恰好填满一条边，且另一边不溢出', () => {
    for (const crop of cases) {
      for (const rot of [0, 1, 2, 3]) {
        const f = computeFitFor(rot * 90, 'contain', IMG, CW, CH, 0, 'free', crop)!
        const { w: rw, h: rh } = screenSizeOf(crop, rot, f.scale)
        expect(Math.max(rw / CW, rh / CH), `最大化率 @${rot * 90}°`).toBeCloseTo(1, 6)
        expect(rw).toBeLessThanOrEqual(CW + 1e-6)
        expect(rh).toBeLessThanOrEqual(CH + 1e-6)
      }
    }
  })

  it('框选区中心精确落在容器中心（含旋转）', () => {
    for (const crop of cases) {
      for (const rot of [0, 1, 2, 3]) {
        const f = computeFitFor(rot * 90, 'contain', IMG, CW, CH, 0, 'free', crop)!
        const { w: rotW, h: rotH } = rotatedImageSize(IMG.width, IMG.height, rot)
        // 中心在旋转后图里的局部偏移（fineAngle = 0，不再需要旋转补偿）
        const lx = (crop.x + crop.w / 2 - 0.5) * rotW * f.scale
        const ly = (crop.y + crop.h / 2 - 0.5) * rotH * f.scale
        expect(f.tx + lx, `中心 x @${rot * 90}°`).toBeCloseTo(CW / 2, 6)
        expect(f.ty + ly, `中心 y @${rot * 90}°`).toBeCloseTo(CH / 2, 6)
      }
    }
  })

  it('角上的框选也会被正确居中（不会跑到画面外），细调角同样成立', () => {
    const corner = { x: 0.85, y: 0.85, w: 0.15, h: 0.15 }
    // 细调角只能走 fineAngle（`rotation` 只接 90° 的整数倍）
    const fine = 30
    const f = computeFitFor(0, 'contain', IMG, CW, CH, fine, 'free', corner)!
    const rad = (fine * Math.PI) / 180
    const cos = Math.cos(rad)
    const sin = Math.sin(rad)
    const lx = (corner.x + corner.w / 2 - 0.5) * IMG.width * f.scale
    const ly = (corner.y + corner.h / 2 - 0.5) * IMG.height * f.scale
    expect(f.tx + (lx * cos - ly * sin)).toBeCloseTo(CW / 2, 6)
    expect(f.ty + (lx * sin + ly * cos)).toBeCloseTo(CH / 2, 6)
  })
})

describe('computeFitFor —— 边界与退化输入', () => {
  it('无图 / 零尺寸容器返回 null', () => {
    expect(computeFitFor(0, 'contain', null, CW, CH)).toBeNull()
    expect(computeFitFor(0, 'contain', IMG, 0, CH)).toBeNull()
    expect(computeFitFor(0, 'contain', IMG, CW, 0)).toBeNull()
  })

  it("fit='none' 返回 null（保持用户手动缩放）", () => {
    expect(computeFitFor(0, 'none', IMG, CW, CH)).toBeNull()
  })

  it('缩放被夹在 [MIN_SCALE, MAX_SCALE] 内', () => {
    const huge = { width: 100000, height: 100000 } as DecodedImage
    const f = computeFitFor(0, 'contain', huge, CW, CH)!
    expect(f.scale).toBeGreaterThanOrEqual(0.05)
    expect(f.scale).toBeLessThanOrEqual(MAX_SCALE)
  })

  it('tx/ty 无 crop 时就是容器中心', () => {
    const f = computeFitFor(37, 'contain', IMG, CW, CH)!
    expect(f.tx).toBe(CW / 2)
    expect(f.ty).toBe(CH / 2)
  })
})

describe('screenToImageNorm / rectToCrop / cropToScreenRect', () => {
  it('屏幕中心映射到图像归一化中心', () => {
    const p = screenToImageNorm(CW / 2, CH / 2, CW / 2, CH / 2, 0.5, 0, IMG.width, IMG.height)
    expect(p.x).toBeCloseTo(0.5, 9)
    expect(p.y).toBeCloseTo(0.5, 9)
  })

  it('rot90 特判：屏幕点与图像点的对应关系正确', () => {
    // 100% 缩放、无平移、旋转 90°：图像左上角往 +x 方向映射
    const s = 1
    const p = screenToImageNorm(0, 0, 0, 0, s, 90, 100, 200)
    // 反变换后应落在图像坐标内
    expect(p.x).toBeGreaterThanOrEqual(0)
    expect(p.x).toBeLessThanOrEqual(1)
    expect(p.y).toBeGreaterThanOrEqual(0)
    expect(p.y).toBeLessThanOrEqual(1)
  })

  it('rectToCrop 结果被夹到 [0,1] 且 x+w / y+h 不超过 1', () => {
    const crop = rectToCrop(-500, -500, CW + 500, CH + 500, CW / 2, CH / 2, 0.5, 0, IMG.width, IMG.height)
    expect(crop).not.toBeNull()
    expect(crop!.x).toBeGreaterThanOrEqual(0)
    expect(crop!.y).toBeGreaterThanOrEqual(0)
    expect(crop!.x + crop!.w).toBeLessThanOrEqual(1 + 1e-9)
    expect(crop!.y + crop!.h).toBeLessThanOrEqual(1 + 1e-9)
  })

  it('过小的框被判为误触返回 null', () => {
    const tiny = MIN_CROP_SIZE / 4
    const crop = rectToCrop(100, 100, 100 + tiny * 100, 100 + tiny * 100, CW / 2, CH / 2, 0.001, 0, 10000, 10000)
    expect(crop).toBeNull()
  })

  it('cropToScreenRect 是 rectToCrop 的近似逆运算（无旋转时精确）', () => {
    const crop = { x: 0.2, y: 0.3, w: 0.4, h: 0.25 }
    const tx = 800
    const ty = 485
    const scale = 0.3
    const r = cropToScreenRect(crop, tx, ty, scale, 0, IMG.width, IMG.height)
    const back = rectToCrop(
      r.left,
      r.top,
      r.left + r.width,
      r.top + r.height,
      tx,
      ty,
      scale,
      0,
      IMG.width,
      IMG.height,
    )
    expect(back).not.toBeNull()
    expect(back!.x).toBeCloseTo(crop.x, 6)
    expect(back!.y).toBeCloseTo(crop.y, 6)
    expect(back!.w).toBeCloseTo(crop.w, 6)
    expect(back!.h).toBeCloseTo(crop.h, 6)
  })
})

describe('ASPECT_VALUES', () => {
  it('8 个比例（含 free）与数值映射齐全', () => {
    expect(Object.keys(ASPECT_VALUES)).toHaveLength(7)
    expect(ASPECT_VALUES['1:1']).toBe(1)
    expect(ASPECT_VALUES['3:4']).toBeCloseTo(0.75, 9)
    expect(ASPECT_VALUES['4:3']).toBeCloseTo(4 / 3, 9)
    expect(ASPECT_VALUES['16:9']).toBeCloseTo(16 / 9, 9)
    expect(ASPECT_VALUES['9:16']).toBeCloseTo(9 / 16, 9)
    expect(ASPECT_VALUES['2.35:1']).toBeCloseTo(2.35, 9)
    expect(ASPECT_VALUES['2:3']).toBeCloseTo(2 / 3, 9)
  })
})

describe('effectiveFrameRect —— 固定比例的「当前框」真源（问题 2）', () => {
  it('frameRect = null 时返回**居中的预置框**（该比例视口内最大）', () => {
    for (const [key, ratio] of Object.entries(ASPECT_VALUES)) {
      const px = frameRectToPixels(effectiveFrameRect(CW, CH, ratio, null, 1), CW, CH)
      const base = targetViewport(CW, CH, ratio)
      expect(px.width, `${key} 宽`).toBeCloseTo(base.w, 9)
      expect(px.height, `${key} 高`).toBeCloseTo(base.h, 9)
      // 居中
      expect(px.left + px.width / 2, `${key} 水平居中`).toBeCloseTo(CW / 2, 9)
      expect(px.top + px.height / 2, `${key} 垂直居中`).toBeCloseTo(CH / 2, 9)
    }
  })

  it('返回的是**归一化值**（0~1），不是像素', () => {
    const fr = effectiveFrameRect(CW, CH, ASPECT_VALUES['1:1'], null, 1)
    expect(fr.w).toBeLessThanOrEqual(1)
    expect(fr.h).toBeLessThanOrEqual(1)
    expect(fr.x).toBeGreaterThanOrEqual(0)
    expect(fr.y).toBeGreaterThanOrEqual(0)
  })

  it('frameRect 非空时**原样返回**（用户拖过的框优先）', () => {
    const custom = { x: 0.1, y: 0.2, w: 0.3, h: 0.4 }
    const fr = effectiveFrameRect(CW, CH, 16 / 9, custom, 1)
    expect(fr).toEqual(custom)
  })

  it('容器尺寸为 0 时返回零矩形（不抛错、不产生 NaN）', () => {
    const fr = effectiveFrameRect(0, 0, 1, null, 1)
    expect(fr).toEqual({ x: 0, y: 0, w: 0, h: 0 })
  })

  it('预置框满足所选比例，且完整落在视口内（按像素核对）', () => {
    for (const [key, ratio] of Object.entries(ASPECT_VALUES)) {
      const px = frameRectToPixels(effectiveFrameRect(CW, CH, ratio, null, 1), CW, CH)
      expect(px.width / px.height, `${key} 宽高比`).toBeCloseTo(ratio, 6)
      expect(px.left, `${key} 左不越界`).toBeGreaterThanOrEqual(-1e-9)
      expect(px.top, `${key} 上不越界`).toBeGreaterThanOrEqual(-1e-9)
      expect(px.left + px.width, `${key} 右不越界`).toBeLessThanOrEqual(CW + 1e-6)
      expect(px.top + px.height, `${key} 下不越界`).toBeLessThanOrEqual(CH + 1e-6)
    }
  })
})

describe('frameRectToPixels —— 归一化框 → 容器 CSS 像素', () => {
  it('乘以容器尺寸即为像素值', () => {
    const px = frameRectToPixels({ x: 0.25, y: 0.5, w: 0.5, h: 0.25 }, CW, CH)
    expect(px.left).toBeCloseTo(CW * 0.25, 9)
    expect(px.top).toBeCloseTo(CH * 0.5, 9)
    expect(px.width).toBeCloseTo(CW * 0.5, 9)
    expect(px.height).toBeCloseTo(CH * 0.25, 9)
  })

  it('与 effectiveFrameRect 组合后仍是原比例', () => {
    const fr = effectiveFrameRect(CW, CH, 16 / 9, null, 1)
    const px = frameRectToPixels(fr, CW, CH)
    expect(px.width / px.height).toBeCloseTo(16 / 9, 6)
  })
})

describe('拖框改大小 —— 比例锁死 + 限制在视口内（问题 2 核心）', () => {
  /** 模拟 hook 里 resize 分支的数学，验证两条硬约束 */
  const resize = (
    frame: { x: number; y: number; w: number; h: number },
    corner: 'nw' | 'ne' | 'sw' | 'se',
    mx: number,
    my: number,
    screenTarget: number,
    cw = CW,
    ch = CH,
  ) => {
    const box = frameRectToPixels(frame, cw, ch)
    const anchors = {
      se: { ax: box.left, ay: box.top },
      sw: { ax: box.left + box.width, ay: box.top },
      ne: { ax: box.left, ay: box.top + box.height },
      nw: { ax: box.left + box.width, ay: box.top + box.height },
    } as const
    const { ax, ay } = anchors[corner]
    const r = constrainRectToRatio(ax, ay, mx, my, screenTarget)
    let left = Math.min(r.x1, r.x2)
    let top = Math.min(r.y1, r.y2)
    let w = Math.abs(r.x2 - r.x1)
    let h = Math.abs(r.y2 - r.y1)
    const maxW = corner === 'nw' || corner === 'sw' ? ax : cw - ax
    const maxH = corner === 'nw' || corner === 'ne' ? ay : ch - ay
    const fit = Math.min(maxW / w, maxH / h, 1)
    w *= fit
    h *= fit
    if (corner === 'nw' || corner === 'sw') left = ax - w
    if (corner === 'nw' || corner === 'ne') top = ay - h
    left = Math.min(Math.max(left, 0), cw - w)
    top = Math.min(Math.max(top, 0), ch - h)
    return { x: left / cw, y: top / ch, w: w / cw, h: h / ch }
  }

  it('拖角改大小：框**始终不越界**（四角 × 多方向）', () => {
    const startFrame = effectiveFrameRect(CW, CH, ASPECT_VALUES['16:9'], null, 1)
    // 16:9 无旋转 → 屏幕比 = 目标比
    for (const corner of ['nw', 'ne', 'sw', 'se'] as const) {
      // 往各个方向狠拖，包括拖出容器外
      const drags: [number, number][] = [
        [-500, -500],
        [CW + 500, -500],
        [-500, CH + 500],
        [CW + 500, CH + 500],
        [CW / 2, CH / 2],
      ]
      for (const [mx, my] of drags) {
        const r = resize(startFrame, corner, mx, my, ASPECT_VALUES['16:9'])
        const px = frameRectToPixels(r, CW, CH)
        expect(px.left, `${corner}(${mx},${my}) 左`).toBeGreaterThanOrEqual(-1e-6)
        expect(px.top, `${corner}(${mx},${my}) 上`).toBeGreaterThanOrEqual(-1e-6)
        expect(px.left + px.width, `${corner}(${mx},${my}) 右`).toBeLessThanOrEqual(CW + 1e-6)
        expect(px.top + px.height, `${corner}(${mx},${my}) 下`).toBeLessThanOrEqual(CH + 1e-6)
      }
    }
  })

  it('拖角改大小：框宽高比**锁死**为屏幕比（不随拖拽方向漂移）', () => {
    const startFrame = effectiveFrameRect(CW, CH, ASPECT_VALUES['1:1'], null, 1)
    for (const corner of ['nw', 'ne', 'sw', 'se'] as const) {
      const r = resize(startFrame, corner, CW * 0.8, CH * 0.3, 1)
      const px = frameRectToPixels(r, CW, CH)
      expect(px.width / px.height, `${corner} 宽高比`).toBeCloseTo(1, 5)
    }
  })

  it('抓的角**对角顶点固定不动**（锚点语义）', () => {
    const startFrame = { x: 0.2, y: 0.2, w: 0.4, h: 0.4 }
    const box = frameRectToPixels(startFrame, CW, CH)
    // 拖 se 角 → nw 顶点应保持不动
    const r = resize(startFrame, 'se', box.left + box.width + 100, box.top + box.height + 100, 1)
    const px = frameRectToPixels(r, CW, CH)
    expect(px.left, 'se: nw.x 不动').toBeCloseTo(box.left, 4)
    expect(px.top, 'se: nw.y 不动').toBeCloseTo(box.top, 4)
  })

  /**
   * ⭐ 预置框（没拖过）在任何旋转角下都**不许骗人**：
   * 画面上是什么比例，后端裁出来就是什么比例。
   *
   * 旧版 `aspectCenterCrop` 多交叉了一次宽高（把「旋转后图的归一化尺寸」又当
   * 原图算了一遍），于是 90°/270° 下「框是正方形、裁出来是 0.44 的扁图」。
   * 用户报的「顺逆 90 度旋转后选固定比例调整大小不正常」正是这一族的症状。
   */
  it('各旋转角下预置框的屏幕比 == 后端输出比（框不许骗人）', () => {
    for (const rot of [0, 1, 2, 3]) {
      const { w: rotW, h: rotH } = rotatedImageSize(IMG.width, IMG.height, rot)
      // 取「整张旋转后的图刚好放进容器」的视图，保证预置框完整落在图内（不被夹边）
      const scale = Math.min(CW / rotW, CH / rotH)
      const view = { tx: CW / 2, ty: CH / 2, scale, rotation: rot * 90, fineAngle: 0 }
      for (const aspect of ['1:1', '16:9', '3:4', '9:16'] as const) {
        const target = ASPECT_VALUES[aspect]
        const fr = activeFrameRect(null, aspect, CW, CH, view, IMG)!
        const px = frameRectToPixels(fr, CW, CH)
        expect(px.width / px.height, `rot=${rot * 90}° ${aspect} 屏幕比`).toBeCloseTo(target, 6)

        const crop = rectToCrop(
          px.left, px.top, px.left + px.width, px.top + px.height,
          view.tx, view.ty, view.scale, 0, rotW, rotH,
        )!
        expect(crop, `rot=${rot * 90}° ${aspect} 框应在图内`).not.toBeNull()
        expect(
          (crop.w * rotW) / (crop.h * rotH),
          `rot=${rot * 90}° ${aspect} 后端输出比`,
        ).toBeCloseTo(target, 6)
        // 预置框 = 后端「居中裁」，所以 crop 必须居中
        expect(crop.x + crop.w / 2, `rot=${rot * 90}° ${aspect} 水平居中`).toBeCloseTo(0.5, 6)
        expect(crop.y + crop.h / 2, `rot=${rot * 90}° ${aspect} 垂直居中`).toBeCloseTo(0.5, 6)
      }
    }
  })
})

describe('框位置驱动视图定位 —— 框拖偏后画面跟着走（问题 2 定位）', () => {
  it('固定比例下 tx/ty 对到**框的中心**，而不是容器中心', () => {
    const frame = { x: 0.6, y: 0.7, w: 0.3, h: 0.2 }
    const f = computeFitFor(0, 'contain', IMG, CW, CH, 0, '16:9', null, 1, frame)!
    const px = frameRectToPixels(frame, CW, CH)
    expect(f.tx).toBeCloseTo(px.left + px.width / 2, 6)
    expect(f.ty).toBeCloseTo(px.top + px.height / 2, 6)
  })

  it('frameRect = null 时仍居中（预置框对到容器中心）', () => {
    const f = computeFitFor(0, 'contain', IMG, CW, CH, 0, '1:1', null, 1, null)!
    expect(f.tx).toBeCloseTo(CW / 2, 6)
    expect(f.ty).toBeCloseTo(CH / 2, 6)
  })

  it('无论框在哪，画面都**覆盖**满框（不露黑边）', () => {
    for (const frame of [
      { x: 0, y: 0, w: 0.3, h: 0.3 },
      { x: 0.7, y: 0.7, w: 0.3, h: 0.3 },
      { x: 0.35, y: 0.1, w: 0.3, h: 0.3 },
    ]) {
      for (const deg of [0, 90]) {
        const f = computeFitFor(deg, 'contain', IMG, CW, CH, 0, '1:1', null, 1, frame)!
        const px = frameRectToPixels(frame, CW, CH)
        const rot90 = Math.abs((((deg % 180) + 180) % 180) - 90) < 1e-9
        const fw = rot90 ? IMG.height : IMG.width
        const fh = rot90 ? IMG.width : IMG.height
        expect(fw * f.scale, `frame@${JSON.stringify(frame)} ${deg}deg 宽覆盖`).toBeGreaterThanOrEqual(
          px.width - 1e-6,
        )
        expect(fh * f.scale, `frame@${JSON.stringify(frame)} ${deg}deg 高覆盖`).toBeGreaterThanOrEqual(
          px.height - 1e-6,
        )
      }
    }
  })
})

describe('切换比例 —— 必须重算刷新（问题 1）', () => {
  it('不同比例的预置框确实不同，且各自满足自己的比例', () => {
    const a = frameRectToPixels(effectiveFrameRect(CW, CH, ASPECT_VALUES['1:1'], null, 1), CW, CH)
    const b = frameRectToPixels(effectiveFrameRect(CW, CH, ASPECT_VALUES['16:9'], null, 1), CW, CH)
    // 正方形在 1600x970 里受高限制 → 970x970；16:9 受宽限制 → 1600x900
    expect(a.width).toBeCloseTo(a.height, 6)
    expect(b.width / b.height).toBeCloseTo(16 / 9, 6)
    // 尺寸确实不同（否则就没刷新）
    expect(Math.abs(a.width - b.width) + Math.abs(a.height - b.height)).toBeGreaterThan(1)
  })

  it('每次切比例都用 contain 重算，scale 跟着变', () => {
    const f1 = computeFitFor(0, 'contain', IMG, CW, CH, 0, '1:1', null, 1, null)!
    const f2 = computeFitFor(0, 'contain', IMG, CW, CH, 0, '16:9', null, 1, null)!
    expect(f1.scale).not.toBeCloseTo(f2.scale, 6)
  })
})

describe('imageVisibleRect —— 图像可见矩形（自由模式预置框 + 拖框可行域）', () => {
  it('图像完全在视口内 → 就是图像自身的矩形', () => {
    // 图像 200×100、scale 1，居中放在 400×300 容器
    const r = imageVisibleRect(200, 150, 1, 0, 200, 100, 400, 300)
    expect(r.w * 400).toBeCloseTo(200, 6)
    expect(r.h * 300).toBeCloseTo(100, 6)
    expect(r.x * 400).toBeCloseTo(100, 6)
    expect(r.y * 300).toBeCloseTo(100, 6)
  })

  it('图像比视口大 → 结果就是整个视口', () => {
    const r = imageVisibleRect(200, 150, 4, 0, 200, 100, 400, 300)
    expect(r).toEqual({ x: 0, y: 0, w: 1, h: 1 })
  })

  it('90° 时用旋转后的外包尺寸（宽高互换）', () => {
    const r = imageVisibleRect(200, 150, 1, 90, 200, 100, 400, 300)
    expect(r.w * 400).toBeCloseTo(100, 6)
    expect(r.h * 300).toBeCloseTo(200, 6)
  })

  it('结果永远夹在视口内（含图偏到视口外的情况）', () => {
    const cases: [number, number, number][] = [
      [0, 0, 1],
      [-500, -500, 0.5],
      [999, 999, 8],
    ]
    for (const [tx, ty, s] of cases) {
      const r = imageVisibleRect(tx, ty, s, 33, IMG.width, IMG.height, CW, CH)
      expect(r.x, `tx=${tx} 左`).toBeGreaterThanOrEqual(0)
      expect(r.y, `ty=${ty} 上`).toBeGreaterThanOrEqual(0)
      expect(r.x + r.w, `tx=${tx} 右`).toBeLessThanOrEqual(1 + 1e-9)
      expect(r.y + r.h, `ty=${ty} 下`).toBeLessThanOrEqual(1 + 1e-9)
    }
  })

  it('容器或图像尺寸为 0 时退回整视口（不产生 NaN）', () => {
    expect(imageVisibleRect(0, 0, 1, 0, 0, 0, CW, CH)).toEqual({ x: 0, y: 0, w: 1, h: 1 })
    expect(imageVisibleRect(0, 0, 1, 0, 100, 100, 0, 0)).toEqual({ x: 0, y: 0, w: 1, h: 1 })
  })
})

describe('activeFrameRect —— 当前生效的框（两种模式的统一入口）', () => {
  const V = { tx: CW / 2, ty: CH / 2, scale: 1, rotation: 0, fineAngle: 0 }

  it('拖过框 → 直接用用户的框（两种模式都一样）', () => {
    const custom = { x: 0.1, y: 0.2, w: 0.3, h: 0.4 }
    expect(activeFrameRect(custom, 'free', CW, CH, V, IMG)).toEqual(custom)
    expect(activeFrameRect(custom, '16:9', CW, CH, V, IMG)).toEqual(custom)
  })

  it('自由 + 没拖过 → 图像可见矩形', () => {
    const r = activeFrameRect(null, 'free', CW, CH, V, IMG)!
    expect(r).toEqual(
      imageVisibleRect(V.tx, V.ty, V.scale, V.rotation + V.fineAngle, IMG.width, IMG.height, CW, CH),
    )
  })

  /// 固定比例 + 没拖过 → 框 = **后端将裁那块区域**的屏幕投影。
  ///
  /// ⚠️ 这里刻意用 `scale = 1`（图像按原始像素铺开、远大于容器）：
  /// 投影语义下框会**溢出视口** —— 因为后端确实要裁那么大一块，
  /// 框如实反映它。旧实现是靠把画面缩到框上（`setAspect` 重算视图）
  /// 让框永远"正好"，2026-10-11 用户要求去掉那个副作用。
  it('固定比例 + 没拖过 → 投影（图很大时会溢出视口，不是钉在视口中央）', () => {
    const r = activeFrameRect(null, '1:1', CW, CH, V, IMG)!
    const px = frameRectToPixels(r, CW, CH)
    // 正方形（图像 1.5:1 取 1:1 → 高度用满）
    expect(px.width).toBeCloseTo(px.height, 6)
    expect(px.height).toBeCloseTo(IMG.height, 6)
    // 中心落在图像中心
    expect(px.left + px.width / 2).toBeCloseTo(V.tx, 6)
    expect(px.top + px.height / 2).toBeCloseTo(V.ty, 6)
    // 图远大于容器 → 框确实溢出（这条断言本身就在钉住"投影"语义）
    expect(px.width).toBeGreaterThan(CW)
  })

  it('尺寸未就绪 / 自由模式下无图 → null', () => {
    expect(activeFrameRect(null, 'free', 0, 0, V, IMG)).toBeNull()
    expect(activeFrameRect(null, 'free', CW, CH, V, null)).toBeNull()
  })
})

describe('自由模式的预置框 = 整图（统一后「没拖过 = 不裁剪」的前提）', () => {
  it('contain 显示下，预置框反变换回图像坐标应覆盖整张图', () => {
    const fit = computeFitFor(0, 'contain', IMG, CW, CH, 0, 'free', null, 1, null)!
    const v = { tx: fit.tx, ty: fit.ty, scale: fit.scale, rotation: 0, fineAngle: 0 }
    const fr = activeFrameRect(null, 'free', CW, CH, v, IMG)!
    const px = frameRectToPixels(fr, CW, CH)
    const crop = rectToCrop(
      px.left,
      px.top,
      px.left + px.width,
      px.top + px.height,
      v.tx,
      v.ty,
      v.scale,
      0,
      IMG.width,
      IMG.height,
    )
    expect(crop, '应能反算出 crop').not.toBeNull()
    expect(crop!.x).toBeCloseTo(0, 2)
    expect(crop!.y).toBeCloseTo(0, 2)
    expect(crop!.w).toBeCloseTo(1, 2)
    expect(crop!.h).toBeCloseTo(1, 2)
  })
})

/**
 * 固定比例下「没拖过框」时，框必须画在**后端将要裁的那块区域**上。
 *
 * 契约（见 `crop` 派生 effect）：`frameRect === null` → 前端**不产出 `crop`**，
 * 保存时由后端在**旋转后的整图**上按比例居中裁（`pv_decode::crop_to_aspect`）。
 * 所以框 = 那块区域的屏幕投影。
 *
 * 旧实现是靠 `setAspect` 重算 scale/tx/ty（把画面缩放平移到框上）来让两者重合的，
 * 副作用是**换个比例就把用户的构图挪走** —— 2026-10-11 用户要求去掉它：
 * 「选取了比例后，画面还是保持原来的大小，不需要调整适应」。
 */
describe('activeFrameRect —— 固定比例预置框 = 后端将裁区域的投影（2026-10-11）', () => {
  /** 与画布实际一致：图像 contain 进容器后的视图（图 1.5:1，容器 1.65:1） */
  const contained = (() => {
    const s = Math.min(CW / IMG.width, CH / IMG.height)
    return { tx: CW / 2, ty: CH / 2, scale: s, rotation: 0, fineAngle: 0 }
  })()

  const pxOf = (v: typeof contained, ratio: Parameters<typeof activeFrameRect>[1]) => {
    const fr = activeFrameRect(null, ratio, CW, CH, v, IMG)!
    return frameRectToPixels(fr, CW, CH)
  }

  it('画面适应窗口时，框 = 图像内该比例的最大居中矩形', () => {
    const px = pxOf(contained, '1:1')
    const vis = imageVisibleRect(
      contained.tx, contained.ty, contained.scale, 0, IMG.width, IMG.height, CW, CH,
    )
    // 正方形：边长取可见区域的高（图比容器宽，高度是限制边）
    expect(px.width).toBeCloseTo(px.height, 6)
    expect(px.height).toBeCloseTo(vis.h * CH, 4)
    // 居中于图像
    expect(px.left + px.width / 2).toBeCloseTo((vis.x + vis.w / 2) * CW, 4)
    expect(px.top + px.height / 2).toBeCloseTo((vis.y + vis.h / 2) * CH, 4)
  })

  /// ⭐ 核心：投影必须跟着**图像**走。画面缩放/平移后，框随图像一起动，
  /// 而不是永远待在容器中央。
  it('画面缩放平移后，框跟着图像（而不是钉在容器上）', () => {
    const moved = { tx: 300, ty: 200, scale: contained.scale * 0.5, rotation: 0, fineAngle: 0 }
    const px = pxOf(moved, '1:1')
    // 中心必须落在图像中心（tx, ty）上 —— 这是"投影"的定义
    expect(px.left + px.width / 2).toBeCloseTo(moved.tx, 6)
    expect(px.top + px.height / 2).toBeCloseTo(moved.ty, 6)
    // 尺寸随 scale 等比缩小
    const fullPx = pxOf(contained, '1:1')
    expect(px.width).toBeCloseTo(fullPx.width / 2, 6)
  })

  /// 90° 旋转：`crop` 是相对**原图**归一化的，旋转只互换宽高 ——
  /// 映射回原图时 w/h 交叉，写错的直接后果是裁出错误的宽高比。
  it('90° 旋转下仍然得到正确的裁剪比例', () => {
    const rot = { ...contained, rotation: 90 }
    const px = pxOf(rot, '1:1')
    expect(px.width).toBeCloseTo(px.height, 6)

    // 16:9 时，屏幕上的比例应等于"旋转后图像"里的 16:9
    const px169 = pxOf(rot, '16:9')
    const cropped = rectToCrop(
      px169.left, px169.top, px169.left + px169.width, px169.top + px169.height,
      rot.tx, rot.ty, rot.scale, rot.rotation + rot.fineAngle, IMG.width, IMG.height,
    )!
    // 反算出的 crop 在「旋转后图像」里的宽高比必须是 16:9。
    // ⚠️ 交叉：`crop` 是相对**原图**归一化的，旋转 90° 后原图的宽成了旋转后图像的高
    const rotW = cropped.h * IMG.height
    const rotH = cropped.w * IMG.width
    expect(rotW / rotH).toBeCloseTo(16 / 9, 4)
    // 屏幕上的比例：旋转 90° 后，"旋转后图像"（3898×5847 的竖图）里 16:9 的
    // 扁矩形投影到屏幕上就是 16:9（宽 = 旋转后图的宽方向）。
    // ⚠️ 不要拿它跟 `screenRatioFor` 比 —— 那个函数服务的是**另一套**坐标语义
    // （crop 相对"旋转后图像"），与"框在屏幕上的比例"不是同一个量。
    expect(px169.width / px169.height).toBeCloseTo(16 / 9, 4)
  })

  it('自由比例仍然框住整张可见的图（不裁剪）', () => {
    const fr = activeFrameRect(null, 'free', CW, CH, contained, IMG)!
    const vis = imageVisibleRect(
      contained.tx, contained.ty, contained.scale, 0, IMG.width, IMG.height, CW, CH,
    )
    expect(fr.x).toBeCloseTo(vis.x, 9)
    expect(fr.y).toBeCloseTo(vis.y, 9)
    expect(fr.w).toBeCloseTo(vis.w, 9)
    expect(fr.h).toBeCloseTo(vis.h, 9)
  })

  /// 用户拖过框之后（`frameRect` 非空），画面再动也不该改框的位置 ——
  /// 否则用户摆好的取景会被一次滚轮缩放毁掉。
  it('用户拖过框之后，画面变化不再改动框', () => {
    const custom = { x: 0.2, y: 0.3, w: 0.4, h: 0.25 }
    const fr = activeFrameRect(custom, '1:1', CW, CH, contained, IMG)!
    expect(fr).toEqual(custom)
  })

  it('无图 / 容器为 0 时不返回框', () => {
    expect(activeFrameRect(null, '1:1', CW, CH, contained, null)).toBeNull()
    expect(activeFrameRect(null, '1:1', 0, 0, contained, IMG)).toBeNull()
  })
})

describe('aspectCenterCrop —— 与后端 crop_to_aspect 同规则', () => {
  it('不旋转时就是整图上该比例的最大居中矩形', () => {
    const c = aspectCenterCrop(1, IMG, 0)!
    // 图 1.5:1，取 1:1 → 高度用满、左右各裁
    expect(c.h).toBeCloseTo(1, 9)
    expect(c.w).toBeCloseTo(IMG.height / IMG.width, 9)
    expect(c.x).toBeCloseTo((1 - c.w) / 2, 9)
    expect(c.y).toBeCloseTo(0, 9)
  })

  /**
   * ⭐ 归一化基准是**旋转后的图**（后端先旋转再裁剪），所以：
   * - 90°/270° 时矩形短边是「旋转后图的宽」= 原图的高；
   * - 归一化值直接除以旋转后尺寸，**不许再和原图宽高交叉一次**
   *   （旧版多交叉了一次 → 框是正方形、裁出来是 0.44 扁图）。
   */
  it('旋转 90° 时按「旋转后图」归一化（不再交叉宽高）', () => {
    // 旋转后是 3898×5847 的竖图，取 16:9（宽扁）→ 宽度用满、上下裁
    const c = aspectCenterCrop(16 / 9, IMG, 1)!
    expect(c.w).toBeCloseTo(1, 9)
    expect(c.h).toBeCloseTo(IMG.height / 16 * 9 / IMG.width, 9)
    // 与不旋转明显不同 —— 证明这条断言不是白写的
    // 与不旋转明显不同 —— 证明这条断言不是白写的
    // 16:9 比两个朝向都「宽」，所以宽度都吃满，差别体现在高度上
    const flat = aspectCenterCrop(16 / 9, IMG, 0)!
    expect(flat.w).toBeCloseTo(1, 9)
    expect(flat.h).toBeCloseTo(IMG.width / 16 * 9 / IMG.height, 9)
    expect(Math.abs(flat.h - c.h)).toBeGreaterThan(0.1)

    // 1:1：取旋转后图的短边（= 原图的高）
    const sq = aspectCenterCrop(1, IMG, 1)!
    expect(sq.w).toBeCloseTo(1, 9)
    expect(sq.h).toBeCloseTo(IMG.height / IMG.width, 9)
    // ← 这两条正是「0.44 扁图」bug 的反面：w 必须用满
    expect(aspectCenterCrop(1, IMG, 0)!.h).toBeCloseTo(1, 9)
  })

  it('比例与图像同向时不裁剪（crop 覆盖整图）', () => {
    expect(aspectCenterCrop(IMG.width / IMG.height, IMG, 0)!.w).toBeCloseTo(1, 9)
    expect(aspectCenterCrop(IMG.width / IMG.height, IMG, 0)!.h).toBeCloseTo(1, 9)
    // 转 90° 后「同向」变成竖比
    expect(aspectCenterCrop(IMG.height / IMG.width, IMG, 1)!.w).toBeCloseTo(1, 9)
    expect(aspectCenterCrop(IMG.height / IMG.width, IMG, 1)!.h).toBeCloseTo(1, 9)
  })

  it('任何旋转角下，crop 的像素比都等于目标比', () => {
    for (const rot of [0, 1, 2, 3]) {
      const { w: rotW, h: rotH } = rotatedImageSize(IMG.width, IMG.height, rot)
      for (const target of [1, 3 / 4, 16 / 9, 9 / 16, 2.35]) {
        const c = aspectCenterCrop(target, IMG, rot)!
        expect((c.w * rotW) / (c.h * rotH), `rot=${rot * 90}° target=${target}`).toBeCloseTo(
          target,
          9,
        )
        // 居中
        expect(c.x + c.w / 2).toBeCloseTo(0.5, 9)
        expect(c.y + c.h / 2).toBeCloseTo(0.5, 9)
      }
    }
  })

  it('非法输入返回 null', () => {
    expect(aspectCenterCrop(0, IMG, 0)).toBeNull()
    expect(aspectCenterCrop(1, { width: 0, height: 0 }, 0)).toBeNull()
  })
})

/**
 * ⭐ 裁剪坐标系 = **旋转后的图**（2026-10-11 统一，与后端 `apply_edits` 对齐）。
 *
 * 这是整套框选的地基，也是「顺逆 90° 旋转后选固定比例、调整大小不正常」的根因：
 * 旧版 `rectToCrop` 按**原图**归一化，与后端差一次 90° 的宽高互换，
 * 于是预置框「看着是正方形、裁出来是扁的」，拖框时还得用一个
 * `(H/W)²/target` 的假比例去凑 —— 那个假比例正是拖拽跳变的来源。
 *
 * 这里用**具体数字**把约定钉死：屏幕上「旋转后图的左上四分之一」，
 * 反算出来必须正好是 `{0, 0, 0.5, 0.5}`。
 */
describe('裁剪坐标系 = 旋转后的图（换算法钉死，含 90°）', () => {
  const cases: Array<[number, number]> = [
    [0, 0],
    [1, 90],
    [2, 180],
    [3, 270],
  ]

  it('屏幕上「旋转后图的左上四分之一」→ crop {0, 0, 0.5, 0.5}', () => {
    for (const [rot, deg] of cases) {
      const { w: rotW, h: rotH } = rotatedImageSize(IMG.width, IMG.height, rot)
      const scale = Math.min(CW / rotW, CH / rotH)
      const tx = CW / 2
      const ty = CH / 2
      const qx = tx - (rotW * scale) / 2 // 屏幕上的左上角
      const qy = ty - (rotH * scale) / 2

      const crop = rectToCrop(qx, qy, tx, ty, tx, ty, scale, 0, rotW, rotH)!
      expect(crop.x, `${deg}° x`).toBeCloseTo(0, 9)
      expect(crop.y, `${deg}° y`).toBeCloseTo(0, 9)
      expect(crop.w, `${deg}° w`).toBeCloseTo(0.5, 9)
      expect(crop.h, `${deg}° h`).toBeCloseTo(0.5, 9)
    }
  })

  it('cropToScreenRect ∘ rectToCrop 恒等（fineAngle = 0 时精确互逆）', () => {
    // ⚠️ 矩形必须完整落在**图像**内，否则 `rectToCrop` 会夹边（那就不互逆了）。
    // 90°/270° 时图像是竖的，所以取一个居中、尺寸保守的框。
    const rect = { left: CW / 2 - 150, top: CH / 2 - 100, width: 300, height: 200 }
    for (const [rot] of cases) {
      const { w: rotW, h: rotH } = rotatedImageSize(IMG.width, IMG.height, rot)
      const scale = Math.min(CW / rotW, CH / rotH)
      const tx = CW / 2
      const ty = CH / 2
      const crop = rectToCrop(
        rect.left, rect.top, rect.left + rect.width, rect.top + rect.height,
        tx, ty, scale, 0, rotW, rotH,
      )!
      const back = cropToScreenRect(crop, tx, ty, scale, 0, rotW, rotH)
      expect(back.left, `${rot * 90}° left`).toBeCloseTo(rect.left, 6)
      expect(back.top, `${rot * 90}° top`).toBeCloseTo(rect.top, 6)
      expect(back.width, `${rot * 90}° width`).toBeCloseTo(rect.width, 6)
      expect(back.height, `${rot * 90}° height`).toBeCloseTo(rect.height, 6)
    }
  })
})
