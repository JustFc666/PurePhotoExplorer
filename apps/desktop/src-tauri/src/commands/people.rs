//! 人物（人脸归纳）命令：列表 / 改名 / 合并 / 拆分 / 清除。
//!
//! ## 分层
//! 这里只做 **DTO 转换 + 参数校验**，真正的 SQL 在 `pv-index::repo::{faces, persons}`；
//! 业务规则（≥3 张脸才建人物、阈值、两阶段聚类）在
//! `tools/ai/face_cluster.py` 与文档 §3.3，**不在这一层**。
//!
//! ## 为什么所有写操作都是 `async`
//! ⛔ 见 `commands/mod.rs` 的说明：非 async 的重活命令会堵 Tauri 的消息泵，
//! 界面直接「未响应」。这里虽然只是 SQL，但合并可能一次改几千行，仍按 async 写。

use pv_index::models::Person;
use pv_index::repo::{faces as faces_repo, persons as persons_repo};
use tauri::State;

use super::dto::ErrorDto;
use super::manager::merr;
use crate::state::AppState;

/// 侧栏「人物」条目。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonDto {
    pub id: i64,
    /// `None` = 未命名（前端显示「未命名 ①」）
    pub name: Option<String>,
    pub face_count: i64,
    /// 封面脸所在的图片 + 归一化框（前端拿缩略图自己裁头像）
    pub cover: Option<FaceBoxDto>,
}

/// 一张人脸的几何信息（归一化坐标，相对 `tier` 那一档）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FaceBoxDto {
    pub id: i64,
    pub image_id: i64,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// 脸宽像素（质量指标，UI 可据此给「小脸」提示）
    pub quality: Option<f32>,
}

impl From<&pv_index::models::Face> for FaceBoxDto {
    fn from(f: &pv_index::models::Face) -> Self {
        Self {
            id: f.id,
            image_id: f.image_id,
            x: f.x,
            y: f.y,
            w: f.w,
            h: f.h,
            quality: f.quality,
        }
    }
}

/// 人脸库概览（设置面板显示"已分析多少脸 / 几个人物"）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FacesStatsDto {
    pub faces: i64,
    /// 已归属人物的人脸数
    pub assigned: i64,
    pub persons: i64,
    pub named_persons: i64,
    /// 出现过的人脸模型版本（换模型后这里会有两条，提示需要重建）
    pub models: Vec<String>,
}

/// 人物列表。默认只返回 ≥3 张脸的（与聚类口径一致）。
#[tauri::command]
pub async fn manager_people(
    state: State<'_, AppState>,
    min_faces: Option<i64>,
    library_id: Option<i64>,
) -> Result<Vec<PersonDto>, ErrorDto> {
    let min = min_faces.unwrap_or(3).max(1);
    state
        .with_manager(|s| {
            /*
             * 按图库过滤（2026-10-25 用户要求）：人像只能出现在"他自己的图库"里。
             * 判定 = 这个人**至少有一张脸**落在该图库的成员目录里。
             * ⚠️ 用 EXISTS 子查询，不要 JOIN —— 一个人在该库有多张脸时会被复制成多行。
             */
            let mut list = persons_repo::list(s.conn(), min).map_err(|e| e.to_string())?;
            if let Some(lib) = library_id {
                list.retain(|p| {
                    s.conn()
                        .query_row(
                            "SELECT 1 FROM faces f JOIN images i ON i.id = f.image_id JOIN library_folders lf ON lf.folder_id = i.dir_id WHERE f.person_id = ?1 AND lf.library_id = ?2 LIMIT 1",
                            (p.id, lib),
                            |_| Ok(()),
                        )
                        .is_ok()
                });
            }
            let mut out = Vec::with_capacity(list.len());
            for p in list {
                // 封面：优先用库里记的 cover_face_id，找不到就取这片簇里最大的一张
                let cover = match p.cover_face_id {
                    Some(cid) => faces_repo::list_by_person(s.conn(), p.id)
                        .map_err(|e| e.to_string())?
                        .into_iter()
                        .find(|f| f.id == cid)
                        .map(|f| FaceBoxDto::from(&f)),
                    None => None,
                };
                out.push(PersonDto {
                    id: p.id,
                    name: p.name,
                    face_count: p.face_count,
                    cover,
                });
            }
            Ok(out)
        })
        .map_err(merr)
}

/// 某个人物下的所有脸（人物详情页的网格用）。
#[tauri::command]
pub async fn manager_person_faces(
    state: State<'_, AppState>,
    person_id: i64,
) -> Result<Vec<FaceBoxDto>, ErrorDto> {
    state
        .with_manager(|s| {
            Ok(faces_repo::list_by_person(s.conn(), person_id)
                .map_err(|e| e.to_string())?
                .iter()
                .map(FaceBoxDto::from)
                .collect::<Vec<_>>())
        })
        .map_err(merr)
}

/// 给人物改名；`name = None` 表示取消命名。
#[tauri::command]
pub async fn manager_rename_person(
    state: State<'_, AppState>,
    person_id: i64,
    name: Option<String>,
) -> Result<(), ErrorDto> {
    // 空串按"取消命名"处理：界面上清空输入框就是不要这个名字了
    let clean = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    state
        .with_manager(|s| {
            persons_repo::rename(s.conn(), person_id, clean.as_deref())
                .map_err(|e| e.to_string())
        })
        .map_err(merr)
}

/// 把人脸从一个人物**合并**到另一个人物，返回被并过去的脸数。
///
/// ⚠️ 误合并比误拆分更伤（用户不易察觉），所以 UI 上必须二次确认。
#[tauri::command]
pub async fn manager_merge_persons(
    state: State<'_, AppState>,
    from: Vec<i64>,
    into: i64,
) -> Result<usize, ErrorDto> {
    if from.contains(&into) {
        return Err(merr("不能把人物合并到它自己"));
    }
    state
        .with_manager(|s| persons_repo::merge(s.conn(), &from, into).map_err(|e| e.to_string()))
        .map_err(merr)
}

/// 把选中的几张脸从人物里**拆出来**，建成一个新的未命名人物。
///
/// 返回新人物 id（`None` = 一张都没拆成，比如那些脸本就不属于该人物）。
#[tauri::command]
pub async fn manager_split_faces(
    state: State<'_, AppState>,
    person_id: i64,
    face_ids: Vec<i64>,
) -> Result<Option<i64>, ErrorDto> {
    state
        .with_manager(|s| {
            persons_repo::split(s.conn(), person_id, &face_ids, Some("manual-split"))
                .map_err(|e| e.to_string())
        })
        .map_err(merr)
}

/// 把某个人的封面换成「这张图里该人的那张脸」。
///
/// 同一张图里同一个人可能被检出多张脸（合影、浮雕、海报），取**面积最大**的那张。
/// 返回新封面；`None` = 这张图里没有这个人的脸 —— 前端要提示，不能静默成功。
///
/// ⚠️ 只动 `persons.cover_face_id`，不碰 `faces` 与聚类结果。
#[tauri::command]
pub async fn manager_set_person_cover(
    state: State<'_, AppState>,
    person_id: i64,
    image_id: i64,
) -> Result<Option<FaceBoxDto>, ErrorDto> {
    state
        .with_manager(|s| {
            let faces =
                faces_repo::list_by_person(s.conn(), person_id).map_err(|e| e.to_string())?;
            let best = faces
                .into_iter()
                .filter(|f| f.image_id == image_id)
                .max_by(|a, b| {
                    (a.w * a.h)
                        .partial_cmp(&(b.w * b.h))
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            match best {
                Some(f) => {
                    persons_repo::set_cover(s.conn(), person_id, Some(f.id))
                        .map_err(|e| e.to_string())?;
                    Ok(Some(FaceBoxDto::from(&f)))
                }
                None => Ok(None),
            }
        })
        .map_err(merr)
}

/// 删除人物：**只解绑人脸，不删人脸本身**（识别结果不属于人物）。
#[tauri::command]
pub async fn manager_delete_person(
    state: State<'_, AppState>,
    person_id: i64,
) -> Result<(), ErrorDto> {
    state
        .with_manager(|s| persons_repo::delete(s.conn(), person_id).map_err(|e| e.to_string()))
        .map_err(merr)
}

/// 人脸库概览。
#[tauri::command]
pub async fn manager_faces_stats(state: State<'_, AppState>) -> Result<FacesStatsDto, ErrorDto> {
    state
        .with_manager(|s| {
            let conn = s.conn();
            let faces: i64 = conn
                .query_row("SELECT COUNT(*) FROM faces", [], |r| r.get(0))
                .map_err(|e| e.to_string())?;
            let assigned: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM faces WHERE person_id IS NOT NULL",
                    [],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            let persons: i64 = conn
                .query_row("SELECT COUNT(*) FROM persons", [], |r| r.get(0))
                .map_err(|e| e.to_string())?;
            let named: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM persons WHERE name IS NOT NULL",
                    [],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            let models = faces_repo::count_by_model(conn)
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|(m, _)| m)
                .collect();
            Ok(FacesStatsDto {
                faces,
                assigned,
                persons,
                named_persons: named,
                models,
            })
        })
        .map_err(merr)
}

/// **清除全部人脸数据**（隐私开关）。
///
/// ⛔ 只删 `faces` / `persons`，**不碰图片索引与缩略图缓存** ——
/// 用户关掉人脸功能不该丢掉他的图库。
#[tauri::command]
pub async fn manager_faces_clear(state: State<'_, AppState>) -> Result<usize, ErrorDto> {
    state
        .with_manager(|s| {
            persons_repo::delete_all(s.conn()).map_err(|e| e.to_string())?;
            faces_repo::delete_all(s.conn(), None).map_err(|e| e.to_string())
        })
        .map_err(merr)
}

/// 供测试与后续命令复用：把 `Person` 转成 DTO（封面留空，由调用方补）。
#[allow(dead_code)]
pub(crate) fn person_dto(p: &Person) -> PersonDto {
    PersonDto {
        id: p.id,
        name: p.name.clone(),
        face_count: p.face_count,
        cover: None,
    }
}
