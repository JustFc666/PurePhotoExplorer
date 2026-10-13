//! PurePhotoExplorer 加密容器。
//!
//! **威胁模型：防离线翻阅**（见开发文档 §0.0 确认三）。
//! 攻击者拿到磁盘/文件副本后，无法读取内容、无法得知文件名。
//!
//! 明确不做：隐写、内存加密、反调试。
//! 密码学一律用成熟 crate，本 crate 只做组装，**禁止自研算法**。

// 测试函数名用中文（可读性优先），关闭 snake_case 检查
#![cfg_attr(test, allow(non_snake_case))]

pub mod format;

pub use format::{
    BackoffState, KdfAlgo, KdfParams, LockTrigger, SessionState, VaultEntryMeta, VaultHeader,
};
