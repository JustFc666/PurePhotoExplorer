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
import { resampleMks } from '../../lib/mksResample'
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
 * ⭐ 2026-10-26 回退定稿：**恢复外扩 0.06**。
 * 试过内收 0.92（脸占满、等效清晰度 +33%），用户实测「效果不好」——
 * 切掉额头/下巴后看着像被裁坏的证件照，认人反而变差。
 * 现状：外扩 6% 保留完整头部轮廓，清晰度由封面脸尺寸（最大脸规则）保证。
 *
 * ⛔ 别再试 `FACE_FILL` 内收方案。
 */
const PAD = 0.06

/** 临时诊断：头像底部画 4px 色条（绿=走了 MKS，红=回退了）。定位完删掉。 */
const DIAG_STRIP = false

/**
 * 位图尺寸是否跟随屏幕 DPI。
 *
 * ⭐ 正确做法是 `true`：`outPx = size × devicePixelRatio`。
 *    100% 缩放的屏 → 位图 80；125% → 100；150% → 120。位图与设备像素始终 1:1，
 *    这才是**发行版适配不同机器**的做法；写死 80 反而是「只按 100% 缩放的那台机器」。
 *
 * ⚠️ 置为 `false` 时位图恒为 `size`（80），在 125%/150% 屏上会被浏览器放大 1.25/1.5 倍 → 糊。
 *    2026-10-27 实机对比过：false 版 lap_var 341，true 版 1127，肉眼一眼能看出差别。
 *    **结论：保持 true**。
 */
const BITMAP_FOLLOW_DPR = true

/** 位图边长：跟随 dpr 时按屏幕算，否则写死等于 CSS 尺寸。 */
function bitmapPx(size: number, dpr: number): number {
  return BITMAP_FOLLOW_DPR ? Math.max(1, Math.round(size * dpr)) : size
}

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

    /*
     * ⭐ 先把 canvas 清空（2026-10-29 修）。
     *
     * React 会复用同一个 <canvas> DOM 节点。人脸重跑后 `persons` 的 face_count
     * 全变、列表顺序大改，同一个位置换成了别的 person —— 而新封面要走
     * `managerThumbnail` 的异步解码，这段时间里 canvas 上**还留着上一个人的图**。
     * 如果缩略图一直取不到（卷离线/解码失败），残留就会**长期停留**，
     * 用户看到的就是「两个头像一模一样」「第一个头像错了」。
     *
     * ⛔ 不能只在异步成功后 putImageData —— 失败路径没有覆盖，残留会一直显着。
     */
    const ctxClear = canvas.getContext('2d')
    if (ctxClear) ctxClear.clearRect(0, 0, canvas.width, canvas.height)

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
      const px0 = bitmapPx(size, window.devicePixelRatio || 1)
      canvas.width = px0
      canvas.height = px0
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
        /* 归一化框 → 像素框，再按 PAD 外扩（回退版：保留完整头部轮廓） */
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

        const outPx = bitmapPx(size, window.devicePixelRatio || 1)
        canvas.width = outPx
        canvas.height = outPx
        const ctx = canvas.getContext('2d')
        if (!ctx) return

        /*
         * 缩放走 Magic Kernel Sharp 2013（见 `lib/mksResample.ts`）。
         * ⛔ 别换回 `ctx.drawImage` + `imageSmoothingQuality='high'`：Skia 的 high
         *    是双三次且缩小时不做预滤波，3:1 这种幅度会有明显锯齿
         *    （Skia issue #500117356）。
         * 走 MKS 时这条路是 3:1 左右，正好是它最擅长的区间。
         */
        const rx = Math.max(0, Math.round(sx))
        const ry = Math.max(0, Math.round(sy))
        const rw = Math.max(1, Math.min(W - rx, Math.round(sw)))
        const rh = Math.max(1, Math.min(H - ry, Math.round(sh)))

        try {
          /* 直接从已解码的 RGBA 里裁子矩形，省一次 getImageData（那条路要回读 GPU） */
          const sub = new Uint8ClampedArray(rw * rh * 4)
          const stride = W * 4
          for (let y = 0; y < rh; y++) {
            const from = (ry + y) * stride + rx * 4
            sub.set(data.data.subarray(from, from + rw * 4), y * rw * 4)
          }
          /*
           * 强度 1.5：标准 MKS2013 是 s=1，但 100px 头像上偏软；
           * s=1.5 在「清晰」和「无白边」之间平衡最好（s=2 起嘴唇/牙齿边缘开始发硬）。
           * 实测 lap_var：s=0→182，s=1→662，s=1.5→1127，s=2→1706。
           */
          const scaled = resampleMks(
            { data: sub, width: rw, height: rh },
            outPx,
            outPx,
            true,
            1.5,
          )
          const out = ctx.createImageData(outPx, outPx)
          out.data.set(scaled.data)
          ctx.putImageData(out, 0, 0)
          if (DIAG_STRIP) {
            ctx.fillStyle = '#00c853'
            ctx.fillRect(0, outPx - 4, outPx, 4)
          }
        } catch {
          /* MKS 出问题不能让头像空着 —— 退回 canvas 原生缩放 */
          ctx.imageSmoothingQuality = 'high'
          ctx.drawImage(src, sx, sy, sw, sh, 0, 0, outPx, outPx)
          if (DIAG_STRIP) {
            ctx.fillStyle = '#d50000'
            ctx.fillRect(0, outPx - 4, outPx, 4)
          }
        }
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
        <canvas
          /* key 跟人脸走：换人时直接重建 canvas，不靠清空来防残留 */
          key={face.id}
          ref={ref}
          className="pv-face-canvas"
          style={{ width: size, height: size }}
        />
      ) : (
        <span className="pv-face-fallback">{fallback ?? ''}</span>
      )}
    </span>
  )
}
