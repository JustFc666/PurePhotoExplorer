//! 批量导出：把选中的**原图**复制到目标目录。
//!
//! ## 为什么是「复制」而不是「移动」
//! 图库里的条目是**索引**，原图始终在用户磁盘上。导出只读源文件、写副本，
//! 不碰索引也不删原图 —— 用户点了「导出」不该有任何不可逆后果。
//!
//! ## 重名
//! 目标目录里已有同名文件时，自动退成 `name (1).jpg` / `name (2).jpg`，
//! 与 Windows 资源管理器的习惯一致。**同批次内部**也去重（两张不同目录的
//! `IMG_0001.jpg` 导出到同一个文件夹时不会互相覆盖）。
//!
//! ## 跳过
//! 源文件所在目录 == 目标目录 → 跳过。否则会把文件复制成自己的副本
//! （`a.jpg` → `a (1).jpg`），不是用户想要的结果，还会白占一倍空间。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::Result;

/// 单张失败原因（给用户看的短句，不带路径前缀 —— 名字已经单独给了）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportFailure {
    pub name: String,
    pub reason: String,
}

/// 一次导出的结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExportSummary {
    /// 成功复制的张数
    pub copied: usize,
    /// 源目录 == 目标目录而跳过的张数
    pub skipped: usize,
    /// 失败的张数（明细在 `failed`）
    pub failed: Vec<ExportFailure>,
    /// 实际写入的目录（已创建）
    pub dest_dir: PathBuf,
}

impl ExportSummary {
    /// 是否有任何一张没按预期落地（用于前端决定提示语气）。
    pub fn has_problem(&self) -> bool {
        self.skipped > 0 || !self.failed.is_empty()
    }
}

/// 把 `paths` 里的文件复制到 `dest_dir`（不存在就创建）。
///
/// `paths` 里允许包含不存在的文件（卷没挂载 / 已被外部删除），
/// 它们会进 `failed` 而不是让整批失败。
pub fn copy_images(paths: &[String], dest_dir: &Path) -> Result<ExportSummary> {
    std::fs::create_dir_all(dest_dir)?;

    // 目标目录的规范路径：用来判断「源和目标是不是同一个目录」。
    // 盘符大小写、短路径（8.3）、尾随分隔符都可能不一样，必须规范化后再比。
    let dest_canon = normalize_dir(dest_dir);

    let mut out = ExportSummary {
        dest_dir: dest_dir.to_path_buf(),
        ..Default::default()
    };
    // 本批次已经占用的目标名（小写），防止同批次互相覆盖
    let mut used: HashSet<String> = HashSet::new();

    for raw in paths {
        let src = Path::new(raw);
        let Some(name) = src.file_name().and_then(|n| n.to_str()) else {
            out.failed.push(ExportFailure {
                name: raw.clone(),
                reason: "路径不合法".into(),
            });
            continue;
        };

        // 读元数据顺便完成「存在 + 是文件」两项检查
        match std::fs::metadata(src) {
            Ok(m) if m.is_file() => {}
            Ok(_) => {
                out.failed.push(ExportFailure {
                    name: name.into(),
                    reason: "不是文件".into(),
                });
                continue;
            }
            Err(e) => {
                out.failed.push(ExportFailure {
                    name: name.into(),
                    reason: io_reason(&e),
                });
                continue;
            }
        }

        // 同一个目录 → 跳过（复制成自己的副本没有意义）
        if let (Some(src_dir), Some(dst)) = (
            src.parent().and_then(normalize_dir_opt),
            dest_canon.as_deref(),
        ) {
            if src_dir == dst {
                out.skipped += 1;
                continue;
            }
        }

        let target = unique_target(dest_dir, name, &mut used);
        match std::fs::copy(src, &target) {
            Ok(_) => out.copied += 1,
            Err(e) => out.failed.push(ExportFailure {
                name: name.into(),
                reason: io_reason(&e),
            }),
        }
    }

    Ok(out)
}

/// 把 `dir` 规范化；失败就原样返回（不因为规范化失败就不让导出）。
fn normalize_dir(dir: &Path) -> Option<PathBuf> {
    dir.canonicalize().ok()
}

fn normalize_dir_opt(dir: &Path) -> Option<PathBuf> {
    // `canonicalize` 在 Windows 上会展开到 `\\?\C:\...` 形式，两边都用同一套，
    // 比较才可靠。取不到就返回 None（这条跳过判断直接不生效）。
    dir.canonicalize().ok()
}

/// 在 `dir` 里给 `name` 找一个不冲突的文件名。
///
/// `used` 是本批次已分配的名字（小写），`exists` 查磁盘上有没有。
/// `IMG_1.jpg` → `IMG_1 (1).jpg` → `IMG_1 (2).jpg` …
pub fn unique_target(dir: &Path, name: &str, used: &mut HashSet<String>) -> PathBuf {
    // 先试原名
    let mut cand = dir.join(name);
    if is_free(&cand, used) {
        used.insert(cand.to_string_lossy().to_lowercase());
        return cand;
    }

    let (stem, ext) = split_name(name);
    let mut n = 1u32;
    loop {
        let file = match ext {
            Some(e) => format!("{stem} ({n}).{e}"),
            None => format!("{stem} ({n})"),
        };
        cand = dir.join(file);
        if is_free(&cand, used) {
            used.insert(cand.to_string_lossy().to_lowercase());
            return cand;
        }
        n += 1;
        // 防御：正常目录不会有 10 万个同名文件；到这里说明有别的问题，
        // 直接返回当前候选名，交给 `fs::copy` 去报真实的错误。
        if n > 100_000 {
            return cand;
        }
    }
}

fn is_free(path: &Path, used: &HashSet<String>) -> bool {
    if used.contains(&path.to_string_lossy().to_lowercase()) {
        return false;
    }
    !path.exists()
}

/// 拆成（主名, 扩展名）。扩展名保留原大小写。
fn split_name(name: &str) -> (&str, Option<&str>) {
    match name.rfind('.') {
        // 前导点或纯 `.jpg` 这种不当作扩展名
        Some(i) if i > 0 && i < name.len() - 1 => (&name[..i], Some(&name[i + 1..])),
        _ => (name, None),
    }
}

/// 把 IO 错误压成一句人话（不把 `os error 5` 这种甩给用户）。
fn io_reason(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => "文件不存在（卷可能未挂载）".into(),
        std::io::ErrorKind::PermissionDenied => "没有权限".into(),
        std::io::ErrorKind::AlreadyExists => "目标已存在".into(),
        _ => e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "pve_export_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn make_file(dir: &Path, name: &str, body: &[u8]) -> String {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn 复制到新目录并保留内容() {
        let src_dir = tmp_dir("src");
        let dst_dir = tmp_dir("dst");
        let a = make_file(&src_dir, "a.jpg", b"AAA");
        let b = make_file(&src_dir, "b.png", b"BBB");

        let sum = copy_images(&[a, b], &dst_dir).unwrap();
        assert_eq!(sum.copied, 2);
        assert_eq!(sum.skipped, 0);
        assert!(sum.failed.is_empty());
        assert_eq!(std::fs::read(dst_dir.join("a.jpg")).unwrap(), b"AAA");
        assert_eq!(std::fs::read(dst_dir.join("b.png")).unwrap(), b"BBB");

        let _ = std::fs::remove_dir_all(src_dir);
        let _ = std::fs::remove_dir_all(dst_dir);
    }

    #[test]
    fn 重名自动加序号且不覆盖已有文件() {
        let src_dir = tmp_dir("src2");
        let dst_dir = tmp_dir("dst2");
        std::fs::write(dst_dir.join("a.jpg"), b"OLD").unwrap();
        let a = make_file(&src_dir, "a.jpg", b"NEW");

        let sum = copy_images(&[a], &dst_dir).unwrap();
        assert_eq!(sum.copied, 1);
        // 原有文件不动
        assert_eq!(std::fs::read(dst_dir.join("a.jpg")).unwrap(), b"OLD");
        assert_eq!(std::fs::read(dst_dir.join("a (1).jpg")).unwrap(), b"NEW");

        let _ = std::fs::remove_dir_all(src_dir);
        let _ = std::fs::remove_dir_all(dst_dir);
    }

    #[test]
    fn 同批次同名不互相覆盖() {
        let d1 = tmp_dir("d1");
        let d2 = tmp_dir("d2");
        let dst = tmp_dir("out");
        let a = make_file(&d1, "same.jpg", b"ONE");
        let b = make_file(&d2, "same.jpg", b"TWO");

        // 注意：目标目录不能是源目录，这里的 d1/d2/out 是三个不同目录
        let sum = copy_images(&[a, b], &dst).unwrap();
        assert_eq!(sum.copied, 2);
        assert_eq!(std::fs::read(dst.join("same.jpg")).unwrap(), b"ONE");
        assert_eq!(std::fs::read(dst.join("same (1).jpg")).unwrap(), b"TWO");

        for d in [d1, d2, dst] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn 源目录等于目标目录时跳过() {
        let dir = tmp_dir("same");
        let a = make_file(&dir, "a.jpg", b"X");

        let sum = copy_images(&[a], &dir).unwrap();
        assert_eq!(sum.skipped, 1);
        assert_eq!(sum.copied, 0);
        // 不该产生 a (1).jpg
        assert!(!dir.join("a (1).jpg").exists());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn 不存在的文件进失败列表而不是整批失败() {
        let dir = tmp_dir("miss");
        let dst = tmp_dir("missout");
        let a = make_file(&dir, "ok.jpg", b"OK");

        let sum = copy_images(
            &[dir.join("nope.jpg").to_string_lossy().into_owned(), a],
            &dst,
        )
        .unwrap();
        assert_eq!(sum.copied, 1);
        assert_eq!(sum.failed.len(), 1);
        assert_eq!(sum.failed[0].name, "nope.jpg");

        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(dst);
    }

    #[test]
    fn split_name_处理无扩展名与隐藏文件() {
        assert_eq!(split_name("a.jpg"), ("a", Some("jpg")));
        assert_eq!(split_name("a.tar.gz"), ("a.tar", Some("gz")));
        assert_eq!(split_name("noext"), ("noext", None));
        // 前导点：`.gitignore` 整体当文件名
        assert_eq!(split_name(".gitignore"), (".gitignore", None));
        // 结尾点不当作扩展名
        assert_eq!(split_name("weird."), ("weird.", None));
    }

    #[test]
    fn 扩展名大小写保留() {
        let src_dir = tmp_dir("case");
        let dst_dir = tmp_dir("caseout");
        let a = make_file(&src_dir, "PIC.JPG", b"Z");
        let sum = copy_images(&[a], &dst_dir).unwrap();
        assert_eq!(sum.copied, 1);
        assert!(dst_dir.join("PIC.JPG").exists());

        let _ = std::fs::remove_dir_all(src_dir);
        let _ = std::fs::remove_dir_all(dst_dir);
    }
}
