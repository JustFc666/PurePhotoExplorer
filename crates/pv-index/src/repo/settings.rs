//! `settings` 键值表仓储（应用配置）。

use crate::error::Result;
use rusqlite::{params, Connection, OptionalExtension};

/// 写入或覆盖一个键。
pub fn set(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// 读取一个键（不存在返回 `None`）。
pub fn get(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM settings WHERE key = ?1", params![key], |r| {
            r.get(0)
        })
        .optional()?)
}

/// 读取一个键，不存在时返回默认值。
pub fn get_or(conn: &Connection, key: &str, default: &str) -> Result<String> {
    Ok(get(conn, key)?.unwrap_or_else(|| default.to_string()))
}

/// 删除一个键。
pub fn delete(conn: &Connection, key: &str) -> Result<()> {
    conn.execute("DELETE FROM settings WHERE key = ?1", params![key])?;
    Ok(())
}
