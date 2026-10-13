/**
 * 格式清单 —— **全应用唯一真源**。
 *
 * ## 为什么要有这个文件
 * 之前「能打开哪些格式」这件事散在三处，各写一份：
 * 1. Rust `commands/app.rs::supported_formats()` 的能力表；
 * 2. `App.tsx::DIALOG_FILTERS` 打开对话框的过滤器；
 * 3. `HelpPanel.tsx::ASSOCIATION_TARGETS` 帮助面板设置区的勾选列表。
 *
 * 结果就是三者互不同步，出现了真实的用户可见问题：
 * - 「所有**支持的**图片」里列着 `tif` / `bmp` / `avif` 这些**当时就没有解码器**
 *   的格式 —— 用户能选中，选完只看到「暂不支持」（`webp` / `heic` 后来补上了）；
 * - 帮助面板能勾 `bmp` 去关联，但 `bmp` 同样打不开。
 *
 * 现在 2 与 3 都从这里派生。**Rust 侧那份仍需人工保持一致** ——
 * 两边的契约由 `apps/desktop/src-tauri/tests/anim_protocol.rs` 等测试和
 * `crates/pv-decode/src/bitmap/mod.rs` 的「支持范围」单测把守：
 * 声明了就必须真的有实现。
 *
 * ## 收录标准
 * **只有真能打开、且有测试覆盖的格式才进 `SUPPORTED_FORMATS`。**
 * 没有解码器的格式一律不列 —— 宁可不显示，也不要让用户点了才报错。
 */

/**
 * 已确认能打开的格式：`[扩展名, 显示标签]`。
 *
 * 标签可以重复（`jpg` 与 `jpeg` 都是 "JPEG"），因为它是给用户看的格式名。
 */
export const SUPPORTED_FORMATS: ReadonlyArray<readonly [string, string]> = [
  ['jpg', 'JPEG'],
  ['jpeg', 'JPEG'],
  ['png', 'PNG'],
  ['gif', 'GIF'],
  ['webp', 'WebP'],
  ['heic', 'HEIC'],
  ['heif', 'HEIC'],
  ['bmp', 'BMP'],
  ['tif', 'TIFF'],
  ['tiff', 'TIFF'],
  ['avif', 'AVIF'],
  ['psd', 'PSD'],
  // ── RAW：走「扫内嵌 JPEG 预览」，已用真实样本实测 ──
  ['cr3', 'Canon CR3'],
  ['dng', 'Adobe DNG'],
  ['nrw', 'Nikon NRW'],
  ['raf', 'Fujifilm RAF'],
  ['rw2', 'Panasonic RW2'],
  ['orf', 'Olympus ORF'],
  ['sr2', 'Sony SR2'],
  ['srf', 'Sony SRF'],
]

/**
 * 尚未实测的 RAW 变体。
 *
 * 走的是同一套内嵌预览管线，理论上能开，但没有真实样本验证 ——
 * 所以**不进** `SUPPORTED_FORMATS`（不承诺），只出现在打开对话框的
 * 「相机 RAW」过滤器里，让用户手上有这些文件时能选中去试。
 */
export const UNVERIFIED_RAW_EXTS: readonly string[] = ['cr2', 'nef', 'arw', 'pef', 'iiq']

/** 支持列表中属于 RAW 的扩展名（用于分组过滤） */
export const VERIFIED_RAW_EXTS: readonly string[] = ['cr3', 'dng', 'nrw', 'raf', 'rw2', 'orf', 'sr2', 'srf']

/**
 * 「打开方式」可关联的格式。
 *
 * **刻意排除 RAW**：把它们设成默认打开方式体验很差 —— 双击一个 26MB 的
 * CR3 只是想看看，却要等内嵌预览提取。用户真想关联可以自己从
 * Windows 的「打开方式」里选。
 */
export const ASSOCIATION_FORMATS: ReadonlyArray<readonly [string, string]> =
  SUPPORTED_FORMATS.filter(([ext]) => !VERIFIED_RAW_EXTS.includes(ext))

/** 支持格式的扩展名数组（顺序稳定，供对话框过滤器使用） */
export const SUPPORTED_EXTS: readonly string[] = SUPPORTED_FORMATS.map(([e]) => e)

/** 「打开方式」候选的扩展名数组（与 `HelpPanel` 显示的勾选项一一对应） */
export const ASSOCIATION_EXTS: readonly string[] = ASSOCIATION_FORMATS.map(([e]) => e)
