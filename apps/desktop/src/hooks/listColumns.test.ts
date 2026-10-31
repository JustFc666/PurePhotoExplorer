import { describe, expect, it } from 'vitest'

import {
  clampWidth,
  formatDateTime,
  formatMtime,
  formatRating,
  LIST_COLUMNS,
  COLUMN_BY_ID,
} from './listColumns'

describe('formatDateTime', () => {
  it('秒 → 本地时间字符串', () => {
    const sec = new Date(2026, 9, 31, 9, 6, 0).getTime() / 1000
    expect(formatDateTime(sec)).toBe('2026-10-31 09:06')
  })

  it('空值给破折号，不能留空白', () => {
    expect(formatDateTime(null)).toBe('—')
    expect(formatDateTime(undefined)).toBe('—')
    expect(formatDateTime(0)).toBe('—')
    expect(formatDateTime(Number.NaN)).toBe('—')
  })
})

describe('formatMtime', () => {
  it('纳秒 → 秒（后端存的是 mtime_ns）', () => {
    const sec = new Date(2026, 0, 2, 3, 4, 0).getTime() / 1000
    expect(formatMtime(sec * 1e9)).toBe('2026-01-02 03:04')
  })

  it('空值给破折号', () => {
    expect(formatMtime(null)).toBe('—')
    expect(formatMtime(0)).toBe('—')
  })
})

describe('formatRating', () => {
  it('0 星给破折号，避免一排空星', () => {
    expect(formatRating(0)).toBe('—')
    expect(formatRating(-1)).toBe('—')
  })

  it('星级用实心星', () => {
    expect(formatRating(1)).toBe('★')
    expect(formatRating(5)).toBe('★★★★★')
    // 脏数据不应渲染出 8 颗星
    expect(formatRating(8)).toBe('★★★★★')
  })
})

describe('clampWidth', () => {
  it('低于下限被夹住', () => {
    expect(clampWidth('format', 10)).toBe(COLUMN_BY_ID.format.min)
  })

  it('超过上限被夹住（防手滑拖出几万像素）', () => {
    expect(clampWidth('name', 99999)).toBe(640)
  })

  it('正常值原样四舍五入', () => {
    expect(clampWidth('name', 300.6)).toBe(301)
  })
})

describe('列定义自洽', () => {
  it('文件名 / 格式 / 大小 必须不可关闭', () => {
    const locked = LIST_COLUMNS.filter((c) => c.locked).map((c) => c.id)
    expect(locked).toContain('name')
    expect(locked).toContain('format')
    expect(locked).toContain('size')
  })

  it('每列默认宽度都不小于自己的下限', () => {
    for (const c of LIST_COLUMNS) {
      expect(c.width, `${c.id} 默认宽度小于 min`).toBeGreaterThanOrEqual(c.min)
    }
  })

  it('id 不重复（重复会让 localStorage 配置串味）', () => {
    const ids = LIST_COLUMNS.map((c) => c.id)
    expect(new Set(ids).size).toBe(ids.length)
  })
})
