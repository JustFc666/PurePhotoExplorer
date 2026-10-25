/**
 * 文件浏览器的**懒加载树**状态（侧栏「文件夹」用）。
 *
 * ## 为什么是懒加载
 * 目录树的规模是**无界的**（`C:\` 下可能有几万个子目录）。
 * 一次性递归枚举会在网络盘上卡死，也没必要 ——
 * 用户点开哪个节点才拉哪个节点的**直接子目录**，拉过的缓存住。
 *
 * ## 展开状态为什么放在这里而不是节点自身
 * 节点是后端返回的普通对象，每次展开都是**新对象**；把它们的状态
 * 挂在对象上会在下一次重新拉取时全部丢失。所以状态一律以
 * **路径为 key** 存在这个 hook 里（`path` 为空时用 [`FS_COMPUTER_KEY`]）。
 */

import { useCallback, useEffect, useRef, useState } from 'react'

import { FS_COMPUTER_KEY, fsListDirs, fsRoots, type FsNode } from '../ipc/browse'

/** 节点的稳定 key（路径优先，「我的电脑」用占位符）。 */
export function nodeKey(node: FsNode): string {
  return node.path ?? FS_COMPUTER_KEY
}

export interface FsTreeApi {
  roots: FsNode[]
  error: string | null
  loadingRoots: boolean
  /** 已加载过的子节点；没加载过为 `null` */
  childrenOf: (node: FsNode) => FsNode[] | null
  isLoading: (node: FsNode) => boolean
  isExpanded: (node: FsNode) => boolean
  /** 点击箭头：展开（并按需拉取）或折叠 */
  toggle: (node: FsNode) => void
  /** 主动展开（默认展开「我的电脑」用） */
  expand: (node: FsNode) => void
  /**
   * **展开到指定目录**：从「我的电脑」一路往下把沿途每一级都展开，
   * 返回末端节点；某一级找不到（权限 / 网络盘 / 路径不存在）返回 `null`。
   *
   * 地址栏跳转用：光切主区的话侧栏里什么都看不见，
   * 用户会以为「没跳过去」。
   */
  revealTo: (path: string) => Promise<FsNode | null>
}

/** 把路径切成段：`C:\Users\Fc` → `['C:', 'Users', 'Fc']` */
function splitPath(path: string): string[] {
  return path.split(/[\\/]+/).filter((s) => s.length > 0)
}

export function useFsTree(): FsTreeApi {
  const [roots, setRoots] = useState<FsNode[]>([])
  const [loadingRoots, setLoadingRoots] = useState(true)
  const [error, setError] = useState<string | null>(null)
  /** key → 子节点 */
  const [children, setChildren] = useState<Map<string, FsNode[]>>(new Map())
  const [expanded, setExpanded] = useState<Set<string>>(new Set())
  const [loading, setLoading] = useState<Set<string>>(new Set())

  /**
   * 给异步流程（`revealTo`）读的镜像。
   *
   * ⚠️ `useCallback` 里读 `state` 拿到的是**那次渲染的快照**，
   * 而 `revealTo` 是「逐级 await」的长流程，中途 state 早变了 ——
   * 必须用 ref 才看得到最新值。
   */
  const childrenRef = useRef<Map<string, FsNode[]>>(new Map())
  childrenRef.current = children
  const rootsRef = useRef<FsNode[]>([])
  rootsRef.current = roots
  /** 同一个 key 的请求合流，避免「展开 + revealTo」同时打两次 */
  const pending = useRef<Map<string, Promise<FsNode[]>>>(new Map())

  useEffect(() => {
    let alive = true
    fsRoots()
      .then((r) => {
        if (alive) setRoots(r)
      })
      .catch((e: unknown) => {
        if (alive) setError(e instanceof Error ? e.message : String(e))
      })
      .finally(() => {
        if (alive) setLoadingRoots(false)
      })
    return () => {
      alive = false
    }
  }, [])

  /** 取某节点的直接子目录（有缓存就给缓存），**可 await**。 */
  const loadChildren = useCallback((node: FsNode): Promise<FsNode[]> => {
    const key = nodeKey(node)
    const cached = childrenRef.current.get(key)
    if (cached) return Promise.resolve(cached)
    const inflight = pending.current.get(key)
    if (inflight) return inflight

    setLoading((prev) => new Set(prev).add(key))
    const p = fsListDirs(node.path)
      .then((kids) => {
        setChildren((prev) => new Map(prev).set(key, kids))
        return kids
      })
      .finally(() => {
        pending.current.delete(key)
        setLoading((prev) => {
          const next = new Set(prev)
          next.delete(key)
          return next
        })
      })
    pending.current.set(key, p)
    return p
  }, [])

  const expandNode = useCallback((node: FsNode) => {
    setExpanded((prev) => new Set(prev).add(nodeKey(node)))
  }, [])

  const expand = useCallback(
    (node: FsNode) => {
      expandNode(node)
      loadChildren(node).catch((e: unknown) => {
        setError(e instanceof Error ? e.message : String(e))
      })
    },
    [expandNode, loadChildren],
  )

  const toggle = useCallback(
    (node: FsNode) => {
      const key = nodeKey(node)
      setExpanded((prev) => {
        if (prev.has(key)) {
          const next = new Set(prev)
          next.delete(key)
          return next
        }
        return new Set(prev).add(key)
      })
      // 折叠再展开时不再重复请求 —— `loadChildren` 内部按 key 去重且已有缓存
      loadChildren(node).catch((e: unknown) => {
        setError(e instanceof Error ? e.message : String(e))
      })
    },
    [loadChildren],
  )

  const revealTo = useCallback(
    async (path: string): Promise<FsNode | null> => {
      const parts = splitPath(path)
      if (parts.length === 0) return null
      // `noUncheckedIndexedAccess` 打开时 `parts[0]` 是 `string | undefined`，逐个取出来判空
      const first = parts[0]
      if (first === undefined) return null
      const computer = rootsRef.current.find((n) => n.kind === 'computer')
      if (!computer) return null

      // 1) 展开「我的电脑」拿到盘符，按 `C:` 匹配（盘符节点的 path 是 `C:\`）
      const driveNodes = await loadChildren(computer)
      expandNode(computer)
      const root = driveNodes.find((n) => {
        const p = n.path
        if (!p) return false
        return p.replace(/\\+$/, '').toLowerCase() === first.toLowerCase()
      })
      if (!root) return null

      // 2) 逐级往下：每级都「拉子目录 + 按名字找下一级」，并把父级展开
      // ⚠️ 用独立的 `node` 变量逐级推进：`let cur: T | null` 在循环里被赋值后
      //    TS 会**丢掉收窄**（每次迭代都得重新判空），换成非空局部变量最省事。
      let node: FsNode = root
      for (let i = 1; i < parts.length; i += 1) {
        const seg = parts[i]
        if (seg === undefined) break
        const want = seg.toLowerCase()
        const kids = await loadChildren(node)
        const next = kids.find((n) => n.name.toLowerCase() === want)
        if (!next) return null
        expandNode(node)
        node = next
      }
      // 末端也展开，用户能直接看到里面
      expandNode(node)
      return node
    },
    [expandNode, loadChildren],
  )

  const childrenOf = useCallback((node: FsNode) => children.get(nodeKey(node)) ?? null, [children])
  const isLoading = useCallback((node: FsNode) => loading.has(nodeKey(node)), [loading])
  const isExpanded = useCallback((node: FsNode) => expanded.has(nodeKey(node)), [expanded])

  return { roots, error, loadingRoots, childrenOf, isLoading, isExpanded, toggle, expand, revealTo }
}
