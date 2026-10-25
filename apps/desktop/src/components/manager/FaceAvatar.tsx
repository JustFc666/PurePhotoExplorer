/**
 * 人脸头像：从缩略图里**按归一化框裁出正方形**，画进 canvas。
 *
 * ## 为什么在前端裁而不是后端
 * 缩略图本来就是前端拿的（`manager_thumbnail` 走 PVI1 裸字节），
 * 加一个后端"裁头像"命令等于多一次解码 + 多一条 IPC；这里用 canvas 裁，
 * 成本接近 0，而且能顺手做 80×80 的缩放。
 *
 * ## 关键细节
 * - 人脸框是**归一化坐标**（相对检测时那一档，0..1），与缩略图分辨率无关，
 *   直接乘缩略图的宽高即可 —— 不要按原图宽高换算。
 * - 裁的时候**四周留 12% 边距**：检测框通常只框到眉毛到下巴，贴边裁会把额头/下巴切掉。
 * - 框可能超出图片边界（靠边的脸），所以源矩形要 clamp 到图内，
 *   再按"实际裁到的比例"补边（否则会出现拉伸变形）。
 */

import { useEffect, useRef } from 'react'
import { managerThumbnail } from '../../ipc/manager'
import type { FaceBox } from '../../ipc/people'

interface Props {
  face: FaceBox | null
  /** 输出边长（px），默认 80 */
  size?: number
  className?: string
  /** 显示用的文字兜底（没有 face 时） */
  fallback?: string
}

/**
 * 裁剪时四周留白比例（相对脸框边长）。
 *
 * 2026-10-25 从 0.12 收到 0.06：YuNet 的框本身已经含额头和下巴，
 * 再外扩 12% 等于把脸缩小一圈 —— 同样的源像素被摊到更多背景上，看起来更糊。
 * 留 6% 只是防止边缘贴太紧。
 */
const PAD = 0.06

/**
 * 裁好的头像缓存（key = 脸 id + 输出尺寸）。
 *
 * 不进缓存的话：进一次人像专栏、滚一次列表，同一张脸会被反复解码
 * （`manager_thumbnail` 每次都走一次后端解码），39 人就能卡出明显的白屏。
 */
const cropCache = new Map<string, HTMLCanvasElement>()

export function FaceAvatar({ face, size = 80, className, fallback }: Props) {
  const ref = useRef<HTMLCanvasElement | null>(null)

  useEffect(() => {
    const canvas = ref.current
    if (!canvas) return
    if (!face) return

    let cancelled = false
    /**
     * 档位按**显示尺寸**选：脸在原图宽约 180px，缩到 1024 长边只剩约 90px，
     * 再画成 80px 就是 1:1 上下 —— 一旦 dpr>1 就必须放大，看着就糊。
     * 所以 ≥80px 的头像改取 `xl`（2048 长边，脸约 180px），源够用；
     * 小于 80 的（浮层 56）用 `lg` 足够，还省一次大图解码。
     */
    const tier: 'lg' | 'xl' = size >= 80 ? 'xl' : 'lg'
    const key = `${face.id}:${size}:${tier}`
    const hit = cropCache.get(key)
    if (hit) {
      const dpr0 = window.devicePixelRatio || 1
      canvas.width = Math.round(size * dpr0)
      canvas.height = Math.round(size * dpr0)
      const c0 = canvas.getContext('2d')
      if (c0) c0.drawImage(hit, 0, 0)
      return
    }
    void (async () => {
      try {
        const img = await managerThumbnail(face.imageId, tier)
        if (cancelled) return
        const src = document.createElement('canvas')
        src.width = img.width
        src.height = img.height
        const sctx = src.getContext('2d')
        if (!sctx) return
        /* DecodedImage 的像素是 RGBA，先铺到离屏 canvas 上才有 drawImage 的源 */
        const data = new ImageData(new Uint8ClampedArray(img.data), img.width, img.height)
        sctx.putImageData(data, 0, 0)

        const W = img.width
        const H = img.height
        /* 归一化框 → 像素框，再按 PAD 外扩 */
        const bw = face.w * W
        const bh = face.h * H
        const pad = Math.max(bw, bh) * PAD
        let sx = face.x * W - pad / 2
        let sy = face.y * H - pad / 2
        let sw = bw + pad
        let sh = bh + pad

        /*
         * 贴边处理：**保持中心不变，缩小边长**，而不是平移。
         * ⛔ 之前的写法是 `clamp(sx, 0, W-side)` —— 脸靠边时会被整体推走，
         *    裁出来就是"脸在左上角、其余全是背景"（2026-10-25 用户报的截脸问题）。
         * 正确做法：先取中心，再把边长缩到"以该中心能完整放进图内"的最大值。
         */
        const cx = sx + sw / 2
        const cy = sy + sh / 2
        const side = Math.max(
          1,
          Math.min(Math.max(sw, sh), 2 * Math.min(cx, W - cx), 2 * Math.min(cy, H - cy), W, H),
        )
        sx = cx - side / 2
        sy = cy - side / 2
        sw = side
        sh = side

        const dpr = window.devicePixelRatio || 1
        canvas.width = Math.round(size * dpr)
        canvas.height = Math.round(size * dpr)
        const ctx = canvas.getContext('2d')
        if (!ctx) return
        ctx.imageSmoothingQuality = 'high'
        ctx.drawImage(src, sx, sy, sw, sh, 0, 0, canvas.width, canvas.height)
        /* 存一份裁剪结果（尺寸很小，39 人也就几百 KB） */
        const keep = document.createElement('canvas')
        keep.width = canvas.width
        keep.height = canvas.height
        keep.getContext('2d')?.drawImage(canvas, 0, 0)
        cropCache.set(key, keep)
      } catch {
        /* 缩略图取不到（卷离线/解码失败）→ 保留占位文字，不抛 */
      }
    })()

    return () => {
      cancelled = true
    }
  }, [face, size])

  return (
    <span className={`pv-face${className ? ` ${className}` : ''}`} style={{ width: size, height: size }}>
      {face ? (
        <canvas ref={ref} className="pv-face-canvas" style={{ width: size, height: size }} />
      ) : (
        <span className="pv-face-fallback">{fallback ?? ''}</span>
      )}
    </span>
  )
}
