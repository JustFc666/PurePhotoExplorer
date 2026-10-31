//! 磁盘缩略图缓存的占用与清理。
//!
//! ⛔ 只碰 `thumbs/` 目录：**原图和索引都不动**。代价是下次浏览要重新生成，
//! 所以这是用户主动点的操作，不做自动清理。

use tauri::State;

use crate::commands::dto::ErrorDto;
use super::{merr, CacheStatsDto};
use crate::state::AppState;

/// 读缓存占用。
///
/// 只遍历 `thumbs/` 目录，**不查数据库** —— 一万个缩略图也就是几毫秒。
#[tauri::command]
pub async fn manager_cache_stats(
    state: State<'_, AppState>,
) -> Result<CacheStatsDto, ErrorDto> {
    state
        .with_manager(|s| {
            let (bytes, files) = s.cache_usage();
            Ok(CacheStatsDto { bytes, files })
        })
        .map_err(merr)
}

/// 清空磁盘缩略图缓存，返回清理后的占用（应为 0）。
///
/// ⛔ 只删 `thumbs/` 下的缓存文件：**原图和索引都不动**。
/// 代价是下次浏览要重新生成缩略图（首屏会慢一点），所以这是用户主动点的操作，
/// 不做自动清理。
#[tauri::command]
pub async fn manager_clear_cache(state: State<'_, AppState>) -> Result<CacheStatsDto, ErrorDto> {
    state
        .with_manager(|s| {
            s.clear_cache();
            let (bytes, files) = s.cache_usage();
            Ok(CacheStatsDto { bytes, files })
        })
        .map_err(merr)
}
