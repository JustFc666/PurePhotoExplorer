/**
 * 单图浏览（viewer）领域共享类型。
 */

/** 浏览状态机：空 / 加载中 / 就绪 / 出错 */
export type Status =
  | { kind: 'empty' }
  | { kind: 'loading' }
  | { kind: 'ready' }
  | { kind: 'error'; message: string; hint?: string; raw?: unknown }
