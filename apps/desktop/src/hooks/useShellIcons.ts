/**
 * 侧栏树的**系统图标**（盘符 / 文件夹 / 此电脑），进程内只取一次。
 *
 * 图标是后端用 `SHGetFileInfoW` 现取的（资源管理器那一套），编码成
 * `data:image/bmp;base64` —— 一张 32×32 的 32bpp BMP 约 5KB，base64 后 ~7KB，
 * 三个图标二十来 KB，一次取完放模块级缓存即可。
 *
 * ⚠️ 缓存放在**模块级**而不是组件里：树是递归渲染的，每行都调这个 hook，
 * 没有模块级缓存就会每行发一次 IPC。
 */

import { useEffect, useState } from 'react'

import { fsTreeIcons, type TreeIcons } from '../ipc/browse'

let cache: TreeIcons | null = null
/** 进行中的请求（多个组件同时挂载时只发一次） */
let inflight: Promise<TreeIcons> | null = null

export function useShellIcons(): TreeIcons | null {
  const [icons, setIcons] = useState<TreeIcons | null>(cache)

  useEffect(() => {
    if (cache) {
      setIcons(cache)
      return
    }
    inflight ??= fsTreeIcons()
    let alive = true
    inflight
      .then((v) => {
        cache = v
        if (alive) setIcons(v)
      })
      .catch(() => {
        // 取不到就回退到字形图标（`iconOf`），不打扰用户。
        // ⚠️ 失败要把 inflight 清空，否则后续挂载永远拿到同一个 rejected promise。
        inflight = null
      })
    return () => {
      alive = false
    }
  }, [])

  return icons
}
