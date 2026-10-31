//! `pv-store`：**服务层**，把索引 / 扫描 / 缓存 / 解码串成 UI 可直接用的 API。
//!
//! 分工：
//! - `pv-index` 存索引、`pv-scan` 扫文件系统、`pv-cache` 存缩略图、`pv-decode` 解码；
//! - 本 crate 持有 `Connection` + `L1`/`L2` + `DecoderRegistry` + `VolumeResolver`，
//!   对外只暴露三个动作：**扫描 / 查询 / 取缩略图**。
//!
//! 取缩略图是三级回退：**L1 内存 → L2 磁盘 → 现生成（回填 L1+L2）**。

#![cfg_attr(test, allow(non_snake_case))]

use std::path::{Path, PathBuf};

use pv_cache::{compute_key, CachedImage, CacheKey, CacheSource, L1, L2, ThumbTier};
use pv_decode::{
    BitmapBackend, DecodePurpose, DecodeTarget, DecoderRegistry, EmbeddedPreviewPipeline,
};
use pv_index::models::Image;
use pv_index::{conn as index_conn, migrate, repo, Connection};
use pv_scan::{Summary, TreeSummary, VolumeResolver};

/// 统一错误：索引错误 + 解码错误。
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Index(#[from] pv_index::IndexError),
    #[error(transparent)]
    Decode(#[from] pv_core::error::PvError),
    #[error(transparent)]
    Lib(#[from] pv_lib::LibError),
    #[error("IO 错误：{0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// 批量导出：把选中的原图复制到目标目录（纯文件操作，不碰索引）。
pub mod export;

// 便捷再导出（UI 层无需知道底下拆了几个 crate）
pub use pv_cache::ThumbTier as Tier;
pub use pv_index::models::Image as ImageRow;
pub use pv_index::repo::query::{ImageFilter, ImageQuery, SortDir, SortKey};
/// 单目录打开的结果（`folder_id` + 图像增改删）
pub use pv_scan::OpenDir;

#[cfg(windows)]
pub use pv_scan::WindowsVolumeResolver;

/// 一张缩略图（RGBA8）。
pub struct Thumb {
    pub width: u32,
    pub height: u32,
    /// 长度 = width * height * 4
    pub rgba: Vec<u8>,
}

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

/// 侧栏「文件夹」树的一个节点（扁平存储，`parent_id` 表达层级）。
///
/// 为什么给**扁平表**而不是嵌套结构：前端要自己决定展开/折叠，
/// 树形结构在 React 里还得递归渲染，扁平表配合一个 `parent_id → children` 索引更好用。
pub struct FolderNode {
    pub id: i64,
    /// 父文件夹 id（`None` = 顶层，即某个卷的根目录）
    pub parent_id: Option<i64>,
    /// 显示名（取绝对路径最后一段；根目录取卷标签或盘符）
    pub name: String,
    /// 相对卷根的路径，`/` 分隔（根为 `""`）
    pub rel_path: String,
    /// 该文件夹**直属**图像数（不含子目录）
    pub image_count: i64,
    pub is_root: bool,
    pub volume_id: i64,
    /// 卷标签；没有标签时回落成挂载点（`C:\`）
    pub volume_label: String,
    /// 卷当前挂载点；`None` = 卷未挂载（离线）
    pub mount_path: Option<String>,
}

/// 侧栏「图库」条目。
pub struct LibNode {
    pub id: i64,
    pub name: String,
    /// `normal` / `smart` / `encrypted`
    pub kind: String,
    /// 智能图库的规则原文（普通图库为 `None`）
    pub rule: Option<String>,
    pub count: i64,
}

/// 侧栏「标签」条目：标签 + 使用次数。
pub struct TagCount {
    pub id: i64,
    pub name: String,
    pub count: i64,
}

/// 侧栏「地点」条目：离线城市名 + 照片数。
pub struct PlaceCount {
    pub name: String,
    pub count: i64,
}

/// 单个格式的统计。
pub struct FormatStat {
    pub format: String,
    pub count: i64,
    pub bytes: i64,
}

/// 全库统计（状态栏 + 筛选条）。
pub struct Stats {
    pub total_count: i64,
    pub total_bytes: i64,
    /// 张数降序
    pub formats: Vec<FormatStat>,
}

/// 从图库移除一个目录的结果。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Removed {
    /// 删掉的目录行数（子树，含自身）
    pub folders: u64,
    /// 删掉的图像行数
    pub images: u64,
    /// 顺带清掉的缩略图缓存文件数
    pub thumbs: u64,
    /// 因为**卷未挂载**而没能清缓存的图片数（索引已删，缓存文件成了孤儿）
    pub thumbs_orphan: u64,
}

/// 所有缩略图档位 —— 删缓存要逐档试，因为**缓存键含档位**。
const ALL_TIERS: [ThumbTier; 4] = [
    ThumbTier::Small,
    ThumbTier::Medium,
    ThumbTier::Large,
    ThumbTier::Xlarge,
];

/// 服务层。
pub struct Store {
    conn: Connection,
    l1: L1,
    l2: L2,
    registry: DecoderRegistry,
    resolver: Box<dyn VolumeResolver>,
}

/// EXIF 方向归一化：只有 1~8 有意义，其余（0 / 9+ / 读不到）一律当 1（正常）。
///
/// 0 是坏 EXIF —— 主流软件也都不转，跟着「不转」才是和别的软件一致。
fn normalize_orientation(raw: Option<u16>) -> u16 {
    match raw {
        Some(v) if (1..=8).contains(&v) => v,
        _ => 1,
    }
}

fn default_registry() -> DecoderRegistry {
    let mut r = DecoderRegistry::new();
    r.register(Box::new(BitmapBackend));
    r.set_raw_pipeline(Box::new(EmbeddedPreviewPipeline));
    r
}

/// 文件夹在侧栏显示的名字。
///
/// 非根目录直接取 `rel_path` 最后一段；根目录没有「最后一段」可取，
/// 于是取挂载点的末段（`D:\Photos` → `Photos`），盘根（`C:\`）没有末段就
/// 回落成卷标签 / 盘符（`C:`）。
fn folder_display_name(mount: Option<&str>, rel_path: &str, fallback: &str) -> String {
    if !rel_path.is_empty() {
        return rel_path.rsplit('/').next().unwrap_or(rel_path).to_string();
    }
    let Some(m) = mount else {
        return fallback.to_string();
    };
    let trimmed = m.trim_end_matches(['\\', '/']);
    match Path::new(trimmed).file_name() {
        Some(n) => n.to_string_lossy().into_owned(),
        None => fallback.to_string(),
    }
}

impl Store {
    /// 打开（或新建）数据目录里的库与缩略图缓存。
    pub fn open(data_dir: &Path, resolver: Box<dyn VolumeResolver>) -> Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let conn = index_conn::open(&data_dir.join("library.db"))?;
        migrate(&conn)?;
        Ok(Self {
            conn,
            l1: L1::new(L1::DEFAULT_BUDGET),
            l2: L2::new(data_dir.join("thumbs")),
            registry: default_registry(),
            resolver,
        })
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// L1 当前条目数（测试/诊断用）。
    pub fn l1_len(&self) -> usize {
        self.l1.len()
    }

    /// 清空内存缓存（锁定加密图库前必须调用）。
    pub fn clear_memory(&self) {
        self.l1.clear();
    }

    /// 磁盘缩略图缓存占用：`(字节数, 文件数)`。
    ///
    /// 设置面板要显示「缓存占了多少」—— 用户看不懂「缓存目录」，但看得懂 MB。
    pub fn cache_usage(&self) -> (u64, usize) {
        let bytes = self.l2.total_bytes().unwrap_or(0);
        (bytes, self.l2.count_files())
    }

    /// 清空磁盘缩略图缓存，返回**释放的字节数**。
    ///
    /// ⚠️ 只删 `thumbs/` 下的缓存文件，**不碰原图、不碰索引**。
    /// 内存缓存也一起清（否则刚清完又立刻回写一遍旧的）。
    /// 清完以后缩略图会在下次浏览时重新生成（首屏会慢一点，这是预期代价）。
    pub fn clear_cache(&self) -> u64 {
        let freed = self.cache_usage().0;
        self.l1.clear();
        let _ = self.l2.clear_all();
        freed
    }

    /// 递归首扫：把目录树建成 `folders` + 图像。
    ///
    /// ⚠️ **UI 不要用它来「打开一个目录」** —— 那会遍历整棵子树。
    /// 文件浏览器点目录走 [`Store::open_folder`]（单层）。
    pub fn scan_tree(&mut self, root: &Path) -> Result<TreeSummary> {
        Ok(pv_scan::scan_tree(&mut self.conn, self.resolver.as_ref(), root)?)
    }

    /// 打开目录 **并递归包含全部子目录**（用户勾了「包含子文件夹」时用）。
    ///
    /// ## 与 [`Store::open_folder`] 的区别
    /// 那个只登记并扫描**单层**，用于「点一下目录看一眼」；
    /// 这个会走 `scan_tree` 把整棵子树建进 `folders` 并逐目录扫图。
    ///
    /// ## 为什么要逐个目录补尺寸/时间
    /// `scan_tree` 只做魔数探测（不知道像素尺寸、不读 EXIF）。只补根目录的话，
    /// **子目录里的图全都没宽高**：卡片比例退回默认、按分辨率排序全是 0、
    /// 「宽 × 高」标签也空着 —— 而且不会报错，只有用下去才发现不对。
    ///
    /// ⚠️ 目录数超 [`pv_scan::tree::MAX_TREE_DIRS`] 会直接报错（不静默截断）。
    pub fn open_folder_tree(&mut self, dir: &Path) -> Result<pv_scan::TreeSummary> {
        let t = pv_scan::scan_tree(&mut self.conn, self.resolver.as_ref(), dir)?;
        for id in &t.folder_ids {
            self.fill_folder_dimensions(*id);
            self.fill_folder_times(*id);
        }
        Ok(t)
    }

    /// 打开**单个目录**（不递归）：登记卷与目录行、扫描**该目录里**的图片。
    ///
    /// 返回的 `folder_id` 就是「只看这个目录」的查询条件
    /// （`ImageFilter { dir_ids: vec![folder_id] }`）。
    pub fn open_folder(&mut self, dir: &Path) -> Result<OpenDir> {
        let out = pv_scan::open_dir(&mut self.conn, self.resolver.as_ref(), dir)?;
        // 扫完立刻把这一目录里**还没尺寸**的行补上：新扫进来的行本来就没有
        // （`walk_dir` 只做魔数探测），老库迁移来的行也没有。
        // 放在这里而不是扫描层，是因为探测要 pv-decode 的注册表 —— 分层上
        // 扫描层只管「磁盘上有什么」，解码层才知道「这张图多大」。
        self.fill_folder_dimensions(out.folder_id);
        // 拍摄时间同理：`walk_dir` 只读魔数，没有 EXIF 时间。
        // 没有它，「按拍摄时间排序」会静默退化成「按入库时间」（曾经就是坏的）。
        self.fill_folder_times(out.folder_id);
        Ok(out)
    }

    /// 用解码注册表探一个文件的**显示尺寸**和 EXIF 方向；失败/不支持返回 `None`。
    ///
    /// ⭐ 2026-10-30：返回的是**摆正之后**的宽高（6/8 已互换）。
    /// 库里 `width/height` 存显示尺寸，网格比例、卡片上的「宽×高」、
    /// 裁剪框算法全按它算 —— 存原始像素会让竖拍照片的长宽比整个反掉。
    fn probe_dims(&self, abs: &Path) -> Option<(i64, i64, u16)> {
        let (w, h) = pv_decode::probe_dimensions(&self.registry, abs).ok()?;
        let (w, h) = (i64::from(w), i64::from(h));
        // 读不到 / 值非法（0、>8）都归一成 1：
        // 0 是坏 EXIF，别的软件也不会转它，跟着「不转」才是对的。
        // 同时也把「已检查过」这个事实记下来，别每次重读。
        let o = normalize_orientation(pv_meta::read_orientation(abs));
        if matches!(o, 6 | 8) {
            Some((h, w, o))
        } else {
            Some((w, h, o))
        }
    }

    /// 把一批 id 的缺失尺寸补上。返回补上的数量。
    ///
    /// ⛔ **RAW 一律跳过**：RAW 的探测要把整个文件读进来找内嵌 JPEG 预览
    /// （`MAX_SCAN_BYTES` 128MB，一张 ARW 平均 47MB），一个 600 张 RAW 的目录
    /// 就是 ~28GB 读取 —— 加入图库时会**卡死几分钟**。而且内嵌预览的尺寸
    /// 并不是传感器尺寸，显示出来是误导。
    ///
    /// 探测失败只有两种：卷没挂载（拿不到绝对路径）、格式探不出 ——
    /// 两种都**留空下次再试**，不能瞎写个 0 进去。
    fn fill_dims_for(&mut self, ids: &[i64]) -> usize {
        let mut filled = 0usize;
        for id in ids {
            let img = match repo::images::get(&self.conn, *id) {
                Ok(Some(i)) => i,
                _ => continue,
            };
            if img.is_raw {
                continue;
            }
            let abs = match self.abs_path(&img) {
                Ok(Some(p)) => p,
                _ => continue,
            };
            if let Some((w, h, orientation)) = self.probe_dims(&abs) {
                if repo::images::update_dimensions(&self.conn, *id, w, h).is_ok() {
                    // 方向必须和宽高**同一次**写入：两者来自同一次探测，
                    // 分开写就会出现「宽高换了、方向还是 NULL」的中间态，
                    // 下次回填又会换一遍 → 宽高反复横跳。
                    let _ = repo::images::set_orientation(&self.conn, *id, orientation);
                    filled += 1;
                }
            }
        }
        filled
    }

    /// 补齐某个目录里缺失的尺寸 / 方向。返回补上的数量。
    ///
    /// ⚠️ 判据是「尺寸缺失 **或** 方向没读过」：老库的行尺寸早填过（但按原始像素），
    /// 方向却还是 NULL —— 只看尺寸的话，重扫这个目录也不会把它们纠正过来
    /// （2026-10-30 实测：重启前 7 行一直是 NULL）。
    pub fn fill_folder_dimensions(&mut self, dir_id: i64) -> usize {
        let Ok(rows) = repo::images::list_by_dir(&self.conn, dir_id) else {
            return 0;
        };
        let mut ids: Vec<i64> = rows
            .into_iter()
            .filter(|r| r.width.is_none() || r.height.is_none())
            .map(|r| r.id)
            .collect();
        if let Ok(extra) = repo::images::list_ids_in_dir_missing_orientation(&self.conn, dir_id) {
            ids.extend(extra);
        }
        ids.sort_unstable();
        ids.dedup();
        if ids.is_empty() {
            return 0;
        }
        self.fill_dims_for(&ids)
    }

    /// **全局回填**缺尺寸的行（老库升级用）。
    ///
    /// `limit` 防止大库首启卡太久 —— 没补完的下次启动接着补
    /// （`width IS NULL` 的行会越来越少，最终一轮为空、零开销）。
    /// 返回 (本次补上的数量, 还剩多少没补)。
    pub fn backfill_dimensions(&mut self, limit: i64) -> (usize, i64) {
        let Ok(ids) = repo::images::list_ids_missing_dimensions(&self.conn, limit) else {
            return (0, 0);
        };
        if ids.is_empty() {
            return (0, repo::images::count_missing_dimensions(&self.conn).unwrap_or(0));
        }
        let filled = self.fill_dims_for(&ids);
        let left = repo::images::count_missing_dimensions(&self.conn).unwrap_or(0);
        (filled, left)
    }

    /// 老库升级：补 EXIF 方向，并把 6/8 那批**当年按原始像素存的宽高换回来**。
    ///
    /// ## 为什么不能只靠 `backfill_dimensions`
    /// 那个只看 `width IS NULL` 的行。老库的宽高早填过了（但填的是原始像素），
    /// 所以竖拍照片一直是横的、卡片上的长宽比也是反的（2026-10-30 修的）。
    /// 这里以 `orientation IS NULL` 为「没处理过」的判据，一次过。
    ///
    /// 幂等：处理完的行方向不再是 NULL，下次不再进来。
    /// `limit` 控制单次量（1 万张的库首启别卡太久），没补完的下次接着补。
    ///
    /// 返回 (本次处理的行数, 还剩多少)。
    pub fn backfill_orientation(&mut self, limit: i64) -> (usize, i64) {
        let Ok(ids) = repo::images::list_ids_missing_orientation(&self.conn, limit) else {
            return (0, 0);
        };
        if ids.is_empty() {
            return (
                0,
                repo::images::count_missing_orientation(&self.conn).unwrap_or(0),
            );
        }
        let mut done = 0usize;
        for id in ids {
            let img = match repo::images::get(&self.conn, id) {
                Ok(Some(i)) => i,
                _ => continue,
            };
            // 卷没挂载 → 拿不到路径，留 NULL 等下次（不能瞎写个 1 糊过去）
            let Ok(Some(abs)) = self.abs_path(&img) else {
                continue;
            };
            let o = normalize_orientation(pv_meta::read_orientation(&abs));
            if matches!(o, 6 | 8) {
                if let (Some(w), Some(h)) = (img.width, img.height) {
                    let _ = repo::images::update_dimensions(&self.conn, id, h, w);
                }
            }
            if repo::images::set_orientation(&self.conn, id, o).is_ok() {
                done += 1;
            }
        }
        let left = repo::images::count_missing_orientation(&self.conn).unwrap_or(0);
        (done, left)
    }

    /// 给一批 id 补**拍摄时间**（EXIF `DateTimeOriginal` → Unix 秒）。返回**读到时间**的数量。
    ///
    /// ⚠️ 读不到也照样标记 `meta_checked = 1`：不然「本来就没有 EXIF 的图」
    /// 每次启动都会被重新读一遍，一万张的库就是几十秒的纯浪费。
    ///
    /// ⚠️ 拿不到绝对路径（卷没挂载）时**不标记**，等下次挂载再补。
    fn fill_times_for(&mut self, ids: &[i64]) -> usize {
        let mut filled = 0usize;
        for id in ids {
            let img = match repo::images::get(&self.conn, *id) {
                Ok(Some(i)) => i,
                _ => continue,
            };
            if img.taken_at.is_some() {
                // 已经有时间（用户改过 / 之前读过），不用再读 EXIF
                let _ = repo::images::set_taken_at(&self.conn, *id, None);
                continue;
            }
            let Ok(Some(abs)) = self.abs_path(&img) else {
                continue;
            };
            let taken = pv_meta::read_exif(&abs).and_then(|e| e.taken_at_unix());
            if repo::images::set_taken_at(&self.conn, *id, taken).is_ok() && taken.is_some() {
                filled += 1;
            }
        }
        filled
    }

    /// 把一个目录里**还没有拍摄时间**的行补上（入库扫完之后调用）。
    ///
    /// ⚠️ 代价：每张要 `open` 一次读 EXIF（JPEG 只读前 64KB，RAW 只读文件头）。
    /// 一个 1200 张的目录在机械盘上要多花几秒 —— 入库本来就是个慢动作（有进度条），
    /// 但**浏览路径绝不能做这件事**（那是另一条不写索引的路）。
    pub fn fill_folder_times(&mut self, dir_id: i64) -> usize {
        let Ok(ids) = repo::images::list_ids_missing_taken_in_dir(&self.conn, dir_id) else {
            return 0;
        };
        if ids.is_empty() {
            return 0;
        }
        self.fill_times_for(&ids)
    }

    /// 启动时回填老库缺失的拍摄时间（分批，`limit` 控制一轮做多少）。
    ///
    /// 全补完后每次启动只剩一条 `COUNT` 查询，几乎零开销。
    pub fn backfill_times(&mut self, limit: i64) -> (usize, i64) {
        let Ok(ids) = repo::images::list_ids_missing_taken(&self.conn, limit) else {
            return (0, 0);
        };
        if ids.is_empty() {
            return (0, repo::images::count_missing_taken(&self.conn).unwrap_or(0));
        }
        let filled = self.fill_times_for(&ids);
        let left = repo::images::count_missing_taken(&self.conn).unwrap_or(0);
        (filled, left)
    }

    /// 把一个**已在索引里**的目录归入图库，返回图库 id。
    ///
    /// - `library_id = Some(id)` → 挂到该图库
    /// - `library_id = None` → 按 `name` **查同名图库：有就挂上去，没有才新建**
    ///
    /// ⭐ 同名复用这条很要紧：否则「用文件夹名加入」点两次会得到两个同名图库，
    /// 侧栏里两行长得一模一样，用户根本分不清哪个是哪个。
    ///
    /// ⚠️ 调用前必须先 `open_folder` 把目录扫进索引 —— 目录不在 `folders` 表里
    /// 就挂不上（`library_folders.folder_id` 指着它）。
    ///
    /// `recursive = true` → 连同**全部后代目录**一起挂（用户答了「包含子文件夹」）。
    /// ⛔ 只挂根目录是不够的：图库查询按 `library_folders` 里的 id 精确匹配，
    /// 子目录不在里面就一张都查不到。
    ///
    /// 幂等：重复挂同一个目录不会报错（`INSERT OR IGNORE`）。
    pub fn add_to_library(
        &mut self,
        folder_id: i64,
        name: &str,
        library_id: Option<i64>,
        recursive: bool,
    ) -> Result<i64> {
        /*
         * ⭐ 递归入库必须把**整棵子树**都挂进图库（2026-10-31）。
         *
         * 只挂根目录的话，图库查询走的是
         * `dir_id IN (SELECT folder_id FROM library_folders WHERE library_id=?)`
         * —— 子目录一张都查不出来。表现就是「点了包含子文件夹、状态栏说加了 300 张，
         * 进图库只看到 5 张」，用户会以为扫描没生效。
         */
        let ids = if recursive {
            repo::folders::descendant_ids(&self.conn, folder_id)?
        } else {
            vec![folder_id]
        };

        match library_id {
            Some(id) => {
                for f in &ids {
                    repo::libraries::add_folder(&self.conn, id, *f)?;
                }
                Ok(id)
            }
            None => {
                let existing = repo::libraries::list(&self.conn)?
                    .into_iter()
                    .find(|l| l.name == name);
                match existing {
                    Some(l) => {
                        for f in &ids {
                            repo::libraries::add_folder(&self.conn, l.id, *f)?;
                        }
                        Ok(l.id)
                    }
                    None => Ok(pv_lib::library::create_normal(&self.conn, name, &ids)?),
                }
            }
        }
    }

    /// 新建一个**空图库**（不绑定任何目录）。
    ///
    /// ⚠️ `libraries.name` **没有 UNIQUE 约束** —— 同名会静默插入两条，
    /// 侧栏上就是两个长得一样的行，用户没法分辨。这里主动挡住。
    ///
    /// 空库建成后 `library_folders` 没有行 → 查出来就是 0 张，
    /// 用户可以之后再往里加目录（或者直接拖图进来，后续功能）。
    pub fn create_library(&mut self, name: &str) -> Result<i64> {
        let name = name.trim();
        if name.is_empty() {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "图库名不能为空",
            )));
        }
        let dup = repo::libraries::list(&self.conn)?
            .into_iter()
            .any(|l| l.name == name);
        if dup {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "已经有同名图库了",
            )));
        }
        Ok(pv_lib::library::create_normal(&self.conn, name, &[])?)
    }

    /// 改图库名（**不动成员关系、不动任何文件、不动缓存**）。
    ///
    /// ⚠️ 同样要挡重名：`libraries.name` 没有 UNIQUE 约束。
    /// 改成自己现在的名字不算冲突（用户点了确认但没改字，不该报错）。
    pub fn rename_library(&mut self, id: i64, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "图库名不能为空",
            )));
        }
        let dup = repo::libraries::list(&self.conn)?
            .into_iter()
            .any(|l| l.id != id && l.name == name);
        if dup {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "已经有同名图库了",
            )));
        }
        Ok(repo::libraries::rename(&self.conn, id, name)?)
    }

    /// 新建一个**智能图库**：规则文本按 `pv-lib` 的 DSL 存进 `libraries.rule`。
    ///
    /// 规则写法（空格 = AND）：
    /// `tag:风景` / `rating:>=4` / `type:raw` / `ext:jpg` / `date:2024` / `date:2024-05`
    /// / 裸词 → 全文关键词。
    ///
    /// ⚠️ 规则**必须先解析校验再落库**：语法错的话建出来是个永远空的图库，
    /// 界面上完全看不出哪儿错了（`create_smart` 内部已经 `parse_rule` 过一次）。
    /// ⚠️ 同样挡重名。
    pub fn create_smart_library(&mut self, name: &str, rule: &str) -> Result<i64> {
        let name = name.trim();
        if name.is_empty() {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "图库名不能为空",
            )));
        }
        let rule = rule.trim();
        if rule.is_empty() {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "智能图库的规则不能为空",
            )));
        }
        let dup = repo::libraries::list(&self.conn)?
            .into_iter()
            .any(|l| l.name == name);
        if dup {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "已经有同名图库了",
            )));
        }
        Ok(pv_lib::library::create_smart(&self.conn, name, rule)?)
    }

    /// 按图库取图：**普通库走 `library_folders` JOIN，智能库走规则求值**。
    ///
    /// 为什么不能在 SQL 里一把梭：智能库没有成员目录行，`library_id` 的 JOIN
    /// 结果恒为空 —— 必须先去 `pv-lib` 把规则翻成 `ImageFilter`，再查。
    ///
    /// ⭐ 界面上的**格式筛选 / 文件名搜索**会叠加到规则之上（规则本身的条件不放松）：
    /// 用户在智能库里再点「只看 PNG」是合理预期。
    ///
    /// ⚠️ 规则里含**全文关键词**时走 `pv-lib` 的 id 列表（FTS 命中集），
    /// 此时排序/分页仍由调用方决定不了 —— 按 `taken_at` 倒序返回。
    pub fn query_in_library(&self, q: &ImageQuery, library_id: i64) -> Result<Vec<Image>> {
        let Some(lib) = repo::libraries::get(&self.conn, library_id)? else {
            return Ok(Vec::new());
        };
        if lib.kind != pv_index::models::LibraryKind::Smart {
            let mut q2 = q.clone();
            q2.filter.library_id = Some(library_id);
            return Ok(repo::query::list(&self.conn, &q2)?);
        }

        let rule = pv_lib::parse_rule(lib.rule.as_deref().unwrap_or_default())?;
        if !rule.keywords.is_empty() {
            let ids = pv_lib::library::members(&self.conn, library_id, 0, 0)?;
            return self.images_by_ids(&ids);
        }

        let mut filter = rule.filter;
        if !q.filter.formats.is_empty() {
            filter.formats = q.filter.formats.clone();
        }
        if q.filter.name_like.is_some() {
            filter.name_like = q.filter.name_like.clone();
        }
        let q2 = ImageQuery {
            filter,
            sort: q.sort,
            dir: q.dir,
            limit: q.limit,
            offset: q.offset,
        };
        Ok(repo::query::list(&self.conn, &q2)?)
    }

    /// 图库命中数（智能库按规则算，普通库按 JOIN 算）。
    ///
    /// ⚠️ 智能库**不叠加界面筛选**：侧栏上那个数字是「这个图库一共多少张」，
    /// 不该因为主区点了「只看 PNG」就变。
    pub fn count_in_library(&self, f: &ImageFilter, library_id: i64) -> Result<i64> {
        let Some(lib) = repo::libraries::get(&self.conn, library_id)? else {
            return Ok(0);
        };
        if lib.kind == pv_index::models::LibraryKind::Smart {
            return Ok(pv_lib::library::count_of(&self.conn, &lib)?);
        }
        let mut f2 = f.clone();
        f2.library_id = Some(library_id);
        Ok(repo::query::count(&self.conn, &f2)?)
    }

    /// 删除图库：**连它独占的目录一起从索引里清掉**。
    ///
    /// ## 为什么不再「只删分组」（2026-10-31 改）
    /// 旧语义是「删库 ≠ 删照片」，但那样删完图库后，那些照片仍然挂在
    /// 「全部照片」里（用户反馈：图库删了，全部照片也得清）。
    ///
    /// ## 什么才真的删
    /// 只删**没有别的图库引用**的目录（整棵子树）。同一个目录同时挂在两个库里时，
    /// 删掉 A 库不动它的索引 —— B 库的照片必须还在。
    ///
    /// ⚠️ 删索引会连带删掉这些图的**评分、标签、人脸关联**（都挂在 image_id 上）。
    /// 重扫只能恢复「有哪些图」，恢复不了这些 —— 所以 UI 侧必须先确认。
    ///
    /// ⚠️ **绝不碰磁盘上的原图**，只删索引行 + 缩略图缓存。
    ///
    /// 幂等：id 不存在时返回 `None`，不报错。
    pub fn delete_library(&mut self, id: i64) -> Result<Option<Removed>> {
        if repo::libraries::get(&self.conn, id)?.is_none() {
            return Ok(None);
        }

        // ① 先记下这个库挂了哪些目录 —— 删完库行就查不到了
        let folders = repo::libraries::list_folder_ids(&self.conn, id)?;
        repo::libraries::delete(&self.conn, id)?;

        /*
         * ② 再逐个判断「能不能连索引一起清掉」。
         *
         * ⚠️ **必须在删库之后判**：自己的引用还留着的话 `count_refs_in_subtree`
         *    永远 > 0，一个目录都删不掉。
         * ⚠️ 子树里只要还有**别的**库引用着，就整个跳过：`remove_folder` 删的是
         *    整棵子树，放过去就是把另一个图库的照片一起清了。
         * ⚠️ `remove_folder` 之后该子树的行已经没了，遍历里要能容忍
         *    「目录不存在」（同一个库挂了父子两层时会重复命中）。
         */
        let mut out = Removed::default();
        for fid in folders {
            if repo::folders::get(&self.conn, fid)?.is_none() {
                continue;
            }
            if repo::libraries::count_refs_in_subtree(&self.conn, fid)? > 0 {
                continue;
            }
            let r = self.remove_folder(fid)?;
            out.folders += r.folders;
            out.images += r.images;
            out.thumbs += r.thumbs;
            out.thumbs_orphan += r.thumbs_orphan;
        }
        Ok(Some(out))
    }

    /// 从图库移除一个目录**及其整棵子树**：删索引行 + 顺带清掉它们的缩略图缓存。
    ///
    /// ⚠️ **只动索引与缓存，绝不碰用户的原始图片文件。**
    ///
    /// ## 顺序是关键
    /// 必须**先按索引算出缓存文件路径并删掉，再删索引行**：
    /// 缓存键 = 规范化路径 + size + mtime，索引行一没，`abs_path`/`cache_file`
    /// 就都算不出来了 —— 那些缓存文件会永远变成没人认领的孤儿，且再也无法定位。
    ///
    /// 卷**未挂载**时算不出缓存键（挂载点是缓存键的一部分），这些图的缓存
    /// 只能留着，计入 `thumbs_orphan`。**不能猜路径** —— 猜错会删掉别人的缩略图。
    ///
    /// 幂等：目录不在库里时返回全 0，不报错（重复点「移除」不该弹错）。
    pub fn remove_folder(&mut self, folder_id: i64) -> Result<Removed> {
        let mut out = Removed::default();
        let Some(root) = repo::folders::get(&self.conn, folder_id)? else {
            return Ok(out);
        };

        // 1) 收集子树（含自身）。DFS 的产出是「父在子前」，删的时候反序即可。
        let all = repo::folders::list_by_volume(&self.conn, root.volume_id)?;
        let mut children: std::collections::HashMap<i64, Vec<i64>> =
            std::collections::HashMap::new();
        for f in &all {
            if let Some(p) = f.parent_id {
                children.entry(p).or_default().push(f.id);
            }
        }
        let mut dirs = vec![folder_id];
        let mut stack = vec![folder_id];
        while let Some(id) = stack.pop() {
            if let Some(kids) = children.get(&id) {
                for k in kids {
                    dirs.push(*k);
                    stack.push(*k);
                }
            }
        }

        /*
         * 2) 此刻索引还在，先把**缓存键**算出来（键含绝对路径 + size + mtime，
         *    索引行一没就再也算不出来了）。⚠️ 只算不删 —— 删要等事务提交之后。
         *
         * ⛔ 以前是「边算边删」：万一下面的事务失败回滚，索引行还在、照片还在，
         *    缩略图却被删了 —— 用户看到一整片空白卡片，还以为图坏了。
         *    缓存删早了没有回滚，索引删错了却能回滚，所以顺序只能是「先提交、后删缓存」。
         */
        let mut victims: Vec<i64> = Vec::new();
        let mut cache_keys = Vec::new();
        for dir in &dirs {
            for img in repo::images::list_by_dir(&self.conn, *dir)? {
                victims.push(img.id);
                match self.abs_path(&img)? {
                    Some(abs) => {
                        for tier in ALL_TIERS {
                            cache_keys.push(self.cache_key(&img, &abs, tier));
                        }
                    }
                    None => out.thumbs_orphan += 1,
                }
            }
        }
        out.images = victims.len() as u64;
        out.folders = dirs.len() as u64;

        // 3) 删索引行。`images::delete` 会连带清 thumbs / image_tags / ai_results /
        //    image_exif / faces / fts_images —— 所以必须走它，不能自己写 DELETE。
        index_conn::with_tx(&mut self.conn, |tx| {
            for id in &victims {
                repo::images::delete(tx, *id)?;
            }
            // 脸被删了，人物的 face_count / 封面得跟着重算（空人物一并清掉），
            // 否则人像条会停在「457 张脸」而实际只剩几百张，封面还可能指向已删的脸。
            repo::persons::refresh_all(tx)?;
            // 反序：先子后父（`folders.parent_id` 指着父行）
            for id in dirs.iter().rev() {
                // ⚠️ 先断图库成员关系再删目录行：`library_folders` 是 NO ACTION 外键且
                //    SQLite 默认不开 foreign_keys，不显式删就留下指向已删目录的脏行。
                repo::libraries::detach_folder(tx, *id)?;
                repo::folders::delete(tx, *id)?;
            }
            Ok(())
        })?;

        /*
         * 4) 事务已提交 —— 现在删缓存才是安全的。删不掉只是浪费几 MB 磁盘，
         *    不影响正确性，所以计数归计数、不报错。
         */
        for key in &cache_keys {
            if self.l2.remove(key).unwrap_or(false) {
                out.thumbs += 1;
            }
        }

        // 5) L1 里还留着刚删掉的位图，清掉（纯内存，清了不影响别的）
        self.l1.clear();
        Ok(out)
    }

    /// 重扫单个已索引文件夹。
    pub fn scan_folder(&mut self, dir_id: i64, abs_dir: &Path) -> Result<Summary> {
        Ok(pv_scan::scan_folder(&mut self.conn, dir_id, abs_dir)?)
    }

    /// 重命名单个已索引的图片文件，并同步索引与缩略图缓存。
    ///
    /// ## 顺序
    /// 1. 解析绝对路径（卷未挂载 → 报错，无从改名）
    /// 2. 目标已存在 → 报错（**绝不覆盖**，重名会让用户丢文件）
    /// 3. **先删旧缓存**（缓存键含路径，改完名就算不出旧键了）
    /// 4. `fs::rename`
    /// 5. 更新索引 `rel_name`（`content_key` 目前未被写入，原样保留）
    ///
    /// 返回新文件名（调用方拼提示用）。
    pub fn rename_image(&mut self, id: i64, new_name: &str) -> Result<String> {
        let Some(img) = repo::images::get(&self.conn, id)? else {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "索引里没有这张图",
            )));
        };
        let Some(old_abs) = self.abs_path(&img)? else {
            return Err(StoreError::Io(std::io::Error::other(
                "所在卷未挂载，无法重命名",
            )));
        };
        let Some(parent) = old_abs.parent() else {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "路径没有父目录",
            )));
        };
        let new_abs = parent.join(new_name);
        if new_name.is_empty() || new_name.contains(['\\', '/', ':', '*', '?', '"', '<', '>', '|'])
        {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "文件名里有非法字符",
            )));
        }
        if new_abs.exists() {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "目标文件名已存在",
            )));
        }

        // 旧缓存必须在改名**之前**清（键里含旧路径）
        for tier in ALL_TIERS {
            let key = self.cache_key(&img, &old_abs, tier);
            let _ = self.l2.remove(&key);
        }

        std::fs::rename(&old_abs, &new_abs)?;
        repo::images::rename(&self.conn, id, new_name, img.content_key.as_deref())?;
        // L1 里的是「旧路径那批」位图，清掉避免与新的混淆
        self.l1.clear();
        Ok(new_name.to_string())
    }

    /// 从图库移除**指定的几张图**（删索引 + 删缩略图缓存）。
    ///
    /// ⚠️ **绝不碰原图**：调用方负责先把文件删到回收站，这里只清索引与缓存。
    /// ⚠️ 顺序同 `remove_folder`：**先算缓存路径删缓存、再删索引行**
    /// （缓存键含 `content_key`，索引一没就算不出来了）。
    pub fn remove_images(&mut self, ids: &[i64]) -> Result<Removed> {
        let mut out = Removed::default();
        let mut victims: Vec<i64> = Vec::new();

        for id in ids {
            let Some(img) = repo::images::get(&self.conn, *id)? else {
                continue; // 已经不在了，幂等跳过
            };
            victims.push(img.id);
            match self.abs_path(&img)? {
                Some(abs) => {
                    for tier in ALL_TIERS {
                        let key = self.cache_key(&img, &abs, tier);
                        if self.l2.remove(&key).unwrap_or(false) {
                            out.thumbs += 1;
                        }
                    }
                }
                None => out.thumbs_orphan += 1,
            }
        }

        out.images = victims.len() as u64;
        index_conn::with_tx(&mut self.conn, |tx| {
            for id in &victims {
                repo::images::delete(tx, *id)?;
            }
            Ok(())
        })?;

        // L1 里还留着刚删掉的位图（纯内存，清了不影响别的）
        self.l1.clear();
        Ok(out)
    }

    /// 组合查询（过滤/排序/分页）。
    pub fn query(&self, q: &ImageQuery) -> Result<Vec<Image>> {
        Ok(repo::query::list(&self.conn, q)?)
    }

    /// 命中数量。
    pub fn count(&self, f: &ImageFilter) -> Result<i64> {
        Ok(repo::query::count(&self.conn, f)?)
    }

    /// 侧栏用：全部文件夹节点（按卷 → 相对路径排序）。
    ///
    /// 计数走一条 `GROUP BY dir_id`（不是逐个 `count_by_dir`）——文件夹上千时
    /// N+1 会变成上千次查询。
    pub fn folders(&self) -> Result<Vec<FolderNode>> {
        let counts: std::collections::HashMap<i64, i64> =
            repo::images::counts_by_dir(&self.conn)?.into_iter().collect();

        let mut out = Vec::new();
        for vol in repo::volumes::list(&self.conn)? {
            let mount = vol.last_mount_path.clone();
            for f in repo::folders::list_by_volume(&self.conn, vol.id)? {
                let volume_label = vol
                    .label
                    .clone()
                    .filter(|s| !s.trim().is_empty())
                    .or_else(|| mount.clone())
                    .unwrap_or_else(|| vol.volume_guid.clone());
                out.push(FolderNode {
                    id: f.id,
                    parent_id: f.parent_id,
                    name: folder_display_name(mount.as_deref(), &f.rel_path, &volume_label),
                    rel_path: f.rel_path.clone(),
                    image_count: counts.get(&f.id).copied().unwrap_or(0),
                    is_root: f.is_root,
                    volume_id: vol.id,
                    volume_label,
                    mount_path: mount.clone(),
                });
            }
        }
        // `!is_root`：假(0) 排在真(1) 前面 → **根目录在前**，其余按相对路径
        out.sort_by(|a, b| {
            (a.volume_id, !a.is_root, &a.rel_path).cmp(&(b.volume_id, !b.is_root, &b.rel_path))
        });
        Ok(out)
    }

    /// 侧栏用：全部图库 + 各自条目数。
    pub fn libraries(&self) -> Result<Vec<LibNode>> {
        Ok(pv_lib::library::list_summaries(&self.conn)?
            .into_iter()
            .map(|s| LibNode {
                id: s.library.id,
                name: s.library.name,
                kind: s.library.kind.as_str().to_string(),
                rule: s.library.rule,
                count: s.count,
            })
            .collect())
    }

    /// 侧栏图库树的**二级**用：全部图库的成员关系 `(library_id, folder_id)`。
    /// 前端拿它和 `folders()` 按 folder id 拼装（目录名 / 绝对路径 / 张数都在那边）。
    pub fn library_members(&self) -> Result<Vec<(i64, i64)>> {
        Ok(repo::libraries::list_members(&self.conn)?)
    }

    /// 批量设置星级（0–5）。**只改索引**，不动原图、不动缓存。
    pub fn set_rating(&self, ids: &[i64], rating: i64) -> Result<usize> {
        Ok(repo::images::set_rating(&self.conn, ids, rating)?)
    }

    /// 批量设置旗标（0 = 无，1 = 保留）。**只改索引**。
    pub fn set_flag(&self, ids: &[i64], flag: i64) -> Result<usize> {
        Ok(repo::images::set_flag(&self.conn, ids, flag)?)
    }

    /* ── 标签（2026-10-19）────────────────────────────────────────
     * 标签只是索引里的元数据：**改标签不碰文件、不碰缓存**。
     * 手工标签的来源统一记 `source = 'user'`，将来 AI 打的标签记别的来源，
     * 两者在 `image_tags` 里共存，用户手动移除时一起撤（`remove_any`）。
     */

    /// 给一批图**加**同名标签（不存在则新建）。返回建立的挂载数。
    ///
    /// 幂等：同一张图重复加同一个标签不会报错（`INSERT OR REPLACE`）。
    pub fn add_tags(&mut self, ids: &[i64], names: &[String]) -> Result<usize> {
        let names: Vec<&str> = names
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if ids.is_empty() || names.is_empty() {
            return Ok(0);
        }
        let mut n = 0usize;
        for name in names {
            let tag_id = repo::tags::ensure(&self.conn, name, Some("user"))?;
            for id in ids {
                repo::tags::add_image_tag(&self.conn, *id, tag_id, "user", None)?;
                n += 1;
            }
        }
        Ok(n)
    }

    /// 在一批图里挑出「明显不是人像」的（人脸检测可跳过，省时间）。
    ///
    /// 判据与取舍见 [`repo::tags::non_portrait_ids`] —— 只对**打过标**的图做减法，
    /// 没标签的图不在此列（照常检测），所以这不构成对打标的硬依赖。
    pub fn non_portrait_ids(
        &self,
        ids: &[i64],
    ) -> Result<std::collections::HashSet<i64>> {
        Ok(repo::tags::non_portrait_ids(&self.conn, ids)?
            .into_iter()
            .collect())
    }

    /// 一批图里「打过 AI 标签」和「已有人脸」的图数，返回 `(tagged, faced)`。
    pub fn ai_coverage(&self, ids: &[i64]) -> Result<(usize, usize)> {
        Ok((
            repo::tags::count_tagged(&self.conn, ids)?,
            repo::faces::count_images(&self.conn, ids)?,
        ))
    }

    /// **覆盖**一张图的 AI 标签（先删该图所有 `source='ai'` 的挂载，再写新的）。
    ///
    /// ⭐ 用「按图覆盖」而不是「全库先删再写」：中途崩溃/断电时，
    /// 全库先删会让**所有**图的 AI 标签都没了，而按图覆盖最多丢失正在处理的那张。
    ///
    /// `tags` 里是 `(词条, 分数)`；分数写进 `confidence`，前端可用来排序/调试。
    pub fn replace_ai_tags(&mut self, image_id: i64, tags: &[(String, f32)]) -> Result<()> {
        repo::tags::clear_image_source(&self.conn, image_id, "ai")?;
        for (name, score) in tags {
            let tag_id = repo::tags::ensure(&self.conn, name, Some("ai"))?;
            repo::tags::add_image_tag(
                &self.conn,
                image_id,
                tag_id,
                "ai",
                Some(*score as f64),
            )?;
        }
        Ok(())
    }

    /// 清掉历史遗留的「一级类目」派生标签（幂等，见 `repo::tags::drop_redundant_l1`）。
    ///
    /// 只删「这张图已经有二级词」的那些一级标签；只有一级的图保留。
    /// `l1_names` 传空时是空操作。
    pub fn drop_redundant_l1_tags(&mut self, l1_names: &[String]) -> Result<usize> {
        Ok(repo::tags::drop_redundant_l1(&self.conn, l1_names)?)
    }

    /// 从一批图**移除**某个标签（按 id）。返回受影响的张数。
    pub fn remove_tag(&mut self, ids: &[i64], tag_id: i64) -> Result<usize> {
        let mut n = 0usize;
        for id in ids {
            repo::tags::remove_any(&self.conn, *id, tag_id)?;
            n += 1;
        }
        Ok(n)
    }

    /// 侧栏「标签」分区：全部标签 + 各自使用次数（多的在前）。
    pub fn tags(&self) -> Result<Vec<TagCount>> {
        Ok(repo::tags::list_with_counts(&self.conn)?
            .into_iter()
            .map(|(id, name, count)| TagCount { id, name, count })
            .collect())
    }

    /// 带范围的标签计数（图库 / 目录）。
    pub fn tags_scoped(
        &self,
        library_id: Option<i64>,
        dir_id: Option<i64>,
    ) -> Result<Vec<TagCount>> {
        Ok(repo::tags::counts_scoped(&self.conn, library_id, dir_id)?
            .into_iter()
            .map(|(id, name, count)| TagCount { id, name, count })
            .collect())
    }

    /// 一级类目的**去重**图片数（`(标签名, 一级类目)` 映射由调用方给）。
    ///
    /// ⚠️ 不能拿 `tags_scoped` 的二级计数相加代替 —— 一张图挂多个二级会被重复计数。
    pub fn tag_group_counts(
        &self,
        library_id: Option<i64>,
        dir_id: Option<i64>,
        pairs: &[(String, String)],
    ) -> Result<Vec<(String, i64)>> {
        Ok(repo::tags::group_counts(
            &self.conn,
            library_id,
            dir_id,
            pairs,
        )?)
    }

    /// 读一个设置项（不存在返回 `None`）。
    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(repo::settings::get(&self.conn, key)?)
    }

    /// 写一个设置项（覆盖）。
    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        Ok(repo::settings::set(&self.conn, key, value)?)
    }

    /// 单张图的全部标签：`(标签名, 来源, 置信度)`。卡片上「标签」按钮用。
    pub fn image_tags(&self, image_id: i64) -> Result<Vec<(String, String, Option<f64>)>> {
        Ok(repo::tags::list_by_image(&self.conn, image_id)?)
    }

    /// 一次性回填 `added_ts`（老库全 NULL）。用 settings 标记保证只跑一次。
    pub fn backfill_added_ts(&mut self) -> usize {
        const FIX_KEY: &str = "images.added_ts.v1";
        if repo::settings::get(&self.conn, FIX_KEY)
            .ok()
            .flatten()
            .is_some()
        {
            return 0;
        }
        let n = repo::images::backfill_added_ts(&self.conn).unwrap_or(0);
        let _ = repo::settings::set(&self.conn, FIX_KEY, "1");
        n
    }

    /// 库里出现过的拍摄年份及张数（顶部「年份」下拉用）。
    pub fn years(&self) -> Result<Vec<(i64, i64)>> {
        Ok(repo::query::list_years(&self.conn)?)
    }

    /// 库里出现过的相机机型及张数（顶部「相机」下拉用）。
    pub fn cameras(&self, limit: i64) -> Result<Vec<(String, i64)>> {
        Ok(repo::exif::list_models(&self.conn, limit)?)
    }

    /// 标签选择器用：这批图**各自**已有的标签（`name → 有几张带`）。
    pub fn tag_counts_for(&self, ids: &[i64]) -> Result<Vec<(String, i64)>> {
        Ok(repo::tags::counts_for_images(&self.conn, ids)?)
    }

    /// 标签改名。⚠️ 撞上同名标签要明确报「已有同名标签」，别让 UNIQUE 错误裸奔。
    pub fn rename_tag(&mut self, id: i64, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "标签名不能为空",
            )));
        }
        let dup = repo::tags::list_all(&self.conn)?
            .into_iter()
            .any(|t| t.id != id && t.name == name);
        if dup {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "已经有同名标签了",
            )));
        }
        Ok(repo::tags::rename(&self.conn, id, name)?)
    }

    /// 删除标签（拆掉所有挂载 + 删标签行）。**照片一张不动**。
    pub fn delete_tag(&mut self, id: i64) -> Result<()> {
        Ok(repo::tags::delete(&self.conn, id)?)
    }

    /* ── 地点（2026-10-19）────────────────────────────────────────
     * 坐标 → 城市名一律走 `pv-geo` 内置离线表（⛔ 绝不联网）。
     * 结果落在 `image_places`：**有行 = 已经解析过**（`place` 为 NULL = 附近没城市），
     * 所以没 GPS 的图不会被反复重算——它们压根不进待办列表。
     */

    /// 侧栏「地点」分区：城市名 + 各自张数（多的在前），**全库**。
    pub fn places(&self) -> Result<Vec<PlaceCount>> {
        self.places_scoped(None, None)
    }

    /// 侧栏「地点」分区（**限定范围**）。
    ///
    /// ⭐ 必须和 [`tags_scoped`](Self::tags_scoped) 传同一对 `(library_id, dir_id)`，
    /// 否则标签跟着图库切换、地点却一直是全库数字（2026-10-28 用户报的 bug）。
    pub fn places_scoped(
        &self,
        library_id: Option<i64>,
        dir_id: Option<i64>,
    ) -> Result<Vec<PlaceCount>> {
        Ok(repo::places::list_counts_scoped(&self.conn, library_id, dir_id)?
            .into_iter()
            .map(|(name, count)| PlaceCount { name, count })
            .collect())
    }

    /// 给**还没解析过地点**的图补上地点（分批，`limit` 一轮各做多少）。
    /// 返回 `(本轮解析出城市的张数, 还剩多少张待解析)`。
    ///
    /// 两段式（2026-10-19 修正：`image_exif` 在扫描链路**从不写入**，老库全是空表）：
    /// ① **快路径**：`image_exif` 已有坐标的 → 纯 SQL 取坐标 + 查离线表，不开文件；
    /// ② **慢路径**：剩下的（`image_exif` 没行 / 没 GPS 的）→ 每张 `open` 读一次 EXIF，
    ///    顺带把品牌/机型/镜头/GPS 写进 `image_exif`（搜索命中机型靠它），
    ///    再解析地点。**每张图一生只读一次文件**：解析结果（含 NULL）落 `image_places`，
    ///    下次启动就不会再碰它 —— 和 `backfill_times` 的 `meta_checked` 一个思路。
    ///
    /// ⭐ 城市名一律 `pv-geo` 内置离线表，**绝不联网**。
    /// ⚠️ 拿不到绝对路径（卷没挂载）时**不落行**，等下次挂载再补。
    pub fn backfill_places(&mut self, limit: i64) -> (usize, i64) {
        // 一次性的「死行」清理：旧版本把「有 GPS 但解析失败」的图也标成了已解析
        // （place = NULL），修好解析器后它们不会自动重试 —— 必须清掉重跑。
        // 用 settings 里的标记保证只做一次，别每次启动都全表重读 EXIF。
        const FIX_KEY: &str = "places.recheck_null.v1";
        if repo::settings::get(&self.conn, FIX_KEY).ok().flatten().is_none() {
            let n = repo::places::delete_unresolved(&self.conn).unwrap_or(0);
            let _ = repo::settings::set(&self.conn, FIX_KEY, "1");
            let _ = n; // 只在诊断时关心条数（pv-store 没引 tracing）
        }

        let mut hits = 0usize;

        // ① 快路径：坐标已在索引里
        if let Ok(rows) = repo::places::list_gps_missing(&self.conn, limit) {
            for (id, lat, lon) in rows {
                if self.resolve_place(id, lat, lon) {
                    hits += 1;
                }
            }
        }

        // ② 慢路径：`image_exif` 里没坐标的 —— 开文件读 EXIF（每张一生一次）
        let Ok(ids) = repo::places::list_ids_missing(&self.conn, limit) else {
            return (hits, 0);
        };
        for id in ids {
            let img = match repo::images::get(&self.conn, id) {
                Ok(Some(i)) => i,
                _ => continue,
            };
            let Ok(Some(abs)) = self.abs_path(&img) else {
                continue; // 卷没挂载：不落行，下次启动接着补
            };
            let exif = pv_meta::read_exif(&abs);
            let gps = exif
                .as_ref()
                .and_then(|e| e.gps.as_ref())
                .and_then(|g| g.lat_lon());
            // 顺带把拍摄参数写进 image_exif（⛔ 就算全 None 也要落行，
            // 否则这批图每轮启动都会被重读一遍文件）
            let _ = repo::exif::upsert(
                &self.conn,
                id,
                exif.as_ref().and_then(|e| e.make.as_deref()),
                exif.as_ref().and_then(|e| e.model.as_deref()),
                exif.as_ref().and_then(|e| e.lens.as_deref()),
                gps.map(|(la, _)| la),
                gps.map(|(_, lo)| lo),
            );
            if let Some((lat, lon)) = gps {
                if self.resolve_place(id, lat, lon) {
                    hits += 1;
                }
            } else {
                // 没 GPS：也落一行（place = NULL）标记「已解析」，别再读第二次
                let _ = repo::places::upsert(&self.conn, id, None, None, None);
            }
        }

        (hits, repo::places::count_missing(&self.conn).unwrap_or(0))
    }

    /// 查离线表并把结果写进 `image_places`。返回是否解析出了城市。
    fn resolve_place(&self, id: i64, lat: f64, lon: f64) -> bool {
        let place = pv_geo::nearest_place(lat, lon);
        let hit = place.is_some();
        let _ = repo::places::upsert(
            &self.conn,
            id,
            place.as_ref().map(|p| p.name.as_str()),
            Some(lat),
            Some(lon),
        );
        hit
    }

    /// 按 id 取一行索引（批量写文件前要拿 `dir_id` / `rel_name` / `content_key`）。
    pub fn image(&self, id: i64) -> Result<Option<Image>> {
        Ok(repo::images::get(&self.conn, id)?)
    }

    /// 批量取索引行，**顺序与 `ids` 一致**（取不到的跳过）。
    pub fn images_by_ids(&self, ids: &[i64]) -> Result<Vec<Image>> {
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(img) = self.image(*id)? {
                out.push(img);
            }
        }
        Ok(out)
    }

    /// 状态栏 / 筛选条用：全库计数、总字节、分格式统计。
    ///
    /// ⚠️ 这是**全库**统计，不受当前查询过滤影响 —— 筛选条要显示的是
    /// 「每个格式各有多少张」，用它当分母才对得上。
    pub fn stats(&self) -> Result<Stats> {
        let formats: Vec<FormatStat> = repo::images::stats_by_format(&self.conn)?
            .into_iter()
            .map(|(format, count, bytes)| FormatStat {
                format,
                count,
                bytes,
            })
            .collect();
        Ok(Stats {
            total_count: formats.iter().map(|f| f.count).sum(),
            total_bytes: formats.iter().map(|f| f.bytes).sum(),
            formats,
        })
    }

    /// 由索引行还原**绝对路径**：卷挂载点 + 文件夹相对路径 + 文件名。
    ///
    /// 卷未挂载（`last_mount_path` 为空）时返回 `None`——离线状态。
    pub fn abs_path(&self, image: &Image) -> Result<Option<PathBuf>> {
        let Some(dir) = repo::folders::get(&self.conn, image.dir_id)? else {
            return Ok(None);
        };
        let Some(vol) = repo::volumes::get(&self.conn, dir.volume_id)? else {
            return Ok(None);
        };
        let Some(mount) = vol.last_mount_path else {
            return Ok(None);
        };
        let Some(name) = image.rel_name.as_deref() else {
            return Ok(None);
        };
        let mut p = PathBuf::from(mount);
        for part in dir.rel_path.split('/').filter(|s| !s.is_empty()) {
            p.push(part);
        }
        p.push(name);
        Ok(Some(p))
    }

    /// 取缩略图：**L1 内存 → L2 磁盘 → 现生成**（生成后回填 L1 与 L2）。
    pub fn thumbnail(&self, image: &Image, tier: ThumbTier) -> Result<Option<Thumb>> {
        let Some(abs) = self.abs_path(image)? else {
            return Ok(None);
        };
        let key = self.cache_key(image, &abs, tier);

        // L1：内存位图
        if let Some(c) = self.l1.get(&key) {
            return Ok(Some(Thumb {
                width: c.width,
                height: c.height,
                rgba: c.rgba.to_vec(),
            }));
        }

        // L2：磁盘字节 → 解回位图
        if let Some(bytes) = self.l2.get(&key)? {
            /*
             * ⛔⛔ 必须 `exact(Thumbnail, None)`，**不能** `new(Thumbnail)`。
             *     `DecodePurpose::Thumbnail` 的 `suggested_max_edge()` 是 **256**，
             *     用它解码会把缓存里的 720×1280 又缩成 144×256 —— 所有 L2 命中的档位
             *     一律退化成 256 长边（md/lg/xl 全废），表现为「缩略图和头像都比预期糊」。
             *     2026-10-27 实测：xl 档缓存文件 720×1280，`thumbnail()` 返回 144×256。
             *     缓存文件本身就是该档位的目标尺寸，直接按原始大小解码即可。
             */
            let img = pv_decode::decode_jpeg_from_bytes(
                &bytes,
                DecodeTarget::exact(DecodePurpose::Thumbnail, None),
            )?;
            let t = Thumb {
                width: img.width,
                height: img.height,
                rgba: img.data.clone(),
            };
            self.l1.put(key, CachedImage::new(img.width, img.height, img.data));
            return Ok(Some(t));
        }

        // 都没有：现解码 + 编码，回填 L1/L2
        let (bmp, bytes) =
            pv_decode::make_thumbnail_with_bitmap(&self.registry, &abs, tier.long_edge())?;
        self.l2.put(&key, &bytes)?;
        let t = Thumb {
            width: bmp.width,
            height: bmp.height,
            rgba: bmp.data.clone(),
        };
        self.l1.put(key, CachedImage::new(bmp.width, bmp.height, bmp.data));
        Ok(Some(t))
    }

    /// 缓存键对应的 L2 文件路径（测试/诊断用）。
    pub fn cache_file(&self, image: &Image, tier: ThumbTier) -> Option<PathBuf> {
        let abs = self.abs_path(image).ok().flatten()?;
        Some(self.l2.path_of(&self.cache_key(image, &abs, tier)))
    }

    fn cache_key(&self, image: &Image, abs: &Path, tier: ThumbTier) -> CacheKey {
        compute_key(
            &CacheSource::Plain {
                path: abs.to_path_buf(),
                size: image.size.unwrap_or(0) as u64,
                mtime_ns: image.mtime_ns.unwrap_or(0) as i128,
            },
            tier,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// 假解析器：把给定目录当作卷根（跨平台，不依赖真实盘符）。
    struct FakeResolver {
        mount: PathBuf,
        guid: String,
    }

    impl VolumeResolver for FakeResolver {
        fn resolve(&self, path: &Path) -> pv_index::Result<Option<pv_scan::VolumeInfo>> {
            if path.starts_with(&self.mount) {
                Ok(Some(pv_scan::VolumeInfo {
                    volume_guid: self.guid.clone(),
                    serial: Some(1),
                    label: Some("测试卷".into()),
                    fs: Some("NTFS".into()),
                    removable: false,
                    mount_path: self.mount.clone(),
                }))
            } else {
                Ok(None)
            }
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("pv_store_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// 造一个源目录：从 `test-image/` 拷一张小图进去（缺样本则跳过）。
    fn sample_source(tag: &str) -> Option<PathBuf> {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-image/iloveu.webp");
        if !src.is_file() {
            return None;
        }
        let dir = temp_dir(tag);
        std::fs::copy(&src, dir.join("iloveu.webp")).unwrap();
        Some(dir)
    }

    /// 递归入库（用户 2026-10-31「包含子文件夹」）：
    ///
    /// ① 子目录真的进了 `folders`；
    /// ② **子目录里的图也要有宽高** —— 只补根目录的话它们全是 NULL，
    ///    卡片比例退回默认、按分辨率排序全是 0，而且不报错（最难查的那种）。
    #[test]
    fn 递归入库把子目录也算进尺寸回填() {
        let Some(src) = sample_source("tree_src") else {
            eprintln!("跳过：找不到样本");
            return;
        };
        // 造两级：src/a/b/iloveu.webp，根目录自己也放一张
        let sub = src.join("a").join("b");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::copy(src.join("iloveu.webp"), sub.join("deep.webp")).unwrap();

        let data = temp_dir("data_tree");
        let resolver = Box::new(FakeResolver {
            mount: src.clone(),
            guid: r"\?\Volume{store_tree}\".into(),
        });
        let mut store = Store::open(&data, resolver).unwrap();

        let t = store.open_folder_tree(&src).unwrap();
        assert_eq!(t.folders, 3, "src / src/a / src/a/b 三个目录");
        assert!(t.root_folder_id > 0, "根目录 id 要带回来（挂图库要用）");
        assert_eq!(t.images.added, 2, "根目录 1 张 + 深层 1 张");

        // 总共有 2 行，且**两行的宽高都不是 NULL**
        let imgs = store
            .query(&ImageQuery {
                sort: SortKey::RelName,
                dir: SortDir::Asc,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(imgs.len(), 2);
        for i in &imgs {
            assert!(
                i.width.is_some() && i.height.is_some(),
                "{} 没有宽高 —— 递归入库只补了根目录？",
                i.rel_name.as_deref().unwrap_or("?")
            );
        }
    }

    #[test]
    fn 扫描查询与缩略图三级回退() {
        let Some(src) = sample_source("scan") else {
            eprintln!("跳过：找不到样本");
            return;
        };
        let data = temp_dir("data_scan");
        let resolver = Box::new(FakeResolver {
            mount: src.clone(),
            guid: r"\\?\Volume{store}\".into(),
        });
        let mut store = Store::open(&data, resolver).unwrap();

        // 扫描
        let t = store.scan_tree(&src).unwrap();
        assert_eq!(t.folders, 1);
        assert_eq!(t.images.added, 1);

        // 查询
        let imgs = store
            .query(&ImageQuery {
                sort: SortKey::RelName,
                dir: SortDir::Asc,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(imgs.len(), 1);
        let img = &imgs[0];
        assert_eq!(img.rel_name.as_deref(), Some("iloveu.webp"));
        assert_eq!(store.count(&ImageFilter::default()).unwrap(), 1);

        // 绝对路径由「卷挂载点 + 相对路径 + 文件名」还原
        assert_eq!(store.abs_path(img).unwrap().unwrap(), src.join("iloveu.webp"));

        // 首次取缩略图：L1 空 → 现生成
        assert_eq!(store.l1_len(), 0);
        let th = store.thumbnail(img, ThumbTier::Small).unwrap().unwrap();
        assert!(th.width > 0 && th.height > 0);
        assert!(th.width.max(th.height) <= 256, "长边应 ≤ 256");
        assert_eq!(th.rgba.len(), (th.width * th.height * 4) as usize);
        assert_eq!(store.l1_len(), 1, "生成后应进 L1");

        // L2 已落盘
        let cf = store.cache_file(img, ThumbTier::Small).unwrap();
        assert!(cf.is_file(), "L2 应落盘：{}", cf.display());

        // 清 L1 → 应命中 L2 并回填 L1
        store.clear_memory();
        assert_eq!(store.l1_len(), 0);
        let th2 = store.thumbnail(img, ThumbTier::Small).unwrap().unwrap();
        assert_eq!((th2.width, th2.height), (th.width, th.height));
        assert_eq!(store.l1_len(), 1, "L2 命中后应回填 L1");

        // 不同档位落不同文件
        let cf_lg = store.cache_file(img, ThumbTier::Large).unwrap();
        assert_ne!(cf, cf_lg);

        drop(store);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn 卷离线则拿不到绝对路径与缩略图() {
        let Some(src) = sample_source("offline") else {
            return;
        };
        let data = temp_dir("data_off");
        let resolver = Box::new(FakeResolver {
            mount: src.clone(),
            guid: r"\\?\Volume{off}\".into(),
        });
        let mut store = Store::open(&data, resolver).unwrap();
        store.scan_tree(&src).unwrap();
        let imgs = store.query(&ImageQuery::default()).unwrap();

        // 模拟"移动盘未挂载"：清掉 last_mount_path
        let vid = repo::volumes::list(store.conn()).unwrap()[0].id;
        let mut v = repo::volumes::get(store.conn(), vid).unwrap().unwrap();
        v.last_mount_path = None;
        repo::volumes::upsert(store.conn(), &v).unwrap();
        repo::volumes::set_state(store.conn(), vid, pv_index::models::VolumeState::Offline).unwrap();

        assert!(store.abs_path(&imgs[0]).unwrap().is_none(), "未挂载应无绝对路径");
        assert!(
            store.thumbnail(&imgs[0], ThumbTier::Small).unwrap().is_none(),
            "未挂载应取不到缩略图（而非报错）"
        );
        // 关键：离线**不删数据**
        assert_eq!(store.count(&ImageFilter::default()).unwrap(), 1);

        drop(store);
        let _ = std::fs::remove_dir_all(&data);
    }

    /// 递归收集目录下所有文件的相对路径（排序），用于「有没有多出文件」的比对。
    fn snapshot(dir: &Path) -> Vec<String> {
        fn walk(root: &Path, cur: &Path, out: &mut Vec<String>) {
            let Ok(rd) = std::fs::read_dir(cur) else { return };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(root, &p, out);
                } else if let Ok(rel) = p.strip_prefix(root) {
                    out.push(rel.to_string_lossy().into_owned());
                }
            }
        }
        let mut out = Vec::new();
        walk(dir, dir, &mut out);
        out.sort();
        out
    }

    /// ⭐⭐ **浏览路径必须零持久化副作用**（2026-10-16 与用户确认的产品语义）。
    ///
    /// 「浏览」（左键点目录）只做临时预览：直读磁盘列文件 + 现解码缩略图，
    /// **不写 `library.db`、不写缩略图缓存**；只有右键「加入图库」才落盘。
    ///
    /// 这条测试是**护栏**：浏览用的 [`list_dir_images`] / `pv_decode::decode_scaled`
    /// 结构上就不接 `Store`，所以本来也写不了盘；一旦日后有人图省事把它们
    /// 接到 Store 上（比如「顺手缓存一下更快」），这里会立刻变红。
    #[test]
    fn 浏览路径不写索引也不写缩略图缓存() {
        let Some(src) = sample_source("browse") else {
            eprintln!("跳过：找不到样本");
            return;
        };
        let data = temp_dir("data_browse");
        let resolver = Box::new(FakeResolver {
            mount: src.clone(),
            guid: r"\\?\Volume{browse}\".into(),
        });
        let mut store = Store::open(&data, resolver).unwrap();
        store.scan_tree(&src).unwrap();

        // 先正经生成一张缩略图，让数据目录里**确实有东西**可比 ——
        // 否则 before 是空的，后面的相等断言等于什么都没验证。
        let imgs = store.query(&ImageQuery::default()).unwrap();
        store.thumbnail(&imgs[0], ThumbTier::Small).unwrap().unwrap();
        let before = snapshot(&data);
        assert!(
            !before.is_empty(),
            "前置条件：数据目录里应已有索引与缩略图缓存"
        );

        // ── 走浏览路径：列目录 + 解临时缩略图 ──
        let listing = list_dir_images(&src).unwrap();
        assert_eq!(listing.len(), 1, "应列出样本目录里的那一张");
        assert_eq!(listing[0].name, "iloveu.webp");
        assert!(
            Path::new(&listing[0].path).is_file(),
            "返回的应是可直接用的绝对路径"
        );

        let mut reg = pv_decode::DecoderRegistry::new();
        reg.register(Box::new(pv_decode::BitmapBackend));
        let bmp = pv_decode::decode_scaled(&reg, Path::new(&listing[0].path), 64).unwrap();
        assert!(
            bmp.width.max(bmp.height) <= 64,
            "临时缩略图也要受长边约束，实际 {}x{}",
            bmp.width,
            bmp.height
        );

        // ── 关键断言：数据目录一个文件都没多 ──
        assert_eq!(
            snapshot(&data),
            before,
            "浏览不该往数据目录写任何东西（索引与缩略图缓存都不行）"
        );

        drop(store);
        let _ = std::fs::remove_dir_all(&data);
    }

    /// 造一棵带子目录的样本树：`<dir>/iloveu.webp` + `<dir>/sub/b.webp`。
    fn nested_source(tag: &str) -> Option<(PathBuf, PathBuf)> {
        let src = sample_source(tag)?;
        let sub = src.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::copy(src.join("iloveu.webp"), sub.join("b.webp")).unwrap();
        Some((src, sub))
    }

    /// ⭐⭐ **从图库移除 = 删索引 + 删缓存，但绝不碰用户的原始图片。**
    ///
    /// 这条是护栏：`remove_folder` 一旦哪天写成「顺手把文件也删了」
    /// （有人会觉得「移除嘛，当然连文件一起」），这里立刻红。
    #[test]
    fn 移除目录只删索引与缓存不碰原图() {
        let Some((src, sub)) = nested_source("rm") else {
            eprintln!("跳过：找不到样本");
            return;
        };
        let data = temp_dir("data_rm");
        let resolver = Box::new(FakeResolver {
            mount: src.clone(),
            guid: r"\\?\Volume{rm}\".into(),
        });
        let mut store = Store::open(&data, resolver).unwrap();
        let t = store.scan_tree(&src).unwrap();
        assert_eq!(t.folders, 2, "根 + sub");
        assert_eq!(t.images.added, 2);

        // 生成缩略图，让 L2 里确实有东西可删
        let imgs = store.query(&ImageQuery::default()).unwrap();
        for i in &imgs {
            store.thumbnail(i, ThumbTier::Small).unwrap();
        }
        let thumbs_dir = data.join("thumbs");
        let cached_before = snapshot(&thumbs_dir).len();
        assert!(cached_before >= 2, "前置条件：应已生成缩略图缓存");

        let vid = repo::volumes::list(store.conn()).unwrap()[0].id;
        let root_id = repo::folders::find(store.conn(), vid, "").unwrap().unwrap().id;
        let sub_id = repo::folders::find(store.conn(), vid, "sub").unwrap().unwrap().id;

        // 挂进一个图库，验证 `library_folders` 也会被清。
        // ⚠️ 两个目录都要挂上：`count_of` 是 `dir_ids IN (...)` 的**精确匹配**，
        //    不含子目录（挂父目录并不会把子目录的图算进来）。
        let lib_id =
            pv_lib::library::create_normal(store.conn(), "测试库", &[root_id, sub_id]).unwrap();
        let lib = repo::libraries::get(store.conn(), lib_id).unwrap().unwrap();
        assert_eq!(pv_lib::library::count_of(store.conn(), &lib).unwrap(), 2);

        let r = store.remove_folder(root_id).unwrap();
        assert_eq!(r.folders, 2, "父 + 子两个目录行都要删");
        assert_eq!(r.images, 2);
        assert!(r.thumbs >= 2, "至少两张的缓存该被清掉，实际 {}", r.thumbs);
        assert_eq!(r.thumbs_orphan, 0, "卷在线，不该有清不掉的缓存");

        // 索引清空
        assert_eq!(store.count(&ImageFilter::default()).unwrap(), 0);
        assert!(repo::folders::list_by_volume(store.conn(), vid)
            .unwrap()
            .is_empty());
        // 图库挂载关系也清了（留着会让图库条目数算出一个诡异的数字）
        assert_eq!(
            pv_lib::library::count_of(store.conn(), &lib).unwrap(),
            0,
            "移除目录后图库条目数应归零"
        );
        // ⭐ 直接看 `library_folders`：`count_of` 是 JOIN images 算的，删了图它自然为 0，
        //    证明不了成员行被删。这里必须查表 —— 2026-10-27 之前 `remove_folder`
        //    只在删 images，`library_folders` 的脏行一直留着（NO ACTION 外键 +
        //    SQLite 默认不开 foreign_keys）。
        let members = repo::libraries::list_members(store.conn()).unwrap();
        assert!(
            !members.iter().any(|(_, fid)| *fid == root_id || *fid == sub_id),
            "被移除的目录不能还挂在任何图库里，实际 {members:?}"
        );
        // 缓存文件删干净
        assert_eq!(snapshot(&thumbs_dir).len(), 0, "对应的缩略图缓存要一起清掉");

        // ⭐⭐ 原图一个都不能少
        assert!(src.join("iloveu.webp").is_file(), "原图必须原封不动");
        assert!(sub.join("b.webp").is_file(), "子目录里的原图也必须原封不动");

        // 幂等：重复移除不报错、返回全 0
        assert_eq!(store.remove_folder(root_id).unwrap(), Removed::default());

        drop(store);
        let _ = std::fs::remove_dir_all(&data);
        let _ = std::fs::remove_dir_all(&src);
    }

    /// ⭐⭐ 移除目录时**人脸与被删图片必须一起走**，人物的脸数/封面要跟着重算。
    ///
    /// 2026-10-27 之前 `images::delete` 只清了 7 张关联表、**漏了 `faces`**：
    /// 移除目录后留下孤儿人脸，人像条会拿着不存在的 `image_id` 取缩略图（永远失败），
    /// 人物 `face_count` 虚高。
    #[test]
    fn 移除目录不留孤儿人脸且人物脸数同步() {
        use pv_index::models::{Face, Person};

        let Some((src, _sub)) = nested_source("rmface") else {
            eprintln!("跳过：找不到样本");
            return;
        };
        let data = temp_dir("data_rmface");
        let resolver = Box::new(FakeResolver {
            mount: src.clone(),
            guid: r"\?\Volume{rmface}\".into(),
        });
        let mut store = Store::open(&data, resolver).unwrap();
        store.scan_tree(&src).unwrap();

        let imgs = store.query(&ImageQuery::default()).unwrap();
        assert_eq!(imgs.len(), 2);

        // 给根目录那张图插两张脸，归到同一个人物
        let pid = repo::persons::insert(
            store.conn(),
            &Person::new(Some("测试人物".into()), Some("t".into())),
        )
        .unwrap();
        let mk = |image_id: i64, w: f32| Face {
            id: 0,
            image_id,
            person_id: Some(pid),
            x: 0.1,
            y: 0.1,
            w,
            h: w,
            tier: Some(0),
            model: "test".into(),
            score: Some(0.9),
            quality: Some(w * 100.0),
            embedding: vec![0u8; 128],
        };
        repo::faces::replace_for_image(store.conn(), imgs[0].id, &[mk(imgs[0].id, 0.2), mk(imgs[0].id, 0.3)])
            .unwrap();
        repo::persons::refresh_count(store.conn(), pid).unwrap();
        assert_eq!(repo::faces::count_for_person(store.conn(), pid).unwrap(), 2);

        let vid = repo::volumes::list(store.conn()).unwrap()[0].id;
        let root_id = repo::folders::find(store.conn(), vid, "").unwrap().unwrap().id;

        store.remove_folder(root_id).unwrap();

        // 孤儿脸一个都不许剩
        let orphans: i64 = store
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM faces f LEFT JOIN images i ON i.id = f.image_id
                  WHERE i.id IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphans, 0, "移除目录后不能留下孤儿人脸");
        // 脸随图走，人物没脸了就连人物一起清掉
        assert_eq!(repo::persons::list(store.conn(), 0).unwrap().len(), 0, "没有脸的人物要清掉");

        drop(store);
        let _ = std::fs::remove_dir_all(&data);
        let _ = std::fs::remove_dir_all(&src);
    }

    /// 移除**子目录**只影响它自己，兄弟目录与父目录都要留着。
    #[test]
    fn 移除子目录不影响兄弟与父目录() {
        let Some((src, sub)) = nested_source("rmsub") else {
            eprintln!("跳过：找不到样本");
            return;
        };
        let data = temp_dir("data_rmsub");
        let resolver = Box::new(FakeResolver {
            mount: src.clone(),
            guid: r"\\?\Volume{rmsub}\".into(),
        });
        let mut store = Store::open(&data, resolver).unwrap();
        store.scan_tree(&src).unwrap();
        assert_eq!(store.count(&ImageFilter::default()).unwrap(), 2);

        let vid = repo::volumes::list(store.conn()).unwrap()[0].id;
        let sub_id = repo::folders::find(store.conn(), vid, "sub").unwrap().unwrap().id;

        let r = store.remove_folder(sub_id).unwrap();
        assert_eq!((r.folders, r.images), (1, 1), "只该删 sub 自己那一层");
        assert_eq!(store.count(&ImageFilter::default()).unwrap(), 1, "父目录那张还在");
        assert!(
            repo::folders::find(store.conn(), vid, "").unwrap().is_some(),
            "父目录行必须还在"
        );
        assert!(sub.join("b.webp").is_file(), "原图不动");
        assert!(src.join("iloveu.webp").is_file(), "原图不动");

        drop(store);
        let _ = std::fs::remove_dir_all(&data);
        let _ = std::fs::remove_dir_all(&src);
    }

    /// ⭐ 「加入图库」的两条保证：**同名复用** 与 **幂等**。
    ///
    /// 不这么做的话，「用文件夹名加入」点两次会得到两个同名图库 ——
    /// 侧栏里两行长得一模一样，用户根本分不清哪个是哪个。
    #[test]
    fn 归入图库同名复用且幂等() {
        let Some((src, _sub)) = nested_source("lib") else {
            eprintln!("跳过：找不到样本");
            return;
        };
        let data = temp_dir("data_lib");
        let resolver = Box::new(FakeResolver {
            mount: src.clone(),
            guid: r"\\?\Volume{lib}\".into(),
        });
        let mut store = Store::open(&data, resolver).unwrap();
        store.scan_tree(&src).unwrap();
        let vid = repo::volumes::list(store.conn()).unwrap()[0].id;
        let root_id = repo::folders::find(store.conn(), vid, "").unwrap().unwrap().id;
        let sub_id = repo::folders::find(store.conn(), vid, "sub").unwrap().unwrap().id;

        // 同名两次 → 同一个图库，且两个目录都挂上了
        let a = store.add_to_library(root_id, "我的收藏", None, false).unwrap();
        let b = store.add_to_library(sub_id, "我的收藏", None, false).unwrap();
        assert_eq!(a, b, "同名应复用同一个图库");
        assert_eq!(repo::libraries::list(store.conn()).unwrap().len(), 1);
        let lib = repo::libraries::get(store.conn(), a).unwrap().unwrap();
        assert_eq!(lib.name, "我的收藏");
        assert_eq!(pv_lib::library::count_of(store.conn(), &lib).unwrap(), 2);

        // 幂等：重复挂同一个目录不报错、不重复
        assert_eq!(store.add_to_library(root_id, "我的收藏", None, false).unwrap(), a);
        assert_eq!(
            repo::libraries::list_folder_ids(store.conn(), a).unwrap(),
            vec![root_id, sub_id]
        );

        // 显式指定 library_id：挂到它，name 只是摆设
        assert_eq!(
            store.add_to_library(sub_id, "随便起的名字", Some(a), false).unwrap(),
            a,
            "给了 library_id 就该挂到它，不该另建一个"
        );
        assert_eq!(repo::libraries::list(store.conn()).unwrap().len(), 1);

        drop(store);
        let _ = std::fs::remove_dir_all(&data);
        let _ = std::fs::remove_dir_all(&src);
    }

    /// ⭐ 标签：打上之后要能**按标签筛**、能**搜到**（搜索是文件名 ∪ 标签名）。
    ///
    /// 护栏的由来：`fts_images` 用的是 unicode61，**不切中文** ——
    /// 存"海边日落"搜"日落"匹配不到，所以搜索只能走 LIKE（文件名 OR 标签名）。
    /// 这里钉住：加标签后搜索能命中，按标签名筛选能命中，计数随之变化。
    #[test]
    fn 标签可打可按名搜索() {
        let Some((src, _sub)) = nested_source("tag") else {
            eprintln!("跳过：找不到样本");
            return;
        };
        let data = temp_dir("data_tag");
        let resolver = Box::new(FakeResolver {
            mount: src.clone(),
            guid: r"\\?\Volume{tag}\".into(),
        });
        let mut store = Store::open(&data, resolver).unwrap();
        store.scan_tree(&src).unwrap();
        let all = store.query(&ImageQuery::default()).unwrap();
        assert_eq!(all.len(), 2);
        let one = all[0].id;

        // 打标签（同名会复用）
        store.add_tags(&[one], &["风景".into(), "风景".into()]).unwrap();
        let tags = store.tags().unwrap();
        assert_eq!(tags.len(), 1, "同名标签不该建出两条");
        assert_eq!(tags[0].count, 1);

        // ① 按标签名筛选（`tags_any`）
        let by_tag = store
            .query(&ImageQuery {
                filter: ImageFilter {
                    tags_any: vec!["风景".into()],
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();
        assert_eq!(by_tag.len(), 1);
        assert_eq!(by_tag[0].id, one);

        // ② 搜索命中标签名（文件名里没有"景"字）
        let by_search = store
            .query(&ImageQuery {
                filter: ImageFilter {
                    name_like: Some("景".into()),
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();
        assert_eq!(by_search.len(), 1, "搜索应该能搜到标签名");

        // ③ picker 用的"这批里有几张带" 
        let hits = store.tag_counts_for(&[one]).unwrap();
        assert_eq!(hits[0], ("风景".to_string(), 1));

        // ④ 改名 + 移除 + 删除
        store.rename_tag(tags[0].id, "风光").unwrap();
        assert_eq!(store.tags().unwrap()[0].name, "风光");
        store.remove_tag(&[one], tags[0].id).unwrap();
        assert_eq!(store.tags().unwrap()[0].count, 0, "移除后计数归零");
        store.delete_tag(tags[0].id).unwrap();
        assert!(store.tags().unwrap().is_empty(), "标签删干净");

        drop(store);
        let _ = std::fs::remove_dir_all(&data);
        let _ = std::fs::remove_dir_all(&src);
    }

    /// ⭐⭐ 地点回填的两段式（2026-10-19 修正）：扫描链路**从不写 `image_exif`**，
    /// 老库全是空表 —— 所以慢路径必须开文件读 EXIF，并**顺带**把品牌/机型/镜头/GPS
    /// 写进 `image_exif`（搜索命中机型靠它）。没 GPS 的图也要落行（place = NULL），
    /// 否则每轮启动都会重读一遍文件。
    /// ⭐⭐ 删图库的**新语义**（2026-10-31 用户要求）：
    /// 独占的目录连索引一起清掉，**「全部照片」里这些图也要消失**；
    /// 但**绝不碰磁盘上的原图**。
    ///
    /// 这条同时是护栏：`delete_library` 哪天写成了「顺手把文件也删了」，
    /// 或者「只删库行、索引留着」（旧语义），这里都会红。
    #[test]
    fn 删除图库会清掉它独占的索引但不动原图() {
        let Some((src, _sub)) = nested_source("libdel") else {
            eprintln!("跳过：找不到样本");
            return;
        };
        let data = temp_dir("data_libdel");
        let resolver = Box::new(FakeResolver {
            mount: src.clone(),
            guid: r"\\?\Volume{libdel}\".into(),
        });
        let mut store = Store::open(&data, resolver).unwrap();
        store.scan_tree(&src).unwrap();
        let vid = repo::volumes::list(store.conn()).unwrap()[0].id;
        let root_id = repo::folders::find(store.conn(), vid, "").unwrap().unwrap().id;

        let lib_id = store.add_to_library(root_id, "我的收藏", None, true).unwrap();
        let before = store.count(&ImageFilter::default()).unwrap();
        assert!(before > 1, "样本要有根目录 + 子目录至少两张");

        let r = store.delete_library(lib_id).unwrap().expect("库存在，应当返回统计");
        assert!(repo::libraries::get(store.conn(), lib_id).unwrap().is_none());

        // 「全部照片」必须清空 —— 这正是用户反馈的问题（删了库照片还在）
        assert_eq!(
            store.count(&ImageFilter::default()).unwrap(),
            0,
            "删库后全部照片应当为空"
        );
        assert_eq!(r.images as i64, before, "统计要和实际删掉的行数一致");
        assert!(r.folders >= 1);

        // 原图一张都不能少
        assert!(src.join("iloveu.webp").is_file(), "根目录的原图不能删");
        assert!(src.join("sub").join("b.webp").is_file(), "子目录的原图不能删");

        // 幂等：删一个不存在的 id 返回 None，不报错
        assert!(store.delete_library(lib_id).unwrap().is_none());

        drop(store);
        let _ = std::fs::remove_dir_all(&data);
        let _ = std::fs::remove_dir_all(&src);
    }

    /// 同一个目录挂在**两个**图库上时，删掉一个库**不能**动它的索引 ——
    /// 另一个库的照片必须还在。
    ///
    /// ⚠️ 注意「共享」的判断是**按目录**算的，不是按父子：甲库递归挂了
    /// `root`（含 `sub`）、乙库只挂 `root` 时，`sub` 其实只属于甲库 ——
    /// 删甲库把 `sub` 的索引清掉是**对的**（乙库本来就不含它）。
    #[test]
    fn 删除图库不动还被别人引用的目录() {
        let Some((src, _sub)) = nested_source("libshare") else {
            eprintln!("跳过：找不到样本");
            return;
        };
        let data = temp_dir("data_libshare");
        let resolver = Box::new(FakeResolver {
            mount: src.clone(),
            guid: r"\\?\Volume{libshare}\".into(),
        });
        let mut store = Store::open(&data, resolver).unwrap();
        store.scan_tree(&src).unwrap();
        let vid = repo::volumes::list(store.conn()).unwrap()[0].id;
        let root_id = repo::folders::find(store.conn(), vid, "").unwrap().unwrap().id;

        // 两个库都**递归**挂根目录 —— 否则乙库只含根目录那一层，
        // 甲库删掉后子目录「确实没人要了」，删索引反而是对的（见下方注释）
        let a = store.add_to_library(root_id, "甲", None, true).unwrap();
        let _b = store.add_to_library(root_id, "乙", None, true).unwrap();
        let before = store.count(&ImageFilter::default()).unwrap();
        assert!(before > 0);

        let r = store.delete_library(a).unwrap().unwrap();
        assert_eq!(r.images, 0, "还被乙库引用着，一张都不能删");
        assert_eq!(
            store.count(&ImageFilter::default()).unwrap(),
            before,
            "乙库的照片必须还在"
        );

        drop(store);
        let _ = std::fs::remove_dir_all(&data);
        let _ = std::fs::remove_dir_all(&src);
    }

    /// ⭐ 智能图库：`query_in_library` 必须**按规则求值**，不能当成普通库 JOIN。
    ///
    /// 智能库没有 `library_folders` 成员行 —— 直接 JOIN 的结果恒为空，
    /// 界面上就是一个永远空着的图库，看着像「规则写错了」，很难查。
    #[test]
    fn 智能图库按规则求值() {
        let Some((src, _sub)) = nested_source("libsmart") else {
            eprintln!("跳过：找不到样本");
            return;
        };
        let data = temp_dir("data_libsmart");
        let resolver = Box::new(FakeResolver {
            mount: src.clone(),
            guid: r"\\?\Volume{libsmart}\".into(),
        });
        let mut store = Store::open(&data, resolver).unwrap();
        store.scan_tree(&src).unwrap();

        // 两张图，只给其中一张打 4 星
        let all = store.query(&ImageQuery::default()).unwrap();
        assert_eq!(all.len(), 2);
        let picked = all[0].id;
        store.set_rating(&[picked], 4).unwrap();

        let smart = store.create_smart_library("四星以上", "rating:>=4").unwrap();

        let rows = store
            .query_in_library(&ImageQuery::default(), smart)
            .unwrap();
        assert_eq!(rows.len(), 1, "智能库只该命中打过星的那张");
        assert_eq!(rows[0].id, picked);
        assert_eq!(
            store
                .count_in_library(&ImageFilter::default(), smart)
                .unwrap(),
            1
        );

        // 规则改成「全部图片」→ 两张都在（切换规则后求值要跟着变）
        let all_lib = store.create_smart_library("全部", "type:image").unwrap();
        assert_eq!(
            store
                .query_in_library(&ImageQuery::default(), all_lib)
                .unwrap()
                .len(),
            2
        );

        // 非法规则建库直接报错（不能建出一个永远空的库）
        assert!(store.create_smart_library("坏规则", "ext:xyz").is_err());
        // 空规则也不行
        assert!(store.create_smart_library("空规则", "  ").is_err());
        // 重名挡住
        assert!(store.create_smart_library("四星以上", "rating:>=5").is_err());

        drop(store);
        let _ = std::fs::remove_dir_all(&data);
        let _ = std::fs::remove_dir_all(&src);
    }

    /// 改图库名：只改名字，关系不动；重名要挡（改成自己现在的名字不算冲突）。
    #[test]
    fn 改图库名挡重名且不动成员() {
        let Some((src, _sub)) = nested_source("libren") else {
            eprintln!("跳过：找不到样本");
            return;
        };
        let data = temp_dir("data_libren");
        let resolver = Box::new(FakeResolver {
            mount: src.clone(),
            guid: r"\\?\Volume{libren}\".into(),
        });
        let mut store = Store::open(&data, resolver).unwrap();
        store.scan_tree(&src).unwrap();
        let vid = repo::volumes::list(store.conn()).unwrap()[0].id;
        let root_id = repo::folders::find(store.conn(), vid, "").unwrap().unwrap().id;

        let a = store.add_to_library(root_id, "甲", None, false).unwrap();
        let b = store.create_library("乙").unwrap();

        store.rename_library(a, "丙").unwrap();
        assert_eq!(repo::libraries::get(store.conn(), a).unwrap().unwrap().name, "丙");
        // 成员关系不受影响
        assert_eq!(repo::libraries::list_folder_ids(store.conn(), a).unwrap(), vec![root_id]);

        // 改成自己现在的名字：不算冲突
        store.rename_library(a, "丙").unwrap();

        // 撞上别人的名字：报错，且两边都没被改
        assert!(store.rename_library(a, "乙").is_err());
        assert_eq!(repo::libraries::get(store.conn(), a).unwrap().unwrap().name, "丙");
        assert_eq!(repo::libraries::get(store.conn(), b).unwrap().unwrap().name, "乙");

        // 空名挡住
        assert!(store.rename_library(a, "   ").is_err());

        drop(store);
        let _ = std::fs::remove_dir_all(&data);
        let _ = std::fs::remove_dir_all(&src);
    }

    /// test-image 里找一张**确实带拍摄时间**的样本（没有就跳过）。
    fn sample_with_taken(name: &str) -> Option<PathBuf> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-image")
            .join(name);
        if !p.is_file() {
            return None;
        }
        // 先确认它真能读出拍摄时间，否则这条测试什么也没证明
        pv_meta::read_exif(&p)?.taken_at_unix()?;
        Some(p)
    }

    /// ⭐⭐ 「按拍摄时间排序」不能是假按钮 —— 入库时必须把 EXIF 的
    /// `DateTimeOriginal` 落进 `images.taken_at`。
    ///
    /// 这条护栏的由来：字段、索引、排序 SQL、菜单项全都齐了，**但没人填数据**，
    /// 于是 `COALESCE(taken_at, added_ts)` 静默退化成「按入库时间」——
    /// 界面看起来完全正常，只是顺序不对。
    #[test]
    fn 入库时填上拍摄时间() {
        let Some(src) = ["canon-cr3-sample.CR3", "IMG_7507.DNG", "sony-sr2-sample.SR2"]
            .iter()
            .find_map(|n| sample_with_taken(n))
        else {
            eprintln!("跳过：test-image 里没有带拍摄时间的样本");
            return;
        };

        let dir = temp_dir("times_src");
        let name = src.file_name().unwrap().to_owned();
        std::fs::copy(&src, dir.join(&name)).unwrap();

        let data = temp_dir("data_times");
        let resolver = Box::new(FakeResolver {
            mount: dir.clone(),
            guid: r"\\?\Volume{times}\".into(),
        });
        let mut store = Store::open(&data, resolver).unwrap();
        let out = store.open_folder(&dir).unwrap();

        let rows = repo::images::list_by_dir(store.conn(), out.folder_id).unwrap();
        assert_eq!(rows.len(), 1, "应该只有刚复制进去的那一张");
        assert!(
            rows[0].taken_at.is_some(),
            "{} 有 EXIF 拍摄时间，入库后 taken_at 不该是空的",
            name.to_string_lossy()
        );

        // 再跑一次回填：没有缺的，应当零补零剩
        assert_eq!(store.backfill_times(100), (0, 0));

        drop(store);
        let _ = std::fs::remove_dir_all(&data);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ⭐ 卡片要显示「宽 × 高」—— 尺寸必须在**入库时**就探出来，
    /// 老库迁移进来（没有尺寸）的行靠回填补上。
    #[test]
    fn 入库即有尺寸且回填只补缺的() {
        let Some(src) = sample_source("dims") else {
            eprintln!("跳过：找不到样本");
            return;
        };
        // 造一个「老库行」：文件在磁盘上，但索引里没有尺寸（历史数据都这样）
        std::fs::copy(src.join("iloveu.webp"), src.join("legacy.webp")).unwrap();

        let data = temp_dir("data_dims");
        let resolver = Box::new(FakeResolver {
            mount: src.clone(),
            guid: r"\\?\Volume{dims}\".into(),
        });
        let mut store = Store::open(&data, resolver).unwrap();
        // ⚠️ 走 `open_folder`（含扫描后的尺寸补齐），别走 `scan_tree`
        store.open_folder(&src).unwrap();

        let imgs = store.query(&ImageQuery::default()).unwrap();
        assert_eq!(imgs.len(), 2);
        for i in &imgs {
            assert!(
                i.width.unwrap_or(0) > 0 && i.height.unwrap_or(0) > 0,
                "扫描后每一行都该有尺寸，{} 却是 {:?}×{:?}",
                i.rel_name.as_deref().unwrap_or("?"),
                i.width,
                i.height
            );
        }

        // 再造一行缺尺寸的（模拟老库升级），回填应只补它
        let vid = repo::volumes::list(store.conn()).unwrap()[0].id;
        let dir_id = repo::folders::find(store.conn(), vid, "").unwrap().unwrap().id;
        let mut legacy = pv_index::models::Image::new(dir_id, "legacy2.webp");
        legacy.size = Some(10);
        legacy.mtime_ns = Some(1);
        legacy.format = Some("webp".into());
        std::fs::copy(src.join("iloveu.webp"), src.join("legacy2.webp")).unwrap();
        let legacy_id = repo::images::insert(store.conn(), &legacy).unwrap();
        assert_eq!(
            repo::images::count_missing_dimensions(store.conn()).unwrap(),
            1
        );

        let (filled, left) = store.backfill_dimensions(100);
        assert_eq!(filled, 1, "应只补缺的那一行");
        assert_eq!(left, 0, "补完就不该再缺");
        let img = repo::images::get(store.conn(), legacy_id).unwrap().unwrap();
        assert!(img.width.unwrap_or(0) > 0, "回填后应有尺寸");

        // 全补完后再跑一次：零开销（一条 COUNT 就返回）
        assert_eq!(store.backfill_dimensions(100), (0, 0));

        // ⛔ RAW 行必须被跳过：探测它要把整个文件读进来找内嵌预览
        // （一张 ARW 平均 47MB），不跳过的话加入 RAW 目录会卡死几分钟。
        let mut raw_row = pv_index::models::Image::new(dir_id, "legacy3.webp");
        raw_row.size = Some(10);
        raw_row.mtime_ns = Some(1);
        raw_row.format = Some("raw".into());
        raw_row.is_raw = true;
        std::fs::copy(src.join("iloveu.webp"), src.join("legacy3.webp")).unwrap();
        let raw_id = repo::images::insert(store.conn(), &raw_row).unwrap();

        assert_eq!(repo::images::count_missing_dimensions(store.conn()).unwrap(), 1);
        let (filled, left) = store.backfill_dimensions(100);
        assert_eq!((filled, left), (0, 1), "RAW 行不该被补，也不该被删");
        assert!(
            repo::images::get(store.conn(), raw_id)
                .unwrap()
                .unwrap()
                .width
                .is_none(),
            "RAW 不该有尺寸"
        );

        drop(store);
        let _ = std::fs::remove_dir_all(&data);
        let _ = std::fs::remove_dir_all(&src);
    }
    /// ⭐ EXIF 方向（2026-10-30）：竖拍照片入库时，`width/height` 必须是**显示尺寸**。
    ///
    /// 这张样本（IMGP9873.JPG）传感器写的是 3840×2880 + `Orientation=6`，
    /// 用户看到的应是 **2880×3840**。存成原始像素的话，卡片长宽比会整个反掉，
    /// 缩略图/看图/打印里照片还是躺着的。
    #[test]
    fn 竖拍入库按方向摆正宽高() {
        let src_file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-image/IMGP9873.JPG");
        if !src_file.is_file() {
            eprintln!("跳过：test-image 里没有 IMGP9873.JPG");
            return;
        }
        let dir = temp_dir("orient_src");
        std::fs::copy(&src_file, dir.join("rot.jpg")).unwrap();

        let data = temp_dir("data_orient");
        let resolver = Box::new(FakeResolver {
            mount: dir.clone(),
            guid: r"\\?\Volume{orient}\".into(),
        });
        let mut store = Store::open(&data, resolver).unwrap();
        store.open_folder(&dir).unwrap();

        let imgs = store.query(&ImageQuery::default()).unwrap();
        assert_eq!(imgs.len(), 1);
        let i = &imgs[0];
        assert_eq!(
            (i.width, i.height),
            (Some(2880), Some(3840)),
            "宽高必须是显示尺寸（6/8 要换过来）"
        );
        let o = repo::images::get_orientation(store.conn(), i.id).unwrap();
        assert_eq!(o, Some(6), "方向要写进 orientation 列");

        // ── 老库模拟：行里是原始像素 + 方向 NULL，回填要把宽高换过来 ──
        repo::images::update_dimensions(store.conn(), i.id, 3840, 2880).unwrap();
        repo::images::clear_orientation(store.conn(), i.id).unwrap();
        let (done, left) = store.backfill_orientation(100);
        assert_eq!((done, left), (1, 0), "应当处理 1 行、无剩余");
        let imgs = store.query(&ImageQuery::default()).unwrap();
        assert_eq!(
            (imgs[0].width, imgs[0].height),
            (Some(2880), Some(3840)),
            "回填要把 6/8 的宽高换回来"
        );
        // 再跑一次：幂等，什么都不做
        assert_eq!(store.backfill_orientation(100), (0, 0));

        drop(store);
        let _ = std::fs::remove_dir_all(&data);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
