//! 目录 → 索引的两种入口：
//! - [`scan_tree`]：**递归首扫**，把整棵目录树建成 `folders` + 图像
//! - [`open_dir`]：**只登记并扫描单个目录**（不递归），文件浏览器点目录时用

use crate::volume::{relative_to_mount, VolumeInfo, VolumeResolver};
use crate::{scan_folder, walker, Result, Summary};
use pv_index::models::{Folder, Volume};
use pv_index::{repo, Connection, IndexError};
use std::path::{Path, PathBuf};

/// 递归首扫的统计。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TreeSummary {
    /// 处理过的目录数
    pub folders: u64,
    /// 图像增/改/删合计
    pub images: Summary,
}

fn volume_row(info: &VolumeInfo) -> Volume {
    let mut v = Volume::new(info.volume_guid.clone());
    v.serial = info.serial;
    v.label = info.label.clone();
    v.fs = info.fs.clone();
    v.removable = info.removable;
    v.last_mount_path = Some(info.mount_path.to_string_lossy().into_owned());
    v
}

/// 递归扫描 `root`：建/复用**卷**与 `folders`，逐目录落图像。
///
/// - 目录用 **卷 + 相对路径** 建行（换盘符无感，§5.5）；
/// - **不跟随符号链接**（避免环）；单目录访问失败只跳过、不中断整体扫描。
pub fn scan_tree(
    conn: &mut Connection,
    resolver: &dyn VolumeResolver,
    root: &Path,
) -> Result<TreeSummary> {
    let info = resolver
        .resolve(root)?
        .ok_or_else(|| IndexError::BadValue(format!("无法解析卷：{}", root.display())))?;
    let vol_id = repo::volumes::upsert(conn, &volume_row(&info))?;

    let mut summary = TreeSummary::default();
    // 显式栈做 DFS：(目录绝对路径, 父 folder id)
    let mut stack: Vec<(PathBuf, Option<i64>)> = vec![(root.to_path_buf(), None)];

    while let Some((dir, parent_id)) = stack.pop() {
        let Some(rel) = relative_to_mount(&info.mount_path, &dir) else {
            continue; // 不在该卷下，跳过
        };

        let mut f = Folder::new(vol_id, rel);
        f.parent_id = parent_id;
        f.is_root = parent_id.is_none();
        let folder_id = repo::folders::upsert(conn, &f)?;
        summary.folders += 1;

        // 本目录内的图像
        let s = scan_folder(conn, folder_id, &dir)?;
        summary.images.added += s.added;
        summary.images.changed += s.changed;
        summary.images.removed += s.removed;

        // 子目录入栈
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() || !ft.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if walker::is_skippable(&name) {
                continue;
            }
            stack.push((entry.path(), Some(folder_id)));
        }
    }

    Ok(summary)
}

/// 打开**单个目录**（不递归）的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenDir {
    /// 索引里的文件夹 id —— 调用方拿它去做「只看这个目录」的查询
    pub folder_id: i64,
    /// 本次的图像增/改/删
    pub images: Summary,
}

/// 把**单个目录**登记进索引并扫描它 —— **完全不碰子目录**。
///
/// ## 与 [`scan_tree`] 的唯一区别就是「不递归」
/// 文件浏览器里点一个目录时用这个。用 `scan_tree` 会顺着目录树一路往下扫：
/// 点一下盘符就等于**遍历整块盘**（本机 `E:\` 下有近万张图），
/// 用户只是想看看这个目录里有什么，不该触发一次全盘索引。
///
/// 目录**读不到**（不存在 / 无权限 / 盘已拔出）会返回错误，由调用方提示用户 ——
/// 不要吞掉：那会表现为「点了没反应」，比一条明确的报错更难排查。
pub fn open_dir(
    conn: &mut Connection,
    resolver: &dyn VolumeResolver,
    dir: &Path,
) -> Result<OpenDir> {
    let info = resolver
        .resolve(dir)?
        .ok_or_else(|| IndexError::BadValue(format!("无法解析卷：{}", dir.display())))?;
    let vol_id = repo::volumes::upsert(conn, &volume_row(&info))?;

    let rel = relative_to_mount(&info.mount_path, dir)
        .ok_or_else(|| IndexError::BadValue(format!("路径不在卷根之下：{}", dir.display())))?;

    let mut f = Folder::new(vol_id, rel);
    // 直接打开的目录就是「根」：不需要先有父行才能建它
    // （`folders::upsert` 对**已存在**的行只返回 id、不覆盖 parent，所以既有的父子关系保留）
    f.is_root = true;
    let folder_id = repo::folders::upsert(conn, &f)?;

    let images = scan_folder(conn, folder_id, dir)?;
    Ok(OpenDir { folder_id, images })
}
