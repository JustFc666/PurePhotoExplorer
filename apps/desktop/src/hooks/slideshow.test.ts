/**
 * 护栏：幻灯片放映 + 复制/移动 + 设置面板（2026-10-19，C 批次）。
 *
 * 三条最容易回归的点：
 * 1. **放映与查看器快捷键互斥** —— 两套都活着的话，空格既是「下一张」又是「暂停」，
 *    Esc 还会顺手退全屏、关浮层；
 * 2. **移动必须清理源索引** —— 文件搬走了、列表里还留着行，用户点开就是「文件不存在」；
 * 3. **设置改默认排序要立刻生效** —— `ManagerView` 常驻挂载，只写 localStorage 不通知，
 *    用户会觉得「设置没反应」。
 */

import { describe, expect, it } from 'vitest'
import slideSrc from './useSlideshow.ts?raw'
import managerSrc from './useManager.ts?raw'
// 排序偏好已拆到 `manager/prefs.ts`（2026-11）：事件名与初值校验都在那里，
// 所以护栏要查那个文件，而不是继续在 useManager 里找字符串。
import managerPrefsSrc from './manager/prefs.ts?raw'
import appSrc from '../App.tsx?raw'
import shortcutSrc from './useShortcuts.ts?raw'
import settingsSrc from '../components/settings/SettingsWindow.tsx?raw'
import viewSrc from '../components/manager/ManagerView.tsx?raw'
import ipcSrc from '../ipc/manager.ts?raw'
import {
  clampSlideSeconds,
  FX_MS_MAX,
  readSlideSeconds,
  SLIDE_FX_OPTIONS,
  SLIDE_MAX,
  SLIDE_MIN,
  SLIDE_STEP,
} from './useSlideshow'
import loaderSrc from './useImageLoader.ts?raw'
import toolbarSrc from '../components/chrome/Toolbar.tsx?raw'
import canvasSrc from '../components/viewer/ImageCanvas.tsx?raw'

describe('幻灯片放映', () => {
  it('没有 localStorage 时用 1 秒默认值，并收进 0.5–10 / 步长 0.5', () => {
    // node 环境下没有 localStorage：读不到也不能抛
    expect(readSlideSeconds()).toBe(1)
    expect(SLIDE_MIN).toBe(0.5)
    expect(SLIDE_MAX).toBe(10)
    expect(SLIDE_STEP).toBe(0.5)
    // 浮点尾巴要削掉：0.5 的整数倍之外一律吸附
    expect(clampSlideSeconds(0.3)).toBe(0.5)
    expect(clampSlideSeconds(99)).toBe(10)
    expect(clampSlideSeconds(1.5 + 0.5)).toBe(2)
  })

  it('放映期间查看器的快捷键整体关掉', () => {
    expect(appSrc).toContain("enabled: mode === 'viewer' && !slideshow")
  })

  it('键盘用捕获阶段，Esc/空格拦住不冒泡', () => {
    expect(slideSrc).toContain("window.addEventListener('keydown', onKey, true)")
    expect(slideSrc).toContain('e.stopPropagation()')
    // 推进后 +1 重新计时：只有一张图 / 索引未知时否则会卡住
    expect(slideSrc).toContain('setTick((v) => v + 1)')
  })

  it('S 键能开放映，右键菜单也有入口', () => {
    expect(shortcutSrc).toContain("=== 's'")
    expect(shortcutSrc).toContain('onToggleSlideshow')
    expect(appSrc).toContain("id: 'slideshow'")
  })

  it('末尾回到第一张（next() 在边界是静默返回，不能靠它循环）', () => {
    expect(appSrc).toContain('if (index + 1 < total)')
    expect(appSrc).toContain('const first = siblings[0]')
  })

  it('单图模式工具栏有「放映」下拉菜单', () => {
    // 菜单里两项：开始/停止放映 + 放映设置
    expect(toolbarSrc).toContain("id: 'slide-toggle'")
    expect(toolbarSrc).toContain("id: 'slide-settings'")
    expect(toolbarSrc).toContain('useContextMenu()')
  })

  it('图库/文件夹右键「幻灯片放映」走外部播放列表', () => {
    // 列表必须交给 useImageLoader：否则「下一张」只会在同目录里翻，第 2 张就跳出图库
    expect(viewSrc).toContain('onPlaySlideshow(playPaths, startPath)')
    expect(appSrc).toContain('onPlaySlideshow={playPlaylist}')
    expect(loaderSrc).toContain('playlistRef')
    expect(loaderSrc).toContain('PLAYLIST_DIR')
  })

  it('动图按自身循环时长停留（逐帧 delay 累加，不按固定间隔）', () => {
    expect(appSrc).toContain('animDurationMs')
    expect(appSrc).toContain('f.delayMs')
    // 计时器用「动图时长优先，否则设置间隔」，两者都再叠加特效时长
    expect(appSrc).toContain('animDurationMs ?? slideSeconds * 1000')
  })

  it('放映时隐藏左侧胶片条', () => {
    expect(appSrc).toContain('showRail && !slideshow')
  })

  it('放映特效三选一，且用 WAAPI 动画布（不碰内联 transform）', () => {
    expect(SLIDE_FX_OPTIONS.map((o) => o.value)).toEqual(['none', 'fade', 'slide'])
    expect(settingsSrc).toContain('applySlideFx')
    expect(canvasSrc).toContain('main.animate(')
    // 位移走独立的 `translate` 属性，不能动 transform（那里是缩放/平移/旋转）
    expect(canvasSrc).toContain('translate: `${shift}px 0px`')
  })

  it('滑动：新图从右滑入 + 旧图幻影从左侧滑出', () => {
    // 旧画面先被拷进**临时**幻影元素（换图前 canvas 上还是旧像素）
    expect(canvasSrc).toContain('ghostElRef')
    expect(canvasSrc).toContain('ctx.drawImage(src, 0, 0)')
    expect(canvasSrc).toContain("translate: `-${shift}px 0px`")
  })

  it('幻影是一次性的：用完必须从 DOM 摘掉', () => {
    // ⛔ 常驻幻影 + 动画被 cancel → 回到 opacity 1 卡在画面上（2026-10-19 线上 bug）
    expect(canvasSrc).toContain('ghost?.remove()')
    expect(canvasSrc).toContain("ghost.className = 'pv-canvas-img pv-slide-ghost'")
  })

  it('幻影动画必须 fill: forwards + onfinish 摘除', () => {
    // ⛔ WAAPI 默认 fill:none → 播完弹回 opacity 1，上一张整幅糊在新图上，
    // 一直糊到下一次换图才被清掉（用户看到"中间一条长图"就是这个）
    expect(canvasSrc).toContain("fill: 'forwards'")
    expect(canvasSrc).toContain('g.onfinish = finishGhost')
  })

  it('effect 依赖里不能有 view.scale（新图 fit 会改它，导致动画被中途取消）', () => {
    expect(canvasSrc).toContain('}, [image, fx, fxMs])')
    expect(canvasSrc).not.toContain('view.scale])')
    expect(canvasSrc).toContain('scaleRef.current')
  })

  it('特效时长可调（方案 A：一个值两种特效共用）', () => {
    expect(settingsSrc).toContain('FX_MS_MIN')
    expect(settingsSrc).toContain('type="range"')
    expect(canvasSrc).toContain('fxMs')
    // 用户 2026-10-19：特效最长 2 秒
    expect(FX_MS_MAX).toBe(2000)
  })

  it('开始放映：进全屏 → 刷新当前图 → 停 1 秒 → 才开演', () => {
    expect(appSrc).toContain('const SLIDE_WARMUP_MS = 1000')
    // 预热期计时器不能跑，否则第一张 1 秒就被切走
    expect(appSrc).toContain('active: slideshow && slideReady')
    expect(appSrc).toContain('void reload()')
    // 退出放映必须清预热定时器，否则 1 秒后自己"开始"
    expect(appSrc).toContain('window.clearTimeout(slideWarmupTimer.current)')
  })

  it('间隔是纯停留时间，特效时间另算', () => {
    expect(appSrc).toContain("+ (slideFx === 'none' ? 0 : fxMs)")
  })
})

describe('复制 / 移动到文件夹', () => {
  it('后端两条命令都注册了，复制走字节复制', () => {
    expect(ipcSrc).toContain("invoke<BatchSummary>('manager_copy_to_folder'")
    expect(ipcSrc).toContain("invoke<BatchSummary>('manager_move_to_folder'")
  })

  it('移动之后必须刷新（源目录少了一批图）', () => {
    expect(viewSrc).toContain('managerMoveToFolder(')
    expect(viewSrc).toMatch(/managerMoveToFolder\([\s\S]{0,260}refreshAfterMutation\(\)/)
  })

  it('移动要二次确认，且说明会从图库索引移除', () => {
    expect(viewSrc).toContain("title: '移动到文件夹'")
    expect(viewSrc).toContain('真的从原位置搬走')
  })
})

describe('设置面板', () => {
  it('默认排序改完要通知常驻挂载的图库视图', () => {
    expect(settingsSrc).toContain('applySortPref')
    expect(managerPrefsSrc).toContain("export const EVENT_SET_SORT = 'pv:set-sort'")
    expect(managerSrc).toContain('window.addEventListener(EVENT_SET_SORT, onSet)')
    // 初值来自持久化的偏好，不是硬编码
    expect(managerSrc).toContain('useState<ManagerSort>(readSortPref)')
    // 图库自己换排序只写不广播 —— 广播的话监听端会再被自己触发一次
    expect(managerSrc).toContain('writeSortPref(s)')
  })

  it('缩略图缓存能读能清', () => {
    expect(settingsSrc).toContain('managerCacheStats()')
    expect(settingsSrc).toContain('managerClearCache()')
    expect(ipcSrc).toContain("invoke<CacheStats>('manager_clear_cache'")
  })

  it('放映间隔改完立刻生效', () => {
    expect(settingsSrc).toContain('saveSlideSeconds')
    expect(slideSrc).toContain("export const EVENT_SET_SLIDE_SECONDS = 'pv:set-slide-seconds'")
    expect(appSrc).toContain('EVENT_SET_SLIDE_SECONDS')
  })
})

/**
 * 护栏：单图浏览不该出现「白框」（2026-10-19 用户反馈）。
 *
 * `.pv-crop-frame` 带 1px 白描边 + 四周 45% 压暗，以前**无条件渲染** ——
 * 结果每张照片都像被框住。现在只在裁剪语境里画。
 */
describe('单图浏览：裁剪框只在裁剪语境显示', () => {
  it('调整浮层 / 固定比例 / 已拖过框 三者之一才画框', () => {
    expect(canvasSrc).toContain(
      "frameBox && (cropUi || view.aspect !== 'free' || view.frameRect !== null)",
    )
  })

  it('调整浮层的开关由 App 传进来', () => {
    expect(appSrc).toContain('cropUi={showAdjust}')
  })
})
