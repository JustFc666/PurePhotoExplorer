//! PurePhotoExplorer 索引层：SQLite schema、迁移、缓存键、仓储。
//!
//! 分模块（读代码从下往上即可）：
//! - [`conn`]       —— 连接与 pragma
//! - [`schema`]     —— DDL 与 schema 版本
//! - [`migrations`] —— 迁移执行（可重入）
//! - [`models`]     —— 行结构体
//! - [`repo`]       —— 最小 CRUD（volumes / folders / images）
//! - [`cache_key`]  —— 缓存键（S1 起既有）

// 测试函数名用中文（可读性优先），关闭 snake_case 检查
#![cfg_attr(test, allow(non_snake_case))]

pub mod cache_key;
pub mod conn;
pub mod error;
pub mod migrations;
pub mod models;
pub mod repo;
pub mod schema;

pub use cache_key::{compute_key, CacheKey, CacheSource, ThumbTier};
pub use error::{IndexError, Result};
pub use migrations::{migrate, schema_version, SCHEMA_VERSION};
/// 复用底层连接类型，使上层（如 `pv-scan`）无需直接依赖 `rusqlite`。
pub use rusqlite::Connection;

/// 打开一个已迁移好的内存库（测试与本地试验用）。
pub fn open_test_db() -> Result<rusqlite::Connection> {
    let c = conn::open_in_memory()?;
    migrate(&c)?;
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 迁移可重入且版本正确() {
        let c = conn::open_in_memory().unwrap();
        migrate(&c).unwrap();
        assert_eq!(schema_version(&c).unwrap(), SCHEMA_VERSION);
        // 再跑一次不应报错、版本不变
        migrate(&c).unwrap();
        assert_eq!(schema_version(&c).unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn FTS5模块可用() {
        let c = conn::open_in_memory().unwrap();
        migrate(&c).unwrap();
        // 建表时就已包含 fts5 虚拟表；这里做一次读写验证
        c.execute(
            "INSERT INTO fts_images (image_id, filename, keywords, ocr_text, place)
             VALUES (1, '海边日落.jpg', '海边 日落', '', '三亚')",
            [],
        )
        .unwrap();
        let n: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM fts_images WHERE fts_images MATCH '日落'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn 全部表已建立() {
        let c = open_test_db().unwrap();
        let expected = [
            "volumes", "vaults", "folders", "libraries", "library_folders", "places",
            "images", "image_exif", "tags", "image_tags", "ai_results", "persons",
            "faces", "thumbs", "settings",
        ];
        for t in expected {
            let n: i64 = c
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE name = ?1",
                    rusqlite::params![t],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "缺少表 {t}");
        }
    }

    #[test]
    fn 卷文件夹图像往返() {
        let c = open_test_db().unwrap();

        // 卷
        let vid = repo::volumes::insert(&c, &models::Volume::new(r"\\?\Volume{abc}\")).unwrap();
        assert!(vid > 0);

        // 文件夹（卷 + 相对路径）
        let mut f = models::Folder::new(vid, r"2024\春节");
        f.is_root = true;
        let fid = repo::folders::insert(&c, &f).unwrap();

        // 图像
        let mut img = models::Image::new(fid, "IMG_0001.jpg");
        img.size = Some(6_000_000);
        img.mtime_ns = Some(1_700_000_000_000_000_000);
        img.format = Some("jpeg".into());
        img.content_key = Some("deadbeef".into());
        let iid = repo::images::insert(&c, &img).unwrap();

        // 读回校验
        let back = repo::images::get(&c, iid).unwrap().unwrap();
        assert_eq!(back.rel_name.as_deref(), Some("IMG_0001.jpg"));
        assert_eq!(back.size, Some(6_000_000));
        assert_eq!(repo::images::count_by_dir(&c, fid).unwrap(), 1);

        let vol = repo::volumes::get_by_guid(&c, r"\\?\Volume{abc}\").unwrap().unwrap();
        assert_eq!(vol.id, vid);
        assert_eq!(vol.state, models::VolumeState::Online);

        // 拔出：卷 → offline，其下文件夹 → offline
        repo::volumes::set_state(&c, vid, models::VolumeState::Offline).unwrap();
        let n = repo::folders::set_state_by_volume(&c, vid, models::EntryState::Offline).unwrap();
        assert_eq!(n, 1);
        assert_eq!(
            repo::volumes::get_by_guid(&c, r"\\?\Volume{abc}\").unwrap().unwrap().state,
            models::VolumeState::Offline
        );
        // 离线不删数据
        assert_eq!(repo::images::count_by_dir(&c, fid).unwrap(), 1);
    }

    #[test]
    fn 磁盘库WAL持久化并可重开() {
        // 临时文件路径（进程号后缀，避免并发冲突）
        let mut path = std::env::temp_dir();
        path.push(format!("ppx_index_test_{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);

        // 第一次打开：迁移 + 写入，然后关闭
        {
            let c = conn::open(&path).unwrap();
            migrate(&c).unwrap();
            assert_eq!(schema_version(&c).unwrap(), SCHEMA_VERSION);
            repo::volumes::insert(&c, &models::Volume::new("vol-persist")).unwrap();
        }

        // 第二次打开：版本仍在、数据仍在
        {
            let c = conn::open(&path).unwrap();
            assert_eq!(schema_version(&c).unwrap(), SCHEMA_VERSION);
            assert!(repo::volumes::get_by_guid(&c, "vol-persist").unwrap().is_some());
        }

        // 清理（含 WAL 旁文件）
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
        }
    }

    // ── Step 2：其余表仓储 / FTS / 事务 ──

    #[test]
    fn 设置读写与删除() {
        let c = open_test_db().unwrap();
        assert_eq!(repo::settings::get(&c, "theme").unwrap(), None);
        assert_eq!(repo::settings::get_or(&c, "theme", "system").unwrap(), "system");
        repo::settings::set(&c, "theme", "dark").unwrap();
        repo::settings::set(&c, "theme", "light").unwrap(); // 覆盖
        assert_eq!(repo::settings::get(&c, "theme").unwrap().as_deref(), Some("light"));
        repo::settings::delete(&c, "theme").unwrap();
        assert!(repo::settings::get(&c, "theme").unwrap().is_none());
    }

    #[test]
    fn 图库与成员文件夹() {
        let c = open_test_db().unwrap();
        let vid = repo::volumes::insert(&c, &models::Volume::new("v1")).unwrap();
        let f1 = repo::folders::insert(&c, &models::Folder::new(vid, "a")).unwrap();
        let f2 = repo::folders::insert(&c, &models::Folder::new(vid, "b")).unwrap();

        let mut lib = models::Library::new("家庭照片", models::LibraryKind::Normal);
        lib.cover_image = None;
        let lid = repo::libraries::insert(&c, &lib).unwrap();
        repo::libraries::add_folder(&c, lid, f1).unwrap();
        repo::libraries::add_folder(&c, lid, f2).unwrap();
        repo::libraries::add_folder(&c, lid, f2).unwrap(); // 重复应忽略
        assert_eq!(repo::libraries::list_folder_ids(&c, lid).unwrap(), vec![f1, f2]);

        let got = repo::libraries::get(&c, lid).unwrap().unwrap();
        assert_eq!(got.name, "家庭照片");
        assert_eq!(got.kind, models::LibraryKind::Normal);

        repo::libraries::remove_folder(&c, lid, f1).unwrap();
        assert_eq!(repo::libraries::list_folder_ids(&c, lid).unwrap(), vec![f2]);

        // 删图库只删定义与关系，不动文件（此处用 folders 仍在来印证）
        repo::libraries::delete(&c, lid).unwrap();
        assert!(repo::libraries::get(&c, lid).unwrap().is_none());
        assert!(repo::libraries::list_folder_ids(&c, lid).unwrap().is_empty());
        assert!(repo::folders::get(&c, f1).unwrap().is_some(), "文件夹不应被删");
    }

    #[test]
    fn 标签ensure幂等与挂载() {
        let c = open_test_db().unwrap();
        let vid = repo::volumes::insert(&c, &models::Volume::new("v")).unwrap();
        let fid = repo::folders::insert(&c, &models::Folder::new(vid, "d")).unwrap();
        let iid = repo::images::insert(&c, &models::Image::new(fid, "a.jpg")).unwrap();

        let t1 = repo::tags::ensure(&c, "风景", Some("manual")).unwrap();
        let t2 = repo::tags::ensure(&c, "风景", Some("manual")).unwrap();
        assert_eq!(t1, t2, "同名标签应复用同一 id");

        repo::tags::add_image_tag(&c, iid, t1, "manual", None).unwrap();
        let tags = repo::tags::list_by_image(&c, iid).unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].0, "风景");

        repo::tags::remove_image_tag(&c, iid, t1, "manual").unwrap();
        assert!(repo::tags::list_by_image(&c, iid).unwrap().is_empty());
        assert_eq!(repo::tags::list_all(&c).unwrap().len(), 1, "标签本体仍在");
    }

    #[test]
    fn 缩略图索引与容量统计() {
        let c = open_test_db().unwrap();
        let vid = repo::volumes::insert(&c, &models::Volume::new("v")).unwrap();
        let fid = repo::folders::insert(&c, &models::Folder::new(vid, "d")).unwrap();
        let iid = repo::images::insert(&c, &models::Image::new(fid, "a.jpg")).unwrap();

        let mk = |tier, bytes| models::Thumb {
            image_id: iid,
            tier,
            cache_path: Some(format!("ab/x/{tier}.webp")),
            bytes: Some(bytes),
            generated_ts: Some(1),
        };
        repo::thumbs::upsert(&c, &mk(256, 1000)).unwrap();
        repo::thumbs::upsert(&c, &mk(512, 3000)).unwrap();
        assert_eq!(repo::thumbs::total_bytes(&c).unwrap(), 4000);
        assert!(repo::thumbs::get(&c, iid, 256).unwrap().is_some());

        repo::thumbs::upsert(&c, &mk(256, 1500)).unwrap(); // 覆盖同档
        assert_eq!(repo::thumbs::total_bytes(&c).unwrap(), 4500);

        assert_eq!(repo::thumbs::delete_image(&c, iid).unwrap(), 2);
        assert_eq!(repo::thumbs::total_bytes(&c).unwrap(), 0);
    }

    #[test]
    fn 加密容器记录() {
        let c = open_test_db().unwrap();
        let mut v = models::Vault::new("私密图库");
        v.kdf_params = Some("argon2id:m=64MiB,t=3,p=4".into());
        v.salt = Some(vec![1, 2, 3]);
        let id = repo::vaults::insert(&c, &v).unwrap();

        let got = repo::vaults::get(&c, id).unwrap().unwrap();
        assert_eq!(got.name.as_deref(), Some("私密图库"));
        assert_eq!(got.salt.as_deref(), Some(&[1u8, 2, 3][..]));
        assert_eq!(repo::vaults::list(&c).unwrap().len(), 1);

        repo::vaults::set_last_unlock(&c, id, Some(123)).unwrap();
        assert_eq!(repo::vaults::get(&c, id).unwrap().unwrap().last_unlock_ts, Some(123));
    }

    #[test]
    fn FTS同步检索与更新() {
        let c = open_test_db().unwrap();
        repo::fts::upsert(&c, 7, "海边日落.jpg", "海边 日落", "", "三亚").unwrap();
        assert_eq!(repo::fts::search(&c, "日落", 10).unwrap(), vec![7]);

        // 覆盖：同 image_id 不应产生重复；旧内容应消失
        repo::fts::upsert(&c, 7, "海边日出.jpg", "海边 日出", "", "三亚").unwrap();
        assert!(repo::fts::search(&c, "日落", 10).unwrap().is_empty());
        assert_eq!(repo::fts::search(&c, "日出", 10).unwrap(), vec![7]);

        repo::fts::delete(&c, 7).unwrap();
        assert!(repo::fts::search(&c, "日出", 10).unwrap().is_empty());
    }

    #[test]
    fn 事务提交与回滚() {
        let mut c = open_test_db().unwrap();

        // 提交
        conn::with_tx(&mut c, |tx| {
            repo::settings::set(tx, "k1", "v1")?;
            repo::settings::set(tx, "k2", "v2")?;
            Ok(())
        })
        .unwrap();
        assert_eq!(repo::settings::get(&c, "k1").unwrap().as_deref(), Some("v1"));

        // 回滚：闭包返回 Err，前面的写入应全部撤销
        let r: Result<()> = conn::with_tx(&mut c, |tx| {
            repo::settings::set(tx, "k3", "v3")?;
            Err(IndexError::BadValue("故意失败".into()))
        });
        assert!(r.is_err());
        assert!(repo::settings::get(&c, "k3").unwrap().is_none(), "回滚后不应存在");
    }

    #[test]
    fn 图像更新级联删除与FTS清理() {
        let c = open_test_db().unwrap();
        let vid = repo::volumes::insert(&c, &models::Volume::new("v")).unwrap();
        let fid = repo::folders::insert(&c, &models::Folder::new(vid, "d")).unwrap();
        let mut img = models::Image::new(fid, "a.jpg");
        let id = repo::images::insert(&c, &img).unwrap();

        img.id = id;
        img.rating = 4;
        img.format = Some("jpeg".into());
        repo::images::update_meta(&c, &img).unwrap();
        let back = repo::images::get(&c, id).unwrap().unwrap();
        assert_eq!(back.rating, 4);
        assert_eq!(back.format.as_deref(), Some("jpeg"));
        assert_eq!(repo::images::list_by_dir(&c, fid).unwrap().len(), 1);
        assert!(repo::images::get_by_content_key(&c, "nope").unwrap().is_none());

        // 附属数据
        repo::thumbs::upsert(
            &c,
            &models::Thumb { image_id: id, tier: 256, cache_path: None, bytes: Some(9), generated_ts: None },
        )
        .unwrap();
        let tag = repo::tags::ensure(&c, "风景", None).unwrap();
        repo::tags::add_image_tag(&c, id, tag, "ai", Some(0.9)).unwrap();
        repo::fts::upsert(&c, id, "a.jpg", "风景", "", "").unwrap();

        // 级联删除：图像 + 缩略图 + 标签挂载 + FTS 都应清掉
        repo::images::delete(&c, id).unwrap();
        assert_eq!(repo::images::count(&c).unwrap(), 0);
        assert_eq!(repo::thumbs::total_bytes(&c).unwrap(), 0);
        assert!(repo::tags::list_by_image(&c, id).unwrap().is_empty());
        assert!(repo::fts::search(&c, "风景", 10).unwrap().is_empty(), "FTS 不应留幽灵条目");
    }
}
