import { describe, expect, it } from 'vitest'

import { resampleMks, type RgbaImage } from './mksResample'

/** 造一张纯色图 */
function solid(w: number, h: number, r: number, g: number, b: number): RgbaImage {
  const data = new Uint8ClampedArray(w * h * 4)
  for (let i = 0; i < w * h; i++) {
    data[i * 4] = r
    data[i * 4 + 1] = g
    data[i * 4 + 2] = b
    data[i * 4 + 3] = 255
  }
  return { data, width: w, height: h }
}

/** 造一张水平灰阶渐变图 */
function gradient(w: number, h: number): RgbaImage {
  const data = new Uint8ClampedArray(w * h * 4)
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const v = Math.round((x / (w - 1)) * 255)
      const i = (y * w + x) * 4
      data[i] = v
      data[i + 1] = v
      data[i + 2] = v
      data[i + 3] = 255
    }
  }
  return { data, width: w, height: h }
}

/** 造一张黑白棋盘（用于验证降采样是否真的在做区域平均） */
function checkerboard(size: number): RgbaImage {
  const data = new Uint8ClampedArray(size * size * 4)
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const v = (x + y) % 2 === 0 ? 255 : 0
      const i = (y * size + x) * 4
      data[i] = v
      data[i + 1] = v
      data[i + 2] = v
      data[i + 3] = 255
    }
  }
  return { data, width: size, height: size }
}

function grayAt(img: RgbaImage, x: number, y: number): number {
  return img.data[(y * img.width + x) * 4]!
}

describe('resampleMks', () => {
  it('纯色缩放后仍是同一个颜色（不能有亮度漂移）', () => {
    const src = solid(64, 64, 120, 80, 200)
    const out = resampleMks(src, 16, 16)
    expect(out.width).toBe(16)
    expect(out.height).toBe(16)
    for (let i = 0; i < 16 * 16; i++) {
      expect(out.data[i * 4]).toBeCloseTo(120, 0)
      expect(out.data[i * 4 + 1]).toBeCloseTo(80, 0)
      expect(out.data[i * 4 + 2]).toBeCloseTo(200, 0)
      expect(out.data[i * 4 + 3]).toBe(255)
    }
  })

  it('输出尺寸与请求一致（含非等比）', () => {
    const out = resampleMks(solid(200, 100, 60, 60, 60), 80, 41)
    expect(out.width).toBe(80)
    expect(out.height).toBe(41)
    expect(out.data.length).toBe(80 * 41 * 4)
  })

  it('2×2 棋盘缩到 1 像素应接近中灰（证明确实在做低通）', () => {
    const out = resampleMks(checkerboard(2), 1, 1)
    const v = grayAt(out, 0, 0)
    // 线性光下 0 与 1 各占一半 = 0.5 → sRGB 约 188；
    // 若在 sRGB 空间直接平均会得到 128。这里断言「不是 128」，说明走了线性光。
    expect(v).toBeGreaterThan(170)
    expect(v).toBeLessThan(200)
  })

  it('灰阶渐变保持单调不减', () => {
    const out = resampleMks(gradient(240, 8), 40, 8)
    let prev = -1
    for (let x = 0; x < out.width; x++) {
      const v = grayAt(out, x, 0)
      expect(v).toBeGreaterThanOrEqual(prev)
      prev = v
    }
  })

  it('sharp=true 比 sharp=false 更锐（相邻像素差更大）', () => {
    const src = checkerboard(64)
    const soft = resampleMks(src, 12, 12, false)
    const sharp = resampleMks(src, 12, 12, true)

    const contrast = (img: RgbaImage) => {
      let max = 0
      for (let y = 0; y < img.height; y++) {
        for (let x = 1; x < img.width; x++) {
          max = Math.max(max, Math.abs(grayAt(img, x, y) - grayAt(img, x - 1, y)))
        }
      }
      return max
    }

    expect(contrast(sharp)).toBeGreaterThan(contrast(soft))
  })

  it('放大走双线性兜底，不抛错', () => {
    const out = resampleMks(solid(8, 8, 10, 20, 30), 20, 20)
    expect(out.width).toBe(20)
    expect(out.data[0]).toBeCloseTo(10, 0)
  })
})
