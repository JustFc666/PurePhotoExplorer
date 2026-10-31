//! 图库的增删改与成员目录维护。
//!
//! ⛔ **删图库 ≠ 删照片**：`manager_delete_library` 只清索引行与缩略图缓存，
//! 磁盘上的原图一个字节都不动（还被别的图库引用的目录更是整棵留下）。
//! ⛔ **移除目录同理**，只删索引 + 缓存。

use tauri::State;

use crate::commands::dto::ErrorDto;
use super::{merr, LibraryDto, LibraryMemberDto, RemovedDto};
use crate::state::AppState;

/// 侧栏「图库」列表（含各自条目数）。
#[tauri::command]
pub async fn manager_libraries(state: State<'_, AppState>) -> Result<Vec<LibraryDto>, ErrorDto> {
    state
        .with_manager(|s| {
            s.libraries()
                .map(|v| {
                    v.into_iter()
                        .map(|l| LibraryDto {
                            id: l.id,
                            name: l.name,
                            kind: l.kind,
                            rule: l.rule,
                            count: l.count,
                        })
                        .collect()
                })
                .map_err(|e| e.to_string())
        })
        .map_err(merr)
}

/// 图库 ↔ 目录的归属关系（扁平表：一行一个「图库包含某目录」）。
///
/// 前端用它算「这个目录**直接**属于哪几个图库」；子树归属不在这里展开。
#[tauri::command]
pub async fn manager_library_members(
    state: State<'_, AppState>,
) -> Result<Vec<LibraryMemberDto>, ErrorDto> {
    state
        .with_manager(|s| {
            s.library_members()
                .map(|v| {
                    v.into_iter()
                        .map(|(library_id, folder_id)| LibraryMemberDto {
                            library_id,
                            folder_id,
                        })
                        .collect()
                })
                .map_err(|e| e.to_string())
        })
        .map_err(merr)
}

/// 从图库移除一个目录**及其整棵子树**（删索引 + 删对应缩略图缓存）。
///
/// ⚠️ **只动索引与缓存，绝不碰用户的原始图片文件** —— 这条由
/// `pv-store` 的 `移除目录只删索引与缓存不碰原图` 测试把守。
///
/// 幂等：目录已经不在库里时返回全 0，不报错（重复点「移除」不该弹错）。
#[tauri::command]
pub async fn manager_remove_folder(
    state: State<'_, AppState>,
    folder_id: i64,
) -> Result<RemovedDto, ErrorDto> {
    state
        .with_manager_mut(|s| {
            let r = s.remove_folder(folder_id).map_err(|e| e.to_string())?;
            Ok(RemovedDto {
                folders: r.folders,
                images: r.images,
                thumbs: r.thumbs,
                thumbs_orphan: r.thumbs_orphan,
            })
        })
        .map_err(merr)
}

/// 把一个**已入库**的目录归入图库，返回图库 id。
///
/// - `libraryId = null` → 按 `name` **同名复用**：已有同名图库就挂上去，没有才新建
/// - `libraryId = 某个 id` → 挂到该图库
///
/// ⚠️ 调用前必须先用 `manager_open_folder` 把目录扫进索引 —— 目录不在
/// `folders` 表里就挂不上（`library_folders.folder_id` 指着它）。
///
/// `recursive = true` → 连同**全部后代目录**一起挂。⛔ 少了这一步，子目录里的图
/// 在图库查询里一张都出不来（查询按 `library_folders` 的 id 精确匹配）。
#[tauri::command]
pub async fn manager_add_to_library(
    state: State<'_, AppState>,
    folder_id: i64,
    name: String,
    library_id: Option<i64>,
    recursive: Option<bool>,
) -> Result<i64, ErrorDto> {
    state
        .with_manager_mut(|s| {
            s.add_to_library(folder_id, &name, library_id, recursive.unwrap_or(false))
                .map_err(|e| e.to_string())
        })
        .map_err(merr)
}

/// 新建一个**空图库**，返回新库 id。
///
/// 侧栏「图库」标题右侧的 `+` 用它。建完库之后前端会立刻打开命名对话框
/// 之外的事情都不做 —— 空库没有成员目录，也就没有条目，用户接着可以
/// 往里面加目录（右键目录 →「加进图库」）。
///
/// ⚠️ 重名由 `Store::create_library` 挡（`libraries.name` 没有 UNIQUE 约束）。
#[tauri::command]
pub async fn manager_create_library(
    state: State<'_, AppState>,
    name: String,
) -> Result<i64, ErrorDto> {
    state
        .with_manager_mut(|s| s.create_library(&name).map_err(|e| e.to_string()))
        .map_err(|e| ErrorDto {
            code: "managerError".into(),
            message: e,
            user_visible: true,
        })
}

/// 新建一个**智能图库**：规则文本（`pv-lib` DSL）存进 `libraries.rule`。
///
/// 侧栏「图库 +」右键 →「智能图库」预设走这里。规则语法错会直接报错
/// （建出来一个永远空的图库，用户根本看不出哪儿错）。
///
/// 规则写法：`date:2024` / `date:2024-05` / `rating:>=4` / `type:raw` /
/// `ext:jpg` / `tag:风景` / 裸词（全文关键词），空格 = AND。
#[tauri::command]
pub async fn manager_create_smart_library(
    state: State<'_, AppState>,
    name: String,
    rule: String,
) -> Result<i64, ErrorDto> {
    state
        .with_manager_mut(|s| {
            s.create_smart_library(&name, &rule).map_err(|e| e.to_string())
        })
        .map_err(|e| ErrorDto {
            code: "managerError".into(),
            message: e,
            user_visible: true,
        })
}

/// 改图库名（侧栏图库行右键「重命名」）。只改 `libraries.name`，不动任何文件。
#[tauri::command]
pub async fn manager_rename_library(
    state: State<'_, AppState>,
    id: i64,
    name: String,
) -> Result<(), ErrorDto> {
    state
        .with_manager_mut(|s| s.rename_library(id, &name).map_err(|e| e.to_string()))
        .map_err(|e| ErrorDto {
            code: "managerError".into(),
            message: e,
            user_visible: true,
        })
}

/// 删除图库（侧栏图库行右键「删除图库」）。
///
/// ⭐ 2026-10-31 改：**连它独占的目录一起从索引里清掉**，这些照片会从
/// 「全部照片」里消失（用户反馈「图库删了，全部照片也要清」）。
///
/// 但两种东西**绝不动**：
/// - 还被**别的图库**引用的目录（同一个目录挂两个库时，删一个不影响另一个）
/// - 磁盘上的**原图**（只删索引行 + 缩略图缓存）
///
/// ⚠️ 删索引会连带丢掉这些图的评分、标签、人脸关联 —— UI 侧必须先确认。
#[tauri::command]
pub async fn manager_delete_library(
    state: State<'_, AppState>,
    id: i64,
) -> Result<Option<RemovedDto>, ErrorDto> {
    state
        .with_manager_mut(|s| s.delete_library(id).map_err(|e| e.to_string()))
        .map(|o| {
            o.map(|r| RemovedDto {
                folders: r.folders,
                images: r.images,
                thumbs: r.thumbs,
                thumbs_orphan: r.thumbs_orphan,
            })
        })
        .map_err(|e| ErrorDto {
            code: "managerError".into(),
            message: e,
            user_visible: true,
        })
}
