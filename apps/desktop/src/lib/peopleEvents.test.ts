/**
 * 人物变更广播的护栏。
 *
 * 这里不是在测「事件系统能工作」，而是锁住**派发与监听用的是同一个标识**：
 * 原来各组件手写 `dispatchEvent(new CustomEvent('pv:person-renamed'))`，
 * 而删目录那条路径压根没派发 —— 后端 `persons 31 → 8` 改对了，
 * 界面上「更多图像 共31人」还是旧数字（2026-10-27 用户报）。
 */

import { beforeEach, describe, expect, test, vi } from 'vitest'
import {
  META_CHANGED_EVENT,
  PEOPLE_CHANGED_EVENT,
  notifyMetaChanged,
  notifyPeopleChanged,
  onMetaChanged,
  onPeopleChanged,
} from './peopleEvents'

describe('人物变更广播', () => {
  beforeEach(() => {
    /* 默认是 node 环境，没有 window；给个真实 EventTarget 顶上 */
    vi.stubGlobal('window', new EventTarget())
  })

  test('事件名固定为 pv:people-changed', () => {
    // 改名要同步改所有监听方，这里红一下就是提醒
    expect(PEOPLE_CHANGED_EVENT).toBe('pv:people-changed')
  })

  test('派发后订阅者被调用', () => {
    const fn = vi.fn()
    const off = onPeopleChanged(fn)

    notifyPeopleChanged()
    expect(fn).toHaveBeenCalledTimes(1)

    notifyPeopleChanged()
    expect(fn).toHaveBeenCalledTimes(2)

    off()
    notifyPeopleChanged()
    expect(fn).toHaveBeenCalledTimes(2)
  })

  test('多个订阅者都能收到（人像条 + 人像墙同时挂着）', () => {
    const a = vi.fn()
    const b = vi.fn()
    const offA = onPeopleChanged(a)
    const offB = onPeopleChanged(b)

    notifyPeopleChanged()
    expect(a).toHaveBeenCalledTimes(1)
    expect(b).toHaveBeenCalledTimes(1)

    offA()
    offB()
  })
})

/**
 * 「库元数据变了」是另一条线：标签 / 统计 / 筛选维度变了，照片本身没变。
 *
 * 加它的原因（2026-11-01）：在设置面板跑完 AI，主区的标签条和人像条都不动 ——
 * 库里明明写进了 22 个人物，界面上还是空的人像条，用户只能认为「识别没完成」。
 */
describe('元数据变更广播', () => {
  beforeEach(() => {
    vi.stubGlobal('window', new EventTarget())
  })

  test('和人物事件不是同一个标识 —— 两条线各管各的', () => {
    expect(META_CHANGED_EVENT).toBe('pv:meta-changed')
    expect(META_CHANGED_EVENT).not.toBe(PEOPLE_CHANGED_EVENT)
  })

  test('派发元数据事件不会惊动只关心人物的监听方', () => {
    const people = vi.fn()
    const meta = vi.fn()
    const offP = onPeopleChanged(people)
    const offM = onMetaChanged(meta)

    notifyMetaChanged()
    expect(meta).toHaveBeenCalledTimes(1)
    // 主区的 refreshMeta 里本来就会重拉人物，再收一次人物事件等于白跑一遍
    expect(people).not.toHaveBeenCalled()

    offP()
    offM()
  })

  test('退订后不再收到', () => {
    const fn = vi.fn()
    const off = onMetaChanged(fn)
    off()
    notifyMetaChanged()
    expect(fn).not.toHaveBeenCalled()
  })
})
