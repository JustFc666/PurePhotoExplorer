/**
 * 元素是否曾进入视口。
 *
 * ## 为什么用 IntersectionObserver 而不是监听 scroll
 * 监听 scroll 的话每个单元格都要自己 `getBoundingClientRect()`，滚动时是在
 * 主线程上做 N 次布局查询 —— 正是我们要避免的事。Observer 由浏览器在合成
 * 阶段算好再回调，滚动本身零开销。
 *
 * ## `once` 语义
 * 首次进入视口后就**断开观察**（`inView` 只从 `false` 变 `true`，不回头）。
 * 缩略图一旦拿到就留在 DOM 的 canvas 上，滚出去再滚回来不需要重新判断；
 * 反复触发的唯一后果是同一张图被重复请求。
 *
 * `root` 保持默认（视图层）。**这是对的**：`overflow: auto` 的滚动容器本身
 * 在视图层里，浏览器的相交计算会按最近的滚动裁剪祖先去裁，
 * 所以「滚出网格可视区」同样会被判定为不相交，不需要显式传 `root`。
 */

import { useEffect, useState } from 'react'

/** 提前一屏开始加载，快速滚动时不至于看到成片空白。 */
const DEFAULT_ROOT_MARGIN = '320px'

/**
 * 返回**回调 ref**（不是 `useRef` 对象）。
 *
 * ⚠️ 别改回 `useRef`：本仓库解析到的 `@types/react` 是 **19.x**，它的
 * `RefObject<T>` 语义是 `{ current: T }`（不含 null），于是
 * `useRef<T | null>(null)` 得到的 `RefObject<T | null>` **赋不给** `ref` 属性
 * （`Type 'X | null' is not assignable to type 'X'`）。回调 ref 两种语义下都成立，
 * 而且 `el` 进 state 后 effect 依赖它是真实的，不用再靠 `ref.current` 读。
 */
export function useInView<T extends Element>(
  rootMargin: string = DEFAULT_ROOT_MARGIN,
): { ref: (el: T | null) => void; inView: boolean } {
  const [el, setEl] = useState<T | null>(null)
  const [inView, setInView] = useState(false)

  useEffect(() => {
    if (inView || !el) return
    // 老 WebView 没有 IntersectionObserver 时退化门「直接可见」，宁可多请求也别不显示
    if (typeof IntersectionObserver === 'undefined') {
      setInView(true)
      return
    }
    const io = new IntersectionObserver(
      (entries) => {
        for (const e of entries) {
          if (!e.isIntersecting) continue
          setInView(true)
          io.disconnect()
          return
        }
      },
      { rootMargin },
    )
    io.observe(el)
    return () => io.disconnect()
  }, [el, inView, rootMargin])

  return { ref: setEl, inView }
}
