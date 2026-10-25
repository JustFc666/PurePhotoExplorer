/**
 * 缩略图请求闸门 —— 限制「同时在飞」的 `manager_thumbnail` 数量。
 *
 * ## 为什么需要
 * 图库首屏会把一页（`PAGE_LIMIT = 500`）的行全部挂到网格上，每个单元格都要
 * 取一次缩略图。不加限制时 JS 会瞬间抛出几百个 `invoke`，后果有两层：
 * 1. **进度不可见** —— 后端的 L1/L2 缓存是串行的（`Store` 被 `Mutex` 保护），
 *    几百个请求排成一队，用户只看到一片空白，无从判断是在跑还是卡死；
 * 2. 浪费 —— 滚不到的位置也在抢正在生成的那几张的带宽。
 *
 * 配套的另外两层（缺一不可）：
 * - 后端：`manager_thumbnail` 是 `async` 命令（否则它跑在主线程上，
 *   整个窗口会「未响应」，详见 `commands/manager.rs` 的注释）；
 * - 组件层：`ManagerView` 的单元格只在**进入视口**后才来排队。
 *
 * 这里**不做**去重和缓存 —— 后端 L1 内存 + L2 磁盘已经是缓存，
 * 前端再存一份只会让内存翻倍（而且前端拿不到 LRU 的容量感知）。
 */

/**
 * 同时在飞的上限。
 *
 * 选 4 而不是更大：后端那把 `Mutex` 是**串行**的，排更多并发只是把等待挪个地方，
 * 却会让「已经生成好的图尽快上屏」这件事变慢。4 个刚好让「磁盘 L2 命中」
 * 这种极快的请求能插进来。
 */
const MAX_CONCURRENT = 4

/**
 * 请求优先级。
 *
 * - `high`（默认）= 用户**正看着**的那几张
 * - `low` = **预取**：视口之后若干张，只为让快速滚动时少露白
 *
 * 分开排队的理由：预取一次会往队列里塞几十个请求，而用户随时可能滚到新位置。
 * 若只有一个 FIFO 队列，新进入视口的图会排在几十个「已经滚过去」的预取后面，
 * 等于预取反而拖慢了真正要看的那张。
 */
type Priority = 'high' | 'low'

let active = 0
/** 排队中的等待者，按优先级分成两队（每个元素是「轮到你占槽位了」的 resolve） */
const waiting: Record<Priority, Array<() => void>> = { high: [], low: [] }
/** 订阅者：待办数量变化时被通知（供底栏显示进度） */
const listeners = new Set<(pending: number) => void>()

function pending(): number {
  return active + waiting.high.length + waiting.low.length
}

function notify(): void {
  const n = pending()
  for (const fn of listeners) fn(n)
}

/**
 * 订阅「还在排队或正在生成」的数量。
 *
 * 订阅时会**立即回调一次**当前值，所以调用方不用自己初始化 state。
 * 返回退订函数（可直接作为 `useEffect` 的返回值）。
 */
export function subscribeThumbPending(fn: (pending: number) => void): () => void {
  listeners.add(fn)
  fn(pending())
  return () => {
    listeners.delete(fn)
  }
}

/** 让出一个槽位：唤醒下一个等待者（`active` 的加减在调用方完成）。 */
function shift(): void {
  // 高优先级先来：滚动到新位置的那几张不该排在预取后面
  const release = waiting.high.shift() ?? waiting.low.shift()
  if (!release) return
  active += 1
  release()
  notify()
}

/**
 * 占用一个槽位执行 `fn`。
 *
 * 走到 `fn` 内部时保证 `active <= MAX_CONCURRENT`；无论成功失败都会释放槽位。
 *
 * `priority` 只影响**排队顺序**，不影响并发上限：高优先级插到队首，
 * 低优先级（预取）排后面。默认 `high`，所以不传的调用点行为不变。
 */
export async function withThumbSlot<T>(
  fn: () => Promise<T>,
  priority: Priority = 'high',
): Promise<T> {
  if (active >= MAX_CONCURRENT) {
    await new Promise<void>((resolve) => {
      waiting[priority].push(resolve)
      notify()
    })
  } else {
    active += 1
    notify()
  }
  try {
    return await fn()
  } finally {
    active -= 1
    shift()
    notify()
  }
}

/** 仅供测试/诊断：当前在飞数量。 */
export const __activeCount = (): number => active
