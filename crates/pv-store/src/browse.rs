//! 浏览模式：**不写索引、不生成缩略图**地列一个目录里的图片。
//!
//! 与入库路径（`scan`）严格分开 —— 两者混用会让「看一眼」变成「偷偷入库」。

use std::path::Path;

use crate::Result;

/// 一次浏览列表里的单个文件（**不经过索引**）。
pub struct DirImage {
    /// 绝对路径
    pub path: String,
    pub name: String,
    pub size: i64,
    /// 修改时间（Unix 纳秒）。
    ///
    /// 浏览列表**没有索引**，所以「按修改时间排序」「按拍摄时间排序的兜底」
    /// 都只能靠它 —— 必须带出来，不能只给入库模式用。
    pub mtime_ns: i64,
    pub format: String,
    pub is_raw: bool,
}

impl DirImage {
    /// 从遍历结果造一行。两个列表函数（慢/快）共用，避免字段漏传。
    fn from_found(dir: &Path, f: pv_scan::FoundFile) -> Self {
        Self {
            path: dir.join(&f.rel_name).to_string_lossy().into_owned(),
            name: f.rel_name,
            size: f.size,
            mtime_ns: f.mtime_ns,
            format: f.format,
            is_raw: f.is_raw,
        }
    }
}

/// 列出目录里的图片文件：**单层、不写索引、不生成缩略图**。
///
/// 与 [`Store::open_folder`] 的区别是：那个会**建索引行**（从此进入图库），
/// 这个纯粹是「看一眼」—— 文件浏览器点目录时用。
///
/// 复用 `pv_scan::walk_dir`，所以格式判定与入库那条路**完全一致**
/// （只看魔数不信扩展名），不会出现「浏览看得到、加库加不进去」这种偏差。
///
/// ⚠️ 浏览走的是 [`list_dir_images_fast`]（扩展名优先、不 open 文件）——
/// 那是为了把一个 1224 个文件的目录从 9.5 秒降到几十毫秒。这个函数留给
/// 需要「严格按魔数」的场合（测试对照用）。
pub fn list_dir_images(dir: &Path) -> Result<Vec<DirImage>> {
    Ok(pv_scan::walk_dir(dir)?
        .into_iter()
        .map(|f| DirImage::from_found(dir, f))
        .collect())
}

/// 浏览模式专用的「快速列目录」：**不打开文件**，靠扩展名判格式。
///
/// 实测（1224 个文件 / 37.9 GB，机械盘冷启动）：
/// `list_dir_images` **9.47 秒** → `list_dir_images_fast` **几十毫秒**。
/// 慢的根源是一次 1224 个 `open`（每个 8ms 级随机读），而它们的扩展名全在白名单里。
///
/// ⛔ **只有浏览能用它**。入库（`open_folder`）必须用 `walk_dir` 的魔数判定 ——
/// 索引写错比界面慢严重得多。详见 `pv_scan::walker::walk_dir_fast` 的注释。
pub fn list_dir_images_fast(dir: &Path) -> Result<Vec<DirImage>> {
    Ok(pv_scan::walk_dir_fast(dir)?
        .into_iter()
        .map(|f| DirImage::from_found(dir, f))
        .collect())
}
