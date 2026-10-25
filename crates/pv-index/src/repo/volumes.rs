//! `volumes` 表仓储。

use crate::error::Result;
use crate::models::{Volume, VolumeState};
use crate::repo::parse_enum;
use rusqlite::{params, Connection, OptionalExtension};

/// 查询列（顺序与 [`map_row`] 一致）。
const COLS: &str =
    "id, volume_guid, serial, label, fs, removable, last_mount_path, last_seen_ts, state";

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Volume> {
    let raw: String = row.get(8)?;
    Ok(Volume {
        id: row.get(0)?,
        volume_guid: row.get(1)?,
        serial: row.get(2)?,
        label: row.get(3)?,
        fs: row.get(4)?,
        removable: row.get::<_, i64>(5)? != 0,
        last_mount_path: row.get(6)?,
        last_seen_ts: row.get(7)?,
        state: parse_enum(8, raw.clone(), VolumeState::parse(&raw))?,
    })
}

/// 插入卷，返回自增 id。
pub fn insert(conn: &Connection, v: &Volume) -> Result<i64> {
    conn.execute(
        "INSERT INTO volumes
           (volume_guid, serial, label, fs, removable, last_mount_path, last_seen_ts, state)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            v.volume_guid,
            v.serial,
            v.label,
            v.fs,
            v.removable as i64,
            v.last_mount_path,
            v.last_seen_ts,
            v.state.as_str()
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// 按卷 GUID 查（可移动盘插回时用）。
pub fn get_by_guid(conn: &Connection, guid: &str) -> Result<Option<Volume>> {
    let sql = format!("SELECT {COLS} FROM volumes WHERE volume_guid = ?1");
    Ok(conn.query_row(&sql, params![guid], map_row).optional()?)
}

/// 列出全部卷。
pub fn list(conn: &Connection) -> Result<Vec<Volume>> {
    let sql = format!("SELECT {COLS} FROM volumes ORDER BY id");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], map_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 按 GUID **取或建**卷：已存在则更新可变字段（序列号/卷标/文件系统/最近盘符），返回 id。
///
/// 移动盘换盘符后重新挂载时，靠它把 `last_mount_path` 刷新成新盘符。
pub fn upsert(conn: &Connection, v: &Volume) -> Result<i64> {
    if let Some(existing) = get_by_guid(conn, &v.volume_guid)? {
        conn.execute(
            "UPDATE volumes SET serial=?2, label=?3, fs=?4, removable=?5, last_mount_path=?6
             WHERE id=?1",
            params![
                existing.id,
                v.serial,
                v.label,
                v.fs,
                v.removable as i64,
                v.last_mount_path
            ],
        )?;
        return Ok(existing.id);
    }
    insert(conn, v)
}

/// 按 id 查卷。
pub fn get(conn: &Connection, id: i64) -> Result<Option<Volume>> {
    let sql = format!("SELECT {COLS} FROM volumes WHERE id = ?1");
    Ok(conn.query_row(&sql, params![id], map_row).optional()?)
}

/// 设置卷状态（拔出 → offline / 插回 → online）。
pub fn set_state(conn: &Connection, id: i64, state: VolumeState) -> Result<()> {
    conn.execute(
        "UPDATE volumes SET state = ?1 WHERE id = ?2",
        params![state.as_str(), id],
    )?;
    Ok(())
}
