//! `pv-scan`：把**文件系统**变成**索引**（写侧）。
//!
//! 分模块：
//! - [`format`]  —— 魔数判定（复用 `pv-core`，只看头不信任扩展名）
//! - [`walker`]  —— 目录遍历 → 候选文件
//! - [`plan`]    —— 与已索引条目比对 → 差异计划（纯函数）
//! - [`apply`]   —— 计划落库（单事务）+ 一步到位 `scan_folder`
//! - [`watch`]   —— `notify` 实时监听 + 去抖聚合（变更 → 待重扫目录）
//! - [`volume`]  —— **卷身份**解析（Volume GUID，不用盘符）
//! - [`tree`]    —— [`scan_tree`]（递归首扫）/ [`open_dir`]（单目录，不递归）
//!
//! ⚠️ **尚未实现**（后续步骤）：快速探测宽高、缩略图生成。

// 测试函数名用中文（可读性优先），关闭 snake_case 检查
#![cfg_attr(test, allow(non_snake_case))]

pub mod apply;
pub mod format;
pub mod plan;
pub mod tree;
pub mod volume;
pub mod walker;
pub mod watch;

pub use apply::{apply, scan_folder, Summary};
pub use plan::{diff, FoundFile, ScanPlan};
pub use tree::{open_dir, scan_tree, OpenDir, TreeSummary};
pub use volume::{relative_to_mount, VolumeInfo, VolumeResolver};
pub use walker::{walk_dir, walk_dir_fast};
pub use watch::{ChangeBatch, Debouncer, Watcher};

#[cfg(windows)]
pub use volume::WindowsVolumeResolver;

pub use pv_index::{IndexError, Result};

#[cfg(test)]
mod tests {
    use super::*;
    use pv_index::models::{Folder, Volume};
    use pv_index::{open_test_db, repo, Connection};
    use std::fs;
    use std::path::{Path, PathBuf};

    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0];
    const BMP: &[u8] = b"BM\x00\x00\x00\x00";

    /// 临时目录（测试结束自动清理）。
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let mut p = std::env::temp_dir();
            p.push(format!("pv_scan_{tag}_{}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
        /// 写一个文件（可含子目录）：魔数 + 尾部填充（制造可辨识大小）。
        fn write_at(&self, rel: &str, head: &[u8]) {
            let p = self.0.join(rel);
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            let mut data = head.to_vec();
            data.extend_from_slice(&[0u8; 64]);
            fs::write(p, data).unwrap();
        }
        fn write(&self, name: &str, head: &[u8]) {
            self.write_at(name, head);
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn new_dir(conn: &Connection) -> i64 {
        let v = repo::volumes::insert(conn, &Volume::new("v")).unwrap();
        repo::folders::insert(conn, &Folder::new(v, "d")).unwrap()
    }

    #[test]
    fn 探测只认魔数并跳过非图像() {
        let d = TempDir::new("probe");
        d.write("a.png", PNG);
        d.write("b.jpg", JPEG);
        d.write("note.txt", b"hello, this is not an image");
        d.write(".hidden.png", PNG);

        let found = walk_dir(d.path()).unwrap();
        let names: Vec<&str> = found.iter().map(|f| f.rel_name.as_str()).collect();
        assert_eq!(names, ["a.png", "b.jpg"], "只应找到 2 张（按名排序，隐藏/文本被跳过）");
        assert_eq!(found[0].format, "png");
        assert!(!found[0].is_raw);
        assert_eq!(found[1].format, "jpeg");
        assert!(found.iter().all(|f| f.size > 60), "大小应为头部+填充");
    }

    #[test]
    fn 首扫建索引且幂等() {
        let d = TempDir::new("first");
        d.write("a.png", PNG);
        d.write("b.jpg", JPEG);
        let mut conn = open_test_db().unwrap();
        let dir = new_dir(&conn);

        let s = scan_folder(&mut conn, dir, d.path()).unwrap();
        assert_eq!((s.added, s.changed, s.removed), (2, 0, 0));
        assert_eq!(repo::images::count(&conn).unwrap(), 2);

        // 再扫一次：应无变化、不重复插入
        let s2 = scan_folder(&mut conn, dir, d.path()).unwrap();
        assert_eq!((s2.added, s2.changed, s2.removed), (0, 0, 0));
        assert_eq!(repo::images::count(&conn).unwrap(), 2);
    }

    #[test]
    fn 增量_新增_修改_删除() {
        let d = TempDir::new("inc");
        d.write("a.png", PNG);
        d.write("b.png", PNG);
        let mut conn = open_test_db().unwrap();
        let dir = new_dir(&conn);
        scan_folder(&mut conn, dir, d.path()).unwrap();
        assert_eq!(repo::images::count(&conn).unwrap(), 2);

        // 新增 c.png
        d.write("c.png", PNG);
        let s = scan_folder(&mut conn, dir, d.path()).unwrap();
        assert_eq!((s.added, s.changed, s.removed), (1, 0, 0));

        // a.png 变大（size 变 → changed）
        d.write("a.png", &[PNG, &[1u8; 32]].concat());
        let s = scan_folder(&mut conn, dir, d.path()).unwrap();
        assert_eq!((s.added, s.changed, s.removed), (0, 1, 0));

        // 删掉 b.png
        fs::remove_file(d.path().join("b.png")).unwrap();
        let s = scan_folder(&mut conn, dir, d.path()).unwrap();
        assert_eq!((s.added, s.changed, s.removed), (0, 0, 1));

        let left: Vec<String> = repo::images::list_by_dir(&conn, dir)
            .unwrap()
            .into_iter()
            .map(|i| i.rel_name.unwrap())
            .collect();
        assert_eq!(left, ["a.png", "c.png"]);
    }

    #[test]
    fn 差异计划是纯函数() {
        // 不碰文件系统：直接喂 existing / found
        let existing = vec![
            {
                let mut i = pv_index::models::Image::new(1, "keep.png");
                i.id = 10;
                i.size = Some(100);
                i.mtime_ns = Some(1);
                i
            },
            {
                let mut i = pv_index::models::Image::new(1, "gone.png");
                i.id = 11;
                i
            },
        ];
        let found = vec![
            FoundFile { rel_name: "keep.png".into(), size: 100, mtime_ns: 1, format: "png".into(), is_raw: false },
            FoundFile { rel_name: "new.png".into(), size: 5, mtime_ns: 9, format: "png".into(), is_raw: false },
        ];
        let p = diff(&existing, &found);
        assert_eq!(p.added.len(), 1);
        assert_eq!(p.added[0].rel_name, "new.png");
        assert!(p.changed.is_empty(), "keep.png 未变");
        assert_eq!(p.removed, vec![11]);
    }

    #[test]
    fn BMP也被识别() {
        let d = TempDir::new("bmp");
        d.write("x.bmp", BMP);
        let found = walk_dir(d.path()).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].format, "bmp");
    }

    #[test]
    fn 扫描后可查询_闭环() {
        use pv_index::repo::query::{self, ImageFilter, ImageQuery, SortDir, SortKey};

        let d = TempDir::new("loop");
        d.write("a.png", PNG);
        d.write("b.jpg", JPEG);
        d.write("c.bmp", BMP);

        let mut conn = open_test_db().unwrap();
        let dir = new_dir(&conn);
        let s = scan_folder(&mut conn, dir, d.path()).unwrap();
        assert_eq!(s.added, 3);

        // 读侧：按格式过滤，只取 jpeg
        let f = ImageFilter {
            formats: vec!["jpeg".into()],
            ..Default::default()
        };
        let got = query::list(
            &conn,
            &ImageQuery { filter: f, sort: SortKey::RelName, dir: SortDir::Asc, ..Default::default() },
        )
        .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].rel_name.as_deref(), Some("b.jpg"));

        // 读侧：全部 + 按大小升序（3 张）
        let all = query::list(
            &conn,
            &ImageQuery { sort: SortKey::Size, dir: SortDir::Asc, ..Default::default() },
        )
        .unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(query::count(&conn, &ImageFilter::default()).unwrap(), 3);
    }

    #[test]
    fn 真实监听_写入后能在防抖后报出目录() {
        use std::time::Duration;

        let d = TempDir::new("watch");
        let w = Watcher::spawn(&[d.path().to_path_buf()], Duration::from_millis(150)).unwrap();

        d.write("new.png", PNG); // 写入触发文件事件

        // 最多等 10 秒拿到一批（文件事件很快，留足余量避免偶发慢导致 flaky）
        let batch = w.recv_timeout(Duration::from_secs(10)).expect("应收到变更批次");
        assert!(
            batch.dirs.iter().any(|p| p.file_name() == d.path().file_name()),
            "批次应含被监听目录，实际：{:?}",
            batch.dirs
        );
    }

    /// 假解析器：把给定目录当作一个卷根（用于跨平台单测递归扫描）。
    struct FakeResolver {
        mount: PathBuf,
        guid: String,
    }

    impl VolumeResolver for FakeResolver {
        fn resolve(&self, path: &Path) -> Result<Option<VolumeInfo>> {
            if path.starts_with(&self.mount) {
                Ok(Some(VolumeInfo {
                    volume_guid: self.guid.clone(),
                    serial: Some(42),
                    label: Some("测试卷".into()),
                    fs: Some("NTFS".into()),
                    removable: false,
                    mount_path: self.mount.clone(),
                }))
            } else {
                Ok(None)
            }
        }
    }

    #[test]
    fn 递归首扫建目录树并落图像() {
        let d = TempDir::new("tree");
        d.write_at("a.png", PNG);
        d.write_at("skip.txt", b"not an image");
        d.write_at("sub/b.jpg", JPEG);
        d.write_at("sub/deep/c.bmp", BMP);

        let mut conn = open_test_db().unwrap();
        let resolver = FakeResolver {
            mount: d.path().to_path_buf(),
            guid: r"\\?\Volume{test}\".into(),
        };

        let s = scan_tree(&mut conn, &resolver, d.path()).unwrap();
        assert_eq!(s.folders, 3, "根 + sub + sub/deep");
        assert_eq!(s.images.added, 3);
        assert_eq!(repo::images::count(&conn).unwrap(), 3);

        // 卷已登记（以 GUID 为身份）
        let vols = repo::volumes::list(&conn).unwrap();
        assert_eq!(vols.len(), 1);
        assert!(vols[0].volume_guid.contains("test"));

        // folders 的相对路径正确（卷根为 ""）
        let mut rels: Vec<String> = repo::folders::list_by_volume(&conn, vols[0].id)
            .unwrap()
            .into_iter()
            .map(|f| f.rel_path)
            .collect();
        rels.sort();
        assert_eq!(rels, ["", "sub", "sub/deep"]);

        // 幂等：再扫不新增
        let s2 = scan_tree(&mut conn, &resolver, d.path()).unwrap();
        assert_eq!(s2.folders, 3, "目录不重复建");
        assert_eq!((s2.images.added, s2.images.changed, s2.images.removed), (0, 0, 0));
        assert_eq!(repo::images::count(&conn).unwrap(), 3);
    }

    /// **单目录打开的核心保证：不递归。**
    ///
    /// 用 `scan_tree` 的话点一下盘符就会遍历整块盘（本机 `E:\` 下有近万张图），
    /// 而用户只是想在文件浏览器里看看这个目录里有什么。
    #[test]
    fn 单目录打开不递归子目录() {
        let d = TempDir::new("open");
        d.write_at("a.png", PNG);
        d.write_at("b.jpg", JPEG);
        d.write_at("sub/c.bmp", BMP);

        let mut conn = open_test_db().unwrap();
        let resolver = FakeResolver {
            mount: d.path().to_path_buf(),
            guid: r"\\?\Volume{open}\".into(),
        };

        let r = open_dir(&mut conn, &resolver, d.path()).unwrap();
        assert_eq!(r.images.added, 2, "只该收本目录的 2 张，不含 sub/c.bmp");
        assert_eq!(repo::images::count(&conn).unwrap(), 2, "子目录里的图不该被索引");
        let vols = repo::volumes::list(&conn).unwrap();
        assert_eq!(
            repo::folders::list_by_volume(&conn, vols[0].id).unwrap().len(),
            1,
            "不该为子目录建行"
        );

        // 只看这个目录：查出来的就是它自己的 2 张
        use pv_index::repo::query::{self, ImageFilter, ImageQuery};
        let f = ImageFilter {
            dir_ids: vec![r.folder_id],
            ..Default::default()
        };
        assert_eq!(query::count(&conn, &f).unwrap(), 2);
        let rows = query::list(&conn, &ImageQuery { filter: f, ..Default::default() }).unwrap();
        assert!(rows.iter().all(|i| i.dir_id == r.folder_id));

        // 幂等：重复打开不重复插入、folder_id 稳定
        let r2 = open_dir(&mut conn, &resolver, d.path()).unwrap();
        assert_eq!(r2.folder_id, r.folder_id, "重复打开应复用同一行");
        assert_eq!((r2.images.added, r2.images.changed, r2.images.removed), (0, 0, 0));
    }

    /// 父目录与子目录**各自独立**打开，各看各的图片。
    #[test]
    fn 父子目录各自独立打开() {
        let d = TempDir::new("open2");
        d.write_at("a.png", PNG);
        d.write_at("sub/c.bmp", BMP);

        let mut conn = open_test_db().unwrap();
        let resolver = FakeResolver {
            mount: d.path().to_path_buf(),
            guid: r"\\?\Volume{open2}\".into(),
        };

        let root = open_dir(&mut conn, &resolver, d.path()).unwrap();
        assert_eq!(root.images.added, 1, "根目录只有 a.png");
        let sub = open_dir(&mut conn, &resolver, &d.path().join("sub")).unwrap();
        assert_eq!(sub.images.added, 1, "子目录只有 c.bmp");
        assert_ne!(root.folder_id, sub.folder_id, "父子是两个独立的目录行");

        // 打开子目录**不会**把父目录的图算进子目录
        use pv_index::repo::query::{self, ImageFilter};
        let f = ImageFilter {
            dir_ids: vec![sub.folder_id],
            ..Default::default()
        };
        assert_eq!(query::count(&conn, &f).unwrap(), 1);
    }

    /// 目录读不到要**报错**，不能静默当成「空目录」—— 那会把该目录下
    /// 已索引的条目全删掉（表现为「点了没反应还丢了数据」）。
    #[test]
    fn 打不开的目录返回错误而不是清空() {
        let d = TempDir::new("openbad");
        d.write_at("a.png", PNG);
        let mut conn = open_test_db().unwrap();
        let resolver = FakeResolver {
            mount: d.path().to_path_buf(),
            guid: r"\\?\Volume{bad}\".into(),
        };
        open_dir(&mut conn, &resolver, d.path()).unwrap();
        assert_eq!(repo::images::count(&conn).unwrap(), 1);

        let missing = d.path().join("nope");
        assert!(open_dir(&mut conn, &resolver, &missing).is_err());
        // 关键：原来那 1 张还在
        assert_eq!(repo::images::count(&conn).unwrap(), 1, "报错不该动已有数据");
    }

    #[cfg(windows)]
    #[test]
    fn Windows卷身份解析真实路径() {        let d = TempDir::new("vol");
        let info = WindowsVolumeResolver
            .resolve(d.path())
            .unwrap()
            .expect("临时目录应能解析到卷");
        assert!(
            info.volume_guid.starts_with(r"\\?\Volume{") && info.volume_guid.ends_with('\\'),
            "应拿到卷 GUID 路径，实际：{}",
            info.volume_guid
        );
        assert!(!info.mount_path.as_os_str().is_empty(), "应拿到挂载点");
    }
}
