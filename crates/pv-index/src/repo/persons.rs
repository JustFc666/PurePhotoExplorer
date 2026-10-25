//! `persons` 表仓储：聚类产出的人物簇 + 用户命名。
//!
//! ## 产品口径（2026-10-24 定）
//! - **零幻觉**：`name` 只有用户能给，`NULL` 就显示「未命名 ①②」；
//! - **只给 ≥3 张脸的簇建人物**：实测 745 张家庭+社媒混合库，两阶段聚类后
//!   105 个簇里有 65 个是单例（模特图里很多人只出现一张），全建成人物只是噪声；
//! - **合并 / 拆分是纠正入口**：误合并比误拆分更伤（用户不易察觉），
//!   所以阈值偏严、靠这里兜底。

use crate::error::Result;
use crate::models::Person;
use rusqlite::{params, Connection};

const COLS: &str = "id, name, cover_face_id, face_count, cluster_version, created_ts";

fn map_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Person> {
    Ok(Person {
        id: r.get(0)?,
        name: r.get(1)?,
        cover_face_id: r.get(2)?,
        face_count: r.get(3)?,
        cluster_version: r.get(4)?,
        created_ts: r.get(5)?,
    })
}

/// 新建一个人物（`name` 一般传 `None`，用户命名后再填）。
pub fn insert(conn: &Connection, p: &Person) -> Result<i64> {
    conn.execute(
        "INSERT INTO persons (name, cover_face_id, face_count, cluster_version, created_ts)
         VALUES (?1, ?2, ?3, ?4, COALESCE(?5, strftime('%s','now')))",
        params![p.name, p.cover_face_id, p.face_count, p.cluster_version, p.created_ts],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn get(conn: &Connection, id: i64) -> Result<Option<Person>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLS} FROM persons WHERE id = ?1"))?;
    let mut rows = stmt.query_map(params![id], map_row)?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

/// 列表：**只返回 `min_faces` 及以上的**，已命名的永远排在前面。
///
/// 排序意图：用户命名过的是"真人物"，未命名的一堆数字只是候选，
/// 不能抢占侧栏顶部。
pub fn list(conn: &Connection, min_faces: i64) -> Result<Vec<Person>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM persons WHERE face_count >= ?1
          ORDER BY (name IS NULL), face_count DESC, id"
    ))?;
    let rows = stmt.query_map(params![min_faces], map_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 改名（`None` = 取消命名）。
pub fn rename(conn: &Connection, id: i64, name: Option<&str>) -> Result<()> {
    conn.execute("UPDATE persons SET name = ?1 WHERE id = ?2", params![name, id])?;
    Ok(())
}

/// 设封面脸。
pub fn set_cover(conn: &Connection, id: i64, face_id: Option<i64>) -> Result<()> {
    conn.execute(
        "UPDATE persons SET cover_face_id = ?1 WHERE id = ?2",
        params![face_id, id],
    )?;
    Ok(())
}

/// 合并：把 `from` 里所有人物的脸并到 `into`，然后删掉空壳人物。
///
/// 返回被并过去的脸数。`into` 不存在则报错（不做隐式创建，避免把
/// "合并到不存在的人物"变成静默的数据错乱）。
pub fn merge(conn: &Connection, from: &[i64], into: i64) -> Result<usize> {
    if get(conn, into)?.is_none() {
        return Err(crate::error::IndexError::BadValue(format!(
            "要合并到的目标人物不存在：{into}"
        )));
    }
    let mut moved = 0;
    for src in from.iter().copied().filter(|id| *id != into) {
        moved += conn.execute(
            "UPDATE faces SET person_id = ?1 WHERE person_id = ?2",
            params![into, src],
        )?;
        conn.execute("DELETE FROM persons WHERE id = ?1", params![src])?;
    }
    refresh_count(conn, into)?;
    Ok(moved)
}

/// 拆分：把选中的几张脸从人物里摘出来，建成一个**新的未命名人物**。
///
/// 返回新人物 id；一张都没摘（`face_ids` 全不属于该人物）时返回 `None`。
pub fn split(
    conn: &Connection,
    from_person: i64,
    face_ids: &[i64],
    cluster_version: Option<&str>,
) -> Result<Option<i64>> {
    if face_ids.is_empty() {
        return Ok(None);
    }
    let new_id = insert(
        conn,
        &Person::new(None, cluster_version.map(|s| s.to_string())),
    )?;
    let mut moved = 0;
    for id in face_ids {
        moved += conn.execute(
            "UPDATE faces SET person_id = ?1 WHERE id = ?2 AND person_id = ?3",
            params![new_id, id, from_person],
        )?;
    }
    if moved == 0 {
        conn.execute("DELETE FROM persons WHERE id = ?1", params![new_id])?;
        return Ok(None);
    }
    refresh_count(conn, new_id)?;
    refresh_count(conn, from_person)?;
    Ok(Some(new_id))
}

/// 删除人物定义：**只把人脸解绑，不删人脸**（人脸是识别结果，不属于人物）。
pub fn delete(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("UPDATE faces SET person_id = NULL WHERE person_id = ?1", params![id])?;
    conn.execute("DELETE FROM persons WHERE id = ?1", params![id])?;
    Ok(())
}

/// 重算某个人物的 `face_count` 与封面（聚类/合并/拆分后调）。
///
/// 封面策略：取**面积最大的脸**（大头像当头像最清楚）；没有脸就置 NULL。
pub fn refresh_count(conn: &Connection, id: i64) -> Result<i64> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM faces WHERE person_id = ?1",
        params![id],
        |r| r.get(0),
    )?;
    let cover: Option<i64> = conn
        .query_row(
            "SELECT id FROM faces WHERE person_id = ?1 ORDER BY w * h DESC LIMIT 1",
            params![id],
            |r| r.get(0),
        )
        .ok();
    conn.execute(
        "UPDATE persons SET face_count = ?1, cover_face_id = ?2 WHERE id = ?3",
        params![n, cover, id],
    )?;
    Ok(n)
}

/// 全表重算（一次聚类跑完后调一次，比逐个人物查快得多）。
pub fn refresh_all(conn: &Connection) -> Result<()> {
    conn.execute(
        "UPDATE persons SET face_count =
           (SELECT COUNT(*) FROM faces WHERE faces.person_id = persons.id)",
        [],
    )?;
    conn.execute(
        "UPDATE persons SET cover_face_id =
           (SELECT faces.id FROM faces WHERE faces.person_id = persons.id
             ORDER BY faces.w * faces.h DESC LIMIT 1)",
        [],
    )?;
    // 一个人物都没有的人物行（全被拆走）直接清掉
    conn.execute("DELETE FROM persons WHERE face_count = 0", [])?;
    Ok(())
}

/// 清空全部人物（「清除全部人脸数据」时与 [`faces::delete_all`] 配对调用）。
///
/// [`faces::delete_all`]: crate::repo::faces::delete_all
pub fn delete_all(conn: &Connection) -> Result<usize> {
    conn.execute("UPDATE faces SET person_id = NULL", [])?;
    Ok(conn.execute("DELETE FROM persons", [])?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Face, Folder, Image, Volume};
    use crate::open_test_db;
    use crate::repo::{faces, folders, images, volumes};

    fn seed(conn: &Connection, n_img: usize) -> Vec<i64> {
        let vid = volumes::insert(conn, &Volume::new(r"\\?\Volume{p}")).unwrap();
        let fid = folders::insert(conn, &Folder::new(vid, r"pics")).unwrap();
        (0..n_img)
            .map(|i| images::insert(conn, &Image::new(fid, format!("{i}.jpg"))).unwrap())
            .collect()
    }

    fn add_faces(conn: &Connection, image_id: i64, n: usize) -> Vec<i64> {
        let mut list = Vec::new();
        for i in 0..n {
            let mut f = Face::new(image_id, i as f32 * 0.1, 0.1, 0.2, 0.2);
            f.model = "sface_2021dec".into();
            f.quality = Some(80.0);
            list.push(f);
        }
        faces::replace_for_image(conn, image_id, &list).unwrap()
    }

    #[test]
    fn 只列够张数的人物且命名优先() {
        let c = open_test_db().unwrap();
        let imgs = seed(&c, 4);
        // 人物 A：2 张脸；人物 B：4 张脸；人物 C：1 张脸（不该出现）
        let a = insert(&c, &Person::new(None, Some("v1".into()))).unwrap();
        let b = insert(&c, &Person::new(Some("老王".into()), Some("v1".into()))).unwrap();
        let cc = insert(&c, &Person::new(None, Some("v1".into()))).unwrap();
        faces::set_person(&c, &add_faces(&c, imgs[0], 2), Some(a)).unwrap();
        faces::set_person(&c, &add_faces(&c, imgs[1], 3), Some(b)).unwrap();
        faces::set_person(&c, &add_faces(&c, imgs[2], 1), Some(b)).unwrap();
        faces::set_person(&c, &add_faces(&c, imgs[3], 1), Some(cc)).unwrap();
        for id in [a, b, cc] {
            refresh_count(&c, id).unwrap();
        }

        let got = list(&c, 3).unwrap();
        assert_eq!(got.len(), 1, "只有 B 有 4 张脸");
        assert_eq!(got[0].name.as_deref(), Some("老王"));
        assert_eq!(got[0].face_count, 4);

        let all = list(&c, 1).unwrap();
        assert_eq!(all[0].name.as_deref(), Some("老王"), "命名过的排最前");
    }

    #[test]
    fn 合并把人脸并过去并删掉空壳() {
        let c = open_test_db().unwrap();
        let imgs = seed(&c, 2);
        let a = insert(&c, &Person::new(None, None)).unwrap();
        let b = insert(&c, &Person::new(None, None)).unwrap();
        let fa = add_faces(&c, imgs[0], 2);
        let fb = add_faces(&c, imgs[1], 3);
        faces::set_person(&c, &fa, Some(a)).unwrap();
        faces::set_person(&c, &fb, Some(b)).unwrap();
        refresh_count(&c, a).unwrap();
        refresh_count(&c, b).unwrap();

        let moved = merge(&c, &[b], a).unwrap();
        assert_eq!(moved, 3);
        assert!(get(&c, b).unwrap().is_none(), "源人物应被删除");
        assert_eq!(refresh_count(&c, a).unwrap(), 5);
    }

    #[test]
    fn 合并到不存在的人物要报错() {
        let c = open_test_db().unwrap();
        let a = insert(&c, &Person::new(None, None)).unwrap();
        assert!(merge(&c, &[a], 9999).is_err());
    }

    #[test]
    fn 拆分把选中的脸移进新人物() {
        let c = open_test_db().unwrap();
        let imgs = seed(&c, 1);
        let a = insert(&c, &Person::new(Some("甲".into()), None)).unwrap();
        let fa = add_faces(&c, imgs[0], 4);
        faces::set_person(&c, &fa, Some(a)).unwrap();
        refresh_count(&c, a).unwrap();

        let new_id = split(&c, a, &fa[2..], Some("v2")).unwrap().expect("应新建人物");
        assert_eq!(refresh_count(&c, new_id).unwrap(), 2);
        assert_eq!(refresh_count(&c, a).unwrap(), 2);
        assert!(get(&c, new_id).unwrap().unwrap().name.is_none(), "新人物不许自动命名");

        // 拆不存在的脸 → 不留空壳
        let empty = split(&c, a, &[99999], None).unwrap();
        assert!(empty.is_none());
    }

    #[test]
    fn 删除人物只解绑不删人脸() {
        let c = open_test_db().unwrap();
        let imgs = seed(&c, 1);
        let a = insert(&c, &Person::new(None, None)).unwrap();
        let fa = add_faces(&c, imgs[0], 3);
        faces::set_person(&c, &fa, Some(a)).unwrap();
        delete(&c, a).unwrap();
        let left = faces::list_by_image(&c, imgs[0]).unwrap();
        assert_eq!(left.len(), 3, "人脸本身必须还在");
        assert!(left.iter().all(|f| f.person_id.is_none()));
    }

    #[test]
    fn 全表重算会清掉没有脸的人物() {
        let c = open_test_db().unwrap();
        let imgs = seed(&c, 2);
        let a = insert(&c, &Person::new(None, None)).unwrap();
        let empty = insert(&c, &Person::new(None, None)).unwrap();
        let fa = add_faces(&c, imgs[0], 2);
        faces::set_person(&c, &fa, Some(a)).unwrap();
        refresh_all(&c).unwrap();
        assert_eq!(get(&c, a).unwrap().unwrap().face_count, 2);
        assert!(get(&c, empty).unwrap().is_none(), "空人物应被清掉");
    }
}
