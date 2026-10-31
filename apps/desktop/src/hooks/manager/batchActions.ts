/**
 * 图库的**文件级批量操作**（状态 + 动作 + 结果提示）。
 *
 * 从 `ManagerView.tsx` 整块搬出来的（2026-11）：那个文件一度 3000 行，
 * 而这一族动作是全文件里唯一「跟视图布局完全无关」的一块 —— 它们只依赖
 * `mgr`（图库数据 + 选中集）与右键作用对象 `cardCtxItems`。
 *
 * ## 为什么这些动作全在这里
 * 因为它们共用**一条提示通道**（`exportNotice`）和**一把互斥锁**（`exporting`）。
 * 分成几个 hook 反而要各自复制这两个状态，用户会看到两条提示抢位置。
 *
 * ## 三条不能改的语义
 * 1. **导出 ≠ 转格式**：导出是字节复制（无损、瞬间），转格式要重新编码
 *    （慢、JPEG 有损）。别合并成一个入口。
 * 2. **写回原文件必须先说清后果**：`doAutoOrient(overwrite=true)` 会弹二次确认，
 *    `doDelete` 会讲明「进回收站 / 清索引」两件事。旋转翻转是高频操作，不弹框。
 * 3. **改过文件之后必须刷新**：`refreshAfterMutation` 同时负责
 *    「浏览模式重新列目录 / 入库模式重查索引」+ 广播人物数据可能变了。
 *    ⛔ 别只 `mgr.reload()` —— 浏览模式的数据来自 `fs_list_images`，`reload()` 是空操作。
 *
 * ## 打印为什么带一个预览队列
 * `printBatch` 是「有哪几张、正在预览第几张」，`printSrc` 是当前样本的解码图
 * （走 `fs_thumbnail` 的 2048 档，够看清版面又不解原图）。版面参数对整批共用一套，
 * 换图只换样本不换设置 —— 否则「预览的是这张的设置，印的是另一套」。
 */

import { useCallback, useEffect, useRef, useState } from 'react'
import { confirm, open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog'

import type { DecodedImage } from '../../ipc'
import { fsThumbnail } from '../../ipc/browse'
import {
  managerAutoOrient,
  managerConvertImages,
  managerCopyToFolder,
  managerDeleteImages,
  managerExportImages,
  managerMoveToFolder,
  managerPrintImages,
  managerRenameImages,
  managerSaveAs,
  managerTransformImages,
  type BatchSummary,
} from '../../ipc/manager'
import { notifyPeopleChanged } from '../../lib/peopleEvents'
import type { GridItem, ManagerApi } from './types'
import { errText } from './utils'

export function useManagerBatch(mgr: ManagerApi, cardCtxItems: GridItem[]) {
  /**
   * 打印预览 / 送印进行中时，`Esc` 归预览自己处理。
   *
   * 由调用方（`ManagerView` 的 keydown）读取：本组件的 keydown 比 `PrintPreview`
   * 注册得**更早**，会先执行；不拦的话按 `Esc` 会「关预览」+「退出图库模式」同时发生。
   * 用 ref 读，避免把那个 effect 的依赖搞成每渲染一换。
   */
  const printOpenRef = useRef(false)

  // ── 批量动作共用的状态（一把互斥锁 + 一条提示通道 + 打印预览队列）──
  const [exporting, setExporting] = useState(false)
  /** 送印中（禁用预览里的确认按钮，避免连点重复送印） */
  const [printing, setPrinting] = useState(false)
  /** 送印错误（显示在预览浮层里，不关浮层，用户可以改版面重试） */
  const [printError, setPrintError] = useState<string | undefined>(undefined)
  /**
   * 批量打印的待印队列 + 当前预览到第几张。
   *
   * `null` = 预览浮层关着。**版面参数对整批共用一套** —— 预览换图只换样本，
   * 不换设置（否则「预览的是这张的设置，印的是另一套」）。
   */
  const [printBatch, setPrintBatch] = useState<{ paths: string[]; index: number } | null>(null)
  /** 当前预览那张的解码图（走 fs_thumbnail 的 2048 档，够看清版面、又不至于解原图） */
  const [printSrc, setPrintSrc] = useState<DecodedImage | null>(null)

  // 预览/送印开着时，`Esc` 交给预览自己处理（见 `printOpenRef` 的说明）
  useEffect(() => {
    printOpenRef.current = printBatch !== null || printing
  }, [printBatch, printing])
  /**
   * 导出结果提示。`null` = 不显示。
   *
   * 用一条贴在主区底部的浮层，而不是 `alert`：批量导出后用户接着就要看结果，
   * 弹窗还得再点一次「确定」打断节奏。
   */
  const [exportNotice, setExportNotice] = useState<
    { tone: 'ok' | 'warn' | 'err'; text: string; detail?: string } | null
  >(null)
  /** 待重命名的目标集合（`null` = 对话框关着） */
  const [renameFor, setRenameFor] = useState<GridItem[] | null>(null)
  /**
   * 单张重命名的**原地编辑**（用户 2026-10-24：不弹对话框，直接在缩略图的
   * 文件名上改，文本全选）。只有一张且当前是网格视图时走这里；
   * 多张 / 列表视图仍走 `renameFor` 的模板对话框。
   * 提交回调 `doInlineRename` 在 `doRename` 旁边（要用 runBatch，声明顺序）。
   */
  const [inlineRename, setInlineRename] = useState<GridItem | null>(null)

  /** 提示自动消失（错误多留一会儿，够看清） */
  useEffect(() => {
    if (!exportNotice) return
    const ms = exportNotice.tone === 'ok' ? 4000 : 9000
    const t = window.setTimeout(() => setExportNotice(null), ms)
    return () => window.clearTimeout(t)
  }, [exportNotice])

  const doExport = useCallback(async (paths: string[]) => {
    if (exporting) return
    if (paths.length === 0) {
      setExportNotice({ tone: 'warn', text: '这些图片没有可用的本地路径（卷可能未挂载）' })
      return
    }
    const picked = await openDialog({
      directory: true,
      multiple: false,
      title: `导出 ${paths.length} 张图片到文件夹`,
    })
    if (typeof picked !== 'string') return

    setExporting(true)
    try {
      const r = await managerExportImages(paths, picked)
      const tone = r.failed.length > 0 ? 'warn' : r.skipped > 0 ? 'warn' : 'ok'
      let text = `已导出 ${r.copied} 张 → ${r.destDir}`
      if (r.skipped > 0) text += `（${r.skipped} 张因目标就是源目录已跳过）`
      if (r.failed.length > 0) text += `（${r.failed.length} 张失败）`
      setExportNotice({
        tone,
        text,
        detail:
          r.failed.length > 0
            ? r.failed
                .slice(0, 4)
                .map((f) => `${f.name}：${f.reason}`)
                .join('\n') +
              (r.failed.length > 4 ? `\n…另有 ${r.failed.length - 4} 张` : '')
            : undefined,
      })
    } catch (e) {
      setExportNotice({
        tone: 'err',
        text: '导出失败',
        detail: errText(e),
      })
    } finally {
      setExporting(false)
    }
  }, [exporting])

  /** 把某个路径复制到剪贴板（多选时每行一个） */
  const copyPaths = useCallback(async (paths: string[]) => {
    try {
      await navigator.clipboard.writeText(paths.join('\r\n'))
      setExportNotice({
        tone: 'ok',
        text: paths.length > 1 ? `已复制 ${paths.length} 个路径` : '已复制路径',
      })
    } catch {
      setExportNotice({ tone: 'err', text: '复制到剪贴板失败' })
    }
  }, [])

  /** 把批量结果统一成一条提示（转格式 / 摆正 / 打印共用）。 */
  const showBatchResult = useCallback((title: string, r: BatchSummary) => {
    const tone = r.failed.length > 0 ? 'warn' : 'ok'
    const parts = [`${title}：完成 ${r.done} 张`]
    if (r.skipped > 0) parts.push(`跳过 ${r.skipped} 张`)
    if (r.failed.length > 0) parts.push(`失败 ${r.failed.length} 张`)
    if (r.destDir) parts.push(`→ ${r.destDir}`)
    setExportNotice({
      tone,
      text: parts.join(' · '),
      detail:
        r.failed.length > 0
          ? r.failed
              .slice(0, 4)
              .map((f) => `${f.name}：${f.reason}`)
              .join('\n') + (r.failed.length > 4 ? `\n…另有 ${r.failed.length - 4} 张` : '')
          : undefined,
    })
  }, [])

  /** 跑一个批量任务：统一锁按钮、统一捕错。 */
  const runBatch = useCallback(
    async (title: string, task: () => Promise<BatchSummary>) => {
      if (exporting) return
      setExporting(true)
      try {
        showBatchResult(title, await task())
      } catch (e) {
        setExportNotice({
          tone: 'err',
          text: `${title}失败`,
          detail: errText(e),
        })
      } finally {
        setExporting(false)
      }
    },
    [exporting, showBatchResult],
  )

  /**
   * 批量转格式：先选输出目录，再按目标格式重编码。
   *
   * ⚠️ 与「导出」并列而不是合并：导出是字节复制（无损、瞬间），
   * 转格式要整幅解码再编码（慢、JPEG 有损），两者代价差一个数量级。
   */
  const doConvert = useCallback(
    async (format: 'jpg' | 'png' | 'bmp', paths: string[]) => {
      if (paths.length === 0) {
        setExportNotice({ tone: 'warn', text: '这些图片没有可用的本地路径' })
        return
      }
      const picked = await openDialog({
        directory: true,
        multiple: false,
        title: `把 ${paths.length} 张转换为 ${format.toUpperCase()}，选择输出文件夹`,
      })
      if (typeof picked !== 'string') return
      await runBatch(`转换为 ${format.toUpperCase()}`, () =>
        managerConvertImages(paths, picked, format),
      )
    },
    [runBatch],
  )

  /**
   * 批量「另存为」：用**系统原生的保存对话框**（用户更熟、能直接改目录和文件名）。
   *
   * ## 系统对话框只有一个文件名，多张时怎么命名
   * 单张 → 就用用户输入的名字。
   * 多张 → 把它当作**基名**，其余按 `基名_001`、`基名_002` 排下去
   * （序号位数按总数补零）。标题里写明这条规则，避免用户以为会覆盖。
   *
   * ## 格式由扩展名决定
   * 用户把后缀改成 `.png` 就转 PNG；保持原后缀就按原格式（后端**字节复制**，不重编码）。
   * 不在 jpg/png/bmp 白名单里的后缀（如 RAW 的 `.CR3`）→ 按「保持原格式」处理。
   */
  const doSaveAs = useCallback(
    async (paths: string[]) => {
      if (paths.length === 0) {
        setExportNotice({ tone: 'warn', text: '这些图片没有可用的本地路径' })
        return
      }
      const first = paths[0]!
      const picked = await saveDialog({
        title:
          paths.length > 1
            ? `另存为（共 ${paths.length} 张，其余按「名称_序号」命名）`
            : '另存为',
        // 预填第一张的原名与所在目录，用户不改就是「复制一份同名文件」
        defaultPath: first,
        filters: [
          { name: 'JPEG', extensions: ['jpg', 'jpeg'] },
          { name: 'PNG', extensions: ['png'] },
          { name: 'BMP', extensions: ['bmp'] },
        ],
      })
      if (typeof picked !== 'string') return

      // 拆出目录 / 文件名 / 扩展名（Windows 与 POSIX 分隔符都认）
      const sep = Math.max(picked.lastIndexOf('\\'), picked.lastIndexOf('/'))
      const dir = sep >= 0 ? picked.slice(0, sep) : ''
      if (!dir) {
        setExportNotice({ tone: 'err', text: '无法从保存路径确定目标文件夹' })
        return
      }
      const full = sep >= 0 ? picked.slice(sep + 1) : picked
      const dot = full.lastIndexOf('.')
      const base = dot > 0 ? full.slice(0, dot) : full
      const ext = dot > 0 ? full.slice(dot + 1).toLowerCase() : ''
      const fmt: 'jpg' | 'png' | 'bmp' | null =
        ext === 'jpg' || ext === 'jpeg' ? 'jpg' : ext === 'png' ? 'png' : ext === 'bmp' ? 'bmp' : null

      await runBatch('另存为', () =>
        managerSaveAs(paths, {
          destDir: dir,
          format: fmt,
          // 多张：基名 + 序号；单张：用用户给的完整文件名
          nameTemplate: paths.length > 1 ? `${base}_{n}` : null,
          fileName: paths.length === 1 ? full : null,
        }),
      )
    },
    [runBatch],
  )

  /**
   * 按 EXIF 方向摆正。
   *
   * `overwrite = true` 会**改写原文件** —— 必须二次确认，且只在用户明确选择时走这条路。
   */
  const doAutoOrient = useCallback(
    async (overwrite: boolean, paths: string[]) => {
      if (paths.length === 0) {
        setExportNotice({ tone: 'warn', text: '这些图片没有可用的本地路径' })
        return
      }
      let dest: string | null = null
      if (overwrite) {
        const ok = await confirm(
          `按 EXIF 方向摆正并**写回原文件**？\n\n` +
            `· 共 ${paths.length} 张，只处理方向需要旋转（90°/180°）的\n` +
            `· 会重新编码并覆盖原文件，**不可撤销**\n` +
            `· 需要镜像的方向（2/4/5/7）会被跳过\n\n` +
            `建议先在副本上试一次。`,
          { title: '写回原文件', kind: 'warning', okLabel: '写回', cancelLabel: '取消' },
        )
        if (!ok) return
      } else {
        const picked = await openDialog({
          directory: true,
          multiple: false,
          title: '选择摆正后的输出文件夹',
        })
        if (typeof picked !== 'string') return
        dest = picked
      }
      await runBatch(overwrite ? '摆正并写回' : '摆正另存', () =>
        managerAutoOrient(paths, dest, overwrite),
      )
    },
    [runBatch],
  )

  /**
   * 打印：**先开版面预览**（与单图模式同一套 `PrintPreview`），确认后才送印。
   *
   * 单张 = 预览只有一页；多选 = 预览可翻页、确认后逐张按同一版面送印。
   * ⛔ 不要退回「点一下就直接印」——用户没法确认版面，印歪了纸已经浪费了。
   * （旧注释曾写「直接送印、不弹系统对话框」，那是加预览之前的行为，已废弃。）
   */
  const doPrint = useCallback((paths: string[]) => {
    if (paths.length === 0) {
      setExportNotice({ tone: 'warn', text: '这些图片没有可用的本地路径' })
      return
    }
    setPrintError(undefined)
    setPrintSrc(null)
    setPrintBatch({ paths, index: 0 })
  }, [])

  /** 预览样本：换一张就重新取一张缩略图（2048 档够看清版面，不必解原图） */
  useEffect(() => {
    if (!printBatch) {
      setPrintSrc(null)
      return
    }
    let alive = true
    const p = printBatch.paths[printBatch.index]
    if (!p) return
    setPrintSrc(null)
    fsThumbnail(p, 'xl')
      .then((img) => {
        if (alive) setPrintSrc(img)
      })
      .catch(() => {
        if (alive) setPrintSrc(null)
      })
    return () => {
      alive = false
    }
  }, [printBatch])

  /**
   * 确认送印：把预览里选的版面原样交给后端，逐张打印。
   *
   * ⚠️ `quick` 由预览强制为 true（批量时每张弹一次系统对话框没法用），
   * 这里不再从参数里取。
   */
  const confirmBatchPrint = useCallback(
    async (opts: {
      paper: { widthMm: number; heightMm: number }
      fit: boolean
      orientation: string
      quality: string
      marginMm: number
      printer: string
    }) => {
      if (!printBatch || printing) return
      setPrinting(true)
      setPrintError(undefined)
      try {
        const r = await managerPrintImages(printBatch.paths, {
          fit: opts.fit,
          orientation: opts.orientation,
          quality: opts.quality,
          paperWmm: opts.paper.widthMm,
          paperHmm: opts.paper.heightMm,
          marginMm: opts.marginMm,
          printer: opts.printer || null,
        })
        showBatchResult('打印', r)
        setPrintBatch(null)
      } catch (e) {
        // 不关浮层：用户改个版面就能重试，不必从头再点一遍
        setPrintError(errText(e))
      } finally {
        setPrinting(false)
      }
    },
    [printBatch, printing, showBatchResult],
  )

  /**
   * 文件被改过（删除/重命名）之后刷新列表。
   *
   * ⚠️ 两种模式的数据来源不同，不能都只 `reload()`：
   * - 入库模式 → 重查索引（`queryVersion` 触发）；
   * - **浏览模式 → 数据来自 `fs_list_images`，`reload()` 在那儿是空操作**
   *   （查询 effect 对 browse 提前 return），必须重新列目录。
   */
  const refreshAfterMutation = useCallback(async () => {
    // 删除/重命名：行会消失或 key 会变，缩略图自然重取，**不需要**动版本号
    if (mgr.isBrowsing) await mgr.rescan()
    else mgr.reload()
    /*
     * 一并广播「人物数据可能变了」。
     *
     * 这里走的是移动 / 删除 / 旋转保存 / EXIF 摆正等所有「索引已改」的批量出口。
     * 其中重命名、旋转并不动脸，多发一次事件只是让人像条多拉一次列表（一次查询，
     * 开销可忽略）；但反过来漏发就会让界面停在旧数字上且无法自愈
     * —— 2026-10-27「更多图像 共31人」不更新就是这么来的。宁可多发。
     */
    notifyPeopleChanged()
  }, [mgr])

  /**
   * **复制**到文件夹：字节复制（不重新编码、保留原格式与 EXIF）。
   *
   * 与「导出」是同一条后端实现，区别只在于入口在这里、目标目录当场选。
   */
  const doCopyTo = useCallback(
    async (paths: string[]) => {
      if (paths.length === 0) {
        setExportNotice({ tone: 'warn', text: '这些图片没有可用的本地路径' })
        return
      }
      const picked = await openDialog({
        directory: true,
        multiple: false,
        title: `把 ${paths.length} 张复制到文件夹`,
      })
      if (typeof picked !== 'string') return
      await runBatch('复制到文件夹', () => managerCopyToFolder(paths, picked))
    },
    [runBatch],
  )

  /**
   * **移动**到文件夹：文件真的从原位置搬走。
   *
   * ⚠️ 比复制危险，必须二次确认，并说清「会从当前图库索引里移除」——
   * 用户看到照片从列表里消失时才知道是自己干的那件事。
   */
  const doMoveTo = useCallback(
    async (targets: GridItem[]) => {
      const withPath = targets.filter((i) => !!i.path)
      if (withPath.length === 0) {
        setExportNotice({ tone: 'warn', text: '这些图片没有可用的本地路径（卷可能未挂载）' })
        return
      }
      const picked = await openDialog({
        directory: true,
        multiple: false,
        title: `把 ${withPath.length} 张移动到文件夹`,
      })
      if (typeof picked !== 'string') return
      const ok = await confirm(
        `把 ${withPath.length} 张移动到：\n${picked}\n\n` +
          '· 文件会**真的从原位置搬走**（不是复制）\n' +
          '· 已入库的会同时从当前图库的索引与缩略图缓存里移除\n' +
          '· 目标目录里已有同名文件时自动加序号，不会覆盖',
        { title: '移动到文件夹', kind: 'warning', okLabel: '移动', cancelLabel: '取消' },
      )
      if (!ok) return
      await runBatch('移动到文件夹', () =>
        managerMoveToFolder(
          withPath.map((i) => ({ id: i.id, path: i.path })),
          picked,
        ),
      )
      // 源目录少了一批图 —— 必须刷新，否则列表里还留着已经不存在的行
      await refreshAfterMutation()
    },
    [runBatch, refreshAfterMutation],
  )

  /**
   * 批量删除：**移入回收站**（可撤销），再清索引与缓存。
   *
   * ⚠️ 必须说清三件事，否则用户不敢点或者误以为文件彻底没了：
   * 能还原、会清索引、卷未挂载的只能清索引。
   */
  const doDelete = useCallback(async (targetsIn?: GridItem[]) => {
    if (exporting) return
    // 浏览模式的项没有 id，但有 path —— 照样能删（只删文件，没有索引要清）
    // `targetsIn` 由「文件 → 删除…」传选中项；不传就沿用右键作用对象
    const targets = (targetsIn ?? cardCtxItems).filter((i) => i.path != null || i.id != null)
    if (targets.length === 0) {
      setExportNotice({ tone: 'warn', text: '这些项目既没有本地路径也没有索引记录' })
      return
    }
    const offline = targets.filter((i) => !i.path).length
    const withIndex = targets.filter((i) => i.id != null).length
    const ok = await confirm(
      `把 ${targets.length} 张移到回收站？\n\n` +
        `· 文件会**进入回收站**，之后可以从回收站还原\n` +
        (withIndex > 0
          ? `· 其中 ${withIndex} 张已入库，会同时从图库索引移除并清缩略图缓存\n`
          : '· 这些是文件夹里的文件（未入库），只删文件\n') +
        (offline > 0
          ? `· 其中 ${offline} 张所在卷未挂载，只能清索引、删不到文件\n`
          : '') +
        `\n⚠️ 清空回收站之后就无法恢复了。`,
      { title: '删除到回收站', kind: 'warning', okLabel: '移到回收站', cancelLabel: '取消' },
    )
    if (!ok) return

    setExporting(true)
    try {
      const r = await managerDeleteImages(
        targets.map((i) => ({ id: i.id, path: i.path })),
      )
      // 索引已经变了：清空选择 + 重查（已删的行必须真的消失）
      mgr.clearSelection()
      await refreshAfterMutation()
      const parts = [`已移入回收站 ${r.deletedFiles} 个文件`]
      if (r.removedIndex > 0) parts.push(`清理索引 ${r.removedIndex} 条`)
      if (r.offlineIndexOnly > 0) parts.push(`${r.offlineIndexOnly} 张仅清索引`)
      if (r.failed.length > 0) parts.push(`失败 ${r.failed.length} 张`)
      setExportNotice({
        tone: r.failed.length > 0 ? 'warn' : 'ok',
        text: parts.join(' · '),
        detail:
          r.failed.length > 0
            ? r.failed.map((f) => `${f.name}：${f.reason}`).join('\n')
            : '可以到系统回收站里还原',
      })
    } catch (e) {
      setExportNotice({
        tone: 'err',
        text: '删除失败',
        detail: errText(e),
      })
    } finally {
      setExporting(false)
    }
  }, [cardCtxItems, exporting, mgr])

  /**
   * 旋转 / 翻转并**写回原文件**。
   *
   * ## 为什么不弹确认框（2026-10-30 用户要求）
   * 旋转/翻转是**高频**操作，每按一次都弹「会覆盖原文件、不可撤销」的警告
   * 只是噪音 —— 用户早就知道自己在干什么。结果照旧由 `runBatch` 的提示条反馈
   * （成功张数 / 跳过张数 / 失败明细都在那）。
   *
   * ⚠️ RAW / HEIC / WebP / TIFF **写不回去会被跳过**，这是后端行为，前端不拦。
   *
   * ## 顺序
   * 后端是**先旋转、后翻转**；这里每种组合只发一次调用，所以顺序天然一致。
   */
  const doTransform = useCallback(
    async (
      paths: string[],
      opts: { rotate?: number; flipH?: boolean; flipV?: boolean },
      label: string,
    ) => {
      if (paths.length === 0) {
        setExportNotice({ tone: 'warn', text: '这些图片没有可用的本地路径' })
        return
      }
      await runBatch(label, () => managerTransformImages(paths, opts))
      // 文件被真的改写了（size / mtime 都变）：
      // 1) 只给**这几张**打版本号 —— 全量刷新会让 500 张卡片一起重取并闪烁
      // 2) 等重扫落地（索引/目录列表拿到新的 size、mtime）
      mgr.bumpThumbs(paths)
      await mgr.rescan()
    },
    [mgr, runBatch],
  )

  /**
   * 批量重命名：按模板展开新文件名，交给后端逐张执行。
   *
   * ## 模板规则
   * - `{name}` = 原文件名（不含扩展名）
   * - `{n}` = 序号（从 1 开始，位数按总数自动补零：9 张 → `1..9`，100 张 → `001..100`）
   * - **扩展名永远保留**（用户在模板里写破天也不会把 .jpg 弄丢）
   * - 模板里没有 `{n}` 时**自动补 `_{n}`**：否则 N 张会全叫同一个名字，
   *   除第一张外全部失败 —— 那不是用户想要的「批量重命名」
   */
  const doRename = useCallback(
    async (template: string) => {
      const items = renameFor
      setRenameFor(null)
      if (!items || items.length === 0) return
      // 浏览模式的行没有 id，但照样能改名（纯文件重命名）
      const rows = items
      if (rows.length === 0) return

      const hasIndex = /\{n\}/i.test(template)
      const tpl = hasIndex ? template : `${template}_{n}`
      const width = String(rows.length).length < 3 ? 3 : String(rows.length).length

      const payload = rows.map((it, i) => {
        const dot = it.name.lastIndexOf('.')
        const stem = dot > 0 ? it.name.slice(0, dot) : it.name
        const ext = dot > 0 ? it.name.slice(dot) : ''
        const n = String(i + 1).padStart(width, '0')
        // 非法字符换成下划线：Windows 上会让 rename 直接失败
        const body = tpl
          .replace(/\{name\}/gi, stem)
          .replace(/\{n\}/gi, n)
          .replace(/[\\/:*?"<>|]/g, '_')
          .trim()
        return { id: it.id, path: it.path, newName: (body || stem) + ext }
      })

      await runBatch('重命名', () => managerRenameImages(payload))
      // 文件名变了：入库模式改的是索引 rel_name，浏览模式改的是磁盘 —— 两边都要刷
      await refreshAfterMutation()
    },
    [mgr, renameFor, runBatch],
  )

  /**
   * 原地重命名提交（单张，见 `inlineRename`）：输入的是**去掉扩展名的主体**，
   * 扩展名始终保留；非法字符换成下划线（与批量重命名同一套规则）。
   */
  const doInlineRename = useCallback(
    async (item: GridItem, input: string) => {
      setInlineRename(null)
      const body = input.trim().replace(/[\\/:*?"<>|]/g, '_')
      if (!body) return
      const dot = item.name.lastIndexOf('.')
      const stem = dot > 0 ? item.name.slice(0, dot) : item.name
      const ext = dot > 0 ? item.name.slice(dot) : ''
      if (body === stem) return
      await runBatch('重命名', () =>
        managerRenameImages([{ id: item.id, path: item.path, newName: body + ext }]),
      )
      await refreshAfterMutation()
    },
    // 与 doRename 保持同一组依赖（refreshAfterMutation 随 mgr 稳定）
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [runBatch],
  )

  /*
   * 只导出**调用方真的用到**的成员。`showBatchResult` / `runBatch` /
   * `refreshAfterMutation` 是内部工具（前者被那几个动作共用），不对外。
   */
  return {
    // 状态
    exporting,
    printing,
    printError,
    printBatch,
    setPrintBatch,
    printSrc,
    exportNotice,
    setExportNotice,
    renameFor,
    setRenameFor,
    inlineRename,
    setInlineRename,
    printOpenRef,
    // 动作
    doExport,
    copyPaths,
    doConvert,
    doSaveAs,
    doAutoOrient,
    doPrint,
    confirmBatchPrint,
    doCopyTo,
    doMoveTo,
    doDelete,
    doTransform,
    doRename,
    doInlineRename,
  }
}
