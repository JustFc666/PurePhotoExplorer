//! 前端拿到的数据结构（IPC 契约）。
//!
//! 全部 `#[serde(rename_all = "camelCase")]`：前端按 `camelCase` 读，
//! 字段名一旦变，TS 侧拿到 `undefined` 而且**不会报错** —— 改这里必须同步
//! `apps/desktop/src/ipc/` 的类型定义。
//!
//! ⭐ [`ManagerQueryDto`] 是唯一的入参结构，[`ImageRowDto`] 是唯一的出参行 ——
//! 其余都是侧栏/统计用的窄结构。

use pv_store::{ImageFilter, ImageQuery, SortDir};

use super::parse_sort;

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
    pub(super) fn into_filter(self) -> ImageFilter {
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

    pub(super) fn into_query(self) -> ImageQuery {
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
    /// 本次**登记了几个目录**（递归入库时是整棵子树的目录数，单层恒为 1）。
    /// 前端用它给用户一句「包含 N 个子文件夹」的确认，否则用户不知道到底加了什么。
    pub folders: u64,
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

/// 磁盘缩略图缓存的占用（设置面板显示「N 个文件 / X MB」）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheStatsDto {
    pub bytes: u64,
    pub files: usize,
}

/// 侧栏「地点」条目（离线城市名 + 张数）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaceDto {
    pub name: String,
    pub count: i64,
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
