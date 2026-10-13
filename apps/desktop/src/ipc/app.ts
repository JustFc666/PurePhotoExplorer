/**
 * 应用级后端命令封装（版本、能力列表）。
 */

import { invoke } from '@tauri-apps/api/core'
import type { AppInfo, AssociationItem, AssociationState, FormatCapability } from './types'

export async function getAppInfo(): Promise<AppInfo> {
  return invoke<AppInfo>('app_info')
}

/**
 * 启动时命令行传入的待打开文件；没有则为 `null`。
 *
 * 用于 `photovault.exe photo.cr3` 直接打开。
 */
export async function getStartupFile(): Promise<string | null> {
  return invoke<string | null>('startup_file')
}

export async function getSupportedFormats(): Promise<FormatCapability[]> {
  return invoke<FormatCapability[]>('supported_formats')
}

/**
 * 查询「打开方式」列表里已注册了哪些扩展名。
 *
 * `candidates` 由前端给（就是它能打开的扩展名）—— 后端不重复维护一份清单，
 * 避免两边漏同步。
 */
export async function getAssociationState(
  candidates: readonly string[],
): Promise<AssociationState> {
  return invoke<AssociationState>('association_state', { candidates })
}

/**
 * 把某扩展名加入 / 移出「打开方式」列表。
 *
 * ⚠️ **不是「设为默认」**：Windows 8 起 UserChoice 有哈希保护，
 * 程序无法静默改默认程序（改了会被系统回滚）。这里做的是合法的那一半 ——
 * 之后用户右键图片 →「打开方式」就能看到 PurePhotoExplorer，
 * 并可在那里自己勾选「始终使用此应用」。
 *
 * 只写 `HKCU`，不需要管理员权限。
 */
export async function setFileAssociation(
  ext: string,
  displayName: string,
  enable: boolean,
): Promise<void> {
  return invoke<void>('set_file_association', { ext, displayName, enable })
}

/**
 * 批量加入 / 移出「打开方式」列表（帮助面板的「全选」用）。
 *
 * 相比循环调用 [`setFileAssociation`]：
 * 1. 后端只在循环结束后广播**一次** `SHCNE_ASSOCCHANGED`
 *    （单次调用每次都广播，7 个扩展名就是 7 次 shell 通知）；
 * 2. 返回**失败的扩展名列表**，前端不用自己聚合多次调用的结果。
 *
 * @returns 失败的扩展名（空数组 = 全部成功）。全失败时后端直接抛错。
 */
export async function setFileAssociationBulk(
  items: ReadonlyArray<AssociationItem>,
  enable: boolean,
): Promise<string[]> {
  return invoke<string[]>('set_file_association_bulk', { items, enable })
}

/**
 * 用系统默认浏览器打开一个链接（信息面板「在地图中查看」用）。
 *
 * 后端**只放行 http/https**（其余前缀直接报错）—— ShellExecute 对
 * `file:`、网络共享、可执行文件一律照常执行，白名单必须拦在后端，
 * 前端校验是可以被绕过的。
 *
 * 失败会抛出可读错误（例如「没有可用的默认浏览器」），调用方按需提示。
 */
export async function openExternalUrl(url: string): Promise<void> {
  return invoke<void>('open_external_url', { url })
}

/**
 * 把一条诊断信息打到后端日志（Rust 的 tracing → stderr）。
 *
 * **为什么需要**：发布版不挂 devtools，WebView2 里的 `console.log` 根本看不到，
 * 而「解码花了多久」「走的是哪条 IPC 通道」这类问题只能从 WebView2 侧取数。
 * 用 `PHOTOVAULT_LOG=info photovault.exe` 启动即可在终端看到。
 *
 * 仅诊断用，失败静默 —— 绝不能影响主流程。
 */
export function traceLog(msg: string): void {
  void invoke('trace_log', { msg }).catch(() => undefined)
}
