//! `libraries` / `library_folders` 表仓储（图库与其成员文件夹）。

use crate::error::Result;
use crate::models::{Library, LibraryKind};
use crate::repo::parse_enum;
use rusqlite::{params, Connection, OptionalExtension};

const COLS: &str = "id, name, kind, rule, cover_image, vault_id, sort_field, created_ts";

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Library> {
    let raw: String = row.get(2)?;
    Ok(Library {
        id: row.get(0)?,
        name: row.get(1)?,
        kind: parse_enum(2, raw.clone(), LibraryKind::parse(&raw))?,
        rule: row.get(3)?,
        cover_image: row.get(4)?,
        vault_id: row.get(5)?,
        sort_field: row.get(6)?,
        created_ts: row.get(7)?,
    })
}

/// 新建图库，返回自增 id。
pub fn insert(conn: &Connection, lib: &Library) -> Result<i64> {
    conn.execute(
        "INSERT INTO libraries(name, kind, rule, cover_image, vault_id, sort_field, created_ts)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            lib.name,
            lib.kind.as_str(),
            lib.rule,
            lib.cover_image,
            lib.vault_id,
            lib.sort_field,
            lib.created_ts
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// 按 id 查图库。
pub fn get(conn: &Connection, id: i64) -> Result<Option<Library>> {
    let sql = format!("SELECT {COLS} FROM libraries WHERE id = ?1");
    Ok(conn.query_row(&sql, params![id], map_row).optional()?)
}

/// 列出全部图库。
pub fn list(conn: &Connection) -> Result<Vec<Library>> {
    let sql = format!("SELECT {COLS} FROM libraries ORDER BY id");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], map_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 更新智能图库的规则文本。
pub fn set_rule(conn: &Connection, id: i64, rule: Option<&str>) -> Result<()> {
    conn.execute("UPDATE libraries SET rule = ?2 WHERE id = ?1", params![id, rule])?;
    Ok(())
}

/// 设置封面图。
pub fn set_cover(conn: &Connection, id: i64, image_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE libraries SET cover_image = ?2 WHERE id = ?1",
        params![id, image_id],
    )?;
    Ok(())
}

/// 改图库名（**不动成员关系、不动任何文件**）。
pub fn rename(conn: &Connection, id: i64, name: &str) -> Result<()> {
    conn.execute(
        "UPDATE libraries SET name = ?2 WHERE id = ?1",
        params![id, name],
    )?;
    Ok(())
}

/// 删除图库（**只删定义与成员关系，不动任何文件**）。
pub fn delete(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM library_folders WHERE library_id = ?1", params![id])?;
    conn.execute("DELETE FROM libraries WHERE id = ?1", params![id])?;
    Ok(())
}

/// 把文件夹加入图库（重复加入自动忽略）。
pub fn add_folder(conn: &Connection, library_id: i64, folder_id: i64) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO library_folders(library_id, folder_id) VALUES (?1, ?2)",
        params![library_id, folder_id],
    )?;
    Ok(())
}

/// 从图库移除文件夹（不动文件）。
pub fn remove_folder(conn: &Connection, library_id: i64, folder_id: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM library_folders WHERE library_id = ?1 AND folder_id = ?2",
        params![library_id, folder_id],
    )?;
    Ok(())
}

/// 断开**某个目录**在**所有图库**里的成员关系。
///
/// `Store::remove_folder` 删目录行时必须调它：`library_folders` 的外键是
/// `NO ACTION` 且 SQLite 默认不开 `foreign_keys`，不显式删就会留下指向已删目录的
/// 脏行（界面靠 `byId.get(folderId)` 找不到而"看不见"，但库会一直变脏，
/// 且万一哪天目录 id 被复用就会串库）。
pub fn detach_folder(conn: &Connection, folder_id: i64) -> Result<usize> {
    let n = conn.execute(
        "DELETE FROM library_folders WHERE folder_id = ?1",
        params![folder_id],
    )?;
    Ok(n)
}

/// 列出图库的成员文件夹 id。
pub fn list_folder_ids(conn: &Connection, library_id: i64) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT folder_id FROM library_folders WHERE library_id = ?1 ORDER BY folder_id",
    )?;
    let rows = stmt.query_map(params![library_id], |r| r.get::<_, i64>(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 列出**所有**图库的成员关系 `(library_id, folder_id)`（侧栏二级树用，
/// 一次查询把全部图库的成员关系拿走，前端再和目录表按 id 拼装）。
pub fn list_members(conn: &Connection) -> Result<Vec<(i64, i64)>> {
    let mut stmt =
        conn.prepare("SELECT library_id, folder_id FROM library_folders ORDER BY library_id, folder_id")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}
