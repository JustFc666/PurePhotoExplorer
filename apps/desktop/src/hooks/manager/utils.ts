/**
 * 图库模式的小工具：路径归一 + 错误文案（转发 `lib/errText`）。
 *
 * 路径相关的两个函数留在这一层，是因为它们只服务图库模式；
 * `errText` 是通用的，实现放在 `lib/errText.ts`，这里只是顺手转发，
 * 让图库内部代码只 import 一个 `./manager/utils`。
 */

import type { FolderNode } from '../../ipc/manager'

export { errText } from '../../lib/errText'

/**
 * 索引里的文件夹 → 绝对路径。
 *
 * `folders` 存的是「卷挂载点 + 相对路径」，拼起来才能得到绝对路径
 * （**索引里不存盘符**是这个项目的既定设计，手册 §5.5）。
 * 重扫一个已入库目录时要用它还原路径。
 */
export function absOfFolder(f: FolderNode): string | null {
  if (!f.mountPath) return null
  const base = f.mountPath.replace(/\\/g, '/').replace(/\/+$/, '')
  return f.relPath ? `${base}/${f.relPath}` : base
}

/** 路径归一化：`\`→`/`、去掉尾部斜杠、转小写（Windows 路径不区分大小写）。 */
export function normPath(p: string): string {
  return p.replace(/\\/g, '/').replace(/\/+$/, '').toLowerCase()
}
