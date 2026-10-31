import { describe, expect, it } from 'vitest'

import { baseName, parentPath } from './parentPath'

describe('parentPath', () => {
  it('普通目录往上退一层', () => {
    expect(parentPath('E:\\a\\b')).toBe('E:\\a')
    expect(parentPath('E:/a/b')).toBe('E:/a')
    expect(parentPath('E:\\a\\b\\')).toBe('E:\\a')
  })

  it('盘根是终点，不再往上', () => {
    expect(parentPath('E:\\')).toBeNull()
    expect(parentPath('E:')).toBeNull()
  })

  it('盘下第一层回到盘根（补回反斜杠）', () => {
    expect(parentPath('E:\\a')).toBe('E:\\')
  })

  it('压缩包的上一级是它所在的目录，不是它自己', () => {
    expect(parentPath('C:\\Coser\\新毛披肩.zip')).toBe('C:\\Coser')
  })

  it('UNC 共享根也是终点', () => {
    expect(parentPath('\\\\nas\\share\\a')).toBe('\\\\nas\\share')
    expect(parentPath('\\\\nas\\share')).toBeNull()
  })
})

describe('baseName', () => {
  it('取最后一段，盘根取盘符', () => {
    expect(baseName('E:\\a\\b')).toBe('b')
    expect(baseName('E:\\')).toBe('E:')
    expect(baseName('C:\\Coser\\写真合集')).toBe('写真合集')
  })
})
