//! 增量判定：把「磁盘现状」与「已索引条目」比对，得出**差异计划**。
//!
//! 纯函数，不碰数据库、不碰文件系统，便于单测。

use pv_index::models::Image;
use std::collections::HashMap;

/// 磁盘上一个候选图像文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundFile {
    /// 所在文件夹内的文件名
    pub rel_name: String,
    pub size: i64,
    pub mtime_ns: i64,
    /// 魔数探测出的格式标签（jpeg/png/cr3…）
    pub format: String,
    pub is_raw: bool,
}

/// 一次扫描产生的差异。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ScanPlan {
    /// 新增（磁盘有、索引没有）
    pub added: Vec<FoundFile>,
    /// 变更（两边都有，但 size/mtime 变了）：`(image_id, 新状态)`
    pub changed: Vec<(i64, FoundFile)>,
    /// 删除（索引有、磁盘没有）：`image_id`
    pub removed: Vec<i64>,
}

impl ScanPlan {
    /// 是否没有任何变化（用于跳过无谓的写入）。
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.changed.is_empty() && self.removed.is_empty()
    }
}

/// 以 `rel_name` 为身份做差异比对。
///
/// 判定规则：`size` 或 `mtime_ns` **任一变化**即视为 `changed`
/// （与缓存键 `volume_guid+rel_path+size+mtime_ns` 同一套失效口径）。
pub fn diff(existing: &[Image], found: &[FoundFile]) -> ScanPlan {
    let mut by_name: HashMap<&str, &Image> = existing
        .iter()
        .filter_map(|i| i.rel_name.as_deref().map(|n| (n, i)))
        .collect();

    let mut plan = ScanPlan::default();
    for f in found {
        match by_name.remove(f.rel_name.as_str()) {
            // 未变
            Some(old) if old.size == Some(f.size) && old.mtime_ns == Some(f.mtime_ns) => {}
            // 变了
            Some(old) => plan.changed.push((old.id, f.clone())),
            // 新增
            None => plan.added.push(f.clone()),
        }
    }
    // 剩下的（索引里有、磁盘上没有）即删除
    plan.removed = by_name.values().map(|i| i.id).collect();
    plan.removed.sort_unstable();
    plan
}
