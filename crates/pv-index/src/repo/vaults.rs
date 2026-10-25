//! `vaults` 表仓储（加密容器注册信息）。

use crate::error::Result;
use crate::models::Vault;
use rusqlite::{params, Connection, OptionalExtension};

const COLS: &str = "id, name, kdf_params, salt, verifier, created_ts, last_unlock_ts";

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Vault> {
    Ok(Vault {
        id: row.get(0)?,
        name: row.get(1)?,
        kdf_params: row.get(2)?,
        salt: row.get(3)?,
        verifier: row.get(4)?,
        created_ts: row.get(5)?,
        last_unlock_ts: row.get(6)?,
    })
}

/// 新建容器记录，返回自增 id。
pub fn insert(conn: &Connection, v: &Vault) -> Result<i64> {
    conn.execute(
        "INSERT INTO vaults(name, kdf_params, salt, verifier, created_ts, last_unlock_ts)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![v.name, v.kdf_params, v.salt, v.verifier, v.created_ts, v.last_unlock_ts],
    )?;
    Ok(conn.last_insert_rowid())
}

/// 按 id 查容器。
pub fn get(conn: &Connection, id: i64) -> Result<Option<Vault>> {
    let sql = format!("SELECT {COLS} FROM vaults WHERE id = ?1");
    Ok(conn.query_row(&sql, params![id], map_row).optional()?)
}

/// 列出全部容器。
pub fn list(conn: &Connection) -> Result<Vec<Vault>> {
    let sql = format!("SELECT {COLS} FROM vaults ORDER BY id");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], map_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 记录最近一次解锁时间（未解锁用 `None` 清除）。
pub fn set_last_unlock(conn: &Connection, id: i64, ts: Option<i64>) -> Result<()> {
    conn.execute(
        "UPDATE vaults SET last_unlock_ts = ?2 WHERE id = ?1",
        params![id, ts],
    )?;
    Ok(())
}
