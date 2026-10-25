//! `pv-lib`：**图库领域模型**（普通 / 智能图库）。
//!
//! - [`library`]：图库 CRUD、成员求值、计数
//! - [`rule`]：智能图库的**规则 DSL**（解析成「结构化过滤 + 全文关键词」）
//!
//! ⚠️ **加密图库**（就地容器收纳 + 可自救，手册 §7.3）是 S2.11 的大工程：
//! 本 crate 只在类型层面认识它（`kind = encrypted`），操作返回明确错误。

#![cfg_attr(test, allow(non_snake_case))]

pub mod library;
pub mod rule;

pub use library::{count_of, create_normal, create_smart, list_summaries, members, LibrarySummary};
pub use rule::{parse_rule, Rule};

/// 图库层错误。
#[derive(Debug, thiserror::Error)]
pub enum LibError {
    #[error(transparent)]
    Index(#[from] pv_index::IndexError),
    /// 规则语法错误（带原文，便于前端提示）
    #[error("规则语法错误：{0}")]
    Rule(String),
    /// 本阶段尚未支持的能力（如加密图库）
    #[error("{0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, LibError>;
