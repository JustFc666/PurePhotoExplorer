//! 仓储层：按表拆分的最小 CRUD。
//!
//! 设计：每个函数都是 `(&Connection, ...) -> Result<...>` 的纯函数，
//! 不持有连接、不管事务，便于组合与单测。事务由调用方（`pv-scan`）用
//! [`conn::with_tx`](crate::conn::with_tx) 控制。

pub mod exif;
pub mod faces;
pub mod folders;
pub mod fts;
pub mod images;
pub mod libraries;
pub mod persons;
pub mod places;
pub mod query;
pub mod settings;
pub mod tags;
pub mod thumbs;
pub mod vaults;
pub mod volumes;

use crate::error::IndexError;

/// 生成 `?,?,?` 这样的占位符串，配合 [`rusqlite::params_from_iter`] 做批量 `IN`。
///
/// ⚠️ 只拼 `?`，绝不把值拼进 SQL —— 参数仍走绑定，避免注入。
pub(crate) fn placeholders(n: usize) -> String {
    let mut s = String::with_capacity(n.saturating_mul(2));
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        s.push('?');
    }
    s
}

/// 把库里的枚举字符串转成枚举；失败时包装为可诊断的 SQL 转换错误。
pub(crate) fn parse_enum<T>(
    col: usize,
    raw: String,
    parsed: Option<T>,
) -> rusqlite::Result<T> {
    parsed.ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            col,
            rusqlite::types::Type::Text,
            Box::new(IndexError::BadValue(raw)),
        )
    })
}
