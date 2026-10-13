/**
 * 设置浮层（居中模态）—— **默认打开方式 + 关于**。
 *
 * ## 为什么从帮助里拆出来（2026-10-11）
 * 「查一个快捷键」是纯查阅，「改默认打开方式」是修改系统的设置行为 ——
 * 两者的风险与节奏完全不同，混在一个面板里会：
 * - 让查阅变得有等待感（打开设置要先查注册表）；
 * - 让用户不敢随手点开帮助（怕误改关联）。
 *
 * 所以拆成两个面板：帮助只管快捷键，设置管这两项。
 *
 * ## 为什么关联状态要**打开时才查**
 * 注册表读取虽快，但这是**低频设置**，没必要占用启动路径；
 * 而且用户可能在外部改过关联，每次打开都重读更准。
 */

import { useEffect, useRef } from 'react'
import { ASSOCIATION_FORMATS } from '../../constants/formats'
import type { AssociationState } from '../../ipc'
import { ModalShell } from './ModalShell'

interface SettingsPanelProps {
  onClose: () => void
  /**
   * 文件关联状态；`null` = 尚未取到（正在查询）。
   *
   * 含 `registered`（已勾选哪些扩展名）与 `note`（Windows 的限制说明，
   * 由 Rust 侧提供，避免前后端两套说法）。
   */
  associations: AssociationState | null
  /** 勾选 / 取消某个扩展名的关联 */
  onToggleAssociation: (ext: string, label: string, enable: boolean) => void
  /** 一次性全选 / 全不选所有可关联格式（`enable` 是**目标状态**） */
  onToggleAllAssociations: (enable: boolean) => void
  /** 关联请求进行中（期间禁用复选框，避免连点） */
  associationBusy: boolean
  /** 关联操作的错误提示（成功时为 null） */
  associationError: string | null
}

/**
 * 可加入「打开方式」的扩展名 —— 从共享清单派生，**不要在这里另写一份**。
 *
 * 曾经这里是硬编码的 `['jpg','jpeg','png','gif','bmp']`，而 `bmp`
 * 根本没有解码器：用户能把 bmp 关联给 PurePhotoExplorer，双击却打不开。
 * 现在唯一的写入点在 `src/constants/formats.ts`。
 */
const ASSOCIATION_TARGETS = ASSOCIATION_FORMATS

export function SettingsPanel({
  onClose,
  associations,
  onToggleAssociation,
  onToggleAllAssociations,
  associationBusy,
  associationError,
}: SettingsPanelProps) {
  const registered = new Set(associations?.registered ?? [])
  const total = ASSOCIATION_TARGETS.length
  const checkedCount = ASSOCIATION_TARGETS.filter(([ext]) => registered.has(ext)).length
  const allChecked = total > 0 && checkedCount === total
  /** 部分选中 → 全选框显示成「半选」态 */
  const partlyChecked = checkedCount > 0 && !allChecked
  const checksDisabled = associationBusy || associations === null

  /**
   * `indeterminate` 是 DOM 上的**属性**，React 没有对应的 prop，
   * 只能拿 ref 直接写。漏掉这一步，部分选中时会显示成完全未选，
   * 用户会以为还有东西没勾上却看不出来。
   */
  const allRef = useRef<HTMLInputElement>(null)
  useEffect(() => {
    if (allRef.current) allRef.current.indeterminate = partlyChecked
  }, [partlyChecked])

  return (
    <ModalShell title="设置" onClose={onClose}>
      {/* ── 默认打开方式 ── */}
      <section className="pv-modal-section">
        <h3 className="pv-modal-group">默认打开方式</h3>
        <p className="pv-setting-note">
          {associations?.note ?? '正在读取文件关联状态…'}
        </p>

        {/* 全选行：右侧给出「已选 n / N」，否则部分选中时用户得自己数格子 */}
        <div className="pv-setting-all">
          <label className="pv-check pv-check-all">
            <input
              ref={allRef}
              type="checkbox"
              checked={allChecked}
              disabled={checksDisabled}
              onChange={(e) => onToggleAllAssociations(e.target.checked)}
            />
            <span className="pv-check-label">全选</span>
          </label>
          <span className="pv-setting-all-count">
            已选 {checkedCount} / {total}
          </span>
        </div>

        <div className="pv-setting-checks">
          {ASSOCIATION_TARGETS.map(([ext, label]) => (
            <label className="pv-check" key={ext}>
              <input
                type="checkbox"
                checked={registered.has(ext)}
                disabled={checksDisabled}
                onChange={(e) => onToggleAssociation(ext, label, e.target.checked)}
              />
              <span className="pv-check-label">
                {label}
                <span className="pv-check-ext">.{ext}</span>
              </span>
            </label>
          ))}
        </div>
        {associationError && <p className="pv-setting-error">{associationError}</p>}
      </section>

      {/* ── 关于 ── */}
      <section className="pv-modal-section">
        <h3 className="pv-modal-group">关于</h3>
        <dl className="pv-modal-list">
          <div className="pv-modal-row">
            <dt className="pv-about-label">软件</dt>
            <dd>
              PurePhotoExplorer <span className="pv-about-version">{__APP_VERSION__}</span>
            </dd>
          </div>
          <div className="pv-modal-row">
            <dt className="pv-about-label">作者</dt>
            <dd>Justin F.</dd>
          </div>
          <div className="pv-modal-row">
            <dt className="pv-about-label">邮箱</dt>
            <dd>
              <span className="pv-about-mail">fc666@hotmail.com</span>
            </dd>
          </div>
          {/* GeoNames 为 CC BY 4.0，署名是使用条件（离线地名库的数据来源） */}
          <div className="pv-modal-row">
            <dt className="pv-about-label">地名数据</dt>
            <dd>
              <span className="pv-about-credit">GeoNames</span>
              <span className="pv-about-license">（CC BY 4.0）</span>
            </dd>
          </div>
        </dl>
      </section>
    </ModalShell>
  )
}
