//! 统一错误类型。所有 crate 共用，便于上层一次性 match。
//!
//! 设计原则：解码失败**永远不 panic**（前端要求 FR-1.6），
//! 任何单文件问题都以 Err 返回，由上层降级为占位图。

use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, PvError>;

#[derive(Debug, thiserror::Error)]
pub enum PvError {
    #[error("文件不存在: {0}")]
    NotFound(PathBuf),

    #[error("权限不足: {0}")]
    PermissionDenied(PathBuf),

    #[error("IO 错误 ({path}): {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// 格式已识别但当前版本不支持（界面上应显示"暂不支持"，非崩溃）
    #[error("暂不支持该格式: {0}")]
    UnsupportedFormat(String),

    /// 文件结构损坏或已截断
    #[error("文件损坏: {0}")]
    Corrupt(String),

    #[error("解码失败: {0}")]
    Decode(String),

    /// 编码/写回失败（画布调整后覆盖原图时）
    #[error("保存失败: {0}")]
    Encode(String),

    /// 目标格式不支持写回（含格式名与原因，直接展示给用户）
    #[error("{0}")]
    NotWritable(String),

    /// 加密容器相关
    #[error("容器已锁定或密码错误")]
    VaultLocked,

    #[error("容器文件损坏: {0}")]
    VaultCorrupt(String),

    /// 二期 RAW 开发流程（一期不实现）
    #[error("RAW 开发流程尚未实现（一期仅支持浏览）")]
    RawDevelopNotImplemented,

    #[error("操作超时: {0}")]
    Timeout(String),

    #[error("内存预算超限: 需要 {needed} MB，预算 {budget} MB")]
    MemoryBudget { needed: u64, budget: u64 },

    #[error("{0}")]
    Other(String),
}

impl PvError {
    /// 是否为"用户可理解"错误（可安全展示到界面）
    pub fn is_user_visible(&self) -> bool {
        matches!(
            self,
            Self::UnsupportedFormat(_)
                | Self::Corrupt(_)
                | Self::VaultLocked
                | Self::RawDevelopNotImplemented
                | Self::PermissionDenied(_)
                | Self::MemoryBudget { .. }
                | Self::Encode(_)
                | Self::NotWritable(_)
        )
    }

    /// 是否可重试（供队列决定是否重排）
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Io { .. } | Self::Timeout(_) | Self::MemoryBudget { .. })
    }
}
