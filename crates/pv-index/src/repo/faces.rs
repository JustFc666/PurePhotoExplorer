//! `faces` 表仓储：人脸框 + 128 维 int8 向量。
//!
//! ## 两个硬约定
//! 1. **按图重写是幂等的**：[`replace_for_image`] 先删该图所有旧行再插，
//!    重跑分析/换阈值都不会攒出重复行（没有唯一索引兜底，靠这个函数保证）；
//! 2. **向量存 int8**：[`quantize`] / [`dequantize`] 一对，写库前 L2 归一化过，
//!    读出来直接点积就是余弦相似度（省一次除法，聚类时十万次调用不是小数目）。

use crate::error::Result;
use crate::models::Face;
use rusqlite::{params, Connection};

/// 把已 L2 归一化的 f32 向量量化成 int8（值域 [-1,1] → [-127,127]）。
///
/// 量化误差 ≈ 1/127 ≈ 0.008，远小于人脸判定阈值的量级（0.4~0.55），可忽略。
pub fn quantize(v: &[f32]) -> Vec<u8> {
    v.iter()
        .map(|&x| {
            let q = (x.clamp(-1.0, 1.0) * 127.0).round() as i32;
            (q.clamp(-127, 127) as i8) as u8
        })
        .collect()
}

/// int8 → f32（**不再重新归一化**：量化前已归一化，误差 0.8% 可忽略）。
pub fn dequantize(b: &[u8]) -> Vec<f32> {
    b.iter().map(|&x| (x as i8) as f32 / 127.0).collect()
}

/// 余弦相似度（两边都应是归一化向量）。
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// 用新的一批人脸**整体替换**某张图的旧结果（幂等，重跑安全）。
///
/// 返回新插入的行 id。已存在的 `person_id` **不保留** —— 换模型/重跑后
/// 旧归属本来就不能信，重新聚类才是对的。
pub fn replace_for_image(conn: &Connection, image_id: i64, faces: &[Face]) -> Result<Vec<i64>> {
    conn.execute("DELETE FROM faces WHERE image_id = ?1", params![image_id])?;
    let mut ids = Vec::with_capacity(faces.len());
    let mut stmt = conn.prepare(
        "INSERT INTO faces
           (image_id, person_id, x, y, w, h, tier, model, score, quality, embedding)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
    )?;
    for f in faces {
        stmt.execute(params![
            image_id,
            f.person_id,
            f.x,
            f.y,
            f.w,
            f.h,
            f.tier,
            f.model,
            f.score,
            f.quality,
            f.embedding,
        ])?;
        ids.push(conn.last_insert_rowid());
    }
    Ok(ids)
}

/// 删掉一批图片的所有人脸（「按图库清空」用）。返回删除行数。
///
/// 分批在调用方做：SQLite 的 `IN (?,?,…)` 变量数有上限（默认 999），
/// 图库上万张图必须切块，否则直接报 "too many SQL variables"。
pub fn delete_for_images(conn: &Connection, image_ids: &[i64]) -> Result<usize> {
    if image_ids.is_empty() {
        return Ok(0);
    }
    let ph = vec!["?"; image_ids.len()].join(",");
    let sql = format!("DELETE FROM faces WHERE image_id IN ({ph})");
    let mut stmt = conn.prepare(&sql)?;
    Ok(stmt.execute(rusqlite::params_from_iter(image_ids.iter().copied()))?)
}

const COLS: &str = "id, image_id, person_id, x, y, w, h, tier, model, score, quality, embedding";

fn map_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Face> {
    Ok(Face {
        id: r.get(0)?,
        image_id: r.get(1)?,
        person_id: r.get(2)?,
        x: r.get(3)?,
        y: r.get(4)?,
        w: r.get(5)?,
        h: r.get(6)?,
        tier: r.get(7)?,
        model: r.get(8)?,
        score: r.get(9)?,
        quality: r.get(10)?,
        embedding: r.get(11)?,
    })
}

/// 某张图的所有人脸（按脸大小降序，大头像排前面当封面更合适）。
pub fn list_by_image(conn: &Connection, image_id: i64) -> Result<Vec<Face>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM faces WHERE image_id = ?1 ORDER BY w * h DESC"
    ))?;
    let rows = stmt.query_map(params![image_id], map_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 某个人物下的所有人脸。
pub fn list_by_person(conn: &Connection, person_id: i64) -> Result<Vec<Face>> {
    let mut stmt =
        conn.prepare(&format!("SELECT {COLS} FROM faces WHERE person_id = ?1 ORDER BY id"))?;
    let rows = stmt.query_map(params![person_id], map_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 还没归属人物的脸（聚类输入）。`min_quality` 用来滤掉小脸 ——
/// 实测小脸向量是噪声，放进聚类会把不相干的人粘成一片。
pub fn list_unassigned(
    conn: &Connection,
    model: &str,
    min_quality: f32,
    limit: i64,
) -> Result<Vec<Face>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM faces
          WHERE person_id IS NULL AND model = ?1 AND COALESCE(quality, 0) >= ?2
          ORDER BY id LIMIT ?3"
    ))?;
    let rows = stmt.query_map(params![model, min_quality, limit], map_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 全部人脸（聚类时一次性读进内存算相似度矩阵；10 万张 = 12.8 MB int8，可接受）。
pub fn list_all(conn: &Connection, model: &str) -> Result<Vec<Face>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM faces WHERE model = ?1 ORDER BY id"
    ))?;
    let rows = stmt.query_map(params![model], map_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 把一批脸归到某个人物（`person_id = None` 表示从人物里摘出来）。
pub fn set_person(conn: &Connection, face_ids: &[i64], person_id: Option<i64>) -> Result<usize> {
    if face_ids.is_empty() {
        return Ok(0);
    }
    let mut stmt = conn.prepare("UPDATE faces SET person_id = ?1 WHERE id = ?2")?;
    let mut n = 0;
    for id in face_ids {
        n += stmt.execute(params![person_id, id])?;
    }
    Ok(n)
}

/// 某个人物有几张脸。
pub fn count_for_person(conn: &Connection, person_id: i64) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM faces WHERE person_id = ?1",
        params![person_id],
        |r| r.get(0),
    )?)
}

/// 按模型统计人脸数（判断"换模型后要重建"用）。
pub fn count_by_model(conn: &Connection) -> Result<Vec<(String, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT model, COUNT(*) FROM faces GROUP BY model ORDER BY COUNT(*) DESC",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 删光人脸（「清除全部人脸数据」）。`model = None` 表示不分模型全清。
///
/// ⛔ 只删 `faces`，**不碰图片索引**；`persons` 由调用方决定是否一起删。
pub fn delete_all(conn: &Connection, model: Option<&str>) -> Result<usize> {
    let n = match model {
        Some(m) => conn.execute("DELETE FROM faces WHERE model = ?1", params![m])?,
        None => conn.execute("DELETE FROM faces", [])?,
    };
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Folder, Image, Volume};
    use crate::repo::{folders, images, volumes};
    use crate::open_test_db;

    fn seed_image(conn: &Connection) -> i64 {
        let vid = volumes::insert(conn, &Volume::new(r"\\?\Volume{face}\")).unwrap();
        let fid = folders::insert(conn, &Folder::new(vid, r"pics")).unwrap();
        images::insert(conn, &Image::new(fid, "a.jpg")).unwrap()
    }

    fn face(x: f32, emb: &[f32]) -> Face {
        let mut f = Face::new(1, x, 0.1, 0.2, 0.2);
        f.model = "sface_2021dec".into();
        f.score = Some(0.9);
        f.quality = Some(88.0);
        f.tier = Some(2048);
        f.embedding = quantize(emb);
        f
    }

    #[test]
    fn int8量化往返误差可忽略() {
        let v = [0.6f32, -0.8, 0.0, 1.0, -1.0];
        let back = dequantize(&quantize(&v));
        for (a, b) in v.iter().zip(&back) {
            assert!((a - b).abs() < 0.01, "{a} vs {b}");
        }
        // 归一化向量量化后点积仍接近 1
        let unit = [0.6f32, 0.8];
        let q = dequantize(&quantize(&unit));
        assert!((cosine(&unit, &q) - 1.0).abs() < 0.02);
    }

    #[test]
    fn 按图重写是幂等的() {
        let c = open_test_db().unwrap();
        let img = seed_image(&c);
        let f = vec![face(0.1, &[0.6, 0.8])];
        assert_eq!(replace_for_image(&c, img, &f).unwrap().len(), 1);
        // 重跑：仍然只有 1 行（⛔ 不能越跑越多）
        assert_eq!(replace_for_image(&c, img, &f).unwrap().len(), 1);
        assert_eq!(list_by_image(&c, img).unwrap().len(), 1);

        // 重跑必须**刷新内容**，不是留着旧值
        let mut moved = vec![face(0.7, &[0.8, 0.6])];
        moved[0].quality = Some(120.0);
        replace_for_image(&c, img, &moved).unwrap();
        let got = list_by_image(&c, img).unwrap();
        assert_eq!(got.len(), 1);
        assert!((got[0].x - 0.7).abs() < 1e-6);
        assert_eq!(got[0].quality, Some(120.0));
    }

    #[test]
    fn 大脸排在前面便于选封面() {
        let c = open_test_db().unwrap();
        let img = seed_image(&c);
        let mut small = face(0.1, &[1.0, 0.0]);
        small.w = 0.05;
        small.h = 0.05;
        let big = face(0.5, &[0.0, 1.0]); // w=h=0.2
        replace_for_image(&c, img, &[small, big]).unwrap();
        let got = list_by_image(&c, img).unwrap();
        assert!(got[0].w > got[1].w);
    }

    #[test]
    fn 小脸被聚类输入过滤掉() {
        let c = open_test_db().unwrap();
        let img = seed_image(&c);
        let mut tiny = face(0.1, &[1.0, 0.0]);
        tiny.quality = Some(20.0);
        let ok = face(0.5, &[0.0, 1.0]);
        replace_for_image(&c, img, &[tiny, ok]).unwrap();
        let pool = list_unassigned(&c, "sface_2021dec", 56.0, 100).unwrap();
        assert_eq!(pool.len(), 1, "小脸不该进聚类池");
        assert_eq!(pool[0].quality, Some(88.0));
    }

    #[test]
    fn 换模型的人脸被分开统计与清理() {
        let c = open_test_db().unwrap();
        let img = seed_image(&c);
        let mut old = face(0.1, &[1.0, 0.0]);
        old.model = "old_model".into();
        replace_for_image(&c, img, &[old, face(0.5, &[0.0, 1.0])]).unwrap();
        assert_eq!(count_by_model(&c).unwrap().len(), 2);
        assert_eq!(delete_all(&c, Some("old_model")).unwrap(), 1);
        assert_eq!(list_all(&c, "sface_2021dec").unwrap().len(), 1);
    }
}
