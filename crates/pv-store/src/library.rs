//! 图库：把目录挂进图库、建库 / 改名 / 删库、按库取图，以及「从图库移除目录」。
//!
//! 这一组最容易踩的坑是**引用关系**：一个目录可以同时挂在多个图库下，
//! 所以「删库」只有在确认子树不再被别的库引用时才允许连索引一起清。

use pv_index::models::Image;
use pv_index::{conn as index_conn, repo};

use crate::{ImageFilter, ImageQuery, Removed, Result, Store, StoreError, ALL_TIERS};

impl Store {
    /// 把一个**已在索引里**的目录归入图库，返回图库 id。
    ///
    /// - `library_id = Some(id)` → 挂到该图库
    /// - `library_id = None` → 按 `name` **查同名图库：有就挂上去，没有才新建**
    ///
    /// ⭐ 同名复用这条很要紧：否则「用文件夹名加入」点两次会得到两个同名图库，
    /// 侧栏里两行长得一模一样，用户根本分不清哪个是哪个。
    ///
    /// ⚠️ 调用前必须先 `open_folder` 把目录扫进索引 —— 目录不在 `folders` 表里
    /// 就挂不上（`library_folders.folder_id` 指着它）。
    ///
    /// `recursive = true` → 连同**全部后代目录**一起挂（用户答了「包含子文件夹」）。
    /// ⛔ 只挂根目录是不够的：图库查询按 `library_folders` 里的 id 精确匹配，
    /// 子目录不在里面就一张都查不到。
    ///
    /// 幂等：重复挂同一个目录不会报错（`INSERT OR IGNORE`）。
    pub fn add_to_library(
        &mut self,
        folder_id: i64,
        name: &str,
        library_id: Option<i64>,
        recursive: bool,
    ) -> Result<i64> {
        /*
         * ⭐ 递归入库必须把**整棵子树**都挂进图库（2026-10-31）。
         *
         * 只挂根目录的话，图库查询走的是
         * `dir_id IN (SELECT folder_id FROM library_folders WHERE library_id=?)`
         * —— 子目录一张都查不出来。表现就是「点了包含子文件夹、状态栏说加了 300 张，
         * 进图库只看到 5 张」，用户会以为扫描没生效。
         */
        let ids = if recursive {
            repo::folders::descendant_ids(&self.conn, folder_id)?
        } else {
            vec![folder_id]
        };

        match library_id {
            Some(id) => {
                for f in &ids {
                    repo::libraries::add_folder(&self.conn, id, *f)?;
                }
                Ok(id)
            }
            None => {
                let existing = repo::libraries::list(&self.conn)?
                    .into_iter()
                    .find(|l| l.name == name);
                match existing {
                    Some(l) => {
                        for f in &ids {
                            repo::libraries::add_folder(&self.conn, l.id, *f)?;
                        }
                        Ok(l.id)
                    }
                    None => Ok(pv_lib::library::create_normal(&self.conn, name, &ids)?),
                }
            }
        }
    }

    /// 新建一个**空图库**（不绑定任何目录）。
    ///
    /// ⚠️ `libraries.name` **没有 UNIQUE 约束** —— 同名会静默插入两条，
    /// 侧栏上就是两个长得一样的行，用户没法分辨。这里主动挡住。
    ///
    /// 空库建成后 `library_folders` 没有行 → 查出来就是 0 张，
    /// 用户可以之后再往里加目录（或者直接拖图进来，后续功能）。
    pub fn create_library(&mut self, name: &str) -> Result<i64> {
        let name = name.trim();
        if name.is_empty() {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "图库名不能为空",
            )));
        }
        let dup = repo::libraries::list(&self.conn)?
            .into_iter()
            .any(|l| l.name == name);
        if dup {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "已经有同名图库了",
            )));
        }
        Ok(pv_lib::library::create_normal(&self.conn, name, &[])?)
    }

    /// 改图库名（**不动成员关系、不动任何文件、不动缓存**）。
    ///
    /// ⚠️ 同样要挡重名：`libraries.name` 没有 UNIQUE 约束。
    /// 改成自己现在的名字不算冲突（用户点了确认但没改字，不该报错）。
    pub fn rename_library(&mut self, id: i64, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "图库名不能为空",
            )));
        }
        let dup = repo::libraries::list(&self.conn)?
            .into_iter()
            .any(|l| l.id != id && l.name == name);
        if dup {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "已经有同名图库了",
            )));
        }
        Ok(repo::libraries::rename(&self.conn, id, name)?)
    }

    /// 新建一个**智能图库**：规则文本按 `pv-lib` 的 DSL 存进 `libraries.rule`。
    ///
    /// 规则写法（空格 = AND）：
    /// `tag:风景` / `rating:>=4` / `type:raw` / `ext:jpg` / `date:2024` / `date:2024-05`
    /// / 裸词 → 全文关键词。
    ///
    /// ⚠️ 规则**必须先解析校验再落库**：语法错的话建出来是个永远空的图库，
    /// 界面上完全看不出哪儿错了（`create_smart` 内部已经 `parse_rule` 过一次）。
    /// ⚠️ 同样挡重名。
    pub fn create_smart_library(&mut self, name: &str, rule: &str) -> Result<i64> {
        let name = name.trim();
        if name.is_empty() {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "图库名不能为空",
            )));
        }
        let rule = rule.trim();
        if rule.is_empty() {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "智能图库的规则不能为空",
            )));
        }
        let dup = repo::libraries::list(&self.conn)?
            .into_iter()
            .any(|l| l.name == name);
        if dup {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "已经有同名图库了",
            )));
        }
        Ok(pv_lib::library::create_smart(&self.conn, name, rule)?)
    }

    /// 按图库取图：**普通库走 `library_folders` JOIN，智能库走规则求值**。
    ///
    /// 为什么不能在 SQL 里一把梭：智能库没有成员目录行，`library_id` 的 JOIN
    /// 结果恒为空 —— 必须先去 `pv-lib` 把规则翻成 `ImageFilter`，再查。
    ///
    /// ⭐ 界面上的**格式筛选 / 文件名搜索**会叠加到规则之上（规则本身的条件不放松）：
    /// 用户在智能库里再点「只看 PNG」是合理预期。
    ///
    /// ⚠️ 规则里含**全文关键词**时走 `pv-lib` 的 id 列表（FTS 命中集），
    /// 此时排序/分页仍由调用方决定不了 —— 按 `taken_at` 倒序返回。
    pub fn query_in_library(&self, q: &ImageQuery, library_id: i64) -> Result<Vec<Image>> {
        let Some(lib) = repo::libraries::get(&self.conn, library_id)? else {
            return Ok(Vec::new());
        };
        if lib.kind != pv_index::models::LibraryKind::Smart {
            let mut q2 = q.clone();
            q2.filter.library_id = Some(library_id);
            return Ok(repo::query::list(&self.conn, &q2)?);
        }

        let rule = pv_lib::parse_rule(lib.rule.as_deref().unwrap_or_default())?;
        if !rule.keywords.is_empty() {
            let ids = pv_lib::library::members(&self.conn, library_id, 0, 0)?;
            return self.images_by_ids(&ids);
        }

        let mut filter = rule.filter;
        if !q.filter.formats.is_empty() {
            filter.formats = q.filter.formats.clone();
        }
        if q.filter.name_like.is_some() {
            filter.name_like = q.filter.name_like.clone();
        }
        let q2 = ImageQuery {
            filter,
            sort: q.sort,
            dir: q.dir,
            limit: q.limit,
            offset: q.offset,
        };
        Ok(repo::query::list(&self.conn, &q2)?)
    }

    /// 图库命中数（智能库按规则算，普通库按 JOIN 算）。
    ///
    /// ⚠️ 智能库**不叠加界面筛选**：侧栏上那个数字是「这个图库一共多少张」，
    /// 不该因为主区点了「只看 PNG」就变。
    pub fn count_in_library(&self, f: &ImageFilter, library_id: i64) -> Result<i64> {
        let Some(lib) = repo::libraries::get(&self.conn, library_id)? else {
            return Ok(0);
        };
        if lib.kind == pv_index::models::LibraryKind::Smart {
            return Ok(pv_lib::library::count_of(&self.conn, &lib)?);
        }
        let mut f2 = f.clone();
        f2.library_id = Some(library_id);
        Ok(repo::query::count(&self.conn, &f2)?)
    }

    /// 删除图库：**连它独占的目录一起从索引里清掉**。
    ///
    /// ## 为什么不再「只删分组」（2026-10-31 改）
    /// 旧语义是「删库 ≠ 删照片」，但那样删完图库后，那些照片仍然挂在
    /// 「全部照片」里（用户反馈：图库删了，全部照片也得清）。
    ///
    /// ## 什么才真的删
    /// 只删**没有别的图库引用**的目录（整棵子树）。同一个目录同时挂在两个库里时，
    /// 删掉 A 库不动它的索引 —— B 库的照片必须还在。
    ///
    /// ⚠️ 删索引会连带删掉这些图的**评分、标签、人脸关联**（都挂在 image_id 上）。
    /// 重扫只能恢复「有哪些图」，恢复不了这些 —— 所以 UI 侧必须先确认。
    ///
    /// ⚠️ **绝不碰磁盘上的原图**，只删索引行 + 缩略图缓存。
    ///
    /// 幂等：id 不存在时返回 `None`，不报错。
    pub fn delete_library(&mut self, id: i64) -> Result<Option<Removed>> {
        if repo::libraries::get(&self.conn, id)?.is_none() {
            return Ok(None);
        }

        // ① 先记下这个库挂了哪些目录 —— 删完库行就查不到了
        let folders = repo::libraries::list_folder_ids(&self.conn, id)?;
        repo::libraries::delete(&self.conn, id)?;

        /*
         * ② 再逐个判断「能不能连索引一起清掉」。
         *
         * ⚠️ **必须在删库之后判**：自己的引用还留着的话 `count_refs_in_subtree`
         *    永远 > 0，一个目录都删不掉。
         * ⚠️ 子树里只要还有**别的**库引用着，就整个跳过：`remove_folder` 删的是
         *    整棵子树，放过去就是把另一个图库的照片一起清了。
         * ⚠️ `remove_folder` 之后该子树的行已经没了，遍历里要能容忍
         *    「目录不存在」（同一个库挂了父子两层时会重复命中）。
         */
        let mut out = Removed::default();
        for fid in folders {
            if repo::folders::get(&self.conn, fid)?.is_none() {
                continue;
            }
            if repo::libraries::count_refs_in_subtree(&self.conn, fid)? > 0 {
                continue;
            }
            let r = self.remove_folder(fid)?;
            out.folders += r.folders;
            out.images += r.images;
            out.thumbs += r.thumbs;
            out.thumbs_orphan += r.thumbs_orphan;
        }
        Ok(Some(out))
    }

    /// 从图库移除一个目录**及其整棵子树**：删索引行 + 顺带清掉它们的缩略图缓存。
    ///
    /// ⚠️ **只动索引与缓存，绝不碰用户的原始图片文件。**
    ///
    /// ## 顺序是关键
    /// 必须**先按索引算出缓存文件路径并删掉，再删索引行**：
    /// 缓存键 = 规范化路径 + size + mtime，索引行一没，`abs_path`/`cache_file`
    /// 就都算不出来了 —— 那些缓存文件会永远变成没人认领的孤儿，且再也无法定位。
    ///
    /// 卷**未挂载**时算不出缓存键（挂载点是缓存键的一部分），这些图的缓存
    /// 只能留着，计入 `thumbs_orphan`。**不能猜路径** —— 猜错会删掉别人的缩略图。
    ///
    /// 幂等：目录不在库里时返回全 0，不报错（重复点「移除」不该弹错）。
    pub fn remove_folder(&mut self, folder_id: i64) -> Result<Removed> {
        let mut out = Removed::default();
        let Some(root) = repo::folders::get(&self.conn, folder_id)? else {
            return Ok(out);
        };

        // 1) 收集子树（含自身）。DFS 的产出是「父在子前」，删的时候反序即可。
        let all = repo::folders::list_by_volume(&self.conn, root.volume_id)?;
        let mut children: std::collections::HashMap<i64, Vec<i64>> =
            std::collections::HashMap::new();
        for f in &all {
            if let Some(p) = f.parent_id {
                children.entry(p).or_default().push(f.id);
            }
        }
        let mut dirs = vec![folder_id];
        let mut stack = vec![folder_id];
        while let Some(id) = stack.pop() {
            if let Some(kids) = children.get(&id) {
                for k in kids {
                    dirs.push(*k);
                    stack.push(*k);
                }
            }
        }

        /*
         * 2) 此刻索引还在，先把**缓存键**算出来（键含绝对路径 + size + mtime，
         *    索引行一没就再也算不出来了）。⚠️ 只算不删 —— 删要等事务提交之后。
         *
         * ⛔ 以前是「边算边删」：万一下面的事务失败回滚，索引行还在、照片还在，
         *    缩略图却被删了 —— 用户看到一整片空白卡片，还以为图坏了。
         *    缓存删早了没有回滚，索引删错了却能回滚，所以顺序只能是「先提交、后删缓存」。
         */
        let mut victims: Vec<i64> = Vec::new();
        let mut cache_keys = Vec::new();
        for dir in &dirs {
            for img in repo::images::list_by_dir(&self.conn, *dir)? {
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
        }
        out.images = victims.len() as u64;
        out.folders = dirs.len() as u64;

        // 3) 删索引行。`images::delete` 会连带清 thumbs / image_tags / ai_results /
        //    image_exif / faces / fts_images —— 所以必须走它，不能自己写 DELETE。
        index_conn::with_tx(&mut self.conn, |tx| {
            for id in &victims {
                repo::images::delete(tx, *id)?;
            }
            // 脸被删了，人物的 face_count / 封面得跟着重算（空人物一并清掉），
            // 否则人像条会停在「457 张脸」而实际只剩几百张，封面还可能指向已删的脸。
            repo::persons::refresh_all(tx)?;
            // 反序：先子后父（`folders.parent_id` 指着父行）
            for id in dirs.iter().rev() {
                // ⚠️ 先断图库成员关系再删目录行：`library_folders` 是 NO ACTION 外键且
                //    SQLite 默认不开 foreign_keys，不显式删就留下指向已删目录的脏行。
                repo::libraries::detach_folder(tx, *id)?;
                repo::folders::delete(tx, *id)?;
            }
            Ok(())
        })?;

        /*
         * 4) 事务已提交 —— 现在删缓存才是安全的。删不掉只是浪费几 MB 磁盘，
         *    不影响正确性，所以计数归计数、不报错。
         */
        for key in &cache_keys {
            if self.l2.remove(key).unwrap_or(false) {
                out.thumbs += 1;
            }
        }

        // 5) L1 里还留着刚删掉的位图，清掉（纯内存，清了不影响别的）
        self.l1.clear();
        Ok(out)
    }
}
