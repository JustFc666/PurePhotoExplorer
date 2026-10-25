//! 删除到 **回收站**（不是直接删）。
//!
//! ## 为什么必须走 Shell
//! `std::fs::remove_file` 是**永久删除**，在照片管理里这是不可接受的默认行为。
//! 用户点了「删除」的预期是「进回收站，还能捡回来」—— 这正是
//! `SHFileOperationW` + `FOF_ALLOWUNDO` 的语义。
//!
//! ## 为什么不是 `trash` crate
//! 那会为一条 Win32 调用引入一个依赖（还带着跨平台后端）。
//! 这台机器上只有 Windows，直接用 shell32 更可控，也便于解释失败原因。
//!
//! ## 调用约定
//! `pFrom` 是**双 null 结尾**的宽字符串列表：`"a\0b\0\0"`。
//! 少了最后那个 `\0`，系统会把相邻内存当成下一个路径继续处理。

#![cfg(windows)]

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;

/// `SHFileOperationW` 的操作码：删除
const FO_DELETE: u32 = 0x0003;
/// 允许撤销 → 进回收站
const FOF_ALLOWUNDO: u16 = 0x0040;
/// 不弹确认框（确认由我们自己的 UI 负责，避免两层弹窗）
const FOF_NOCONFIRMATION: u16 = 0x0010;
/// 不弹「正在删除」进度框（批量大时那个框会抢焦点）
const FOF_SILENT: u16 = 0x0004;
/// 出错时不弹系统消息框，错误码由返回值给出
const FOF_NOERRORUI: u16 = 0x0400;

#[repr(C)]
struct ShFileOpStructW {
    hwnd: *mut c_void,
    w_func: u32,
    p_from: *const u16,
    p_to: *const u16,
    f_flags: u16,
    f_any_operations_aborted: i32,
    h_name_mappings: *mut c_void,
    lpsz_progress_title: *const u16,
}

#[link(name = "shell32")]
extern "system" {
    fn SHFileOperationW(op: *mut ShFileOpStructW) -> i32;
}

/// 把一批文件移到回收站。
///
/// 返回 `Ok(())` 表示 Shell 报告全部成功；`Err` 里带的是给人看的原因。
/// ⚠️ 这个函数**不检查文件是否存在** —— 不存在的路径 Shell 会自己跳过并计入失败，
/// 调用方应当先自己过滤（顺便给用户更准确的计数）。
pub fn move_to_recycle_bin(paths: &[PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }

    // 双 null 结尾的宽字符路径列表
    let mut buf: Vec<u16> = Vec::new();
    for p in paths {
        buf.extend(p.as_os_str().encode_wide());
        buf.push(0);
    }
    buf.push(0);

    let mut op = ShFileOpStructW {
        hwnd: std::ptr::null_mut(),
        w_func: FO_DELETE,
        p_from: buf.as_ptr(),
        p_to: std::ptr::null(),
        f_flags: FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI,
        f_any_operations_aborted: 0,
        h_name_mappings: std::ptr::null_mut(),
        lpsz_progress_title: std::ptr::null(),
    };

    // SAFETY: `buf` 在调用期间一直存活；`op` 的指针全部指向它或 null。
    let code = unsafe { SHFileOperationW(&mut op) };

    if op.f_any_operations_aborted != 0 {
        return Err("操作被中止".into());
    }
    if code == 0 {
        return Ok(());
    }
    Err(shell_error_text(code))
}

/// Shell 的 `DE_*` 错误码 → 人话（这些码是 `SHFileOperationW` 特有的）。
fn shell_error_text(code: i32) -> String {
    match code {
        0x71 => "回收站已满或不可用".to_string(),
        0x74 => "源文件不存在".to_string(),
        0x75 => "目标文件已存在".to_string(),
        0x7C => "路径无效".to_string(),
        0x7E => "文件名无效".to_string(),
        0x7F => "文件系统不支持该操作".to_string(),
        0x81 => "文件正在被使用（请先关闭打开它的程序）".to_string(),
        0x82 => "无法读取源文件或磁盘".to_string(),
        0x83 => "无法写入目标".to_string(),
        0x84 => "来源和目标是同一个文件".to_string(),
        0x86 => "文件被占用".to_string(),
        0x87 => "参数无效".to_string(),
        0x88 => "内存不足".to_string(),
        0x89 => "路径太长".to_string(),
        0x8A => "文件名太长".to_string(),
        other => format!("删除失败（Shell 错误码 0x{other:X}）"),
    }
}
