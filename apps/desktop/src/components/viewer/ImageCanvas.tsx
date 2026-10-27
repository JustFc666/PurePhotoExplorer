/**
 * 图片画布 —— S1 单图浏览的核心**渲染**组件。
 *
 * 交互逻辑与视图状态由父级通过 [`UseImageView`](../../hooks/useImageView.ts)
 * 注入（`controller`），本组件只做：
 * 1. 把 `containerRef` 绑到画布容器（供 hook 量取尺寸/挂事件）；
 * 2. 用 `putImageData` 把像素写入 canvas（1:1）；
 * 3. 用 CSS `transform` 呈现缩放/平移/旋转。
 *
 * **变换公式**（transform-origin 为 0 0）：
 * `translate(tx,ty) rotate(r) scale(s) translate(-w/2,-h/2)`
 * 即 `tx/ty` 是**图像中心**在屏幕中的位置，故旋转/缩放天然绕中心进行。
 *
 * **为什么用 `putImageData` 而不是 `createImageBitmap` / `drawImage`**：
 * Tauri 2 / WebView2 上 `createImageBitmap(ImageData)` 会在某些图片/尺寸下
 * 于原生层抛出 `Maximum call stack size exceeded`（C++ 递归）。`putImageData` 无此坑。
 *
 * ## 裁剪的实现（2026-10-08 统一，两种模式同一套交互）
 *
 * 画面上**始终有一个裁剪框**：
 * - **自由比例** → 预置框 = 图像可见矩形（初始就是整图，等于「不裁剪」）；
 * - **固定比例** → 预置框 = 该比例在视口内的最大矩形（居中）。
 *
 * 用户操作：
 * - 拖**四角** → 改大小（固定比例**锁死宽高比**，自由比例不锁）；
 * - 拖**框内部** → 平移整个框（尺寸不变）。
 *
 * 两条硬约束：框**限制在「视口 ∩ 图像可见范围」内**（不许拖出可见区、
 * 也不许拖到没有像素的背景上），且全程**画面不动** —— 只改框。
 *
 * 框的位置来自归一化「视口坐标」（`view.frameRect`）；
 * **没拖过框时它保持 `null`**（此时不产生 `crop` = 不裁剪），
 * 拖过之后由派生 effect 反变换成图像归一化 `crop` 供落盘。
 */

import { useEffect, useRef } from 'react'
import type { FrameDragMode, UseImageView } from '../../hooks/useImageView'
import type { SlideFx } from '../../hooks/useSlideshow'
import type { DecodedImage } from '../../ipc'

interface ImageCanvasProps {
  image: DecodedImage | null
  /** 视图控制器（状态 + 事件处理器），由父级用 `useImageView` 创建 */
  controller: UseImageView
  /**
   * 放映切换特效（`null` = 不在放映，不做任何动画）。
   *
   * 用 **Web Animations API** 而不是 CSS class/keyframe：
   * 画布的缩放/平移是**内联 transform**（`tx/ty/rotate/scale`），
   * 用 class 触发动画会跟它抢同一套属性；而且换图等于换 `key`，
   * 重挂载 canvas 会把画布尺寸和当前视图状态一起丢掉。
   * `el.animate()` 只临时叠加一层合成动画，结束后原样归还，最干净。
   */
  fx?: SlideFx | null
  /** 换图标识（图片路径）。变了就放一次特效 */
  fxKey?: string | null
  /** 特效时长（毫秒，两种特效共用；设置面板可调） */
  fxMs?: number
  /**
   * 是否显示裁剪框（用户 2026-10-19：普通浏览时画面上的**白框**要去掉）。
   *
   * 传 `true` = 已经进入裁剪语境（调整浮层打开），或者图上有真实裁剪
   * （`aspect !== 'free'` / 拖过框）—— 后两种由 `controller.view` 自己判断，
   * 这里只接「调整浮层开关」。
   */
  cropUi?: boolean
}

/** 四个角的拖拽模式与光标（模块级常量，避免每次渲染重建） */
const CORNERS: { corner: 'nw' | 'ne' | 'sw' | 'se'; mode: FrameDragMode }[] = [
  { corner: 'nw', mode: { kind: 'resize', corner: 'nw' } },
  { corner: 'ne', mode: { kind: 'resize', corner: 'ne' } },
  { corner: 'sw', mode: { kind: 'resize', corner: 'sw' } },
  { corner: 'se', mode: { kind: 'resize', corner: 'se' } },
]

export function ImageCanvas({
  image,
  controller,
  fx = null,
  fxKey = null,
  fxMs = 300,
  cropUi = false,
}: ImageCanvasProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  /**
   * 本次切换的「幻影」元素（放上一张的画面，用来做滑出/淡出）。
   *
   * ⚠️ **每次切换新建一个、用完就 remove**，不再是常驻的 `<canvas ref>`。
   * 常驻版本踩过一个很脏的坑（2026-10-19 用户报"放映时画面中间糊着另一张长图"）：
   * 动画被 `cancel()` 之后元素会**回到基础样式**（opacity 1 / 无位移），
   * 于是上一张的整幅画面就永远糊在屏幕上，直到下一次换图覆盖它。
   */
  const ghostElRef = useRef<HTMLCanvasElement | null>(null)
  /** 上一次拿到的像素对象（区分「换图」与「GIF 换帧」） */
  const lastImage = useRef<DecodedImage | null>(null)
  /**
   * 「换了一张图，等新像素落地后要播特效」。
   *
   * ⚠️ 必须有这个标记：动图每帧都会换 `image`，只凭 `image` 变化就播特效的话，
   * GIF 播放时每帧都会"滑一下"，看起来像卡顿。
   */
  const pendingFx = useRef(false)

  const {
    view,
    dragging,
    canPan,
    startPan,
    frameBox,
    frameDragging,
    onFrameMouseDown,
    isPixelated,
    containerRef,
    onWheel,
    onDoubleClick,
  } = controller

  /**
   * 是否处于**裁剪语境**：这时左键归裁剪框（拖内部平移、拖四角改大小），
   * 画面必须保持不动 —— 记忆里的硬规定「裁剪全程画面不动，松手也不动」。
   * 这个判断得在这里做，`cropUi` 是本组件的 prop，hook 里看不到。
   */
  const cropping = cropUi || view.aspect !== 'free' || view.frameRect !== null

  /**
   * `view.scale` 用 ref 读，**不能进 effect 依赖**。
   *
   * ⚠️ 这是上面那个坑的真正触发点：新图解码完会重新"适应窗口"，
   * `view.scale` 在动画跑到一半时变掉 → effect 重跑 → cleanup 取消动画 →
   * 幻影回到 opacity 1 卡在画面上。动画时长只有几百毫秒，读到哪一帧的
   * 缩放都不影响观感，所以快照一次就够了。
   */
  const scaleRef = useRef(view.scale)
  scaleRef.current = view.scale

  /**
   * ① 换图**之前**把旧画面拷到幻影元素。
   *
   * 为什么是「换图之前」：`fxKey`（路径）比新像素早一步更新 —— 解码还要几十毫秒，
   * 这段时间 canvas 上仍是上一张。所以在这个 effect 里 `drawImage` 拿到的就是旧图。
   *
   * 用 `drawImage`（GPU 拷贝，几毫秒）而不是 `toDataURL`：
   * 6000×4000 的画布编码成 base64 要几百毫秒，正好卡在换图那一刻。
   */
  useEffect(() => {
    // 清掉上一次没用掉的幻影（解码失败时 ② 不会跑，不能留着它糊在画面上）
    ghostElRef.current?.remove()
    ghostElRef.current = null
    pendingFx.current = false
    if (!fxKey || !fx || fx === 'none') return
    // 换了图：等新像素到（effect ②）就播一次
    pendingFx.current = true

    const src = canvasRef.current
    const box = src?.parentElement
    if (!src || !box || src.width === 0 || src.height === 0) return
    const ghost = document.createElement('canvas')
    ghost.className = 'pv-canvas-img pv-slide-ghost'
    ghost.setAttribute('aria-hidden', 'true')
    ghost.width = src.width
    ghost.height = src.height
    const ctx = ghost.getContext('2d')
    if (!ctx) return
    ctx.drawImage(src, 0, 0)
    // 幻影要摆在和主画面**一模一样**的位置（旧图的缩放/平移可能跟新图不同）
    ghost.style.transform = src.style.transform
    box.appendChild(ghost)
    ghostElRef.current = ghost

    // 兜底：万一新图一直没来（解码失败），3 秒后自己消失
    const t = window.setTimeout(() => {
      if (ghostElRef.current === ghost) {
        ghost.remove()
        ghostElRef.current = null
      }
    }, 3000)
    return () => window.clearTimeout(t)
  }, [fxKey, fx])

  /**
   * ② 新像素到了 → 播切换特效。
   *
   * 为什么不在 `fxKey` 变化时播：那时画布上还是**旧图**，淡入会把旧图淡进来一次，
   * 滑动会把旧图再滑进来一次（幻影同时在滑出，看起来是两张旧图）。
   * 所以等 `image` 真的换了（新图已画进 canvas）才开演。
   *
   * `lastImage` 用来把「换图」和「GIF 换帧」区分开：动图每帧都会换 `image`，
   * 但 `fxKey` 没变 → `pendingFx` 是 false → 直接跳过。
   */
  useEffect(() => {
    const changed = lastImage.current !== image
    lastImage.current = image
    if (!changed || !image || !fx || fx === 'none') return
    // 不是「换图」（动图换帧 / 同图重绘）→ 不播
    if (!pendingFx.current) return
    pendingFx.current = false

    const main = canvasRef.current
    if (!main) return
    /** 接管这一张的幻影：置空引用，别人（①/上一次 cleanup）就不会再动它 */
    const ghost = ghostElRef.current
    ghostElRef.current = null
    /**
     * 幻影播完就地摘掉。
     *
     * ⚠️⚠️ WAAPI 的 `fill` 默认是 `none`：动画**结束后元素会弹回基础样式**
     * （幻影的基础样式就是 opacity 1 / 无位移），于是上一张又完整地糊在屏幕上了 ——
     * 而且会一直糊到下一次换图的 cleanup 才被摘掉（2026-10-19 用户报的正是这个：
     * 长条图停在画面中间、新图只能从旁边露出来）。
     * 所以既要 `fill: 'forwards'`（停在结束帧 opacity 0），也要 `onfinish` 里 remove。
     */
    const finishGhost = () => ghost?.remove()
    const easing = 'cubic-bezier(0.22, 0.61, 0.36, 1)'
    const anims: Animation[] = []
    /*
     * ⚠️ 放到下一帧再开演：本组件的「像素 → canvas」effect 声明在本 effect **之后**，
     * 同一个 commit 里我这边先跑 —— 此刻 canvas 上还是旧像素。
     * `requestAnimationFrame` 在绘制前回调，正好等到新图已经写进 canvas。
     */
    const raf = requestAnimationFrame(() => {
      if (fx === 'fade') {
        /*
         * 淡入淡出 = **交叉淡变**：新图淡入的同时旧图（幻影）淡出。
         * 只淡入新图的话，旧图会"啪"地消失，看起来是闪一下而不是过渡。
         */
        anims.push(main.animate([{ opacity: 0 }, { opacity: 1 }], { duration: fxMs, easing }))
        if (ghost) {
          const g = ghost.animate([{ opacity: 1 }, { opacity: 0 }], {
            duration: fxMs,
            easing,
            fill: 'forwards',
          })
          g.onfinish = finishGhost
          anims.push(g)
        }
        return
      }
      /*
       * 滑动：新图**从右侧滑入**、旧图（幻影）**向左侧滑出**。
       *
       * ⚠️ 位移用 `translate` 属性而不是 `transform`：画布上的 `transform` 是
       * 缩放/平移/旋转的内联表达式，动它就把用户当前的视图状态改掉了。
       * `translate` 是独立属性，但**会被后面的 `scale()` 一起放大** ——
       * 所以按当前缩放反算一下，屏幕上才真的是 64px。
       */
      const scale = Math.max(0.01, scaleRef.current)
      const shift = 64 / scale
      anims.push(
        main.animate(
          [
            { opacity: 0.3, translate: `${shift}px 0px` },
            { opacity: 1, translate: '0px 0px' },
          ],
          { duration: fxMs, easing },
        ),
      )
      if (ghost) {
        const g = ghost.animate(
          [
            { opacity: 1, translate: '0px 0px' },
            { opacity: 0, translate: `-${shift}px 0px` },
          ],
          { duration: fxMs, easing, fill: 'forwards' },
        )
        g.onfinish = finishGhost
        anims.push(g)
      }
    })

    return () => {
      cancelAnimationFrame(raf)
      for (const a of anims) a.cancel()
      // 幻影播完（或被取消）必须**从 DOM 里摘掉**：留着它就是一个卡住的画面
      ghost?.remove()
    }
  }, [image, fx, fxMs])

  // 像素 → canvas（尺寸变化时重设画布分辨率）
  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas || !image) return

    if (canvas.width !== image.width || canvas.height !== image.height) {
      canvas.width = image.width
      canvas.height = image.height
    }

    const ctx = canvas.getContext('2d')
    if (!ctx) return
    // putImageData 是 1:1 像素写入，不受 CSS transform 影响
    const imgData = new ImageData(image.data, image.width, image.height)
    ctx.putImageData(imgData, 0, 0)
  }, [image])

  const halfW = image ? image.width / 2 : 0
  const halfH = image ? image.height / 2 : 0
  /** 视觉总角度 = 90° 粗旋转 + 细调角 */
  const totalAngle = view.rotation + view.fineAngle

  return (
    <div
      ref={containerRef}
      className="pv-canvas"
      data-dragging={dragging}
      data-cropping={view.aspect !== 'free'}
      /* 只有「能拖」时才给 grab 光标（CSS 靠它切换），否则画面明明没超出也显示抓手会误导 */
      data-draggable={canPan && !cropping}
      /*
       * 左键拖拽平移。裁剪语境下不绑 —— 那时左键要留给裁剪框。
       * ⛔ 别去掉 `startPan` 里的 preventDefault：WebView2 会把拖拽当成
       * 「拖拽图片」的原生行为（拖出半透明幽灵图）。
       */
      onMouseDown={cropping ? undefined : startPan}
      onWheel={onWheel}
      onDoubleClick={onDoubleClick}
      role="img"
      aria-label="图片查看区域"
    >
      <canvas
        ref={canvasRef}
        className="pv-canvas-img"
        style={{
          transform: `translate3d(${view.tx}px, ${view.ty}px, 0) rotate(${totalAngle}deg) scale(${view.scale}) translate3d(${-halfW}px, ${-halfH}px, 0)`,
          imageRendering: isPixelated ? 'pixelated' : 'auto',
        }}
      />

      {/*
        「幻影」层不写在这里 —— 它由放映特效的 effect **临时创建**并 append 到本容器，
        播完立即 remove（见文件上方 ① 的说明）。写成常驻节点会留下"卡住的画面"。
      */}

      {/* **裁剪框**（两种模式共用）：画面上的框本身就是生效的裁剪范围。
          拖四角改大小（固定比例锁比例）、拖内部平移；框外压暗 = 框内高亮。 */}
      {/*
        ⚠️ 框只在**裁剪语境**里出现（2026-10-19 用户反馈"单图浏览有白框"）：
        以前这个框无条件渲染 —— 它带 1px 白描边 + 四周 45% 压暗，
        于是每张照片看起来都被一个白框框住。
        现在只有三种情况才画：调整浮层打开 / 选了固定比例 / 已经拖过框。
        ⛔ 别改回无条件渲染。
      */}
      {frameBox && (cropUi || view.aspect !== 'free' || view.frameRect !== null) && (
        <div className="pv-crop-overlay" aria-hidden>
          <div
            className="pv-crop-frame"
            style={{
              left: `${frameBox.left}px`,
              top: `${frameBox.top}px`,
              width: `${frameBox.width}px`,
              height: `${frameBox.height}px`,
            }}
          />
          {/* 框内部的平移热区（拖整体挪位置） */}
          <div
            className="pv-crop-move"
            data-dragging={frameDragging?.kind === 'move'}
            style={{
              left: `${frameBox.left}px`,
              top: `${frameBox.top}px`,
              width: `${frameBox.width}px`,
              height: `${frameBox.height}px`,
            }}
            onMouseDown={onFrameMouseDown({ kind: 'move' })}
          />
          {/* 四个角的缩放热区 */}
          {CORNERS.map(({ corner, mode }) => (
            <div
              key={corner}
              className="pv-crop-handle"
              data-corner={corner}
              onMouseDown={onFrameMouseDown(mode)}
              style={
                corner === 'nw'
                  ? { left: `${frameBox.left}px`, top: `${frameBox.top}px` }
                  : corner === 'ne'
                    ? { left: `${frameBox.left + frameBox.width}px`, top: `${frameBox.top}px` }
                    : corner === 'sw'
                      ? { left: `${frameBox.left}px`, top: `${frameBox.top + frameBox.height}px` }
                      : {
                          left: `${frameBox.left + frameBox.width}px`,
                          top: `${frameBox.top + frameBox.height}px`,
                        }
              }
            />
          ))}
        </div>
      )}
    </div>
  )
}
