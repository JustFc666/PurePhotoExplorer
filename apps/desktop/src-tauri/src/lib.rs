//! PurePhotoExplorer 桌面应用入口。
//!
//! 业务逻辑全部在 workspace 的 pv-* crates，本 crate 只做三件事：
//! 1. 启动 Tauri 应用
//! 2. 注册 IPC 命令
//! 3. 维护全局状态（解码器注册表）

// 测试函数名用中文（可读性优先），关闭 snake_case 检查
#![cfg_attr(test, allow(non_snake_case))]

mod commands;
mod state;
// 无条件声明：模块内部按平台分了 imp（非 Windows 是空实现），
// 这样 `normalize_ext` 这类纯逻辑在任何平台都能编译与单测。
mod win_assoc;
#[cfg(windows)]
mod win_style;
// 打印：内部按平台分了实现（非 Windows 返回明确错误）
mod win_print;
// 打开外部链接（默认浏览器）：同样按平台分实现
mod win_shell;

use state::AppState;

/// 像素传输协议的公开接口。
///
/// 抽成 pub module 是为了让集成测试（`tests/`）能验证协议自洽性。
/// 协议本身在前后端各有一份实现（Rust / TypeScript），
/// 两者必须严格对应，否则前端会显示错位或全黑图。
///
/// 静图是 PVI1（单帧），动图是 PVA1（多帧，见 `commands/pixel.rs`）。
pub mod ipc_protocol {
    pub use crate::commands::{
        decode_animation_blob, decode_pixels, pack_animation, pack_pixels, supported_formats,
        AnimHeader, AnimPack, ErrorDto, FormatCapability, ImageInfoDto, ANIM_HEADER_FIXED,
        ANIM_MAGIC, ANIM_VERSION, PIXEL_HEADER_LEN, PIXEL_MAGIC,
    };
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 日志：默认写 stderr，避免污染用户目录
    // 详见开发文档 §9.3，日志默认脱敏（只记文件名不记完整路径）
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("PHOTOVAULT_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::probe_image,
            commands::decode_image,
            commands::decode_animation,
            commands::save_edited_image,
            commands::list_siblings,
            commands::app_info,
            commands::startup_file,
            commands::supported_formats,
            commands::trace_log,
            commands::file_version,
            commands::print_image,
            commands::list_printers,
            commands::association_state,
            commands::set_file_association,
            commands::set_file_association_bulk,
            commands::maximize_to_work_area,
            commands::restore_window_bounds,
            commands::window_rect_physical,
            commands::open_external_url,
        ])
        .run(tauri::generate_context!())
        .expect("启动 PurePhotoExplorer 失败");
}
