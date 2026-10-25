//! 索引层错误类型。

use thiserror::Error;

/// 索引层统一错误。
#[derive(Debug, Error)]
pub enum IndexError {
    /// SQLite 原生错误。
    #[error("SQLite 错误：{0}")]
    Sqlite(#[from] rusqlite::Error),

    /// 数据库 schema 版本比本程序支持的更新（防止旧程序写坏新库）。
    #[error("schema 版本过新：库为 v{found}，本程序仅支持到 v{supported}")]
    SchemaTooNew { found: i64, supported: i64 },

    /// 取值/枚举字符串无法识别。
    #[error("无法识别的取值：{0}")]
    BadValue(String),

    /// IO 错误（扫描/读取文件时）。
    #[error("IO 错误：{0}")]
    Io(#[from] std::io::Error),
}

/// 便捷别名。
pub type Result<T> = std::result::Result<T, IndexError>;
