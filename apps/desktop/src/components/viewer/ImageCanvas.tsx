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
import type { DecodedImage } from '../../ipc'

interface ImageCanvasProps {
  image: DecodedImage | null
  /** 视图控制器（状态 + 事件处理器），由父级用 `useImageView` 创建 */
  controller: UseImageView
}

/** 四个角的拖拽模式与光标（模块级常量，避免每次渲染重建） */
const CORNERS: { corner: 'nw' | 'ne' | 'sw' | 'se'; mode: FrameDragMode }[] = [
  { corner: 'nw', mode: { kind: 'resize', corner: 'nw' } },
  { corner: 'ne', mode: { kind: 'resize', corner: 'ne' } },
  { corner: 'sw', mode: { kind: 'resize', corner: 'sw' } },
  { corner: 'se', mode: { kind: 'resize', corner: 'se' } },
]

export function ImageCanvas({ image, controller }: ImageCanvasProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const {
    view,
    dragging,
    frameBox,
    frameDragging,
    onFrameMouseDown,
    isPixelated,
    containerRef,
    onWheel,
    onDoubleClick,
  } = controller

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

      {/* **裁剪框**（两种模式共用）：画面上的框本身就是生效的裁剪范围。
          拖四角改大小（固定比例锁比例）、拖内部平移；框外压暗 = 框内高亮。 */}
      {frameBox && (
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
