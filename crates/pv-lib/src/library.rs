//! 图库服务：普通 / 智能图库的 CRUD 与求值。
//!
//! ⚠️ **加密图库**（就地容器收纳 + 可自救，手册 §7.3）是 S2.11 的大工程，
//! 本模块只在类型层面认识它（`kind = encrypted`），具体操作暂返回明确错误。

use std::collections::HashSet;

use pv_index::models::{Image, Library, LibraryKind};
use pv_index::repo;
use pv_index::repo::query::{ImageQuery, SortDir, SortKey};
use pv_index::Connection;

use crate::rule::{parse_rule, Rule};
use crate::{LibError, Result};

/// 图库概览：图库本体 + 条目数。
#[derive(Debug, Clone)]
pub struct LibrarySummary {
    pub library: Library,
    pub count: i64,
}

/// 新建**普通图库**并挂载给定文件夹。
pub fn create_normal(conn: &Connection, name: &str, folder_ids: &[i64]) -> Result<i64> {
    let id = repo::libraries::insert(conn, &Library::new(name, LibraryKind::Normal))?;
    for f in folder_ids {
        repo::libraries::add_folder(conn, id, *f)?;
    }
    Ok(id)
}

/// 新建**智能图库**（规则文本）；规则必须先能解析。
pub fn create_smart(conn: &Connection, name: &str, rule_text: &str) -> Result<i64> {
    parse_rule(rule_text)?; // 校验：语法错立刻报，别等到用的时候才炸
    let mut lib = Library::new(name, LibraryKind::Smart);
    lib.rule = Some(rule_text.to_string());
    Ok(repo::libraries::insert(conn, &lib)?)
}

/// 全部图库 + 各自条目数。
pub fn list_summaries(conn: &Connection) -> Result<Vec<LibrarySummary>> {
    let mut out = Vec::new();
    for library in repo::libraries::list(conn)? {
        let count = count_of(conn, &library)?;
        out.push(LibrarySummary { library, count });
    }
    Ok(out)
}

/// 图库条目数。
pub fn count_of(conn: &Connection, library: &Library) -> Result<i64> {
    match library.kind {
        LibraryKind::Normal => {
            let ids = repo::libraries::list_folder_ids(conn, library.id)?;
            if ids.is_empty() {
                return Ok(0);
            }
            let f = pv_index::repo::query::ImageFilter {
                dir_ids: ids,
                ..Default::default()
            };
            Ok(repo::query::count(conn, &f)?)
        }
        LibraryKind::Smart => {
            let ids = eval_smart(conn, library)?;
            Ok(ids.len() as i64)
        }
        // 加密图库：解锁前不可见
        LibraryKind::Encrypted => Ok(0),
    }
}

/// 图库成员（图像 id），带分页。加密图库未解锁时返回明确错误。
pub fn members(conn: &Connection, library_id: i64, limit: i64, offset: i64) -> Result<Vec<i64>> {
    let Some(library) = repo::libraries::get(conn, library_id)? else {
        return Err(LibError::Unsupported(format!("图库 {library_id} 不存在")));
    };
    let mut ids = match library.kind {
        LibraryKind::Normal => {
            let dirs = repo::libraries::list_folder_ids(conn, library_id)?;
            let q = ImageQuery {
                filter: pv_index::repo::query::ImageFilter {
                    dir_ids: dirs,
                    ..Default::default()
                },
                sort: SortKey::TakenAt,
                dir: SortDir::Desc,
                limit: None,
                offset: 0,
            };
            repo::query::list(conn, &q)?
                .into_iter()
                .map(|i: Image| i.id)
                .collect()
        }
        LibraryKind::Smart => eval_smart(conn, &library)?,
        LibraryKind::Encrypted => {
            return Err(LibError::Unsupported(
                "加密图库需先解锁（S2.11 实现，见手册 §7.3）".into(),
            ))
        }
    };
    let off = offset.max(0) as usize;
    let take = if limit > 0 { limit as usize } else { usize::MAX };
    ids = ids.into_iter().skip(off).take(take).collect();
    Ok(ids)
}

/// 求值智能图库：结构化过滤 → 再与全文关键词命中**取交集**。
fn eval_smart(conn: &Connection, library: &Library) -> Result<Vec<i64>> {
    let text = library.rule.clone().unwrap_or_default();
    let rule: Rule = parse_rule(&text)?;

    let q = ImageQuery {
        filter: rule.filter,
        sort: SortKey::TakenAt,
        dir: SortDir::Desc,
        limit: None,
        offset: 0,
    };
    let mut ids: Vec<i64> = repo::query::list(conn, &q)?.into_iter().map(|i| i.id).collect();

    if !rule.keywords.is_empty() {
        // FTS5 默认按空格 AND，正好对应"多词都要命中"
        let query = rule.keywords.join(" ");
        let hit: HashSet<i64> = repo::fts::search(conn, &query, 1_000_000)?.into_iter().collect();
        ids.retain(|id| hit.contains(id));
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pv_index::models::{Folder, Image, LibraryKind, Volume};
    use pv_index::{open_test_db, repo};

    fn seed(conn: &Connection) -> (i64, i64) {
        let v = repo::volumes::insert(conn, &Volume::new("v")).unwrap();
        let d1 = repo::folders::insert(conn, &Folder::new(v, "a")).unwrap();
        let _d2 = repo::folders::insert(conn, &Folder::new(v, "b")).unwrap();

        let mk = |dir: i64, name: &str| {
            let mut i = Image::new(dir, name);
            i.format = Some("jpeg".into());
            repo::images::insert(conn, &i).unwrap()
        };
        let i1 = mk(d1, "a1.jpg");
        let i2 = mk(d1, "a2.jpg");
        // 给 i1 打标签
        let t = repo::tags::ensure(conn, "风景", None).unwrap();
        repo::tags::add_image_tag(conn, i1, t, "ai", None).unwrap();
        // 全文：i2 的名字进 FTS
        repo::fts::upsert(conn, i2, "海边日落.jpg", "海边 日落", "", "三亚").unwrap();
        (d1, i1)
    }

    #[test]
    fn 普通图库的成员与计数() {
        let conn = open_test_db().unwrap();
        let (d1, _) = seed(&conn);
        let id = create_normal(&conn, "家庭", &[d1]).unwrap();

        let libs = list_summaries(&conn).unwrap();
        assert_eq!(libs.len(), 1);
        assert_eq!(libs[0].count, 2, "d1 下两张图");
        assert_eq!(members(&conn, id, 0, 0).unwrap().len(), 2);
        assert_eq!(members(&conn, id, 1, 1).unwrap().len(), 1, "分页");
    }

    #[test]
    fn 智能图库按标签求值() {
        let conn = open_test_db().unwrap();
        let (_, i1) = seed(&conn);
        let id = create_smart(&conn, "风景集", "tag:风景").unwrap();
        assert_eq!(count_of(&conn, &repo::libraries::get(&conn, id).unwrap().unwrap()).unwrap(), 1);
        assert_eq!(members(&conn, id, 0, 0).unwrap(), vec![i1]);
    }

    #[test]
    fn 智能图库按全文词求值() {
        let conn = open_test_db().unwrap();
        let (_, _) = seed(&conn);
        let id = create_smart(&conn, "日落集", "日落").unwrap();
        let got = members(&conn, id, 0, 0).unwrap();
        assert_eq!(got.len(), 1, "FTS 命中一张（海边日落）");
    }

    #[test]
    fn 智能图库规则语法校验() {
        let conn = open_test_db().unwrap();
        assert!(create_smart(&conn, "坏的", "type:music").is_err(), "非法 type 应报错");
        assert!(create_smart(&conn, "坏的2", "rating:abc").is_err());
        assert!(create_smart(&conn, "好的", "rating:>=3 tag:风景").is_ok());
    }

    #[test]
    fn 加密图库暂不可用() {
        let conn = open_test_db().unwrap();
        let mut lib = Library::new("私密", LibraryKind::Encrypted);
        lib.rule = None;
        let id = repo::libraries::insert(&conn, &lib).unwrap();
        let err = members(&conn, id, 0, 0).unwrap_err();
        assert!(matches!(err, LibError::Unsupported(_)));
    }
}
