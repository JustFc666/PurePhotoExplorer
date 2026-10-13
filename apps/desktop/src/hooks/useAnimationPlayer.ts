/**
 * GIF 动图播放器。
 *
 * ## 为什么用 `setTimeout` 链而不是 `setInterval`
 * GIF 每一帧的停留时长是**逐帧独立**的（规范里 `delay` 就是每帧自己的），
 * 常见的动图会混用 40ms / 100ms / 500ms。用固定周期的 `setInterval`
 * 会把所有帧拉成同一个节奏，看起来「该慢的地方也很快」。
 * 所以每帧结束后按**下一帧自己的** `delayMs` 重新排一个定时器。
 *
 * ## 暂停时机
 * 由调用方决定（`playing` 是受控的）：比如打开画布调整面板时暂停，
 * 否则画面一直在跳，用户没法对准裁剪框。
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { DecodedAnimation, DecodedImage } from '../ipc'

/** 单帧延时的下限（毫秒）。
 *
 * 有些动图把 delay 写成 10ms（= 1 厘秒）甚至更小，直接照做会疯狂重绘、
 * 白白吃 CPU 且看不出差别；主流浏览器也都有下限（约 20ms）。
 */
const MIN_DELAY_MS = 20

export interface UseAnimationPlayer {
  /**
   * 当前应显示的**帧**；不可播放（非动图 / 只有一帧）时为 `null`。
   *
   * 调用方按 `frame ?? image` 取最终要画的图。
   */
  frame: DecodedImage | null
  /** 当前帧序号（0 起） */
  index: number
  /** 总帧数 */
  count: number
  /** 是否是可播放的动图（帧数 > 1） */
  isAnimated: boolean
  /** 是否正在播放 */
  playing: boolean
  /** 播放 / 暂停切换 */
  toggle: () => void
  /** 显式暂停（调用方在特定场景下用，如打开调整面板） */
  pause: () => void
  /** 显式播放 */
  play: () => void
}

/**
 * GIF 动图播放器。
 *
 * ## 翻页时为什么要显式传 `resetKey`
 * 动图是**渐进式加载**的：先播首批，后台批次不断往 `frames` 尾巴上追加
 * （每次追加都是一个新的 `animation` 对象）。若拿 `animation` 对象本身当
 * 「换图了」的判据，每来一批就会把播放位置重置回第 0 帧 —— 看起来就是
 * 「动画一直在开头卡着重播」。所以重置判据取与帧数无关的
 * **图片路径**（`info.path`），追加帧不会打断播放。
 *
 * @param resetKey 换图时变化的标识（传图片路径即可）；变化即回到第 0 帧并恢复播放
 */
export function useAnimationPlayer(
  animation: DecodedAnimation | null,
  resetKey: string | null,
): UseAnimationPlayer {
  const [index, setIndex] = useState(0)
  const [playing, setPlaying] = useState(true)

  const count = animation?.frames.length ?? 0
  const isAnimated = count > 1

  /**
   * 帧数组放 ref：定时器只关心「现在是第几帧、下一帧多久」，
   * 后台追加帧不该重启当前帧的计时（否则播放会被一批批地"顶住"）。
   */
  const framesRef = useRef<DecodedAnimation['frames']>([])
  framesRef.current = animation?.frames ?? []

  // 换动图（或换成静图）：回到第一帧并恢复播放。
  // ⚠️ 判据是 `resetKey` 而不是 `animation` —— 见上方 docblock。
  useEffect(() => {
    setIndex(0)
    setPlaying(true)
  }, [resetKey])

  // 定时器：每帧按自己的 delayMs 排下一次
  useEffect(() => {
    if (!isAnimated || !playing) return
    const current = framesRef.current[index]
    if (!current) return

    const delay = Math.max(MIN_DELAY_MS, current.delayMs)
    const timer = window.setTimeout(() => {
      // 用函数式更新取最新帧数，避免闭包里的 count 过期
      setIndex((i) => (i + 1) % Math.max(1, framesRef.current.length))
    }, delay)

    return () => window.clearTimeout(timer)
  }, [resetKey, isAnimated, playing, index])

  const toggle = useCallback(() => setPlaying((p) => !p), [])
  const pause = useCallback(() => setPlaying(false), [])
  const play = useCallback(() => setPlaying(true), [])

  const frame = useMemo(
    () => (isAnimated && animation ? (animation.frames[index]?.image ?? null) : null),
    [isAnimated, animation, index],
  )

  return { frame, index, count, isAnimated, playing, toggle, pause, play }
}
