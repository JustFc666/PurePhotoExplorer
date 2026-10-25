/**
 * 缩略图加载单元 —— 图库卡片网格与单图左侧胶片条**共用**的取图单元。
 *
 * ## 两种来源（浏览模式与入库模式的根本区别就在这里）
 * - `{ kind: 'index', imageId }` → `manager_thumbnail`：查 L1/L2，miss 就现生成并**落盘缓存**
 * - `{ kind: 'path', path }`      → `fs_thumbnail`：只解码缩放，**不写任何缓存**
 *
 * ⚠️ 两条路必须分开，不能「同一个命令加个开关」：入库那条要索引行 id，
 * 而浏览的目录**根本没入过库**，没有 id 可用。
 *
 * ## 其余四件事都在这里收口，避免两处各写一遍
 * 1. **进视口才取图**（`useInView`）：一页最多 500 行，全量请求会把后端的串行队列塞满；
 * 2. **并发闸门**（`withThumbSlot`）：最多 4 个在飞，可见的插队、预取的排后面；
 * 3. **画布常驻挂载**：`.then` 里要往 `canvasRef.current` 上 `putImageData`，
 *    若画布是加载完成才挂上去的，这一刻 ref 还是 `null` → 图永远画不出来。
 *    没图时用 CSS 藏（否则会露出浏览器给的默认 300×150 空白画布）。
 * 4. **预取**（`prefetch`）：不等进视口就先取，让下一屏滚动过去时不是白卡。
 */

import { useEffect, useRef, useState } from 'react'

import { managerThumbnail, type ThumbTier } from '../../ipc/manager'
import { fsThumbnail } from '../../ipc/browse'
import { withThumbSlot } from '../../ipc/thumbQueue'
import { useInView } from '../../hooks/useInView'

/** 取图来源：已入库（索引 id）或浏览模式的临时文件（绝对路径）。 */
export type ThumbSource =
  | { kind: 'index'; imageId: number; rev?: number }
  | { kind: 'path'; path: string; rev?: number }

/** `idle` = 还没轮到它（无可预取、也未进视口），尚未发请求。 */
type State = 'idle' | 'loading' | 'ok' | 'bad'

/**
 * 「轮到我了，但已经不需要了」的标记。
 *
 * 预取请求可能在队列里等几百毫秒 —— 那时用户早就滚到别处去了。
 * 这种请求要**静默放弃**，不能算「无法预览」（否则回滚时会看到一堆错标）。
 */
const STALE = 'pv:stale-thumb'

/** 把来源压成稳定字符串，用作 effect 依赖。 */
function sourceKey(source: ThumbSource): string {
  // ⚠️ `rev` 必须进 key：文件被原地改写（旋转/翻转保存）时 imageId / path 都没变，
  // 只有 mtime 变了。不带 rev 的话 effect 不会重跑，用户看到的是**旧画面**。
  const base = source.kind === 'index' ? `i:${source.imageId}` : `p:${source.path}`
  return source.rev ? `${base}#${source.rev}` : base
}

/**
 * 由「一行数据」推出取图来源。
 *
 * 结构类型（只要带 `id`/`path`）而不是 import `GridItem`：
 * 卡片、胶片条、`GridItem` 三处都不用互相依赖。
 * 返回 `null` = 这一行**没法取图**（已入库但卷没挂载，`absPath` 为 null）。
 */
export function thumbSourceOf(
  item: { id: number | null; path: string | null },
  /**
   * 内容版本号：文件被**原地改写**（旋转/翻转保存、覆盖另存）后由调用方 +1，
   * 逼 `ThumbCanvas` 重取一次。不传 = 不参与缓存键（绝大多数场景）。
   */
  rev?: number,
): ThumbSource | null {
  if (item.id != null) return { kind: 'index', imageId: item.id, rev }
  if (item.path) return { kind: 'path', path: item.path, rev }
  return null
}

interface Props {
  source: ThumbSource
  tier: ThumbTier
  /** 外层容器的额外类名（尺寸/圆角由使用方定） */
  className?: string
  /**
   * 预取：还没进视口就先把图取回来。
   *
   * 由父层按「可见窗口 + 后 30 张」算出来。⚠️ 正常显示路径**不要**开它 ——
   * 一页 500 张全开就等于没有懒加载。
   */
  prefetch?: boolean
}

export function ThumbCanvas({ source, tier, className, prefetch = false }: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const [state, setState] = useState<State>('idle')
  const { ref, inView } = useInView<HTMLDivElement>()
  const key = sourceKey(source)

  const trigger = inView || prefetch

  /**
   * 最近一次渲染时「这张还需要吗」。
   *
   * 排队期间父层可能把 `prefetch` 撤回（用户滚走了），那一刻组件**已经不再是
   * 那个状态**，但 effect 里的闭包还是旧的 —— 用 ref 才读得到最新值。
   */
  const wantedRef = useRef(false)
  /** 上一次加载的身份键（不含 `rev`）；用来判断是「换图」还是「同图内容更新」 */
  const lastIdentity = useRef<string>('')
  wantedRef.current = trigger

  useEffect(() => {
    if (!trigger) return
    let alive = true
    /**
     * 「同一张图的内容更新」vs「换了一张图」。
     *
     * `rev` 只表示**同一个文件被原地改写**（旋转/翻转保存）。这种情况要
     * **保留旧画面静默替换**：一进 loading 就把 canvas 藏起来的话，
     * 刷新时整页缩略图会同时闪一下白。
     *
     * 只有身份（imageId / path）真的变了才值得显示 loading 占位。
     */
    const identity = source.kind === 'index' ? `i:${source.imageId}` : `p:${source.path}`
    const sameImage = lastIdentity.current === identity
    if (!sameImage) setState('loading')
    lastIdentity.current = identity
    const load =
      source.kind === 'index'
        ? () => managerThumbnail(source.imageId, tier)
        : () => fsThumbnail(source.path, tier)
    withThumbSlot(
      () => {
        /**
         * ⚠️ 排队期间组件可能已经卸载（用户切了目录 / 换了范围）：
         * 轮到它时**直接放弃**，别白解码。
         *
         * 这条判据很关键：一页能挂 500 个单元格、闸门只放 4 个并发，
         * 于是队列里随时压着几百个待办。没有这道检查的话，
         * 「在一个大目录里看一眼就走」会在后台继续把几百张图全解一遍
         * （每张 0.2–2s，等于白白跑几分钟 CPU 与磁盘）。
         *
         * 预取的请求多一条：**不再需要**也得放弃 —— 用户已经滚过去，
         * 取回来只是白占一个槽位。
         */
        if (!alive || !wantedRef.current) throw new Error(STALE)
        return load()
      },
      // 正在看的插队；预取的排后面（否则预取会拖慢用户真正要看的下一张）
      inView ? 'high' : 'low',
    )
      .then((img) => {
        if (!alive) return
        const cv = canvasRef.current
        if (!cv) return
        cv.width = img.width
        cv.height = img.height
        cv.getContext('2d')?.putImageData(new ImageData(img.data, img.width, img.height), 0, 0)
        setState('ok')
      })
      .catch((e: unknown) => {
        if (!alive) return
        // 放弃 ≠ 失败：回到 idle，等它再被需要时重新触发
        if (e instanceof Error && e.message === STALE) {
          setState('idle')
          return
        }
        setState('bad')
      })
    return () => {
      alive = false
    }
    // 依赖用 `key` 而不是 `source` 对象：父组件每次渲染都会新建这个对象，
    // 直接进依赖会无限重取。
    // 依赖 `trigger` 而不是 `inView`：预取已经把图取回来之后，进视口不该再请求一次。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [trigger, key, tier])

  return (
    <div className={`pv-mg-thumbbox${className ? ` ${className}` : ''}`} ref={ref}>
      <canvas ref={canvasRef} className={state === 'ok' ? undefined : 'is-hidden'} />
      {state === 'bad' && <span className="pv-mg-bad">无法预览</span>}
      {(state === 'idle' || state === 'loading') && <span className="pv-mg-loading">…</span>}
    </div>
  )
}
