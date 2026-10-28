//! PurePhotoExplorer 桌面应用入口。
//!
//! 业务逻辑全部在 workspace 的 pv-* crates，本 crate 只做三件事：
//! 1. 启动 Tauri 应用
//! 2. 注册 IPC 命令
//! 3. 维护全局状态（解码器注册表）

// 测试函数名用中文（可读性优先），关闭 snake_case 检查
#![cfg_attr(test, allow(non_snake_case))]

mod commands;
// 文件浏览器（侧栏「文件夹」的「我的电脑」式导航）：内部按平台分实现
mod fs_browse;
mod state;
// 无条件声明：模块内部按平台分了 imp（非 Windows 是空实现），
// 这样 `normalize_ext` 这类纯逻辑在任何平台都能编译与单测。
mod win_assoc;
#[cfg(windows)]
mod win_style;
// 打印：内部按平台分了实现（非 Windows 返回明确错误）
mod win_print;
// 删除到**回收站**（SHFileOperationW + FOF_ALLOWUNDO）。Windows-only。
// ⛔ 不要换成 std::fs::remove_file —— 那是永久删除，照片管理里不可接受。
#[cfg(windows)]
mod win_trash;
// 打开外部链接（默认浏览器）：同样按平台分实现
mod win_shell;
// 侧栏文件树的**系统图标**（盘符/文件夹/此电脑）—— 就是资源管理器那套图标。
// Windows-only：内部直接用 SHGetFileInfoW + GDI。
#[cfg(windows)]
mod shell_icon;

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
            commands::manager_open,
            commands::app_data_dir,
            commands::manager_scan,
            commands::manager_query,
            commands::manager_count,
            commands::manager_thumbnail,
            commands::manager_folders,
            commands::manager_open_folder,
            commands::manager_remove_folder,
            commands::manager_add_to_library,
            commands::manager_libraries,
            commands::manager_library_members,
            commands::manager_stats,
            commands::manager_export_images,
            commands::reveal_in_explorer,
            commands::manager_set_rating,
            commands::manager_set_flag,
            commands::manager_convert_images,
            commands::manager_auto_orient,
            commands::manager_print_images,
            commands::manager_delete_images,
            commands::manager_rename_images,
            commands::manager_save_as,
            commands::manager_transform_images,
            commands::manager_copy_to_folder,
            commands::manager_move_to_folder,
            commands::manager_create_library,
            commands::manager_tags,
            commands::manager_tag_groups,
            commands::manager_add_tags,
            commands::manager_remove_tag,
            commands::manager_tag_counts,
            commands::manager_rename_tag,
            commands::manager_delete_tag,
            commands::manager_places,
            commands::manager_people,
            commands::manager_person_faces,
            commands::manager_rename_person,
            commands::manager_set_person_cover,
            commands::manager_merge_persons,
            commands::manager_split_faces,
            commands::manager_delete_person,
            commands::manager_faces_stats,
            commands::manager_faces_clear,
            commands::manager_cameras,
            commands::manager_years,
            commands::manager_image_tags,
            commands::ai_config,
            commands::ai_save_config,
            commands::ai_vocab,
            commands::ai_vocab_toggle,
            commands::ai_vocab_export,
            commands::ai_vocab_import,
            commands::manager_create_smart_library,
            commands::manager_cache_stats,
            commands::manager_clear_cache,
            commands::manager_rename_library,
            commands::manager_delete_library,
            commands::fs_roots,
            commands::fs_list_dirs,
            commands::fs_resolve_dir,
            commands::fs_list_images,
            commands::fs_probe_dimensions,
            commands::fs_probe_times,
            commands::fs_tree_icons,
            commands::fs_thumbnail,
        ])
        .run(tauri::generate_context!())
        .expect("启动 PurePhotoExplorer 失败");
}
