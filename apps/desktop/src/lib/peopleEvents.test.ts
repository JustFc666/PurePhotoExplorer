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
  PEOPLE_CHANGED_EVENT,
  notifyPeopleChanged,
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
