/**
 * 主应用组件 —— S1 单图浏览的**组合根**。
 *
 * 只负责装配：
 * - 状态与行为来自 hooks（`useImageLoader` / `useImageView` / `useDragDrop` / `useShortcuts`）；
 * - 渲染交给子组件（`Toolbar` / `ImageCanvas` / `EmptyState` / `StatusBar`）。
 *
 * 视图控制器（`useImageView`）上提到此处，是为了让**缩放控件**能放进底部信息栏
 * 而不必从画布内部向上传递状态。
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog'
import { ManagerView } from './components/manager/ManagerView'
import { getCurrentWindow } from '@tauri-apps/api/window'
import {
  decodeError,
  EXPORT_FORMATS,
  formatFileSize,
  getAppInfo,
  getAssociationState,
  getStartupFile,
  isWritableFormat,
  notWritableReason,
  printImage,
  rawExportHint,
  saveEditedImage,
  setFileAssociation,
  setFileAssociationBulk,
  type AppInfo,
  type AssociationState,
  type DecodedImage,
} from './ipc'
import { useDragDrop } from './hooks/useDragDrop'
import { getLibDropHover } from './hooks/libDrop'
import {
  clampFxMs,
  clampSlideSeconds,
  EVENT_SET_SLIDE_FX,
  EVENT_SET_SLIDE_FX_MS,
  EVENT_SET_SLIDE_SECONDS,
  readFxMs,
  readSlideFx,
  readSlideSeconds,
  useSlideshow,
  type SlideFx,
} from './hooks/useSlideshow'
import { useImageLoader } from './hooks/useImageLoader'
import { useAnimationPlayer } from './hooks/useAnimationPlayer'
import { useImageView } from './hooks/useImageView'
import { useWheelNavigation } from './hooks/useWheelNavigation'
import { useShortcuts } from './hooks/useShortcuts'
import { useWindowDrag } from './hooks/useWindowDrag'
import { ImageCanvas } from './components/viewer/ImageCanvas'
import { ThumbRail } from './components/viewer/ThumbRail'
import { EmptyState } from './components/viewer/EmptyState'
import { InfoPanel } from './components/viewer/InfoPanel'
import { HelpPanel } from './components/viewer/HelpPanel'
import { SettingsWindow, type SettingsPage } from './components/settings/SettingsWindow'
import { PrintPreview, type PaperSize, type PrintOrientation, type PrintQuality } from './components/viewer/PrintPreview'
import { ContextMenu, useContextMenu } from './components/chrome/ContextMenu'
import type { ContextMenuItem } from './components/chrome/ContextMenu'
import { AdjustPanel, type SaveState } from './components/viewer/AdjustPanel'
import { Toolbar } from './components/chrome/Toolbar'
import { StatusBar } from './components/chrome/StatusBar'
import {
  ASSOCIATION_EXTS,
  ASSOCIATION_FORMATS,
  SUPPORTED_EXTS,
  UNVERIFIED_RAW_EXTS,
  VERIFIED_RAW_EXTS,
} from './constants/formats'
import { ResizeHandles } from './components/chrome/ResizeHandles'

/** 左侧胶片条的开/关偏好（localStorage） */
const RAIL_PREF_KEY = 'pv.viewer.rail'
/**
 * 放映开始前的「预热」时长（毫秒）。
 *
 * 进全屏那一刻视口尺寸会变，画布得按新尺寸重新适应；用户要求「先刷新图片、
 * 停 1 秒再开始播放」，这样第一张不会以没摆正的样子一闪而过。
 */
const SLIDE_WARMUP_MS = 1000
/**
 * 读偏好。默认**开** —— 只有明确存过 `'0'` 才算关。
 * localStorage 不可用（隐私模式）时按默认值处理，不该因此崩掉整个 App。
 */
function readRailPref(): boolean {
  try {
    return localStorage.getItem(RAIL_PREF_KEY) !== '0'
  } catch {
    return true
  }
}

/** 「打开」对话框的文件类型过滤器 */
const DIALOG_FILTERS = [
  { name: '支持的图片', extensions: [...SUPPORTED_EXTS] },
  { name: 'JPEG', extensions: ['jpg', 'jpeg'] },
  { name: 'PNG', extensions: ['png'] },
  { name: 'HEIC（苹果等手机）', extensions: ['heic', 'heif'] },
  { name: 'Photoshop PSD', extensions: ['psd'] },
  { name: 'GIF', extensions: ['gif'] },
  // RAW 过滤器含"未实测"的那些：用户手上有 DNG/NEF 时得能选中去试
  { name: '相机 RAW', extensions: [...VERIFIED_RAW_EXTS, ...UNVERIFIED_RAW_EXTS] },
  { name: '所有文件', extensions: ['*'] },
]

export function App() {
  const {
    image,
    animation,
    info,
    status,
    load,
    reload,
    refreshSiblings,
    setPlaylist,
    path: imagePath,
    fail,
    index,
    total,
    siblings,
    canPrev,
    canNext,
    prev,
    next,
  } = useImageLoader()
  /**
   * GIF 播放器。
   *
   * 注意「是否动图」的判据是 `player.isAnimated`（帧数 > 1），
   * 而不是 `animation !== null` —— 静态 GIF 同样会产生 `animation`。
   *
   * 第二个参数（重置键）传**图片路径**：动图是渐进式加载的，后台批次会
   * 不断往 `animation.frames` 追加（每次都换成新对象），若拿对象本身当
   * 「换图了」的判据，每来一批就会把播放位置弹回第 0 帧。
   */
  const player = useAnimationPlayer(animation, info?.path ?? null)
  /** 真正要画在画布上的图：动图取当前帧，否则就是解码结果本身 */
  const displayImage = player.frame ?? image
  const [appInfo, setAppInfo] = useState<AppInfo | null>(null)
  const [isFullscreen, setIsFullscreen] = useState(false)
  /**
   * 视图模式：`viewer` = 单图浏览（S1），`manager` = 图库模式（S2）。
   *
   * 两者**共用同一窗口**（手册 §2.1 D1），所以 manager 只是在最上层盖一个
   * 全屏视图；切回 viewer 时单图浏览的状态（当前图/缩放/位置）都还在。
   *
   * ⚠️ 默认值是 **`manager`**（用户 2026-10-15 明确要求「打开默认进图库」）：
   * 这个应用的主要用途是**管理**照片，看图是从图库里双击进去的动作。
   */
  const [mode, setMode] = useState<'viewer' | 'manager'>('manager')
  /** 图片信息浮层是否展开 */
  const [showInfo, setShowInfo] = useState(false)
  /** 画布调整浮层是否展开 */
  const [showAdjust, setShowAdjust] = useState(false)
  /** 帮助浮层（快捷键速查）是否展开 */
  const [showHelp, setShowHelp] = useState(false)
  /** 设置浮层（默认打开方式 + 关于）是否展开 */
  const [showSettings, setShowSettings] = useState(false)
  /** 从齿轮进来时要直接落到哪一页（默认「数据位置」） */
  const [settingsPage, setSettingsPage] = useState<SettingsPage>('data')

  /** 文件关联状态（设置浮层的「默认打开方式」区回显） */
  const [associations, setAssociations] = useState<AssociationState | null>(null)
  const [associationBusy, setAssociationBusy] = useState(false)
  const [associationError, setAssociationError] = useState<string | null>(null)

  /**
   * 查询文件关联状态。
   *
   * ⚠️ **打开设置时才查**（而不是启动时、也不是打开帮助时）：
   * 注册表读取虽快，但这是**低频设置**，没必要占用启动路径；
   * 而且用户可能在外部改过关联，每次打开都重读更准。
   */
  const refreshAssociations = useCallback(async () => {
    try {
      setAssociations(await getAssociationState(ASSOCIATION_EXTS))
      setAssociationError(null)
    } catch (err) {
      setAssociations(null)
      setAssociationError(decodeError(err, '读取文件关联失败').message)
    }
  }, [])

  useEffect(() => {
    if (showSettings) void refreshAssociations()
  }, [showSettings, refreshAssociations])

  /**
   * 「全选 / 全不选」所有可关联格式。
   *
   * **只处理需要变更的那些**（拿 `registered` 求差集）—— 幂等重复写注册表
   * 没有收益，还平添失败面。全部已一致时直接返回，不惊动后端。
   *
   * 一次性提交给后端的批量命令：它走静默写入、末尾只广播**一次**
   * `SHCNE_ASSOCCHANGED`（逐个调用单扩展名命令会广播 N 次）。
   */
  const onToggleAllAssociations = useCallback(
    async (enable: boolean) => {
      const registered = new Set(associations?.registered ?? [])
      const pending = ASSOCIATION_FORMATS.filter(([ext]) => registered.has(ext) !== enable)
      if (pending.length === 0) return

      setAssociationBusy(true)
      setAssociationError(null)
      try {
        const failed = await setFileAssociationBulk(
          pending.map(([ext, label]) => ({ ext, displayName: `${label} 图片` })),
          enable,
        )
        // 同样以注册表的真实状态为准回显
        await refreshAssociations()
        if (failed.length > 0) {
          setAssociationError(
            `以下格式设置失败：${failed.map((e) => `.${e}`).join('、')}`,
          )
        }
      } catch (err) {
        setAssociationError(decodeError(err, '设置文件关联失败').message)
      } finally {
        setAssociationBusy(false)
      }
    },
    [associations, refreshAssociations],
  )

  /**
   * 勾选 / 取消「打开方式」关联。
   *
   * 写的是 `HKCU`（不需要管理员），且**只是加入「打开方式」列表**，
   * 不是设为默认 —— Windows 不允许程序静默改默认程序，详见 Rust 侧 `win_assoc`。
   */
  const onToggleAssociation = useCallback(
    async (ext: string, label: string, enable: boolean) => {
      setAssociationBusy(true)
      setAssociationError(null)
      try {
        await setFileAssociation(ext, `${label} 图片`, enable)
        // 改完立刻重读，让复选框反映注册表的**真实**状态
        // （而不是乐观地按用户点击翻转 —— 万一写入失败就显示错了）
        await refreshAssociations()
      } catch (err) {
        setAssociationError(decodeError(err, '设置文件关联失败').message)
      } finally {
        setAssociationBusy(false)
      }
    },
    [refreshAssociations],
  )

  // 加载应用信息（版本、能力）
  useEffect(() => {
    getAppInfo()
      .then(setAppInfo)
      .catch(() => setAppInfo(null))
  }, [])

  // 启动时若命令行带了文件路径，直接打开（`photovault.exe photo.cr3`）
  useEffect(() => {
    getStartupFile()
      .then((path) => {
        if (path) {
          void load(path)
          // ⚠️ 必须切到单图查看：默认模式是**图库**，不切的话用户从资源管理器
          // 双击一张图片，看到的是一个图库界面，图片只在背后装着 ——
          // 显式要求「打开这个文件」就应该显示这个文件。
          setMode('viewer')
        }
      })
      .catch(() => {})
    // 只在挂载时执行一次：`load` 是稳定引用，但刻意不放进依赖，
    // 避免它变化时重复打开同一个文件
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // 启动时同步一次全屏状态
  useEffect(() => {
    getCurrentWindow()
      .isFullscreen()
      .then(setIsFullscreen)
      .catch(() => {})
  }, [])

  /** 拖放接入（原生通道 + HTML5 降级） */
  const { dragging, html5DropProps } = useDragDrop({
    onDropFile: (path) => {
      /*
       * 落在侧栏**图库行**上的拖放是「把文件夹加进这个图库」，由 `useLibDrop`
       * 接手 —— 这里不能再 `load(path)`（会拿一个目录去当图片打开，弹「解码失败」）。
       * 悬停标记在 `over` 阶段就置好了，所以先于这个回调可读。
       */
      if (getLibDropHover() !== null) return
      void load(path)
    },
    onError: (message) => fail(message),
  })

  /** 窗口拖动 / 双击最大化 / 自绘按钮 / 边缘 resize（decorations: false 自绘方案） */
  const {
    onDragHandleMouseDown,
    isMaximized,
    minimize,
    toggleMaximize: toggleWindowMaximize,
    close: closeWindow,
    startResize,
  } = useWindowDrag()

  /** 通过对话框选择文件 */
  const handleOpenDialog = useCallback(async () => {
    try {
      const selected = await openDialog({ multiple: false, filters: DIALOG_FILTERS })
      if (typeof selected === 'string') {
        await load(selected)
      }
    } catch (err) {
      fail(err)
    }
  }, [load, fail])

  /** 工具栏动作分发到画布（通过自定义事件与画布解耦） */
  const dispatchAction = useCallback((action: string) => {
    document
      .querySelector('.pv-canvas')
      ?.dispatchEvent(new CustomEvent('pv-action', { detail: { action } }))
  }, [])

  /** 普通滚轮（窗口级监听）→ 翻页 */
  const handleNavigate = useCallback(
    (direction: 1 | -1) => {
      if (direction > 0) next()
      else prev()
    },
    [next, prev],
  )

  /** 全屏切换 */
  const toggleFullscreen = useCallback(async () => {
    try {
      const win = getCurrentWindow()
      const fs = await win.isFullscreen()
      await win.setFullscreen(!fs)
      setIsFullscreen(!fs)
    } catch {
      // 非 Tauri 环境或权限不足时忽略
    }
  }, [])

  // 视图控制器（状态 + 事件处理器），供画布与信息栏共用
  const viewController = useImageView(image)

  // ── 幻灯片放映 ──────────────────────────────────────────────────
  /** 是否正在放映 */
  const [slideshow, setSlideshow] = useState(false)
  /**
   * 是否已经「预热」完毕、可以让计时器跑起来。
   *
   * 进全屏 → 刷新当前图 → 停 1 秒，这 1 秒里 `slideshow` 已经是 true
   * （快捷键/胶片条都已经按放映处理），但计时器还没开。
   */
  const [slideReady, setSlideReady] = useState(false)
  /** 预热的定时器（退出放映时要清掉，否则 1 秒后自己又"开始"了） */
  const slideWarmupTimer = useRef<number | null>(null)
  /** 每张停留秒数（设置面板可改，持久化在 localStorage） */
  const [slideSeconds, setSlideSeconds] = useState(readSlideSeconds)

  /** 设置面板改了间隔 → 立刻生效（不用重启） */
  useEffect(() => {
    const onSet = (e: Event) => {
      const v = Number((e as CustomEvent<number>).detail)
      if (Number.isFinite(v)) setSlideSeconds(clampSlideSeconds(v))
    }
    window.addEventListener(EVENT_SET_SLIDE_SECONDS, onSet)
    return () => window.removeEventListener(EVENT_SET_SLIDE_SECONDS, onSet)
  }, [])

  /** 放映切换特效（设置里可改） */
  const [slideFx, setSlideFx] = useState<SlideFx>(readSlideFx)
  useEffect(() => {
    const onSet = (e: Event) => setSlideFx((e as CustomEvent<SlideFx>).detail)
    window.addEventListener(EVENT_SET_SLIDE_FX, onSet)
    return () => window.removeEventListener(EVENT_SET_SLIDE_FX, onSet)
  }, [])

  /** 特效时长（毫秒，两种特效共用） */
  const [fxMs, setFxMs] = useState<number>(readFxMs)
  useEffect(() => {
    const onSet = (e: Event) => {
      const v = Number((e as CustomEvent<number>).detail)
      if (Number.isFinite(v)) setFxMs(clampFxMs(v))
    }
    window.addEventListener(EVENT_SET_SLIDE_FX_MS, onSet)
    return () => window.removeEventListener(EVENT_SET_SLIDE_FX_MS, onSet)
  }, [])

  /**
   * 动图的**完整循环时长**（毫秒）—— 放映时用它当停留时间。
   *
   * 为什么不是「帧数 × 固定值」：GIF 每帧的 `delayMs` 是逐帧独立的
   * （常见 40ms / 100ms / 500ms 混用），只能累加。
   * 逐帧追加的渐进加载会让这个值变大，`useSlideshow` 的计时器会跟着重排，
   * 所以不用怕「按首批帧算短了」。
   */
  const animDurationMs = useMemo(() => {
    const frames = animation?.frames ?? []
    if (frames.length <= 1) return null
    const total = frames.reduce((sum, f) => sum + Math.max(20, f.delayMs || 0), 0)
    return total > 0 ? total : null
  }, [animation])
  /**
   * 放映开始时是不是**本来就在全屏**。
   *
   * 是的话退出放映不该动全屏状态（用户自己按 F 进的）；
   * 不是的话说明全屏是我们为了放映打开的，退出时顺手关掉，
   * 别把用户晾在一个黑边全屏里不知道怎么办。
   */
  const fsBeforeSlide = useRef(false)

  /**
   * 开始放映（**不检查有没有图**）：图库右键放映时，图还在解码就先进入状态，
   * 解码完自动开始计时（`useSlideshow` 的 `hasImage` 守卫会等到有图）。
   *
   * ## 「先刷新、停 1 秒再开演」（2026-10-19 用户要求）
   * 进全屏那一刻窗口尺寸变了，画布还是按旧尺寸适应的；直接开播的话第一张
   * 会以"没摆正"的样子出现，紧接着就被切走。
   * 所以顺序是：进入放映态（锁快捷键/隐藏胶片） → 全屏 → **重读一次当前图**
   * → 停 1 秒 → 才让计时器开始（`slideReady`）。
   */
  const beginSlideshow = useCallback(
    async (skipReload = false) => {
      fsBeforeSlide.current = isFullscreen
      setSlideshow(true)
      setSlideReady(false)
      if (!isFullscreen) await toggleFullscreen()
      // 全屏后按新视口重新适应一次（reload 会重新解码 + 重新 fit）
      // ⚠️ 从图库右键进来时刚 `load()` 过新图，别再来一次（大图会重复解码）
      if (!skipReload) void reload()
      if (slideWarmupTimer.current !== null) window.clearTimeout(slideWarmupTimer.current)
      slideWarmupTimer.current = window.setTimeout(() => {
        slideWarmupTimer.current = null
        setSlideReady(true)
        setNotice(`幻灯片放映中 · 每 ${slideSeconds} 秒 · 空格暂停 · Esc 退出`)
      }, SLIDE_WARMUP_MS)
    },
    [isFullscreen, toggleFullscreen, reload, slideSeconds],
  )

  /**
   * 放映推进：到末尾**回到第一张**。
   *
   * `useImageLoader` 的 `next()` 在边界是静默返回（不循环）—— 那是看图的正确语义，
   * 但放映卡在最后一张就等于坏了，所以这里自己绕回列表头部。
   */
  const slideshowAdvance = useCallback(() => {
    if (index < 0 || total < 2) return
    if (index + 1 < total) {
      next()
      return
    }
    const first = siblings[0]
    if (first !== undefined) void load(first)
  }, [index, total, siblings, next, load])

  const toggleSlideshow = useCallback(() => {
    if (slideshow) {
      setSlideshow(false)
      setSlideReady(false)
      // ⚠️ 必须清掉预热定时器：否则退出放映 1 秒后它会 setSlideReady(true)，
      // 而 slideshow 已经 false —— 状态错位，下次进来会跳过预热
      if (slideWarmupTimer.current !== null) {
        window.clearTimeout(slideWarmupTimer.current)
        slideWarmupTimer.current = null
      }
      if (!fsBeforeSlide.current && isFullscreen) void toggleFullscreen()
      return
    }
    if (!image) return
    void beginSlideshow()
  }, [slideshow, image, isFullscreen, toggleFullscreen, beginSlideshow])

  /**
   * 从一份**外部列表**放幻灯片（图库 / 文件夹右键「幻灯片放映」）。
   *
   * 为什么要把列表交给 `useImageLoader`：图库里的照片散落在几十个目录，
   * 单图模式的「下一张」原本只在**当前文件所在目录**里翻 —— 放到第 2 张就跳出图库了。
   */
  const playPlaylist = useCallback(
    (paths: string[], startPath?: string) => {
      const list = paths.filter((p) => !!p)
      if (list.length === 0) return
      setPlaylist(list)
      setMode('viewer')
      const start = startPath && list.includes(startPath) ? startPath : list[0]!
      void load(start)
      // 刚 load 过新图，跳过 beginSlideshow 里的重复 reload
      if (!slideshow) void beginSlideshow(true)
    },
    [setPlaylist, load, slideshow, beginSlideshow],
  )

  const { paused: slidePaused } = useSlideshow({
    // ⚠️ 预热期（只刷新、不计时）不算 active：否则第一张会在 1 秒里被切走
    active: slideshow && slideReady,
    /*
     * 计时 = **纯停留时间 + 特效时间**（用户 2026-10-19 明确：间隔是"去掉特效"的时长）。
     * 特效与计时同时开始，所以要把特效那段补回来，屏幕上静止的时长才真等于设置值。
     * 动图按自身循环时长停留，同样加上特效时间。
     */
    intervalMs: (animDurationMs ?? slideSeconds * 1000) + (slideFx === 'none' ? 0 : fxMs),
    imageKey: imagePath,
    hasImage: !!image,
    onAdvance: slideshowAdvance,
    onBack: prev,
    onExit: toggleSlideshow,
  })

  /** 切回图库模式就停掉放映（否则回到网格后还会偷偷翻图） */
  useEffect(() => {
    if (mode !== 'viewer') setSlideshow(false)
  }, [mode])

  /**
   * 放映暂停时**动图也一起停**。
   *
   * 不然按了空格「暂停」，GIF 还在那儿自顾自地循环 —— 用户看不出暂停了什么。
   * 恢复播放时同步恢复，节奏也就对上了。
   */
  useEffect(() => {
    if (!slideshow) return
    if (slidePaused) player.pause()
    else player.play()
    // player 的 pause/play 是稳定的 useCallback，不必进依赖
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [slideshow, slidePaused])

  /** 打印进行中（禁用按钮，避免连点弹出多个系统对话框） */
  const [printing, setPrinting] = useState(false)

  /** 打印预览浮层是否打开（2026-10-10：打印改成先预览再送印） */
  const [showPrintPreview, setShowPrintPreview] = useState(false)

  /**
   * 普通滚轮翻页 —— **挂在 window 上**，不是画布上。
   *
   * 画布在解码失败时会被错误页替换掉（组件卸载），监听随之消失，
   * 用户就会卡在坏图上翻不动。详见 `useWheelNavigation` 的模块注释。
   *
   * `enabled`：同目录至少有两张才值得监听（一张图翻不了）；
   * **打印预览开着或送印中必须禁用** —— 浮层里滚动纸张/调页边距时
   * 滚轮事件冒泡到 window 会把底下的图翻走，预览和送印的图就错位了。
   * ⚠️ `printing`/`showPrintPreview` 必须声明在本 hook **之前**（const TDZ）。
   */
  useWheelNavigation({
    onNavigate: handleNavigate,
    enabled: mode === 'viewer' && total > 1 && !showPrintPreview && !printing,
  })

  /** 切换图片信息浮层 */
  const toggleInfo = useCallback(() => setShowInfo((v) => !v), [])

  /**
   * 切换帮助 / 设置浮层。
   *
   * ⚠️ **两者互斥**：打开一个时先关掉另一个。
   * 否则两个 z-index 相同的模态会叠在一起，用户看到两层遮罩、
   * 底下那层还点不到（`Esc` 只能关掉最上面一个）。
   * 模态之间「互斥」是通用惯例，用户不需要解释也预期如此。
   */
  const toggleHelp = useCallback(() => {
    setShowHelp((v) => {
      if (!v) setShowSettings(false)
      return !v
    })
  }, [])
  /** 打开设置窗口并指定页（图库标签条的齿轮 → 'ai'） */
  const openSettings = useCallback((page: SettingsPage = 'data') => {
    setSettingsPage(page)
    setShowHelp(false)
    setShowSettings(true)
  }, [])

  const toggleSettings = useCallback(() => {
    setShowSettings((v) => {
      if (!v) setShowHelp(false)
      return !v
    })
  }, [])

  /**
   * 打开调整面板时的视图快照。
   *
   * 用户诉求（2026-10-07）：「如果调整没有覆盖原图，退出调整后应恢复调整前的显示」。
   * 所以打开面板那一刻把整份视图状态（缩放/平移/角度/比例/框选）存下来，
   * 关闭面板时若这次会话**没保存过**，就原样还原 = 「放弃这次调整」。
   * 保存成功则清空快照，避免退出时把已落盘的画面又倒回去。
   */
  const adjustSnapshot = useRef<ReturnType<typeof viewController.snapshotView> | null>(null)

  /**
   * 当前快照对应的图片。用于判断「这张图的快照打过了没」——
   * 调整面板跨图保持打开（2026-10-11）后，每张新图都要重打一次，见下方 effect。
   */
  const adjustSnapshotImage = useRef<DecodedImage | null>(null)

  /**
   * 打开/关闭调整面板（打开时记快照，关闭时按需回滚）。
   *
   * ⚠️ 不能在 `setShowAdjust(updater)` 的回调里调 `restoreView`：
   * React 18 的 updater 在渲染阶段执行，可能被调用多次（StrictMode）甚至被丢弃，
   * 在里面做副作用会导致「还原」执行次数不确定。所以这里**先读当前开关值**
   * （`showAdjust` 已在依赖里）再决定，副作用放在 updater 之外。
   */
  const onToggleAdjustPanel = useCallback(() => {
    if (showAdjust) {
      // 正在关闭：若本次会话**没保存过**，回滚到打开面板前的显示
      if (adjustSnapshot.current) {
        viewController.restoreView(adjustSnapshot.current)
        adjustSnapshot.current = null
      }
      setShowAdjust(false)
      return
    }
    // 正在打开：先记下当前显示，便于取消时回滚
    adjustSnapshot.current = viewController.snapshotView()
    adjustSnapshotImage.current = image
    setShowAdjust(true)
  }, [showAdjust, viewController, image])

  /**
   * `Esc` 的第一优先级处理：关模态浮层 → 取消裁剪。
   *
   * ① **帮助 / 设置浮层是模态**（带遮罩），开着时 `Esc` 先关它们 —— 这是所有
   *    桌面应用的通用直觉，用户不该为了关一个浮层去想别的键。
   *    两者互斥（见 `toggleHelp`），所以这里顺序判断即可，不需要同时关。
   * ② 否则看裁剪框：判据是 **`frameRect` 非空**（用户真的拖过框），不是 `crop`
   *    —— 2026-10-08 统一两种模式后 `crop` 由 `frameRect` 派生：
   *    只要框存在它就一定有值，「没拖过 = 不裁剪」这条规则体现在 `frameRect` 上。
   *
   * 两件都不能处理时**必须返回 `false`**，让快捷键层继续走
   * 「退出全屏 / 适应窗口」，否则 `Esc` 会变成哑键。
   *
   * ⚠️ 刻意**不**在这里关「信息 / 调整」两个角落浮层：关闭调整面板会回滚视图
   * 快照（= 放弃这次调整），用户按 `Esc` 的本意多半是「适应窗口」而不是
   * 「丢弃刚做的调整」。
   */
  /** 覆盖保存的状态机（三态由 App 持有，便于换图时统一清理） */
  const [saveState, setSaveState] = useState<SaveState>({ kind: 'idle' })

  /**
   * 预览阶段的错误（保留浮层让用户改版面重试，所以错误存在这里而不是 `notice`）。
   *
   * 用 `undefined` 而不是 `null` 表示「无错误」——
   * `PrintPreview` 的 `error` prop 类型是 `string | undefined`，
   * 混用 `null` 会在这里报类型错。
   */
  const [printError, setPrintError] = useState<string | undefined>(undefined)

  /** 右键菜单（2026-10-10 新增） */
  const contextMenu = useContextMenu()

  /**
   * 左侧胶片条（同目录缩略图列）。
   *
   * 默认**开**：用户要的就是「看着一张图时能顺手跳到同目录的其它张」。
   * 存在 localStorage 里，关掉之后下次打开也不弹出来 —— 窄屏用户会一直关着它。
   */
  const [showRail, setShowRail] = useState(readRailPref)
  const toggleRail = useCallback(() => {
    setShowRail((v) => {
      const next = !v
      try {
        localStorage.setItem(RAIL_PREF_KEY, next ? '1' : '0')
      } catch {
        /* 写不进去（隐私模式）就只对本会话生效 */
      }
      return next
    })
  }, [])

  /**
   * `T` 切换胶片条。
   *
   * ⛔ `useShortcuts` 里**没有** `T`（I/F/M/A/H + 缩放/旋转那几组），不冲突。
   * 输入框内不拦：用户在搜索/重命名里打 t 不该把胶片条关掉。
   */
  useEffect(() => {
    if (mode !== 'viewer') return
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 't' && e.key !== 'T') return
      if (e.ctrlKey || e.metaKey || e.altKey) return
      const t = e.target as HTMLElement | null
      const tag = t?.tagName
      if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || t?.isContentEditable) return
      e.preventDefault()
      toggleRail()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [mode, toggleRail])

  /**
   * `Esc` 分层处理。⚠️ **返回 true 表示「我处理掉了」**，
   * 快捷键链据此停止继续往下走。
   *
   * ⚠️ 刻意**不**在这里关「信息 / 调整」两个角落浮层：关闭调整面板会回滚视图
   * 快照（= 放弃这次调整），用户按 `Esc` 的本意多半是「适应窗口」而不是
   * 「丢弃刚做的调整」。
   *
   * ⚠️ 上面几个 state / hook 必须**声明在本函数之前** —— 否则依赖数组里的
   * `contextMenu` 触发 TDZ（访问尚未初始化的 const），一按 Esc 就崩。
   */
  const onEscape = useCallback((): boolean => {
    // ⚠️ 顺序要紧：菜单 → 预览 → 帮助/设置。
    // 都是「临时层」，Esc 应该只关**最上面那一个**，
    // 一口气把菜单、预览、全屏都关了就是 bug。
    // （右键菜单与预览浮层各自也监听 Esc 抢在前面，这里是双保险）
    if (contextMenu.state) {
      contextMenu.close()
      return true
    }
    if (showPrintPreview) {
      setShowPrintPreview(false)
      return true
    }
    if (showHelp) {
      setShowHelp(false)
      return true
    }
    if (showSettings) {
      setShowSettings(false)
      return true
    }
    if (viewController.view.frameRect === null) return false
    viewController.clearCrop()
    return true
  }, [contextMenu, showPrintPreview, showHelp, showSettings, viewController])

  /**
   * 状态栏的一次性提示（打印结果等）。
   *
   * **会自动清除**：这类结果不需要用户确认，但一直留在状态栏就会与
   * 文件信息混在一起 —— 下次翻页时看起来像当前文件的属性。
   */
  const [notice, setNotice] = useState<string | null>(null)

  // 换图时的清理：只清「属于上一张图」的东西，面板开关不清
  // （信息面板一直是这个行为；调整面板 2026-10-11 起也对齐 —— 连续修图时
  //  每翻一张都要重新点开太烦。角度/比例会随新图复位，面板里显示的值同步跟着变）
  useEffect(() => {
    // 上一次的"已保存"提示不该跟着走到新图（会让人以为这张已经存过了）
    setSaveState({ kind: 'idle' })
    // ⚠️ 右键菜单也必须关：它是 `position: fixed` 的独立层，
    // 不会随主区重渲染消失 —— 翻页后菜单里的「另存为」会作用于**上一张**图。
    // 那种"菜单开着、翻了个页、导出的是旧图"最难发现（导出成功了，只是内容不对）。
    contextMenu.close()
    // 打印预览同理：浮层开着时翻页，预览的版面是按旧图算的。
    // ⚠️ 但 `printing` 期间不能关 —— 那样用户会以为送印被取消了。
    if (!printing) setShowPrintPreview(false)
    setPrintError(undefined)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [image])

  /**
   * 调整面板跨图保持打开时的快照维护。
   *
   * `adjustSnapshot` 是「打开面板那一刻的视图」，用于「没保存就关闭 → 回滚」。
   * 面板现在会跟着翻到新图，那份快照就变成**旧图**的了 —— 直接沿用会在关闭时
   * 把旧图的缩放/角度套到新图上（画面位置毫无意义）。所以每张新图重打一次。
   *
   * ⚠️ 判据是「视图已被复位」，不是「image 变了」：换图那一刻 `view` 里还是旧值
   * （复位靠 `setView`，要下一次渲染才生效），此时打快照会打到旧图画面。
   * `useImageView` 换图时写回 `rotation=0 / aspect='free' / frameRect=null /
   * crop=null`，这里逐个核对，确保打的是**新图复位后**的状态。
   *
   * 无依赖数组：需要跟随复位后的那次渲染才能读到新 `view`。
   */
  useEffect(() => {
    if (!showAdjust || !image) {
      // 面板关着（或没图）：快照没有意义，别让它在下次打开时被误用
      adjustSnapshot.current = null
      adjustSnapshotImage.current = null
      return
    }
    if (adjustSnapshotImage.current === image) return
    const v = viewController.view
    const resetDone =
      v.rotation === 0 && v.aspect === 'free' && v.frameRect === null && v.crop === null
    if (!resetDone) return
    adjustSnapshot.current = viewController.snapshotView()
    adjustSnapshotImage.current = image
  })

  /** 当前文件能否覆盖写回（RAW / WebP 等不支持，按钮置灰） */
  const writable = info ? isWritableFormat(info.format) : false
  const notWritableTip = useMemo(
    () => (info ? notWritableReason(info.formatLabel, info.isRaw) : undefined),
    [info],
  )

  /**
   * 把画布调整**覆盖写回原文件**。
   *
   * ⚠️ **会覆盖用户的原始文件**（用户 2026-10-07 的明确决策）。
   * 成功后会重新加载，让画布反映写回后的真实像素。
   *
   * 只对 `can_overwrite_original` 放行的格式有效（JPEG/PNG/BMP）。
   * RAW 走 [`handleSaveAs`]。
   */
  const handleSave = useCallback(async () => {
    if (!info || !image) return
    setSaveState({ kind: 'saving' })
    try {
      // 用视图的 rotation 折算成 90° 步数（rotation 已是 0/90/180/270）
      const steps = Math.round(viewController.view.rotation / 90)
      // `crop` 由 view 里的 `frameRect` 派生（拖过框才有值 = 按框裁）；
      // 没拖过就是 `null` —— 自由比例 = 不裁，固定比例 = 后端按 `aspect` 居中裁。
      const crop = viewController.view.crop
      const result = await saveEditedImage(info.path, steps, viewController.view.aspect, crop)

      setSaveState({
        kind: 'ok',
        message: `已保存 ${result.width} × ${result.height}（${formatFileSize(result.fileSize)}）`,
      })
      // 已经落盘了 —— 丢弃「打开面板时」的快照，
      // 否则关闭面板会把画面倒回保存前的样子（与「已保存」自相矛盾）。
      adjustSnapshot.current = null
      // 裁剪已经落到文件上了，画布上那个框必须清掉：
      // 留着会让用户以为「还要再存一次」，而它此刻框的是**已经裁过的新图**，
      // 再按一次「覆盖原图」就是二次裁剪。
      if (crop) viewController.clearCrop()
      // 重新解码：文件内容变了，画布必须跟着变
      await load(info.path)
      // 覆盖保存不新增文件，但保险起见也同步一次目录列表
      void refreshSiblings()
    } catch (err) {
      // 后端已把「无权限 / 文件被占用 / 格式不可写」等都标为 userVisible，
      // 会直接返回可操作的中文说明；技术性错误才落到兜底文案
      setSaveState({ kind: 'error', message: decodeError(err, '保存失败').message })
    }
  }, [info, image, load, viewController, refreshSiblings])

  /**
   * **另存为** —— 几何结果导出到新文件，**原文件一个字节都不动**。
   *
   * 存在的理由：RAW 的画布像素来自**相机内嵌的 JPEG 预览**，
   * 覆盖原文件等于用预览替换传感器数据（画质永久降级），
   * 所以 RAW 只能走这条路。
   *
   *与 [`handleSave`] 的三处不同：
   * 1. 先弹保存框拿目标路径（用户取消就什么都不做）；
   * 2. **不刷新画布** —— 原文件像素没变，刷了反而会把裁剪框冲掉；
   * 3. 成功提示要**披露预览分辨率**，否则用户以为导出变糊了。
   */
  const handleSaveAs = useCallback(async () => {
    if (!info || !image) return

    // 默认文件名：`原名_已裁剪.jpg`（同目录，用户容易找到）
    const ext = EXPORT_FORMATS[0]!.ext
    const base = info.fileName.replace(/\.[^.]+$/, '')
    const defaultPath = info.path.replace(/[^/\\]+$/, `${base}_已裁剪.${ext}`)

    const filters = EXPORT_FORMATS.map((f) => ({ name: f.label, extensions: [f.ext] }))
    const picked = await saveDialog({
      title: '另存为（原文件不变）',
      defaultPath,
      filters,
    })
    if (!picked) return // 用户取消：静默，不要报错

    setSaveState({ kind: 'saving' })
    try {
      const steps = Math.round(viewController.view.rotation / 90)
      const crop = viewController.view.crop
      const result = await saveEditedImage(
        info.path,
        steps,
        viewController.view.aspect,
        crop,
        picked,
      )

      // ⚠️ RAW 导出必须说清「这是预览分辨率」——
      // 用户看到文件变小/变糊却没人解释，会当成 bug。
      setSaveState({
        kind: 'ok',
        message: result.previewResolution
          ? `${rawExportHint(result.width, result.height, info.width, info.height)} · ${formatFileSize(result.fileSize)}`
          : `已导出 ${result.width} × ${result.height}（${formatFileSize(result.fileSize)}）`,
      })
      // ⚠️ **不调 load()、不清 crop**：原文件字节没变。
      // 重新加载会把用户刚导出的那张图的裁剪框冲掉，
      // 也让「导出」看起来像「保存」（两者语义不同，用户会困惑）。
      // 丢弃快照只为避免关闭面板时画面回滚 —— 几何状态保留在画布上。
      adjustSnapshot.current = null
      // ⭐ 但**目录列表必须重新列**：另存为会在当前文件夹下**新增一个文件**，
      // 而 `siblingsRef` 只在目录路径变化时才刷新（见 `refreshSiblings` 注释）。
      // 不刷新的话新文件不在翻页序列里、状态栏计数也不对 ——
      // 用户会以为导出坏了，或必须关掉重开才认出来。
      void refreshSiblings()
    } catch (err) {
      setSaveState({ kind: 'error', message: decodeError(err, '导出失败').message })
    }
  }, [info, image, viewController, refreshSiblings])

  /**
   * **刷新**（右键菜单）：重新读取当前这张图。
   *
   * ## 为什么需要它
   * 文件被别的程序改过时，界面上的像素是**上一次读到的**。缓存的文件版本
   * 校验（`decodeImage`）能保证"重新读时是最新的"，但它**不会替你触发重读** ——
   * 而正在看的那张图没人会替它翻页。所以给一个显式入口。
   *
   * 两种典型用法：
   * 1. 图被外部工具改过（或导出覆盖了同名的图）→ 立刻看到新内容；
   * 2. 图是坏的 / 读到一半失败 → 修好文件后重读，不用重新走"打开"对话框。
   *
   * ⚠️ **代价要说清**：重读会让视图**复位到适应窗口**（缩放/旋转/裁剪都清掉），
   * 与「覆盖保存后自动刷新」是同一行为。这是有意的 —— 文件内容都换了，
   * 把旧的取景套上去只会得到一张莫名其妙的画面。
   */
  const handleRefresh = useCallback(async () => {
    if (!imagePath) return
    await reload()
    // ⭐ 同时重新列一次目录：右键「刷新」也是用户发现"我从别处拷了张图进来"
    // 的主要入口。只重读当前图的话，新图仍不在翻页序列里（症状同上）。
    await refreshSiblings()
    setNotice('已重新读取当前图片')
  }, [imagePath, reload, refreshSiblings])

  /**
   * **打开打印预览**（工具栏按钮 / Ctrl+P / 右键菜单都走这里）。
   *
   * 2026-10-10 改：原来这里是**直接送印**，一点就打印。
   * 现在先开预览让用户确认版面，再由 [`handlePrint`] 真正送印。
   */
  const handleOpenPrintPreview = useCallback(() => {
    if (!info || !image || printing) return
    setPrintError(undefined)
    setShowPrintPreview(true)
  }, [info, image, printing])

  /**
   * 真正送印（由预览浮层的「打印…」触发）。
   *
   * **参数与 [`handleSave`] 完全一致** —— 后端 `print_image` 与
   * `save_edited_image` 共用同一套几何变换，所以「打印出来的」
   * 就是「保存后会得到的」那张图。
   *
   * `fit` 来自预览浮层的版面选择（适应页面 / 原始尺寸）。
   * 后端会弹**系统打印对话框**（选打印机/份数/方向）；
   * 用户取消返回 `cancelled`，不算失败，也不提示。
   *
   * ⚠️ 失败时**不关预览**：用户还要改版面重试，关掉就得重新打开。
   */
  const handlePrint = useCallback(
    async (opts?: {
      fit?: boolean
      paper?: PaperSize
      orientation?: PrintOrientation
      quality?: PrintQuality
      quick?: boolean
      printer?: string
      marginMm?: number
    }) => {
      if (!info || !image || printing) return
      setPrinting(true)
      setPrintError(undefined)
      try {
        const steps = Math.round(viewController.view.rotation / 90)
        const result = await printImage(
          info.path,
          steps,
          viewController.view.aspect,
          viewController.view.crop,
          opts?.fit,
          opts?.orientation ?? 'portrait',
          opts?.quality ?? 'high',
          opts?.paper?.widthMm ?? 210,
          opts?.paper?.heightMm ?? 297,
          opts?.marginMm ?? 10,
          opts?.printer || null,
          opts?.quick ?? false,
        )
        if (result.cancelled) {
          // 用户在**系统对话框**里取消：静默，别用提示打扰，
          // 也**不关预览**（他可能只是改了主意，还想在这儿调版面）
          return
        }
        setNotice(`已送印 ${result.width} × ${result.height}`)
        setShowPrintPreview(false)
      } catch (err) {
      setNotice(decodeError(err, '打印失败').message)
    } finally {
      setPrinting(false)
    }
  }, [info, image, printing, viewController])

  /**
   * 右键菜单项。
   *
   * ## 为什么这两项要用现成的处理函数，不能自己另写一套
   * - **「另存为…」**→ 直接复用 `handleSaveAs`，与调整浮层的按钮**同一段逻辑**。
   *   另写一套的后果是「面板里能导出、右键里不能」，用户会当成 bug。
   * - **「打印…」**→ 直接复用 `handleOpenPrintPreview`，即工具栏/Ctrl+P 那条路。
   *   所以右键打印 = 先看预览再送印，不是绕过预览直接打。
   *
   * ⚠️ 全部项都依赖**当前这张图**，所以 `!image` 时整体不可用（而不是逐项判断）。
   */
  const contextMenuItems = useMemo<ContextMenuItem[]>(
    () => [
      { id: 'save-as', label: '另存为…', hint: '当前图片', onSelect: () => void handleSaveAs() },
      { id: 'print', label: '打印…', hint: 'Ctrl+P', onSelect: handleOpenPrintPreview, separatorBefore: true },
      { id: 'open', label: '打开…', hint: 'Ctrl+O', onSelect: () => void handleOpenDialog() },
      {
        id: 'refresh',
        label: '刷新',
        hint: 'F5',
        // 没有图时置灰：这条命令的语义就是"重读当前文件"
        disabled: !imagePath,
        onSelect: () => void handleRefresh(),
      },
      { id: 'fit', label: '适应窗口', hint: '0', onSelect: () => dispatchAction('fit'), separatorBefore: true },
      { id: 'actual', label: '原始大小', hint: '1', onSelect: () => dispatchAction('actual-size') },
      {
        id: 'rot-ccw',
        label: '逆时针旋转',
        hint: '[',
        onSelect: () => dispatchAction('rotate-left'),
      },
      {
        id: 'rot-cw',
        label: '顺时针旋转',
        hint: ']',
        onSelect: () => dispatchAction('rotate-right'),
      },
      /**
       * 全屏切换。
       *
       * ## 为什么是**一项动态文案**，而不是「全屏」「退出全屏」两项
       * 两项并排的话，任何时候都必有一项是废的（已经全屏了还点「全屏」），
       * 菜单里多一行永远点不动的灰项是纯噪音。动态文案只占一行、
       * 且当前能做什么一目了然 —— 用户要的「能进能出」两个能力都在。
       *
       * ## 为什么这项特别值得放进右键菜单
       * **全屏时工具栏是隐藏的**（`{!isFullscreen && <Toolbar>}`），
       * 退出全屏在界面上就没有按钮了，只剩快捷键。右键菜单是全屏状态下
       * 唯一还在的入口（`<main onContextMenu>` 不随全屏隐藏），
       * 所以这项在"进去之后出不来"的场景里是刚需。
       */
      {
        id: 'rail',
        label: showRail ? '隐藏左侧胶片条' : '显示左侧胶片条',
        hint: 'T',
        disabled: total <= 1,
        onSelect: toggleRail,
        separatorBefore: true,
      },
      {
        id: 'slideshow',
        label: slideshow ? '停止幻灯片放映' : '幻灯片放映',
        hint: 'S',
        disabled: !image,
        onSelect: toggleSlideshow,
        separatorBefore: true,
      },
      {
        id: 'fullscreen',
        label: isFullscreen ? '退出全屏' : '全屏',
        // F 是**切换**键（不是"进入全屏"）—— 提示写切换键本身，别写成 Enter 之类
        hint: 'F',
        onSelect: () => void toggleFullscreen(),
        separatorBefore: true,
      },
    ],
    [
      handleSaveAs,
      handleOpenPrintPreview,
      handleOpenDialog,
      handleRefresh,
      dispatchAction,
      toggleFullscreen,
      isFullscreen,
      image,
      imagePath,
      showRail,
      toggleRail,
      total,
      slideshow,
      toggleSlideshow,
    ],
  )

  /** 提示自动消失（4 秒足够读完一行，也不至于一直占着状态栏） */
  useEffect(() => {
    if (!notice) return
    const t = window.setTimeout(() => setNotice(null), 4000)
    return () => window.clearTimeout(t)
  }, [notice])

  /** 键盘快捷键 */
  useShortcuts({
    onOpen: handleOpenDialog,
    // ⚠️ Ctrl+P 只**开预览**，不再直接送印（2026-10-10）
    onPrint: handleOpenPrintPreview,
    onAction: dispatchAction,
    hasImage: !!image,
    onNext: next,
    onPrev: prev,
    onToggleFullscreen: toggleFullscreen,
    onToggleSlideshow: toggleSlideshow,
    onToggleMaximize: toggleWindowMaximize,
    onToggleInfo: toggleInfo,
    onToggleAdjust: onToggleAdjustPanel,
    onToggleHelp: toggleHelp,
    onEscape,
    isFullscreen,
    onRefresh: handleRefresh,
    // 放映时整套快捷键让位给 `useSlideshow`（空格/方向键语义完全不同）
    enabled: mode === 'viewer' && !slideshow,
  })

  return (
    <div className="pv-app" data-dragging={dragging} data-fullscreen={isFullscreen} {...html5DropProps}>
      {/* 自绘窗口边缘 resize 热区（decorations: false 后系统边框失效） */}
      <ResizeHandles enabled={!isFullscreen} onResize={startResize} />

      {/*
        放映暂停的小提示。**只在暂停时出现**：
        播放中加任何浮层都会挡照片，而用户按了空格后确实需要一个"停住了"的反馈。
        不拦截鼠标（`pointer-events: none`），点哪儿都不受影响。
      */}
      {slideshow && slidePaused && (
        <div className="pv-slide-pause" role="status">
          已暂停 · 空格继续 · Esc 退出
        </div>
      )}

      {!isFullscreen && (
        <Toolbar
          onOpen={handleOpenDialog}
          onAction={dispatchAction}
          hasImage={!!image}
          rawEnabled={appInfo?.rawEnabled ?? false}
          onPrev={prev}
          onNext={next}
          canPrev={canPrev}
          canNext={canNext}
          onToggleFullscreen={toggleFullscreen}
          isFullscreen={isFullscreen}
          onToggleSlideshow={toggleSlideshow}
          slideshow={slideshow}
          onToggleInfo={toggleInfo}
          showInfo={showInfo}
          onToggleAdjust={onToggleAdjustPanel}
          showAdjust={showAdjust}
          onToggleHelp={toggleHelp}
          showHelp={showHelp}
          onToggleSettings={toggleSettings}
          showSettings={showSettings}
          onPrint={handleOpenPrintPreview}
          printing={printing}
          animation={
            player.isAnimated
              ? { playing: player.playing, frameIndex: player.index, frameCount: player.count }
              : null
          }
          onTogglePlay={player.toggle}
          onDragMouseDown={onDragHandleMouseDown}
          isMaximized={isMaximized}
          onMinimize={minimize}
          onToggleMaximize={toggleWindowMaximize}
          onClose={closeWindow}
          onOpenManager={() => setMode('manager')}
        />
      )}

      <main
        className="pv-main"
        onContextMenu={contextMenu.open}
      >
        {/*
          同目录胶片条：`.pv-main` 是 flex 容器，它自然成为左侧一列，
          画布（`.pv-canvas { flex: 1 }`）占剩余宽度，ResizeObserver 会 refit。
        */}
        {/*
          ⚠️ 必须带 `mode === 'viewer'`：图库模式是**盖在** `<main>` 上的，
          不加这个条件会在后台继续请求一整列缩略图（用户根本看不见）。
          ⚠️ 放映时也隐藏（`!slideshow`）：全屏放映不该留一条缩略图栏占画面。
        */}
        {mode === 'viewer' && showRail && !slideshow && siblings.length > 0 && (
          <ThumbRail
            paths={siblings}
            index={index}
            onPick={(p) => {
              // 点当前这张不用重载（会白白重解码一次）
              if (p !== imagePath) void load(p)
            }}
          />
        )}

        {displayImage ? (
          <ImageCanvas
            image={displayImage}
            controller={viewController}
            fx={slideshow ? slideFx : null}
            fxKey={imagePath}
            fxMs={fxMs}
            cropUi={showAdjust}
          />
        ) : (
          <EmptyState status={status} onOpen={handleOpenDialog} />
        )}

        {/* 图片信息浮层（右下角） */}
        {showInfo && info && <InfoPanel info={info} onClose={() => setShowInfo(false)} />}

        {/* 画布调整浮层（右上角） */}
        {showAdjust && image && (
          <AdjustPanel
            rotation={viewController.view.rotation}
            fineAngle={viewController.view.fineAngle}
            aspect={viewController.view.aspect}
            crop={viewController.view.crop}
            onFineAngle={viewController.setFineAngle}
            onAspect={viewController.setAspect}
            onClearCrop={viewController.clearCrop}
            onRotateCw={() => dispatchAction('rotate-right')}
            onRotateCcw={() => dispatchAction('rotate-left')}
            onReset={viewController.resetAdjust}
            onSave={handleSave}
            onSaveAs={handleSaveAs}
            saveState={saveState}
            writable={writable}
            notWritableReason={notWritableTip}
            onClose={onToggleAdjustPanel}
          />
        )}

        {/* 帮助浮层（居中模态：快捷键速查） */}
        {showHelp && <HelpPanel onClose={toggleHelp} />}

        {/* 设置窗口（左分类树 + 右内容；2026-10-24 取代旧的居中模态 SettingsPanel） */}
        {showSettings && (
          <SettingsWindow
            /* key 里带上页码：从齿轮进来要**立刻**落到 AI 页，而不是沿用上次停留的页 */
            key={settingsPage}
            initialPage={settingsPage}
            onClose={toggleSettings}
            associations={associations}
            onToggleAssociation={onToggleAssociation}
            onToggleAllAssociations={onToggleAllAssociations}
            associationBusy={associationBusy}
            associationError={associationError}
          />
        )}

        {/*
          打印预览（居中模态）。
          ⚠️ `imageWidth/imageHeight` 用的是 **view 变换后的**语义 ——
          实际送印的尺寸由后端 `print_image` 解码+变换后给出，
          预览这里用 `displayImage` 的像素尺寸近似，
          旋转会让宽高互换，所以先按 90° 步数对调。
        */}
        {showPrintPreview && displayImage && (
          <PrintPreview
            imageWidth={
              Math.abs(viewController.view.rotation / 90) % 2 === 1
                ? displayImage.height
                : displayImage.width
            }
            imageHeight={
              Math.abs(viewController.view.rotation / 90) % 2 === 1
                ? displayImage.width
                : displayImage.height
            }
            sourceImage={displayImage}
            rotation={viewController.view.rotation}
            printing={printing}
            error={printError}
            onClose={() => setShowPrintPreview(false)}
            onConfirm={({ fit, paper, orientation, quality, quick, printer, marginMm }) =>
              void handlePrint({ fit, paper, orientation, quality, quick, printer, marginMm })
            }
          />
        )}

        {/* 右键菜单：固定定位在光标处，渲染在 main 内以复用同一套层级。
            key 用坐标：换位置重新右键时强制重挂载，pos 重新取新坐标（确保出现在新鼠标处）。 */}
        {contextMenu.state && (
          <ContextMenu
            key={`${contextMenu.state.x},${contextMenu.state.y}`}
            position={{ x: contextMenu.state.x, y: contextMenu.state.y }}
            items={contextMenuItems}
            onClose={contextMenu.close}
          />
        )}
      </main>

      {/* 拖拽悬停提示 */}
      {dragging && (
        <div className="pv-drop-overlay" aria-hidden>
          <div className="pv-drop-box">松开以打开图片</div>
        </div>
      )}

      {!isFullscreen && (
        <StatusBar
          info={info}
          notice={notice}
          status={status}
          index={index}
          total={total}
          animation={
            player.isAnimated
              ? {
                  playing: player.playing,
                  frameIndex: player.index,
                  frameCount: player.count,
                  truncated: animation?.truncated ?? false,
                }
              : null
          }
          zoomScale={viewController.view.scale}
          onZoomScale={viewController.setZoom}
        />
      )}

      {/*
        图库模式（S2）：**始终挂载**，切到单图查看时只是 `display:none`。
        卸载会丢掉「当前在哪个文件夹 / 选了哪些图 / 滚到第几屏」，
        用户从单图返回时就只剩一个空白主页。
        ⚠️ 所以必须传 `active` 让它把键盘监听关掉 ——
        否则单图查看时按 Esc 会顺手把图库模式也退了。
      */}
      <ManagerView
        active={mode === 'manager'}
        onExit={() => setMode('viewer')}
        onOpenSettings={openSettings}
        onOpenImage={(p, siblings) => {
          /*
           * ⭐ 把主区当前显示的列表交给单图浏览，让「下一张 / 上一张」在这份列表里翻。
           * 不传的话 `useImageLoader` 会走「列当前文件所在目录」那条路 ——
           * 按人像/标签筛出来的图散落在不同目录，翻一张就跳出筛选结果了。
           * 列表里含当前路径时才用（理论上必然含，防御一下脏数据）。
           */
          setPlaylist(siblings.includes(p) ? siblings : null)
          setMode('viewer')
          void load(p)
        }}
        onPlaySlideshow={playPlaylist}
        onDragMouseDown={onDragHandleMouseDown}
        isMaximized={isMaximized}
        onMinimize={minimize}
        onToggleMaximize={toggleWindowMaximize}
        onClose={closeWindow}
      />
    </div>
  )
}
