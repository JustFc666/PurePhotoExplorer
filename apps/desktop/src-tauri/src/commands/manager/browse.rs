//! 打开数据目录、扫描入库、组合查询 —— 「有什么图」这条线。
//!
//! ⚠️ 全是 `async`：Tauri 里**非 async 命令跑在主线程**，`Store::open` 建表、
//! 目录扫描、首屏几百条查询都会把窗口堵成「未响应」。

use std::path::PathBuf;

use tauri::State;

use crate::commands::dto::ErrorDto;
use super::{
    default_data_dir, merr, FolderNodeDto, FormatStatDto, ImageRowDto, ManagerQueryDto,
    OpenFolderDto, ScanSummaryDto, StatsDto,
};
use crate::state::AppState;

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
    /*
     * EXIF 方向回填（2026-10-30）：老库的 `orientation` 全是 NULL，
     * `width/height` 又是按**原始像素**填的 —— 竖拍照片一直是横的。
     * 这一步读一次 EXIF、写方向，并把 6/8 那批的宽高换回来（幂等，补完即空）。
     */
    state
        .with_manager_mut(|s| {
            s.backfill_orientation(2000);
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

    /*
     * 老库清理：删掉「已经有二级词支撑」的一级类目标签。
     *
     * 早期 Python 版打标把一级类目也当独立标签写库，于是一张刀剑图会同时挂着
     * 「兵器 0.45（一级）」「刀剑 0.47（二级）」「物品 0.40（一级）」——
     * 用户看到它被同时归进「兵器」和「物品」两个目录。
     * 一级本来就能由词表的 l2→l1 反推，属于派生数据，不该单独存一份。
     *
     * 幂等：清过一次之后这条 DELETE 命中 0 行，每次启动只剩一次空扫描。
     * 词表读不到（`l1` 为空）时函数本身是空操作，不会误删。
     */
    let l1_names = crate::commands::ai::vocab_l1_names();
    state
        .with_manager_mut(|s| {
            s.drop_redundant_l1_tags(&l1_names).map_err(|e| e.to_string())?;
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

/// 打开目录并（可选）扫描其中的图片，用于「加入图库」。
///
/// - `recursive = false`（默认）：**只扫这一层** —— 文件浏览器里点目录看照片走这条，
///   否则点一下盘符就会遍历整块盘。
/// - `recursive = true`：**连同全部子目录**一起入库。用户右键「加入图库」时，
///   如果该目录有子文件夹，前端会先问一句「是否包含子文件夹」，答是才传 true。
///
/// ⚠️ 递归版的目录数超过 `pv_scan::tree::MAX_TREE_DIRS` 会**报错**（不静默截断）。
#[tauri::command]
pub async fn manager_open_folder(
    state: State<'_, AppState>,
    path: String,
    recursive: Option<bool>,
) -> Result<OpenFolderDto, ErrorDto> {
    let dir = std::path::Path::new(&path);
    let recurse = recursive.unwrap_or(false);

    state
        .with_manager_mut(|s| {
            if recurse {
                /*
                 * 「包含子文件夹」——用户 2026-10-31 要求。
                 * 走 `open_folder_tree`：整棵子树建进 folders，逐目录扫图 + 补尺寸/时间。
                 * ⚠️ 目录数超上限会直接报错（`MAX_TREE_DIRS`），不静默截断。
                 */
                let t = s.open_folder_tree(dir).map_err(|e| e.to_string())?;
                return Ok(OpenFolderDto {
                    folder_id: t.root_folder_id,
                    added: t.images.added,
                    changed: t.images.changed,
                    removed: t.images.removed,
                    folders: t.folders,
                });
            }
            let r = s.open_folder(dir).map_err(|e| e.to_string())?;
            Ok(OpenFolderDto {
                folder_id: r.folder_id,
                added: r.images.added,
                changed: r.images.changed,
                removed: r.images.removed,
                folders: 1,
            })
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
