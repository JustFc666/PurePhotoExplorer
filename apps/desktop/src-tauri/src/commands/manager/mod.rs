//! 管理器（S2 图库）IPC 命令。
//!
//! 薄壳：把 [`pv_store::Store`] 的能力暴露给前端，**业务逻辑全在 `pv-store`**。
//! 所以这里每个命令都短得只剩一次 `with_manager(...)` + DTO 映射 ——
//! 看到几百行的命令就说明逻辑放错了层。
//!
//! ## 这个目录里有什么
//! | 文件 | 职责 |
//! |---|---|
//! | [`dto`] | 前端拿到的数据结构（IPC 契约）|
//! | [`browse`] | 打开数据目录、扫描入库、组合查询 —— 「有什么图」|
//! | [`library`] | 图库增删改 + 成员目录 —— 「归到哪儿」|
//! | [`media`] | 缩略图、导出、定位、评分/旗标 —— 「对图做什么」|
//! | [`meta`] | 地点、标签、相机、年份 —— 「按什么筛」|
//! | [`cache`] | 缩略图缓存的占用与清理 |
//!
//! 本文件只放**所有子模块共享的三样东西**：错误映射（[`merr`] /
//! `impl From<StoreError>`）、数据目录位置、查询参数解析（档位 / 排序键）。
//!
//! ## 为什么这么拆（2026-11）
//! 原来是一个 1192 行的 `manager.rs`：四十个命令 + 二十个 DTO 混在一起，
//! 想找「导出为什么跳过这张」要滚过标签 CRUD。按上面的六格切开后，
//! 每个文件只回答一个问题；命令的注册路径完全不变。
//!
//! ⛔⛔ **下面的 `pub use` 必须用通配**：`#[tauri::command]` 除了函数本体，
//! 还会在定义它的模块里生成 `__cmd__<name>` 隐藏项，`generate_handler!` 按
//! 「父模块::__cmd__名字」查找。逐项列名只搬函数、不搬隐藏项 → 编译期报
//! `cannot find __cmd__manager_query`（`commands/ai` 拆分层时踩过同一个坑）。


use std::path::PathBuf;

use pv_store::{SortKey, StoreError, Tier};

use crate::commands::dto::ErrorDto;

/// 把服务层错误映射成前端错误。
impl From<StoreError> for ErrorDto {
    fn from(e: StoreError) -> Self {
        Self {
            code: "managerError".into(),
            message: e.to_string(),
            user_visible: true,
        }
    }
}

pub(crate) fn merr(msg: impl Into<String>) -> ErrorDto {
    ErrorDto {
        code: "managerError".into(),
        message: msg.into(),
        user_visible: true,
    }
}

/// 默认数据目录：`%LOCALAPPDATA%\PurePhotoExplorer`。
///
/// `pub(super)`：只有兄弟模块 [`browse`] 用它（`manager_open` 的缺省值）。
pub(super) fn default_data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("PurePhotoExplorer")
}

pub(crate) fn parse_tier(s: &str) -> Option<Tier> {
    match s {
        "sm" => Some(Tier::Small),
        "md" => Some(Tier::Medium),
        "lg" => Some(Tier::Large),
        "xl" => Some(Tier::Xlarge),
        _ => None,
    }
}

/// 排序键字符串 → [`SortKey`]。未知/缺省一律「拍摄时间」。
///
/// `pub(super)`：入参 DTO 在 [`dto`] 里，它要把字符串翻成 `SortKey`。
pub(super) fn parse_sort(s: Option<&str>) -> SortKey {
    match s {
        Some("name") => SortKey::RelName,
        Some("size") => SortKey::Size,
        Some("mtime") => SortKey::Mtime,
        Some("format") => SortKey::Format,
        Some("rating") => SortKey::Rating,
        Some("dims") => SortKey::Dims,
        Some("flag") => SortKey::Flag,
        Some("added") => SortKey::Added,
        Some("random") => SortKey::Random,
        _ => SortKey::TakenAt,
    }
}

mod browse;
mod cache;
mod dto;
mod library;
mod media;
mod meta;

pub use browse::*;
pub use cache::*;
pub use dto::*;
pub use library::*;
pub use media::*;
pub use meta::*;
