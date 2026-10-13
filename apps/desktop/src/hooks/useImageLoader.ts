/**
 * 图像加载状态机 + 同目录浏览。
 *
 * - 探测 → 解码 → 状态切换，快速连续打开时用请求序号丢弃过期响应（防串图）；
 * - 记录当前图片所在文件夹的图片列表，提供上一张/下一张。
 */

import { useCallback, useEffect, useRef, useState } from 'react'
import {
  decodeAnimationProgressive,
  decodeImage,
  getFileVersion,
  invalidateImage,
  listSiblings,
  prefetchImage,
  probeImage,
  toUserMessage,
  traceLog,
} from '../ipc'
import type { DecodedAnimation, DecodedImage, ImageInfo } from '../ipc'
import type { Status } from '../types/viewer'
import { locateInList, shouldRelist } from './siblingsCache'

/** 预取窗口：以当前图为中心，向后 5 张、向前 2 张（按「最可能先看」排序） */
const PREFETCH_OFFSETS = [1, -1, 2, -2, 3, 4, 5]

/** 取路径所在目录（兼容 `/` 与 `\`） */
function dirOf(p: string): string {
  const i = Math.max(p.lastIndexOf('/'), p.lastIndexOf('\\'))
  return i >= 0 ? p.slice(0, i) : ''
}

export interface UseImageLoader {
  /** 已解码的当前图（无图时为 null）。动图时是**第一帧**，播放用 `animation` */
  image: DecodedImage | null
  /**
   * 动图数据（目前只有 GIF）；非动图为 `null`。
   *
   * GIF 走 `decode_animation` **渐进式**取帧（首批约 8MB 先播，
   * 其余后台补齐）—— 它既能覆盖静态 GIF
   * （只有一帧），也能喂给播放器。所以判断「能不能播」要看
   * `animation.frames.length > 1`，而不是这个字段是否为 null。
   */
  animation: DecodedAnimation | null
  /** 当前图的探测信息 */
  info: ImageInfo | null
  /** 状态机 */
  status: Status
  /** 打开并加载一张图片 */
  load: (path: string) => Promise<void>
  /**
   * **重新读取当前图片**（右键「刷新」）。
   *
   * 会先失效该路径的解码缓存再加载 —— 手动刷新就该"无条件重读磁盘"。
   * ⚠️ 视图会复位到适应窗口（与「覆盖保存后自动刷新」同一行为）。
   */
  reload: () => Promise<void>
  /**
   * **重新列一次当前目录**（不重新解码当前图）。
   *
   * 用于「同目录里多了一张新图」之后刷新翻页序列与「N / M」计数 ——
   * 另存为、拷贝文件进来之后调用，否则新文件要关掉重开才会被认出来。
   *
   * ⚠️ 与 [`reload`] 不同：它**不动画布**（当前图原样留在屏幕上）。
   */
  refreshSiblings: () => Promise<void>
  /**
   * 当前（最后一次请求的）图片路径；没有图时为 `null`。
   *
   * ⚠️ **加载失败时也保留**（不像 `info` 会被清空）—— 「刷新」要能用于
   * 「文件坏了 / 被替换了，重读一遍」这个场景。
   */
  path: string | null
  /** 外部失败（对话框/拖放）统一走这里设置错误态 */
  fail: (err: unknown) => void
  /** 当前项在同目录列表中的索引（-1 = 未知） */
  index: number
  /** 同目录图片总数 */
  total: number
  canPrev: boolean
  canNext: boolean
  prev: () => void
  next: () => void
}

export function useImageLoader(): UseImageLoader {
  const [info, setInfo] = useState<ImageInfo | null>(null)
  const [image, setImage] = useState<DecodedImage | null>(null)
  const [animation, setAnimation] = useState<DecodedAnimation | null>(null)
  const [status, setStatus] = useState<Status>({ kind: 'empty' })
  const [index, setIndex] = useState(-1)
  const [total, setTotal] = useState(0)
  /** 「刷新」用的当前路径；失败也保留（见 `UseImageLoader.path` 的说明） */
  const [path, setPath] = useState<string | null>(null)
  /**
   * `path` 的 ref 镜像 —— 给 `refreshSiblings` 这类**不带参数**的回调读当前路径用。
   *
   * `useCallback(..., [])` 捕获的是**首次渲染**的 `path`（一直是 `null`），
   * 直接用 state 会永远拿不到路径。state 负责渲染、ref 负责回调内读取。
   */
  const pathRef = useRef<string | null>(null)

  /** 解码请求序号，用于丢弃过期响应（防串图） */
  const requestId = useRef(0)
  /** 同目录图片列表与当前索引（ref 供同步读取，state 供渲染） */
  const siblingsRef = useRef<string[]>([])
  const indexRef = useRef(-1)
  const dirRef = useRef<string | null>(null)
  /** 预取任务代号：切图后自增，使旧的预取链自动放弃 */
  const prefetchGen = useRef(0)

  const fail = useCallback((err: unknown) => {
    setStatus({ kind: 'error', message: toUserMessage(err), raw: err })
  }, [])

  /**
   * 刷新同目录列表并定位到 `path`。
   *
   * **必须在探测之前调用**：这样即使这张图读不出来（损坏 / 格式不支持），
   * `indexRef` 也已指向正确位置，上下张快捷键依然能翻走 ——
   * 否则用户会卡在一张坏图上，只能拿鼠标点工具栏。
   *
   * ⚠️ `force = false` 时目录未变就跳过重列（省一次 IPC）。
   * 代价是**看不见「同目录里多了一张新图」** —— 所以任何往目录写新文件的
   * 入口（另存为、手动刷新）都必须传 `force = true`，见 [`refreshSiblings`]。
   */
  const syncSiblings = useCallback(async (path: string, id: number, force = false) => {
    const dir = dirOf(path)
    const cache = { dir: dirRef.current, list: siblingsRef.current }
    if (shouldRelist(cache, dir, force)) {
      const list = await listSiblings(path).catch(() => [] as string[])
      if (id !== requestId.current) {
        traceLog(`[nav] 列目录结果被丢弃 id=${id} cur=${requestId.current}`)
        // ⚠️ **刻意不写 dirRef**：这次列的结果已经作废，若此时把 dirRef
        // 标成「已列过」，而列表还是旧的，下一次 load() 就会命中缓存分支
        // 跳过重列 —— 新文件永远进不来（正是本函数要修的那个症状）。
        // 留下 `null` / 旧值，让下一次调用重新列。
        return
      }
      // 结果落地后才提交缓存：目录 + 列表 + 索引三者始终一致
      dirRef.current = dir
      siblingsRef.current = list
      setTotal(list.length)
      indexRef.current = locateInList(list, path)
      setIndex(indexRef.current)
    } else {
      indexRef.current = locateInList(siblingsRef.current, path)
      setIndex(indexRef.current)
    }
  }, [])

  /**
   * 强制重新列一次当前目录，并重新定位到 `path`。
   *
   * ## 为什么需要它
   * [`syncSiblings`] 为了省一次 IPC，**只在目录路径变化时**才重新列目录。
   * 这对"翻图"是对的（同目录翻页时目录内容不会自己变），但对
   * **「同目录里多了一张新图」是错的**：
   *
   * - 用户在当前文件夹下**另存为**出一张新图；
   * - 或从别处拷进来一张图。
   *
   * 此时 `siblingsRef` 还是旧列表 —— 新文件**根本不在翻页序列里**，
   * 状态栏的「N / M」也不对，用户会以为程序没认出新文件
   * （症状：必须关掉重开才能看到）。
   *
   * 后端 `list_siblings` 每次都真读磁盘、没有缓存，所以只要让它重新调一次即可。
   * ⚠️ 与解码缓存**无关**：解码缓存那条线（`invalidateImage` / 文件版本）
   * 已经由 `ipc/edit.ts` 在保存后处理好，这里修的是**目录列表**。
   */
  const refreshSiblings = useCallback(async () => {
    const cur = pathRef.current
    if (!cur) return
    // 复用 syncSiblings 并强制重列 —— 避免两份「怎么更新 index」的实现漂移。
    // id 传当前请求号：refreshSiblings 不改请求状态，让结果正常落地。
    await syncSiblings(cur, requestId.current, true)
  }, [syncSiblings])

  const load = useCallback(
    async (path: string) => {
      const id = ++requestId.current
      setPath(path)
      pathRef.current = path
      setStatus({ kind: 'loading' })
      // 立刻清掉上一张的动图：否则新图还没解码完，播放器仍在循环旧 GIF
      setAnimation(null)

      // 先同步目录与索引（与探测成败无关，保证坏图也能翻页）
      void syncSiblings(path, id)

      try {
        // 探测（快，能拿到尺寸与格式，便于立刻显示信息）+ 取文件版本
        // （纯 stat）：版本一起交给 `decodeImage` 做**缓存校验** ——
        // 文件被覆盖过就重新解码，而不是把旧像素显示给用户。
        const [meta, fileVersion] = await Promise.all([probeImage(path), getFileVersion(path)])
        if (id !== requestId.current) return // 已有更新的请求，丢弃
        setInfo(meta)

        // 再解码（慢）。**GIF 与 WebP** 走动图通道（后端 `decode_animation`
        // 按魔数分发到各自的解码器，并统一成同一套 Animation）：
        // **渐进式**——首批约 8MB 到位就立刻显示并开播，剩下的帧在后台补齐
        // （大动图整段 70MB+，一次拿完要 1.2 秒，用户得干等）。
        // 静态的（只有一帧）也走这里，播放器自然按「不可播放」处理。
        if (meta.format === 'gif' || meta.format === 'webp') {
          await decodeAnimationProgressive(
            path,
            'preview',
            (batch, isFirst) => {
              if (id !== requestId.current) return
              if (isFirst) {
                setAnimation(batch)
                setImage(batch.frames[0]?.image ?? null)
                // 首批到手即可显示：别让状态栏一直停在「载入中」
                setStatus({ kind: 'ready' })
              } else {
                // 追加后续帧。**新建对象**是为了让依赖 `animation` 的组件
                // （帧号显示等）重新渲染；播放位置不会被打断，
                // 因为 `useAnimationPlayer` 的重置判据是「图片路径变化」。
                setAnimation((prev) =>
                  prev
                    ? {
                        ...prev,
                        frames: [...prev.frames, ...batch.frames],
                        count: prev.frames.length + batch.count,
                      }
                    : batch,
                )
              }
            },
            () => id !== requestId.current,
          )
        } else {
          const img = await decodeImage(path, 'preview', fileVersion)
          if (id !== requestId.current) return
          setImage(img)
        }
        setStatus({ kind: 'ready' })
      } catch (err) {
        if (id !== requestId.current) return
        setImage(null)
        setInfo(null)
        setAnimation(null)

        const message = toUserMessage(err)
        // 后端已经把「具体原因」写在 message 里了，这里只补「下一步怎么办」。
        // 顺序有讲究：先匹配最具体的原因，避免被宽泛的 RAW 分支吃掉。
        if (message.includes('Git LFS') || message.includes('0 字节') || message.includes('文本')) {
          // 非图像文件：后端文案已足够清楚，不再追加提示
          setStatus({ kind: 'error', message, raw: err })
        } else if (message.includes('不含可用') || message.includes('未内嵌')) {
          setStatus({
            kind: 'error',
            message,
            hint: '该 RAW 未内嵌预览图，需等完整 RAW 解码（Stage 2）接入后才能查看',
            raw: err,
          })
        } else if (message.includes('RAW')) {
          setStatus({
            kind: 'error',
            message,
            hint: 'RAW 通过提取相机内嵌预览来显示，暂不支持无内嵌预览的文件',
            raw: err,
          })
        } else {
          setStatus({ kind: 'error', message, raw: err })
        }
      }
    },
    [syncSiblings],
  )

  /**
   * **重新读取当前图片**（右键「刷新」）。
   *
   * 与普通 `load` 的唯一区别：**先失效缓存**再加载。
   *
   * 缓存里带着文件版本校验（`decodeImage`），能自动发现"文件被覆盖"——
   * 但那只在**重新读**的时候生效，不会替你触发重读；而正在看的那张图
   * 没人会替它翻页。所以需要一个显式入口。
   *
   * 为什么还要显式失效：版本校验依赖 mtime+size。要是别的工具改完图还
   * 原样写回 mtime（或用户就是想强制重读），只有失效才可靠。
   */
  const reload = useCallback(async () => {
    if (!path) return
    invalidateImage(path)
    await load(path)
  }, [path, load])

  /** 跳到相对偏移的图片 */
  const go = useCallback(
    (delta: number) => {
      const list = siblingsRef.current
      const target = indexRef.current + delta
      if (indexRef.current < 0) {
        // 索引未知 = 目录列表还没就绪。这是**异常**（正常翻到两头不算），
        // 留一条日志：这种情况用户看到的是「滚了没反应」，而从界面看不出原因。
        traceLog(`[nav] go(${delta}) 无法翻页：索引未知（list=${list.length}）`)
        return
      }
      // 到头了 —— 正常边界，静默返回
      if (target < 0 || target >= list.length) return
      indexRef.current = target
      setIndex(target)
      void load(list[target]!)
    },
    [load],
  )
  const next = useCallback(() => go(1), [go])
  const prev = useCallback(() => go(-1), [go])

  /**
   * 预取相邻图片（后 5 + 前 2）。
   *
   * - **顺序**预取：最近的先就绪，不并发抢 CPU；
   * - 每次索引变化都会使旧任务作废（`gen` 令牌），避免无谓解码；
   * - 命中缓存时 `decodeImage` 直接返回，翻页几乎瞬时。
   */
  useEffect(() => {
    const gen = ++prefetchGen.current
    const list = siblingsRef.current
    const current = indexRef.current
    if (current < 0 || list.length === 0) return

    const targets: string[] = []
    for (const offset of PREFETCH_OFFSETS) {
      const t = current + offset
      if (t >= 0 && t < list.length) targets.push(list[t]!)
    }

    void (async () => {
      for (const path of targets) {
        if (prefetchGen.current !== gen) return // 已切图，放弃剩余预取
        await prefetchImage(path)
      }
    })()
  }, [index, total])

  return {
    image,
    animation,
    info,
    status,
    load,
    reload,
    refreshSiblings,
    path,
    fail,
    index,
    total,
    canPrev: index > 0,
    canNext: index >= 0 && index < total - 1,
    prev,
    next,
  }
}
