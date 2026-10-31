//! 标签：手工标签与 AI 标签的读写、计数、改名、删除。
//!
//! ⛔ 整组**只改索引** —— 不碰文件、不碰缓存、不联网。

use pv_index::repo;

use crate::{Result, Store, StoreError, TagCount};

impl Store {
    /// 批量设置星级（0–5）。**只改索引**，不动原图、不动缓存。
    pub fn set_rating(&self, ids: &[i64], rating: i64) -> Result<usize> {
        Ok(repo::images::set_rating(&self.conn, ids, rating)?)
    }

    /// 批量设置旗标（0 = 无，1 = 保留）。**只改索引**。
    pub fn set_flag(&self, ids: &[i64], flag: i64) -> Result<usize> {
        Ok(repo::images::set_flag(&self.conn, ids, flag)?)
    }

    /* ── 标签（2026-10-19）────────────────────────────────────────
     * 标签只是索引里的元数据：**改标签不碰文件、不碰缓存**。
     * 手工标签的来源统一记 `source = 'user'`，将来 AI 打的标签记别的来源，
     * 两者在 `image_tags` 里共存，用户手动移除时一起撤（`remove_any`）。
     */

    /// 给一批图**加**同名标签（不存在则新建）。返回建立的挂载数。
    ///
    /// 幂等：同一张图重复加同一个标签不会报错（`INSERT OR REPLACE`）。
    pub fn add_tags(&mut self, ids: &[i64], names: &[String]) -> Result<usize> {
        let names: Vec<&str> = names
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if ids.is_empty() || names.is_empty() {
            return Ok(0);
        }
        let mut n = 0usize;
        for name in names {
            let tag_id = repo::tags::ensure(&self.conn, name, Some("user"))?;
            for id in ids {
                repo::tags::add_image_tag(&self.conn, *id, tag_id, "user", None)?;
                n += 1;
            }
        }
        Ok(n)
    }

    /// 在一批图里挑出「明显不是人像」的（人脸检测可跳过，省时间）。
    ///
    /// 判据与取舍见 [`repo::tags::non_portrait_ids`] —— 只对**打过标**的图做减法，
    /// 没标签的图不在此列（照常检测），所以这不构成对打标的硬依赖。
    pub fn non_portrait_ids(
        &self,
        ids: &[i64],
    ) -> Result<std::collections::HashSet<i64>> {
        Ok(repo::tags::non_portrait_ids(&self.conn, ids)?
            .into_iter()
            .collect())
    }

    /// 一批图里「打过 AI 标签」和「已有人脸」的图数，返回 `(tagged, faced)`。
    pub fn ai_coverage(&self, ids: &[i64]) -> Result<(usize, usize)> {
        Ok((
            repo::tags::count_tagged(&self.conn, ids)?,
            repo::faces::count_images(&self.conn, ids)?,
        ))
    }

    /// **覆盖**一张图的 AI 标签（先删该图所有 `source='ai'` 的挂载，再写新的）。
    ///
    /// ⭐ 用「按图覆盖」而不是「全库先删再写」：中途崩溃/断电时，
    /// 全库先删会让**所有**图的 AI 标签都没了，而按图覆盖最多丢失正在处理的那张。
    ///
    /// `tags` 里是 `(词条, 分数)`；分数写进 `confidence`，前端可用来排序/调试。
    pub fn replace_ai_tags(&mut self, image_id: i64, tags: &[(String, f32)]) -> Result<()> {
        repo::tags::clear_image_source(&self.conn, image_id, "ai")?;
        for (name, score) in tags {
            let tag_id = repo::tags::ensure(&self.conn, name, Some("ai"))?;
            repo::tags::add_image_tag(
                &self.conn,
                image_id,
                tag_id,
                "ai",
                Some(*score as f64),
            )?;
        }
        Ok(())
    }

    /// 清掉历史遗留的「一级类目」派生标签（幂等，见 `repo::tags::drop_redundant_l1`）。
    ///
    /// 只删「这张图已经有二级词」的那些一级标签；只有一级的图保留。
    /// `l1_names` 传空时是空操作。
    pub fn drop_redundant_l1_tags(&mut self, l1_names: &[String]) -> Result<usize> {
        Ok(repo::tags::drop_redundant_l1(&self.conn, l1_names)?)
    }

    /// 从一批图**移除**某个标签（按 id）。返回受影响的张数。
    pub fn remove_tag(&mut self, ids: &[i64], tag_id: i64) -> Result<usize> {
        let mut n = 0usize;
        for id in ids {
            repo::tags::remove_any(&self.conn, *id, tag_id)?;
            n += 1;
        }
        Ok(n)
    }

    /// 侧栏「标签」分区：全部标签 + 各自使用次数（多的在前）。
    pub fn tags(&self) -> Result<Vec<TagCount>> {
        Ok(repo::tags::list_with_counts(&self.conn)?
            .into_iter()
            .map(|(id, name, count)| TagCount { id, name, count })
            .collect())
    }

    /// 带范围的标签计数（图库 / 目录）。
    pub fn tags_scoped(
        &self,
        library_id: Option<i64>,
        dir_id: Option<i64>,
    ) -> Result<Vec<TagCount>> {
        Ok(repo::tags::counts_scoped(&self.conn, library_id, dir_id)?
            .into_iter()
            .map(|(id, name, count)| TagCount { id, name, count })
            .collect())
    }

    /// 一级类目的**去重**图片数（`(标签名, 一级类目)` 映射由调用方给）。
    ///
    /// ⚠️ 不能拿 `tags_scoped` 的二级计数相加代替 —— 一张图挂多个二级会被重复计数。
    pub fn tag_group_counts(
        &self,
        library_id: Option<i64>,
        dir_id: Option<i64>,
        pairs: &[(String, String)],
    ) -> Result<Vec<(String, i64)>> {
        Ok(repo::tags::group_counts(
            &self.conn,
            library_id,
            dir_id,
            pairs,
        )?)
    }

    /// 单张图的全部标签：`(标签名, 来源, 置信度)`。卡片上「标签」按钮用。
    pub fn image_tags(&self, image_id: i64) -> Result<Vec<(String, String, Option<f64>)>> {
        Ok(repo::tags::list_by_image(&self.conn, image_id)?)
    }

    /// 标签选择器用：这批图**各自**已有的标签（`name → 有几张带`）。
    pub fn tag_counts_for(&self, ids: &[i64]) -> Result<Vec<(String, i64)>> {
        Ok(repo::tags::counts_for_images(&self.conn, ids)?)
    }

    /// 标签改名。⚠️ 撞上同名标签要明确报「已有同名标签」，别让 UNIQUE 错误裸奔。
    pub fn rename_tag(&mut self, id: i64, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "标签名不能为空",
            )));
        }
        let dup = repo::tags::list_all(&self.conn)?
            .into_iter()
            .any(|t| t.id != id && t.name == name);
        if dup {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "已经有同名标签了",
            )));
        }
        Ok(repo::tags::rename(&self.conn, id, name)?)
    }

    /// 删除标签（拆掉所有挂载 + 删标签行）。**照片一张不动**。
    pub fn delete_tag(&mut self, id: i64) -> Result<()> {
        Ok(repo::tags::delete(&self.conn, id)?)
    }

    /* ── 地点（2026-10-19）────────────────────────────────────────
     * 坐标 → 城市名一律走 `pv-geo` 内置离线表（⛔ 绝不联网）。
     * 结果落在 `image_places`：**有行 = 已经解析过**（`place` 为 NULL = 附近没城市），
     * 所以没 GPS 的图不会被反复重算——它们压根不进待办列表。
     */
}
