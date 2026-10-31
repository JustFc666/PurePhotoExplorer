//! 批量/单个改名。
//!
//! 两种目标：已入库的行（改索引 + 改文件）与浏览模式下的裸文件（只改文件，
//! 走 [`rename_plain`]）。⛔ 绝不覆盖已有文件。

use std::path::Path;

use tauri::State;

use super::{merr, BatchFailureDto, BatchSummaryDto};
use crate::commands::dto::ErrorDto;
use crate::state::AppState;

/// 纯文件重命名（**不碰索引**）。浏览模式（未入库）走这里。
///
/// 校验规则与 `Store::rename_image` 保持一致：非法字符 / 目标已存在都要拒绝。
/// ⛔ 绝不覆盖 —— 覆盖用户文件比让改名失败严重得多。
fn rename_plain(path: Option<&str>, new_name: &str) -> std::result::Result<(), String> {
    let Some(path) = path else {
        return Err("既没有索引记录也没有本地路径".into());
    };
    if new_name.trim().is_empty() {
        return Err("文件名不能为空".into());
    }
    if new_name.chars().any(|c| r#"\/:*?"<>|"#.contains(c)) {
        return Err("文件名里有非法字符".into());
    }
    let old = Path::new(path);
    if !old.is_file() {
        return Err("文件不存在（卷可能未挂载）".into());
    }
    let Some(parent) = old.parent() else {
        return Err("路径没有父目录".into());
    };
    let new = parent.join(new_name);
    if new.exists() {
        return Err("目标文件名已存在".into());
    }
    std::fs::rename(old, &new).map_err(|e| match e.kind() {
        std::io::ErrorKind::PermissionDenied => "没有权限".into(),
        std::io::ErrorKind::NotFound => "文件不存在".into(),
        _ => e.to_string(),
    })
}

/// 重命名请求里的一项
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameItemDto {
    /// 入库行的索引 id；浏览模式（未入库）为 `None`
    pub id: Option<i64>,
    /// 当前绝对路径（`id == None` 时用它做纯文件重命名）
    pub path: Option<String>,
    /// 目标文件名（**含扩展名**，由前端按模板算好）
    pub new_name: String,
}

/// 批量重命名：改文件名 + 同步索引 + 迁移缩略图缓存。
///
/// ⚠️ 新名字由**前端**按模板算好（`{name}` / `{n}` 的展开在前端更容易做），
/// 后端只负责执行与校验（非法字符、目标已存在）。
///
/// ⚠️ 逐张执行、逐张报告：一张重名失败不该让整批停住。
/// 但因为**绝不覆盖**，「A→B 且 B→C」这种链式改名会有一部分失败 ——
/// 这是有意的：覆盖用户文件比让几个改名失败严重得多。
#[tauri::command]
pub async fn manager_rename_images(
    state: State<'_, AppState>,
    items: Vec<RenameItemDto>,
) -> Result<BatchSummaryDto, ErrorDto> {
    if items.is_empty() {
        return Err(merr("没有选中任何图片"));
    }
    let mut out = BatchSummaryDto::new();
    for it in &items {
        let res: std::result::Result<(), String> = match it.id {
            // 入库行：走 Store，改名同时同步索引 + 清旧缓存
            Some(id) => state
                .with_manager_mut(|s| s.rename_image(id, &it.new_name).map_err(|e| e.to_string()))
                .map(|_| ()),
            // 浏览模式：没有索引行，纯文件改名。
            // ⚠️ 同样**绝不覆盖**：目标存在就报错，跟入库那条保持一致。
            None => rename_plain(it.path.as_deref(), &it.new_name),
        };
        match res {
            Ok(()) => out.done += 1,
            Err(e) => out.failed.push(BatchFailureDto {
                name: it.new_name.clone(),
                reason: e,
            }),
        }
    }
    Ok(out)
}
