/** 顶部工具栏（同时充当窗口标题栏 —— 见 decorations: false 自绘方案） */

import type { MouseEvent as ReactMouseEvent } from 'react'
import { ContextMenu, useContextMenu } from './ContextMenu'
import { WindowControls } from './WindowControls'

/**
 * 应用 logo —— 「圆角方框 + 山峰折线」，外形 **4:3**（宽:高）。
 *
 * 用**内联 SVG** 而不是位图：任意 DPI 下都锐利，且描边走 `currentColor`，
 * 于是 CSS 一条 `color: #fff` 就能把线条染成白色（用户要求「线条变白色」）；
 * 底色来自 `.pv-logo` 的 `background`（= 标题栏底色，用户要求）。
 *
 * `viewBox` 直接取 32×24（= 4:3），与 `.pv-logo` 的 `aspect-ratio: 4/3` 一致，
 * 所以图标**不会被拉伸**，图形本身就是 4:3 的横幅。
 * 尺寸交给 CSS（高度 28px → 宽 37.33px），这里只画形状。
 */
export function LogoTile() {
  return (
    <span className="pv-logo" role="img" aria-label="PurePhotoExplorer">
      <svg viewBox="0 0 32 24" fill="none">
        {/* 外框：4:3 圆角方框 */}
        <rect
          x="4.53"
          y="3.4"
          width="22.93"
          height="17.2"
          rx="4.5"
          stroke="currentColor"
          strokeWidth="2.4"
        />
        {/* 内部：山峰折线（大峰 → 谷 → 小峰，两端落到底边附近） */}
        <path
          d="M7.5 18.2 13.1 10.6 17.1 14.8 21.2 12.3 24.8 17.6"
          stroke="currentColor"
          strokeWidth="2.4"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      </svg>
    </span>
  )
}

interface ToolbarProps {
  onOpen: () => void
  onAction: (action: string) => void
  hasImage: boolean
  rawEnabled: boolean
  /** 上一张 / 下一张（同目录浏览） */
  onPrev: () => void
  onNext: () => void
  canPrev: boolean
  canNext: boolean
  /** 全屏切换（F） */
  onToggleFullscreen: () => void
  isFullscreen: boolean
  /** 图片信息浮层（I） */
  onToggleInfo: () => void
  showInfo: boolean
  /** 画布调整浮层 */
  onToggleAdjust: () => void
  showAdjust: boolean
  /** 帮助浮层（快捷键速查） */
  onToggleHelp: () => void
  showHelp: boolean
  /** 设置浮层（默认打开方式 + 关于） */
  onToggleSettings: () => void
  showSettings: boolean
  /** 幻灯片放映开 / 关（S） */
  onToggleSlideshow: () => void
  /** 正在放映（「放映」菜单文案与状态） */
  slideshow: boolean
  /**
   * 打印当前图像（Ctrl+P）。
   *
   * 2026-10-10 起**只是打开预览浮层**，不直接送印 ——
   * 确认版面后由浮层里的「打印…」才真正弹系统对话框。
   */
  onPrint: () => void
  /** 打印进行中（禁用按钮，避免连点弹多个对话框） */
  printing: boolean
  /**
   * 动图播放状态。非动图（含静态 GIF、普通图片）时传 `null` ——
   * 此时工具栏不渲染播放按钮。
   */
  animation: {
    playing: boolean
    frameIndex: number
    frameCount: number
  } | null
  /** 播放 / 暂停切换（仅动图时有意义） */
  onTogglePlay: () => void
  /** 空白区域按下 → 拖动窗口 / 双击最大化 */
  onDragMouseDown: (e: ReactMouseEvent) => void
  /** 窗口控制按钮 */
  isMaximized: boolean
  onMinimize: () => void
  onToggleMaximize: () => void
  onClose: () => void
  /** 切到图库模式（S2 管理器） */
  onOpenManager?: () => void
}

export function Toolbar({
  onOpen,
  onAction,
  hasImage,
  rawEnabled,
  onPrev,
  onNext,
  canPrev,
  canNext,
  onToggleFullscreen,
  isFullscreen,
  onToggleInfo,
  showInfo,
  onToggleAdjust,
  showAdjust,
  onToggleHelp,
  showHelp,
  onToggleSettings,
  showSettings,
  onToggleSlideshow,
  slideshow,
  onPrint,
  printing,
  animation,
  onTogglePlay,
  onDragMouseDown,
  isMaximized,
  onMinimize,
  onToggleMaximize,
  onClose,
  onOpenManager,
}: ToolbarProps) {
  /** 「放映」下拉菜单的开关状态（Portal 渲染，不影响工具栏布局） */
  const slideMenu = useContextMenu()
  return (
    <header className="pv-toolbar" onMouseDown={onDragMouseDown}>
      <LogoTile />

      <button className="pv-btn pv-btn-primary" onClick={onOpen}>
        {/* 「打 开」两个字中间的间隔：字保持 16px，间隔单独定为 12px。
            用一个 `inline-block` 占位 span 而不是空格字符 ——
            宽度与字号、字体都解耦，能精确到 12px；
            外层 `span` 是必须的：Flex 容器的 `gap` 会给每个直接子项之间留缝，
            若把「打」/占位/「开」放在同一层会多出两段 gap。 */}
        <span className="pv-open-label">
          打<span className="pv-open-gap" />开
        </span>
      </button>

      <div className="pv-divider" />

      <button
        className="pv-btn"
        disabled={!canPrev}
        onClick={onPrev}
        title="上一张 (← / PageUp / Shift+空格 / 滚轮向上)"
        aria-label="上一张"
      >
        上一张
      </button>
      <button
        className="pv-btn"
        disabled={!canNext}
        onClick={onNext}
        title="下一张 (空格 / → / PageDown / 滚轮向下)"
        aria-label="下一张"
      >
        下一张
      </button>

      <div className="pv-divider" />

      <button
        className="pv-btn"
        disabled={!hasImage}
        onClick={() => onAction('zoom-out')}
        title="缩小 (-)"
        aria-label="缩小"
      >
        缩小
      </button>
      <button
        className="pv-btn"
        disabled={!hasImage}
        onClick={() => onAction('zoom-in')}
        title="放大 (+)"
        aria-label="放大"
      >
        放大
      </button>
      <button
        className="pv-btn"
        disabled={!hasImage}
        onClick={() => onAction('actual')}
        title="原始像素 (1)"
        aria-label="原始像素"
      >
        1:1
      </button>
      <button
        className="pv-btn"
        disabled={!hasImage}
        onClick={() => onAction('fit')}
        title="适应窗口 (0)"
        aria-label="适应窗口"
      >
        适应
      </button>

      <div className="pv-divider" />

      <button
        className="pv-btn"
        disabled={!hasImage}
        onClick={() => onAction('rotate-left')}
        title="向左旋转"
        aria-label="向左旋转"
      >
        左转
      </button>
      <button
        className="pv-btn"
        disabled={!hasImage}
        onClick={() => onAction('rotate-right')}
        title="向右旋转"
        aria-label="向右旋转"
      >
        右转
      </button>
      <button
        className="pv-btn"
        disabled={!hasImage}
        onClick={onToggleAdjust}
        title="调整角度与画布比例 (A)"
        aria-pressed={showAdjust}
        aria-label="调整"
      >
        调整
      </button>

      <div className="pv-divider" />

      <button
        className="pv-btn"
        onClick={onToggleFullscreen}
        title="全屏 (F)"
        aria-pressed={isFullscreen}
        aria-label="全屏"
      >
        {isFullscreen ? '退出全屏' : '全屏'}
      </button>
      <button
        className="pv-btn"
        onClick={onToggleInfo}
        title="图片信息 (I)"
        aria-pressed={showInfo}
        aria-label="图片信息"
      >
        信息
      </button>
      {/*
        「放映」菜单（2026-10-19 用户要求）。
        单图模式的工具栏没有菜单栏，而这个功能下有两个入口（放映 / 设置间隔），
        所以做成**下拉菜单**而不是又一个平铺按钮 —— 平铺会再多挤一格。
      */}
      <button
        className="pv-btn"
        disabled={!hasImage}
        // 阻止冒泡：① 别让标题栏的拖动逻辑把这次按下当成"拖窗口"；
        // ② 菜单开着时再点按钮，window 的 mousedown 会先关掉再被 click 打开（闪一下）
        onMouseDown={(e) => e.stopPropagation()}
        onClick={(e) => slideMenu.open(e)}
        title="放映（S）—— 全屏自动翻页"
        aria-haspopup="menu"
        aria-expanded={slideMenu.state !== null}
        aria-label="放映"
      >
        放映
      </button>
      <button
        className="pv-btn"
        disabled={!hasImage || printing}
        onClick={onPrint}
        title="打印（Ctrl+P）—— 先看版面预览，确认后弹系统打印对话框"
        aria-label="打印"
      >
        {printing ? '打印中…' : '打印'}
      </button>
      {onOpenManager && (
        <button
          className="pv-btn"
          onClick={onOpenManager}
          title="回到图库（停在离开时浏览的位置）"
          aria-label="图库"
        >
          图库
        </button>
      )}
      <button
        className="pv-btn"
        onClick={onToggleHelp}
        title="帮助：快捷键速查 (H / F1)"
        aria-pressed={showHelp}
        aria-label="帮助"
      >
        帮助
      </button>
      <button
        className="pv-btn"
        onClick={onToggleSettings}
        title="设置：默认打开方式与关于"
        aria-pressed={showSettings}
        aria-label="设置"
      >
        设置
      </button>

      {/* 动图播放控制：只在真的有多帧时出现，静态图不占位置 */}
      {animation && (
        <button
          className="pv-btn"
          onClick={onTogglePlay}
          title={animation.playing ? '暂停动图 (空格保持翻页，这里单独控制)' : '播放动图'}
          aria-pressed={animation.playing}
          aria-label={animation.playing ? '暂停动图' : '播放动图'}
        >
          {animation.playing ? '暂停' : '播放'}
          <span className="pv-btn-sub">
            {animation.frameIndex + 1}/{animation.frameCount}
          </span>
        </button>
      )}

      <div className="pv-spacer" />

      {!rawEnabled && (
        <span className="pv-hint" title="需在 S3 阶段接入 LibRaw">
          相机 RAW 暂不支持
        </span>
      )}

      {/* 自绘窗口控制按钮（decorations: false 后系统标题栏已移除） */}
      <WindowControls
        isMaximized={isMaximized}
        onMinimize={onMinimize}
        onToggleMaximize={onToggleMaximize}
        onClose={onClose}
      />

      {/* 「放映」下拉菜单（Portal 到 body，不参与工具栏布局） */}
      {slideMenu.state && (
        <ContextMenu
          key={`${slideMenu.state.x},${slideMenu.state.y}`}
          position={{ x: slideMenu.state.x, y: slideMenu.state.y }}
          items={[
            {
              id: 'slide-toggle',
              label: slideshow ? '停止放映' : '开始放映',
              hint: 'S',
              onSelect: onToggleSlideshow,
            },
            {
              id: 'slide-settings',
              label: '放映设置…',
              hint: '间隔 / 排序',
              separatorBefore: true,
              onSelect: onToggleSettings,
            },
          ]}
          onClose={slideMenu.close}
        />
      )}
    </header>
  )
}
