//! `fts_images` 全文索引的同步辅助。
//!
//! **约定：用 `image_id` 作为 FTS5 的 `rowid`**——这样按 `image_id` 更新/删除是
//! 精确操作，不会残留"幽灵"条目。
//!
//! ⚠️ 中文分词：FTS5 默认 `unicode61` 不切中文。当前把关键词以**空格分隔**存入
//! （AI 产出的关键词本身就是分词的），故 `MATCH '日落'` 可命中；若将来要按
//! 任意子串检索，需接自定义分词器（超出本阶段范围）。

use crate::error::Result;
use rusqlite::{params, Connection};

/// 新建或覆盖某张图的全文条目。
pub fn upsert(
    conn: &Connection,
    image_id: i64,
    filename: &str,
    keywords: &str,
    ocr_text: &str,
    place: &str,
) -> Result<()> {
    // 先删后插，等价于 upsert（FTS5 对 rowid 的删除是精确的）。
    delete(conn, image_id)?;
    conn.execute(
        "INSERT INTO fts_images(rowid, image_id, filename, keywords, ocr_text, place)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![image_id, image_id, filename, keywords, ocr_text, place],
    )?;
    Ok(())
}

/// 删除某张图的全文条目。
pub fn delete(conn: &Connection, image_id: i64) -> Result<()> {
    conn.execute("DELETE FROM fts_images WHERE rowid = ?1", params![image_id])?;
    Ok(())
}

/// 关键词检索，返回命中的 `image_id`（按相关性排序）。
pub fn search(conn: &Connection, query: &str, limit: i64) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT rowid FROM fts_images WHERE fts_images MATCH ?1 ORDER BY rank LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![query, limit], |r| r.get::<_, i64>(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}
