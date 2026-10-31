/**
 * 护栏：图库网格的**多选规则**与**原地重命名**（2026-10-24 用户定稿）。
 *
 * ## 为什么用源码断言而不是渲染测试
 * 这两条规则都长在组件的**事件处理**里（点击语义、输入框提交时机），
 * 渲染一遍要 mock 整套 manager 状态。本仓库既有的护栏（`sidebarLayout.test.ts`）
 * 也是用 `?raw` 读源码做断言 —— 这里沿用同一套路，改坏了会红。
 *
 * ## 规则原文
 * 1. 普通单击 = **加选**（连续点缩略图即多选，不必按 Ctrl）；
 *    再点**已选中的那张** = 取消**本次全部**选择；点**网格空白处** = 全部取消；
 *    Ctrl 单击 = 只切换这一张；**Shift 单击 = 从锚点一路选过来**（2026-10-31 改）。
 * 2. 单张重命名 = **直接在卡片文件名上原地编辑**，文本**全选**（打字即覆盖），
 *    Enter 提交 / Esc 取消 / 失焦提交；扩展名由上层自动保留。
 */

import managerViewSrc from './ManagerView.tsx?raw'
import photoCardSrc from './PhotoCard.tsx?raw'
import aiTagsCssSrc from '../../styles/aiTags.css?raw'
import { describe, expect, it } from 'vitest'

describe('图库多选规则', () => {
  it('普通单击是加选，不是「只选这一张」', () => {
    // 点已选中的那张 → 取消本次全部；否则 → 加选
    expect(managerViewSrc).toMatch(
      /if\s*\(selected\.has\(item\.key\)\)\s*\{\s*mgr\.clearSelection\(\)[\s\S]{0,120}?else\s*\{\s*mgr\.toggleSelect\(item\.key\)/,
    )
    // 旧的「单击只选一张」写法不许回来
    expect(managerViewSrc).not.toMatch(/else\s+mgr\.selectOnly\(item\.key\)/)
  })

  it('Ctrl 单击只切换这一张', () => {
    expect(managerViewSrc).toMatch(
      /if\s*\(e\.ctrlKey \|\| e\.metaKey\)\s*\{\s*mgr\.toggleSelect\(item\.key\)/,
    )
  })

  it('Shift 单击是区间选择，不是「等价于 Ctrl」', () => {
    // ⚠️ 这条是 2026-10-31 加的：Shift 以前和 Ctrl 一个分支（只切换一张），
    //    用户要的是「大量选取」，所以必须走 `selectRange`。
    expect(managerViewSrc).toMatch(/if\s*\(e\.shiftKey\)\s*\{[\s\S]{0,160}?mgr\.selectRange\(/)
    // 且不能再退回「Shift 和 Ctrl 写在一起」
    expect(managerViewSrc).not.toMatch(/e\.ctrlKey \|\| e\.metaKey \|\| e\.shiftKey/)
  })

  it('点网格空白处取消全部选择', () => {
    expect(managerViewSrc).toContain('e.target === e.currentTarget && mgr.canSelect')
    expect(managerViewSrc).toContain('mgr.clearSelection()')
  })
})

describe('单张重命名原地编辑', () => {
  it('文件名变输入框，文本全选', () => {
    expect(photoCardSrc).toContain('className="pv-mg-namedit"')
    expect(photoCardSrc).toContain('onFocus={(e) => e.currentTarget.select()}')
    // 输入框的点击不许冒泡成卡片选中切换
    expect(photoCardSrc).toContain('onClick={(e) => e.stopPropagation()}')
  })

  it('Enter 提交 / Esc 取消 / 失焦提交，且只提交一次', () => {
    expect(photoCardSrc).toContain("if (e.key === 'Enter')")
    expect(photoCardSrc).toContain("else if (e.key === 'Escape')")
    expect(photoCardSrc).toContain('onBlur={(e) => {')
    expect(photoCardSrc).toContain('if (renameDone.current) return')
  })

  it('扩展名由上层保留（输入框里只有主体名）', () => {
    expect(photoCardSrc).toContain('defaultValue={stemOf(item.name)}')
    expect(managerViewSrc).toContain("const ext = dot > 0 ? item.name.slice(dot) : ''")
    expect(managerViewSrc).toContain('newName: body + ext')
  })

  it('多张仍走模板对话框，单张 + 网格才原地编辑', () => {
    expect(managerViewSrc).toContain(
      "if (n === 1 && mgr.viewMode === 'grid') setInlineRename(cardCtxItems[0] ?? null)",
    )
    expect(managerViewSrc).toContain('else setRenameFor(cardCtxItems)')
  })
})

describe('标签条行高不被选中块撑高', () => {
  it('行内按钮压到 22px（与 chip 同高），不用全局 .pv-btn 的 30px', () => {
    expect(aiTagsCssSrc).toMatch(
      /\.pv-ai-chiprow > \.pv-mg-headr \.pv-btn\s*\{[^}]*height:\s*22px/,
    )
    expect(aiTagsCssSrc).toMatch(/\.pv-ai-chiprow > \.pv-mg-headr\s*\{[^}]*height:\s*22px/)
  })
})
