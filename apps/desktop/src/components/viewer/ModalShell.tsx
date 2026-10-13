/**
 * 共用的居中模态外壳（帮助 / 设置两个面板都用它）。
 *
 * ## 为什么要抽出来
 * 拆成「帮助」「设置」两个面板后，二者的外壳**完全一样**（遮罩 + 卡片 +
 * 标题栏 + 内容区 + 右上角关闭）。外壳各写一份的话，改动很容易只改一处，
 * 久了两个面板外观就分叉了（高度、内边距、圆角、z-index 全对不上）。
 * 所以把外壳收在这里，内容由各面板通过 `children` 填。
 *
 * 关闭方式三种，符合直觉：
 * - 右上角 `×`
 * - 点击卡片外的遮罩
 * - `Esc`（由 `App` 在快捷键链最前面处理）
 */

import type { ReactNode } from 'react'

interface ModalShellProps {
  /** 卡片标题（同时用作 `aria-label`） */
  title: string
  onClose: () => void
  children: ReactNode
  /** 卡片宽度，`default` = 帮助/设置通用；`wide` 给内容更长的场景 */
  width?: 'default' | 'wide'
}

export function ModalShell({ title, onClose, children, width = 'default' }: ModalShellProps) {
  return (
    <div className="pv-modal-backdrop" onClick={onClose}>
      <div
        className={`pv-modal${width === 'wide' ? ' pv-modal-wide' : ''}`}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        /* 卡片内部点击不该关闭浮层 */
        onClick={(e) => e.stopPropagation()}
      >
        <div className="pv-modal-head">
          <span className="pv-modal-title">{title}</span>
          <button
            className="pv-info-close"
            onClick={onClose}
            aria-label={`关闭${title}`}
            title="关闭"
          >
            ×
          </button>
        </div>
        <div className="pv-modal-body">{children}</div>
      </div>
    </div>
  )
}
