//! `tags` / `image_tags` 表仓储（标签与图像挂载）。

use crate::error::Result;
use crate::models::Tag;
use rusqlite::{params, params_from_iter, Connection};

/// 取（必要时创建）同名标签，返回其 id。**同名幂等**（依赖 `tags.name UNIQUE`）。
///
/// ## `kind` 只升不降
/// 同名标签已存在时**不改** `kind` —— 一个标签是手工标签还是 AI 标签，由**第一次**
/// 建立时定下，后续挂载不改变它的身份。
///
/// ⭐ 唯一例外：原来是 `kind='exif'`（机型 / 地点派生，侧栏**故意隐藏**），
/// 现在被当作真标签（`user` / `ai`）使用 —— 必须摘掉隐藏标记，否则用户手动打的
/// 标签在侧栏「标签」分区里永远看不见（`counts_scoped` 有 `WHERE t.kind <> 'exif'`）。
///
/// ⛔ 反过来**不许降级**：`exif` 写入不能把已有 `user` / `ai` 标签重新藏起来。
pub fn ensure(conn: &Connection, name: &str, kind: Option<&str>) -> Result<i64> {
    conn.execute(
        "INSERT INTO tags(name, kind) VALUES (?1, ?2)
         ON CONFLICT(name) DO UPDATE SET kind = excluded.kind
           WHERE (tags.kind IS NULL OR tags.kind = 'exif')
             AND excluded.kind IS NOT NULL
             AND excluded.kind <> 'exif'",
        params![name, kind],
    )?;
    Ok(conn.query_row("SELECT id FROM tags WHERE name = ?1", params![name], |r| r.get(0))?)
}

/// 在一批图里挑出「明显不是人像」的（人脸检测可跳过，省时间）。
///
/// ## 判据（保守，宁可多跑不可漏检）
/// 该图**有 AI 标签**、命中了「明确不可能有真人脸」的标签（截图/图表/扫描件/文档…），
/// **同时**没有任何「人像类」标签。
///
/// ⭐ 只对打过标的图做减法 —— 没标签的图不会出现在结果里（照常检测），
/// 所以这**不是硬依赖**：新装的库、打标失败的图都不会被漏掉。
pub fn non_portrait_ids(conn: &Connection, ids: &[i64]) -> Result<Vec<i64>> {
    const NON_PORTRAIT: &str = "'软件截图','图表','扫描','白板','文档','PPT','思维导图'";
    const PORTRAIT: &str = "'人像','单人','合照','儿童','婴儿','证件','自拍','全家福'";

    let mut out = Vec::new();
    for chunk in ids.chunks(500) {
        let ph = vec!["?"; chunk.len()].join(",");
        let sql = format!(
            "SELECT it.image_id
               FROM image_tags it JOIN tags t ON t.id = it.tag_id
              WHERE it.source = 'ai' AND it.image_id IN ({ph})
              GROUP BY it.image_id
             HAVING SUM(CASE WHEN t.name IN ({NON_PORTRAIT}) THEN 1 ELSE 0 END) > 0
                AND SUM(CASE WHEN t.name IN ({PORTRAIT}) THEN 1 ELSE 0 END) = 0"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(chunk.iter().copied()), |r| r.get::<_, i64>(0))?;
        for v in rows {
            out.push(v?);
        }
    }
    Ok(out)
}

/// 一批图里有多少张打过 AI 标签。
pub fn count_tagged(conn: &Connection, ids: &[i64]) -> Result<usize> {
    let mut n = 0usize;
    for chunk in ids.chunks(500) {
        let ph = vec!["?"; chunk.len()].join(",");
        let sql = format!(
            "SELECT COUNT(DISTINCT image_id) FROM image_tags
              WHERE source = 'ai' AND image_id IN ({ph})"
        );
        let c: i64 = conn.query_row(&sql, params_from_iter(chunk.iter().copied()), |r| r.get(0))?;
        n += c.max(0) as usize;
    }
    Ok(n)
}

/// 清掉某张图在某个来源下的全部标签挂载（AI 重跑前用）。
///
/// 按 `source` 过滤是必须的：`image_tags` 里同时有 user / ai / exif / ocr / face，
/// 清 ai 时不能把用户手动打的标签一起删了。
pub fn clear_image_source(conn: &Connection, image_id: i64, source: &str) -> Result<usize> {
    Ok(conn.execute(
        "DELETE FROM image_tags WHERE image_id = ?1 AND source = ?2",
        params![image_id, source],
    )?)
}

/// 删掉「**已经有二级词支撑**」的图上的**一级类目标签**。返回删除行数。
///
/// ## 为什么要有这条
/// 一级类目（自然 / 活动 / 兵器 / 物品…）在库里只是**二级的派生**：词表里
/// `l2 -> l1` 是确定的（刀剑 -> 兵器），分组、侧栏计数都能反推出来。
/// 早期 Python 版打标把一级也当独立标签写库，于是同一张图会同时出现
/// 「兵器 0.45（一级）」和「物品 0.40（一级）」—— 一张刀剑图被同时归到
/// 两个一级目录（用户 2026-10-29 报的「应该在兵器，被归进物品」）。
///
/// Rust 版打标**从来只写二级**，所以这里只需要清掉历史遗留。
///
/// ## 判据（用户提的规则）
/// 同一张图上「一级 + 二级」共存时，**那个单独的一级舍去**：
/// 二级已经能推出它，而且没有二级支撑的一级往往是误判（物品 0.40）。
///
/// **只删「这张图已经有二级词」的那些**。只有一级、没有二级的图
/// （老库里「自然」11 张、「建筑」16 张是这样）**保留** —— 那是它唯一的证据，
/// 删掉会让这些图变成「无标签」，侧栏计数也跟着掉。
///
/// `l1_names` 为空时**直接返回 0**（保护）：传空会让 `NOT IN ()` 退化，
/// 把所有标签都当成一级。
pub fn drop_redundant_l1(conn: &Connection, l1_names: &[String]) -> Result<usize> {
    if l1_names.is_empty() {
        return Ok(0);
    }
    let ph = vec!["?"; l1_names.len()].join(",");
    let sql = format!(
        "DELETE FROM image_tags
          WHERE source = 'ai'
            AND tag_id IN (SELECT id FROM tags WHERE name IN ({ph}))
            AND image_id IN (
                  SELECT it2.image_id FROM image_tags it2
                    JOIN tags t2 ON t2.id = it2.tag_id
                   WHERE it2.source = 'ai' AND t2.name NOT IN ({ph}) )"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut params: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(l1_names.len() * 2);
    for n in l1_names {
        params.push(n);
    }
    for n in l1_names {
        params.push(n);
    }
    Ok(stmt.execute(params.as_slice())?)
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
    /*
     * `t.kind <> 'exif'` —— 侧栏「标签」分区**不列 EXIF 派生标签**（用户 2026-10-30）。
     *
     * `kind='exif'` 的那批全是**相机机型**（iQOO 10 / XT1650-05 / Canon EOS R5…）
     * 和**地点**（上海 / 浦东 / 金桥…），一共 48 个，占侧栏一多半，把真正有用的
     * AI 标签（人像 / 单人 / 二次元…）挤到下面去了。
     *
     * 它们不是没有出口：
     *   机型 → 筛选栏「相机」下拉
     *   地点 → 筛选栏「地点」下拉
     * 所以从标签列表里拿掉不丢功能。
     */
    let mut stmt = conn.prepare(
        "SELECT t.id, t.name, COUNT(DISTINCT it.image_id) AS n
           FROM tags t
           JOIN image_tags it ON it.tag_id = t.id
           JOIN images i ON i.id = it.image_id
          WHERE t.kind <> 'exif'
            AND (?1 IS NULL OR EXISTS (
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
    let mut out: Vec<(String, i64)> = rows.collect::<rusqlite::Result<Vec<_>>>()?;

    /*
     * 「人像」这个一级类目的数字改按**检出人脸**算（2026-11-02 用户要求）。
     *
     * 用户的原话：「图库里每张有脸的照片，都应该在人像里」。
     * CN-CLIP 只给极少数图打上「人像/单人/合照」（用户库里 546 张图里只有 10 张），
     * 而人脸识别检出了 168 张 —— 两个数字摆在同一条标签条上，用户只会觉得
     * 「明明有脸，为什么人像里没有」。所以这一格改成和人脸识别对齐。
     *
     * ⚠️ 只有这一格是特例，别顺手把「动物」「风景」也改口径。
     */
    let face_n = count_face_images(conn, library_id, dir_id)?;
    match out.iter_mut().find(|(l1, _)| l1 == "人像") {
        Some(slot) => slot.1 = face_n,
        /* 词表里有人像类目但一张都没命中时，SQL 不会返回这一行，补上 */
        None if face_n > 0 => out.push(("人像".into(), face_n)),
        None => {}
    }
    out.sort_by_key(|x| std::cmp::Reverse(x.1));
    Ok(out)
}

/// 检出过人脸的图片数（**按图去重**，一张合影只算一张）。
///
/// 范围口径与 [`group_counts`] 一致：按图库 / 目录过滤。
fn count_face_images(
    conn: &Connection,
    library_id: Option<i64>,
    dir_id: Option<i64>,
) -> Result<i64> {
    let sql = "SELECT COUNT(*) FROM images i
                WHERE EXISTS (SELECT 1 FROM faces f WHERE f.image_id = i.id)
                  AND (?1 IS NULL OR EXISTS (
                          SELECT 1 FROM library_folders lf
                           WHERE lf.library_id = ?1 AND lf.folder_id = i.dir_id))
                  AND (?2 IS NULL OR i.dir_id = ?2)";
    let n: i64 = conn.query_row(sql, rusqlite::params![library_id, dir_id], |r| r.get(0))?;
    Ok(n)
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

    /// 一张图同时挂「猫」「狗」时，动物类目只算 **1 张**。
    ///
    /// 这是用户 2026-10-23 报的 bug：标签条写「人像 89」，点进去只有 52 张 ——
    /// 因为前端把二级的张数直接相加，同一张图被数了两次。
    ///
    /// ⚠️ 2026-11-02 起「人像」这一格改按检出人脸算（见 `人像类目按检出人脸计数`），
    /// 所以这里换个类目名来测「按图去重」这条规则本身。
    #[test]
    fn 一级类目按图片去重不按标签相加() {
        let conn = open_test_db().unwrap();
        let a = seed(&conn, "a.jpg");
        let b = seed(&conn, "b.jpg");
        mount(&conn, a, "动物");
        mount(&conn, a, "猫");
        mount(&conn, a, "狗");
        mount(&conn, b, "鸟");

        let pairs = vec![
            ("动物".to_string(), "动物".to_string()),
            ("猫".to_string(), "动物".to_string()),
            ("狗".to_string(), "动物".to_string()),
            ("鸟".to_string(), "动物".to_string()),
            ("日出".to_string(), "自然".to_string()),
        ];
        let got = group_counts(&conn, Some(1), None, &pairs).unwrap();
        assert_eq!(got, vec![("动物".to_string(), 2)], "应为 2 张（不是 4 条挂载）");
    }

    /// 只过了一级门槛、没挂上二级的图，也要算进这个一级类目。
    #[test]
    fn 只有一级标签的图也算进类目() {
        let conn = open_test_db().unwrap();
        let a = seed(&conn, "a.jpg");
        mount(&conn, a, "动物");

        let pairs = vec![
            ("动物".to_string(), "动物".to_string()),
            ("猫".to_string(), "动物".to_string()),
        ];
        let got = group_counts(&conn, Some(1), None, &pairs).unwrap();
        assert_eq!(got, vec![("动物".to_string(), 1)]);
    }

    /// 「人像」这一格按**检出人脸**算，不看 AI 标签有没有打上。
    ///
    /// 用户 2026-11-02：「图库里每张有脸的照片，都应该在人像里」。
    /// 他库里 546 张图只有 10 张被打了人像标签，但检出 168 张脸 ——
    /// 标签条上写 10、人脸识别说 168，用户只会觉得「漏了」。
    #[test]
    fn 人像类目按检出人脸计数() {
        let conn = open_test_db().unwrap();
        let a = seed(&conn, "a.jpg");
        let b = seed(&conn, "b.jpg");
        let _c = seed(&conn, "c.jpg"); // 没有脸、也没有人像标签
        mount(&conn, a, "人像"); // 只有 a 打了人像标签
        for id in [a, b] {
            conn.execute(
                "INSERT INTO faces(image_id, x, y, w, h, model) VALUES (?1, 0.1, 0.1, 0.1, 0.1, 'm')",
                params![id],
            )
            .unwrap();
        }

        let pairs = vec![("人像".to_string(), "人像".to_string())];
        let got = group_counts(&conn, Some(1), None, &pairs).unwrap();
        assert_eq!(got, vec![("人像".to_string(), 2)], "应等于有脸的图数 2，而不是标签数 1");
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

#[cfg(test)]
mod l1_cleanup_tests {
    use super::*;
    use crate::open_test_db;

    fn seed_img(conn: &Connection, name: &str) -> i64 {
        conn.execute(
            "INSERT OR IGNORE INTO volumes(id, volume_guid) VALUES (1, 'v1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO folders(id, volume_id, rel_path) VALUES (1, 1, 'root')",
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

    fn tag(conn: &Connection, image_id: i64, name: &str, conf: f64) {
        let id = ensure(conn, name, Some("ai")).unwrap();
        add_image_tag(conn, image_id, id, "ai", Some(conf)).unwrap();
    }

    fn names(conn: &Connection, image_id: i64) -> Vec<String> {
        let mut st = conn
            .prepare(
                "SELECT t.name FROM image_tags it JOIN tags t ON t.id = it.tag_id
                  WHERE it.image_id = ?1 ORDER BY t.name",
            )
            .unwrap();
        st.query_map(params![image_id], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    /// 用户报的那张刀剑图：兵器(一级) + 刀剑(二级) + 物品(一级)
    /// → 两条一级都删掉，只留二级「刀剑」，由词表反推出属于「兵器」。
    #[test]
    fn 有二级支撑时一级标签被舍去() {
        let conn = open_test_db().unwrap();
        let img = seed_img(&conn, "sword.jpg");
        tag(&conn, img, "兵器", 0.45);
        tag(&conn, img, "刀剑", 0.47);
        tag(&conn, img, "物品", 0.40);

        let n = drop_redundant_l1(&conn, &["兵器".to_string(), "物品".to_string()]).unwrap();
        assert_eq!(n, 2, "两条一级都该删");
        assert_eq!(names(&conn, img), vec!["刀剑".to_string()], "只留二级词");
    }

    /// 只有一级、没有二级的图要**保住** —— 那是它唯一的证据。
    #[test]
    fn 只有一级的图保留() {
        let conn = open_test_db().unwrap();
        let img = seed_img(&conn, "nature.jpg");
        tag(&conn, img, "自然", 0.41);

        assert_eq!(
            drop_redundant_l1(&conn, &["自然".to_string()]).unwrap(),
            0
        );
        assert_eq!(names(&conn, img), vec!["自然".to_string()]);
    }

    /// 别误删 EXIF / OCR 等其他来源的标签（只是名字碰巧同名）。
    #[test]
    fn 不碰其他来源的标签() {
        let conn = open_test_db().unwrap();
        let img = seed_img(&conn, "x.jpg");
        let id = ensure(&conn, "物品", Some("exif")).unwrap();
        add_image_tag(&conn, img, id, "exif", None).unwrap();
        tag(&conn, img, "刀剑", 0.50);

        assert_eq!(
            drop_redundant_l1(&conn, &["物品".to_string()]).unwrap(),
            0
        );
        // 排序：刀剑 在 物品 前（中文按字节序）
        let got = names(&conn, img);
        assert_eq!(got.len(), 2, "exif 那条不能被删：{got:?}");
        assert!(got.contains(&"物品".to_string()));
    }

    /// 传空的一级名单必须**什么都不做**（否则 NOT IN () 会把所有标签当一级）。
    #[test]
    fn 空名单是空操作() {
        let conn = open_test_db().unwrap();
        let img = seed_img(&conn, "y.jpg");
        tag(&conn, img, "兵器", 0.45);
        tag(&conn, img, "刀剑", 0.47);

        assert_eq!(drop_redundant_l1(&conn, &[]).unwrap(), 0);
        assert_eq!(names(&conn, img).len(), 2);
    }
}

#[cfg(test)]
mod scoped_counts_tests {
    use super::*;
    use crate::open_test_db;

    /// 造一张图（含卷 + 目录），返回 `image_id`。计数范围查询只要能 JOIN 到图即可。
    fn seed_image(conn: &Connection) -> i64 {
        conn.execute(
            "INSERT OR IGNORE INTO volumes(id, volume_guid) VALUES (1, 'v1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO folders(id, volume_id, rel_path) VALUES (1, 1, 'root')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO images(dir_id, rel_name) VALUES (1, 'a.jpg')", [])
            .unwrap();
        conn.last_insert_rowid()
    }

    /// 侧栏「标签」分区**不列 EXIF 派生标签**（机型 / 地点）。
    ///
    /// 用户 2026-10-30：48 个机型/城市名把真正的 AI 标签挤到下面去了。
    /// 它们没丢：机型走筛选栏「相机」、地点走「地点」下拉。
    #[test]
    fn 侧栏标签不含机型与地点() {
        let conn = open_test_db().unwrap();
        let img = seed_image(&conn);

        for (name, kind) in [("人像", "ai"), ("二次元", "ai"), ("iQOO 10", "exif"), ("上海", "exif")] {
            let id = ensure(&conn, name, Some(kind)).unwrap();
            add_image_tag(&conn, img, id, kind, Some(0.5)).unwrap();
        }

        let mut got: Vec<String> = counts_scoped(&conn, None, None)
            .unwrap()
            .into_iter()
            .map(|(_, name, _)| name)
            .collect();
        got.sort();
        let mut want = vec!["人像".to_string(), "二次元".to_string()];
        want.sort();
        assert_eq!(got, want);
        assert!(!got.iter().any(|n| n == "iQOO 10" || n == "上海"));
    }

    /// 同名标签一旦被当作**真标签**（`user` / `ai`）使用，就要摘掉 `exif` 隐藏标记。
    ///
    /// ## 这个 bug 怎么来的
    /// `ensure` 原先是 `INSERT OR IGNORE` —— 同名已存在时**整行不动**，
    /// 于是 `kind` 永远停在第一次建出来的值上。老库里「iQOO 10」「上海」这类
    /// EXIF 派生标签是 `kind='exif'`，而 `counts_scoped` 有 `WHERE t.kind <> 'exif'`。
    ///
    /// 结果：用户手动打一个与机型/城市同名的标签 → 卡片上有、侧栏「标签」里
    /// 永远找不到（不是空库才有的问题，用户自己的库里就有这 48 个名字）。
    ///
    /// ## 两个方向都要守住
    /// - 被真标签使用时**必须摘标记**（否则看不见）
    /// - 反之 `exif` 写入**不许把真标签重新藏起来**（否则丢标签）
    #[test]
    fn 同名标签被真标签使用时摘掉隐藏标记() {
        let conn = open_test_db().unwrap();
        let img = seed_image(&conn);

        // 老库状态：机型标签是 exif 派生，侧栏看不见。
        let exif_id = ensure(&conn, "iQOO 10", Some("exif")).unwrap();
        add_image_tag(&conn, img, exif_id, "exif", None).unwrap();
        assert!(counts_scoped(&conn, None, None).unwrap().is_empty());

        // 用户手动打了同名标签：还是同一个 tag（不能建出第二个），但要能看见。
        let user_id = ensure(&conn, "iQOO 10", Some("user")).unwrap();
        assert_eq!(user_id, exif_id, "同名必须是同一个标签");
        add_image_tag(&conn, img, user_id, "user", None).unwrap();

        let got = counts_scoped(&conn, None, None).unwrap();
        assert_eq!(got.len(), 1, "机型名现在也是真标签了，侧栏要列出来");
        assert_eq!(got[0].1, "iQOO 10");
        assert_eq!(got[0].2, 1, "同一张图两个来源也只算一张");

        // 反向：之后再有 exif 写入，不许把用户标签重新藏起来。
        ensure(&conn, "iQOO 10", Some("exif")).unwrap();
        assert_eq!(
            counts_scoped(&conn, None, None).unwrap().len(),
            1,
            "exif 写入不能降级已有 user 标签"
        );
    }
}
