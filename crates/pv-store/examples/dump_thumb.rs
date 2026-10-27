//! 一次性诊断：打印某张图 xl 档缩略图的实际尺寸与缓存路径。
//!
//! 用法：`cargo run -p pv-store --example dump_thumb -- <rel_name 前缀>`
//! 用完即删。

#[cfg(not(windows))]
fn main() {
    eprintln!("仅 Windows");
}

#[cfg(windows)]
fn main() {
    use pv_cache::ThumbTier;
    use pv_store::{Store, WindowsVolumeResolver};
    use std::path::PathBuf;

    let prefix = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "cover_6a20f918".to_string());

    let data: PathBuf = dirs_localappdata().join("PurePhotoExplorer");
    println!("数据目录：{}", data.display());

    let mut store = Store::open(&data, Box::new(WindowsVolumeResolver)).expect("打开服务层失败");
    let conn = store.conn();

    let mut stmt = conn
        .prepare("SELECT id, rel_name FROM images WHERE rel_name LIKE ?1 LIMIT 5")
        .expect("查询失败");
    let rows: Vec<(i64, String)> = stmt
        .query_map([format!("{prefix}%")], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("取值失败")
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);

    if rows.is_empty() {
        println!("没找到 rel_name 以 {prefix} 开头的图");
        return;
    }

    for (id, name) in rows {
        let Some(img) = pv_index::repo::images::get(store.conn(), id).ok().flatten() else {
            continue;
        };
        println!("\nimage#{id} {name}");
        println!("  索引尺寸: {:?}x{:?}", img.width, img.height);
        for tier in [ThumbTier::Small, ThumbTier::Medium, ThumbTier::Large, ThumbTier::Xlarge] {
            let path = store.cache_file(&img, tier);
            let exists = path.as_ref().map(|p| p.exists()).unwrap_or(false);
            let mut dims = String::from("-");
            if let Some(p) = path.as_ref().filter(|p| p.exists()) {
                if let Ok(bytes) = std::fs::read(p) {
                    if let Some((w, h)) = jpeg_dims(&bytes) {
                        dims = format!("{w}x{h}");
                    }
                }
            }
            println!(
                "  {:<3} 缓存: {:<10} 实际: {:<10} {}",
                tier.as_str(),
                dims,
                if exists { "存在" } else { "缺失" },
                path.map(|p| p.display().to_string()).unwrap_or_default()
            );
        }
        if let Ok(Some(t)) = store.thumbnail(&img, ThumbTier::Xlarge) {
            println!("  thumbnail(xl) 返回: {}x{}", t.width, t.height);
        }
    }
}

#[cfg(windows)]
fn dirs_localappdata() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("LOCALAPPDATA").expect("没有 LOCALAPPDATA"))
}

/// 从 JPEG 字节里读 SOF 尺寸。
#[cfg(windows)]
fn jpeg_dims(b: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2;
    while i + 9 < b.len() {
        if b[i] != 0xFF {
            i += 1;
            continue;
        }
        let m = b[i + 1];
        if (0xC0..=0xCF).contains(&m) && m != 0xC4 && m != 0xC8 && m != 0xCC {
            let h = u16::from_be_bytes([b[i + 5], b[i + 6]]) as u32;
            let w = u16::from_be_bytes([b[i + 7], b[i + 8]]) as u32;
            return Some((w, h));
        }
        let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        i += 2 + len;
    }
    None
}
