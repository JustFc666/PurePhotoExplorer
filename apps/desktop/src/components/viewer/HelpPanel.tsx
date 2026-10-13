/**
 * 帮助浮层（居中模态）—— **只放快捷键速查**。
 *
 * ## 为什么从「快捷键 + 设置 + 关于」里拆出来（2026-10-11）
 * 三样东西混在一个面板里时，「查阅快捷键」这种一次性动作被「改设置」拖累：
 * 打开设置要等注册表查询，用户想确认一个按键却要先经过一屏无关内容。
 * 拆开后：
 * - **帮助** = 纯查阅，打开即显示、零等待、关掉不留痕迹；
 * - **设置** = 默认打开方式 + 关于（见 `SettingsPanel`）。
 *
 * 两者的外壳（遮罩/卡片/标题栏/关闭）共用 [`ModalShell`]，样式也共用
 * `pv-modal-*` 一套，避免外观慢慢分叉。
 *
 * 外壳为什么做成「模态」而不是像信息/调整那样的角落浮层：帮助是一次性的
 * 「查阅后关闭」场景，居中 + 半透明遮罩能让用户一眼确认
 * 「这是个需要先关掉的东西」，不会与画面混在一起。
 */

import { ModalShell } from './ModalShell'

interface HelpPanelProps {
  onClose: () => void
}

/** 快捷键分组：[组名, [按键, 说明][]] */
const SHORTCUTS: ReadonlyArray<readonly [string, ReadonlyArray<readonly [string, string]>]> = [
  [
    '浏览',
    [
      ['空格 / → / PageDown', '下一张'],
      ['← / PageUp / Shift+空格', '上一张'],
      ['滚轮上下', '翻页'],
      ['Ctrl + O', '打开图片'],
      ['Ctrl + P', '打印预览（确认版面后送印，含裁剪与旋转）'],
      ['F5', '刷新（重读磁盘上的当前图片，并重新列目录）'],
    ],
  ],
  [
    '视图',
    [
      ['Ctrl + 滚轮', '以光标为中心缩放'],
      ['+ / −', '放大 / 缩小'],
      ['0', '适应窗口'],
      ['1', '100% 原始像素'],
      ['双击画面', '100% ↔ 适应窗口'],
      ['[ / ]', '向左 / 向右旋转 90°'],
      ['↑ / ↓', '向上 / 向下平移'],
    ],
  ],
  [
    '裁剪与调整',
    [
      ['拖框四角', '改裁剪框大小（固定比例时锁死宽高比）'],
      ['拖框内部', '平移裁剪框（画面不动）'],
      ['画布比例 1:1 / 16:9 …', '按比例裁剪，预置框居中'],
      ['A', '打开画布调整面板'],
      ['Esc', '取消裁剪 → 退出全屏 → 适应窗口'],
    ],
  ],
  [
    '界面',
    [
      ['I', '图片信息'],
      ['F', '全屏切换'],
      ['M', '最大化 / 还原窗口'],
      ['H / F1', '打开本帮助'],
      ['工具栏「设置」', '默认打开方式与关于'],
    ],
  ],
  [
    '动图（GIF）',
    [
      ['工具栏「暂停 / 播放」', '暂停或继续播放动图'],
      ['状态栏右下', '显示当前帧号 / 总帧数'],
    ],
  ],
]

export function HelpPanel({ onClose }: HelpPanelProps) {
  return (
    <ModalShell title="帮助" onClose={onClose}>
      {SHORTCUTS.map(([group, rows]) => (
        <section className="pv-modal-section" key={group}>
          <h3 className="pv-modal-group">{group}</h3>
          <dl className="pv-modal-list">
            {rows.map(([key, desc]) => (
              <div className="pv-modal-row" key={key}>
                <dt>
                  <kbd className="pv-kbd">{key}</kbd>
                </dt>
                <dd>{desc}</dd>
              </div>
            ))}
          </dl>
        </section>
      ))}
    </ModalShell>
  )
}
