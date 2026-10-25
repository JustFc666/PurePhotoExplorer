//! `tags` / `image_tags` 表仓储（标签与图像挂载）。

use crate::error::Result;
use crate::models::Tag;
use rusqlite::{params, Connection};

/// 取（必要时创建）同名标签，返回其 id。**同名幂等**（依赖 `tags.name UNIQUE`）。
pub fn ensure(conn: &Connection, name: &str, kind: Option<&str>) -> Result<i64> {
    conn.execute(
        "INSERT OR IGNORE INTO tags(name, kind) VALUES (?1, ?2)",
        params![name, kind],
    )?;
    Ok(conn.query_row("SELECT id FROM tags WHERE name = ?1", params![name], |r| r.get(0))?)
}

/// 列出全部标签。
pub fn list_all(conn: &Connection) -> Result<Vec<Tag>> {
    let mut stmt = conn.prepare("SELECT id, name, kind, color FROM tags ORDER BY name")?;
    let rows = stmt.query_map([], |r| {
        Ok(Tag {
            id: r.get(0)?,
            name: r.get(1)?,
            kind: r.get(2)?,
            color: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 给图像挂标签（同 image+tag+source 重复则覆盖置信度）。
pub fn add_image_tag(
    conn: &Connection,
    image_id: i64,
    tag_id: i64,
    source: &str,
    confidence: Option<f64>,
) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO image_tags(image_id, tag_id, source, confidence)
         VALUES (?1, ?2, ?3, ?4)",
        params![image_id, tag_id, source, confidence],
    )?;
    Ok(())
}

/// 移除图像的某个标签挂载。
pub fn remove_image_tag(conn: &Connection, image_id: i64, tag_id: i64, source: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM image_tags WHERE image_id = ?1 AND tag_id = ?2 AND source = ?3",
        params![image_id, tag_id, source],
    )?;
    Ok(())
}

/// 列出图像的全部标签：`(标签名, 来源, 置信度)`。
pub fn list_by_image(conn: &Connection, image_id: i64) -> Result<Vec<(String, String, Option<f64>)>> {
    let mut stmt = conn.prepare(
        "SELECT t.name, it.source, it.confidence
           FROM image_tags it JOIN tags t ON t.id = it.tag_id
          WHERE it.image_id = ?1
          ORDER BY t.name",
    )?;
    let rows = stmt.query_map(params![image_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 标签 + 使用次数（侧栏「标签」分区用）：`(id, name, count)`。
///
/// 排序：**用得多的在前**，同次数按名字（大小写不敏感）。
/// 从没用过的标签（count = 0）排最后 —— 它们只是建过没用，不该占前排。
pub fn list_with_counts(conn: &Connection) -> Result<Vec<(i64, String, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.name, COUNT(it.image_id) AS n
           FROM tags t LEFT JOIN image_tags it ON it.tag_id = t.id
          GROUP BY t.id, t.name
          ORDER BY n DESC, t.name COLLATE NOCASE",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 带范围的标签计数：只统计**某个图库**或**某个目录**里的图。
///
/// ⭐ 侧栏标签与 AI 标签条都要用它 —— 全库计数会让每个图库显示同一份数字
/// （2026-10-23 用户报的 bug：11 张的「二次元」和 286 张的「测试图库」数字一模一样）。
pub fn counts_scoped(
    conn: &Connection,
    library_id: Option<i64>,
    dir_id: Option<i64>,
) -> Result<Vec<(i64, String, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.name, COUNT(DISTINCT it.image_id) AS n
           FROM tags t
           JOIN image_tags it ON it.tag_id = t.id
           JOIN images i ON i.id = it.image_id
          WHERE (?1 IS NULL OR EXISTS (
                    SELECT 1 FROM library_folders lf
                     WHERE lf.library_id = ?1 AND lf.folder_id = i.dir_id))
            AND (?2 IS NULL OR i.dir_id = ?2)
          GROUP BY t.id, t.name
          ORDER BY n DESC, t.name COLLATE NOCASE",
    )?;
    let rows = stmt.query_map(rusqlite::params![library_id, dir_id], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 一级类目的**去重**图片数：`(一级类目, 图片张数)`。
///
/// ## 为什么不能在前端把二级加起来（2026-10-23 踩过）
/// 一张图常常同时挂「单人」「全身」，把二级的计数相加会**重复计数**：
/// 测试图库里人像组相加是 89，实际只有 52 张图。界面上写 89、点进去 52 张，
/// 用户会以为「点一级没有显示全部」。
///
/// `pairs` 是 `(标签名, 一级类目)` 的映射（同一个一级可以出现多次）。
pub fn group_counts(
    conn: &Connection,
    library_id: Option<i64>,
    dir_id: Option<i64>,
    pairs: &[(String, String)],
) -> Result<Vec<(String, i64)>> {
    if pairs.is_empty() {
        return Ok(Vec::new());
    }
    // ⚠️ 用 `?n` 显式编号：VALUES 里的占位符是动态个数的，
    // 匿名 `?` 与后面的范围参数混用会错位。
    let values = (0..pairs.len())
        .map(|i| format!("(?{}, ?{})", i * 2 + 1, i * 2 + 2))
        .collect::<Vec<_>>()
        .join(", ");
    let lib_at = pairs.len() * 2 + 1;
    let dir_at = lib_at + 1;
    let sql = format!(
        "WITH m(name, l1) AS (VALUES {values})
         SELECT m.l1, COUNT(DISTINCT it.image_id) AS n
           FROM m
           JOIN tags t ON t.name = m.name
           JOIN image_tags it ON it.tag_id = t.id
           JOIN images i ON i.id = it.image_id
          WHERE (?{lib_at} IS NULL OR EXISTS (
                    SELECT 1 FROM library_folders lf
                     WHERE lf.library_id = ?{lib_at} AND lf.folder_id = i.dir_id))
            AND (?{dir_at} IS NULL OR i.dir_id = ?{dir_at})
            AND it.source IN ('ai','ocr','face')
          GROUP BY m.l1
          ORDER BY n DESC"
    );
    let mut vals: Vec<rusqlite::types::Value> = Vec::with_capacity(pairs.len() * 2 + 2);
    for (name, l1) in pairs {
        vals.push(name.clone().into());
        vals.push(l1.clone().into());
    }
    vals.push(library_id.into());
    vals.push(dir_id.into());

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(vals.iter()), |r| {
        Ok((r.get(0)?, r.get(1)?))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 一批图像**各自**已有的标签名（标签选择器要判断"全选 / 半选 / 没有"）。
pub fn counts_for_images(conn: &Connection, ids: &[i64]) -> Result<Vec<(String, i64)>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let ph = vec!["?"; ids.len()].join(", ");
    let sql = format!(
        "SELECT t.name, COUNT(DISTINCT it.image_id)
           FROM image_tags it JOIN tags t ON t.id = it.tag_id
          WHERE it.image_id IN ({ph})
          GROUP BY t.id, t.name
          ORDER BY COUNT(DISTINCT it.image_id) DESC, t.name"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(ids.iter()), |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 改名。⚠️ 撞上同名标签会触发 `UNIQUE` 约束错误，由调用方翻译成人话。
pub fn rename(conn: &Connection, id: i64, name: &str) -> Result<()> {
    conn.execute("UPDATE tags SET name = ?2 WHERE id = ?1", params![id, name])?;
    Ok(())
}

/// 删除标签：**先拆挂载、再删标签行**。
///
/// ⛔ 只删 `image_tags` 的关系，照片一张不动（标签只是元数据）。
pub fn delete(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM image_tags WHERE tag_id = ?1", params![id])?;
    conn.execute("DELETE FROM tags WHERE id = ?1", params![id])?;
    Ok(())
}

/// 移除挂载，**不限来源**（用户手动点掉时，AI 打的标签也一起撤掉）。
pub fn remove_any(conn: &Connection, image_id: i64, tag_id: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM image_tags WHERE image_id = ?1 AND tag_id = ?2",
        params![image_id, tag_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::open_test_db;

    /// 造一张图（含卷、目录、图库），返回 `image_id`。
    fn seed(conn: &Connection, name: &str) -> i64 {
        conn.execute(
            "INSERT OR IGNORE INTO volumes(id, volume_guid) VALUES (1, 'v1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO folders(id, volume_id, rel_path) VALUES (1, 1, '\\')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO libraries(id, name, kind) VALUES (1, '测试图库', 'normal')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO library_folders(library_id, folder_id) VALUES (1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO images(dir_id, rel_name) VALUES (1, ?1)",
            params![name],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn mount(conn: &Connection, image: i64, tag: &str) {
        let id = ensure(conn, tag, Some("ai")).unwrap();
        add_image_tag(conn, image, id, "ai", Some(0.42)).unwrap();
    }

    /// 一张图同时挂「单人」「全身」时，人像类目只算 **1 张**。
    ///
    /// 这是用户 2026-10-23 报的 bug：标签条写「人像 89」，点进去只有 52 张 ——
    /// 因为前端把二级的张数直接相加，同一张图被数了两次。
    #[test]
    fn 一级类目按图片去重不按标签相加() {
        let conn = open_test_db().unwrap();
        let a = seed(&conn, "a.jpg");
        let b = seed(&conn, "b.jpg");
        mount(&conn, a, "人像");
        mount(&conn, a, "单人");
        mount(&conn, a, "全身");
        mount(&conn, b, "合照");

        let pairs = vec![
            ("人像".to_string(), "人像".to_string()),
            ("单人".to_string(), "人像".to_string()),
            ("全身".to_string(), "人像".to_string()),
            ("合照".to_string(), "人像".to_string()),
            ("日出".to_string(), "自然".to_string()),
        ];
        let got = group_counts(&conn, Some(1), None, &pairs).unwrap();
        assert_eq!(got, vec![("人像".to_string(), 2)], "应为 2 张（不是 4 条挂载）");
    }

    /// 只过了一级门槛、没挂上二级的图，也要算进这个一级类目。
    #[test]
    fn 只有一级标签的图也算进类目() {
        let conn = open_test_db().unwrap();
        let a = seed(&conn, "a.jpg");
        mount(&conn, a, "人像");

        let pairs = vec![
            ("人像".to_string(), "人像".to_string()),
            ("单人".to_string(), "人像".to_string()),
        ];
        let got = group_counts(&conn, Some(1), None, &pairs).unwrap();
        assert_eq!(got, vec![("人像".to_string(), 1)]);
    }

    /// EXIF 标签（相机/地点）不属于任何一级类目，不能混进去。
    #[test]
    fn 只统计视觉来源的标签() {
        let conn = open_test_db().unwrap();
        let a = seed(&conn, "a.jpg");
        let id = ensure(&conn, "人像", Some("exif")).unwrap();
        add_image_tag(&conn, a, id, "exif", None).unwrap();

        let pairs = vec![("人像".to_string(), "人像".to_string())];
        let got = group_counts(&conn, Some(1), None, &pairs).unwrap();
        assert!(got.is_empty(), "exif 挂载不该计入一级类目张数");
    }
}
