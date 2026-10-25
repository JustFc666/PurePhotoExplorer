//! L1：内存缩略图缓存（按**字节预算**的 LRU）。
//!
//! 键是 [`CacheKey`]（已含档位），值是解码好的 RGBA 位图。
//! 锁定加密图库时记得调 [`L1::clear`]（手册 §6.4）。

use pv_index::CacheKey;
use std::sync::{Arc, Mutex, MutexGuard};

/// 内存中的一张缩略图（RGBA8）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedImage {
    pub width: u32,
    pub height: u32,
    /// RGBA8，长度应等于 `width * height * 4`
    pub rgba: Arc<[u8]>,
}

impl CachedImage {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        Self {
            width,
            height,
            rgba: Arc::from(rgba.into_boxed_slice()),
        }
    }

    /// 占用字节数。
    pub fn nbytes(&self) -> usize {
        self.rgba.len()
    }
}

struct Inner {
    map: lru::LruCache<CacheKey, CachedImage>,
    bytes: usize,
    budget: usize,
}

/// L1 缓存（线程安全）。
pub struct L1 {
    inner: Mutex<Inner>,
}

impl L1 {
    /// 默认预算 512 MB（手册 §6.1）。
    pub const DEFAULT_BUDGET: usize = 512 * 1024 * 1024;

    pub fn new(budget_bytes: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                map: lru::LruCache::unbounded(),
                bytes: 0,
                budget: budget_bytes,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // 中毒也继续用：缓存数据不影响正确性
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn budget(&self) -> usize {
        self.lock().budget
    }

    pub fn used(&self) -> usize {
        self.lock().bytes
    }

    pub fn len(&self) -> usize {
        self.lock().map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 取（命中会提升为最近使用）。
    pub fn get(&self, key: &CacheKey) -> Option<CachedImage> {
        self.lock().map.get(key).cloned()
    }

    /// 放入（超预算时按 LRU 淘汰）。
    pub fn put(&self, key: CacheKey, img: CachedImage) {
        let mut g = self.lock();
        let n = img.nbytes();
        if let Some(old) = g.map.put(key, img) {
            g.bytes = g.bytes.saturating_sub(old.nbytes());
        }
        g.bytes += n;
        // 淘汰到预算内
        while g.bytes > g.budget {
            match g.map.pop_lru() {
                Some((_, evicted)) => g.bytes = g.bytes.saturating_sub(evicted.nbytes()),
                None => break,
            }
        }
    }

    /// 移除一个键。
    pub fn remove(&self, key: &CacheKey) {
        let mut g = self.lock();
        if let Some(old) = g.map.pop(key) {
            g.bytes = g.bytes.saturating_sub(old.nbytes());
        }
    }

    /// 清空（**加密图库锁定前必须调用**，防止明文位图滞留内存）。
    pub fn clear(&self) {
        let mut g = self.lock();
        g.map.clear();
        g.bytes = 0;
    }
}
