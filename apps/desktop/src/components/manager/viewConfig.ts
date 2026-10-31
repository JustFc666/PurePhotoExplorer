/**
 * 图库视图的**模块级配置与纯函数**（不依赖 React，也不碰后端）。
 *
 * 为什么单独成文件：`ManagerView.tsx` 有三千行，开头这一段「常量 + 两个小工具函数」
 * 和下面的组件状态毫无关系，却被夹在同一份 import 里 —— 想改一个侧栏默认宽度
 * 得先滚过一堆 JSX 才找到。搬到这儿以后：
 * - 常量改起来不用碰组件；
 * - 纯函数能直接单测（目前只是没写，随时可以补）；
 * - ⛔ 这里**不许出现 hook / 组件 / 后端调用**，只放常量、类型外的工具。
 */

import type { GridItem } from '../../hooks/useManager'

/**
 * 视口之后预取多少张。
 *
 * 30 是「大约两屏」的量：卡片最小档（116px）一屏约 40 张，最大档（320px）约 12 张，
 * 30 张在任何档位都够铺满下一屏。再多就是纯浪费 —— 用户根本没在滚的时候，
 * 多取的每一张都在和真正要看的图抢那 4 个槽位。
 */
export const PREFETCH_AHEAD = 30

/**
 * 从一组行里取**可操作的绝对路径**（离线/卷未挂载的没有 `path`）。
 *
 * ⚠️ 不能只从「选中集合」取：**浏览模式（未入库）的每一行 `id` 都是 `null`**，
 * 而 `selectedItems` 的条件是 `id != null` —— 那集合在浏览模式永远是空的。
 * 之前导出/转格式全走它，于是文件夹浏览模式下右键一律报
 * 「这些图片没有可用的本地路径」，可实际上路径就在 `item.path` 上。
 */
export function filePathsOf(items: GridItem[]): string[] {
  const out: string[] = []
  for (const it of items) {
    if (it.path) out.push(it.path)
  }
  return out
}

/** 后端格式名 → 筛选条上的显示名。 */
export const FORMAT_LABEL: Record<string, string> = {
  jpeg: 'JPG',
  png: 'PNG',
  gif: 'GIF',
  webp: 'WebP',
  heic: 'HEIC',
  heif: 'HEIF',
  psd: 'PSD',
  bmp: 'BMP',
  tiff: 'TIFF',
  avif: 'AVIF',
  '?': '未知',
}

/** 千分位数字（`12,345`）。`toLocaleString('zh-CN')` 显式给 locale —— 别依赖系统语言。 */
export function fmtCount(n: number): string {
  return n.toLocaleString('zh-CN')
}

/**
 * 格式名 → 筛选条上那颗芯片的字。
 *
 * 后端偶尔会冒出新格式（`FORMAT_LABEL` 还没跟上），这时退化成大写原名 ——
 * ⛔ 别改成小写原样，或显示 `undefined` / `未知`：`未知` 是留给 `?` 的
 * （前端认不出容器时的占位），新格式用原名反而能让用户一眼报上来。
 */
export function formatLabelOf(fmt: string): string {
  return FORMAT_LABEL[fmt] ?? fmt.toUpperCase()
}

/* ── 侧栏宽度（可拖拽，记忆在 localStorage）────────────────── */
const SIDE_WIDTH_KEY = 'pv.mg.sideWidth'
export const SIDE_MIN = 160
export const SIDE_MAX = 480
export const SIDE_DEFAULT = 208

/** 读上次拖到的侧栏宽度（没存过 / 越界 / 非数字 → 默认值）。 */
export function readSideWidth(): number {
  try {
    const raw = Number(localStorage.getItem(SIDE_WIDTH_KEY))
    if (Number.isFinite(raw) && raw >= SIDE_MIN && raw <= SIDE_MAX) return raw
  } catch {
    /* localStorage 不可用（隐私模式）就给默认值 */
  }
  return SIDE_DEFAULT
}

/**
 * 把拖拽算出来的宽度夹进合法区间。
 *
 * 单独提出来是因为它和「读写 localStorage」不是一回事：这条是每次 `mousemove`
 * 都要跑的纯算式（两侧边界各一次），那两条只在开始/结束各跑一次。
 */
export function clampSideWidth(px: number): number {
  return Math.min(SIDE_MAX, Math.max(SIDE_MIN, px))
}

/** 记住侧栏宽度（拖拽结束时调用一次，别在 `mousemove` 里写）。 */
export function saveSideWidth(px: number): void {
  try {
    localStorage.setItem(SIDE_WIDTH_KEY, String(px))
  } catch {
    /* 忽略：只影响下次启动的默认值 */
  }
}

/**
 * 侧栏「图库 +」弹出的新建图库**默认名**（用户 2026-10-18 指定）。
 *
 * ⭐ 打开即全选（见 `PromptDialog` 的 `el.select()`），直接敲字就覆盖，
 * 所以默认值只起「提示格式」的作用，不用怕它挡路。
 */
export const DEFAULT_LIB_NAME = '新的图库'
