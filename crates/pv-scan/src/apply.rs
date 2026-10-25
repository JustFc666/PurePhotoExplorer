//! 把差异计划写进索引（**单事务**），以及一步到位的 `scan_folder`。

use crate::plan::{diff, ScanPlan};
use crate::{walker, Result};
use pv_index::models::Image;
use pv_index::repo;
use pv_index::{conn, Connection, IndexError};
use std::path::Path;

/// 一次扫描的落库统计。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub added: u64,
    pub changed: u64,
    pub removed: u64,
}

impl Summary {
    pub fn total(&self) -> u64 {
        self.added + self.changed + self.removed
    }
}

/// 把计划应用到索引。全部在一个事务里，任一失败则整批回滚。
pub fn apply(conn: &mut Connection, dir_id: i64, plan: &ScanPlan) -> Result<Summary> {
    let mut s = Summary::default();
    conn::with_tx(conn, |tx| {
        for f in &plan.added {
            let mut img = Image::new(dir_id, f.rel_name.as_str());
            img.size = Some(f.size);
            img.mtime_ns = Some(f.mtime_ns);
            img.format = Some(f.format.clone());
            img.is_raw = f.is_raw;
            repo::images::insert(tx, &img)?;
            s.added += 1;
        }
        for (id, f) in &plan.changed {
            let mut img = repo::images::get(tx, *id)?
                .ok_or_else(|| IndexError::BadValue(format!("图像 {id} 已不存在")))?;
            img.size = Some(f.size);
            img.mtime_ns = Some(f.mtime_ns);
            img.format = Some(f.format.clone());
            img.is_raw = f.is_raw;
            repo::images::update_meta(tx, &img)?;
            s.changed += 1;
        }
        for id in &plan.removed {
            repo::images::delete(tx, *id)?;
            s.removed += 1;
        }
        Ok(())
    })?;
    Ok(s)
}

/// 扫描某文件夹并落库：读现有条目 → 遍历磁盘 → 差异 → 写库。
///
/// 返回本次的实际变更数（无变化时返回全 0，不做多余写入）。
pub fn scan_folder(conn: &mut Connection, dir_id: i64, abs_dir: &Path) -> Result<Summary> {
    let existing = repo::images::list_by_dir(conn, dir_id)?;
    let found = walker::walk_dir(abs_dir)?;
    let plan = diff(&existing, &found);
    if plan.is_empty() {
        return Ok(Summary::default());
    }
    apply(conn, dir_id, &plan)
}
