/**
 * `ipc/format.ts` 的行为契约。
 *
 * 这个模块决定「保存按钮长什么样、能不能点」，而它的判定逻辑在前后端
 * 各写了一份（`isWritableFormat` ↔ Rust `can_overwrite_original`）。
 * **两份实现漂移不会报错**，只会让用户看到一个点不动的按钮。
 *
 * 2026-10-10：RAW 从「完全不能保存」改成「只能另存为」，
 * 这批测试就是为了钉住那个转向，别退回去。
 */

import { describe, expect, it } from 'vitest'
import {
  EXPORT_FORMATS,
  isWritableFormat,
  notWritableReason,
  rawExportHint,
} from './format'

describe('isWritableFormat —— 能否覆盖写回原文件', () => {
  it('JPEG / PNG / BMP 可以覆盖', () => {
    expect(isWritableFormat('jpeg')).toBe(true)
    expect(isWritableFormat('jpg')).toBe(true) // 兜底：info.format 可能是 jpg
    expect(isWritableFormat('png')).toBe(true)
    expect(isWritableFormat('bmp')).toBe(true)
  })

  it('大小写不敏感', () => {
    expect(isWritableFormat('JPEG')).toBe(true)
    expect(isWritableFormat('JpG')).toBe(true)
  })

  it('`jpeg` 与 `jpg` 都要认', () => {
    // 后端目前给的是枚举变体名 `jpeg`，但 `jpg` 是最常见的扩展名，
    // 留作兜底：白名单漏一个的表现是「保存按钮莫名置灰」，很难排查。
    expect(isWritableFormat('jpeg')).toBe(true)
    expect(isWritableFormat('jpg')).toBe(true)
  })

  it('RAW 不可覆盖 —— 这是 RAW 只能另存为的根本原因', () => {
    expect(isWritableFormat('cr3')).toBe(false)
    expect(isWritableFormat('cr2')).toBe(false)
    expect(isWritableFormat('nef')).toBe(false)
    expect(isWritableFormat('arw')).toBe(false)
    expect(isWritableFormat('dng')).toBe(false)
    expect(isWritableFormat('raf')).toBe(false)
    expect(isWritableFormat('orf')).toBe(false)
    expect(isWritableFormat('rw2')).toBe(false)
    expect(isWritableFormat('nrw')).toBe(false)
    expect(isWritableFormat('sr2')).toBe(false)
  })

  it('WebP / HEIF / GIF / PSD 也不可覆盖（与后端一致）', () => {
    expect(isWritableFormat('webp')).toBe(false)
    expect(isWritableFormat('heif')).toBe(false)
    expect(isWritableFormat('heic')).toBe(false)
    expect(isWritableFormat('gif')).toBe(false)
    expect(isWritableFormat('psd')).toBe(false)
  })
})

describe('notWritableReason —— 措辞必须说清「有出口」', () => {
  it('RAW 的文案必须提到「另存为」', () => {
    const msg = notWritableReason('Canon CR3', true)
    expect(msg).toContain('另存为')
  })

  it('⛔ 不能说成「不提供保存 / 只读不写」—— 会让用户以为整条路断了', () => {
    const msg = notWritableReason('Canon CR3', true)
    expect(msg).not.toContain('只读不写')
    expect(msg).not.toContain('不提供覆盖保存')
    expect(msg).not.toContain('暂不支持')
  })

  it('非 RAW 的不可写格式也要给出路', () => {
    const msg = notWritableReason('WebP', false)
    expect(msg).toContain('另存为')
  })
})

describe('EXPORT_FORMATS —— 与后端 format_from_extension 对齐', () => {
  it('恰好是 jpg / png / bmp 三项', () => {
    expect(EXPORT_FORMATS.map((f) => f.ext)).toEqual(['jpg', 'png', 'bmp'])
  })

  it('⛔ 不能包含后端 format_from_extension 不认的扩展名', () => {
    // 后端只认 jpg/jpeg/png/bmp。加了 webp 之类会让用户选了却报错。
    const allowed = new Set(['jpg', 'jpeg', 'png', 'bmp'])
    for (const f of EXPORT_FORMATS) {
      expect(allowed.has(f.ext)).toBe(true)
    }
  })

  it('扩展名不带点（后端取 extension() 后不带点）', () => {
    for (const f of EXPORT_FORMATS) {
      expect(f.ext.startsWith('.')).toBe(false)
    }
  })
})

describe('rawExportHint —— 必须披露预览分辨率', () => {
  it('导出比原图小的时候要说明是内嵌预览', () => {
    const msg = rawExportHint(4032, 3024, 6000, 4000)
    expect(msg).toContain('4032 × 3024')
    expect(msg).toContain('6000 × 4000')
    expect(msg).toContain('内嵌预览')
    expect(msg).toContain('非全分辨率')
  })

  it('尺寸一致时不误报降级', () => {
    // 有些机型内嵌预览就是全尺寸，硬说降级是假警报
    const msg = rawExportHint(6000, 4000, 6000, 4000)
    expect(msg).toBe('已导出 6000 × 4000')
    expect(msg).not.toContain('内嵌预览')
  })

  it('拿不到原图尺寸时只报导出尺寸，不臆造对比', () => {
    const msg = rawExportHint(4032, 3024)
    expect(msg).toBe('已导出 4032 × 3024')
    expect(msg).not.toContain('内嵌预览')
  })

  it('导出比原图大时不误报（理论上不该发生，但要防御）', () => {
    const msg = rawExportHint(8000, 6000, 6000, 4000)
    expect(msg).not.toContain('非全分辨率')
  })
})