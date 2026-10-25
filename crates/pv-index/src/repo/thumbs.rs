//! `thumbs` 表仓储（缩略图索引；像素本体在 L2 磁盘缓存）。

use crate::error::Result;
use crate::models::Thumb;
use rusqlite::{params, Connection, OptionalExtension};

/// 写入或覆盖某图某档的缩略图记录（主键 = image_id + tier）。
pub fn upsert(conn: &Connection, t: &Thumb) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO thumbs(image_id, tier, cache_path, bytes, generated_ts)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![t.image_id, t.tier, t.cache_path, t.bytes, t.generated_ts],
    )?;
    Ok(())
}

/// 查某图某档的记录。
pub fn get(conn: &Connection, image_id: i64, tier: i64) -> Result<Option<Thumb>> {
    Ok(conn
        .query_row(
            "SELECT image_id, tier, cache_path, bytes, generated_ts
               FROM thumbs WHERE image_id = ?1 AND tier = ?2",
            params![image_id, tier],
            |r| {
                Ok(Thumb {
                    image_id: r.get(0)?,
                    tier: r.get(1)?,
                    cache_path: r.get(2)?,
                    bytes: r.get(3)?,
                    generated_ts: r.get(4)?,
                })
            },
        )
        .optional()?)
}

/// 删除某图的全部档位记录，返回删除条数。
pub fn delete_image(conn: &Connection, image_id: i64) -> Result<usize> {
    Ok(conn.execute("DELETE FROM thumbs WHERE image_id = ?1", params![image_id])?)
}

/// 缩略图缓存已记录的总字节数（容量控制/统计用）。
pub fn total_bytes(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT COALESCE(SUM(bytes), 0) FROM thumbs", [], |r| r.get(0))?)
}
