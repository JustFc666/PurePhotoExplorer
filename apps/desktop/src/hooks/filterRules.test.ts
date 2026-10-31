import { describe, expect, it } from 'vitest'

import { resolveHasTags } from './filterRules'

describe('resolveHasTags', () => {
  it('按具体标签筛时，「无标签」必须让路', () => {
    // 这就是用户 2026-10-31 报的那个 bug：点「半身」主区空白
    expect(resolveHasTags('none', ['半身'])).toBeUndefined()
    expect(resolveHasTags('has', ['半身'])).toBeUndefined()
  })

  it('没有具体标签时才按开关来', () => {
    expect(resolveHasTags('none', [])).toBe(false)
    expect(resolveHasTags('none', undefined)).toBe(false)
    expect(resolveHasTags('has', [])).toBe(true)
  })

  it('不限（null）就不拼这个条件', () => {
    expect(resolveHasTags(null, [])).toBeUndefined()
  })
})
