/**
 * 单图浏览的**视图状态机**（hook 本体：状态 + 事件处理器）。
 *
 * 把「适应窗口 / 缩放 / 平移 / 旋转 / 拖拽」的交互逻辑从组件里剥离出来，
 * 组件只负责渲染与事件绑定。
 *
 * 纯几何计算已按职责拆到 `./imageView/`（`types` / `geometry` / `frame` / `fit`），
 * 本文件只留状态与副作用。文件末尾的 `export *` 保证原来
 * `import … from './useImageView'` 的引用（组件、测试）**一行都不用改**。
 *
 * 坐标语义与性能约束见 `./imageView/types.ts` 的模块说明。
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'

import type { DecodedImage } from '../ipc'
import {
  ASPECT_VALUES,
  MAX_FINE_ANGLE,
  MAX_SCALE,
  MIN_FINE_ANGLE,
  MIN_SCALE,
} from './imageView/types'
import type {
  AspectRatio,
  CropRect,
  FitMode,
  FrameDragMode,
  FrameRect,
  ViewState,
} from './imageView/types'
import { rectToCrop, rotatedImageSize, sameCrop } from './imageView/geometry'
import {
  activeFrameRect,
  constrainRectToRatio,
  frameRectToPixels,
  imageVisibleRect,
} from './imageView/frame'
import { canPanImage, computeFitFor } from './imageView/fit'

// 对外接口保持不变：组件与测试都从本文件引类型/常量/纯函数。
export * from './imageView/types'
export * from './imageView/geometry'
export * from './imageView/frame'
export * from './imageView/fit'

/** 拖拽状态：只需记录上一次鼠标位置，用增量计算位移 */
interface DragState {
  lastX: number
  lastY: number
}

/** 按钮缩放步进 */
const BUTTON_ZOOM_STEP = 1.25
/** 平移步进（方向键/工具按钮用），CSS 像素 */
const PAN_STEP = 60
/** 滚轮缩放系数 */
const WHEEL_ZOOM_FACTOR = 1.12

const INITIAL_VIEW: ViewState = {
  scale: 1,
  tx: 0,
  ty: 0,
  rotation: 0,
  fineAngle: 0,
  aspect: 'free',
  frameScale: 1,
  frameRect: null,
  crop: null,
  fit: 'contain',
}

export interface UseImageView {
  view: ViewState
  /** 拖拽平移中（用于切换光标） */
  dragging: boolean
  /** 图片当前是否超出容器（只有超出才允许拖拽平移，也才显示 grab 光标） */
  canPan: boolean
  /**
   * 左键按下开始拖拽平移。
   *
   * 绑定到画布容器上即可；**裁剪语境内不要绑**（那时左键归裁剪框，
   * 画面必须保持不动）。是否真的会生效由 `canPan` 决定。
   */
  startPan: (e: React.MouseEvent) => void
  /**
   * 裁剪框的屏幕矩形（容器 CSS 像素）；无图 / 尺寸未就绪时为 `null`。
   *
   * **两种模式都有框**（2026-10-08 统一）：没拖过时显示的是**预置框**
   * （固定比例 = 视口内该比例的最大矩形；自由 = 图像可见矩形，初始即整图），
   * 拖过之后就是用户定下的那个。拖四角改大小、拖内部平移，**全程画面不动**。
   */
  frameBox: { left: number; top: number; width: number; height: number } | null
  /** 正在拖固定比例的框（改大小或平移），用于切换光标 */
  frameDragging: FrameDragMode | null
  /**
   * 开始拖固定比例的框（拖四角改大小 / 拖内部平移）。
   * `aspect === 'free'` 时是空操作。
   */
  onFrameMouseDown: (mode: FrameDragMode) => (e: React.MouseEvent) => void
  /** 放大到 150%+ 时用 pixelated 保持像素锐利 */
  isPixelated: boolean
  /** 量取容器尺寸用，绑定到画布容器元素 */
  containerRef: React.RefObject<HTMLDivElement>
  onWheel: (e: React.WheelEvent) => void
  onDoubleClick: () => void
  /** 直接设定缩放（以视口中心为锚点） */
  setZoom: (scale: number) => void
  /** 设定细调角度（度，会被夹到 ±45），并重算适应 */
  setFineAngle: (deg: number) => void
  /** 设定裁剪比例，并重算适应（会清掉自由框选、固定比例的框回到预置位置） */
  setAspect: (aspect: AspectRatio) => void
  /** 清掉自由框选的裁剪区（不动角度与比例），并把视图退回整图适应 */
  clearCrop: () => void
  /** 把旋转、角度、比例与框选一并复位，并回到整图适应 */
  resetAdjust: () => void
  /**
   * 取当前视图状态的一份**快照**（含 scale/tx/ty/角度/比例/框选）。
   *
   * 用途：打开画布调整面板时先存下来，若用户没点「覆盖原图」就关闭面板，
   * 用 [`restoreView`] 原样还原 —— 这样「退出调整」= 「放弃这次调整」
   * （2026-10-07 用户要求：「如果调整没有覆盖原图，退出调整后应恢复调整前的显示」）。
   * 是**深拷贝**，之后视图继续变化不会影响快照。
   */
  snapshotView: () => ViewState
  /** 用 [`snapshotView`] 的快照还原视图（含缩放平移，逐字段覆盖） */
  restoreView: (snap: ViewState) => void
}

/**
 * 视图状态机 hook。
 *
 * @param image 当前解码图（换图会重置旋转并按 `contain` 适应窗口）
 */
export function useImageView(image: DecodedImage | null): UseImageView {
  const containerRef = useRef<HTMLDivElement>(null)
  const [view, setView] = useState<ViewState>(INITIAL_VIEW)
  const [dragging, setDragging] = useState(false)
  const dragState = useRef<DragState | null>(null)
  // 拖拽时用 ref 累积增量，避免每个 mousemove 都触发重渲染
  const pendingDelta = useRef<{ dx: number; dy: number } | null>(null)
  const rafId = useRef<number | null>(null)
  /** 上次滚轮翻页时间，用于冷却 */
  /** 保持最新的导航回调，避免 effect 依赖抖动 */

  /** 正在拖裁剪框（改大小/平移），`null` = 没在拖 */
  const [frameDragging, setFrameDragging] = useState<FrameDragMode | null>(null)
  /** 容器尺寸镜像（给 `frameBox` 派生用；由渲染帧写入，避免额外 ResizeObserver） */
  const [containerSize, setContainerSize] = useState({ w: 0, h: 0 })
  /** 用 ref 读最新的 view，避免框选 effect 因 view 变化反复重建监听 */
  const viewRef = useRef(view)
  viewRef.current = view

  /** 计算适应窗口的缩放倍率与居中位置（依赖当前旋转角/细调角/比例/框选/框缩放/框矩形） */
  const computeFit = useCallback(
    (fit: FitMode, containerW: number, containerH: number) =>
      computeFitFor(
        view.rotation,
        fit,
        image,
        containerW,
        containerH,
        view.fineAngle,
        view.aspect,
        view.crop,
        view.frameScale,
        view.frameRect,
      ),
    [
      image,
      view.rotation,
      view.fineAngle,
      view.aspect,
      view.crop,
      view.frameScale,
      view.frameRect,
    ],
  )

  // 换图时重置旋转与调整参数，按 contain 适应窗口；容器尺寸变化时跟随重算
  useEffect(() => {
    const el = containerRef.current
    if (!el || !image) return

    const syncSize = () => {
      const r = el.getBoundingClientRect()
      setContainerSize((prev) =>
        prev.w === r.width && prev.h === r.height ? prev : { w: r.width, h: r.height },
      )
    }

    const fitTo = (fitMode: FitMode) => {
      const rect = el.getBoundingClientRect()
      const fit = computeFitFor(0, fitMode, image, rect.width, rect.height, 0, 'free', null, 1)
      // 新图：旋转、细调角、比例、框缩放、框矩形、框选一并复位
      if (fit) {
        setView((v) => ({
          ...v,
          ...fit,
          fit: fitMode,
          rotation: 0,
          fineAngle: 0,
          aspect: 'free',
          frameScale: 1,
          frameRect: null,
          crop: null,
        }))
      }
    }

    fitTo('contain')
    syncSize()

    const ro = new ResizeObserver(() => {
      syncSize()
      // 只有处于适应模式时才跟随容器变化重算
      setView((v) => {
        if (v.fit === 'none') return v
        const rect = el.getBoundingClientRect()
        const fit = computeFitFor(
          v.rotation,
          v.fit,
          image,
          rect.width,
          rect.height,
          v.fineAngle,
          v.aspect,
          v.crop,
          v.frameScale,
          v.frameRect,
        )
        return fit ? { ...v, ...fit } : v
      })
    })
    ro.observe(el)
    return () => ro.disconnect()
  }, [image])

  /**
   * 画布上的滚轮 —— **只处理 Ctrl/⌘ + 滚轮（缩放）**。
   *
   * ⚠️ 普通滚轮的「翻页」不在这里，而是窗口级的 [`useWheelNavigation`]。
   * 原因：画布在**解码失败时会被 `<EmptyState>` 替换掉**（整个组件被卸载），
   * 挂在这里的监听会一起消失 —— 用户遇到坏图就滚不动、翻不了页。
   * 翻页是查看器的整体行为，不能依赖画布是否在渲染。
   *
   * 缩放保留在画布：它以**光标在画布内的坐标**为锚点，且无图时没有意义。
   */
  const onWheel = useCallback(
    (e: React.WheelEvent) => {
      if (!image) return
      // 普通滚轮放行（冒泡到 window 的翻页监听），不 preventDefault
      if (!e.ctrlKey && !e.metaKey) return
      e.preventDefault()

      // 缩放：以光标位置为锚点（保持光标下的图像点不动）
      const el = containerRef.current
      if (!el) return
      const rect = el.getBoundingClientRect()
      const cx = e.clientX - rect.left
      const cy = e.clientY - rect.top

      setView((v) => {
        const factor = e.deltaY < 0 ? WHEEL_ZOOM_FACTOR : 1 / WHEEL_ZOOM_FACTOR
        const newScale = Math.min(MAX_SCALE, Math.max(MIN_SCALE, v.scale * factor))
        const ratio = newScale / v.scale
        return {
          ...v,
          scale: newScale,
          tx: cx - (cx - v.tx) * ratio,
          ty: cy - (cy - v.ty) * ratio,
          fit: 'none',
        }
      })
    },
    [image],
  )

  /**
   * 开始拖裁剪框：**拖四角改大小** / **拖框内部平移**（**两种模式共用**）。
   *
   * 2026-10-08 统一：自由与固定比例不再是两套交互，都是「画面上有个框，
   * 拖角改大小、拖内部平移」。区别只有一处 —— 固定比例改大小时宽高比锁死。
   *
   * 直接在事件里现场挂 window 监听（不用 effect）—— 拖拽几何走的是闭包，
   * 放 effect 里会因为派发时机不对而挂不上监听。
   *
   * 约束：
   * - 固定比例改大小时宽高比**锁死**为目标比例（`ASPECT_VALUES[aspect]`）；
   *   自由比例不锁，鼠标到哪就是哪。
   * - 框**限制在「视口 ∩ 图像可见范围」内**（用户要求不许拖出可见区；
   *   再叠加图像范围，是为保证框内始终有内容、所见即所得）；
   * - 平移时尺寸不变，只挪位置，同样不越界。
   *
   * 全程**画面不动**：只改 `frameRect`；`crop` 由派生 effect 统一维护。
   */
  const onFrameMouseDown = useCallback(
    (mode: FrameDragMode) => (e: React.MouseEvent) => {
      if (e.button !== 0) return
      // 阻止冒泡，避免画布上的其他按下逻辑插手
      e.stopPropagation()
      e.preventDefault()
      const el = containerRef.current
      const v = viewRef.current
      if (!el || !image) return

      const cw = el.clientWidth
      const ch = el.clientHeight
      if (cw === 0 || ch === 0) return

      /**
       * 自由比例不锁宽高比；固定比例锁死为所选比例。
       *
       * ⚠️ 这里传的**就是目标比例本身**（不是「屏幕比」）—— 因为 `crop` 与屏幕
       * 同处「旋转后的图」这一个坐标系，两者宽高比恒等。见
       * [`constrainRectToRatio`] 的说明（旧版 90° 时传 `(H/W)²/target`，
       * 那是坐标系没统一时的补丁，会让旋转后拖框比例跳变）。
       */
      const lockRatio = v.aspect === 'free' ? null : ASPECT_VALUES[v.aspect]

      /** 当前框：拖过就是它，没拖过就是预置框（两种模式都有） */
      const startFrame = activeFrameRect(v.frameRect, v.aspect, cw, ch, v, image)
      if (!startFrame) return
      const rect = el.getBoundingClientRect()
      const px = e.clientX - rect.left
      const py = e.clientY - rect.top

      /**
       * 框的**可行域** = 视口 ∩ 图像可见矩形（就是 [`imageVisibleRect`]）。
       *
       * ⚠️ 光限制视口不够（2026-10-08 补）：画面可能是「按预置框 cover」的，
       * 横向图选 1:1 时图像往往只占视口中间一条，两侧留背景。若允许框拖到
       * 背景区，框内就是空的；而且 `rectToCrop` 会把结果 clamp 回图像范围 ——
       * 于是「看到的框」≠「裁出来的区域」，违背所见即所得。
       * 夹到图像范围后，框内**始终有图像内容**。
       *
       * （图像比视口大时交集就是视口，与「限制在视口内」等价。）
       */
      const vr = imageVisibleRect(
        v.tx,
        v.ty,
        v.scale,
        v.rotation + v.fineAngle,
        image.width,
        image.height,
        cw,
        ch,
      )
      const bounds = {
        left: vr.x * cw,
        top: vr.y * ch,
        right: (vr.x + vr.w) * cw,
        bottom: (vr.y + vr.h) * ch,
      }

      setFrameDragging(mode)

      const onMove = (ev: MouseEvent) => {
        const r2 = containerRef.current?.getBoundingClientRect()
        if (!r2) return
        const mx = ev.clientX - r2.left
        const my = ev.clientY - r2.top
        // 屏幕像素的框（拖动期间实时算，不依赖 state）
        const box = frameRectToPixels(startFrame, cw, ch)
        let next: FrameRect

        if (mode.kind === 'move') {
          // 平移：整体挪，尺寸不变
          const nx = box.left + (mx - px)
          const ny = box.top + (my - py)
          next = {
            x: Math.min(Math.max(nx, bounds.left), bounds.right - box.width) / cw,
            y: Math.min(Math.max(ny, bounds.top), bounds.bottom - box.height) / ch,
            w: box.width / cw,
            h: box.height / ch,
          }
        } else {
          // 改大小：抓的是哪个角，就固定它**对角**那个顶点不动
          const anchors = {
            se: { ax: box.left, ay: box.top },
            sw: { ax: box.left + box.width, ay: box.top },
            ne: { ax: box.left, ay: box.top + box.height },
            nw: { ax: box.left + box.width, ay: box.top + box.height },
          } as const
          const { ax, ay } = anchors[mode.corner]
          // 鼠标作为「动的那个角」：
          // - 固定比例 → 按目标比例约束，宽高比锁死；
          // - 自由比例 → 鼠标到哪就是哪，不约束。
          let left: number
          let top: number
          let w: number
          let h: number
          if (lockRatio === null) {
            left = Math.min(ax, mx)
            top = Math.min(ay, my)
            w = Math.abs(mx - ax)
            h = Math.abs(my - ay)
          } else {
            const r = constrainRectToRatio(ax, ay, mx, my, lockRatio)
            left = Math.min(r.x1, r.x2)
            top = Math.min(r.y1, r.y2)
            w = Math.abs(r.x2 - r.x1)
            h = Math.abs(r.y2 - r.y1)
          }

          // 限制在可行域内：装不下就整体缩到能装下（保持比例，锚点就近）
          const maxW = mode.corner === 'nw' || mode.corner === 'sw' ? ax - bounds.left : bounds.right - ax
          const maxH = mode.corner === 'nw' || mode.corner === 'ne' ? ay - bounds.top : bounds.bottom - ay
          const fit = Math.max(0, Math.min(maxW / w, maxH / h, 1))
          w *= fit
          h *= fit
          if (mode.corner === 'nw' || mode.corner === 'sw') left = ax - w
          if (mode.corner === 'nw' || mode.corner === 'ne') top = ay - h
          // 夹进可行域（浮点误差兜底）
          left = Math.min(Math.max(left, bounds.left), bounds.right - w)
          top = Math.min(Math.max(top, bounds.top), bounds.bottom - h)

          next = { x: left / cw, y: top / ch, w: w / cw, h: h / ch }
        }

        // 实时更新 frameRect 供渲染；`crop` 由派生 effect 统一维护
        setView((prev) => ({ ...prev, frameRect: next }))
      }

      const onUp = () => {
        setFrameDragging(null)
        window.removeEventListener('mousemove', onMove)
        window.removeEventListener('mouseup', onUp)
        // ⚠️ 松手**什么都不做** —— 尤其别再调 `computeFitFor` 重算视图，
        //    那会让画面「一松手就缩放」（用户明确报过的 bug）。
        //    只点击、没拖动时 `frameRect` 仍是 `null`，于是「没拖过 = 不裁剪」
        //    的语义得以保持；`crop` 全权交给派生 effect 维护。
      }

      window.addEventListener('mousemove', onMove)
      window.addEventListener('mouseup', onUp)
    },
    [image],
  )

  /**
   * `crop` 由「框 + 当前视图」**派生**（不要在拖拽时手写）。
   *
   * ## 规则（2026-10-08 定稿，两种模式统一）
   * - **没拖过框（`frameRect === null`）→ `crop = null` = 不裁剪。**
   *   这样「只放大看细节、没动过框」不会把可见区域误存成裁剪区；
   *   固定比例下后端拿到 `crop = null` 会按 `aspect` 居中裁剪，
   *   而居中的预置框正好就是那个范围 —— 两者一致。
   * - **拖过框 → `crop` = 框内那片图像内容**（用当前视图反变换）。
   *   视图变化（缩放/平移/旋转/窗口 resize）会改变框内看到的内容，
   *   所以这里跟着重算，保证「框里看到的」= 「将被裁掉的」。
   *
   * ⚠️ 不要再改成「拖拽时手写 crop」：那样视图一变 crop 就过期了。
   */
  useEffect(() => {
    if (!image) return
    const el = containerRef.current
    if (!el) return
    const cw = el.clientWidth
    const ch = el.clientHeight
    if (cw === 0 || ch === 0) return

    setView((v) => {
      if (!v.frameRect) return v.crop === null ? v : { ...v, crop: null }
      const box = frameRectToPixels(v.frameRect, cw, ch)
      // 归一化基准是**旋转后的图**（后端先旋转再裁剪），所以传旋转后尺寸 +
      // 残余的细调角，而不是原图尺寸 + 总角度。详见 `screenToImageNorm`。
      const { w: rotW, h: rotH } = rotatedImageSize(
        image.width,
        image.height,
        Math.round(v.rotation / 90),
      )
      const next = rectToCrop(
        box.left,
        box.top,
        box.left + box.width,
        box.top + box.height,
        v.tx,
        v.ty,
        v.scale,
        v.fineAngle,
        rotW,
        rotH,
      )
      if (sameCrop(v.crop, next)) return v
      return { ...v, crop: next }
    })
  }, [
    image,
    view.frameRect,
    view.tx,
    view.ty,
    view.scale,
    view.rotation,
    view.fineAngle,
    containerSize,
  ])

  /**
   * 图片在屏幕上是否**超出容器**（任一方向有内容被裁掉）。
   *
   * 只有超出时才允许拖拽平移 —— 适应窗口状态下图片完整可见，能拖反而会让它
   * 偏离中心，观感像「图跑偏了」。这和 Windows 照片 / FastStone 的行为一致。
   */
  const canPan = useMemo(
    () =>
      !!image &&
      canPanImage(
        image.width,
        image.height,
        view.rotation,
        view.scale,
        containerSize.w,
        containerSize.h,
      ),
    [image, view.rotation, view.scale, containerSize],
  )

  /**
   * 左键按下 → 开始拖拽平移。
   *
   * ⚠️ 只在**非裁剪语境**绑定：裁剪时左键归裁剪框（拖内部平移框、拖四角改大小），
   * 记忆里有硬规定「裁剪全程画面不动」。这个判断由 `ImageCanvas` 做，
   * 因为 `cropUi` 是它的 prop，hook 里拿不到。
   *
   * ⚠️ 必须在 `mousedown` 里就 `preventDefault()`：否则 WebView2 会把这次拖拽
   * 当成「拖拽图片」的原生行为（拖出一个半透明的图，鼠标松开就没了）。
   */
  const startPan = useCallback(
    (e: React.MouseEvent) => {
      if (!canPan || e.button !== 0) return
      e.preventDefault()
      dragState.current = { lastX: e.clientX, lastY: e.clientY }
      setDragging(true)
    },
    [canPan],
  )

  // 拖拽平移（rAF 节流）
  useEffect(() => {
    if (!dragging) return

    const flush = () => {
      rafId.current = null
      const d = pendingDelta.current
      pendingDelta.current = null
      if (!d) return
      setView((v) => ({ ...v, tx: v.tx + d.dx, ty: v.ty + d.dy }))
    }

    const handleMove = (e: MouseEvent) => {
      const s = dragState.current
      if (!s) return
      const dx = e.clientX - s.lastX
      const dy = e.clientY - s.lastY
      s.lastX = e.clientX
      s.lastY = e.clientY
      if (dx === 0 && dy === 0) return

      if (pendingDelta.current) {
        pendingDelta.current.dx += dx
        pendingDelta.current.dy += dy
      } else {
        pendingDelta.current = { dx, dy }
      }
      if (rafId.current === null) {
        rafId.current = requestAnimationFrame(flush)
      }
    }

    const handleUp = () => {
      setDragging(false)
      dragState.current = null
      pendingDelta.current = null
      if (rafId.current !== null) {
        cancelAnimationFrame(rafId.current)
        rafId.current = null
      }
    }

    window.addEventListener('mousemove', handleMove)
    window.addEventListener('mouseup', handleUp)
    return () => {
      window.removeEventListener('mousemove', handleMove)
      window.removeEventListener('mouseup', handleUp)
      if (rafId.current !== null) cancelAnimationFrame(rafId.current)
    }
  }, [dragging])

  /** 双击切换 100% / 适应窗口 */
  const onDoubleClick = useCallback(() => {
    const el = containerRef.current
    if (!el || !image) return
    const rect = el.getBoundingClientRect()

    setView((v) => {
      if (v.fit === 'none' && Math.abs(v.scale - 1) < 0.01) {
        const fit = computeFit('contain', rect.width, rect.height)
        return fit ? { ...v, ...fit, fit: 'contain' } : v
      }
      // 从适应状态到 100%，并居中
      return {
        ...v,
        scale: 1,
        tx: rect.width / 2,
        ty: rect.height / 2,
        fit: 'none',
      }
    })
  }, [image, computeFit])

  /** 直接设定缩放（以视口中心为锚点） */
  const setZoom = useCallback((scale: number) => {
    const el = containerRef.current
    if (!el) return
    const rect = el.getBoundingClientRect()
    const cx = rect.width / 2
    const cy = rect.height / 2
    const clamped = Math.min(MAX_SCALE, Math.max(MIN_SCALE, scale))

    setView((v) => {
      const ratio = clamped / v.scale
      return {
        ...v,
        scale: clamped,
        tx: cx - (cx - v.tx) * ratio,
        ty: cy - (cy - v.ty) * ratio,
        fit: 'none',
      }
    })
  }, [])

  /**
   * 旋转 90°。
   *
   * **总是重新按 `contain` 适应窗口**（2026-10-07 用户要求「旋转过后需要大小自适应」）：
   * 90° 会让宽高互换，若沿用原缩放，横图转竖后必然溢出容器、
   * 竖图转横后又会缩在中间很空。所以这里不看当前 `fit`，一律重算。
   *
   * **不改 `fineAngle`** —— 粗旋转与角度微调是两个独立维度，
   * 用户调好 ±10° 再点「左转」时，那 10° 应该保留。
   */
  const rotate = useCallback(
    (delta: number) => {
      setView((v) => {
        const newRotation = (((v.rotation + delta) % 360) + 360) % 360
        const el = containerRef.current
        if (!el) return { ...v, rotation: newRotation }
        const rect = el.getBoundingClientRect()
        const fit = computeFitFor(
          newRotation,
          'contain',
          image,
          rect.width,
          rect.height,
          v.fineAngle,
          v.aspect,
          v.crop,
          v.frameScale,
          v.frameRect,
        )
        return fit
          ? {
              ...v,
              rotation: newRotation,
              ...fit,
              // 适应后 fit 模式也应回到 contain，否则滚轮/窗口变化时行为不一致
              fit: 'contain',
            }
          : { ...v, rotation: newRotation }
      })
    },
    [image],
  )

  /** 设定细调角度（夹到 ±45°），适应模式下重算缩放 */
  const setFineAngle = useCallback((deg: number) => {
    const clamped = Math.min(MAX_FINE_ANGLE, Math.max(MIN_FINE_ANGLE, deg))
    setView((v) => {
      const el = containerRef.current
      if (!el) return { ...v, fineAngle: clamped }
      const rect = el.getBoundingClientRect()
      const fit = computeFitFor(
        v.rotation,
        v.fit,
        image,
        rect.width,
        rect.height,
        clamped,
        v.aspect,
        v.crop,
        v.frameScale,
        v.frameRect,
      )
      return fit
        ? { ...v, fineAngle: clamped, scale: fit.scale, tx: fit.tx, ty: fit.ty }
        : { ...v, fineAngle: clamped }
    })
  }, [image])

  /**
   * 设定裁剪比例：**只换框，画面一动不动**。
   *
   * ## 为什么不再重算视图（2026-10-11 用户要求）
   * 旧实现会跑一遍 `computeFitFor(..., aspect, ...)` 把 scale/tx/ty 推成
   * 「按新比例的框 cover 整张图」—— 效果是**选个比例画面就自己缩放/平移**。
   * 用户的原话是「选取了比例后，画面还是保持原来的大小，不需要调整适应」：
   * 他正在取景，换个比例框不该把他的构图挪走。
   *
   * 所以现在只做三件事：换 `aspect`、清掉旧框（回到预置框）、`frameScale` 归 1。
   * 预置框落在**画面上图像占的区域**内（见 [`activeFrameRect`]），
   * 因此不需要靠移动画面来让框有意义。
   *
   * ⚠️ 清 `frameRect`/`crop` 是必须的：旧框的比例与大小对新比例不再适用。
   * 清掉后 `frameRect === null` → 走预置框 → 也等于「没有裁剪」这条既有契约。
   */
  const setAspect = useCallback((aspect: AspectRatio) => {
    setView((v) => ({
      ...v,
      aspect,
      crop: null as CropRect | null,
      frameRect: null as FrameRect | null,
      frameScale: 1,
    }))
  }, [])

  /**
   * 裁剪框的屏幕矩形（容器 CSS 像素）；**两种模式都有**。
   *
   * ⚠️ 必须在**渲染前**同步算出来（`ImageCanvas` 直接拿它定位框），
   * 所以用 `containerSize`（由渲染帧写入的容器尺寸镜像）而非读 DOM。
   * 尺寸未就绪 / 无图时返回 `null`。
   *
   * 走 [`activeFrameRect`] —— 与拖拽用的是同一个入口，
   * 保证「画面上那个框」就是「正在拖的框」。
   * 注意自由模式的预置框依赖当前视图（图像可见矩形），
   * 所以依赖里必须带上 `view` 的几何字段。
   */
  const frameBox = useMemo(() => {
    const { w: cw, h: ch } = containerSize
    if (cw === 0 || ch === 0) return null
    const fr = activeFrameRect(view.frameRect, view.aspect, cw, ch, view, image)
    return fr ? frameRectToPixels(fr, cw, ch) : null
  }, [view, containerSize, image])

  /**
   * 清掉裁剪框（不动角度与比例），配合「清除」「复位」与切换比例使用。
   *
   * 清的是 `frameRect`（回到预置框）—— 两种模式都这样。
   * `crop` 不必手写：没拖过框时派生 effect 会把它置回 `null`（= 不裁剪）。
   * 清完视图退回「适应」，否则会停留在只显示框内那一块的构图里。
   */
  const clearCrop = useCallback(() => {
    const el = containerRef.current
    const rect = el?.getBoundingClientRect()
    setView((v) => {
      if (v.crop === null && v.frameRect === null) return v
      const fit = rect
        ? computeFitFor(
            v.rotation,
            v.fit,
            image,
            rect.width,
            rect.height,
            v.fineAngle,
            v.aspect,
            null,
            v.frameScale,
            null,
          )
        : null
      const cleared = { ...v, crop: null, frameRect: null }
      return fit ? { ...cleared, ...fit, fit: 'contain' } : cleared
    })
  }, [image])

  /**
   * 把旋转、角度、比例与框选一并复位。
   *
   * ⚠️ **必须清 `rotation`**（2026-10-07 修复）：此前只清 `fineAngle`，
   * 于是「顺时针 90°」之后点「复位」画面纹丝不动 —— 用户报的 bug。
   * 复位后按 `contain` 重算适应，让画面的确回到刚打开这张图的样子。
   */
  const resetAdjust = useCallback(() => {
    const el = containerRef.current
    const rect = el?.getBoundingClientRect()
    setView((v) => {
      const base = {
        ...v,
        rotation: 0,
        fineAngle: 0,
        aspect: 'free' as AspectRatio,
        frameScale: 1,
        frameRect: null,
        crop: null,
      }
      if (!rect) return base
      const fit = computeFitFor(0, 'contain', image, rect.width, rect.height, 0, 'free', null, 1, null)
      return fit ? { ...base, ...fit, fit: 'contain' } : base
    })
  }, [image])

  /**
   * 取当前视图的深拷贝快照。`crop` 是对象，必须一并拷，
   * 否则快照会与实时状态共享同一引用。
   */
  const snapshotView = useCallback((): ViewState => {
    const v = viewRef.current
    return { ...v, crop: v.crop ? { ...v.crop } : null }
  }, [])

  /**
   * 用快照还原视图。
   *
   * ⚠️ **不重算 fit**：快照里的 `scale/tx/ty` 就是当时屏幕上真实的
   * 缩放与位置，直接覆盖才是「恢复到调整前的显示」。若这里再跑一遍
   * `computeFitFor`，容器尺寸没变时结果相同，但一旦用户在此期间
   * 改过窗口大小，就会把用户当时的构图挪走，与诉求不符。
   * 选区的屏幕矩形由「跟随 crop 重算」的 effect 自动同步。
   */
  const restoreView = useCallback((snap: ViewState) => {
    setView({ ...snap, crop: snap.crop ? { ...snap.crop } : null })
  }, [])

  // 响应工具栏/快捷键派发的 `pv-action` 自定义事件
  useEffect(() => {
    const el = containerRef.current
    if (!el) return
    const handler = (e: CustomEvent) => {
      const detail = (e as CustomEvent<{ action: string }>).detail
      const rect = el.getBoundingClientRect()
      switch (detail.action) {
        case 'fit':
          setView((v) => {
            // 适应时保留当前角度与比例（用户调好的构图不该被「适应」抹掉）；
            // 有框选时按框选区域适应，与松手那一刻保持一致
            const fit = computeFitFor(
              v.rotation,
              'contain',
              image,
              rect.width,
              rect.height,
              v.fineAngle,
              v.aspect,
              v.crop,
              v.frameScale,
              v.frameRect,
            )
            return fit ? { ...v, ...fit, fit: 'contain' } : v
          })
          break
        case 'actual': // 100%
          setView((v) =>
            image
              ? { ...v, scale: 1, tx: rect.width / 2, ty: rect.height / 2, fit: 'none' }
              : v,
          )
          break
        case 'zoom-in':
          setView((v) => ({ ...v, scale: Math.min(MAX_SCALE, v.scale * BUTTON_ZOOM_STEP), fit: 'none' }))
          break
        case 'zoom-out':
          setView((v) => ({ ...v, scale: Math.max(MIN_SCALE, v.scale / BUTTON_ZOOM_STEP), fit: 'none' }))
          break
        case 'rotate-left':
          rotate(-90)
          break
        case 'rotate-right':
          rotate(90)
          break
        case 'pan-left':
          setView((v) => ({ ...v, tx: v.tx + PAN_STEP }))
          break
        case 'pan-right':
          setView((v) => ({ ...v, tx: v.tx - PAN_STEP }))
          break
        case 'pan-up':
          setView((v) => ({ ...v, ty: v.ty + PAN_STEP }))
          break
        case 'pan-down':
          setView((v) => ({ ...v, ty: v.ty - PAN_STEP }))
          break
      }
    }
    el.addEventListener('pv-action', handler as EventListener)
    return () => el.removeEventListener('pv-action', handler as EventListener)
  }, [image, computeFit, rotate])

  return {
    view,
    dragging,
    canPan,
    startPan,
    frameBox,
    frameDragging,
    onFrameMouseDown,
    isPixelated: view.scale >= 1.5,
    containerRef,
    onWheel,
    onDoubleClick,
    setZoom,
    setFineAngle,
    setAspect,
    clearCrop,
    resetAdjust,
    snapshotView,
    restoreView,
  }
}
