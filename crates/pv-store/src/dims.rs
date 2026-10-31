//! 尺寸 / EXIF 方向 / 拍摄时间的**探测与回填**。
//!
//! 这三件事都是「开一次文件拿到元数据 → 写回索引」，天然同一批循环，
//! 所以放在一起；判据（`width IS NULL` / `orientation IS NULL` / `meta_checked`）
//! 是幂等的关键，动之前先看清每个函数的注释。

use std::path::Path;

use pv_index::repo;

use crate::{normalize_orientation, Store};

impl Store {
    /// 用解码注册表探一个文件的**显示尺寸**和 EXIF 方向；失败/不支持返回 `None`。
    ///
    /// ⭐ 2026-10-30：返回的是**摆正之后**的宽高（6/8 已互换）。
    /// 库里 `width/height` 存显示尺寸，网格比例、卡片上的「宽×高」、
    /// 裁剪框算法全按它算 —— 存原始像素会让竖拍照片的长宽比整个反掉。
    pub(crate) fn probe_dims(&self, abs: &Path) -> Option<(i64, i64, u16)> {
        let (w, h) = pv_decode::probe_dimensions(&self.registry, abs).ok()?;
        let (w, h) = (i64::from(w), i64::from(h));
        // 读不到 / 值非法（0、>8）都归一成 1：
        // 0 是坏 EXIF，别的软件也不会转它，跟着「不转」才是对的。
        // 同时也把「已检查过」这个事实记下来，别每次重读。
        let o = normalize_orientation(pv_meta::read_orientation(abs));
        if matches!(o, 6 | 8) {
            Some((h, w, o))
        } else {
            Some((w, h, o))
        }
    }

    /// 把一批 id 的缺失尺寸补上。返回补上的数量。
    ///
    /// ⛔ **RAW 一律跳过**：RAW 的探测要把整个文件读进来找内嵌 JPEG 预览
    /// （`MAX_SCAN_BYTES` 128MB，一张 ARW 平均 47MB），一个 600 张 RAW 的目录
    /// 就是 ~28GB 读取 —— 加入图库时会**卡死几分钟**。而且内嵌预览的尺寸
    /// 并不是传感器尺寸，显示出来是误导。
    ///
    /// 探测失败只有两种：卷没挂载（拿不到绝对路径）、格式探不出 ——
    /// 两种都**留空下次再试**，不能瞎写个 0 进去。
    pub(crate) fn fill_dims_for(&mut self, ids: &[i64]) -> usize {
        let mut filled = 0usize;
        for id in ids {
            let img = match repo::images::get(&self.conn, *id) {
                Ok(Some(i)) => i,
                _ => continue,
            };
            if img.is_raw {
                continue;
            }
            let abs = match self.abs_path(&img) {
                Ok(Some(p)) => p,
                _ => continue,
            };
            if let Some((w, h, orientation)) = self.probe_dims(&abs) {
                if repo::images::update_dimensions(&self.conn, *id, w, h).is_ok() {
                    // 方向必须和宽高**同一次**写入：两者来自同一次探测，
                    // 分开写就会出现「宽高换了、方向还是 NULL」的中间态，
                    // 下次回填又会换一遍 → 宽高反复横跳。
                    let _ = repo::images::set_orientation(&self.conn, *id, orientation);
                    filled += 1;
                }
            }
        }
        filled
    }

    /// 补齐某个目录里缺失的尺寸 / 方向。返回补上的数量。
    ///
    /// ⚠️ 判据是「尺寸缺失 **或** 方向没读过」：老库的行尺寸早填过（但按原始像素），
    /// 方向却还是 NULL —— 只看尺寸的话，重扫这个目录也不会把它们纠正过来
    /// （2026-10-30 实测：重启前 7 行一直是 NULL）。
    pub fn fill_folder_dimensions(&mut self, dir_id: i64) -> usize {
        let Ok(rows) = repo::images::list_by_dir(&self.conn, dir_id) else {
            return 0;
        };
        let mut ids: Vec<i64> = rows
            .into_iter()
            .filter(|r| r.width.is_none() || r.height.is_none())
            .map(|r| r.id)
            .collect();
        if let Ok(extra) = repo::images::list_ids_in_dir_missing_orientation(&self.conn, dir_id) {
            ids.extend(extra);
        }
        ids.sort_unstable();
        ids.dedup();
        if ids.is_empty() {
            return 0;
        }
        self.fill_dims_for(&ids)
    }

    /// **全局回填**缺尺寸的行（老库升级用）。
    ///
    /// `limit` 防止大库首启卡太久 —— 没补完的下次启动接着补
    /// （`width IS NULL` 的行会越来越少，最终一轮为空、零开销）。
    /// 返回 (本次补上的数量, 还剩多少没补)。
    pub fn backfill_dimensions(&mut self, limit: i64) -> (usize, i64) {
        let Ok(ids) = repo::images::list_ids_missing_dimensions(&self.conn, limit) else {
            return (0, 0);
        };
        if ids.is_empty() {
            return (0, repo::images::count_missing_dimensions(&self.conn).unwrap_or(0));
        }
        let filled = self.fill_dims_for(&ids);
        let left = repo::images::count_missing_dimensions(&self.conn).unwrap_or(0);
        (filled, left)
    }

    /// 老库升级：补 EXIF 方向，并把 6/8 那批**当年按原始像素存的宽高换回来**。
    ///
    /// ## 为什么不能只靠 `backfill_dimensions`
    /// 那个只看 `width IS NULL` 的行。老库的宽高早填过了（但填的是原始像素），
    /// 所以竖拍照片一直是横的、卡片上的长宽比也是反的（2026-10-30 修的）。
    /// 这里以 `orientation IS NULL` 为「没处理过」的判据，一次过。
    ///
    /// 幂等：处理完的行方向不再是 NULL，下次不再进来。
    /// `limit` 控制单次量（1 万张的库首启别卡太久），没补完的下次接着补。
    ///
    /// 返回 (本次处理的行数, 还剩多少)。
    pub fn backfill_orientation(&mut self, limit: i64) -> (usize, i64) {
        let Ok(ids) = repo::images::list_ids_missing_orientation(&self.conn, limit) else {
            return (0, 0);
        };
        if ids.is_empty() {
            return (
                0,
                repo::images::count_missing_orientation(&self.conn).unwrap_or(0),
            );
        }
        let mut done = 0usize;
        for id in ids {
            let img = match repo::images::get(&self.conn, id) {
                Ok(Some(i)) => i,
                _ => continue,
            };
            // 卷没挂载 → 拿不到路径，留 NULL 等下次（不能瞎写个 1 糊过去）
            let Ok(Some(abs)) = self.abs_path(&img) else {
                continue;
            };
            let o = normalize_orientation(pv_meta::read_orientation(&abs));
            if matches!(o, 6 | 8) {
                if let (Some(w), Some(h)) = (img.width, img.height) {
                    let _ = repo::images::update_dimensions(&self.conn, id, h, w);
                }
            }
            if repo::images::set_orientation(&self.conn, id, o).is_ok() {
                done += 1;
            }
        }
        let left = repo::images::count_missing_orientation(&self.conn).unwrap_or(0);
        (done, left)
    }

    /// 给一批 id 补**拍摄时间**（EXIF `DateTimeOriginal` → Unix 秒）。返回**读到时间**的数量。
    ///
    /// ⚠️ 读不到也照样标记 `meta_checked = 1`：不然「本来就没有 EXIF 的图」
    /// 每次启动都会被重新读一遍，一万张的库就是几十秒的纯浪费。
    ///
    /// ⚠️ 拿不到绝对路径（卷没挂载）时**不标记**，等下次挂载再补。
    pub(crate) fn fill_times_for(&mut self, ids: &[i64]) -> usize {
        let mut filled = 0usize;
        for id in ids {
            let img = match repo::images::get(&self.conn, *id) {
                Ok(Some(i)) => i,
                _ => continue,
            };
            if img.taken_at.is_some() {
                // 已经有时间（用户改过 / 之前读过），不用再读 EXIF
                let _ = repo::images::set_taken_at(&self.conn, *id, None);
                continue;
            }
            let Ok(Some(abs)) = self.abs_path(&img) else {
                continue;
            };
            let taken = pv_meta::read_exif(&abs).and_then(|e| e.taken_at_unix());
            if repo::images::set_taken_at(&self.conn, *id, taken).is_ok() && taken.is_some() {
                filled += 1;
            }
        }
        filled
    }

    /// 把一个目录里**还没有拍摄时间**的行补上（入库扫完之后调用）。
    ///
    /// ⚠️ 代价：每张要 `open` 一次读 EXIF（JPEG 只读前 64KB，RAW 只读文件头）。
    /// 一个 1200 张的目录在机械盘上要多花几秒 —— 入库本来就是个慢动作（有进度条），
    /// 但**浏览路径绝不能做这件事**（那是另一条不写索引的路）。
    pub fn fill_folder_times(&mut self, dir_id: i64) -> usize {
        let Ok(ids) = repo::images::list_ids_missing_taken_in_dir(&self.conn, dir_id) else {
            return 0;
        };
        if ids.is_empty() {
            return 0;
        }
        self.fill_times_for(&ids)
    }

    /// 启动时回填老库缺失的拍摄时间（分批，`limit` 控制一轮做多少）。
    ///
    /// 全补完后每次启动只剩一条 `COUNT` 查询，几乎零开销。
    pub fn backfill_times(&mut self, limit: i64) -> (usize, i64) {
        let Ok(ids) = repo::images::list_ids_missing_taken(&self.conn, limit) else {
            return (0, 0);
        };
        if ids.is_empty() {
            return (0, repo::images::count_missing_taken(&self.conn).unwrap_or(0));
        }
        let filled = self.fill_times_for(&ids);
        let left = repo::images::count_missing_taken(&self.conn).unwrap_or(0);
        (filled, left)
    }

    /// 一次性回填 `added_ts`（老库全 NULL）。用 settings 标记保证只跑一次。
    pub fn backfill_added_ts(&mut self) -> usize {
        const FIX_KEY: &str = "images.added_ts.v1";
        if repo::settings::get(&self.conn, FIX_KEY)
            .ok()
            .flatten()
            .is_some()
        {
            return 0;
        }
        let n = repo::images::backfill_added_ts(&self.conn).unwrap_or(0);
        let _ = repo::settings::set(&self.conn, FIX_KEY, "1");
        n
    }
}
