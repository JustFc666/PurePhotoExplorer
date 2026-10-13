//! PurePhotoExplorer 索引层：SQLite schema、缓存键、查询
//!
//! 本 crate 不含任何 IO 副作用的类型定义，可独立单测。

// 测试函数名用中文（可读性优先），关闭 snake_case 检查
#![cfg_attr(test, allow(non_snake_case))]

pub mod cache_key;

pub use cache_key::{compute_key, CacheKey, CacheSource, ThumbTier};
