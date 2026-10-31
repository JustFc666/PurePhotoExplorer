//! 图库模式的**批量写文件**命令。
//!
//! | 子模块 | 职责 | 碰不碰原图 |
//! |---|---|---|
//! | [`convert`] | 转格式、按 EXIF 摆正 | 否：写新文件（可选写回） |
//! | [`saveas`] | 另存为（目录 + 格式 + 命名模板） | 否 |
//! | [`transfer`] | 复制到文件夹、移动到文件夹 | 复制不碰；移动**会**搬走 |
//! | [`delete`] | 删除到回收站 | 是：进回收站 |
//! | [`rename`] | 批量/单个改名 | 是：只改文件名 |
//! | [`transform`] | 批量旋转/翻转 | 是：原地重写像素 |
//! | [`printout`] | 逐张直接送印 | 否 |
//!
//! ## 与单图命令的关系
//! 单图的 `save_edited_image` / `print_image` 已经包含了完整管线（解码 → 几何变换 →
//! 编码 → 搬运 EXIF）。这里**不重复实现**，只做「循环 + 重名处理 + 逐张失败隔离」，
//! 直接复用 `edit::save_impl` / `print::print_impl` —— 否则批量导出的图和单张另存为的图
//! 会走出两套像素结果，这是最难查的一类不一致。
//!
//! ## 为什么一律 `async` + `spawn_blocking`
//! Tauri 里**非 async 命令跑在主线程**上。批量转格式要把每张原图整幅解码（Export 档，
//! 不是 2048 预览），几十张就能让窗口「未响应」几分钟。`spawn_blocking` 把它挪到
//! 阻塞线程池，UI 线程只管画界面。
//!
//! ## 失败隔离
//! 一张坏了（截断、卷掉线、格式不支持）只进 `failed`，后面的继续做 ——
//! 批量操作里「做了一半就整体回滚」比「尽量多救几张」更让人难受。
//!
//! ## ⚠️ 拆模块时最容易踩的坑
//! `#[tauri::command]` 除了函数本体，还在**定义它的模块里**生成 `__cmd__<名字>`；
//! `generate_handler!` 按「模块路径::`__cmd__`名字」查找。所以下面只能
//! `pub use 子模块::*` 整体重导出，**逐个列名字会报 `cannot find __cmd__xxx`**。
//!
//! 另外：子模块里的 `super::` 指向的是 `batch` 而不是 `commands`，
//! 引用兄弟命令模块必须写全 `crate::commands::xxx`。
//! （`print` 子模块因此叫 `printout`，免得与 `commands::print` 撞名。）

use std::path::Path;

use super::dto::ErrorDto;

/// 单条失败
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchFailureDto {
    pub name: String,
    pub reason: String,
}

/// 批量操作结果（转格式 / 摆正 / 打印共用）
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchSummaryDto {
    /// 成功处理的张数
    pub done: usize,
    /// 按规则跳过（无需处理 / 目标即源目录）
    pub skipped: usize,
    pub failed: Vec<BatchFailureDto>,
    /// 输出目录（写回原文件时为 `None`）
    pub dest_dir: Option<String>,
}

impl BatchSummaryDto {
    fn new() -> Self {
        Self {
            done: 0,
            skipped: 0,
            failed: Vec::new(),
            dest_dir: None,
        }
    }

    fn fail(&mut self, path: &Path, reason: impl Into<String>) {
        self.failed.push(BatchFailureDto {
            name: file_name_of(path),
            reason: reason.into(),
        });
    }
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// 把 IO 错误压成人话。
fn io_reason(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => "文件不存在（卷可能未挂载）".into(),
        std::io::ErrorKind::PermissionDenied => "没有权限".into(),
        _ => e.to_string(),
    }
}

/// 目标格式白名单（与 `pv_decode::format_from_extension` / 前端 `EXPORT_FORMATS` 一致）。
fn parse_target_format(ext: &str) -> Option<&'static str> {
    match ext.trim_start_matches('.').to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => Some("jpg"),
        "png" => Some("png"),
        "bmp" => Some("bmp"),
        _ => None,
    }
}

fn merr(msg: impl Into<String>) -> ErrorDto {
    ErrorDto {
        code: "managerError".into(),
        message: msg.into(),
        user_visible: true,
    }
}

mod convert;
mod delete;
mod printout;
mod rename;
mod saveas;
mod transfer;
mod transform;

pub use convert::*;
pub use delete::*;
pub use printout::*;
pub use rename::*;
pub use saveas::*;
pub use transfer::*;
pub use transform::*;
