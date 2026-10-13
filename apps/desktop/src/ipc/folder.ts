/**
 * 文件夹浏览相关命令。
 */

import { invoke } from '@tauri-apps/api/core'

/** 列出 `path` 所在文件夹内的图片（按文件名升序），用于上一张/下一张 */
export async function listSiblings(path: string): Promise<string[]> {
  return invoke<string[]>('list_siblings', { path })
}
