//! 批量删除：**进回收站**，再清索引与缩略图缓存。
//!
//! 顺序是铁律：先删文件 → 确认不在了 → 才删索引。
//! 反过来一旦删索引后文件删失败，图就从列表里「凭空消失」但还占着磁盘。

use tauri::State;

use super::{file_name_of, merr, BatchFailureDto};
use crate::commands::dto::ErrorDto;
use crate::state::AppState;

/// 删除请求里的一个目标（id + 绝对路径）。
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteItemDto {
    /// 浏览模式（未入库）为 `None` —— 只删文件，没有索引行要清
    pub id: Option<i64>,
    /// 卷未挂载时为 `None` —— 这种只能删索引，删不了文件
    pub path: Option<String>,
}

/// 批量删除的结果。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteSummaryDto {
    /// 已移入回收站的文件数
    pub deleted_files: usize,
    /// 已从索引移除的行数
    pub removed_index: usize,
    /// 因为「卷未挂载」只能清索引、没删到文件的条目数
    pub offline_index_only: usize,
    pub failed: Vec<BatchFailureDto>,
}

/// 批量删除：**移入回收站**（可撤销），再清索引与缩略图缓存。
///
/// ## 顺序很重要
/// 1. **先删文件**：删不掉（占用中、权限不足）就**不动索引** ——
///    否则列表里照片消失了、磁盘上还留着，用户再也找不到它。
/// 2. 删成功（或文件本来就不存在）的，才 `remove_images` 清索引 + 缓存。
/// 3. 删不掉的把原因逐条带回，前端显示。
///
/// ## 为什么回收站而不是直接删
/// 照片管理里「删除」默认必须是可撤销的。`std::fs::remove_file` 直接抹掉，
/// 没有任何挽回余地 —— 这里走 `SHFileOperationW` + `FOF_ALLOWUNDO`。
#[tauri::command]
pub async fn manager_delete_images(
    state: State<'_, AppState>,
    items: Vec<DeleteItemDto>,
) -> Result<DeleteSummaryDto, ErrorDto> {
    if items.is_empty() {
        return Err(merr("没有选中任何图片"));
    }

    let mut out = DeleteSummaryDto {
        deleted_files: 0,
        removed_index: 0,
        offline_index_only: 0,
        failed: Vec::new(),
    };

    #[cfg(windows)]
    {
        use std::path::PathBuf;

        // 1) 有路径的先送去回收站
        let to_trash: Vec<PathBuf> = items
            .iter()
            .filter_map(|i| i.path.as_ref())
            .map(PathBuf::from)
            .collect();
        if !to_trash.is_empty() {
            if let Err(e) = crate::win_trash::move_to_recycle_bin(&to_trash) {
                // Shell 整批失败：索引一个都不动，让用户处理完再来
                return Err(merr(format!("移入回收站失败：{e}")));
            }
        }

        // 2) 只有「文件确实不在了」的才删索引
        let mut ids_to_remove: Vec<i64> = Vec::new();
        for it in &items {
            match it.path.as_deref() {
                Some(p) => {
                    if std::path::Path::new(p).exists() {
                        // Shell 说成功但文件还在（罕见：被别的进程重建 / 权限问题）
                        out.failed.push(BatchFailureDto {
                            name: file_name_of(std::path::Path::new(p)),
                            reason: "文件仍在磁盘上（可能被占用）".into(),
                        });
                    } else {
                        out.deleted_files += 1;
                        // 浏览模式的项没有索引行，只有入库的才需要清
                        if let Some(id) = it.id {
                            ids_to_remove.push(id);
                        }
                    }
                }
                None => {
                    match it.id {
                        // 卷未挂载：删不了文件，但仍然清掉失效索引
                        Some(id) => {
                            out.offline_index_only += 1;
                            ids_to_remove.push(id);
                        }
                        // 既没路径也没索引：无从下手
                        None => out.failed.push(BatchFailureDto {
                            name: "(无路径)".into(),
                            reason: "既没有本地路径也没有索引记录".into(),
                        }),
                    }
                }
            }
        }

        // 3) 清索引 + 缓存
        if !ids_to_remove.is_empty() {
            let n = state
                .with_manager_mut(|s| s.remove_images(&ids_to_remove).map_err(|e| e.to_string()))
                .map_err(merr)?
                .images;
            out.removed_index = n as usize;
        }
        Ok(out)
    }
    #[cfg(not(windows))]
    {
        let _ = (state, items);
        Err(merr("当前平台不支持删除到回收站"))
    }
}
