/**
 * IPC 数据类型定义。
 *
 * 这里只放**类型**，不放实现，便于被 hooks / 组件无副作用地引用。
 */

/** 探测结果（对应后端 `ImageInfoDto`） */
export interface ImageInfo {
  path: string
  fileName: string
  format: string
  formatLabel: string
  width: number
  height: number
  fileSize: number
  isRaw: boolean
  modifiedAt: number
  /** 拍摄参数（EXIF）；文件不含 EXIF 时为 null */
  exif: ExifData | null
  /** RAW 专属信息；常规格式为 null */
  raw: RawInfo | null
  /**
   * 由 GPS 坐标反查出的拍摄地点（离线库，不联网）。
   *
   * 与 `exif.gps` 的分工：`exif.gps` 是**文件里写的**原始坐标，
   * 这里是**我们算出来的**"哪座城市"。无 GPS、或附近 150 km 内没有城镇时为 `null`。
   */
  place: Place | null
}

/**
 * 离线地名库的查询结果（对应后端 `pv_geo::Place`）。
 *
 * `label` 是后端格式化好的展示文本（`上海，中国`），前端直接显示 ——
 * 距离后缀这类规则不在这里再写一遍。
 */
export interface Place {
  /** 城市名（中国城市为中文；国外城市已转简体） */
  name: string
  /** 国家/地区中文名 */
  country: string
  /** 到该城市的直线距离（公里） */
  distanceKm: number
  /** 城市人口 */
  population: number
  /** 展示文本，如 `上海，中国`；离得远时为 `佛山，中国（38 km）` */
  label: string
}

/**
 * RAW 格式信息（对应后端 `RawInfoDto`，源自 `pv_core::raw::RawMetadata`）。
 *
 * 与 `ExifData` 的分工：`ExifData` 是"文件里写的 EXIF 文本"，
 * `RawInfo` 是"RAW 管线消化后的事实"。RAW 的拍摄参数两边都可能提供，
 * 展示时优先用 `raw`（更权威）。
 */
export interface RawInfo {
  /** 原始格式标识，如 `CR3` / `NRW` / `DNG` */
  rawFormat: string
  /** 内嵌预览是否可用（Stage 1 看图路径） */
  hasEmbeddedPreview: boolean
  /** 位深（内嵌预览按 8 位处理） */
  bitDepth: number
  /** 传感器色彩空间的中文标签，如 `sRGB` / `相机原生` */
  colorSpace: string
}

/**
 * EXIF 拍摄参数（对应后端 `ExifData`）。
 *
 * 数值已在 Rust 侧格式化为可直接展示的字符串（`f/2.8`、`1/250 s`、`-0.3 EV`），
 * 前端只负责「有值才显示」，不再做单位换算 —— 避免两侧各写一套换算逻辑。
 * 缺失字段为 `null`。
 */
/**
 * 拍摄地点（EXIF GPS）。
 *
 * `latitude` / `longitude` 是**十进制度的字符串**（南纬、西经为负），
 * 专门给地图链接用；`display` 是拼好方位字母的展示文本。
 * ⚠️ 数值不是 `number`：Rust 侧为保持 `ExifData` 的相等性判断统一用字符串，
 * 前端也不要再 `parseFloat` 回去做换算（要换算说明该逻辑放错了层）。
 */
export interface GpsData {
  /** 纬度（十进制度，南纬为负），如 `31.204839` */
  latitude: string
  /** 经度（十进制度，西经为负），如 `121.440628` */
  longitude: string
  /** 海拔（米，海平面以下为负）；未记录为 `null` */
  altitude: string | null
  /** 展示文本，如 `31.204839°N, 121.440628°E` */
  display: string
}

export interface ExifData {
  /** 相机厂商，如 `SONY` */
  make: string | null
  /** 相机型号，如 `ILCE-7C` */
  model: string | null
  /** 镜头，如 `E 28-200mm F2.8-5.6 A071` */
  lens: string | null
  /** 拍摄时间，如 `2026-05-14 07:57:55` */
  dateTimeOriginal: string | null
  /** 快门速度，如 `1/125 s` */
  exposureTime: string | null
  /** 光圈，如 `f/14` */
  fNumber: string | null
  /** ISO 感光度 */
  iso: number | null
  /** 焦距，如 `73 mm` */
  focalLength: string | null
  /** 35mm 等效焦距 */
  focalLength35mm: number | null
  /** 曝光补偿，如 `+0.3 EV` */
  exposureBias: string | null
  /** 曝光程序，如 `光圈优先` */
  exposureProgram: string | null
  /** 测光模式，如 `评价测光` */
  meteringMode: string | null
  /** 闪光灯，如 `未闪光` */
  flash: string | null
  /** 白平衡，如 `自动` */
  whiteBalance: string | null
  /** 图像方向，如 `顺时针 90°` */
  orientation: string | null
  /** 写入该文件的软件 */
  software: string | null
  /**
   * 拍摄地点（GPS）。
   * 只有开了定位的手机、或带 GPS 模块的相机才会写 —— **绝大多数照片没有**，
   * 为 `null` 时整行隐藏。
   */
  gps: GpsData | null
}

/** 后端返回的错误（对应 `ErrorDto`） */
export interface DecodeError {
  code: string
  message: string
  userVisible: boolean
}

/** 应用信息（对应 `AppInfoDto`） */
export interface AppInfo {
  version: string
  backends: string[]
  rawEnabled: boolean
}

/** 格式能力（对应 `FormatCapability`） */
export interface FormatCapability {
  id: string
  label: string
  tier: string
  enabled: boolean
}

/** 解码用途：决定后端选哪一档尺寸/精度 */
export type DecodePurpose = 'thumbnail' | 'preview' | 'edit' | 'export'

/** 解码结果：原始 RGBA 像素 + 尺寸 */
export interface DecodedImage {
  width: number
  height: number
  /** RGBA 像素，长度 === width * height * 4 */
  data: Uint8ClampedArray<ArrayBuffer>
}

/**
 * 画布调整写回原文件的结果。
 *
 * 键名与后端 `SaveResultDto` 一一对应（`serde(rename_all = "camelCase")`）。
 */
export interface SaveResult {
  /** 写回后的实际像素宽度（旋转/裁剪后可能与原来不同） */
  width: number
  /** 写回后的实际像素高度 */
  height: number
  /** 写回后的文件字节数 */
  fileSize: number
  /** 实际应用的 90° 旋转步数（0-3，顺时针） */
  rotationApplied: number
  /** 是否应用了比例裁剪 */
  cropped: boolean
  /**
   * 实际写入的路径。覆盖保存时等于原路径；另存为时是新路径。
   *
   * 前端要靠它决定「刷新当前图」还是「提示已导出到某处」。
   */
  savedPath: string
  /**
   * 是否是「另存为」（写到了新文件，原文件未被动过）。
   *
   * 为 true 时**不要刷新画布**：原文件像素没变，刷新等于白刷，
   * 还会把用户刚做的裁剪框冲掉。
   */
  exported: boolean
  /**
   * 是否只拿到了**内嵌预览**的分辨率（RAW 恒为 true）。
   *
   * ⚠️ RAW 的画布像素来自相机内嵌的 JPEG 预览，导出的文件就是这个
   * 分辨率，**不是传感器全尺寸**。前端必须据此提示用户画质已降级。
   */
  previewResolution: boolean
}

/** 错误诊断详情 */
export interface ErrorDetailInfo {
  /** 错误码，如 decode_error */
  code: string
  /** 错误来源 */
  source: string
  /** 涉及的文件路径（若有） */
  path: string | null
  /** 时间戳 */
  time: string
  /** 面向用户的提示 */
  message: string
  /** 是否适合直接展示给用户 */
  userVisible: boolean
  /** 技术细节（堆栈等），复制时一并带上 */
  technical: string
  /** 完整的可复制文本 */
  full: string
}

/**
 * 前端解好像素的动图**数据块**。
 *
 * 后端把动图打包成 **PVA1 二进制块**（见 `protocol.ts::parseAnimation`），
 * `decodeAnimation()` 把它切成每帧的**零拷贝视图**，播放器只认这个类型。
 *
 * ## ⚠️ 这是「一块」不是「整段」（2026-10-09）
 * 大动图（实测 299 帧 → 71MB）一次传完要 1.2 秒，用户干等。现在拆成两段：
 * 先取开头几秒的帧让画面立刻动起来，剩下的在后台接着传。
 * 所以 `count` 是**本块**帧数，`totalCount` 才是整段总帧数，
 * `startIndex` 说明本块接在第几帧 —— 加载方靠这三个字段把多块拼成完整动画。
 */
export interface DecodedAnimationFrame {
  /** 该帧显示时长（毫秒） */
  delayMs: number
  /** 解好的像素（指向同一块 ArrayBuffer，没有复制） */
  image: DecodedImage
}

export interface DecodedAnimation {
  width: number
  height: number
  /** **本块**的帧数（= `frames.length`） */
  count: number
  /** 是否因上限被截断（超帧数或总内存） */
  truncated: boolean
  /** `null` = 无限循环 */
  loopCount: number | null
  /** 本块第一帧在整段动画里的序号 */
  startIndex: number
  /** 整段动画的总帧数 */
  totalCount: number
  frames: DecodedAnimationFrame[]
}

/**
 * 文件关联状态（帮助面板的「设置」区回显用）。
 *
 * 键名与后端 `AssociationStateDto` 一一对应。
 */
export interface AssociationState {
  /** 已注册进「打开方式」列表的扩展名（小写、不含点） */
  registered: string[]
  /** 本程序 ProgID 前缀，如 `PurePhotoExplorer` */
  progIdPrefix: string
  /**
   * 说明文案（由 Rust 侧提供）。
   * 内容是「Windows 不允许程序自行设为默认」这类限制说明 ——
   * 放后端写一处，避免前后端两套说法。
   */
  note: string
}

/**
 * 批量设置文件关联时的单个条目（前端 → 后端）。
 *
 * 键名与后端 `AssocItemDto` 一一对应（`displayName` 是 camelCase 的注意点）。
 */
export interface AssociationItem {
  /** 扩展名（小写、不含点） */
  ext: string
  /** 写进注册表的类型显示名，如 `JPEG 图片` */
  displayName: string
}

/**
 * 打印结果。
 *
 * 键名与后端 `PrintResultDto` 一一对应。
 */
export interface PrintResult {
  /** 用户在系统打印对话框里点了取消（此时没有任何东西被送出） */
  cancelled: boolean
  /** 实际送印的像素尺寸（**含**已应用的旋转/裁剪） */
  width: number
  height: number
}
