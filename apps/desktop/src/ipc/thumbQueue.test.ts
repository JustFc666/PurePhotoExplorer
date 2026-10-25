import { describe, expect, it } from 'vitest'

import { __activeCount, subscribeThumbPending, withThumbSlot } from './thumbQueue'

/** 手动放行的 Promise（用来把请求「卡」在飞行中，观察并发数） */
function deferred() {
  let resolve!: () => void
  const promise = new Promise<void>((r) => {
    resolve = r
  })
  return { promise, resolve }
}

/** 跑完当前所有微任务 + 一个宏任务，让排队的 continuation 有机会推进 */
const flush = () => new Promise<void>((r) => setTimeout(r, 0))

describe('缩略图请求闸门', () => {
  it('同时在飞的请求不超过 4 个，且一个都不丢', async () => {
    const gates = Array.from({ length: 12 }, () => deferred())
    const ran: number[] = []
    let peak = 0

    const tasks = gates.map((g, i) =>
      withThumbSlot(async () => {
        ran.push(i)
        peak = Math.max(peak, __activeCount())
        await g.promise
      }),
    )

    // 同步阶段就该只有前 4 个进入 fn，其余仍在 await 排队
    expect(peak).toBe(4)
    expect(ran).toEqual([0, 1, 2, 3])

    // 逐个放行：每放一个，队列里补一个，峰值必须始终是 4
    for (const g of gates) {
      g.resolve()
      await flush()
    }
    await Promise.all(tasks)

    expect(peak).toBe(4)
    expect([...ran].sort((a, b) => a - b)).toEqual([...Array(12).keys()])
    expect(__activeCount()).toBe(0)
  })

  it('任务抛错也要释放槽位（不能把后续请求永久卡住）', async () => {
    await expect(
      withThumbSlot(async () => {
        throw new Error('boom')
      }),
    ).rejects.toThrow('boom')
    expect(__activeCount()).toBe(0)

    // 出过错之后仍然能正常继续拿槽位
    await expect(withThumbSlot(async () => 'ok')).resolves.toBe('ok')
    expect(__activeCount()).toBe(0)
  })

  it('待办计数：订阅时立即回调，跑完回落到 0', async () => {
    const seen: number[] = []
    const unsub = subscribeThumbPending((n) => seen.push(n))
    expect(seen).toEqual([0])

    const gate = deferred()
    const task = withThumbSlot(async () => {
      await gate.promise
    })
    await flush()
    expect(seen).toEqual([0, 1])

    gate.resolve()
    await task
    expect(seen[seen.length - 1]).toBe(0)

    unsub()
    // 退订后不再收到通知
    const before = seen.length
    await withThumbSlot(async () => {})
    expect(seen.length).toBe(before)
  })

  it('排队中的请求也计入待办（否则进度条会提前归零）', async () => {
    const gates = Array.from({ length: 6 }, () => deferred())
    const seen: number[] = []
    const unsub = subscribeThumbPending((n) => seen.push(n))

    const tasks = gates.map((g) =>
      withThumbSlot(async () => {
        await g.promise
      }),
    )
    await flush()
    // 6 个请求：4 个在飞 + 2 个排队 = 6
    expect(seen[seen.length - 1]).toBe(6)

    for (const g of gates) g.resolve()
    await Promise.all(tasks)
    expect(seen[seen.length - 1]).toBe(0)
    unsub()
  })

  it('预取（low）不会挡住可见的请求（high）', async () => {
    // 先把 4 个槽位占满，后面的请求必然排队
    const gates = Array.from({ length: 4 }, () => deferred())
    const blockers = gates.map((g) =>
      withThumbSlot(async () => {
        await g.promise
      }),
    )
    expect(__activeCount()).toBe(4)

    const ran: string[] = []
    // ⚠️ 排队任务内部也要 `await`：否则它会瞬间完成、连锁把整队都消费掉，
    // 就观察不到「谁先拿到槽位」了（第一版测试就是这么写错的）。
    const inner = Array.from({ length: 4 }, () => deferred())
    const task = (name: string, g: { promise: Promise<void> }, priority: 'high' | 'low') =>
      withThumbSlot(async () => {
        ran.push(name)
        await g.promise
      }, priority)

    // ⚠️ 入队顺序：预取**先**、可见的**后**。
    // 若只有一个 FIFO 队列，放行槽位时先跑的就是预取 —— 那正是要避免的事。
    const low = [task('low1', inner[0]!, 'low'), task('low2', inner[1]!, 'low')]
    const high = [task('high1', inner[2]!, 'high'), task('high2', inner[3]!, 'high')]

    await flush()
    expect(ran).toEqual([]) // 全在排队

    gates[0]!.resolve()
    await flush()
    expect(ran).toEqual(['high1'])

    gates[1]!.resolve()
    await flush()
    expect(ran).toEqual(['high1', 'high2'])

    // 可见的都跑完了，才轮到预取
    gates[2]!.resolve()
    await flush()
    expect(ran).toEqual(['high1', 'high2', 'low1'])

    gates[3]!.resolve()
    for (const g of inner) g.resolve()
    await Promise.all([...blockers, ...high, ...low])
    expect(ran).toEqual(['high1', 'high2', 'low1', 'low2'])
    expect(__activeCount()).toBe(0)
  })

  it('预取与可见混在同一批时，并发上限仍是 4', async () => {
    const gates = Array.from({ length: 10 }, () => deferred())
    let peak = 0
    const tasks = gates.map((g, i) =>
      withThumbSlot(
        async () => {
          peak = Math.max(peak, __activeCount())
          await g.promise
        },
        i % 2 === 0 ? 'low' : 'high',
      ),
    )
    expect(peak).toBe(4)
    for (const g of gates) {
      g.resolve()
      await flush()
    }
    await Promise.all(tasks)
    expect(peak).toBe(4)
    expect(__activeCount()).toBe(0)
  })
})
