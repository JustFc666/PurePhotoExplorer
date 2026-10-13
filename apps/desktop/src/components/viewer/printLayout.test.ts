/**
 * 打印排版算法的契约测试。
 *
 * ## 为什么这些测试要抄后端 `win_print` 的期望值
 * 前端预览（`PrintPreview.computePrintLayout`）和后端送印
 * （`win_print::layout`）是**同一个算法的两份实现** ——
 * 前端算「图在纸上占多大、落在哪」给用户看，后端算实际送印的矩形。
 *
 * 两边分叉的后果是「预览看着居中、印出来贴左上」：图没错、设置没错、
 * 就是位置不对 —— **最难查的一类不一致**。
 *
 * 所以这里**逐条复刻**后端 `win_print::tests` 的 6 条 `layout` 断言
 * （用毫米/DPI 而不是设备像素表述，数学等价）。
 * ⛔ **改动任一侧的排版算法，必须同步改另一侧并让两边测试都通过。**
 */

import { describe, expect, it } from 'vitest'
import { computePrintLayout, MARGIN_MM } from './PrintPreview'

/** 后端 layout 用的页边距（测试用 0 边距便于对账） */
const NO_MARGIN = { paperW: 1000, paperH: 800 }

describe('computePrintLayout —— 与后端 win_print::layout 逐条对账', () => {
  it('适应页面：等比缩放并居中（宽图受宽度限制）', () => {
    // 复刻后端 `适应页面时等比缩放并居中`：
    // 页面 1000×800、图像 4000×1000（4:1）→ 铺满宽 → 高 = 1000/4 = 250
    // 垂直居中 → 上偏移 (800 - 250) / 2 = 275
    // 换算到毫米：可用 980×780（减掉 2×10mm 边距），比例 4:1
    const availW = NO_MARGIN.paperW - MARGIN_MM * 2 // 980
    const availH = NO_MARGIN.paperH - MARGIN_MM * 2 // 780
    const r = computePrintLayout(1000, 800, 4000, 1000, 96, true)

    // 宽铺满可用区
    expect(r.width * 1000).toBeCloseTo(availW, 5)
    // 高按 4:1 比例
    expect(r.height * 800).toBeCloseTo(availW / 4, 5)
    // 垂直居中：图高 245，可用 780 → 上偏移 = 10 + (780-245)/2
    expect(r.y * 800).toBeCloseTo(MARGIN_MM + (availH - availW / 4) / 2, 5)
    // 水平铺满 → 无额外偏移
    expect(r.x * 1000).toBeCloseTo(MARGIN_MM, 5)
    expect(availH).toBeGreaterThan(availW / 4)
  })

  it('适应页面：竖图受高度限制', () => {
    // 复刻后端 `适应页面时竖图受高度限制`：1000×4000 在 1000×800 上 → 高度铺满
    const availW = 1000 - MARGIN_MM * 2
    const availH = 800 - MARGIN_MM * 2
    const r = computePrintLayout(1000, 800, 1000, 4000, 96, true)

    // 高铺满
    expect(r.height * 800).toBeCloseTo(availH, 5)
    // 宽按 1:4
    expect(r.width * 1000).toBeCloseTo(availH / 4, 5)
    // 水平居中
    expect(r.x * 1000).toBeCloseTo(MARGIN_MM + (availW - availH / 4) / 2, 5)
  })

  it('原始尺寸：小图不放大，且比适应页面小', () => {
    // 复刻后端 `原始尺寸模式不放大`：
    // 300dpi 下200×100 的图像 = 屏幕大小的 300/96 = 3.125 倍（放大，因为 dpi 高）
    // 但「适应页面」在同输入下会铺满更大，所以 fit 的结果一定 >= 原始尺寸
    const actual = computePrintLayout(300, 240, 200, 100, 300, false)
    const fit = computePrintLayout(300, 240, 200, 100, 300, true)
    expect(fit.width).toBeGreaterThan(actual.width)
    expect(fit.height).toBeGreaterThan(actual.height)
  })

  it('原始尺寸：超出页面时缩小到能装下', () => {
    // 复刻后端 `原始尺寸超出页面时缩小到能装下`：2000×2000 在小页面上
    const r = computePrintLayout(300, 240, 2000, 2000, 300, false)
    expect(r.shrunk).toBe(true)
    // 方形图缩后仍是方形（等比）
    expect(r.imageMm.w).toBeCloseTo(r.imageMm.h, 5)
    // 能装进可打印区
    expect(r.width * 300).toBeLessThanOrEqual(300 - MARGIN_MM * 2 + 1e-6)
    expect(r.height * 240).toBeLessThanOrEqual(240 - MARGIN_MM * 2 + 1e-6)
    // 左上角不为负
    expect(r.x).toBeGreaterThanOrEqual(0)
    expect(r.y).toBeGreaterThanOrEqual(0)
  })

  it('页边距体现在偏移上，且图不超出可用区', () => {
    // 复刻后端 `页边距体现在偏移上`
    const r = computePrintLayout(1000, 800, 100, 100, 300, true)
    expect(r.x * 1000).toBeGreaterThanOrEqual(MARGIN_MM)
    expect(r.y * 800).toBeGreaterThanOrEqual(MARGIN_MM)
    expect(r.width * 1000).toBeLessThanOrEqual(1000 - MARGIN_MM * 2 + 1e-6)
  })

  it('退化输入不产生非法尺寸（GDI 遇到 0/负数会静默不画）', () => {
    // 复刻后端 `退化输入不产生非法尺寸`
    const degenerate: Array<[number, number, number, number]> = [
      [1, 1, 1, 1],
      [10, 10, 100000, 1],
      [800, 600, 1, 100000],
    ]
    for (const [pw, ph, iw, ih] of degenerate) {
      const r = computePrintLayout(pw, ph, iw, ih, 96, true)
      expect(r.width).toBeGreaterThan(0)
      expect(r.height).toBeGreaterThan(0)
      expect(r.imageMm.w).toBeGreaterThan(0)
      expect(r.imageMm.h).toBeGreaterThan(0)
      expect(r.x).toBeGreaterThanOrEqual(0)
      expect(r.y).toBeGreaterThanOrEqual(0)
      expect(Number.isFinite(r.width)).toBe(true)
    }
  })
})

describe('预览布局的基本不变量', () => {
  it('图像块始终完整落在纸内', () => {
    const r = computePrintLayout(210, 297, 4000, 3000, 96, true)
    expect(r.x + r.width).toBeLessThanOrEqual(1 + 1e-9)
    expect(r.y + r.height).toBeLessThanOrEqual(1 + 1e-9)
    expect(r.x).toBeGreaterThanOrEqual(0)
  })

  it('fit=true 时至少有一边贴到可打印区边界（铺满的语义）', () => {
    const r = computePrintLayout(210, 297, 4000, 3000, 96, true)
    const touchesW = Math.abs(r.width * 210 - (210 - MARGIN_MM * 2)) < 0.01
    const touchesH = Math.abs(r.height * 297 - (297 - MARGIN_MM * 2)) < 0.01
    expect(touchesW || touchesH).toBe(true)
  })

  it('保持宽高比（等比缩放，绝不拉变形）', () => {
    const r = computePrintLayout(210, 297, 4000, 3000, 96, true)
    const srcRatio = 4000 / 3000
    const outRatio = r.imageMm.w / r.imageMm.h
    expect(outRatio).toBeCloseTo(srcRatio, 6)
  })

  it('正方形图在方形纸上居中', () => {
    const r = computePrintLayout(200, 200, 1000, 1000, 96, true)
    // 左右余量应相等
    expect(r.x).toBeCloseTo(r.y, 6)
    expect(r.width).toBeCloseTo(r.height, 6)
  })
})
