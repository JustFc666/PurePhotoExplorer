//! 查询与聚合：组合查询、侧栏节点、全库统计、设置项读写。
//!
//! 这里只做「读」，唯一的例外是 `set_setting`（设置项本来就没有别的地方放）。

use pv_index::models::Image;
use pv_index::repo;

use crate::{
    folder_display_name, FolderNode, FormatStat, ImageFilter, ImageQuery, LibNode, Result, Stats,
    Store,
};

impl Store {
    /// 组合查询（过滤/排序/分页）。
    pub fn query(&self, q: &ImageQuery) -> Result<Vec<Image>> {
        Ok(repo::query::list(&self.conn, q)?)
    }

    /// 命中数量。
    pub fn count(&self, f: &ImageFilter) -> Result<i64> {
        Ok(repo::query::count(&self.conn, f)?)
    }

    /// 侧栏用：全部文件夹节点（按卷 → 相对路径排序）。
    ///
    /// 计数走一条 `GROUP BY dir_id`（不是逐个 `count_by_dir`）——文件夹上千时
    /// N+1 会变成上千次查询。
    pub fn folders(&self) -> Result<Vec<FolderNode>> {
        let counts: std::collections::HashMap<i64, i64> =
            repo::images::counts_by_dir(&self.conn)?.into_iter().collect();

        let mut out = Vec::new();
        for vol in repo::volumes::list(&self.conn)? {
            let mount = vol.last_mount_path.clone();
            for f in repo::folders::list_by_volume(&self.conn, vol.id)? {
                let volume_label = vol
                    .label
                    .clone()
                    .filter(|s| !s.trim().is_empty())
                    .or_else(|| mount.clone())
                    .unwrap_or_else(|| vol.volume_guid.clone());
                out.push(FolderNode {
                    id: f.id,
                    parent_id: f.parent_id,
                    name: folder_display_name(mount.as_deref(), &f.rel_path, &volume_label),
                    rel_path: f.rel_path.clone(),
                    image_count: counts.get(&f.id).copied().unwrap_or(0),
                    is_root: f.is_root,
                    volume_id: vol.id,
                    volume_label,
                    mount_path: mount.clone(),
                });
            }
        }
        // `!is_root`：假(0) 排在真(1) 前面 → **根目录在前**，其余按相对路径
        out.sort_by(|a, b| {
            (a.volume_id, !a.is_root, &a.rel_path).cmp(&(b.volume_id, !b.is_root, &b.rel_path))
        });
        Ok(out)
    }

    /// 侧栏用：全部图库 + 各自条目数。
    pub fn libraries(&self) -> Result<Vec<LibNode>> {
        Ok(pv_lib::library::list_summaries(&self.conn)?
            .into_iter()
            .map(|s| LibNode {
                id: s.library.id,
                name: s.library.name,
                kind: s.library.kind.as_str().to_string(),
                rule: s.library.rule,
                count: s.count,
            })
            .collect())
    }

    /// 侧栏图库树的**二级**用：全部图库的成员关系 `(library_id, folder_id)`。
    /// 前端拿它和 `folders()` 按 folder id 拼装（目录名 / 绝对路径 / 张数都在那边）。
    pub fn library_members(&self) -> Result<Vec<(i64, i64)>> {
        Ok(repo::libraries::list_members(&self.conn)?)
    }

    /// 读一个设置项（不存在返回 `None`）。
    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(repo::settings::get(&self.conn, key)?)
    }

    /// 写一个设置项（覆盖）。
    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        Ok(repo::settings::set(&self.conn, key, value)?)
    }

    /// 库里出现过的拍摄年份及张数（顶部「年份」下拉用）。
    pub fn years(&self) -> Result<Vec<(i64, i64)>> {
        Ok(repo::query::list_years(&self.conn)?)
    }

    /// 库里出现过的相机机型及张数（顶部「相机」下拉用）。
    pub fn cameras(&self, limit: i64) -> Result<Vec<(String, i64)>> {
        Ok(repo::exif::list_models(&self.conn, limit)?)
    }

    /// 按 id 取一行索引（批量写文件前要拿 `dir_id` / `rel_name` / `content_key`）。
    pub fn image(&self, id: i64) -> Result<Option<Image>> {
        Ok(repo::images::get(&self.conn, id)?)
    }

    /// 批量取索引行，**顺序与 `ids` 一致**（取不到的跳过）。
    pub fn images_by_ids(&self, ids: &[i64]) -> Result<Vec<Image>> {
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(img) = self.image(*id)? {
                out.push(img);
            }
        }
        Ok(out)
    }

    /// 状态栏 / 筛选条用：全库计数、总字节、分格式统计。
    ///
    /// ⚠️ 这是**全库**统计，不受当前查询过滤影响 —— 筛选条要显示的是
    /// 「每个格式各有多少张」，用它当分母才对得上。
    pub fn stats(&self) -> Result<Stats> {
        let formats: Vec<FormatStat> = repo::images::stats_by_format(&self.conn)?
            .into_iter()
            .map(|(format, count, bytes)| FormatStat {
                format,
                count,
                bytes,
            })
            .collect();
        Ok(Stats {
            total_count: formats.iter().map(|f| f.count).sum(),
            total_bytes: formats.iter().map(|f| f.bytes).sum(),
            formats,
        })
    }
}
