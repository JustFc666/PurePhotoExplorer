/**
 * 像素传输协议的解析测试。
 *
 * 这里**手工按 Rust `pack_animation` 的布局造块**，而不是调用后端 ——
 * 目的是把「协议本身」钉死：Rust 侧改了布局、或前端偏移算错，
 * 都会在这里直接炸出来，不需要启动应用去肉眼看动图。
 */

import { describe, expect, it } from 'vitest'
import { buildAnim } from './animFixture'
import {
  ANIM_HEADER_FIXED,
  ANIM_MAGIC,
  ANIM_VERSION,
  PIXEL_HEADER_LEN,
  parseAnimation,
  parsePixels,
  toBytes,
} from './protocol'

describe('parseAnimation —— PVA1 动图块', () => {
  it('解析出尺寸、帧数、延时与循环次数', () => {
    const { buf } = buildAnim(4, 2, [100, 40, 250], { loop: 3 })
    const anim = parseAnimation(new Uint8Array(buf))

    expect(anim.width).toBe(4)
    expect(anim.height).toBe(2)
    expect(anim.count).toBe(3)
    expect(anim.frames.map((f) => f.delayMs)).toEqual([100, 40, 250])
    expect(anim.loopCount).toBe(3)
    expect(anim.truncated).toBe(false)
  })

  it('每帧像素切在正确偏移上（不串位）', () => {
    const { buf, stride } = buildAnim(4, 2, [10, 10, 10, 10, 10])
    const anim = parseAnimation(new Uint8Array(buf))
    anim.frames.forEach((f, i) => {
      expect(f.image.data.length, `第 ${i} 帧长度`).toBe(stride)
      expect(f.image.data[0], `第 ${i} 帧内容`).toBe(i)
    })
  })

  it('所有帧共享同一块 ArrayBuffer（零拷贝，不是逐帧复制）', () => {
    const { buf } = buildAnim(8, 8, [10, 10, 10])
    const anim = parseAnimation(new Uint8Array(buf))
    const base = anim.frames[0]!.image.data.buffer
    for (const f of anim.frames) {
      expect(f.image.data.buffer).toBe(base)
    }
  })

  it('逐帧偏移严格等于 stride（相邻帧首尾相接）', () => {
    const { buf, stride } = buildAnim(4, 4, [10, 10, 10])
    const anim = parseAnimation(new Uint8Array(buf))
    const o0 = anim.frames[0]!.image.data.byteOffset
    const o1 = anim.frames[1]!.image.data.byteOffset
    const o2 = anim.frames[2]!.image.data.byteOffset
    expect(o1 - o0).toBe(stride)
    expect(o2 - o1).toBe(stride)
  })

  it('loop = -1 映射成 null（无限循环）', () => {
    const { buf } = buildAnim(2, 2, [10], { loop: -1 })
    expect(parseAnimation(new Uint8Array(buf)).loopCount).toBeNull()
  })

  it('loop = 0 与 null 区分开（有限循环 0 次）', () => {
    const { buf } = buildAnim(2, 2, [10], { loop: 0 })
    expect(parseAnimation(new Uint8Array(buf)).loopCount).toBe(0)
  })

  it('截断位正确读出', () => {
    const { buf } = buildAnim(2, 2, [10], { truncated: true })
    expect(parseAnimation(new Uint8Array(buf)).truncated).toBe(true)
  })

  it('零帧块可解析（不抛错）', () => {
    const { buf } = buildAnim(2, 2, [])
    const anim = parseAnimation(new Uint8Array(buf))
    expect(anim.count).toBe(0)
    expect(anim.frames).toEqual([])
  })

  it('魔数不符时抛出可读错误', () => {
    const { buf } = buildAnim(2, 2, [10])
    new DataView(buf).setUint8(0, 0x58) // 'X'
    expect(() => parseAnimation(new Uint8Array(buf))).toThrow(/格式不正确/)
  })

  it('协议版本不符时抛出（而不是照着新布局乱解）', () => {
    const { buf } = buildAnim(2, 2, [10], { version: 99 })
    expect(() => parseAnimation(new Uint8Array(buf))).toThrow(/版本不支持/)
  })

  it('长度不足时抛出（不越界读）', () => {
    expect(() => parseAnimation(new Uint8Array(8))).toThrow(/长度不足/)

    const { buf, stride } = buildAnim(4, 4, [10, 10])
    // 砍掉半帧：声明 2 帧但像素区缺字节
    const short = new Uint8Array(buf, 0, buf.byteLength - stride / 2)
    expect(() => parseAnimation(short)).toThrow(/长度不符/)
  })

  it('尺寸为 0 时抛出', () => {
    const { buf } = buildAnim(0, 2, [10])
    expect(() => parseAnimation(new Uint8Array(buf))).toThrow(/尺寸非法/)
  })

  it('魔数常量与 Rust 侧一致', () => {
    expect(ANIM_MAGIC).toBe('PVA1')
    expect(ANIM_VERSION).toBe(2)
    expect(ANIM_HEADER_FIXED).toBe(32)
    expect(PIXEL_HEADER_LEN).toBe(13)
  })

  it('解析出本块起始序号与整段总帧数（分块传输的依据）', () => {
    // 造「整段 100 帧里的第 40..43 帧」这一块
    const { buf } = buildAnim(2, 2, [10, 10, 10], { start: 40, total: 100 })
    const anim = parseAnimation(new Uint8Array(buf))
    expect(anim.count).toBe(3)
    expect(anim.startIndex).toBe(40)
    expect(anim.totalCount).toBe(100)
  })

  it('分块后剩余帧数正好接得上（start + count = 已到位帧数）', () => {
    const a = parseAnimation(new Uint8Array(buildAnim(2, 2, [10, 10, 10], { start: 0, total: 7 }).buf))
    const b = parseAnimation(new Uint8Array(buildAnim(2, 2, [10, 10, 10, 10], { start: 3, total: 7 }).buf))
    // 第二块必须从第一块的末尾接着来（否则播放到接缝处会跳帧）
    expect(a.startIndex + a.count).toBe(b.startIndex)
    // 两块拼起来应覆盖整段
    expect(b.startIndex + b.count).toBe(b.totalCount)
  })
})

describe('toBytes —— 后端返回形态归一', () => {
  it('ArrayBuffer 直接零拷贝包装', () => {
    const ab = new ArrayBuffer(16)
    const u8 = toBytes(ab)
    expect(u8.byteLength).toBe(16)
    expect(u8.buffer).toBe(ab)
    expect(u8.byteOffset).toBe(0)
  })

  it('TypedArray 保持原偏移与长度', () => {
    const ab = new ArrayBuffer(32)
    const view = new Uint8Array(ab, 8, 10)
    const u8 = toBytes(view)
    expect(u8.byteOffset).toBe(8)
    expect(u8.byteLength).toBe(10)
  })

  it('number[] 兜底可转（旧通道形态）', () => {
    expect(Array.from(toBytes([1, 2, 3]))).toEqual([1, 2, 3])
  })

  it('不认识的类型抛出可读错误', () => {
    expect(() => toBytes({} as unknown)).toThrow(/意外类型/)
  })
})

describe('parsePixels —— PVI1 静图包', () => {
  it('解析裸字节 ArrayBuffer（新路径）', () => {
    const w = 3
    const h = 2
    const ab = new ArrayBuffer(PIXEL_HEADER_LEN + w * h * 4)
    const u8 = new Uint8Array(ab)
    u8[0] = 0x50
    u8[1] = 0x56
    u8[2] = 0x49
    u8[3] = 0x31
    const dv = new DataView(ab)
    dv.setUint32(4, w, true)
    dv.setUint32(8, h, true)

    const img = parsePixels(toBytes(ab))
    expect(img.width).toBe(w)
    expect(img.height).toBe(h)
    expect(img.data.length).toBe(w * h * 4)
    // 零拷贝：像素视图指向同一 buffer
    expect(img.data.buffer).toBe(ab)
  })
})
