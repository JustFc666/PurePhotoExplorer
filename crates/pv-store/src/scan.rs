//! 入库扫描：把目录（单层 / 整棵子树）建进索引，并顺手补齐尺寸与拍摄时间。
//!
//! 「看一眼」的浏览路径不在这里 —— 那是 `browse`，那条路**不写索引**。

use std::path::Path;

use pv_scan::{Summary, TreeSummary};

use crate::{OpenDir, Result, Store};

impl Store {
    /// 递归首扫：把目录树建成 `folders` + 图像。
    ///
    /// ⚠️ **UI 不要用它来「打开一个目录」** —— 那会遍历整棵子树。
    /// 文件浏览器点目录走 [`Store::open_folder`]（单层）。
    pub fn scan_tree(&mut self, root: &Path) -> Result<TreeSummary> {
        Ok(pv_scan::scan_tree(&mut self.conn, self.resolver.as_ref(), root)?)
    }

    /// 打开目录 **并递归包含全部子目录**（用户勾了「包含子文件夹」时用）。
    ///
    /// ## 与 [`Store::open_folder`] 的区别
    /// 那个只登记并扫描**单层**，用于「点一下目录看一眼」；
    /// 这个会走 `scan_tree` 把整棵子树建进 `folders` 并逐目录扫图。
    ///
    /// ## 为什么要逐个目录补尺寸/时间
    /// `scan_tree` 只做魔数探测（不知道像素尺寸、不读 EXIF）。只补根目录的话，
    /// **子目录里的图全都没宽高**：卡片比例退回默认、按分辨率排序全是 0、
    /// 「宽 × 高」标签也空着 —— 而且不会报错，只有用下去才发现不对。
    ///
    /// ⚠️ 目录数超 [`pv_scan::tree::MAX_TREE_DIRS`] 会直接报错（不静默截断）。
    pub fn open_folder_tree(&mut self, dir: &Path) -> Result<pv_scan::TreeSummary> {
        let t = pv_scan::scan_tree(&mut self.conn, self.resolver.as_ref(), dir)?;
        for id in &t.folder_ids {
            self.fill_folder_dimensions(*id);
            self.fill_folder_times(*id);
        }
        Ok(t)
    }

    /// 打开**单个目录**（不递归）：登记卷与目录行、扫描**该目录里**的图片。
    ///
    /// 返回的 `folder_id` 就是「只看这个目录」的查询条件
    /// （`ImageFilter { dir_ids: vec![folder_id] }`）。
    pub fn open_folder(&mut self, dir: &Path) -> Result<OpenDir> {
        let out = pv_scan::open_dir(&mut self.conn, self.resolver.as_ref(), dir)?;
        // 扫完立刻把这一目录里**还没尺寸**的行补上：新扫进来的行本来就没有
        // （`walk_dir` 只做魔数探测），老库迁移来的行也没有。
        // 放在这里而不是扫描层，是因为探测要 pv-decode 的注册表 —— 分层上
        // 扫描层只管「磁盘上有什么」，解码层才知道「这张图多大」。
        self.fill_folder_dimensions(out.folder_id);
        // 拍摄时间同理：`walk_dir` 只读魔数，没有 EXIF 时间。
        // 没有它，「按拍摄时间排序」会静默退化成「按入库时间」（曾经就是坏的）。
        self.fill_folder_times(out.folder_id);
        Ok(out)
    }

    /// 重扫单个已索引文件夹。
    pub fn scan_folder(&mut self, dir_id: i64, abs_dir: &Path) -> Result<Summary> {
        Ok(pv_scan::scan_folder(&mut self.conn, dir_id, abs_dir)?)
    }
}
