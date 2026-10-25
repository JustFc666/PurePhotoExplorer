//! 取 **Windows 系统图标**给侧栏文件树用（盘符 / 文件夹 / 此电脑）。
//!
//! ## 为什么不用 emoji 或自绘 SVG
//! 资源管理器的盘符图标会区分**固定盘 / 可移动盘 / 光驱 / 网络位置**，这是系统
//! 才知道的信息；emoji 只有一个「光盘」形状，而且彩色 emoji 的观感和 Win10
//! 资源管理器差得很远（用户明确反馈「这里 ui 用 win10 的样式」）。
//!
//! ## 为什么输出 PNG 而不是 BMP
//! 图标靠 alpha 通道抠出形状。BMP 32bpp 里 alpha 位**不被渲染器保证使用**，
//! 浏览器画出来是一圈**黑色方块**（实测踩过）。PNG 原生支持 alpha，且项目
//! 已经依赖 `png` crate（缩略图编码在用），零新增成本。
//!
//! ## 为什么每个盘符单独取
//! 所有盘符共用 `C:\` 的图标时，连 D:/E: 都会带上「系统盘」的 Windows 徽标
//! （实测截图一眼能看出来）—— 可移动盘、光驱的图标更是完全不同。

use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::OsStrExt;

use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetObjectW, SelectObject,
    BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
};
use windows_sys::Win32::Storage::FileSystem::{GetLogicalDrives, FILE_ATTRIBUTE_DIRECTORY};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::UI::Shell::{
    SHGetFileInfoW, SHParseDisplayName, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGFI_PIDL,
    SHGFI_USEFILEATTRIBUTES,
};
use windows_sys::Win32::UI::Shell::Common::ITEMIDLIST;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, DrawIconEx, GetIconInfo, DI_NORMAL, HICON, ICONINFO,
};

fn wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// 取一个路径的系统图标，返回 `data:image/png;base64,…`。
///
/// - `path` 是**存在的**目录/盘符时按真实类型取（硬盘/U 盘/光驱各不相同）；
/// - `as_folder = true` 时用 `SHGFI_USEFILEATTRIBUTES` 按「文件夹」取，
///   路径不存在也能拿到标准的黄色文件夹图标。
pub fn icon_data_url(path: &str, as_folder: bool) -> Option<String> {
    icon_png(path, as_folder).map(|b| format!("data:image/png;base64,{}", base64(&b)))
}

/// 同上但返回**原始 PNG 字节**（测试里直接写文件看效果用）。
pub fn icon_png(path: &str, as_folder: bool) -> Option<Vec<u8>> {
    let mut w = wide(path);
    let mut flags = SHGFI_ICON | SHGFI_LARGEICON;
    if as_folder {
        flags |= SHGFI_USEFILEATTRIBUTES;
    }
    let mut info: SHFILEINFOW = unsafe { zeroed() };
    let ok = unsafe {
        SHGetFileInfoW(
            w.as_mut_ptr(),
            if as_folder { FILE_ATTRIBUTE_DIRECTORY } else { 0 },
            &mut info,
            size_of::<SHFILEINFOW>() as u32,
            flags,
        )
    };
    if ok == 0 || info.hIcon.is_null() {
        return None;
    }
    let png = unsafe { icon_to_png(info.hIcon) };
    unsafe { DestroyIcon(info.hIcon) };
    png
}

/// 把 `HICON` 画进 32bpp DIB，取出 RGBA，再编码成 PNG。
unsafe fn icon_to_png(hicon: HICON) -> Option<Vec<u8>> {
    // 真实尺寸：高 DPI 下可能是 48，别硬编码 32
    let mut ii: ICONINFO = zeroed();
    if GetIconInfo(hicon, &mut ii) == 0 {
        return None;
    }
    let mut bm: BITMAP = zeroed();
    if GetObjectW(
        ii.hbmColor,
        size_of::<BITMAP>() as i32,
        &mut bm as *mut _ as *mut c_void,
    ) == 0
    {
        return None;
    }
    let (w, h) = (bm.bmWidth, bm.bmHeight);
    if w <= 0 || h <= 0 || w > 256 || h > 256 {
        return None;
    }

    let dc = CreateCompatibleDC(std::ptr::null_mut());
    if dc.is_null() {
        return None;
    }
    // 负高度 = top-down DIB：读出来的像素顺序和 PNG 一致，不用翻行
    let bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..zeroed()
        },
        ..zeroed()
    };
    let mut bits: *mut c_void = std::ptr::null_mut();
    let dib = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
    if dib.is_null() || bits.is_null() {
        DeleteDC(dc);
        return None;
    }
    let old = SelectObject(dc, dib);
    DrawIconEx(dc, 0, 0, hicon, w, h, 0, std::ptr::null_mut(), DI_NORMAL);
    SelectObject(dc, old);

    let n = (w * h * 4) as usize;
    let src = std::slice::from_raw_parts(bits as *const u8, n);
    // DIB 是 BGRA，PNG 要 RGBA
    let mut rgba = Vec::with_capacity(n);
    for px in src.chunks_exact(4) {
        rgba.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
    }

    DeleteObject(dib);
    DeleteDC(dc);

    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().ok()?;
        wr.write_image_data(&rgba).ok()?;
    }
    Some(out)
}

/// 极简 base64（只为给几百字节的 PNG 编码，不引第三方 crate）。
fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let b = [
            c[0],
            c.get(1).copied().unwrap_or(0),
            c.get(2).copied().unwrap_or(0),
        ];
        let v = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        s.push(T[(v >> 18) as usize & 63] as char);
        s.push(T[(v >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 {
            T[(v >> 6) as usize & 63] as char
        } else {
            '='
        });
        s.push(if c.len() > 2 {
            T[v as usize & 63] as char
        } else {
            '='
        });
    }
    s
}

/// 侧栏文件树要用的图标（一次取齐，前端缓存）。
pub struct TreeIcons {
    /// 「此电脑」
    pub computer: Option<String>,
    /// 标准黄色文件夹（与具体路径无关）
    pub folder: Option<String>,
    /// **每个盘符各自的图标**（可移动盘 / 光驱 / 系统盘各不相同）
    pub drives: Vec<(String, String)>,
}

/// 「此电脑」的图标 —— CLSID 路径**必须先转成 PIDL**。
///
/// ⛔ `SHGetFileInfoW` 不认字符串形式的 `::{GUID}`（实测返回 0，只能回退 emoji，
/// 用户因此看到「我的电脑」是个彩色显示器 emoji，反馈「这里也要用 win10 的默认 ui」）。
/// 正确做法是 `SHParseDisplayName` 解析成 PIDL，再带 `SHGFI_PIDL` 去取。
fn computer_icon() -> Option<Vec<u8>> {
    const COMPUTER_CLSID: &str = "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}";
    let mut w = wide(COMPUTER_CLSID);
    let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
    let hr = unsafe {
        SHParseDisplayName(
            w.as_mut_ptr(),
            std::ptr::null_mut(),
            &mut pidl,
            0,
            std::ptr::null_mut(),
        )
    };
    if hr < 0 || pidl.is_null() {
        return None;
    }
    let mut info: SHFILEINFOW = unsafe { zeroed() };
    let ok = unsafe {
        SHGetFileInfoW(
            pidl as *const u16,
            0,
            &mut info,
            size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON | SHGFI_PIDL,
        )
    };
    let out = if ok != 0 && !info.hIcon.is_null() {
        unsafe { icon_to_png(info.hIcon) }
    } else {
        None
    };
    if ok != 0 && !info.hIcon.is_null() {
        unsafe { DestroyIcon(info.hIcon) };
    }
    unsafe { CoTaskMemFree(pidl as *const c_void) };
    out
}

pub fn tree_icons() -> TreeIcons {

    let mut drives = Vec::new();
    // 与 `fs_browse::roots` 同一套枚举：位掩码 → 盘符
    let mask = unsafe { GetLogicalDrives() };
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let root = format!("{}:\\", (b'A' + i as u8) as char);
        if let Some(url) = icon_data_url(&root, false) {
            drives.push((root, url));
        }
    }

    // 文件夹图标与路径无关，用 `as_folder` 拿标准样式。
    // 借一个一定存在的目录当名义路径即可。
    let base = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());

    TreeIcons {
        computer: computer_icon().map(|b| format!("data:image/png;base64,{}", base64(&b))),
        folder: icon_data_url(&base, true),
        drives,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// 把各类图标导出到 `Temp/icon_*.png`，供人工确认「是不是资源管理器那个图标」。
    /// 不参与断言（图标随系统主题/版本变化，钉死像素没意义）。
    #[test]
    fn 导出图标供人工查看() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../Temp");
        let _ = std::fs::create_dir_all(&dir);
        if let Some(b) = computer_icon() {
            let _ = std::fs::write(dir.join("icon_computer.png"), &b);
        }
        let cases = [
            ("folder", r"C:\Windows", true),
            ("drive_c", r"C:\", false),
            ("drive_d", r"D:\", false),
        ];
        for (name, path, as_folder) in cases {
            if let Some(b) = icon_png(path, as_folder) {
                let _ = std::fs::write(dir.join(format!("icon_{name}.png")), &b);
            }
        }
    }
}
