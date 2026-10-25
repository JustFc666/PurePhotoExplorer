//! L2：磁盘缩略图库。
//!
//! **本模块只存/取不透明的字节**（编解码交给上层，`pv-cache` 因此不依赖任何图像解码器）。
//! 普通图库用 `.jpg`（当前缩略图就是 JPEG）只是约定；加密图库把**已加密的密文字节**放进
//! 另一个根目录（`vaults/<id>/thumbs`）即可，落盘格式不变（手册 §6.4）。
//!
//! 目录布局：`<root>/<前2位hex>/<key.filename()>.jpg`
//! （`CacheKey` 已含档位，因此不同档位天然落到不同文件。）

use pv_index::{CacheKey, Result};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// 磁盘缩略图库。
pub struct L2 {
    root: PathBuf,
}

impl L2 {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 某个键对应的文件路径（分片目录 + 含档位的文件名）。
    pub fn path_of(&self, key: &CacheKey) -> PathBuf {
        let hex = key.hex();
        self.root
            .join(&hex[..2])
            .join(format!("{}.jpg", key.filename()))
    }

    /// 写入（**先写临时文件再原子改名**，避免半个文件被读到）。
    pub fn put(&self, key: &CacheKey, bytes: &[u8]) -> Result<()> {
        let path = self.path_of(key);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("jpg.tmp");
        std::fs::write(&tmp, bytes)?;
        // 同盘 rename 覆盖是原子的（Windows 上 std 用 MOVEFILE_REPLACE_EXISTING）
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// 读取（不存在返回 `None`）。
    pub fn get(&self, key: &CacheKey) -> Result<Option<Vec<u8>>> {
        match std::fs::read(self.path_of(key)) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn has(&self, key: &CacheKey) -> bool {
        self.path_of(key).is_file()
    }

    /// 删除一个键；返回是否真的删了。
    pub fn remove(&self, key: &CacheKey) -> Result<bool> {
        match std::fs::remove_file(self.path_of(key)) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// 当前占用总字节数。
    pub fn total_bytes(&self) -> Result<u64> {
        let mut files = Vec::new();
        collect_files(&self.root, &mut files);
        Ok(files.iter().map(|f| f.size).sum())
    }

    /// 缓存文件数（设置面板显示「N 个文件 / X MB」用）。
    pub fn count_files(&self) -> usize {
        let mut files = Vec::new();
        collect_files(&self.root, &mut files);
        files.len()
    }

    /// **清空整个缓存目录**（设置面板的「清理缓存」）。
    ///
    /// ⚠️ 只删 `root` 下的文件，**不碰原图、不碰索引**。
    /// 顺手删掉空的分片目录（`thumbs/ab/`）—— 留着几百个空目录既难看，
    /// 也会让下次「占用多少」的遍历多做无用功。删不掉（被占用）就留着，不算错。
    pub fn clear_all(&self) -> Result<u64> {
        let mut files = Vec::new();
        collect_files(&self.root, &mut files);
        let mut freed = 0u64;
        for f in &files {
            if std::fs::remove_file(&f.path).is_ok() {
                freed += f.size;
            }
        }
        // 自底向上删空目录：先收集，再按路径长度倒序删（子目录比父目录长）
        let mut dirs: Vec<PathBuf> = Vec::new();
        collect_dirs(&self.root, &mut dirs);
        dirs.sort_by_key(|p| std::cmp::Reverse(p.as_os_str().len()));
        for d in dirs {
            let _ = std::fs::remove_dir(d); // 非空会失败，正是我们要的
        }
        Ok(freed)
    }

    /// 超过 `limit` 时按**修改时间由旧到新**删除，返回释放的字节数。
    pub fn evict_to_limit(&self, limit: u64) -> Result<u64> {
        let mut files = Vec::new();
        collect_files(&self.root, &mut files);
        let mut total: u64 = files.iter().map(|f| f.size).sum();
        if total <= limit {
            return Ok(0);
        }
        // 越旧越先删
        files.sort_by_key(|f| f.mtime);

        let mut freed = 0u64;
        for f in files {
            if total <= limit {
                break;
            }
            if std::fs::remove_file(&f.path).is_ok() {
                total = total.saturating_sub(f.size);
                freed += f.size;
            }
        }
        Ok(freed)
    }
}

struct FileMeta {
    path: PathBuf,
    size: u64,
    mtime: SystemTime,
}

fn collect_files(dir: &Path, out: &mut Vec<FileMeta>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        let path = entry.path();
        if ft.is_dir() {
            collect_files(&path, out);
        } else if ft.is_file() {
            let Ok(md) = entry.metadata() else { continue };
            out.push(FileMeta {
                path,
                size: md.len(),
                mtime: md.modified().unwrap_or(UNIX_EPOCH),
            });
        }
    }
}

/// 收集 `dir` 下的**所有**子目录（不含 `dir` 自己）—— `clear_all` 用它删空的分片目录。
fn collect_dirs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            let path = entry.path();
            collect_dirs(&path, out);
            out.push(path);
        }
    }
}
