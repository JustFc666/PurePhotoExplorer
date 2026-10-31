/**
 * 「我的电脑」里的磁盘卡片（从 `ManagerView.tsx` 拆出，2026-11）。
 *
 * 单击 = 高亮，**双击才进入**（用户 2026-10-31 定稿）。
 * 进入走的是和侧栏点盘符同一条 `gotoWithReveal` —— 会顺便把树展开定位过去，
 * 所以这里只抛 `onOpen`，不关心树怎么滚。
 */

import type { FsNode, TreeIcons } from '../../ipc/browse'
import type { ThumbTier } from '../../ipc/manager'
import { thumbWidthOf } from '../../hooks/manager/prefs'
import { DriveIcon } from '../icons/DriveIcon'

export type DriveGridProps = {
  nodes: FsNode[]
  /** 系统给的盘符图标（拿不到就用内置字形） */
  icons: TreeIcons | null
  /** 当前高亮的盘符路径 */
  selected: string | null
  /** 点一张卡片：再点一次同一张 = 取消高亮（传 null） */
  onSelect: (path: string | null) => void
  /** 双击 / 回车：进入该盘 */
  onOpen: (path: string, name: string) => void
  tier: ThumbTier
}

export function DriveGrid(props: DriveGridProps) {
  const { nodes, icons, selected, onSelect, onOpen, tier } = props

  return (
    <div className="pv-mg-grid" style={{ ['--pv-card' as string]: `${thumbWidthOf(tier)}px` }}>
      {nodes.map((d) => {
        // 侧栏树用的同一份系统图标（后端按盘符分发，系统盘/移动盘/光驱不同）
        const sysIcon = icons?.drives.find(([p]) => p === d.path)?.[1] ?? null
        const on = d.path !== null && selected === d.path
        return (
          <div
            key={d.path ?? d.name}
            className={`pv-mg-card is-dir${on ? ' is-selected' : ''}`}
            role="button"
            tabIndex={0}
            aria-pressed={on}
            title={`${d.path ?? d.name}（双击进入）`}
            onClick={() => {
              if (!d.path) return
              onSelect(on ? null : d.path)
            }}
            onDoubleClick={() => {
              if (d.path) onOpen(d.path, d.name)
            }}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && d.path) onOpen(d.path, d.name)
            }}
          >
            <div className="pv-mg-cardimg">
              <div className="pv-mg-dirface">
                {sysIcon ? (
                  <img
                    className="pv-mg-diricon pv-mg-driveimg"
                    src={sysIcon}
                    alt=""
                    draggable={false}
                  />
                ) : (
                  <DriveIcon className="pv-mg-diricon" />
                )}
              </div>
            </div>
            <div className="pv-mg-cardname">{d.name}</div>
            <div className="pv-mg-cardmeta">{d.note ?? '磁盘'}</div>
          </div>
        )
      })}
    </div>
  )
}
