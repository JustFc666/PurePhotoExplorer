//! PurePhotoExplorer 缓存层：**L1 内存 LRU** + **L2 磁盘缩略图库**。
//!
//! 设计原则：本 crate **只管「键 → 位图/字节」**，不做图像编解码，
//! 因此**不依赖 `pv-decode`**（缩略图的生成/编码由上层完成）。
//!
//! 缓存键复用 [`pv_index::cache_key`]（`volume_guid+rel_path+size+mtime_ns`，**不含盘符**，
//! 移动硬盘换盘符后缓存仍命中）。
//!
//! - [`l1`]：内存 LRU（默认 512 MB 预算；锁定加密图库前必须 `clear()`）
//! - [`l2`]：磁盘缩略图库（前 2 位 hex 分片 + 原子写；按容量 LRU 淘汰）。
//!
//! ⚠️ **尚未实现**：L3（原图 mmap 局部解码）、加密图库缩略图的"落盘前加密"（由上层做，
//! 只需把密文字节交给 `L2::put`）。

#![cfg_attr(test, allow(non_snake_case))]

pub mod l1;
pub mod l2;

pub use l1::{CachedImage, L1};
pub use l2::L2;

pub use pv_index::cache_key::{compute_key, CacheKey, CacheSource, ThumbTier};
pub use pv_index::Result;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn img(n: usize) -> CachedImage {
        let px = (n / 4).max(1) as u32;
        CachedImage {
            width: 1,
            height: px,
            rgba: Arc::from(vec![0u8; n].into_boxed_slice()),
        }
    }

    fn key(i: u8) -> CacheKey {
        CacheKey([i; 20])
    }

    // ── L1 ──

    #[test]
    fn L1按字节预算淘汰() {
        let l1 = L1::new(300);
        l1.put(key(1), img(100));
        l1.put(key(2), img(100));
        l1.put(key(3), img(100));
        assert_eq!(l1.used(), 300);
        assert_eq!(l1.len(), 3);

        // 再加一张 → 挤掉最久未用的 key(1)
        l1.put(key(4), img(100));
        assert_eq!(l1.len(), 3, "预算内最多放 3 张");
        assert_eq!(l1.used(), 300);
        assert!(l1.get(&key(1)).is_none(), "最久未用应被淘汰");
        assert!(l1.get(&key(4)).is_some());
    }

    #[test]
    fn L1命中会提升热度() {
        let l1 = L1::new(300);
        l1.put(key(1), img(100));
        l1.put(key(2), img(100));
        l1.put(key(3), img(100));
        // 触摸 key(1) → 变最近使用；此时最久未用是 key(2)
        assert!(l1.get(&key(1)).is_some());
        l1.put(key(4), img(100));
        assert!(l1.get(&key(2)).is_none(), "应淘汰 key(2) 而非 key(1)");
        assert!(l1.get(&key(1)).is_some());
    }

    #[test]
    fn L1移除与清空() {
        let l1 = L1::new(1000);
        l1.put(key(9), img(64));
        assert_eq!(l1.used(), 64);
        l1.remove(&key(9));
        assert_eq!(l1.used(), 0);
        assert!(l1.is_empty());

        l1.put(key(9), img(64));
        l1.clear();
        assert_eq!(l1.len(), 0);
        assert_eq!(l1.used(), 0);
    }

    #[test]
    fn L1单张超预算则不缓存() {
        let l1 = L1::new(50);
        l1.put(key(1), img(400));
        assert_eq!(l1.len(), 0, "单张就超预算 → 放入后立即被淘汰");
        assert_eq!(l1.used(), 0);
    }

    // ── L2 ──

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Self {
            let mut p = std::env::temp_dir();
            p.push(format!("pv_cache_{tag}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn plain() -> CacheSource {
        CacheSource::Plain {
            path: PathBuf::from("C:/photos/a.jpg"),
            size: 100,
            mtime_ns: 1,
        }
    }

    #[test]
    fn L2存取覆盖与删除() {
        let t = Tmp::new("l2");
        let l2 = L2::new(&t.0);
        let k = compute_key(&plain(), ThumbTier::Small);

        assert!(!l2.has(&k));
        assert!(l2.get(&k).unwrap().is_none());

        l2.put(&k, b"AAA").unwrap();
        assert!(l2.has(&k));
        assert_eq!(l2.get(&k).unwrap().unwrap(), b"AAA");
        assert_eq!(l2.total_bytes().unwrap(), 3);

        l2.put(&k, b"BBBB").unwrap(); // 覆盖（原子改名）
        assert_eq!(l2.get(&k).unwrap().unwrap(), b"BBBB");
        assert_eq!(l2.total_bytes().unwrap(), 4);

        assert!(l2.remove(&k).unwrap());
        assert!(!l2.has(&k));
        assert!(!l2.remove(&k).unwrap(), "再删应为 false");
        assert_eq!(l2.total_bytes().unwrap(), 0);
    }

    #[test]
    fn L2不同档位落不同文件() {
        let t = Tmp::new("tier");
        let l2 = L2::new(&t.0);
        let a = l2.path_of(&compute_key(&plain(), ThumbTier::Small));
        let b = l2.path_of(&compute_key(&plain(), ThumbTier::Large));
        assert_ne!(a, b, "档位不同 → 缓存键与文件都不同");
    }

    #[test]
    fn L2文件变化后旧缓存不可命中() {
        let t = Tmp::new("invalidate");
        let l2 = L2::new(&t.0);
        let k1 = compute_key(&plain(), ThumbTier::Small);
        l2.put(&k1, b"old").unwrap();

        // 文件被改（size 变）→ 键也变 → 旧缓存不该被命中
        let changed = CacheSource::Plain {
            path: PathBuf::from("C:/photos/a.jpg"),
            size: 101,
            mtime_ns: 1,
        };
        let k2 = compute_key(&changed, ThumbTier::Small);
        assert_ne!(k1, k2);
        assert!(l2.get(&k2).unwrap().is_none());
    }

    #[test]
    fn L2按容量淘汰最旧的() {
        let t = Tmp::new("evict");
        let l2 = L2::new(&t.0);
        for i in [1u8, 2, 3] {
            l2.put(&key(i), &[0u8; 100]).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(30)); // 拉开 mtime
        }
        assert_eq!(l2.total_bytes().unwrap(), 300);

        let freed = l2.evict_to_limit(150).unwrap();
        assert!(freed >= 100, "应至少释放 100 字节，实际 {freed}");
        assert!(l2.total_bytes().unwrap() <= 150);
        assert!(l2.has(&key(3)), "最新的应保留");

        // 已在上限内 → 再淘汰不做事
        assert_eq!(l2.evict_to_limit(150).unwrap(), 0);
    }
}
