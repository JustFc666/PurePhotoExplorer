/**
 * 图库的**两个右键菜单**：卡片菜单（对某几张文件）与空白菜单（对当前范围）。
 *
 * 为什么单独成文件：`ManagerView.tsx` 曾把这两块写成 400 行 `useMemo`，夹在
 * 组件状态与 JSX 中间 —— 想加一条菜单项得先滚过上千行，还得确认"这块能读到哪些
 * 闭包变量"。搬成纯函数之后，输入就是签名里那一个对象（能碰什么一目了然），
 * 而且**第一次可以被单测**（见 `cardMenu.test.ts`）。
 *
 * ## 三条不能破的约束
 * 1. ⛔ **子菜单只支持一层**（`ContextMenu` 的既定实现）。所以「旋转 / 翻转」
 *    「转换格式」「按 EXIF 摆正」「加进已有图库」全部是"父项 + 一层叶子"，
 *    不要再往下套第二层 —— 会静默点不到。
 * 2. ⛔ **写文件的项在 `exporting` 期间一律 disabled**（导出/打印正在跑时，
 *    再点一次另存会两批同时写盘）。读操作（复制路径、定位、放映）不禁用。
 * 3. ⛔ **批量操作用「右键作用对象」`cardCtxItems`，不是"当前选中"**：
 *    浏览模式（未入库）的每一行 `id` 都是 `null`，`mgr.selectedKeys` 那套在
 *    浏览模式恒为空；而且右键后用户可能顺手改选区，菜单里的「删除」不该跟着变。
 *
 * ## 目录 / 压缩包是另一套菜单
 * `cardCtxItems` 全是 `dir` 或 `archive` 时走「容器菜单」（加进图库三选一 + 浏览），
 * 其余走「文件菜单」。压缩包的入库三项**全部禁用**：包内条目没有稳定磁盘路径，
 * 硬加进去就是把缓存临时路径写进索引，包一换全失效。
 */

import type { GridItem, ManagerApi } from '../../hooks/useManager'
import { errText } from '../../hooks/useManager'
import { revealInExplorer, type TagItem } from '../../ipc/manager'
import type { ContextMenuItem } from '../chrome/ContextMenu'
import { filePathsOf } from './viewConfig'

/** 底部结果提示（导出/转格式/改名共用一条通道，由 `useManagerBatch` 持有）。 */
type Notice = { tone: 'ok' | 'warn' | 'err'; text: string; detail?: string }

/**
 * 卡片右键菜单的输入。
 *
 * 字段偏多，但这正是这族动作真实的依赖面 —— 原来它们都是组件里的闭包自由变量
 * （隐式），提取后变成显式的：改菜单时一眼能看到它能碰什么，不会误用到组件里
 * 别的状态。分组按「数据 / 索引侧动作 / 文件级动作 / 浮层」来。
 */
export type CardMenuInput = {
  /** 右键**那一刻**的作用对象（菜单开着时选区再变也不影响已弹出的项）。 */
  cardCtxItems: GridItem[]
  /** 图库数据 + 索引侧动作（评分 / 旗标 / 标签 / 图库列表 / 按人筛选 / 切目录）。 */
  mgr: ManagerApi
  /** 批量写文件正在进行：会写盘的项统一置灰。 */
  exporting: boolean

  /* ── 目录入库（只有「容器菜单」用）── */
  addDirToLibrary: (
    path: string,
    mode: 'name' | 'new' | 'existing',
    lib?: { id: number; name: string },
  ) => void | Promise<void>
  gotoWithReveal: (path: string, name: string) => void

  /* ── 索引侧动作 ── */
  /** 整批切换某个标签的挂载（全有 → 去掉，否则 → 全加上）。 */
  toggleTag: (ids: number[], tag: TagItem) => void | Promise<void>
  /** 把这张图里该人物的脸设为头像（按人筛选时才出现）。 */
  setPersonCover: (personId: number, imageId: number) => void | Promise<void>
  /** 打开标签浮层（新建 / 全部标签）。 */
  openTagPicker: (ids: number[]) => void
  /** 打开批量重命名对话框（模板 `{name}` / `{n}`）。 */
  setRenameFor: (items: GridItem[]) => void
  /** 单张重命名的原地编辑（网格视图下直接在缩略图上改名）。 */
  setInlineRename: (item: GridItem | null) => void
  setExportNotice: (n: Notice | null) => void

  /* ── 文件级批量动作（实现见 `hooks/manager/batchActions.ts`）── */
  copyPaths: (paths: string[]) => void | Promise<void>
  doExport: (paths: string[]) => void | Promise<void>
  doSaveAs: (paths: string[]) => void | Promise<void>
  doCopyTo: (paths: string[]) => void | Promise<void>
  doMoveTo: (items: GridItem[]) => void | Promise<void>
  doTransform: (
    paths: string[],
    opts: { rotate?: number; flipH?: boolean; flipV?: boolean },
    label: string,
  ) => void | Promise<void>
  doConvert: (format: 'jpg' | 'png' | 'bmp', paths: string[]) => void | Promise<void>
  doAutoOrient: (overwrite: boolean, paths: string[]) => void | Promise<void>
  doPrint: (paths: string[]) => void | Promise<void>
  doDelete: () => void | Promise<void>

  /* ── 幻灯片 ── */
  /** 当前范围里能放映的张数（0 → 置灰）。 */
  playCount: number
  playSlideshow: (from?: string) => void
}

/** 卡片右键菜单的内容。 */
export function buildCardMenu(input: CardMenuInput): ContextMenuItem[] {
  const {
    cardCtxItems,
    mgr,
    exporting,
    addDirToLibrary,
    gotoWithReveal,
    toggleTag,
    setPersonCover,
    openTagPicker,
    setRenameFor,
    setInlineRename,
    setExportNotice,
    copyPaths,
    doExport,
    doSaveAs,
    doCopyTo,
    doMoveTo,
    doTransform,
    doConvert,
    doAutoOrient,
    doPrint,
    doDelete,
    playCount,
    playSlideshow,
  } = input

  /*
   * 目录卡片：右键给「加进图库」三选一 + 浏览。
   *
   * ⭐ 2026-10-30 起目录也能**多选**（`toggle` 允许 dir 参与选择）——
   * 选了几个目录再右键，三条加库动作就对整批生效：
   * 「用文件夹名」每个目录各建一个库；「加入图库」把它们都挂到同一个库。
   * 「新建图库」只能对一个目录（多个目录该叫什么名？无从判断），多选时置灰。
   */
  /*
   * 「容器」菜单 = 全是文件夹**或**压缩包。
   *
   * 压缩包共用这套菜单（它和文件夹一样是「能进去看的东西」），但
   * **入库三项全部禁用**：包内条目没有稳定的磁盘路径，索引层存不进去；
   * 硬要加就是把「缓存文件路径」写进库，包一换全部失效。
   */
  const allContainers =
    cardCtxItems.length > 0 &&
    cardCtxItems.every((i) => i.kind === 'dir' || i.kind === 'archive')
  const dirs = cardCtxItems.filter((i) => i.kind === 'dir' && i.path)
  if (allContainers) {
    const one = cardCtxItems.length === 1 ? cardCtxItems[0] : null
    /** 选中的全是压缩包 → 没有任何一个能加入图库 */
    const noDir = dirs.length === 0
    const openLabel = noDir ? '打开压缩包' : '浏览此目录'
    return [
      {
        id: 'dir-add-name',
        label:
          dirs.length > 1 ? `加进图库：各自用文件夹名（${dirs.length} 个）` : '加进图库：用文件夹名',
        hint: noDir ? '压缩包不能加入图库' : one?.name,
        disabled: noDir,
        onSelect: () => {
          for (const d of dirs) if (d.path) addDirToLibrary(d.path, 'name')
        },
      },
      {
        id: 'dir-add-new',
        label: '加进新图库…',
        hint: noDir ? '压缩包不能加入图库' : one ? '自定义名称' : '只能选一个目录',
        disabled: noDir || dirs.length !== 1,
        onSelect: () => {
          const d = dirs.length === 1 ? dirs[0] : null
          if (d?.path) addDirToLibrary(d.path, 'new')
        },
      },
      {
        id: 'dir-add-existing',
        label: dirs.length > 1 ? `加入图库（${dirs.length} 个目录）` : '加进已有图库',
        disabled: noDir,
        hint: noDir ? '压缩包不能加入图库' : undefined,
        // 子菜单只支持一层；一条图库都没有时给一条禁用说明项
        children:
          mgr.libraries.length > 0
            ? mgr.libraries.map((l) => ({
                id: `dir-lib-${l.id}`,
                // 不显示张数（用户 2026-10-30：二级菜单里只要图库名）
                label: l.name,
                onSelect: () => {
                  for (const d of dirs) {
                    if (d.path) addDirToLibrary(d.path, 'existing', { id: l.id, name: l.name })
                  }
                },
              }))
            : [{ id: 'dir-lib-none', label: '（还没有其他图库）', disabled: true }],
      },
      {
        id: 'dir-browse',
        label: openLabel,
        hint: one ? (noDir ? '查看包内图片' : '不入库') : '只能选一个',
        separatorBefore: true,
        disabled: !one,
        onSelect: () => {
          if (!one?.path) return
          // 压缩包不走 `gotoWithReveal`：侧栏树里没有「包」这个节点，
          // `revealTo` 必然空转（见 `openItem` 的同款注释）
          if (one.kind === 'archive') void mgr.gotoDir(one.path, one.name)
          else gotoWithReveal(one.path, one.name)
        },
      },
    ]
  }

  const withPath = cardCtxItems.filter((i) => !!i.path)
  /**
   * ⚠️ 右键菜单的批量操作必须用**右键作用对象**的路径，不能用「选中集合」：
   * 浏览模式（未入库）根本没有选中集合可用（`id` 全是 `null`）。
   */
  const paths = filePathsOf(cardCtxItems)
  const n = cardCtxItems.length
  /** 能打星的行（浏览模式的临时项没有索引 id） */
  const rateable = cardCtxItems.map((i) => i.id).filter((id): id is number => id != null)
  /** 多选时星数为「各不相同」就显示「混合」，避免谎报成第一张的值 */
  const ratings = new Set(cardCtxItems.map((i) => i.rating))
  const oneRating = ratings.size === 1 ? ([...ratings][0] ?? 0) : -1
  const ratingHint =
    oneRating < 0 ? '混合' : oneRating === 0 ? undefined : '★'.repeat(oneRating)
  const allFlagged = cardCtxItems.length > 0 && cardCtxItems.every((i) => i.flag > 0)

  /** 按人浏览时才出现的专属项：把这张图里的脸设成该人物的头像 */
  const pf = mgr.personFilter
  const coverTarget = pf && n === 1 && rateable.length === 1 ? rateable[0] : null

  return [
    ...(coverTarget != null
      ? [
          {
            id: 'person-cover',
            label: '设为头像',
            hint: pf?.name ?? undefined,
            onSelect: () => void setPersonCover(pf!.id, coverTarget),
          },
        ]
      : []),
    {
      id: 'rating',
      label: n > 1 ? `评分（${n} 张）` : '评分',
      hint: ratingHint,
      disabled: rateable.length === 0,
      // 子菜单只支持一层（ContextMenu 的既定约束）
      children: [0, 1, 2, 3, 4, 5].map((v) => ({
        id: `rating-${v}`,
        label: v === 0 ? '清除星级' : '★'.repeat(v),
        hint: oneRating === v ? '当前' : undefined,
        onSelect: () => void mgr.setRating(rateable, v),
      })),
    },
    {
      id: 'flag',
      label: allFlagged ? '取消标记' : '标记为待处理',
      hint: allFlagged ? '●' : undefined,
      disabled: rateable.length === 0,
      onSelect: () => void mgr.setFlag(rateable, allFlagged ? 0 : 1),
    },
    {
      id: 'tag',
      label: n > 1 ? `标签（${n} 张）` : '标签',
      hint: 'L',
      disabled: rateable.length === 0,
      // 子菜单只支持一层：常用标签直接平铺，最后一项进浮层（新建 / 全部标签）
      children: [
        ...mgr.tags.slice(0, 8).map((t) => ({
          id: `tag-${t.id}`,
          label: t.name,
          hint: `${t.count} 张`,
          onSelect: () => void toggleTag(rateable, t),
        })),
        {
          id: 'tag-more',
          label: mgr.tags.length > 0 ? '更多 / 新建标签…' : '新建标签…',
          separatorBefore: mgr.tags.length > 0,
          onSelect: () => openTagPicker(rateable),
        },
      ],
    },
    {
      id: 'open',
      label: '在资源管理器中显示',
      separatorBefore: true,
      disabled: withPath.length === 0,
      onSelect: () => {
        const p = withPath[0]?.path
        if (p) {
          void revealInExplorer(p).catch((e: unknown) =>
            setExportNotice({
              tone: 'err',
              text: '定位失败',
              detail: errText(e),
            }),
          )
        }
      },
    },
    {
      id: 'copy-path',
      label: n > 1 ? `复制 ${n} 个文件路径` : '复制文件路径',
      disabled: withPath.length === 0,
      onSelect: () => void copyPaths(withPath.map((i) => i.path as string)),
    },
    {
      id: 'play',
      label: '幻灯片放映',
      hint: '从这张开始',
      separatorBefore: true,
      disabled: playCount === 0,
      onSelect: () => playSlideshow(cardCtxItems[0]?.path ?? undefined),
    },
    {
      id: 'export',
      label: n > 1 ? `导出所选 ${n} 张…` : '导出到文件夹…',
      separatorBefore: true,
      disabled: exporting || withPath.length === 0,
      hint: exporting ? '处理中…' : undefined,
      onSelect: () => void doExport(paths),
    },
    {
      id: 'saveas',
      label: n > 1 ? `另存为…（${n} 张）` : '另存为…',
      disabled: exporting || withPath.length === 0,
      onSelect: () => void doSaveAs(paths),
    },
    {
      id: 'copy-to',
      label: '复制到文件夹…',
      disabled: exporting || withPath.length === 0,
      onSelect: () => void doCopyTo(paths),
    },
    {
      id: 'move-to',
      label: '移动到文件夹…',
      // ⚠️ 移动会真的把文件从原位置搬走，破坏性比复制大 —— 菜单里排在复制后面
      disabled: exporting || withPath.length === 0,
      onSelect: () => void doMoveTo(cardCtxItems),
    },
    {
      id: 'transform',
      label: '旋转 / 翻转并保存',
      disabled: exporting || withPath.length === 0,
      // 第二层只能是叶子项（ContextMenu 只支持一层子菜单）
      children: [
        {
          id: 'tf-cw',
          label: '顺时针 90°',
          onSelect: () => void doTransform(paths, { rotate: 1 }, '顺时针旋转 90°'),
        },
        {
          id: 'tf-ccw',
          label: '逆时针 90°',
          onSelect: () => void doTransform(paths, { rotate: 3 }, '逆时针旋转 90°'),
        },
        {
          id: 'tf-180',
          label: '旋转 180°',
          onSelect: () => void doTransform(paths, { rotate: 2 }, '旋转 180°'),
        },
        {
          id: 'tf-fh',
          label: '水平翻转（左右）',
          onSelect: () => void doTransform(paths, { flipH: true }, '水平翻转'),
        },
        {
          id: 'tf-fv',
          label: '垂直翻转（上下）',
          onSelect: () => void doTransform(paths, { flipV: true }, '垂直翻转'),
        },
      ],
    },
    {
      id: 'convert',
      label: '转换格式',
      disabled: exporting || withPath.length === 0,
      // ⚠️ 与「导出」的区别要在文案里点明：导出是原样复制，这里是重新编码
      hint: '重新编码',
      children: [
        { id: 'conv-jpg', label: '转为 JPEG（有损）', onSelect: () => void doConvert('jpg', paths) },
        { id: 'conv-png', label: '转为 PNG（无损）', onSelect: () => void doConvert('png', paths) },
        { id: 'conv-bmp', label: '转为 BMP（无压缩）', onSelect: () => void doConvert('bmp', paths) },
      ],
    },
    {
      id: 'orient',
      label: '按 EXIF 方向摆正',
      disabled: exporting || withPath.length === 0,
      children: [
        {
          id: 'orient-copy',
          label: '另存到文件夹…',
          onSelect: () => void doAutoOrient(false, paths),
        },
        {
          id: 'orient-write',
          label: '写回原文件…（不可撤销）',
          onSelect: () => void doAutoOrient(true, paths),
        },
      ],
    },
    {
      id: 'print',
      label: n > 1 ? `打印 ${n} 张…` : '打印…',
      disabled: exporting || withPath.length === 0,
      onSelect: () => void doPrint(paths),
    },
    {
      id: 'rename',
      label: n > 1 ? `批量重命名 ${n} 张…` : '重命名…',
      disabled: exporting || withPath.length === 0,
      onSelect: () => {
        // 单张 + 网格视图 → 直接在缩略图文件名上原地编辑（2026-10-24）；
        // 多张 / 列表视图仍走模板对话框
        if (n === 1 && mgr.viewMode === 'grid') setInlineRename(cardCtxItems[0] ?? null)
        else setRenameFor(cardCtxItems)
      },
    },
    {
      id: 'delete',
      label: n > 1 ? `删除 ${n} 张到回收站…` : '删除到回收站…',
      separatorBefore: true,
      disabled: exporting || withPath.length === 0,
      onSelect: () => void doDelete(),
    },
  ]
}


/** 空白菜单的输入。 */
export type BlankMenuInput = {
  mgr: ManagerApi
  /** 当前正在看的目录（浏览 / 已入库目录），`null` = 在「我的电脑」或图库首页。 */
  curDir: { path: string; name: string } | null
  /** 选中行（用来挑「要入库的文件夹」）。 */
  selectedItems: GridItem[]
  addDirToLibrary: CardMenuInput['addDirToLibrary']
  /** 选中数量（决定「取消选择」是否置灰）。 */
  selectedCount: number
  playCount: number
  playSlideshow: (from?: string) => void
}

// ── 空白处右键（刷新 / 全选 / 取消选择）──
/**
 * 网格或列表的**空白区域**右键。
 *
 * 与卡片右键是两个独立菜单：卡片菜单是对「某几张文件」的操作，
 * 空白菜单是对「当前这个文件夹 / 图库」的操作。
 *
 * ⭐ 核心是「刷新」：外部往目录里放了新图（或删了文件）之后，
 * 就地重扫当前范围并留在原地 —— **不切换目录、不弹回「我的电脑」**。
 * 走的是同一个 `mgr.rescan()`（`F5` 也是它）：浏览模式重列目录、
 * 入库模式重扫索引，两条路都不动 `scope`。
 */
export function buildBlankMenu(input: BlankMenuInput): ContextMenuItem[] {
  const { mgr, curDir, selectedItems, addDirToLibrary, selectedCount, playCount, playSlideshow } =
    input
  /**
   * 「新建图库 / 加进图库」的作用对象。
   *
   * ⭐ 用户 2026-10-30：在目录列表里**选中某个文件夹**再右键（菜单弹在空白处或
   * 卡片上），操作对象应该是**那个选中的文件夹**，而不是"当前浏览的这个目录"。
   * 所以优先级：选中的文件夹 > 当前浏览目录。
   */
  const libTargets: { path: string; name: string }[] = (() => {
    const picked = selectedItems
      .filter((i) => i.kind === 'dir' && i.path)
      .map((i) => ({ path: i.path as string, name: i.name }))
    if (picked.length > 0) return picked
    return curDir ? [{ path: curDir.path, name: curDir.name }] : []
  })()

  return [
    {
      id: 'refresh',
      label: '刷新',
      hint: 'F5',
      disabled: mgr.isIdle || mgr.busy,
      onSelect: () => void mgr.rescan(),
    },
    /*
     * 「新建图库 / 加入图库」= 把**当前这个目录**入库（用户 2026-10-30）。
     * 和「文件」菜单里那两项完全同一套动作（都走 `addDirToLibrary` /
     * `setNewLibFor`），不是另写一份 —— 免得两处行为漂移。
     */
    {
      id: 'blank-new-lib',
      label: '新建图库',
      // 提示写**真正会被建库的那个名字**（选中的文件夹名），不是当前目录名
      hint: libTargets.length === 1 ? libTargets[0]?.name : undefined,
      separatorBefore: true,
      // 多选时没法决定叫什么名字（和目录卡片的规矩一致）
      disabled: libTargets.length !== 1,
      onSelect: () => {
        const t = libTargets[0]
        // 走统一入口：它会先问「是否包含子文件夹」（直接 setNewLibFor 会绕过那一问）
        if (t) void addDirToLibrary(t.path, 'new')
      },
    },
    {
      id: 'blank-add-lib',
      label: libTargets.length > 1 ? `加入图库（${libTargets.length} 个）` : '加入图库',
      disabled: libTargets.length === 0,
      // 只列图库名，不带张数（和文件菜单的子菜单保持一致）
      children:
        mgr.libraries.length > 0
          ? mgr.libraries.map((l) => ({
              id: `blank-lib-${l.id}`,
              label: l.name,
              onSelect: () => {
                for (const t of libTargets) {
                  addDirToLibrary(t.path, 'existing', { id: l.id, name: l.name })
                }
              },
            }))
          : [{ id: 'blank-lib-none', label: '（还没有图库）', disabled: true }],
    },
    {
      id: 'sel-all',
      label: '全选',
      hint: 'Ctrl+A',
      separatorBefore: true,
      disabled: mgr.isIdle || mgr.total === 0,
      onSelect: mgr.selectAll,
    },
    {
      id: 'sel-none',
      label: '取消选择',
      disabled: selectedCount === 0,
      onSelect: mgr.clearSelection,
    },
    {
      id: 'play-all',
      label: '幻灯片放映',
      hint: '播放本范围',
      separatorBefore: true,
      disabled: playCount === 0,
      onSelect: () => playSlideshow(),
    },
]
}
