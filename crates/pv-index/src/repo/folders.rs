//! `folders` 表仓储。

use crate::error::Result;
use crate::models::{EntryState, Folder};
use crate::repo::parse_enum;
use rusqlite::{params, Connection, OptionalExtension};

const COLS: &str =
    "id, volume_id, rel_path, parent_id, is_root, ignored, vault_id, file_count, state";

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Folder> {
    let raw: String = row.get(8)?;
    Ok(Folder {
        id: row.get(0)?,
        volume_id: row.get(1)?,
        rel_path: row.get(2)?,
        parent_id: row.get(3)?,
        is_root: row.get::<_, i64>(4)? != 0,
        ignored: row.get::<_, i64>(5)? != 0,
        vault_id: row.get(6)?,
        file_count: row.get(7)?,
        state: parse_enum(8, raw.clone(), EntryState::parse(&raw))?,
    })
}

/// 插入文件夹，返回自增 id。
pub fn insert(conn: &Connection, f: &Folder) -> Result<i64> {
    conn.execute(
        "INSERT INTO folders
           (volume_id, rel_path, parent_id, is_root, ignored, vault_id, file_count, state)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            f.volume_id,
            f.rel_path,
            f.parent_id,
            f.is_root as i64,
            f.ignored as i64,
            f.vault_id,
            f.file_count,
            f.state.as_str()
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// 按 id 查。
pub fn get(conn: &Connection, id: i64) -> Result<Option<Folder>> {
    let sql = format!("SELECT {COLS} FROM folders WHERE id = ?1");
    Ok(conn.query_row(&sql, params![id], map_row).optional()?)
}

/// 按（卷 + 相对路径）查。
pub fn find(conn: &Connection, volume_id: i64, rel_path: &str) -> Result<Option<Folder>> {
    let sql = format!("SELECT {COLS} FROM folders WHERE volume_id = ?1 AND rel_path = ?2");
    Ok(conn
        .query_row(&sql, params![volume_id, rel_path], map_row)
        .optional()?)
}

/// 按（卷 + 相对路径）**取或建**；已存在则返回其 id（不覆盖既有 parent）。
pub fn upsert(conn: &Connection, f: &Folder) -> Result<i64> {
    if let Some(existing) = find(conn, f.volume_id, &f.rel_path)? {
        return Ok(existing.id);
    }
    insert(conn, f)
}

/// 列出某卷下的文件夹。
pub fn list_by_volume(conn: &Connection, volume_id: i64) -> Result<Vec<Folder>> {
    let sql = format!("SELECT {COLS} FROM folders WHERE volume_id = ?1 ORDER BY rel_path");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![volume_id], map_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 删除一个文件夹行。
///
/// ⚠️ **只删这一行**：它下面的图像与子目录由调用方负责先处理干净
/// （`folders.parent_id` 指着父行，先删父会留下悬空的子行）。
///
/// 顺带清掉 `library_folders` 里的挂载关系 —— 留着会指向不存在的目录，
/// 图库条目数会算出一个诡异的数字。
pub fn delete(conn: &Connection, id: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM library_folders WHERE folder_id = ?1",
        params![id],
    )?;
    conn.execute("DELETE FROM folders WHERE id = ?1", params![id])?;
    Ok(())
}

/// 把某卷下所有文件夹置为某状态（拔出 → offline）。返回受影响行数。
pub fn set_state_by_volume(conn: &Connection, volume_id: i64, state: EntryState) -> Result<usize> {
    let n = conn.execute(
        "UPDATE folders SET state = ?1 WHERE volume_id = ?2",
        params![state.as_str(), volume_id],
    )?;
    Ok(n)
}
