//! `images` 表仓储。

use crate::error::Result;
use crate::models::Image;
use rusqlite::{params, Connection, OptionalExtension};

pub(crate) const COLS: &str =
    "id, dir_id, rel_name, size, mtime_ns, width, height, format, is_raw, taken_at, rating, flag, content_key";

pub(crate) fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Image> {
    Ok(Image {
        id: row.get(0)?,
        dir_id: row.get(1)?,
        rel_name: row.get(2)?,
        size: row.get(3)?,
        mtime_ns: row.get(4)?,
        width: row.get(5)?,
        height: row.get(6)?,
        format: row.get(7)?,
        is_raw: row.get::<_, i64>(8)? != 0,
        taken_at: row.get(9)?,
        rating: row.get(10)?,
        flag: row.get(11)?,
        content_key: row.get(12)?,
        has_tags: false, // 只有 `query::list` 会额外算这一列
    })
}

/// 插入图像，返回自增 id。
pub fn insert(conn: &Connection, img: &Image) -> Result<i64> {
    conn.execute(
        // ⭐ `added_ts` 必须在这里写：年份筛选与「按入库时间」排序都依赖它。
        // 之前这里漏了，导致全库 added_ts 恒为 NULL —— 没有 EXIF 时间的图
        // 在年份筛选里会凭空消失（2026-10-21 修）。
        "INSERT INTO images
           (dir_id, rel_name, size, mtime_ns, width, height, format, is_raw, taken_at,
            rating, flag, content_key, added_ts)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                 CAST(strftime('%s','now') AS INTEGER))",
        params![
            img.dir_id,
            img.rel_name,
            img.size,
            img.mtime_ns,
            img.width,
            img.height,
            img.format,
            img.is_raw as i64,
            img.taken_at,
            img.rating,
            img.flag,
            img.content_key
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// 只补尺寸（**其它字段一概不动**）—— 回填宽高用。
///
/// 单独一条而不是复用 [`update_meta`]：回填只想改这两个字段，
/// 顺手把整行重写一遍容易把并发期间别处的修改盖掉。
pub fn update_dimensions(conn: &Connection, id: i64, width: i64, height: i64) -> Result<()> {
    conn.execute(
        "UPDATE images SET width = ?2, height = ?3 WHERE id = ?1",
        params![id, width, height],
    )?;
    Ok(())
}

/// 写入拍摄时间，并标记「这一行的 EXIF 已检查过」。
///
/// - `taken_at = Some(ts)` → 写入并标记
/// - `taken_at = None` → **只标记**（读过但没有拍摄时间，别下次再读一遍）
///
/// ⚠️ 单独一个函数而不是复用 `update_meta`：回填只该碰这两列，
/// 整行重写会把别处刚写的 rating/flag 盖掉（和尺寸回填同一个道理）。
pub fn set_taken_at(conn: &Connection, id: i64, taken_at: Option<i64>) -> Result<()> {
    conn.execute(
        "UPDATE images SET taken_at = COALESCE(?2, taken_at), meta_checked = 1 WHERE id = ?1",
        params![id, taken_at],
    )?;
    Ok(())
}

/// 还有多少张**还没检查过**拍摄时间（EXIF 回填用）。
///
/// ⚠️ 条件是 `meta_checked = 0` 而不是 `taken_at IS NULL`：
/// 后者会把「本来就没有 EXIF 日期」的行永远算作待补，每次启动重读一遍。
pub fn count_missing_taken(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM images WHERE meta_checked = 0",
        [],
        |r| r.get(0),
    )?)
}

/// 待检查拍摄时间的图像 id（按 id 升序取前 `limit` 个）。
pub fn list_ids_missing_taken(conn: &Connection, limit: i64) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM images WHERE meta_checked = 0 ORDER BY id LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 某个目录里还没检查过拍摄时间的图像 id。
pub fn list_ids_missing_taken_in_dir(conn: &Connection, dir_id: i64) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM images WHERE dir_id = ?1 AND meta_checked = 0 ORDER BY id",
    )?;
    let rows = stmt.query_map(params![dir_id], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 还有多少张没探到尺寸（回填进度 / 判断要不要再跑一轮）。
pub fn count_missing_dimensions(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM images WHERE width IS NULL OR height IS NULL",
        [],
        |r| r.get(0),
    )?)
}

/// 待回填的图像 id（缺尺寸的，按 id 升序取前 `limit` 个）。
pub fn list_ids_missing_dimensions(conn: &Connection, limit: i64) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM images WHERE width IS NULL OR height IS NULL ORDER BY id LIMIT ?1",
    )?;
    let ids = stmt
        .query_map(params![limit], |r| r.get::<_, i64>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(ids)
}

/// 按 id 查。
pub fn get(conn: &Connection, id: i64) -> Result<Option<Image>> {
    let sql = format!("SELECT {COLS} FROM images WHERE id = ?1");
    Ok(conn.query_row(&sql, params![id], map_row).optional()?)
}

/// 按 id 更新可变字段（扫描/编辑后回写）。
pub fn update_meta(conn: &Connection, img: &Image) -> Result<()> {
    conn.execute(
        "UPDATE images SET rel_name=?2, size=?3, mtime_ns=?4, width=?5, height=?6,
                           format=?7, is_raw=?8, taken_at=?9, rating=?10, flag=?11, content_key=?12
         WHERE id = ?1",
        params![
            img.id,
            img.rel_name,
            img.size,
            img.mtime_ns,
            img.width,
            img.height,
            img.format,
            img.is_raw as i64,
            img.taken_at,
            img.rating,
            img.flag,
            img.content_key
        ],
    )?;
    Ok(())
}

/// 列出某文件夹下的图像。
pub fn list_by_dir(conn: &Connection, dir_id: i64) -> Result<Vec<Image>> {
    let sql = format!("SELECT {COLS} FROM images WHERE dir_id = ?1 ORDER BY rel_name");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![dir_id], map_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 统计某文件夹下的图像数。
pub fn count_by_dir(conn: &Connection, dir_id: i64) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM images WHERE dir_id = ?1",
        params![dir_id],
        |r| r.get(0),
    )?)
}

/// 全库图像总数。
pub fn count(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM images", [], |r| r.get(0))?)
}

/// 每个文件夹的图像数（一次查询拿全，供侧栏树用）。
///
/// ⚠️ 刻意不用 [`count_by_dir`] 循环：侧栏要显示**所有**文件夹的计数，
/// 逐个查会变成 N+1（文件夹上千时明显）。这里一条 `GROUP BY` 解决。
/// 结果里**不含 0 张的文件夹**，调用方按 0 兜底。
pub fn counts_by_dir(conn: &Connection) -> Result<Vec<(i64, i64)>> {
    let mut stmt = conn.prepare("SELECT dir_id, COUNT(*) FROM images GROUP BY dir_id")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 按格式统计 `(格式, 张数, 总字节)`，张数降序。
///
/// 格式为 `NULL` 的（索引时没探嗅出格式）归到 `"?"`，不能丢 ——
/// 否则「全部 = 各格式之和」这条对不上，界面上的筛选条会让人怀疑。
pub fn stats_by_format(conn: &Connection) -> Result<Vec<(String, i64, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT COALESCE(format, '?') AS f, COUNT(*), COALESCE(SUM(size), 0)
           FROM images GROUP BY f ORDER BY COUNT(*) DESC, f",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 按缓存键查（用于"是否已索引过该文件"的快速判断）。
pub fn get_by_content_key(conn: &Connection, key: &str) -> Result<Option<Image>> {
    let sql = format!("SELECT {COLS} FROM images WHERE content_key = ?1 LIMIT 1");
    Ok(conn.query_row(&sql, params![key], map_row).optional()?)
}

/// 删除图像**及其附属**（缩略图 / EXIF / 标签 / AI 结果 / 全文条目）。
///
/// 建议调用方放在 [`conn::with_tx`](crate::conn::with_tx) 里以保证原子性。
pub fn delete(conn: &Connection, id: i64) -> Result<()> {
    for sql in [
        "DELETE FROM thumbs      WHERE image_id = ?1",
        "DELETE FROM image_tags  WHERE image_id = ?1",
        "DELETE FROM image_places WHERE image_id = ?1",
        "DELETE FROM ai_results  WHERE image_id = ?1",
        "DELETE FROM image_exif  WHERE image_id = ?1",
        "DELETE FROM fts_images  WHERE rowid    = ?1",
        "DELETE FROM images      WHERE id       = ?1",
    ] {
        conn.execute(sql, params![id])?;
    }
    Ok(())
}

/// 批量设置星级（0–5）。返回实际改动行数。
///
/// `ids` 为空时直接返回 0 —— 不要拼出 `IN ()` 这种非法 SQL。
/// 星级范围在这里夹一下：前端传越界值只应被忽略，不该写进库留下脏数据。
pub fn set_rating(conn: &Connection, ids: &[i64], rating: i64) -> Result<usize> {
    if ids.is_empty() {
        return Ok(0);
    }
    let rating = rating.clamp(0, 5);
    let sql = format!(
        "UPDATE images SET rating = ?1 WHERE id IN ({placeholders})",
        placeholders = crate::repo::placeholders(ids.len()),
    );
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::with_capacity(ids.len() + 1);
    params.push(Box::new(rating));
    for id in ids {
        params.push(Box::new(*id));
    }
    let n = conn.execute(
        &sql,
        rusqlite::params_from_iter(params.iter().map(|b| b.as_ref())),
    )?;
    Ok(n)
}

/// 批量设置旗标（0 = 无，1 = 保留）。返回实际改动行数。
pub fn set_flag(conn: &Connection, ids: &[i64], flag: i64) -> Result<usize> {
    if ids.is_empty() {
        return Ok(0);
    }
    let flag = flag.clamp(0, 1);
    let sql = format!(
        "UPDATE images SET flag = ?1 WHERE id IN ({placeholders})",
        placeholders = crate::repo::placeholders(ids.len()),
    );
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::with_capacity(ids.len() + 1);
    params.push(Box::new(flag));
    for id in ids {
        params.push(Box::new(*id));
    }
    let n = conn.execute(
        &sql,
        rusqlite::params_from_iter(params.iter().map(|b| b.as_ref())),
    )?;
    Ok(n)
}

/// 重命名后更新索引里的**文件名与内容键**。
///
/// ⚠️ `content_key` 参与缩略图缓存键（`volume_guid + rel_path + size + mtime_ns`），
/// 改名后必须一起换掉，否则还会命中旧名字的缓存；旧缓存文件由调用方删除。
pub fn rename(conn: &Connection, id: i64, rel_name: &str, content_key: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE images SET rel_name = ?2, content_key = ?3 WHERE id = ?1",
        params![id, rel_name, content_key],
    )?;
    Ok(())
}

/// 一次性回填 `added_ts`（老库全是 NULL，用 `mtime` 当近似值）。
///
/// 为什么用 mtime 而不是 now：`now` 会把所有老照片都算成「今年」，
/// 年份筛选立刻变成一堆假数据；文件修改时间虽然不等于「入库时间」，
/// 但它是**真实拍下来之后的时间点**，比 now 靠谱得多。
pub fn backfill_added_ts(conn: &Connection) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE images
            SET added_ts = CAST(mtime_ns / 1000000000 AS INTEGER)
          WHERE added_ts IS NULL AND mtime_ns IS NOT NULL",
        [],
    )?)
}
