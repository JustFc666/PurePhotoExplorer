//! 应用级命令：`app_info` / `supported_formats` / 文件关联。

use super::dto::{AppInfoDto, AssocItemDto, AssociationStateDto, ErrorDto, FormatCapability};
use crate::win_assoc;

/// 文件关联失败的错误（用户可见 —— 用户需要知道为什么没成功）
fn assoc_err(message: String) -> ErrorDto {
    ErrorDto {
        code: "association_failed".to_string(),
        message,
        user_visible: true,
    }
}

/// 应用信息（供前端显示版本与能力）
#[tauri::command]
pub fn app_info() -> AppInfoDto {
    AppInfoDto {
        version: env!("CARGO_PKG_VERSION").to_string(),
        backends: vec![
            "bitmap-pure-rust".to_string(),
            "raw-embedded-preview".to_string(),
        ],
        // Stage 1 已接入：能看图 + 显示拍摄参数（纯 Rust 提取内嵌预览）
        raw_enabled: true,
    }
}

/// 最大化：让**客户区**精确铺满工作区（物理像素坐标）。
///
/// 不用系统 `SW_MAXIMIZE`：`decorations: false` 下 tao 无法摘掉
/// `WS_EX_WINDOWEDGE`（每次 flag 变化都会被 `apply_diff` 写回）。
/// 该残留边框在客户区外侧留了一圈不可见非客户区（本机 DPI 120 实测
/// 左 9 / 上 0 / 右 9 / 下 9 物理 px），若只把外框摆到工作区，
/// 客户区就会四面内缩，看起来"最大化没铺满"。
///
/// 这里查 `SPI_GETWORKAREA` 后按实测偏移把外框向外扩，再 `SetWindowPos`。
///
/// 返回新的**客户区** `(x, y, w, h)`；失败返回 `None`。
#[tauri::command]
pub fn maximize_to_work_area(window: tauri::WebviewWindow) -> Option<(i32, i32, i32, i32)> {
    #[cfg(windows)]
    {
        if !crate::win_style::fill_work_area(&window) {
            return None;
        }
        crate::win_style::client_rect(&window)
    }
    #[cfg(not(windows))]
    {
        let _ = window;
        None
    }
}

/// 把**客户区**恢复到指定矩形（物理像素，屏幕坐标）。
#[tauri::command]
pub fn restore_window_bounds(
    window: tauri::WebviewWindow,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> Option<(i32, i32, i32, i32)> {
    #[cfg(windows)]
    {
        if !crate::win_style::restore_bounds(&window, x, y, width, height) {
            return None;
        }
        crate::win_style::client_rect(&window)
    }
    #[cfg(not(windows))]
    {
        let _ = (window, x, y, width, height);
        None
    }
}

/// 查询当前窗口**客户区**矩形（物理像素，屏幕坐标）。
///
/// 与 `maximize_to_work_area` 返回同一套度量，供前端记录还原态。
#[tauri::command]
pub fn window_rect_physical(window: tauri::WebviewWindow) -> Option<(i32, i32, i32, i32)> {
    #[cfg(windows)]
    {
        crate::win_style::client_rect(&window)
    }
    #[cfg(not(windows))]
    {
        let _ = window;
        None
    }
}

/// 启动时命令行传入的待打开文件（绝对路径）；没有则为 `null`。
///
/// 用途：`photovault.exe some\photo.cr3` 直接打开。也方便脚本化验证。
///
/// **白名单式返回**：只回传"存在且是文件"的路径，避免把五花八门的
/// 命令行参数（比如 Windows 传进来的开关）当成待打开文件。
#[tauri::command]
pub fn startup_file() -> Option<String> {
    let arg = std::env::args().nth(1)?;
    let p = std::path::PathBuf::from(&arg);
    if p.is_file() {
        // 归一化成绝对路径，前端拿到的路径可直接用于 probe/decode
        Some(
            p.canonicalize()
                .unwrap_or(p)
                .to_string_lossy()
                .into_owned(),
        )
    } else {
        None
    }
}

/// 列出支持的格式（供设置页展示）
///
/// **RAW 的 `enabled` 只标"已实测能打开"的格式**（2026-10-07 用真实样本
/// 端到端验证过提取内嵌预览，见 `crates/pv-decode/tests/e2e_decode.rs`）。
///
/// RAW 走的是通用扫描（不逐家适配容器），理论上任何含内嵌 JPEG 的 RAW
/// 都能打开；但没实测过的格式一律保持 `false` —— 让用户看到的是
/// "我们验证过的能力"，而不是"我们猜的"。
#[tauri::command]
pub fn supported_formats() -> Vec<FormatCapability> {
    // (id, 显示名, 优先级档位, 是否已启用)
    //
    // ⚠️ `enabled` 只标**真正接好并被测试覆盖**的格式。前端打开对话框的
    // 「所有支持的图片」过滤器必须与此保持一致 —— 否则会出现
    // 「列在"支持的图片"里、选中却报不支持」（HEIC 曾经就是这样：
    // 魔数探测早就有了，但解码器没接，用户选中后只看到报错）。
    let table: &[(&str, &str, &str, bool)] = &[
        ("jpeg", "JPEG", "P0", true),
        ("png", "PNG", "P0", true),
        // HEIC：heif-oxide（纯 Rust 容器解析 + rust_h265 解 HEVC）
        ("heic", "HEIF/HEIC", "P0", true),
        // PSD：手写解码器，读文件尾部的"合成图"（图层不支持）
        ("psd", "Photoshop PSD", "P1", true),
        // GIF：静态图 + 动图播放（2026-10-09 接入）
        ("gif", "GIF（含动图）", "P0", true),
        ("webp", "WebP（含有损/无损/动画）", "P0", true),
        // ── RAW：已实测（内嵌预览提取）──
        ("cr3", "Canon CR3", "P0", true),
        // DNG 2026-10-09 实测（TIFF 容器 + 内嵌 JPEG 预览，4032×3024）
        ("dng", "Adobe DNG", "P0", true),
        ("nrw", "Nikon NRW", "P0", true),
        ("raf", "Fujifilm RAF", "P1", true),
        ("rw2", "Panasonic RW2", "P1", true),
        ("orf", "Olympus ORF", "P1", true),
        ("sr2", "Sony SR2", "P2", true),
        ("srf", "Sony SRF", "P2", true),
        // ── RAW：未实测，暂不标可用 ──
        ("nef", "Nikon NEF", "P0", false),
        ("arw", "Sony ARW", "P0", false),
        // ── 其他位图格式 ──
        // TIFF：image-rs 官方 `tiff` crate（纯 Rust）。常规 TIFF 经本解码器；
        // 相机 RAW（NEF/ARW/DNG…）同为 TIFF 容器，由注册表路由到 RAW 管线。
        ("tiff", "TIFF", "P1", true),
        // AVIF：avif-decode → rav1d（纯 Rust，需 nasm）
        ("avif", "AVIF", "P1", true),
        // BMP：纯 Rust 手写解码（1/4/8/16/24/32 位 + 调色板 + RLE + 位域掩码）
        ("bmp", "BMP", "P1", true),
    ];
    table
        .iter()
        .map(|(id, label, tier, enabled)| FormatCapability {
            id: (*id).to_string(),
            label: (*label).to_string(),
            tier: (*tier).to_string(),
            enabled: *enabled,
        })
        .collect()
}

/// 查询「打开方式」列表里已注册了哪些扩展名。
///
/// `candidates` 由前端传（就是它能打开的那些扩展名）——把清单放在前端，
/// Rust 侧不重复维护一份，避免两边漏同步。
#[tauri::command]
pub fn association_state(candidates: Vec<String>) -> AssociationStateDto {
    AssociationStateDto {
        registered: candidates
            .iter()
            .filter(|e| win_assoc::is_registered(e))
            .filter_map(|e| win_assoc::normalize_ext(e))
            .collect(),
        prog_id_prefix: win_assoc::PROG_ID_PREFIX.to_string(),
        note: win_assoc::NOTE.to_string(),
    }
}

/// 用系统默认浏览器打开一个链接（信息面板的「在地图中查看」用）。
///
/// ## 为什么只放行 http / https
/// 这个命令的参数最终交给 `ShellExecuteW`，而 ShellExecute 对
/// `file:`、`\\server\share`、`xxx.exe` 一律照常执行 —— 不加白名单就等于
/// 给 WebView 侧一个「打开/运行任意本地目标」的口子。前端只用它开地图网页，
/// 所以这里硬拦在后端（前端的校验可以被绕过，后端的不行）。
#[tauri::command]
pub fn open_external_url(url: String) -> Result<(), ErrorDto> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(ErrorDto {
            code: "bad_url".to_string(),
            message: "只允许打开 http/https 链接".to_string(),
            user_visible: true,
        });
    }
    crate::win_shell::open_url(&url).map_err(|message| ErrorDto {
        code: "open_failed".to_string(),
        message,
        user_visible: true,
    })
}

/// 前端诊断日志出口。
///
/// **为什么需要**：WebView2 里的 `console.log` 在这类发布版里看不到
/// （没挂 devtools），而「解码到底花了多久」「走的是哪条 IPC 通道」
/// 这类问题必须从 WebView2 侧取数。前端调它把时间点打到 Rust 的
/// tracing（默认 stderr，`PHOTOVAULT_LOG=info` 可见）。
///
/// 只在诊断时由前端调用，不参与业务逻辑；失败也不影响任何流程。
#[tauri::command]
pub fn trace_log(msg: String) {
    tracing::info!(target: "ui", "{msg}");
}

/// 把 `ext` 加入 / 移出「打开方式」列表。
///
/// **不是「设为默认」**：Windows 8 起的 UserChoice 哈希保护不允许程序
/// 静默改默认程序，详见 `win_assoc` 模块文档。这里做的是合法且不会被
/// 系统回滚的那一半 —— 用户之后可在右键「打开方式」里自己勾选「始终使用」。
///
/// 只写 `HKCU`，不需要管理员权限，也不影响同机其他用户。
#[tauri::command]
pub fn set_file_association(
    ext: String,
    display_name: String,
    enable: bool,
) -> Result<(), ErrorDto> {
    let r = if enable {
        win_assoc::register(&ext, &display_name)
    } else {
        win_assoc::unregister(&ext)
    };
    r.map_err(assoc_err)
}

/// 批量设置文件关联（帮助面板的「全选」用）。
///
/// **为什么要做成一个命令而不是让前端循环调单个命令**：
/// 1. 每次 `register`/`unregister` 都会广播一次 `SHCNE_ASSOCCHANGED`，
///    全选 7 个扩展名就是 7 次；这里循环走**静默版**，结束后只通知一次。
/// 2. 部分失败时能一次说清是哪些，前端不用自己聚合 7 次调用的结果。
///
/// 返回值是**失败的扩展名列表**（空 = 全部成功）。刻意不用 `Err` 表示部分失败 ——
/// 那样前端会丢掉「成功了几个」的信息，而这是用户最关心的。
///
/// 单个扩展名写失败不影响其余继续执行（例如某个扩展名被其他软件用
/// `KEY_ALL_ACCESS` 锁住）。
#[tauri::command]
pub fn set_file_association_bulk(
    items: Vec<AssocItemDto>,
    enable: bool,
) -> Result<Vec<String>, ErrorDto> {
    let mut failed = Vec::new();

    for item in &items {
        let r = if enable {
            win_assoc::register_silent(&item.ext, &item.display_name)
        } else {
            win_assoc::unregister_silent(&item.ext)
        };
        if let Err(e) = r {
            tracing::warn!(target: "photovault::assoc", ext = %item.ext, "批量关联失败: {e}");
            failed.push(item.ext.clone());
        }
    }

    // 只在真的动过注册表时才通知 shell
    if failed.len() < items.len() {
        win_assoc::notify_shell();
    }

    // 全失败且确有请求 → 当作整体失败报出来，否则用户会以为「设置成功了」
    if !items.is_empty() && failed.len() == items.len() {
        return Err(assoc_err(format!(
            "全部 {} 个格式都设置失败，请确认当前用户对注册表 HKCU\\Software\\Classes 有写权限",
            items.len()
        )));
    }

    Ok(failed)
}
