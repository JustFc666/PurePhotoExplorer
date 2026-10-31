/**
 * 右键菜单的**规则**测试。
 *
 * 这些规则以前只能靠人眼在 3000 行的 `ManagerView.tsx` 里守，现在菜单被提成
 * 纯函数（`cardMenu.ts`），可以直接喂输入看输出。这里测的是**行为规则**，
 * 不是文案 —— 改文案不该让测试变红，改规则才该。
 */

import { describe, expect, it } from 'vitest'

import { buildBlankMenu, buildCardMenu, type CardMenuInput } from './cardMenu'
import type { BlankMenuInput } from './cardMenu'
import type { GridItem, ManagerApi } from '../../hooks/useManager'
import type { ContextMenuItem } from '../chrome/ContextMenu'

const noop = () => {}

/** 造一行。默认是「已入库的图片」，按需覆盖。 */
function row(over: Partial<GridItem> = {}): GridItem {
  return {
    key: 'k1',
    id: 1,
    path: 'C:/p/a.jpg',
    name: 'a.jpg',
    size: 1024,
    mtime: null,
    takenAt: null,
    width: 100,
    height: 100,
    format: 'jpeg',
    rating: 0,
    flag: 0,
    hasTags: false,
    transient: false,
    kind: 'file',
    ...over,
  }
}

/**
 * 只构造菜单**真正读**的那些字段（libraries / personFilter / tags / viewMode /
 * 三个动作）。其余用 `as unknown as ManagerApi` 跳过 —— 菜单不该碰它们，
 * 真碰了会立刻崩在测试里，比补全 60 个字段更早暴露问题。
 */
function fakeMgr(over: Partial<ManagerApi> = {}): ManagerApi {
  return {
    libraries: [],
    personFilter: null,
    tags: [],
    viewMode: 'grid',
    setRating: noop,
    setFlag: noop,
    gotoDir: noop,
    isIdle: false,
    busy: false,
    total: 3,
    selectAll: noop,
    clearSelection: noop,
    rescan: noop,
    ...over,
  } as unknown as ManagerApi
}

function cardInput(over: Partial<CardMenuInput> = {}): CardMenuInput {
  return {
    cardCtxItems: [row()],
    mgr: fakeMgr(),
    exporting: false,
    addDirToLibrary: noop,
    gotoWithReveal: noop,
    toggleTag: noop,
    setPersonCover: noop,
    openTagPicker: noop,
    setRenameFor: noop,
    setInlineRename: noop,
    setExportNotice: noop,
    copyPaths: noop,
    doExport: noop,
    doSaveAs: noop,
    doCopyTo: noop,
    doMoveTo: noop,
    doTransform: noop,
    doConvert: noop,
    doAutoOrient: noop,
    doPrint: noop,
    doDelete: noop,
    playCount: 1,
    playSlideshow: noop,
    ...over,
  }
}

function blankInput(over: Partial<BlankMenuInput> = {}): BlankMenuInput {
  return {
    mgr: fakeMgr(),
    curDir: { path: 'C:/p', name: 'p' },
    selectedItems: [],
    addDirToLibrary: noop,
    selectedCount: 0,
    playCount: 0,
    playSlideshow: noop,
    ...over,
  }
}

/** 按 id 找菜单项。 */
function byId(items: ContextMenuItem[], id: string): ContextMenuItem {
  const hit = items.find((i) => i.id === id)
  if (!hit) throw new Error(`菜单里没有 ${id}，实际有：${items.map((i) => i.id).join(', ')}`)
  return hit
}

describe('右键菜单规则', () => {
  it('⭐ 子菜单只有一层（ContextMenu 的硬约束）', () => {
    /*
     * `ContextMenu` 只实现了一层子菜单：第二层不会渲染，用户会看到"点了没反应"。
     * 这条断言覆盖两个菜单的全部项，以后谁往下再套一层，这里立刻红。
     */
    const all = [
      ...buildCardMenu(cardInput()),
      ...buildCardMenu(cardInput({ cardCtxItems: [row({ id: null, kind: 'file' })] })),
      ...buildBlankMenu(blankInput()),
    ]
    let checked = 0
    for (const top of all) {
      for (const child of top.children ?? []) {
        checked += 1
        expect(child.children, `「${top.label}」的二级项「${child.label}」不该再有子菜单`).toBeUndefined()
      }
    }
    /*
     * 防"空循环也算过"：这几套菜单的二级项加起来（评分 6 + 标签 9 + 旋转 5 +
     * 转格式 3 + 摆正 2）总数必然超过 20。真有人把子菜单删光，这里也会红。
     */
    expect(checked).toBeGreaterThan(20)
  })

  it('全是目录 → 容器菜单（加进图库三选一），不出现图片那套', () => {
    const items = buildCardMenu(
      cardInput({ cardCtxItems: [row({ id: null, kind: 'dir', path: 'C:/p/sub' })] }),
    )
    const ids = items.map((i) => i.id)
    expect(ids).toContain('dir-add-name')
    expect(ids).not.toContain('rating')
    expect(ids).not.toContain('export')
  })

  it('目录 + 图片混合 → 走文件菜单（容器菜单只认「全是容器」）', () => {
    const items = buildCardMenu(
      cardInput({
        cardCtxItems: [row({ id: null, kind: 'dir', path: 'C:/p/sub' }), row()],
      }),
    )
    const ids = items.map((i) => i.id)
    expect(ids).toContain('rating')
    expect(ids).toContain('export')
    expect(ids).not.toContain('dir-add-name')
  })

  it('选中的全是压缩包 → 容器菜单，但入库三项全部禁用', () => {
    /*
     * 包内条目没有稳定的磁盘路径，索引层存不进去；硬加就是把缓存临时路径
     * 写进库，包一换全部失效。
     */
    const items = buildCardMenu(
      cardInput({
        cardCtxItems: [row({ id: null, kind: 'archive', path: 'C:/p/a.zip' })],
      }),
    )
    for (const id of ['dir-add-name', 'dir-add-new', 'dir-add-existing']) {
      expect(byId(items, id).disabled, `${id} 对压缩包必须禁用`).toBe(true)
    }
    // 但「打开压缩包」必须可用 —— 否则用户右键压缩包什么都做不了
    expect(byId(items, 'dir-browse').disabled).toBe(false)
  })

  it('浏览模式（未入库，id 为 null）→ 评分 / 旗标 / 标签全部禁用', () => {
    const items = buildCardMenu(
      cardInput({ cardCtxItems: [row({ id: null, transient: true })] }),
    )
    for (const id of ['rating', 'flag', 'tag']) {
      expect(byId(items, id).disabled, `${id} 需要索引 id`).toBe(true)
    }
    // 但纯文件操作（复制路径 / 导出）照样能用
    expect(byId(items, 'copy-path').disabled).toBe(false)
    expect(byId(items, 'export').disabled).toBe(false)
  })

  it('批量写文件进行中（exporting）→ 所有写盘项禁用，只读项不禁用', () => {
    const items = buildCardMenu(cardInput({ exporting: true }))
    for (const id of ['export', 'saveas', 'copy-to', 'move-to', 'transform', 'convert', 'orient', 'print', 'rename', 'delete']) {
      expect(byId(items, id).disabled, `${id} 会写文件，exporting 时必须禁用`).toBe(true)
    }
    expect(byId(items, 'copy-path').disabled).toBe(false)
    expect(byId(items, 'open').disabled).toBe(false)
  })

  it('多选时标题带张数', () => {
    const items = buildCardMenu(cardInput({ cardCtxItems: [row(), row({ key: 'k2', id: 2 })] }))
    expect(byId(items, 'tag').label).toBe('标签（2 张）')
    expect(byId(items, 'export').label).toBe('导出所选 2 张…')
  })
})

describe('空白处右键规则', () => {
  it('选中的文件夹优先于「当前浏览的目录」', () => {
    /*
     * 用户 2026-10-30：在目录列表里选中某个文件夹再右键，操作对象应该是
     * **那个选中的文件夹**，而不是当前浏览的这个目录。
     */
    const seen: string[] = []
    const items = buildBlankMenu(
      blankInput({
        curDir: { path: 'C:/cur', name: 'cur' },
        selectedItems: [row({ id: null, kind: 'dir', path: 'C:/picked', name: 'picked' })],
        addDirToLibrary: (p: string) => void seen.push(p),
      }),
    )
    // 提示里写的是会被建库的那个名字
    expect(byId(items, 'blank-new-lib').hint).toBe('picked')
    byId(items, 'blank-new-lib').onSelect?.()
    expect(seen).toEqual(['C:/picked'])
  })

  it('没选中文件夹时退回当前目录', () => {
    const seen: string[] = []
    const items = buildBlankMenu(
      blankInput({
        curDir: { path: 'C:/cur', name: 'cur' },
        selectedItems: [],
        addDirToLibrary: (p: string) => void seen.push(p),
      }),
    )
    expect(byId(items, 'blank-new-lib').hint).toBe('cur')
    byId(items, 'blank-new-lib').onSelect?.()
    expect(seen).toEqual(['C:/cur'])
  })

  it('「新建图库」只能对一个目录，多选时置灰', () => {
    const items = buildBlankMenu(
      blankInput({
        selectedItems: [
          row({ id: null, kind: 'dir', path: 'C:/a', name: 'a' }),
          row({ id: null, kind: 'dir', path: 'C:/b', name: 'b' }),
        ],
      }),
    )
    expect(byId(items, 'blank-new-lib').disabled).toBe(true)
    // 但「加入图库」可以一次挂多个
    expect(byId(items, 'blank-add-lib').label).toBe('加入图库（2 个）')
  })

  it('一条图库都没有时，子菜单给一条禁用说明而不是空白', () => {
    const items = buildBlankMenu(blankInput({ mgr: fakeMgr({ libraries: [] }) }))
    const child = byId(items, 'blank-add-lib').children
    expect(child).toHaveLength(1)
    expect(child?.[0]?.disabled).toBe(true)
    expect(child?.[0]?.label).toContain('还没有图库')
  })

  it('没有可放映的图时「幻灯片放映」置灰', () => {
    expect(byId(buildBlankMenu(blankInput({ playCount: 0 })), 'play-all').disabled).toBe(true)
    expect(byId(buildBlankMenu(blankInput({ playCount: 5 })), 'play-all').disabled).toBe(false)
  })
})
