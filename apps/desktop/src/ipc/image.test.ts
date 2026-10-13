/**
 * 渐进式动图加载（`decodeAnimationProgressive`）的驱动逻辑测试。
 *
 * 这是最值得测的一层：协议解析已经有 `protocol.test.ts` 钉住，
 * 而这里的分块循环有「起始序号算错 → 播放到接缝处跳帧/倒回」
 * 「循环不收敛 → 一直发请求」这两类**肉眼很难发现**的 bug。
 */

import { beforeEach, describe, expect, it, vi } from 'vitest'

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }))

vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))
vi.mock('./app', () => ({ traceLog: vi.fn() }))

import { buildAnim } from './animFixture'
import {
  decodeAnimationProgressive,
  decodeImage,
  getFileVersion,
  invalidateImage,
  prefetchImage,
} from './image'

/** 一次 `decode_animation` 调用记录 */
interface Call {
  path: string
  start: number
  maxBytes: number
  minDurationMs: number
}

let calls: Call[]

/**
 * 造一个「会分块返回」的假后端。
 *
 * `plan` 形如 `{ 0: 3, 3: 4 }` —— 表示请求 start=0 时给 3 帧、
 * start=3 时给 4 帧。`total` 是整段总帧数。
 */
function mockChunkedBackend(plan: Record<number, number>, total: number) {
  calls = []
  invokeMock.mockImplementation((_cmd: string, args: Record<string, unknown>) => {
    const start = Number(args.start ?? 0)
    const maxBytes = Number(args.maxBytes ?? 0)
    calls.push({ path: String(args.path), start, maxBytes, minDurationMs: Number(args.minDurationMs ?? 0) })

    const count = plan[start]
    if (count === undefined) return Promise.reject(new Error(`意外的 start=${start}`))
    const { buf } = buildAnim(2, 2, Array.from({ length: count }, () => 100), {
      start,
      total,
      markerBase: start,
    })
    return Promise.resolve(buf)
  })
}

beforeEach(() => {
  invokeMock.mockReset()
})

describe('decodeAnimationProgressive —— 两段式加载', () => {
  it('首批立即回调（isFirst = true），且不等后续帧就返回', async () => {
    mockChunkedBackend({ 0: 3, 3: 4 }, 7)

    const batches: { count: number; isFirst: boolean }[] = []
    await decodeAnimationProgressive('a.gif', 'preview', (b, isFirst) => {
      batches.push({ count: b.count, isFirst })
    }, () => false)

    // 返回时只有首批（后续在后台）
    expect(batches).toHaveLength(1)
    expect(batches[0]).toEqual({ count: 3, isFirst: true })
  })

  it('首批带 8MB 字节预算 + 3 秒时长下限；后续请求两项都不限', async () => {
    mockChunkedBackend({ 0: 3, 3: 4 }, 7)
    await decodeAnimationProgressive('a.gif', 'preview', () => undefined, () => false)
    await vi.waitFor(() => expect(calls).toHaveLength(2))

    expect(calls[0]!.start).toBe(0)
    expect(calls[0]!.maxBytes).toBe(8 * 1024 * 1024)
    // 只有字节预算不够：短延时的动图会被切得太短，导致后台还没到就"播完了"
    expect(calls[0]!.minDurationMs).toBe(3000)
    expect(calls[1]!.start).toBe(3)
    expect(calls[1]!.maxBytes).toBe(0)
    expect(calls[1]!.minDurationMs).toBe(0)
  })

  it('后续批次的 start **接着上一批的末尾**，且全部到齐', async () => {
    mockChunkedBackend({ 0: 3, 3: 4 }, 7)

    const got: { start: number; count: number }[] = []
    await decodeAnimationProgressive('a.gif', 'preview', (b) => {
      got.push({ start: b.startIndex, count: b.count })
    }, () => false)
    await vi.waitFor(() => expect(got).toHaveLength(2))

    expect(got[0]).toEqual({ start: 0, count: 3 })
    expect(got[1]).toEqual({ start: 3, count: 4 })
    // 拼起来正好覆盖整段，没有空洞也没有重叠
    expect(got[0]!.start + got[0]!.count).toBe(got[1]!.start)
    expect(got[1]!.start + got[1]!.count).toBe(7)
  })

  it('分三批也能正确接续（不是写死的两段）', async () => {
    mockChunkedBackend({ 0: 2, 2: 3, 5: 2 }, 7)

    const got: number[] = []
    await decodeAnimationProgressive('a.gif', 'preview', (b) => got.push(b.startIndex), () => false)
    await vi.waitFor(() => expect(got).toHaveLength(3))
    expect(got).toEqual([0, 2, 5])
  })

  it('首批就覆盖整段时不再发第二次请求', async () => {
    mockChunkedBackend({ 0: 7 }, 7)
    await decodeAnimationProgressive('a.gif', 'preview', () => undefined, () => false)
    // 给后台循环一点时间；若它错误地继续请求，这里会变成 2
    await new Promise((r) => setTimeout(r, 20))
    expect(calls).toHaveLength(1)
  })

  it('首批到达前就已作废（用户翻了别的图）→ 一次回调都不发', async () => {
    mockChunkedBackend({ 0: 3, 3: 4 }, 7)
    const onBatch = vi.fn()
    await decodeAnimationProgressive('a.gif', 'preview', onBatch, () => true)
    expect(onBatch).not.toHaveBeenCalled()
  })

  it('首批之后作废 → 后台不再继续请求', async () => {
    mockChunkedBackend({ 0: 3, 3: 4 }, 7)
    let stale = false
    const got: number[] = []
    await decodeAnimationProgressive(
      'a.gif',
      'preview',
      (b, isFirst) => {
        got.push(b.startIndex)
        if (isFirst) stale = true // 首批一回来就"翻页"
      },
      () => stale,
    )
    await new Promise((r) => setTimeout(r, 20))
    expect(got).toEqual([0])
    expect(calls).toHaveLength(1)
  })

  it('后台批次失败**不影响**已到手的首批（不抛错）', async () => {
    // start=3 时直接拒绝：模拟后台补帧失败
    calls = []
    invokeMock.mockImplementation((_cmd: string, args: Record<string, unknown>) => {
      const start = Number(args.start ?? 0)
      calls.push({
        path: String(args.path),
        start,
        maxBytes: Number(args.maxBytes ?? 0),
        minDurationMs: Number(args.minDurationMs ?? 0),
      })
      if (start === 0) {
        return Promise.resolve(buildAnim(2, 2, [100, 100], { start: 0, total: 7 }).buf)
      }
      return Promise.reject(new Error('后台批次挂了'))
    })

    const got: number[] = []
    // 不应该 reject
    await expect(
      decodeAnimationProgressive('a.gif', 'preview', (b) => got.push(b.startIndex), () => false),
    ).resolves.toBeUndefined()

    await new Promise((r) => setTimeout(r, 20))
    expect(got).toEqual([0])
    expect(calls).toHaveLength(2)
  })

  it('后端给出空块（start 越界）→ 停止，不无限循环', async () => {
    calls = []
    invokeMock.mockImplementation((_cmd: string, args: Record<string, unknown>) => {
      const start = Number(args.start ?? 0)
      calls.push({
        path: String(args.path),
        start,
        maxBytes: Number(args.maxBytes ?? 0),
        minDurationMs: Number(args.minDurationMs ?? 0),
      })
      const count = start === 0 ? 3 : 0
      return Promise.resolve(buildAnim(2, 2, Array.from({ length: count }, () => 100), {
        start,
        total: 7,
      }).buf)
    })

    const got: number[] = []
    await decodeAnimationProgressive('a.gif', 'preview', (b) => got.push(b.startIndex), () => false)
    await new Promise((r) => setTimeout(r, 20))

    expect(got).toEqual([0])
    expect(calls).toHaveLength(2)
  })
})

/**
 * 静图解码缓存 —— **文件版本校验**（2026-10-11 修）。
 *
 * ## 用户报的问题
 * 「已预读取的图像，如果被覆盖了，还是显示原图像」。
 *
 * ## 根因
 * 缓存键只有 `purpose::path`，没有"文件版本"。文件被覆盖后（别的程序改的、
 * 或本程序另存为盖了同目录已有文件），缓存里那份**旧像素**会被一直复用。
 * 实测复现：三张纯色图，预取过的 b.png 被外部改成黄色后翻过去，屏幕仍是绿色。
 *
 * ## 现在的契约
 * `decodeImage(path, purpose, version)` 命中前比对版本：不一致就重新解码。
 * 下面第一条就是这条契约的护栏 —— **删了它这个 bug 会原地复活**。
 */
describe('解码缓存 —— 文件版本校验', () => {
  /** 造一个最小 PVI1 静图包；`marker` 写进每个像素，用来分辨"这是哪一版文件" */
  function pix(w: number, h: number, marker: number): Uint8Array {
    const buf = new Uint8Array(13 + w * h * 4)
    buf.set([0x50, 0x56, 0x49, 0x31], 0) // "PVI1"
    const dv = new DataView(buf.buffer)
    dv.setUint32(4, w, true)
    dv.setUint32(8, h, true)
    buf.fill(marker, 13)
    return buf
  }

  interface FakeFile {
    modifiedMs: number
    size: number
    /** 像素标记 = "文件当前内容" */
    marker: number
  }

  let files: Record<string, FakeFile>
  let decodeCalls: string[]
  let clock: number

  /** 写入/覆盖一个文件（mtime 与 size 都变，就像真的被覆盖过） */
  function write(path: string, marker: number): void {
    files[path] = { modifiedMs: ++clock, size: 1000 + marker, marker }
  }

  beforeEach(() => {
    files = {}
    decodeCalls = []
    clock = 0
    invokeMock.mockImplementation((cmd: string, args: Record<string, unknown>) => {
      const path = String(args.path)
      const f = files[path]
      if (cmd === 'file_version') {
        return f
          ? Promise.resolve({ modifiedMs: f.modifiedMs, size: f.size })
          : Promise.reject(new Error('文件不存在'))
      }
      if (cmd === 'decode_image') {
        decodeCalls.push(path)
        return f ? Promise.resolve(pix(2, 2, f.marker)) : Promise.reject(new Error('文件不存在'))
      }
      return Promise.reject(new Error(`意外的命令 ${cmd}`))
    })
  })

  it('⭐ 版本变了 → 丢弃旧像素重新解码（本修复的核心）', async () => {
    write('a.png', 1)
    const first = await decodeImage('a.png', 'preview', await getFileVersion('a.png'))
    expect(first.data[0], '第一次应解出第 1 版').toBe(1)
    expect(decodeCalls).toHaveLength(1)

    // 文件被**覆盖**：内容与版本都变了
    write('a.png', 9)
    const second = await decodeImage('a.png', 'preview', await getFileVersion('a.png'))

    expect(second.data[0], '必须拿到第 9 版，而不是缓存里的第 1 版').toBe(9)
    expect(decodeCalls, '应重新解码一次').toHaveLength(2)
  })

  it('版本相同 → 复用缓存，不再解码', async () => {
    write('b.png', 3)
    const v = await getFileVersion('b.png')
    await decodeImage('b.png', 'preview', v)
    await decodeImage('b.png', 'preview', v)
    await decodeImage('b.png', 'preview', await getFileVersion('b.png'))
    expect(decodeCalls).toHaveLength(1)
  })

  it('⚠️ 版本是**相同秒**内的覆盖也要能识别（mtime 走毫秒）', async () => {
    write('c.png', 1)
    const first = await decodeImage('c.png', 'preview', await getFileVersion('c.png'))
    expect(first.data[0]).toBe(1)

    // 两个版本只差 1 毫秒 —— 用秒级时间戳会漏判
    files['c.png'] = { modifiedMs: clock + 1, size: 2000, marker: 7 }
    clock += 1
    const second = await decodeImage('c.png', 'preview', await getFileVersion('c.png'))
    expect(second.data[0]).toBe(7)
  })

  it('拿不到版本（stat 失败）→ 放行复用，不让看图变慢（失败开放）', async () => {
    write('d.png', 5)
    await decodeImage('d.png', 'preview', await getFileVersion('d.png'))
    // 模拟 stat 失败：getFileVersion 返回 null → 无法校验 → 沿用缓存
    const again = await decodeImage('d.png', 'preview', null)
    expect(again.data[0]).toBe(5)
    expect(decodeCalls).toHaveLength(1)
  })

  it('getFileVersion 失败时返回 null 而不是抛错', async () => {
    await expect(getFileVersion('不存在.png')).resolves.toBeNull()
  })

  it('预取会把版本记进条目 → 之后带同版本加载能直接命中（预取不白做）', async () => {
    write('e.png', 6)
    await prefetchImage('e.png')
    expect(decodeCalls).toHaveLength(1)

    const img = await decodeImage('e.png', 'preview', await getFileVersion('e.png'))
    expect(img.data[0]).toBe(6)
    expect(decodeCalls, '预取得的像素应被复用').toHaveLength(1)
  })

  it('预取之后文件被改 → 加载时仍会重新解码（预取不会掩盖新内容）', async () => {
    write('f.png', 1)
    await prefetchImage('f.png')

    write('f.png', 8) // 预取完就被覆盖
    const img = await decodeImage('f.png', 'preview', await getFileVersion('f.png'))
    expect(img.data[0]).toBe(8)
    expect(decodeCalls).toHaveLength(2)
  })

  it('invalidateImage 清掉该路径的**所有** purpose', async () => {
    write('g.png', 2)
    const v = await getFileVersion('g.png')
    await decodeImage('g.png', 'preview', v)
    await decodeImage('g.png', 'edit', v)
    expect(decodeCalls).toHaveLength(2)

    invalidateImage('g.png')
    await decodeImage('g.png', 'preview', v)
    await decodeImage('g.png', 'edit', v)
    expect(decodeCalls, '两种 purpose 都要重新解码').toHaveLength(4)
  })

  it('并发相同请求只解码一次（去重仍生效）', async () => {
    write('h.png', 4)
    const v = await getFileVersion('h.png')
    const [a, b] = await Promise.all([
      decodeImage('h.png', 'preview', v),
      decodeImage('h.png', 'preview', v),
    ])
    expect(a.data[0]).toBe(4)
    expect(b.data[0]).toBe(4)
    expect(decodeCalls).toHaveLength(1)
  })

  it('解码失败不入缓存（下次可重试）', async () => {
    const bad = decodeImage('i.png', 'preview')
    await expect(bad).rejects.toThrow()
    // 文件后来出现了
    write('i.png', 7)
    const ok = await decodeImage('i.png', 'preview', await getFileVersion('i.png'))
    expect(ok.data[0]).toBe(7)
  })
})
