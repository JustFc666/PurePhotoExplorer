//! 管理器（S2 图库）IPC 命令。
//!
//! 薄壳：把 [`pv_store::Store`] 的能力暴露给前端，**业务逻辑全在 `pv-store`**。
//!
//! - 缩略图走 **PVI1 裸字节**（复用 [`pack_pixels`]），与单图浏览同一条零 base64 通道；
//! - 其余（扫描统计、图像行、计数）走 JSON。

use std::path::PathBuf;

use tauri::ipc::Response;
use tauri::State;

use pv_store::{ImageFilter, ImageQuery, SortDir, SortKey, StoreError, Tier};

use super::dto::ErrorDto;
use super::pack_pixels;
use crate::state::AppState;

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
fn default_data_dir() -> PathBuf {
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

fn parse_sort(s: Option<&str>) -> SortKey {
    match s {
        Some("name") => SortKey::RelName,
        Some("size") => SortKey::Size,
        Some("mtime") => SortKey::Mtime,
        Some("format") => SortKey::Format,
        Some("rating") => SortKey::Rating,
        Some("added") => SortKey::Added,
        Some("random") => SortKey::Random,
        _ => SortKey::TakenAt,
    }
}

/// 单张图的标签（卡片上的「标签」按钮弹出用）
#[derive(Debug, Clone, serde::Serialize)]
pub struct ImageTagDto {
    pub name: String,
    /// `ai`（模型打的）/ `exif`（元数据）/ `user`（手动打的）
    pub source: String,
    /// 置信度；EXIF 与手动标签没有分数 → `None`
    pub confidence: Option<f64>,
}

/// 相机机型 + 张数（顶部「相机」下拉用）
#[derive(Debug, Clone, serde::Serialize)]
pub struct CameraDto {
    pub model: String,
    pub count: i64,
}

/// 年份 + 张数（顶部「年份」下拉用）
#[derive(Debug, Clone, serde::Serialize)]
pub struct YearDto {
    pub year: i64,
    pub count: i64,
}

/// 查询请求（全部可选；`camelCase` 与前端一致）。
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagerQueryDto {
    pub dir_ids: Option<Vec<i64>>,
    pub library_id: Option<i64>,
    /// 文件名子串（大小写不敏感）
    pub name_like: Option<String>,
    pub formats: Option<Vec<String>>,
    pub is_raw: Option<bool>,
    pub min_rating: Option<i64>,
    pub flag: Option<i64>,
    pub taken_from: Option<i64>,
    pub taken_to: Option<i64>,
    pub min_size: Option<i64>,
    pub max_size: Option<i64>,
    pub tags_all: Option<Vec<String>>,
    pub tags_any: Option<Vec<String>>,
    pub has_gps: Option<bool>,
    /// 地点名（离线城市名），任一命中即可
    pub places: Option<Vec<String>>,
    /// 相机机型，任一命中即可（顶部「相机」下拉）
    pub camera: Option<Vec<String>>,
    /// 拍摄年份，任一命中即可（顶部「年份」下拉）
    pub years: Option<Vec<i64>>,
    /// 标签有无：true=有标签，false=无标签，None=不限
    pub has_tags: Option<bool>,
    /// 只看某个人物的照片（人像专栏点进来）
    pub person_id: Option<i64>,
    /// 排序键：taken(默认) / name / size / mtime / rating / added / random
    pub sort: Option<String>,
    /// 是否降序（默认 true）
    pub desc: Option<bool>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

impl ManagerQueryDto {
    fn into_filter(self) -> ImageFilter {
        ImageFilter {
            dir_ids: self.dir_ids.unwrap_or_default(),
            library_id: self.library_id,
            name_like: self.name_like,
            formats: self.formats.unwrap_or_default(),
            is_raw: self.is_raw,
            min_rating: self.min_rating,
            flag: self.flag,
            taken_from: self.taken_from,
            taken_to: self.taken_to,
            min_size: self.min_size,
            max_size: self.max_size,
            tags_all: self.tags_all.unwrap_or_default(),
            tags_any: self.tags_any.unwrap_or_default(),
            has_gps: self.has_gps,
            places: self.places.unwrap_or_default(),
            cameras: self.camera.unwrap_or_default(),
            years: self.years.unwrap_or_default(),
            has_tags: self.has_tags,
            person_id: self.person_id,
        }
    }

    fn into_query(self) -> ImageQuery {
        let sort = parse_sort(self.sort.as_deref());
        let desc = self.desc.unwrap_or(true);
        let limit = self.limit;
        let offset = self.offset.unwrap_or(0);
        ImageQuery {
            filter: self.into_filter(),
            sort,
            dir: if desc { SortDir::Desc } else { SortDir::Asc },
            limit,
            offset,
        }
    }
}

/// 图像行（网格用）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageRowDto {
    pub id: i64,
    pub dir_id: i64,
    pub rel_name: Option<String>,
    /// 绝对路径（卷未挂载时为 null）；前端双击打开/复制路径要用
    pub abs_path: Option<String>,
    pub size: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub format: Option<String>,
    pub is_raw: bool,
    pub taken_at: Option<i64>,
    pub rating: i64,
    pub flag: i64,
    /// 这张图有没有标签 —— 卡片据此显示「无匹配」
    pub has_tags: bool,
}

impl From<pv_index::models::Image> for ImageRowDto {
    fn from(i: pv_index::models::Image) -> Self {
        Self {
            id: i.id,
            dir_id: i.dir_id,
            rel_name: i.rel_name,
            abs_path: None, // 由 manager_query 用 store 补全
            size: i.size,
            width: i.width,
            height: i.height,
            format: i.format,
            is_raw: i.is_raw,
            taken_at: i.taken_at,
            rating: i.rating,
            flag: i.flag,
            has_tags: i.has_tags,
        }
    }
}

/// 扫描统计。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanSummaryDto {
    pub folders: u64,
    pub added: u64,
    pub changed: u64,
    pub removed: u64,
}

/// 侧栏「文件夹」树节点（扁平表，前端按 `parentId` 组树）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderNodeDto {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
    pub rel_path: String,
    pub image_count: i64,
    pub is_root: bool,
    pub volume_id: i64,
    pub volume_label: String,
    /// 卷未挂载时为 null（离线）
    pub mount_path: Option<String>,
}

/// 侧栏「图库」条目。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryDto {
    pub id: i64,
    pub name: String,
    /// normal / smart / encrypted
    pub kind: String,
    pub rule: Option<String>,
    pub count: i64,
}

/// 图库的成员关系（侧栏图库树的**二级**：图库 → 它包含的目录）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryMemberDto {
    pub library_id: i64,
    pub folder_id: i64,
}

/// 单个格式的统计。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FormatStatDto {
    pub format: String,
    pub count: i64,
    pub bytes: i64,
}

/// 全库统计（状态栏 + 筛选条）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsDto {
    pub total_count: i64,
    pub total_bytes: i64,
    pub formats: Vec<FormatStatDto>,
}

/// 打开单个目录的结果。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenFolderDto {
    /// 索引里的文件夹 id —— 前端拿它做「只看这个目录」的查询
    pub folder_id: i64,
    pub added: u64,
    pub changed: u64,
    pub removed: u64,
}

/// 从图库移除的结果。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemovedDto {
    pub folders: u64,
    pub images: u64,
    /// 顺带清掉的缩略图缓存文件数
    pub thumbs: u64,
    /// 因为卷未挂载而没能清缓存的图片数
    pub thumbs_orphan: u64,
}

/// 导出失败的一张（`name` 只是文件名，路径在提示里不重复)。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportFailureDto {
    pub name: String,
    pub reason: String,
}

/// 一次批量导出的结果。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportSummaryDto {
    pub copied: usize,
    pub skipped: usize,
    pub failed: Vec<ExportFailureDto>,
    pub dest_dir: String,
}

/// 数据目录（设置窗口里显示 + 「打开数据目录」用）。
///
/// 索引库 `library.db`、缩略图缓存、人脸向量都在这里。
#[tauri::command]
pub fn app_data_dir() -> String {
    default_data_dir().to_string_lossy().into_owned()
}

/// 打开管理器（建库 + 缓存目录）。`data_dir` 省略时用 `%LOCALAPPDATA%\PurePhotoExplorer`。
///
/// ⚠️ **必须是 `async`**：Tauri 里**非 `async` 命令跑在主线程上**，会阻塞窗口的
/// 消息泵（`Store::open` 要跑建表/迁移，慢时窗口直接变「未响应」）。
/// `async` 命令改在 async runtime 的线程池上执行。详见下方 `manager_thumbnail` 的注释。
#[tauri::command]
pub async fn manager_open(
    state: State<'_, AppState>,
    data_dir: Option<String>,
) -> Result<String, ErrorDto> {
    let dir = data_dir.map(PathBuf::from).unwrap_or_else(default_data_dir);
    state.open_manager(&dir).map_err(merr)?;
    // 老库回填：把还没探到尺寸的行补上（有上限，防大库首启卡太久；
    // 没补完的下次启动接着补）。放在**首次查询之前**，
    // 卡片的「宽 × 高」才能第一次就显示出来。
    state
        .with_manager_mut(|s| {
            s.backfill_dimensions(4000);
            Ok(())
        })
        .map_err(merr)?;
    // 拍摄时间同样要回填：没有 `taken_at`，「按拍摄时间排序」会静默退化成
    // 「按入库时间」。单张要 `open` 读 EXIF，所以给个更小的上限（1500），
    // 没补完的下次启动接着补 —— 全补完之后每次只剩一条 COUNT。
    state
        .with_manager_mut(|s| {
            s.backfill_times(1500);
            Ok(())
        })
        .map_err(merr)?;
    // 地点同样要回填（坐标 → 离线城市名）。放在时间之后：`image_places`
    // 读的是 `image_exif` 里已有的 GPS，本身不读文件，代价极小（纯查表）。
    state
        .with_manager_mut(|s| {
            s.backfill_places(2000);
            // 老库 added_ts 全是 NULL（入库漏写）→ 年份筛选会漏图，一次性补上
            s.backfill_added_ts();
            Ok(())
        })
        .map_err(merr)?;
    Ok(dir.to_string_lossy().into_owned())
}

/// 递归首扫：把 `root` 目录树建成图库索引。
///
/// ⚠️ `async`：扫描是大目录下的重活（`async` 的理由同 `manager_open`）。
#[tauri::command]
pub async fn manager_scan(
    state: State<'_, AppState>,
    root: String,
) -> Result<ScanSummaryDto, ErrorDto> {
    state
        .with_manager_mut(|s| {
            let t = s
                .scan_tree(std::path::Path::new(&root))
                .map_err(|e| e.to_string())?;
            Ok(ScanSummaryDto {
                folders: t.folders,
                added: t.images.added,
                changed: t.images.changed,
                removed: t.images.removed,
            })
        })
        .map_err(merr)
}

/// 组合查询（过滤/排序/分页）。
///
/// ⭐ 带 `libraryId` 时走 `query_in_library`：**普通库 JOIN，智能库按规则求值**。
/// 智能库没有成员目录行，直接 JOIN `library_folders` 会恒为空 —— 必须分路。
#[tauri::command]
pub async fn manager_query(
    state: State<'_, AppState>,
    query: ManagerQueryDto,
) -> Result<Vec<ImageRowDto>, ErrorDto> {
    let lib_id = query.library_id;
    let q = query.into_query();
    state
        .with_manager(|s| {
            let imgs = match lib_id {
                Some(id) => s.query_in_library(&q, id),
                None => s.query(&q),
            }
            .map_err(|e| e.to_string())?;
            Ok(imgs
                .into_iter()
                .map(|i| {
                    let abs = s
                        .abs_path(&i)
                        .ok()
                        .flatten()
                        .map(|p| p.to_string_lossy().into_owned());
                    let mut dto = ImageRowDto::from(i);
                    dto.abs_path = abs;
                    dto
                })
                .collect::<Vec<_>>())
        })
        .map_err(merr)
}

/// 命中数量（分页界面的"共 N 张"）。
#[tauri::command]
pub async fn manager_count(
    state: State<'_, AppState>,
    query: ManagerQueryDto,
) -> Result<i64, ErrorDto> {
    let lib_id = query.library_id;
    let f = query.into_filter();
    state
        .with_manager(|s| {
            let r = match lib_id {
                Some(id) => s.count_in_library(&f, id),
                None => s.count(&f),
            };
            r.map_err(|e| e.to_string())
        })
        .map_err(merr)
}

/// 侧栏「文件夹」树：全部文件夹节点（扁平表，前端自己组树）。
#[tauri::command]
pub async fn manager_folders(state: State<'_, AppState>) -> Result<Vec<FolderNodeDto>, ErrorDto> {
    state
        .with_manager(|s| {
            s.folders()
                .map(|v| {
                    v.into_iter()
                        .map(|f| FolderNodeDto {
                            id: f.id,
                            parent_id: f.parent_id,
                            name: f.name,
                            rel_path: f.rel_path,
                            image_count: f.image_count,
                            is_root: f.is_root,
                            volume_id: f.volume_id,
                            volume_label: f.volume_label,
                            mount_path: f.mount_path,
                        })
                        .collect()
                })
                .map_err(|e| e.to_string())
        })
        .map_err(merr)
}

/// **打开单个目录**（不递归）：登记卷与目录、扫描该目录里的图片。
///
/// ⚠️ 与 `manager_scan`（递归首扫）是两件事，别混用：
/// 文件浏览器里点一个目录走这里 —— 用递归版点一下盘符就会遍历整块盘。
#[tauri::command]
pub async fn manager_open_folder(
    state: State<'_, AppState>,
    path: String,
) -> Result<OpenFolderDto, ErrorDto> {
    state
        .with_manager_mut(|s| {
            let r = s
                .open_folder(std::path::Path::new(&path))
                .map_err(|e| e.to_string())?;
            Ok(OpenFolderDto {
                folder_id: r.folder_id,
                added: r.images.added,
                changed: r.images.changed,
                removed: r.images.removed,
            })
        })
        .map_err(merr)
}

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

/// 全库统计：张数 / 总字节 / 分格式。
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

/// 全库统计：张数 / 总字节 / 分格式。
#[tauri::command]
pub async fn manager_stats(state: State<'_, AppState>) -> Result<StatsDto, ErrorDto> {
    state
        .with_manager(|s| {
            s.stats()
                .map(|st| StatsDto {
                    total_count: st.total_count,
                    total_bytes: st.total_bytes,
                    formats: st
                        .formats
                        .into_iter()
                        .map(|f| FormatStatDto {
                            format: f.format,
                            count: f.count,
                            bytes: f.bytes,
                        })
                        .collect(),
                })
                .map_err(|e| e.to_string())
        })
        .map_err(merr)
}

/// 取缩略图：**PVI1 裸字节**（与单图浏览同一条零 base64 通道）。
///
/// 三级回退（L1 内存 → L2 磁盘 → 现生成）都在 `pv-store` 内完成。
///
/// ## ⚠️ 为什么这个命令**必须**是 `async`（2026-10-15 实机踩坑）
/// Tauri 的规矩是：**非 `async` 的命令跑在主线程上**（要离开主线程就得写 `async fn`）。
/// 而这里的「现生成」是一次**完整解码**（几十 MB 的 JPEG / 7680×4320 的 RAW 内嵌预览）。
/// 图库首屏会同时挂几百个单元格，每个都 `invoke` 一次 —— 命令在主线程上**串行**跑完，
/// 消息泵被堵住，Windows 直接给窗口打上**「未响应」**（实测 `test-image/` 54 张
/// 生成了 50 个缩略图时窗口一直假死）。
///
/// 改成 `async` 后执行点在 async runtime 的线程池上，主线程只负责 IPC 收发。
/// 前端另外还有 `ipc/thumbQueue.ts` 的并发闸门 + 可视区懒加载，两层都要有：
/// - 后端 `async` 解决「窗口假死」；
/// - 前端限流解决「一次堆几百个请求、进度不可见」。
///
/// ⚠️ 注意 `Store` 仍被 `std::sync::Mutex` 串行保护（`rusqlite::Connection` 不是
/// `Sync`），所以**吞吐仍是串行**的；但窗口不卡了，配合 L2 缓存第二次访问就是瞬时。
/// 真要做到解码并行，得把 L1/L2/解码器从数据库锁里拆出来（留给 S2.2）。
#[tauri::command]
pub async fn manager_thumbnail(
    state: State<'_, AppState>,
    image_id: i64,
    tier: String,
) -> Result<Response, ErrorDto> {
    let tier = parse_tier(&tier).ok_or_else(|| merr(format!("未知缩略图档位：{tier}")))?;
    let bytes = state
        .with_manager(|s| {
            let img = pv_index::repo::images::get(s.conn(), image_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("图像 {image_id} 不在索引里"))?;
            match s.thumbnail(&img, tier).map_err(|e| e.to_string())? {
                Some(th) => {
                    let rgba = pv_decode::RgbaImage {
                        width: th.width,
                        height: th.height,
                        data: th.rgba,
                    };
                    Ok(pack_pixels(&rgba))
                }
                None => Err("图片离线（所在卷未挂载）".to_string()),
            }
        })
        .map_err(merr)?;
    Ok(Response::new(bytes))
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
#[tauri::command]
pub async fn manager_add_to_library(
    state: State<'_, AppState>,
    folder_id: i64,
    name: String,
    library_id: Option<i64>,
) -> Result<i64, ErrorDto> {
    state
        .with_manager_mut(|s| {
            s.add_to_library(folder_id, &name, library_id)
                .map_err(|e| e.to_string())
        })
        .map_err(merr)
}

/// 批量导出：把选中的**原图**复制到 `dest_dir`。
///
/// ⚠️ **只复制、不移动、不删除** —— 索引和原图都不受影响。
/// 重名自动退成 `name (1).jpg`；源目录 == 目标目录的会跳过（见 `pv_store::export`）。
///
/// ⚠️ **必须 `async`**：复制几十上百张原图是纯 IO，非 async 命令会跑在主线程上
/// 把窗口堵成「未响应」（同 `manager_thumbnail` 的教训）。
#[tauri::command]
pub async fn manager_export_images(
    paths: Vec<String>,
    dest_dir: String,
) -> Result<ExportSummaryDto, ErrorDto> {
    if paths.is_empty() {
        return Err(merr("没有选中任何图片"));
    }
    let dest = PathBuf::from(&dest_dir);
    let sum = pv_store::export::copy_images(&paths, &dest).map_err(|e| merr(e.to_string()))?;
    Ok(ExportSummaryDto {
        copied: sum.copied,
        skipped: sum.skipped,
        failed: sum
            .failed
            .into_iter()
            .map(|f| ExportFailureDto {
                name: f.name,
                reason: f.reason,
            })
            .collect(),
        dest_dir: sum.dest_dir.to_string_lossy().into_owned(),
    })
}

/// 在系统资源管理器里定位到某个文件（选中它）。
///
/// 用 `explorer /select,"<path>"`。⚠️ 参数必须用 **`raw_arg`** 传：
/// `Command` 默认会给每个参数加引号，而 explorer 的 `/select,` 要求
/// 「开关 + 路径」是**同一个参数**，被拆开就会当成两个路径打开两个窗口。
#[tauri::command]
pub fn reveal_in_explorer(path: String) -> Result<(), ErrorDto> {
    let p = PathBuf::from(&path);
    if !p.exists() {
        return Err(merr("文件不存在（卷可能未挂载）"));
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // raw_arg 需要「完整的一个参数」，路径带空格时也要保住引号
        let arg = format!("/select,\"{}\"", p.display());
        std::process::Command::new("explorer.exe")
            .raw_arg(arg)
            .spawn()
            .map_err(|e| merr(format!("打开资源管理器失败：{e}")))?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        Err(merr("当前平台不支持在资源管理器中定位"))
    }
}

/// 批量设置星级（0–5）。**只改索引**，不动原图、不动缓存。
///
/// 与「导出/删除」不同，这条不需要 `async` 的 IO 理由 —— 但为一致性仍走
/// `with_manager_mut`，且一次 `UPDATE ... WHERE id IN (...)` 就够快。
#[tauri::command]
pub async fn manager_set_rating(
    state: State<'_, AppState>,
    ids: Vec<i64>,
    rating: i64,
) -> Result<usize, ErrorDto> {
    state
        .with_manager_mut(|s| s.set_rating(&ids, rating).map_err(|e| e.to_string()))
        .map_err(merr)
}

/// 批量设置旗标（0 = 无，1 = 保留）。**只改索引**。
#[tauri::command]
pub async fn manager_set_flag(
    state: State<'_, AppState>,
    ids: Vec<i64>,
    flag: i64,
) -> Result<usize, ErrorDto> {
    state
        .with_manager_mut(|s| s.set_flag(&ids, flag).map_err(|e| e.to_string()))
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

/// 磁盘缩略图缓存的占用（设置面板显示「N 个文件 / X MB」）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheStatsDto {
    pub bytes: u64,
    pub files: usize,
}

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

/// 侧栏「地点」条目（离线城市名 + 张数）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaceDto {
    pub name: String,
    pub count: i64,
}

/// 侧栏「地点」分区：全部解析出来的城市 + 各自张数（多的在前）。
///
/// ⚠️ 必须在查询列表**之前**调一次：地点是启动回填写进 `image_places` 的，
/// 第一次启动时这一轮还没跑完，列表会是空的。
#[tauri::command]
pub async fn manager_places(state: State<'_, AppState>) -> Result<Vec<PlaceDto>, ErrorDto> {
    state
        .with_manager(|s| {
            Ok(s.places()
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

/// 侧栏「标签」条目。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagDto {
    pub id: i64,
    pub name: String,
    pub count: i64,
}

/// 一级类目 → **去重**后的图片张数。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagGroupDto {
    pub l1: String,
    pub count: i64,
}

/// 标签选择器用：这批图里某个标签命中了多少张。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagHitDto {
    pub name: String,
    pub count: i64,
}

/// 全部标签 + 使用次数（侧栏用）。
/// 单张图的标签 + 相机机型 + 地点。
///
/// `camera` / `place` 单独给，是因为前端要把它们渲染成「相机 / 地点」两行，
/// 只靠标签名分不出「这个名字是机型还是城市」。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ImageTagsDto {
    pub tags: Vec<ImageTagDto>,
    pub camera: Option<String>,
    pub place: Option<String>,
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
        .map_err(|e| ErrorDto {
            code: "db_error".into(),
            message: e,
            user_visible: true,
        })
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
        .map_err(|e| ErrorDto {
            code: "db_error".into(),
            message: e,
            user_visible: true,
        })
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
    .map_err(|e| ErrorDto { code: "ai_error".into(), message: e, user_visible: true })
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

#[tauri::command]
pub async fn manager_tags(
    state: State<'_, AppState>,
    library_id: Option<i64>,
    dir_id: Option<i64>,
) -> Result<Vec<TagDto>, ErrorDto> {    state
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
/// ⛔ **只删这个分组**：`libraries` 行 + 它的 `library_folders` 关系。
/// 目录的索引行、缩略图缓存、磁盘上的原图**一律不动** ——
/// 照片仍然在「全部照片」里，还能再挂到别的图库上。
#[tauri::command]
pub async fn manager_delete_library(
    state: State<'_, AppState>,
    id: i64,
) -> Result<bool, ErrorDto> {
    state
        .with_manager_mut(|s| s.delete_library(id).map_err(|e| e.to_string()))
        .map_err(|e| ErrorDto {
            code: "managerError".into(),
            message: e,
            user_visible: true,
        })
}
