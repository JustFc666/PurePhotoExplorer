//! `image_places` 表仓储：每张图解析出的**离线城市名**。
//!
//! 约定（与 schema v3 的注释一致）：
//! - **有行** = 已经解析过；`place` 为 `NULL` 表示没有 GPS / 附近没有城市；
//! - **没有行** = 还没解析，[`list_ids_missing`] 会把它挑出来给回填用。
//!
//! ⛔ 本模块**不联网**：坐标 → 城市名一律走 `pv-geo` 内置的离线表。

use crate::error::Result;
use rusqlite::{params, Connection};

/// 写入一条解析结果（重复写覆盖）。
pub fn upsert(
    conn: &Connection,
    image_id: i64,
    place: Option<&str>,
    lat: Option<f64>,
    lon: Option<f64>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO image_places(image_id, place, lat, lon) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(image_id) DO UPDATE SET place = excluded.place,
                                            lat = excluded.lat,
                                            lon = excluded.lon",
        params![image_id, place, lat, lon],
    )?;
    Ok(())
}

/// 一个地点的照片数（侧栏「地点」分区用）。**只统计有地点的行**。
pub fn list_counts(conn: &Connection) -> Result<Vec<(String, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT place, COUNT(*) AS n FROM image_places
          WHERE place IS NOT NULL AND place <> ''
          GROUP BY place ORDER BY n DESC, place",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 还没解析过地点的图像 id（回填用，`limit` 控制一轮做多少）。
pub fn list_ids_missing(conn: &Connection, limit: i64) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT images.id FROM images
          WHERE images.id NOT IN (SELECT image_id FROM image_places)
          LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit], |r| r.get::<_, i64>(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 待解析的「图 + 坐标」：有 GPS 但还没解析过地点。
///
/// ⭐ 坐标直接来自 `image_exif`（入库时写好的），**不用再开文件读 EXIF** ——
/// 回填地点是纯 SQL + 离线表查表，一万张也就几百毫秒。
pub fn list_gps_missing(conn: &Connection, limit: i64) -> Result<Vec<(i64, f64, f64)>> {
    let mut stmt = conn.prepare(
        "SELECT e.image_id, e.gps_lat, e.gps_lon FROM image_exif e
          WHERE e.gps_lat IS NOT NULL AND e.gps_lon IS NOT NULL
            AND e.image_id NOT IN (SELECT image_id FROM image_places)
          LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?, r.get::<_, f64>(2)?))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 还剩多少张没解析（用于「补完了没有」的判断与状态栏提示）。
pub fn count_missing(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM images
          WHERE images.id NOT IN (SELECT image_id FROM image_places)",
        [],
        |r| r.get(0),
    )?)
}

/// 还剩多少张「有 GPS 但没解析」的图。
///
/// ⭐ 回填进度要用这个，不能用 [`count_missing`] —— 后者把没 GPS 的图也算进去，
/// 而没 GPS 的图永远解析不出地点，进度条永远到不了 100%。
pub fn count_gps_missing(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM image_exif e
          WHERE e.gps_lat IS NOT NULL AND e.gps_lon IS NOT NULL
            AND e.image_id NOT IN (SELECT image_id FROM image_places)",
        [],
        |r| r.get(0),
    )?)
}
/// 删除所有「解析过但没得出地点」的行（`place IS NULL`）。
///
/// 用途：解析器修 bug 之后这些行是**死行** —— 它们让 [`list_ids_missing`] 认为
/// 「这张图已经查过了」，永远不会重试。一次性清掉，让回填重新读一遍 EXIF。
/// （2026-10-20 GPS 写成 SRATIONAL 的兼容修复用）
pub fn delete_unresolved(conn: &Connection) -> Result<usize> {
    Ok(conn.execute("DELETE FROM image_places WHERE place IS NULL", [])?)
}

/// 删除某张图的解析结果（图像行被删时随之清理）。
pub fn delete(conn: &Connection, image_id: i64) -> Result<()> {
    conn.execute("DELETE FROM image_places WHERE image_id = ?1", params![image_id])?;
    Ok(())
}
