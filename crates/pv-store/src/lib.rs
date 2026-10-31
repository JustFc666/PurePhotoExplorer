//! `pv-store`：**服务层**，把索引 / 扫描 / 缓存 / 解码串成 UI 可直接用的 API。
//!
//! 分工：
//! - `pv-index` 存索引、`pv-scan` 扫文件系统、`pv-cache` 存缩略图、`pv-decode` 解码；
//! - 本 crate 持有 `Connection` + `L1`/`L2` + `DecoderRegistry` + `VolumeResolver`，
//!   对外只暴露三个动作：**扫描 / 查询 / 取缩略图**。
//!
//! 取缩略图是三级回退：**L1 内存 → L2 磁盘 → 现生成（回填 L1+L2）**。

#![cfg_attr(test, allow(non_snake_case))]

use std::path::Path;

use pv_cache::{L1, L2, ThumbTier};
use pv_decode::{BitmapBackend, DecoderRegistry, EmbeddedPreviewPipeline};
use pv_index::{conn as index_conn, migrate, Connection};
use pv_scan::VolumeResolver;

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

// 按域拆开的 `impl Store`（纯位移，行为不变）
pub mod browse;
pub mod dims;
pub mod fileops;
pub mod library;
pub mod places;
pub mod query;
pub mod scan;
pub mod tags;
pub mod thumb;

// 便捷再导出（UI 层无需知道底下拆了几个 crate）
pub use browse::{list_dir_images, list_dir_images_fast, DirImage};
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use pv_index::repo;
    use std::path::{Path, PathBuf};

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

    /// 删**单张**图（不是整个目录）：缓存要清、人物脸数要跟着重算。
    ///
    /// 这两件事以前是漏的（`remove_images` 既没 `persons::refresh_all`，
    /// 又是「边算边删缓存」）—— 从图库删几张照片后，人像条会停在旧的「N 张脸」，
    /// 而且事务万一失败回滚，照片还在、缩略图已经没了。
    #[test]
    fn 删单张图同步人物脸数并清缓存() {
        use pv_index::models::{Face, Person};

        let Some((src, _sub)) = nested_source("rmimgs") else {
            eprintln!("跳过：找不到样本");
            return;
        };
        let data = temp_dir("data_rmimgs");
        let resolver = Box::new(FakeResolver {
            mount: src.clone(),
            guid: r"\?\Volume{rmimgs}\".into(),
        });
        let mut store = Store::open(&data, resolver).unwrap();
        store.scan_tree(&src).unwrap();

        let imgs = store.query(&ImageQuery::default()).unwrap();
        assert_eq!(imgs.len(), 2);

        // 先让第一张有缩略图缓存，删完必须跟着消失
        assert!(store.thumbnail(&imgs[0], ThumbTier::Small).unwrap().is_some());
        let cache = store.cache_file(&imgs[0], ThumbTier::Small).unwrap();
        assert!(cache.exists(), "缩略图应已落到 L2");

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
        repo::faces::replace_for_image(store.conn(), imgs[0].id, &[mk(imgs[0].id, 0.2)])
            .unwrap();
        repo::faces::replace_for_image(store.conn(), imgs[1].id, &[mk(imgs[1].id, 0.3)])
            .unwrap();
        repo::persons::refresh_count(store.conn(), pid).unwrap();
        assert_eq!(
            repo::persons::get(store.conn(), pid).unwrap().unwrap().face_count,
            2
        );

        let removed = store.remove_images(&[imgs[0].id]).unwrap();
        assert_eq!(removed.images, 1);
        assert!(!cache.exists(), "删图要把它的缩略图缓存一起清掉");
        // ⚠️ 断言的是 `persons.face_count` **列**（人像条读的就是它），
        //    不是实时 COUNT —— 后者不经过 refresh_all 也会变对，测不出这个 bug。
        let left = repo::persons::get(store.conn(), pid).unwrap().unwrap();
        assert_eq!(left.face_count, 1, "删图后人物缓存的脸数要同步");
        assert_eq!(
            repo::faces::count_for_person(store.conn(), pid).unwrap(),
            1,
            "人物的脸确实只剩一张"
        );

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
