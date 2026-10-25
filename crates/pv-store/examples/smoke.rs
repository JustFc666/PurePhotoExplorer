//! 冒烟：**扫描一个真实目录 → 建索引 → 查询 → 生成一张缩略图**。
//!
//! 用法：`cargo run -p pv-store --example smoke -- <目录>`
//! （缺省用仓库的 `test-image/`）
//!
//! 仅 Windows：卷身份用 Win32 实现。

#[cfg(not(windows))]
fn main() {
    eprintln!("本示例目前仅支持 Windows（卷身份用 Win32 实现）");
}

#[cfg(windows)]
fn main() {
    use pv_cache::ThumbTier;
    use pv_store::{ImageFilter, ImageQuery, SortDir, SortKey, Store, WindowsVolumeResolver};
    use std::path::PathBuf;

    let dir = std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../test-image"))
    });
    println!("扫描目录：{}", dir.display());

    let data = std::env::temp_dir().join(format!("pv_store_smoke_{}", std::process::id()));
    let mut store = Store::open(&data, Box::new(WindowsVolumeResolver)).expect("打开服务层失败");

    let t = store.scan_tree(&dir).expect("扫描失败");
    println!(
        "建索引：目录 {} 个｜图像 +{} ~{} -{}",
        t.folders, t.images.added, t.images.changed, t.images.removed
    );

    let q = ImageQuery {
        sort: SortKey::RelName,
        dir: SortDir::Asc,
        ..Default::default()
    };
    let imgs = store.query(&q).expect("查询失败");
    let total = store.count(&ImageFilter::default()).expect("计数失败");
    println!("查询：本次 {} 张｜全库 {} 张", imgs.len(), total);

    // 挑一张常规格式生成缩略图
    let pick = imgs.iter().find(|i| {
        matches!(
            i.format.as_deref(),
            Some("jpeg") | Some("png") | Some("webp") | Some("bmp") | Some("tiff")
        )
    });
    match pick {
        Some(img) => {
            let name = img.rel_name.clone().unwrap_or_default();
            match store.thumbnail(img, ThumbTier::Small) {
                Ok(Some(th)) => println!(
                    "缩略图 {name}：{}×{}，RGBA {} 字节（L1 命中数 {}）",
                    th.width,
                    th.height,
                    th.rgba.len(),
                    store.l1_len()
                ),
                Ok(None) => println!("缩略图 {name}：卷未挂载，拿不到绝对路径"),
                Err(e) => println!("缩略图 {name}：失败 {e}"),
            }
        }
        None => println!("没有可生成缩略图的常规格式样本"),
    }

    drop(store);
    let _ = std::fs::remove_dir_all(&data);
}
