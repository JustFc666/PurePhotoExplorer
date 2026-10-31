/**
 * 排序选项 —— 顶栏菜单共用一份。
 *
 * ## 为什么不用原生 `<select>`
 * 这个窗口是**自绘无边框 + 深色主题**，原生 select 的弹层由 WebView2 自己画：
 * 配色不跟随应用主题（深底深字，选项看起来「不存在」），位置也可能被窗口边界裁掉。
 * 换成自绘菜单后配色/定位/键盘行为都完全可控，而且和右键菜单是同一套视觉。
 */

import type { ManagerSort } from '../ipc/manager'

export interface SortOption {
  value: ManagerSort
  label: string
  /** 只有已入库的目录才有这些数据（浏览模式置灰） */
  libraryOnly?: boolean
}

export const SORT_OPTIONS: SortOption[] = [
  { value: 'name', label: '文件名' },
  { value: 'taken', label: '拍摄时间' },
  { value: 'mtime', label: '修改时间' },
  { value: 'size', label: '文件大小' },
  { value: 'format', label: '文件类型' },
  { value: 'dims', label: '分辨率' },
  { value: 'added', label: '入库时间', libraryOnly: true },
  { value: 'rating', label: '评分', libraryOnly: true },
  { value: 'flag', label: '标记', libraryOnly: true },
]
