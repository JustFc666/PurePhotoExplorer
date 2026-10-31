/**
 * 护栏：右键菜单不得再引起侧栏布局跳变。
 *
 * ## 曾经的 bug（2026-10-18 用户报告「右键图库子文件夹会跳一下」）
 * `ContextMenu` 曾作为侧栏 `<aside>` 的子元素渲染。菜单一挂载，它的 wrapper
 * 就成了 `<aside>` 的 `:last-child`，把图库分区的
 * `.pv-mg-sec:last-child { flex: 1 1 42% }` **顶掉** → 侧栏两个分区的
 * flex 比例瞬间改变 → 整个侧栏重排，用户看到「跳一下」。
 *
 * 修复有两道，缺一不可：
 * 1. `ContextMenu` 用 `createPortal(..., document.body)`，不参与调用方布局；
 * 2. 侧栏分区改用显式 class（`pv-mg-sec-tree` / `pv-mg-sec-lib`）定 flex，
 *    不再依赖 `:first-child` / `:last-child`。
 *
 * ⚠️ 若日后再往 `<aside>` 里加浮层子元素，或把显式 class 改回结构选择器，
 * 这个 bug 会复活 —— 所以用源码断言把它钉死。
 */

import ctxMenuSrc from '../chrome/ContextMenu.tsx?raw'
import sidebarSrc from './ManagerSidebar.tsx?raw'
import managerViewSrc from './ManagerView.tsx?raw'
import libDropSrc from '../../hooks/libDrop.ts?raw'
import useManagerSrc from '../../hooks/useManager.ts?raw'
// 排序偏好的读写 2026-11 拆到 `manager/prefs.ts`，护栏跟着搬
import managerPrefsSrc from '../../hooks/manager/prefs.ts?raw'
import appSrc from '../../App.tsx?raw'
import cssSrc from '../../styles/manager.css?raw'
import { describe, expect, it } from 'vitest'

describe('右键菜单不引起侧栏布局跳变', () => {
  it('ContextMenu 用 Portal 挂到 document.body', () => {
    expect(ctxMenuSrc).toContain("import { createPortal } from 'react-dom'")
    expect(ctxMenuSrc).toMatch(/return\s+createPortal\(/)
    expect(ctxMenuSrc).toContain('document.body')
  })

  it('ManagerSidebar 两个分区用显式 class', () => {
    expect(sidebarSrc).toContain('pv-mg-sec pv-mg-sec-tree')
    expect(sidebarSrc).toContain('pv-mg-sec pv-mg-sec-lib')
  })

  it('分区高度用显式 class，不依赖 :first-child / :last-child', () => {
    // 核心不变量：不能再出现「结构选择器定 flex」的写法
    // （右键菜单一旦挂进侧栏，wrapper 就会变成 `:last-child`，布局会瞬移）
    expect(cssSrc).not.toMatch(/\.pv-mg-sec:(first|last)-child/)

    /*
     * 两个分区都必须有自己的规则块。
     *
     * ⚠️ 2026-10-28 起**不再断言具体百分比**：上下比例改成用户可拖拽
     * （`pv.set.sideTreePct`，tree 区由内联 `flex: 0 0 X%` 控制），
     * lib 区改成 `flex: 1 1 auto` 吃剩余。绑死 58%/42% 会挡住这个需求，
     * 而真正要防的回归是「有人图省事改回 :first-child」—— 上一行已经守住了。
     */
    expect(cssSrc).toMatch(/\.pv-mg-sec-tree\s*\{/)
    expect(cssSrc).toMatch(/\.pv-mg-sec-lib\s*\{[^}]*flex:\s*1 1 auto/)
    // 分隔条必须存在，否则用户没法拖
    expect(cssSrc).toMatch(/\.pv-mg-hsplit\s*\{/)
  })

  it('菜单定位靠坐标 key 重挂载，不靠 position 同步 effect', () => {
    // 调用方必须用坐标当 key（换位置 = 新实例 = pos 从 useState 拿到新坐标）
    expect(sidebarSrc).toContain('key={`${ctx.state.x},${ctx.state.y}`}')
    /**
     * ⛔ 禁止 `useEffect(..., [position.x, position.y])`：
     * 它**首次挂载时也会跑**，会把 `useLayoutEffect` 设好的 `ready` 重置为 false，
     * 而 pos 又是同值、layout effect 不再跑 → 菜单永远 hidden，变成「一闪就没」。
     */
    // 依赖数组里出现 position 的两个分量 = 有同步 effect（注释不算，匹配收尾写法）
    expect(ctxMenuSrc).not.toMatch(/\},\s*\[position\.x,\s*position\.y\]\)/)
  })

  it('子菜单项不绑定 hover（否则鼠标一进去子菜单就消失）', () => {
    /**
     * 曾经的 bug（2026-10-18）：子菜单项也传了 `onHover={hover}`，
     * 而 `hover` 对「没有 children 的项」会 `setSub(null)` ——
     * 鼠标刚进子菜单就被判成「离开父项」，菜单立刻关掉。
     *
     * `onHover` 必须保持**可选**：子菜单内部不传，才不会再挂这个监听。
     */
    expect(ctxMenuSrc).toContain('onHover?:')

    // 子菜单的 map 只允许 item + onRun，不能出现 onHover
    const subMap = ctxMenuSrc.slice(ctxMenuSrc.indexOf('aria-label="子菜单"'))
    const mapLine = subMap.slice(subMap.indexOf('sub.items.map'))
    expect(mapLine.slice(0, 200)).not.toContain('onHover')
  })
})

/**
 * 护栏：「图库」标题右边的 `+` —— 新建**空**图库（2026-10-18）。
 *
 * 三条不能丢：
 * 1. 按钮在侧栏里、且把开口交给上层（弹窗不能塞进 `<aside>`，理由见上面那个 bug）；
 * 2. 默认名是「新的图库」；
 * 3. **失败不关弹窗**（重名时关掉再点一次 `+` 很烦）——
 *    所以 `setCreateLib(null)` 只能出现在成功分支。
 */
describe('侧栏「图库 +」新建空图库', () => {
  it('侧栏有 + 按钮，且只负责开口', () => {
    expect(sidebarSrc).toContain('className="pv-mg-secadd"')
    expect(sidebarSrc).toContain('onClick={onCreateLibrary}')
    expect(sidebarSrc).toContain('onCreateLibrary: () => void')
    // ⛔ 弹窗不许渲染在侧栏里（会成为 <aside> 的子元素）
    // 只禁 JSX 用法，注释里提到名字不算
    expect(sidebarSrc).not.toMatch(/<PromptDialog/)
  })

  it('默认名「新的图库」，创建成功才关弹窗', () => {
    expect(managerViewSrc).toContain("const DEFAULT_LIB_NAME = '新的图库'")
    expect(managerViewSrc).toContain('setCreateLib({ name: DEFAULT_LIB_NAME })')
    expect(managerViewSrc).toContain('await managerCreateLibrary(name)')
    // 失败分支保留弹窗并带错误
    expect(managerViewSrc).toContain('setCreateLib({ name, error: errText(e) })')
    const fn = managerViewSrc.slice(
      managerViewSrc.indexOf('const doCreateLibrary'),
      managerViewSrc.indexOf('const doCreateLibrary') + 400,
    )
    // 成功：先关弹窗再刷新列表
    expect(fn).toMatch(/await managerCreateLibrary\(name\)[\s\S]*setCreateLib\(null\)/)
    expect(fn).toContain('mgr.refreshLibraries()')
  })

  it('+ 按钮有样式（不是裸 button）', () => {
    expect(cssSrc).toMatch(/\.pv-mg-secadd\s*\{/)
    expect(cssSrc).toMatch(/\.pv-mg-secadd:hover/)
  })
})

/**
 * 护栏：图库行的右键菜单 + 拖文件夹入库（2026-10-18）。
 *
 * 三条不能丢：
 * 1. 图库行要带 `data-pv-lib` —— 外部拖放（原生通道）全靠它做命中测试；
 * 2. 内部拖拽（文件树 → 图库行）走 HTML5，外部走 `onDragDropEvent`，
 *    两边都得在（原生拖放下浏览器**收不到** HTML5 的 drop）；
 * 3. `App.tsx` 的全局「拖文件进来 = 打开看图」必须让位，
 *    否则会拿一个目录去当图片解码。
 */
describe('图库行右键菜单与拖放入库', () => {
  it('图库行是放置目标，且带 id 供命中测试', () => {
    expect(sidebarSrc).toContain('data-pv-lib={lib.id}')
    expect(sidebarSrc).toContain('onDropPath(path, lib)')
    expect(cssSrc).toMatch(/\.pv-mg-row\.is-drop\s*\{/)
  })

  it('文件树目录行可拖出路径', () => {
    expect(sidebarSrc).toContain("draggable={!isComputer}")
    expect(sidebarSrc).toContain("setData('text/plain', node.path)")
  })

  it('外部拖放用原生通道 + 物理像素换算', () => {
    // 原生事件只给物理像素，必须除以 devicePixelRatio 才是 CSS 像素
    expect(libDropSrc).toContain('onDragDropEvent')
    expect(libDropSrc).toContain('window.devicePixelRatio')
    expect(libDropSrc).toContain("closest<HTMLElement>('[data-pv-lib]')")
    // ⛔ 清标记必须延后：App 的处理器还要靠它判断「这次拖放已被图库接走」
    expect(libDropSrc).toContain('setTimeout(() => apply(null), 0)')
  })

  it('App 的全局拖放给图库行让位', () => {
    expect(appSrc).toContain('getLibDropHover() !== null')
  })

  it('菜单三项：添加文件夹 / 重命名 / 删除图库', () => {
    expect(sidebarSrc).toContain("id: 'lib-add'")
    expect(sidebarSrc).toContain("id: 'lib-rename'")
    expect(sidebarSrc).toContain("id: 'lib-delete'")
    // 删除图库走 `mgr.deleteLibrary`（只删分组），不是 removeAddedFolder
    expect(managerViewSrc).toContain('mgr.deleteLibrary(id)')
    expect(managerViewSrc).toContain('mgr.renameLibrary(id, name)')
    expect(managerViewSrc).toContain('mgr.addFolderToLibraryById(picked, name, id, libName)')
  })
})

/**
 * 护栏：智能图库（2026-10-19）。
 *
 * - 规则文本走 `pv-lib` 的 DSL，**建库时就要校验**，不能建出一个永远空的库；
 * - 查询必须分路：智能库没有 `library_folders` 成员行，当普通库 JOIN 会恒空
 *   （后端 `query_in_library` 的分支有 Rust 测试兜着，这里钉前端调用链）。
 */
describe('智能图库', () => {
  it('默认按拍摄时间排（相册的直觉顺序）', () => {
    // 初值来自持久化偏好，未设置时回落到拍摄时间
    expect(useManagerSrc).toContain('useState<ManagerSort>(readSortPref)')
    expect(managerPrefsSrc).toContain("return 'taken'")
  })

  it('`+` 右键出智能图库预设', () => {
    expect(sidebarSrc).toContain("kind: 'newsmart'")
    expect(sidebarSrc).toContain("id: 'smart-year'")
    expect(sidebarSrc).toContain("id: 'smart-rating'")
    expect(sidebarSrc).toContain("id: 'smart-raw'")
    // 规则文本必须带上 DSL 前缀，裸词会被当成全文关键词
    expect(sidebarSrc).toContain('rating:>=4')
    expect(sidebarSrc).toContain('type:raw')
    expect(sidebarSrc).toContain('date:${year}')
  })

  it('建完刷新侧栏再切范围', () => {
    // 顺序不能反：先切范围时侧栏还没这条库，标题会空
    const fn = managerViewSrc + useManagerSrc
    expect(fn).toContain('mgr.createSmartLibrary(name, rule)')
    expect(useManagerSrc).toContain('await managerCreateSmartLibrary(name, rule)')
    expect(useManagerSrc).toMatch(
      /managerCreateSmartLibrary\(name, rule\)[\s\S]{0,200}refreshMeta\(\)[\s\S]{0,120}setScope\(/,
    )
  })
})
