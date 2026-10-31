//! 文件级操作：重命名单个文件、把一批图从索引里删掉。
//!
//! ⛔ 这两个都**不删原图**：真正把文件丢进回收站的是 `commands/batch`，
//! 那边成功之后才回来清索引与缓存。

use std::path::Path;

use pv_index::{conn as index_conn, repo};

use crate::{Removed, Result, Store, StoreError, ALL_TIERS};

/// 两个路径是否指向**同一个文件**（用于「目标已存在」的判定）。
///
/// ## 为什么需要它
/// Windows 的文件系统大小写不敏感：`IMG_0001.JPG` 与 `img_0001.jpg` 是**同一个文件**，
/// 对后者调 `Path::exists()` 一样返回 `true`。如果拿 `exists()` 直接当"重名"，
/// 用户「只改大小写」的改名会被永远拒绝（报「目标文件名已存在」），
/// 这在改名里是很常见的操作（统一扩展名大小写）。
///
/// ## 判定方式
/// 比 `canonicalize` 的结果（Windows 上走 `GetFinalPathNameByHandle`，
/// 返回的是**磁盘上的真实大小写**）：
/// - 同一个文件 → 两侧结果完全一致 → 允许改名（`fs::rename` 在 NTFS 上能只改大小写）；
/// - 大小写敏感的文件系统（Linux）→ 两个名字本就是不同文件 → 结果不同 → 按重名拒绝。
///
/// 任一 `canonicalize` 失败（文件不存在 / 无权限）就返回 `false`：
/// 宁可按"重名"拒绝，也不冒覆盖用户文件的险。
pub fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

impl Store {
    /// 重命名单个已索引的图片文件，并同步索引与缩略图缓存。
    ///
    /// ## 顺序
    /// 1. 解析绝对路径（卷未挂载 → 报错，无从改名）
    /// 2. 目标已存在 → 报错（**绝不覆盖**，重名会让用户丢文件）。
    ///    例外：目标与源是**同一个文件**（Windows 上只改大小写）→ 放行，
    ///    判定见 [`same_file`]。
    /// 3. **先删旧缓存**（缓存键含路径，改完名就算不出旧键了）
    /// 4. `fs::rename`
    /// 5. 更新索引 `rel_name`（`content_key` 目前未被写入，原样保留）
    ///
    /// 返回新文件名（调用方拼提示用）。
    pub fn rename_image(&mut self, id: i64, new_name: &str) -> Result<String> {
        let Some(img) = repo::images::get(&self.conn, id)? else {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "索引里没有这张图",
            )));
        };
        let Some(old_abs) = self.abs_path(&img)? else {
            return Err(StoreError::Io(std::io::Error::other(
                "所在卷未挂载，无法重命名",
            )));
        };
        let Some(parent) = old_abs.parent() else {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "路径没有父目录",
            )));
        };
        let new_abs = parent.join(new_name);
        if new_name.is_empty() || new_name.contains(['\\', '/', ':', '*', '?', '"', '<', '>', '|'])
        {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "文件名里有非法字符",
            )));
        }
        if new_abs.exists() && !same_file(&old_abs, &new_abs) {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "目标文件名已存在",
            )));
        }

        // 旧缓存必须在改名**之前**清（键里含旧路径）
        for tier in ALL_TIERS {
            let key = self.cache_key(&img, &old_abs, tier);
            let _ = self.l2.remove(&key);
        }

        std::fs::rename(&old_abs, &new_abs)?;
        repo::images::rename(&self.conn, id, new_name, img.content_key.as_deref())?;
        // L1 里的是「旧路径那批」位图，清掉避免与新的混淆
        self.l1.clear();
        Ok(new_name.to_string())
    }

    /// 从图库移除**指定的几张图**（删索引 + 删缩略图缓存）。
    ///
    /// ⚠️ **绝不碰原图**：调用方负责先把文件删到回收站，这里只清索引与缓存。
    ///
    /// ## 顺序同 [`remove_folder`](Self::remove_folder)
    /// ① **先算缓存键**（键含路径 + size + mtime，索引行一没就算不出来）；
    /// ② 删索引（事务，可回滚）；③ **事务提交后**才删缓存文件。
    ///
    /// ⛔ 以前是「边算边删缓存」：事务万一失败回滚，索引行和原图都还在，
    /// 缩略图却被删了 —— 用户看到一整片空白卡片，还以为照片坏了。
    /// 缓存删除不可回滚、索引删除可回滚，所以只能「先提交、后删缓存」。
    pub fn remove_images(&mut self, ids: &[i64]) -> Result<Removed> {
        let mut out = Removed::default();
        let mut victims: Vec<i64> = Vec::new();
        let mut cache_keys = Vec::new();

        for id in ids {
            let Some(img) = repo::images::get(&self.conn, *id)? else {
                continue; // 已经不在了，幂等跳过
            };
            victims.push(img.id);
            match self.abs_path(&img)? {
                Some(abs) => {
                    for tier in ALL_TIERS {
                        cache_keys.push(self.cache_key(&img, &abs, tier));
                    }
                }
                None => out.thumbs_orphan += 1,
            }
        }

        out.images = victims.len() as u64;
        index_conn::with_tx(&mut self.conn, |tx| {
            for id in &victims {
                // `images::delete` 连带删 faces —— 删完必须重算人物，
                // 否则人像条的「N 张脸」虚高，封面还可能指向已删的脸。
                // （`remove_folder` 里同样这一步，两处别漏。）
                repo::images::delete(tx, *id)?;
            }
            repo::persons::refresh_all(tx)?;
            Ok(())
        })?;

        // 事务已提交 —— 现在删缓存才安全。删不掉只是浪费几 MB 磁盘，不影响正确性。
        for key in &cache_keys {
            if self.l2.remove(key).unwrap_or(false) {
                out.thumbs += 1;
            }
        }

        // L1 里还留着刚删掉的位图（纯内存，清了不影响别的）
        self.l1.clear();
        Ok(out)
    }
}
