//! 侧栏的元数据：地点、标签、相机、年份 —— 「按什么筛」这条线。
//!
//! ⭐ 全部**只读索引**（`tags` / `image_places` / `image_exif`），不读图片文件、
//! 不碰缓存。唯一的例外是改名/删标签，那也只改数据库。
//!
//! ⛔ 带 `libraryId` / `dirId` 的计数必须把 scope 一路传下去，否则数字会停在
//! 全库（用户看到「人像 52」点进去却只有 3 张）。

use tauri::State;

use crate::commands::dto::ErrorDto;
use super::{
    merr, CameraDto, ImageTagDto, ImageTagsDto, PlaceDto, TagDto, TagGroupDto, TagHitDto, YearDto,
};
use crate::state::AppState;

/// 侧栏「地点」分区：全部解析出来的城市 + 各自张数（多的在前）。
///
/// ⚠️ 必须在查询列表**之前**调一次：地点是启动回填写进 `image_places` 的，
/// 第一次启动时这一轮还没跑完，列表会是空的。
#[tauri::command]
pub async fn manager_places(
    state: State<'_, AppState>,
    library_id: Option<i64>,
    dir_id: Option<i64>,
) -> Result<Vec<PlaceDto>, ErrorDto> {
    state
        .with_manager(|s| {
            Ok(s.places_scoped(library_id, dir_id)
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|p| PlaceDto {
                    name: p.name,
                    count: p.count,
                })
                .collect::<Vec<_>>())
        })
        .map_err(merr)
}

/// 单张图的标签及置信度（按置信度降序，没分数的排后面）。
#[tauri::command]
pub async fn manager_image_tags(
    state: State<'_, AppState>,
    image_id: i64,
) -> Result<ImageTagsDto, ErrorDto> {
    state
        .with_manager(|s| {
            let mut list: Vec<ImageTagDto> = s
                .image_tags(image_id)
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|(name, source, confidence)| ImageTagDto {
                    name,
                    source,
                    confidence,
                })
                .collect();
            // EXIF 标签没有分数，别让它们把有分数的挤到下面
            list.sort_by(|a, b| {
                b.confidence
                    .unwrap_or(-1.0)
                    .partial_cmp(&a.confidence.unwrap_or(-1.0))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

            // 相机 / 地点单独取：前端要渲染成「相机 / 地点」两行
            let conn = s.conn();
            let camera = conn
                .query_row(
                    "SELECT model FROM image_exif WHERE image_id = ?1",
                    [image_id],
                    |r| r.get::<_, Option<String>>(0),
                )
                .ok()
                .flatten();
            let place = conn
                .query_row(
                    "SELECT place FROM image_places WHERE image_id = ?1",
                    [image_id],
                    |r| r.get::<_, Option<String>>(0),
                )
                .ok()
                .flatten();
            Ok(ImageTagsDto {
                tags: list,
                camera,
                place,
            })
        })
        // ⚠️ 以前这里手写 `ErrorDto { code: "db_error", ... }`，与其余命令的
        // `managerError` 并存。前端从没按 code 分支（grep `db_error` 零命中），
        // 两套码只是噪音 → 统一走 `merr`。
        .map_err(merr)
}

/// 拍摄年份列表（顶部「年份」下拉）：按年份倒序。
#[tauri::command]
pub async fn manager_years(state: State<'_, AppState>) -> Result<Vec<YearDto>, ErrorDto> {
    state
        .with_manager(|s| {
            Ok(s.years()
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|(year, count)| YearDto { year, count })
                .collect::<Vec<_>>())
        })
        .map_err(merr)
}

/// 相机机型列表（顶部「相机」下拉）：库里真实出现过的，按张数降序。
#[tauri::command]
pub async fn manager_cameras(state: State<'_, AppState>) -> Result<Vec<CameraDto>, ErrorDto> {
    state.with_manager(|s| {
        Ok(s.cameras(200)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|(model, count)| CameraDto { model, count })
            .collect::<Vec<_>>())
    })
    // ⚠️ 曾经写成 `code: "ai_error"`（复制粘贴自 AI 那侧），跟数据库毫无关系。
    .map_err(merr)
}

/// 一级类目的**去重**图片数（AI 标签条上「人像 52」这个数字）。
///
/// ⚠️ 不能在前端把二级的 `count` 相加：一张图常同时挂「单人」「全身」，
/// 相加会重复计数（测试图库人像组相加 89，实际 52 张，用户会以为筛选没生效）。
#[tauri::command]
pub async fn manager_tag_groups(
    state: State<'_, AppState>,
    library_id: Option<i64>,
    dir_id: Option<i64>,
) -> Result<Vec<TagGroupDto>, ErrorDto> {
    let pairs = crate::commands::ai::vocab_pairs();
    state
        .with_manager(|s| {
            Ok(s.tag_group_counts(library_id, dir_id, &pairs)
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|(l1, count)| TagGroupDto { l1, count })
                .collect::<Vec<_>>())
        })
        .map_err(merr)
}

/// 人脸分布（标签条「人像」展开后的「单人 / 合照」两格用）。
///
/// 用户 2026-11-02：「单人和合照的定义，也和人像一样，和普通 tag 不一样，
/// 使得 单人 + 合照 = 人像」。所以这两格不再看 AI 标签，而是按**一张图里有几张脸**：
/// 恰好 1 张 = 单人，≥2 张 = 合照。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FaceCountsDto {
    /// 有脸的图（= 人像）
    pub images: i64,
    /// 恰好一张脸
    pub single: i64,
    /// 两张及以上
    pub multi: i64,
}

#[tauri::command]
pub async fn manager_face_counts(
    state: State<'_, AppState>,
    library_id: Option<i64>,
    dir_id: Option<i64>,
) -> Result<FaceCountsDto, ErrorDto> {
    state
        .with_manager(|s| {
            let (images, single, multi) =
                pv_index::repo::tags::face_counts(s.conn(), library_id, dir_id)
                    .map_err(|e| e.to_string())?;
            Ok(FaceCountsDto { images, single, multi })
        })
        .map_err(merr)
}

#[tauri::command]
pub async fn manager_tags(
    state: State<'_, AppState>,
    library_id: Option<i64>,
    dir_id: Option<i64>,
) -> Result<Vec<TagDto>, ErrorDto> {
    state
        .with_manager(|s| {
            Ok(s.tags_scoped(library_id, dir_id)
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|t| TagDto {
                    id: t.id,
                    name: t.name,
                    count: t.count,
                })
                .collect::<Vec<_>>())
        })
        .map_err(merr)
}

/// 给一批图加标签（不存在则新建）。返回建立的挂载数。
#[tauri::command]
pub async fn manager_add_tags(
    state: State<'_, AppState>,
    ids: Vec<i64>,
    names: Vec<String>,
) -> Result<usize, ErrorDto> {
    state
        .with_manager_mut(|s| s.add_tags(&ids, &names).map_err(|e| e.to_string()))
        .map_err(merr)
}

/// 从一批图移除某个标签（按标签 id）。返回受影响张数。
#[tauri::command]
pub async fn manager_remove_tag(
    state: State<'_, AppState>,
    ids: Vec<i64>,
    tag_id: i64,
) -> Result<usize, ErrorDto> {
    state
        .with_manager_mut(|s| s.remove_tag(&ids, tag_id).map_err(|e| e.to_string()))
        .map_err(merr)
}

/// 这批图各自已有哪些标签（标签选择器判断全选/半选）。
#[tauri::command]
pub async fn manager_tag_counts(
    state: State<'_, AppState>,
    ids: Vec<i64>,
) -> Result<Vec<TagHitDto>, ErrorDto> {
    state
        .with_manager(|s| {
            Ok(s.tag_counts_for(&ids)
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|(name, count)| TagHitDto { name, count })
                .collect::<Vec<_>>())
        })
        .map_err(merr)
}

/// 标签改名（重名会报「已经有同名标签了」）。
#[tauri::command]
pub async fn manager_rename_tag(
    state: State<'_, AppState>,
    id: i64,
    name: String,
) -> Result<(), ErrorDto> {
    state
        .with_manager_mut(|s| s.rename_tag(id, &name).map_err(|e| e.to_string()))
        .map_err(merr)
}

/// 删除标签（拆挂载 + 删标签行，**照片不动**）。
#[tauri::command]
pub async fn manager_delete_tag(state: State<'_, AppState>, id: i64) -> Result<(), ErrorDto> {
    state
        .with_manager_mut(|s| s.delete_tag(id).map_err(|e| e.to_string()))
        .map_err(merr)
}
