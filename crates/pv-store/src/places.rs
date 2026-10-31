//! 地点：坐标 → 离线城市名（`pv-geo` 内置表，⛔ 绝不联网），结果落 `image_places`。
//!
//! `image_places` 里**有行 = 已解析**（`place` 为 NULL 表示附近没城市），
//! 所以没有 GPS 的图也照样落行 —— 否则每轮启动都要重读一遍文件。

use pv_index::repo;

use crate::{PlaceCount, Result, Store};

impl Store {
    /// 侧栏「地点」分区：城市名 + 各自张数（多的在前），**全库**。
    pub fn places(&self) -> Result<Vec<PlaceCount>> {
        self.places_scoped(None, None)
    }

    /// 侧栏「地点」分区（**限定范围**）。
    ///
    /// ⭐ 必须和 [`tags_scoped`](Self::tags_scoped) 传同一对 `(library_id, dir_id)`，
    /// 否则标签跟着图库切换、地点却一直是全库数字（2026-10-28 用户报的 bug）。
    pub fn places_scoped(
        &self,
        library_id: Option<i64>,
        dir_id: Option<i64>,
    ) -> Result<Vec<PlaceCount>> {
        Ok(repo::places::list_counts_scoped(&self.conn, library_id, dir_id)?
            .into_iter()
            .map(|(name, count)| PlaceCount { name, count })
            .collect())
    }

    /// 给**还没解析过地点**的图补上地点（分批，`limit` 一轮各做多少）。
    /// 返回 `(本轮解析出城市的张数, 还剩多少张待解析)`。
    ///
    /// 两段式（2026-10-19 修正：`image_exif` 在扫描链路**从不写入**，老库全是空表）：
    /// ① **快路径**：`image_exif` 已有坐标的 → 纯 SQL 取坐标 + 查离线表，不开文件；
    /// ② **慢路径**：剩下的（`image_exif` 没行 / 没 GPS 的）→ 每张 `open` 读一次 EXIF，
    ///    顺带把品牌/机型/镜头/GPS 写进 `image_exif`（搜索命中机型靠它），
    ///    再解析地点。**每张图一生只读一次文件**：解析结果（含 NULL）落 `image_places`，
    ///    下次启动就不会再碰它 —— 和 `backfill_times` 的 `meta_checked` 一个思路。
    ///
    /// ⭐ 城市名一律 `pv-geo` 内置离线表，**绝不联网**。
    /// ⚠️ 拿不到绝对路径（卷没挂载）时**不落行**，等下次挂载再补。
    pub fn backfill_places(&mut self, limit: i64) -> (usize, i64) {
        // 一次性的「死行」清理：旧版本把「有 GPS 但解析失败」的图也标成了已解析
        // （place = NULL），修好解析器后它们不会自动重试 —— 必须清掉重跑。
        // 用 settings 里的标记保证只做一次，别每次启动都全表重读 EXIF。
        const FIX_KEY: &str = "places.recheck_null.v1";
        if repo::settings::get(&self.conn, FIX_KEY).ok().flatten().is_none() {
            let n = repo::places::delete_unresolved(&self.conn).unwrap_or(0);
            let _ = repo::settings::set(&self.conn, FIX_KEY, "1");
            let _ = n; // 只在诊断时关心条数（pv-store 没引 tracing）
        }

        let mut hits = 0usize;

        // ① 快路径：坐标已在索引里
        if let Ok(rows) = repo::places::list_gps_missing(&self.conn, limit) {
            for (id, lat, lon) in rows {
                if self.resolve_place(id, lat, lon) {
                    hits += 1;
                }
            }
        }

        // ② 慢路径：`image_exif` 里没坐标的 —— 开文件读 EXIF（每张一生一次）
        let Ok(ids) = repo::places::list_ids_missing(&self.conn, limit) else {
            return (hits, 0);
        };
        for id in ids {
            let img = match repo::images::get(&self.conn, id) {
                Ok(Some(i)) => i,
                _ => continue,
            };
            let Ok(Some(abs)) = self.abs_path(&img) else {
                continue; // 卷没挂载：不落行，下次启动接着补
            };
            let exif = pv_meta::read_exif(&abs);
            let gps = exif
                .as_ref()
                .and_then(|e| e.gps.as_ref())
                .and_then(|g| g.lat_lon());
            // 顺带把拍摄参数写进 image_exif（⛔ 就算全 None 也要落行，
            // 否则这批图每轮启动都会被重读一遍文件）
            let _ = repo::exif::upsert(
                &self.conn,
                id,
                exif.as_ref().and_then(|e| e.make.as_deref()),
                exif.as_ref().and_then(|e| e.model.as_deref()),
                exif.as_ref().and_then(|e| e.lens.as_deref()),
                gps.map(|(la, _)| la),
                gps.map(|(_, lo)| lo),
            );
            if let Some((lat, lon)) = gps {
                if self.resolve_place(id, lat, lon) {
                    hits += 1;
                }
            } else {
                // 没 GPS：也落一行（place = NULL）标记「已解析」，别再读第二次
                let _ = repo::places::upsert(&self.conn, id, None, None, None);
            }
        }

        (hits, repo::places::count_missing(&self.conn).unwrap_or(0))
    }

    /// 查离线表并把结果写进 `image_places`。返回是否解析出了城市。
    pub(crate) fn resolve_place(&self, id: i64, lat: f64, lon: f64) -> bool {
        let place = pv_geo::nearest_place(lat, lon);
        let hit = place.is_some();
        let _ = repo::places::upsert(
            &self.conn,
            id,
            place.as_ref().map(|p| p.name.as_str()),
            Some(lat),
            Some(lon),
        );
        hit
    }
}
