//! `image_exif` 表仓储：入库后回填的拍摄参数（品牌 / 机型 / 镜头 / GPS）。
//!
//! ⚠️ 扫描链路**不写这张表**（扫描要快）：它由 `Store::backfill_places` 在
//! 开文件读 EXIF 时顺带填上 —— 一次文件读取，两份收益（地点 + 搜索命中机型）。

use crate::error::Result;
use rusqlite::{params, Connection};

/// 写入一条拍摄参数（重复写覆盖）。全部字段可空 —— 没 GPS 的图也要落行，
/// 否则「搜机型」对老库永远无效。
pub fn upsert(
    conn: &Connection,
    image_id: i64,
    make: Option<&str>,
    model: Option<&str>,
    lens: Option<&str>,
    gps_lat: Option<f64>,
    gps_lon: Option<f64>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO image_exif(image_id, make, model, lens, gps_lat, gps_lon)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(image_id) DO UPDATE SET make = excluded.make,
                                             model = excluded.model,
                                             lens = excluded.lens,
                                             gps_lat = excluded.gps_lat,
                                             gps_lon = excluded.gps_lon",
        params![image_id, make, model, lens, gps_lat, gps_lon],
    )?;
    Ok(())
}

/// 列出库里出现过的相机机型及张数（给顶部「相机」下拉用，按张数降序）。
pub fn list_models(conn: &Connection, limit: i64) -> Result<Vec<(String, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT model, COUNT(*) AS n
           FROM image_exif
          WHERE model IS NOT NULL AND model <> ''
          GROUP BY model
          ORDER BY n DESC, model
          LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}
