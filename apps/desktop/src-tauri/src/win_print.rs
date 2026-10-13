//! Windows 打印（`PrintDlgW` 系统打印对话框 + GDI `StretchDIBits`）。
//!
//! ## 为什么走 PrintDlg + GDI，而不是「用文件关联去打印」
//! 最省事的做法是 `ShellExecuteW("print", path)` —— 但那是把**原始文件**
//! 交给别的程序去印：用户在 PurePhotoExplorer 里裁剪/旋转后点打印，印出来的却是原图。
//!
//! 自己拿 DC 画则完全可控：我们已经有变换后的 RGBA 像素，直接
//! `StretchDIBits` 贴到打印机 DC 上即可，所见即所印。
//!
//! ## 流程
//! 1. `PrintDlgW` 弹系统对话框（选打印机、纸张、份数、方向）——
//!    拿到打印机 `HDC`。用户取消时返回 0。
//! 2. `GetDeviceCaps` 问出**可打印区域**（`HORZRES`/`VERTRES`）与**DPI**
//!    （`LOGPIXELSX/Y`）。注意：打印机 DC 的 `HORZRES` 已经是页边距之内的
//!    区域，不能按 A4 的毫米尺寸硬算。
//! 3. 按「适应页面」或「原始尺寸」算出目标矩形并居中。
//! 4. `StartDocW` → `StartPage` → `StretchDIBits` → `EndPage` → `EndDoc`。
//!
//! ## 两个易错点
//! - **`StretchDIBits` 要 BGRA**，我们的像素是 RGBA —— 必须换序，
//!   否则印出来红蓝互换（人脸变蓝色是最典型的症状）。
//! - **`biHeight` 用负数**表示自上而下的 DIB。写正数会让图像**上下颠倒**
//!   （DIB 默认原点在左下角）。
//! - 拉伸模式要设 `HALFTONE` 并 `SetBrushOrgEx` 复位，否则打印机 DC 默认的
//!   `COLORONCOLOR` 会把照片抽出严重锯齿。

use pv_core::error::{PvError, Result};
use pv_decode::RgbaImage;

/// GDI `DEVMODE` 字段标志与方向常量（windows-sys 已提供同名常量，这里统一引入）。
#[cfg(windows)]
use windows_sys::Win32::Graphics::Gdi::{
    CreateDCW, DeleteDC, DM_ORIENTATION, DM_OUT_BUFFER, DM_PAPERSIZE, DM_PAPERLENGTH, DM_PAPERWIDTH,
    DM_PRINTQUALITY, DM_YRESOLUTION, DMORIENT_LANDSCAPE, DMORIENT_PORTRAIT, DEVMODEW, ResetDCW,
};
#[cfg(windows)]
use windows_sys::Win32::Graphics::Printing::{
    ClosePrinter, DocumentPropertiesW, GetDefaultPrinterW, OpenPrinterW,
};
#[cfg(windows)]
use windows_sys::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_FIXED,
};
#[cfg(windows)]
use windows_sys::Win32::Foundation::{GlobalFree, HANDLE};

/// 页边距默认值（毫米）。多数打印机的物理不可打印区在 5mm 以内，
/// 留 10mm 既安全又不至于浪费纸面。用户可在预览浮层自行调整。
pub const DEFAULT_MARGIN_MM: f64 = 10.0;

/// 原始尺寸模式下的参考 DPI。
///
/// 图像没有物理尺寸信息（我们不读 DPI 元数据），所以按屏幕惯用的 96 DPI
/// 换算 —— 这样「原始尺寸」意味着"和屏幕上看到的一样大"，
/// 符合用户对"不缩放"的直觉。
const ASSUMED_IMAGE_DPI: f64 = 96.0;

/// 打印质量（DEVMODE `dmPrintQuality` 取值；负数 = 驱动预设档）。
const DMQUALITY_DRAFT: i16 = -4; // DMQUALITY_DRAFT
const DMQUALITY_MEDIUM: i16 = -2; // DMQUALITY_MEDIUM
const DMQUALITY_HIGH: i16 = -1; // DMQUALITY_HIGH

/// 目标矩形的计算（**纯函数，单独测**）。
///
/// 输入都是设备像素：`page_w/page_h` 是可打印区域，`img_w/img_h` 是图像像素，
/// `dpi_x/dpi_y` 是打印机 DPI。返回 `(x, y, 宽, 高)`。
///
/// - `fit = true`：等比缩放到铺满可打印区域（不放大超过 1:1？—— 会放大，
///   因为小图铺满纸面才正常）；
/// - `fit = false`：按 [`ASSUMED_IMAGE_DPI`] 换算成"屏幕大小"，
///   超出页面时才等比缩小（否则会印到纸外面）。
fn layout(
    page_w: i32,
    page_h: i32,
    img_w: u32,
    img_h: u32,
    dpi_x: f64,
    dpi_y: f64,
    margin_x: i32,
    margin_y: i32,
    fit: bool,
) -> (i32, i32, i32, i32) {
    let avail_w = (page_w - margin_x * 2).max(1);
    let avail_h = (page_h - margin_y * 2).max(1);
    let (iw, ih) = (img_w.max(1) as f64, img_h.max(1) as f64);

    let scale = if fit {
        // 适应页面：取两个方向里较小的缩放比（等比，不会拉变形）
        (avail_w as f64 / iw).min(avail_h as f64 / ih)
    } else {
        // 原始尺寸：先按「96dpi 逻辑尺寸 → 设备像素」换算，
        // 再在**超出页面时**才等比缩小（否则会印到纸外被裁掉）。
        // 用 min(dpi_x, dpi_y) 而不是分别乘：分别乘会把图像拉变形。
        let dpi = dpi_x.min(dpi_y).max(1.0);
        let base = dpi / ASSUMED_IMAGE_DPI;
        let w_at_base = iw * base;
        let h_at_base = ih * base;
        let shrink = (avail_w as f64 / w_at_base)
            .min(avail_h as f64 / h_at_base)
            .min(1.0);
        base * shrink
    };

    let dw = (iw * scale).round().max(1.0) as i32;
    let dh = (ih * scale).round().max(1.0) as i32;
    let dx = margin_x + (avail_w - dw) / 2;
    let dy = margin_y + (avail_h - dh) / 2;
    (dx, dy, dw, dh)
}

/// RGBA → BGRA（in-place，交换 R 与 B）
fn rgba_to_bgra(data: &mut [u8]) {
    for px in data.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
}

/// 【方案 A】Win32 PrintDlg + GDI 直绘（当前默认引擎）。
///
/// 把变换后的 RGBA 像素直接 `StretchDIBits` 到打印机 DC，「所见即所印」，
/// 不落临时文件、纯 Rust、零外部依赖。想切到方案 B（`print_shell`）只需把
/// `commands/print.rs` 里的 `USE_SHELL_PRINT` 改成 `true`。
///
/// `orientation`：`"portrait"` / `"landscape"`；`quality`：`"draft"` / `"normal"` / `"high"`；
/// `paper_w_mm` / `paper_h_mm`：整纸物理尺寸（毫米，纵向）。三者都在 `PrintDlgW`
/// 返回后写进 `hDevMode` 再 `ResetDCW`，让系统对话框的默认值跟随我们预览里的选择
/// （避免「预览选了横向、印出来是纵向」这类不一致）。
///
/// `margin_mm`：页边距（毫米，四边同值）——预览浮层可调，必须与预览用同值。
#[cfg(windows)]
pub fn print_rgba_gdi(
    img: &RgbaImage,
    fit: bool,
    doc_name: &str,
    orientation: &str,
    quality: &str,
    paper_w_mm: f64,
    paper_h_mm: f64,
    margin_mm: f64,
) -> Result<bool> {
    use windows_sys::Win32::Graphics::Gdi::DeleteDC;
    use windows_sys::Win32::UI::Controls::Dialogs::{
        PrintDlgW, PRINTDLGW, PD_RETURNDC, PD_USEDEVMODECOPIESANDCOLLATE,
    };

    // ── 1. 系统打印对话框 ──
    let mut pd: PRINTDLGW = unsafe { std::mem::zeroed() };
    pd.lStructSize = std::mem::size_of::<PRINTDLGW>() as u32;
    pd.Flags = PD_RETURNDC | PD_USEDEVMODECOPIESANDCOLLATE;
    pd.nCopies = 1;

    // SAFETY: pd 已按 PRINTDLGW 初始化且 lStructSize 正确；函数失败时返回 0。
    let ok = unsafe { PrintDlgW(&mut pd) };
    if ok == 0 {
        // 用户取消。DialogBox 之后 hDevMode/hDevNames 可能已被分配，需要释放。
        free_print_dlg_buffers(&mut pd);
        return Ok(true);
    }

    let hdc = pd.hDC;
    if hdc.is_null() {
        free_print_dlg_buffers(&mut pd);
        return Err(PvError::Decode("打印对话框没有返回打印机 DC".into()));
    }

    // 应用预览里选的方向/质量/纸张：写进 hDevMode 后 ResetDC，让 DC 与送印一致
    // （否则系统对话框的默认值会覆盖我们 UI 的选择）。
    unsafe {
        apply_devmode(pd.hDevMode, orientation, quality, paper_w_mm, paper_h_mm);
        if !pd.hDevMode.is_null() {
            // SAFETY: hDevMode 指向一个合法 DEVMODEW（PrintDlg 分配的全局内存块）。
            ResetDCW(hdc, pd.hDevMode as *const DEVMODEW);
        }
    }

    let result = render_to_dc(hdc, img, fit, doc_name, margin_mm);

    // ── 清理：DC 与 PrintDlg 分配的两个全局内存块 ──
    unsafe { DeleteDC(hdc) };
    free_print_dlg_buffers(&mut pd);

    result.map(|_| false)
}

/// 【方案 A·直接打印】跳过系统打印对话框，直绘到指定（或默认）打印机。
///
/// 与 [`print_rgba_gdi`] 的唯一区别：**不弹 `PrintDlgW`**，而是
/// `printer` 指定（`None` = `GetDefaultPrinterW` 默认打印机）→
/// `OpenPrinterW` + `DocumentPropertiesW` 取它的默认 `DEVMODE` →
/// 写入我们 UI 选的方向/质量/纸张 → `CreateDCW` 拿 DC 直绘。
/// `render_to_dc` 两边完全复用，所以排版/旋转/调色/页边距一致。
///
/// 适用场景：用户在预览里选好打印机后「一键送印」。
/// 代价：没有份数选择（固定 1 份）、没有「取消」机会
/// （点了就送进打印队列），故返回 `Ok(false)`（永不 `cancelled`）。
#[cfg(windows)]
pub fn print_rgba_gdi_direct(
    img: &RgbaImage,
    fit: bool,
    doc_name: &str,
    orientation: &str,
    quality: &str,
    paper_w_mm: f64,
    paper_h_mm: f64,
    margin_mm: f64,
    printer: Option<&str>,
) -> Result<bool> {
    use std::ptr::null_mut;

    // 1. 打印机名：显式指定优先，否则用系统默认
    let printer_name: Vec<u16> = match printer {
        Some(name) if !name.trim().is_empty() => name
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect(),
        _ => {
            let mut name_buf = [0u16; 256];
            let mut name_len = name_buf.len() as u32;
            // SAFETY: name_buf 是静态分配的 256 u16 缓冲，name_len 初始为其长度；
            // 函数写入字符数（不含 null），缓冲区足够。
            if unsafe { GetDefaultPrinterW(name_buf.as_mut_ptr(), &mut name_len) } == 0 {
                return Err(PvError::Decode(
                    "未找到默认打印机，请先在系统设置里选择一个默认打印机".into(),
                ));
            }
            // name_len 是字符数（不含 null），补齐 null 结尾
            let mut v: Vec<u16> = name_buf[..name_len as usize].to_vec();
            v.push(0);
            v
        }
    };

    // 2. 打开打印机，取默认 DEVMODE 所需大小
    let mut hprinter: HANDLE = null_mut();
    // SAFETY: printer_name 是 null 结尾的 UTF-16 缓冲，hprinter 用于接收句柄，
    // pdefault 传 null（使用默认）。调用期间这些缓冲均有效。
    if unsafe { OpenPrinterW(printer_name.as_ptr(), &mut hprinter, null_mut()) } == 0 {
        return Err(PvError::Decode("无法打开所选打印机".into()));
    }

    let needed = unsafe {
        DocumentPropertiesW(null_mut(), hprinter, printer_name.as_ptr(), null_mut(), null_mut(), 0)
    };
    if needed <= 0 {
        unsafe { ClosePrinter(hprinter) };
        return Err(PvError::Decode("无法获取打印机默认设置".into()));
    }

    // 3. 分配 DEVMODE 全局内存并取出默认设置
    let hmem = unsafe { GlobalAlloc(GMEM_FIXED, needed as usize) };
    if hmem.is_null() {
        unsafe { ClosePrinter(hprinter) };
        return Err(PvError::Decode("内存分配失败（DEVMODE）".into()));
    }
    {
        let dm = unsafe { GlobalLock(hmem) as *mut DEVMODEW };
        if dm.is_null() {
            unsafe {
                GlobalFree(hmem);
                ClosePrinter(hprinter);
            }
            return Err(PvError::Decode("无法锁定 DEVMODE 内存".into()));
        }
        unsafe {
            DocumentPropertiesW(
                null_mut(),
                hprinter,
                printer_name.as_ptr(),
                dm,
                null_mut(),
                DM_OUT_BUFFER,
            );
            GlobalUnlock(hmem);
        }
    }

    // 4. 应用预览里选的方向 / 质量 / 纸张（复用与 gdi 版同样的 helper）
    unsafe {
        apply_devmode(hmem, orientation, quality, paper_w_mm, paper_h_mm);
    }

    // 5. 用这份 DEVMODE 创建打印机 DC（不弹对话框）
    let winspool: Vec<u16> = "WINSPOOL\0".encode_utf16().collect();
    let dm_ptr = unsafe { GlobalLock(hmem) as *const DEVMODEW };
    let dc = unsafe {
        CreateDCW(
            winspool.as_ptr(),
            printer_name.as_ptr(),
            null_mut(),
            dm_ptr,
        )
    };
    unsafe {
        GlobalUnlock(hmem);
        ClosePrinter(hprinter);
    }
    if dc.is_null() {
        unsafe { GlobalFree(hmem) };
        return Err(PvError::Decode("无法创建打印机设备上下文".into()));
    }

    let result = render_to_dc(dc, img, fit, doc_name, margin_mm);

    unsafe {
        DeleteDC(dc);
        GlobalFree(hmem);
    }
    result.map(|_| false)
}

/// 枚举本机可用的打印机名（本地 + 已连接的网络打印机），供「打印机」下拉框用。
///
/// 用 `EnumPrintersW` **Level 4**：只查注册表、不唤醒脱机的网络打印机，
/// 拿名字足够快。失败（如打印服务未启动）时返回空列表 —— 前端下拉框
/// 退化为「系统默认打印机」一项，不阻塞打印。
#[cfg(windows)]
pub fn list_printers() -> Vec<String> {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Graphics::Printing::{
        EnumPrintersW, PRINTER_ENUM_CONNECTIONS, PRINTER_ENUM_LOCAL, PRINTER_INFO_4W,
    };

    const LEVEL: u32 = 4;
    let flags = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;

    unsafe {
        // 第一次调用只拿所需缓冲大小（cbBuf=0 必然失败，pcbNeeded 带回真实大小）
        let mut needed: u32 = 0;
        let mut returned: u32 = 0;
        let _ = EnumPrintersW(flags, null(), LEVEL, null_mut(), 0, &mut needed, &mut returned);
        if needed == 0 {
            return Vec::new();
        }

        let mut buf = vec![0u8; needed as usize];
        if EnumPrintersW(
            flags,
            null(),
            LEVEL,
            buf.as_mut_ptr(),
            needed,
            &mut needed,
            &mut returned,
        ) == 0
        {
            return Vec::new();
        }
        if returned == 0 {
            return Vec::new();
        }

        let items =
            std::slice::from_raw_parts(buf.as_ptr() as *const PRINTER_INFO_4W, returned as usize);
        items
            .iter()
            .filter(|i| !i.pPrinterName.is_null())
            .map(|i| {
                // 宽字符读到 null 终止
                let mut len = 0usize;
                while *i.pPrinterName.add(len) != 0 {
                    len += 1;
                }
                String::from_utf16_lossy(std::slice::from_raw_parts(i.pPrinterName, len))
            })
            .collect()
    }
}

/// 非 Windows：无打印机列表。
#[cfg(not(windows))]
pub fn list_printers() -> Vec<String> {
    Vec::new()
}

/// 释放 PrintDlg 分配的 `hDevMode` / `hDevNames`。
///
/// ⚠️ **必须释放**：`PrintDlgW` 用 `GlobalAlloc` 分配，不释放就是每次打印
/// 泄漏一块（DEVMODE 几百字节、DEVNAMES 上百字节）。系统对话框属于
/// "用户可能开很多次"的路径，不能按"一次性"糊弄过去。
#[cfg(windows)]
fn free_print_dlg_buffers(pd: &mut windows_sys::Win32::UI::Controls::Dialogs::PRINTDLGW) {
    use windows_sys::Win32::Foundation::GlobalFree;
    unsafe {
        if !pd.hDevMode.is_null() {
            GlobalFree(pd.hDevMode);
            pd.hDevMode = std::ptr::null_mut();
        }
        if !pd.hDevNames.is_null() {
            GlobalFree(pd.hDevNames);
            pd.hDevNames = std::ptr::null_mut();
        }
    }
}

/// 把像素画到打印机 DC 上。
///
/// 排版计算已抽到 [`layout`]（可单测）；这里只做 GDI 调用与几个易错点：
/// 拉伸模式必须是 `HALFTONE`，DIB 必须 top-down（`biHeight` 为负），
/// 像素必须是 BGRA。`margin_mm`：四边页边距（毫米），与预览浮层同值。
#[cfg(windows)]
fn render_to_dc(
    hdc: windows_sys::Win32::Graphics::Gdi::HDC,
    img: &RgbaImage,
    fit: bool,
    doc_name: &str,
    margin_mm: f64,
) -> Result<()> {
    use std::ffi::c_void;
    use windows_sys::Win32::Graphics::Gdi::{
        GetDeviceCaps, SetBrushOrgEx, SetStretchBltMode, StretchDIBits, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HALFTONE, HORZRES, LOGPIXELSX, LOGPIXELSY,
        SRCCOPY, VERTRES,
    };
    use windows_sys::Win32::Storage::Xps::{DOCINFOW, EndDoc, EndPage, StartDocW, StartPage};

    let (page_w, page_h, dpi_x, dpi_y) = unsafe {
        (
            // 常量是 u32（GET_DEVICE_CAPS_INDEX），函数要 i32 —— 必须显式转
            GetDeviceCaps(hdc, HORZRES as i32),
            GetDeviceCaps(hdc, VERTRES as i32),
            GetDeviceCaps(hdc, LOGPIXELSX as i32) as f64,
            GetDeviceCaps(hdc, LOGPIXELSY as i32) as f64,
        )
    };
    if page_w <= 0 || page_h <= 0 {
        return Err(PvError::Decode("打印机报告的页面尺寸无效".into()));
    }
    let dpi_x = if dpi_x > 0.0 { dpi_x } else { 96.0 };
    let dpi_y = if dpi_y > 0.0 { dpi_y } else { 96.0 };

    let margin_x = (margin_mm / 25.4 * dpi_x) as i32;
    let margin_y = (margin_mm / 25.4 * dpi_y) as i32;
    let (dx, dy, dw, dh) = layout(
        page_w, page_h, img.width, img.height, dpi_x, dpi_y, margin_x, margin_y, fit,
    );

    // DIB：32bpp、自上而下（biHeight 为负，否则图像上下颠倒）
    let mut bmi: BITMAPINFO = unsafe { std::mem::zeroed() };
    bmi.bmiHeader = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: img.width as i32,
        biHeight: -(img.height as i32),
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        ..unsafe { std::mem::zeroed() }
    };

    // GDI 要 BGRA，源是 RGBA（不换序会红蓝互换）
    let mut bgra = img.data.clone();
    rgba_to_bgra(&mut bgra);

    let name: Vec<u16> = doc_name.encode_utf16().chain(std::iter::once(0)).collect();
    let di = DOCINFOW {
        cbSize: std::mem::size_of::<DOCINFOW>() as i32,
        lpszDocName: name.as_ptr(),
        lpszOutput: std::ptr::null(),
        lpszDatatype: std::ptr::null(),
        fwType: 0,
    };

    unsafe {
        if StartDocW(hdc, &di) <= 0 {
            return Err(PvError::Decode("无法开始打印作业".into()));
        }
        if StartPage(hdc) <= 0 {
            EndDoc(hdc);
            return Err(PvError::Decode("无法开始打印页面".into()));
        }

        // 高质量插值；默认的 COLORONCOLOR 会把照片抽成锯齿
        SetStretchBltMode(hdc, HALFTONE);
        SetBrushOrgEx(hdc, 0, 0, std::ptr::null_mut());

        let n = StretchDIBits(
            hdc,
            dx,
            dy,
            dw,
            dh,
            0,
            0,
            img.width as i32,
            img.height as i32,
            bgra.as_ptr() as *const c_void,
            &bmi,
            DIB_RGB_COLORS,
            SRCCOPY,
        );

        EndPage(hdc);
        EndDoc(hdc);

        if n == 0 || n == i32::MAX {
            // GDI_ERROR = -1
            return Err(PvError::Decode(
                "把图像送到打印机失败（GDI StretchDIBits 返回错误）".into(),
            ));
        }
    }

    tracing::info!(
        target: "photovault::print",
        image = %format!("{}x{}", img.width, img.height),
        page = %format!("{page_w}x{page_h}@{dpi_x:.0}dpi"),
        dest = %format!("{dw}x{dh} @ ({dx},{dy})"),
        fit,
        "已提交打印作业"
    );
    Ok(())
}

/// 非 Windows 平台：明确报不支持，而不是编译不过。
#[cfg(not(windows))]
pub fn print_rgba_gdi(
    _img: &RgbaImage,
    _fit: bool,
    _doc_name: &str,
    _orientation: &str,
    _quality: &str,
    _paper_w_mm: f64,
    _paper_h_mm: f64,
    _margin_mm: f64,
) -> Result<bool> {
    Err(PvError::UnsupportedFormat(
        "打印目前只在 Windows 上实现".into(),
    ))
}

#[cfg(not(windows))]
pub fn print_rgba_gdi_direct(
    _img: &RgbaImage,
    _fit: bool,
    _doc_name: &str,
    _orientation: &str,
    _quality: &str,
    _paper_w_mm: f64,
    _paper_h_mm: f64,
    _margin_mm: f64,
    _printer: Option<&str>,
) -> Result<bool> {
    Err(PvError::UnsupportedFormat(
        "打印目前只在 Windows 上实现".into(),
    ))
}

/// 把预览里选的方向 / 质量 / 纸张写进 `hDevMode`。
///
/// - 方向：`dmOrientation`（纵向 / 横向）。
/// - 质量：`dmPrintQuality`（负数 = 驱动预设档：草稿 -4 / 普通 -2 / 高 -1），
///   同时 `dmYResolution` 给一个代表性 DPI。
/// - 纸张：自定义尺寸用 `dmPaperLength` / `dmPaperWidth`（单位 0.1mm），
///   `dmPaperSize = 0` 表示「用户自定义」，这样任意照片纸（4×6 等）都能下发，
///   不依赖驱动是否恰好定义同名 `DMPAPER_*` 常量。
///
/// 仅在 `hDevMode` 有效时生效；调用方随后应 `ResetDCW` 让 DC 重新读取这份 DEVMODE。
#[cfg(windows)]
unsafe fn apply_devmode(
    h: windows_sys::Win32::Foundation::HANDLE,
    orientation: &str,
    quality: &str,
    paper_w_mm: f64,
    paper_h_mm: f64,
) {
    if h.is_null() {
        return;
    }
    let dm = GlobalLock(h) as *mut DEVMODEW;
    if dm.is_null() {
        return;
    }

    let orient = if orientation == "landscape" {
        DMORIENT_LANDSCAPE as i16
    } else {
        DMORIENT_PORTRAIT as i16
    };
    let qual = match quality {
        "draft" => DMQUALITY_DRAFT,
        "normal" => DMQUALITY_MEDIUM,
        _ => DMQUALITY_HIGH,
    };

    (*dm).dmFields |= DM_ORIENTATION
        | DM_PAPERSIZE
        | DM_PAPERLENGTH
        | DM_PAPERWIDTH
        | DM_PRINTQUALITY
        | DM_YRESOLUTION;
    (*dm).Anonymous1.Anonymous1.dmOrientation = orient;
    (*dm).Anonymous1.Anonymous1.dmPaperSize = 0; // 0 = 用户自定义纸张
    (*dm).Anonymous1.Anonymous1.dmPaperLength = (paper_h_mm * 10.0).round().clamp(1.0, 32767.0) as i16;
    (*dm).Anonymous1.Anonymous1.dmPaperWidth = (paper_w_mm * 10.0).round().clamp(1.0, 32767.0) as i16;
    (*dm).Anonymous1.Anonymous1.dmPrintQuality = qual;
    (*dm).dmYResolution = 300;

    GlobalUnlock(h);
}

/// 【方案 B】`ShellExecuteW("print", 原文件)` —— 把文件交给系统默认关联程序打印。
///
/// ## 与方案 A 的区别
/// - A 自己拿像素画（尊重旋转/裁剪/调色，「所见即所印」）；
/// - B **只把原始文件**交给默认看图器，**不应用任何编辑** —— 旋转/裁剪/调色都会丢失。
///   这正是当初否决 B 的原因；这里仅作为可切换的备选实现供对比测试。
///
/// ## 行为
/// - 同步返回「是否已发起」：调起默认程序的打印流程（多数看图器会再弹自己的打印对话框）。
/// - 返回 `Ok(false)` 表示已成功发起；`ShellExecuteW` 返回值 ≤ 32 视为失败（如没有程序关联
///   `print` 动词 → `SE_ERR_NOASSOC`）。
/// - 本函数本身不弹打印对话框、也无法感知「用户取消」，故不返回 `cancelled=true`。
///
/// ## 已知脆弱点
/// 依赖文件类型已注册 `print` 动词。现代 Windows 上图片的 `print` 动词未必默认注册
/// （Photos / 旧版照片查看器才注册），未注册时 `ShellExecuteW` 会失败。
#[cfg(windows)]
pub fn print_shell(path: &std::path::Path) -> Result<bool> {
    use std::iter::once;
    use std::ptr::null_mut;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let path_str = path.to_string_lossy().into_owned();
    // "print" 动词 + 显式 null 结尾
    let operation: Vec<u16> = "print\0".encode_utf16().collect();
    // 路径 + 显式 null 结尾
    let file: Vec<u16> = path_str.encode_utf16().chain(once(0)).collect();

    // SAFETY: 标准 ShellExecuteW 调用。hwnd 传 null（用桌面作父窗口）；
    // 入参均为静态生命周期的 UTF-16 缓冲，调用期间有效。
    let ret = unsafe {
        ShellExecuteW(
            null_mut(),
            operation.as_ptr(),
            file.as_ptr(),
            null_mut(),
            null_mut(),
            SW_SHOWNORMAL,
        )
    };

    // ShellExecuteW 返回 HINSTANCE（指针型）：> 32 成功，≤ 32 为错误码（SE_ERR_*）。
    let ret_val = ret as usize;
    if ret_val <= 32 {
        return Err(PvError::Decode(format!(
            "无法通过系统默认程序打印（ShellExecuteW 返回 {ret_val}，可能是该文件类型未关联 print 动词）"
        )));
    }
    Ok(false)
}

/// 非 Windows 平台：明确报不支持。
#[cfg(not(windows))]
pub fn print_shell(_path: &std::path::Path) -> Result<bool> {
    Err(PvError::UnsupportedFormat(
        "打印目前只在 Windows 上实现".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 适应页面：等比缩放到铺满（受较短边限制），并居中
    #[test]
    fn 适应页面时等比缩放并居中() {
        // 页面 1000×800 设备像素，边距 0 → 可用 1000×800
        // 图像 4000×1000（4:1 横条）→ 受宽度限制，缩放 0.25 → 1000×250
        let (x, y, w, h) = layout(1000, 800, 4000, 1000, 300.0, 300.0, 0, 0, true);
        assert_eq!((w, h), (1000, 250));
        assert_eq!(x, 0, "宽度已铺满，水平不偏移");
        assert_eq!(y, (800 - 250) / 2, "垂直居中");
    }

    #[test]
    fn 适应页面时竖图受高度限制() {
        // 图像 1000×4000 在 1000×800 页面上 → 受高度限制，缩放 0.2 → 200×800
        let (x, y, w, h) = layout(1000, 800, 1000, 4000, 300.0, 300.0, 0, 0, true);
        assert_eq!((w, h), (200, 800));
        assert_eq!(y, 0);
        assert_eq!(x, (1000 - 200) / 2);
    }

    /// 原始尺寸：小图**不放大**（保持 96dpi 的"屏幕大小"）
    #[test]
    fn 原始尺寸模式不放大() {
        // 打印机 300dpi → 200×100 的图像占 625×312 设备像素（×3.125）
        let (_, _, w, h) = layout(3000, 2400, 200, 100, 300.0, 300.0, 0, 0, false);
        // 300dpi ÷ 96dpi = 3.125 倍：200→625，100→312.5（取整可能差 1）
        assert!((w - 625).abs() <= 1, "宽应按 DPI 换算，实际 {w}");
        assert!((h - 313).abs() <= 1, "高应按 DPI 换算，实际 {h}");

        // 对比：适应页面模式在同样输入下会放大到铺满
        let (_, _, fw, fh) = layout(3000, 2400, 200, 100, 300.0, 300.0, 0, 0, true);
        assert!(fw > w, "适应页面模式应比原始尺寸大");
        assert_eq!((fw, fh), (3000, 1500));
    }

    /// 原始尺寸超出页面时必须缩小，否则会印到纸外
    #[test]
    fn 原始尺寸超出页面时缩小到能装下() {
        // 图像 2000×2000 在 300dpi 下是 6250×6250 设备像素，页面只有 3000×2400
        let (x, y, w, h) = layout(3000, 2400, 2000, 2000, 300.0, 300.0, 0, 0, false);
        assert!(w <= 3000 && h <= 2400, "必须能装进页面，实际 {w}x{h}");
        assert_eq!(w, h, "方形图缩后仍应是方形");
        assert!(x >= 0 && y >= 0, "左上角不能为负");
    }

    #[test]
    fn 页边距体现在偏移上() {
        // 边距 30 → 可用 940×740，图像 100×100 适应后铺满较短边
        let (x, y, w, _h) = layout(1000, 800, 100, 100, 300.0, 300.0, 30, 30, true);
        assert!(x >= 30, "水平偏移应至少含左边距，实际 {x}");
        assert!(y >= 30, "垂直偏移应至少含上边距，实际 {y}");
        assert!(w <= 940, "宽度不能超过可用区");
    }

    /// 极端输入不能产生 0 或负数尺寸（GDI 会静默不画）
    #[test]
    fn 退化输入不产生非法尺寸() {
        for (pw, ph, iw, ih) in [(1, 1, 1, 1), (10, 10, 100000, 1), (800, 600, 1, 100000)] {
            let (x, y, w, h) = layout(pw, ph, iw, ih, 96.0, 96.0, 0, 0, true);
            assert!(w >= 1 && h >= 1, "尺寸必须为正：{pw}x{ph} 图 {iw}x{ih} → {w}x{h}");
            assert!(x >= 0 && y >= 0);
        }
    }

    #[test]
    fn RGBA转BGRA只交换红蓝() {
        let mut d = vec![1, 2, 3, 4, 5, 6, 7, 8];
        rgba_to_bgra(&mut d);
        assert_eq!(d, vec![3, 2, 1, 4, 7, 6, 5, 8], "R↔B 交换，G 与 A 不动");
    }
}
