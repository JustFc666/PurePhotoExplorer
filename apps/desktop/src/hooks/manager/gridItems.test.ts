/**
 * 行模型归一化的**规则测试**。
 *
 * 组件只认 `GridItem`，所以这层错了整片网格就错了：文件夹跑到图片中间、
 * 浏览模式切方向没反应、压缩包丢了图标。以前这些都只能靠肉眼在真机上看。
 */

import { describe, expect, it } from 'vitest'
import { buildGridItems, type GridItemSource } from './gridItems'
import type { FsImage, FsNode } from '../../ipc/browse'
import type { ImageRow } from '../../ipc/manager'

function fsImage(over: Partial<FsImage> = {}): FsImage {
  return {
    path: 'C:/p/a.jpg',
    name: 'a.jpg',
    size: 100,
    mtime: 1_700_000_000,
    format: 'jpeg',
    isRaw: false,
    takenAt: null,
    width: 100,
    height: 100,
    ...over,
  }
}

function fsNode(over: Partial<FsNode> = {}): FsNode {
  return { name: 'sub', path: 'C:/p/sub', kind: 'dir', note: null, size: null, ...over }
}

function imageRow(over: Partial<ImageRow> = {}): ImageRow {
  return {
    id: 1,
    dirId: 1,
    relName: 'a.jpg',
    absPath: 'C:/p/a.jpg',
    size: 100,
    width: 100,
    height: 100,
    format: 'jpeg',
    isRaw: false,
    takenAt: null,
    rating: 0,
    flag: 0,
    hasTags: false,
    ...over,
  }
}

function src(over: Partial<GridItemSource> = {}): GridItemSource {
  return {
    scopeKind: 'browse',
    listing: null,
    subdirs: [],
    rows: [],
    sort: 'name',
    desc: false,
    ...over,
  }
}

describe('启动默认态', () => {
  it('none 一行都不给（rows 里可能还留着上次查询的结果）', () => {
    expect(buildGridItems(src({ scopeKind: 'none', rows: [imageRow()] }))).toEqual([])
  })
})

describe('浏览模式（未入库）', () => {
  it('文件夹永远排在最前，且只按名字排', () => {
    const items = buildGridItems(
      src({
        subdirs: [fsNode({ name: 'b', path: 'C:/p/b' }), fsNode({ name: 'a', path: 'C:/p/a' })],
        listing: { total: 1, items: [fsImage()], isArchive: false },
        // 就算按大小降序（文件夹没有大小），它们也不能被混进图片里
        sort: 'size',
        desc: true,
      }),
    )
    expect(items.map((i) => i.name)).toEqual(['a', 'b', 'a.jpg'])
    expect(items.map((i) => i.kind)).toEqual(['dir', 'dir', 'file'])
  })

  it('切方向会真的改变文件顺序（排序在内存里做）', () => {
    const listing = {
      total: 2,
      items: [fsImage({ name: 'small.jpg', size: 100 }), fsImage({ name: 'big.jpg', size: 900 })],
      isArchive: false,
    }
    const desc = buildGridItems(src({ listing, sort: 'size', desc: true }))
    const asc = buildGridItems(src({ listing, sort: 'size', desc: false }))
    expect(desc.map((i) => i.name)).toEqual(['big.jpg', 'small.jpg'])
    expect(asc.map((i) => i.name)).toEqual(['small.jpg', 'big.jpg'])
  })

  it('压缩包走文件夹分支但 kind 是 archive（卡片据此换图标）', () => {
    const items = buildGridItems(
      src({
        subdirs: [
          fsNode({ name: 'x.zip', path: 'C:/p/x.zip', kind: 'archive', size: 4096 }),
          fsNode({ name: 'sub', path: 'C:/p/sub' }),
        ],
      }),
    )
    const zip = items.find((i) => i.name === 'x.zip')
    expect(zip?.kind).toBe('archive')
    expect(zip?.key).toBe('arc:C:/p/x.zip')
    // 压缩包有大小（卡片右下角显示），文件夹没有
    expect(zip?.size).toBe(4096)
    expect(items.find((i) => i.name === 'sub')?.size).toBeNull()
    expect(items.find((i) => i.name === 'sub')?.key).toBe('dir:C:/p/sub')
  })

  it('浏览行一律 id=null + transient（不能参与任何索引侧操作）', () => {
    const items = buildGridItems(
      src({
        subdirs: [fsNode()],
        listing: { total: 1, items: [fsImage()], isArchive: false },
      }),
    )
    for (const i of items) {
      expect(i.id).toBeNull()
      expect(i.transient).toBe(true)
    }
  })

  it('还没读到列表时只显示文件夹（不崩）', () => {
    const items = buildGridItems(src({ subdirs: [fsNode()], listing: null }))
    expect(items).toHaveLength(1)
    expect(items[0]?.kind).toBe('dir')
  })
})

describe('入库模式', () => {
  it('key 用 id 字符串，transient=false', () => {
    const [it] = buildGridItems(src({ scopeKind: 'all', rows: [imageRow({ id: 42 })] }))
    expect(it?.key).toBe('42')
    expect(it?.id).toBe(42)
    expect(it?.transient).toBe(false)
    expect(it?.kind).toBe('file')
  })

  it('relName 缺失时回落成 #id（而不是空标题）', () => {
    const [it] = buildGridItems(src({ scopeKind: 'all', rows: [imageRow({ id: 7, relName: null })] }))
    expect(it?.name).toBe('#7')
  })

  it('评分 / 旗标 / 有无标签原样带过去（卡片要用）', () => {
    const [it] = buildGridItems(
      src({
        scopeKind: 'library',
        rows: [imageRow({ rating: 4, flag: 2, hasTags: true, takenAt: 1_600_000_000 })],
      }),
    )
    expect(it).toMatchObject({ rating: 4, flag: 2, hasTags: true, takenAt: 1_600_000_000 })
  })

  it('入库模式不掺浏览模式的残留数据', () => {
    const items = buildGridItems(
      src({
        scopeKind: 'all',
        rows: [imageRow()],
        subdirs: [fsNode()],
        listing: { total: 1, items: [fsImage({ name: 'stale.jpg' })], isArchive: false },
      }),
    )
    expect(items.map((i) => i.name)).toEqual(['a.jpg'])
  })
})
