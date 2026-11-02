//! 图像**组合查询**（网格/搜索/智能图库共用的读路径）。
//!
//! 设计要点：
//! - **过滤全部下推到 SQL**（用参数绑定），不在应用层捞出后再过滤；
//! - 标签/GPS 用 `EXISTS` 子查询，避免 JOIN 产生重复行；
//! - 分页用 `LIMIT/OFFSET`，配合 `count` 拿总数。

use crate::error::Result;
use crate::models::Image;
use rusqlite::types::Value;
use rusqlite::Connection;

/// 排序键。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    /// 拍摄时间（缺失时回退到入库时间）
    TakenAt,
    /// 文件名
    RelName,
    /// 文件大小
    Size,
    /// 文件修改时间
    Mtime,
    /// 文件类型 / 格式标签（`jpeg` / `png` / `raw`…），同类型聚在一起
    Format,
    /// 评分
    Rating,
    /// 分辨率（宽 × 高 的像素总数）
    Dims,
    /// 旗标（0 / 1）
    Flag,
    /// 入库时间
    Added,
    /// 随机（⚠️ 每次求值不同，分页语义不稳定）
    Random,
}

/// 排序方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDir {
    Asc,
    Desc,
}

impl SortDir {
    const fn sql(self) -> &'static str {
        match self {
            Self::Asc => "ASC",
            Self::Desc => "DESC",
        }
    }
}

/// 过滤条件（全部可选，之间是 AND）。
#[derive(Debug, Clone, Default)]
pub struct ImageFilter {
    /// 限定文件夹（空 = 不限）
    pub dir_ids: Vec<i64>,
    /// 限定图库（JOIN `library_folders`）
    pub library_id: Option<i64>,
    /// 文件名包含（大小写不敏感的子串；`%`/`_` 会被转义，当字面量处理）
    pub name_like: Option<String>,
    /// 限定格式（精确匹配 `images.format`，如 "jpeg"/"png"）
    pub formats: Vec<String>,
    /// 是否相机 RAW
    pub is_raw: Option<bool>,
    /// 评分下限
    pub min_rating: Option<i64>,
    /// 标记：0 none / 1 pick / 2 reject
    pub flag: Option<i64>,
    /// 拍摄时间区间（epoch 秒，闭区间）
    pub taken_from: Option<i64>,
    pub taken_to: Option<i64>,
    /// 文件大小区间（字节，闭区间）
    pub min_size: Option<i64>,
    pub max_size: Option<i64>,
    /// 同时具备这些标签（按名）
    pub tags_all: Vec<String>,
    /// 具备其中任一标签
    pub tags_any: Vec<String>,
    /// 是否有 GPS
    pub has_gps: Option<bool>,
    /// 地点名（离线城市名，精确匹配 `image_places.place`），任一命中即可
    pub places: Vec<String>,
    /// 拍摄年份（4 位，如 2021），任一命中即可
    ///
    /// ⭐ 用 `COALESCE(taken_at, added_ts)`：没有 EXIF 拍摄时间的图不至于在
    /// 年份筛选里消失（它们按入库时间算，跟排序口径一致）。
    pub years: Vec<i64>,
    /// 是否有标签：Some(true)=只含有标签的，Some(false)=只看**无标签**的
    pub has_tags: Option<bool>,
    /// 相机机型（精确匹配 `image_exif.model`），任一命中即可
    ///
    /// 用**精确匹配**而不是 LIKE：下拉框里的值就是从库里 GROUP BY 出来的，
    /// 模糊匹配会让「Canon EOS R5」把「Canon EOS R5 Mark II」也带进来。
    pub cameras: Vec<String>,
    /// 限定人物（人脸归纳）：只出这个人出现过的图。
    ///
    /// ⚠️ 用 `IN (SELECT …)` 而不是 JOIN `faces`：一张合影里同一个人只算一张图，
    /// JOIN 会把同一张图复制成多行。
    pub person_id: Option<i64>,
    /// 只出**检出过人脸**的图（`Some(true)`）。
    ///
    /// ⭐ 2026-11-02：标签条上的「人像」改按这个口径筛 —— 用户的心智是
    /// 「有脸就是人像」，而不是「CN-CLIP 恰好打上了人像/单人/合照」。
    /// ⚠️ 与 `person_id` 的区别：那个要求脸已经聚进某个**人物**（约 70%），
    /// 这里只要有 `faces` 行就算（100%）。
    pub has_faces: Option<bool>,
    /// 这张图**恰好**有 N 张脸（「单人」= 1，按检出人脸算，不按标签）。
    ///
    /// ⭐ 2026-11-02 用户口径：单人 + 合照 = 人像。合照 = `face_count_min = 2`。
    pub face_count_eq: Option<i64>,
    /// 这张图有 **≥N** 张脸（合照用它，因为三个人也是合照）。
    pub face_count_min: Option<i64>,
}

/// 一次查询：过滤 + 排序 + 分页。
#[derive(Debug, Clone)]
pub struct ImageQuery {
    pub filter: ImageFilter,
    pub sort: SortKey,
    pub dir: SortDir,
    pub limit: Option<i64>,
    pub offset: i64,
}

impl Default for ImageQuery {
    fn default() -> Self {
        Self {
            filter: ImageFilter::default(),
            sort: SortKey::TakenAt,
            dir: SortDir::Desc,
            limit: None,
            offset: 0,
        }
    }
}

fn placeholders(n: usize) -> String {
    vec!["?"; n].join(", ")
}

fn sort_expr(k: SortKey) -> &'static str {
    match k {
        SortKey::TakenAt => "COALESCE(images.taken_at, images.added_ts)",
        SortKey::RelName => "images.rel_name",
        SortKey::Size => "images.size",
        SortKey::Mtime => "images.mtime_ns",
        // 空格式兜底成空串，避免 NULL 在升序里被排到最前（看起来像「未知类型在最上面」）
        SortKey::Format => "COALESCE(images.format, '')",
        SortKey::Rating => "images.rating",
        // 用**像素总数**而不是宽或高之一：竖图横图混排时，「哪张更清晰」
        // 只有面积说得清（4000×3000 和 3000×4000 是同一档）。
        // 老库可能有 width/height 为 NULL 的行，兜底成 0。
        SortKey::Dims => "(COALESCE(images.width, 0) * COALESCE(images.height, 0))",
        SortKey::Flag => "images.flag",
        SortKey::Added => "images.added_ts",
        SortKey::Random => "RANDOM()",
    }
}

/// 由过滤条件拼出 `WHERE ...`（含前导空格），并把绑定值推进 `p`。
fn where_clause(f: &ImageFilter, p: &mut Vec<Value>) -> String {
    let mut w: Vec<String> = Vec::new();

    if !f.dir_ids.is_empty() {
        w.push(format!("images.dir_id IN ({})", placeholders(f.dir_ids.len())));
        p.extend(f.dir_ids.iter().map(|v| Value::Integer(*v)));
    }
    if let Some(lib) = f.library_id {
        w.push(
            "images.dir_id IN (SELECT folder_id FROM library_folders WHERE library_id = ?)".into(),
        );
        p.push(Value::Integer(lib));
    }
    // 搜索：**文件名 / 标签名 / 地点名 / 机型·镜头**，任一命中即可
    // （2026-10-19 接标签与地点时一层层扩出来的）。
    //
    // ⭐ 多个词按空格切开后是 **AND**：`海边 日落` 要同时命中两个词。
    // 单关键词（绝大多数情况）与之前完全等价。
    //
    // ⚠️ 为什么不用 FTS5：`fts_images` 用的是 unicode61 分词器，**不切中文** ——
    // 存进去的"海边日落"是一个整词，用户搜"日落"匹配不到。
    // 要支持中文任意子串只能用 LIKE（几千到几万行的索引上就是毫秒级）。
    //
    // ⚠️ 必须转义 `%` / `_` 并声明 `ESCAPE '\'`：用户输入里带这两个字符时，
    // 不转义就把它们当通配符了（搜 `a_b` 会匹配到 `axb`）。
    // ⚠️ 绑定值里先转义反斜杠本身，顺序不能反，否则会把刚加的转义符再转一遍。
    if let Some(s) = f
        .name_like
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        for raw in s.split_whitespace() {
            let escaped = raw
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            if escaped.is_empty() {
                continue;
            }
            w.push(
                "(images.rel_name LIKE ? ESCAPE '\\'
                  OR EXISTS (SELECT 1 FROM image_tags it JOIN tags t ON t.id = it.tag_id
                              WHERE it.image_id = images.id AND t.name LIKE ? ESCAPE '\\')
                  OR EXISTS (SELECT 1 FROM image_places p
                              WHERE p.image_id = images.id AND p.place LIKE ? ESCAPE '\\')
                  OR EXISTS (SELECT 1 FROM image_exif e
                              WHERE e.image_id = images.id
                                AND (COALESCE(e.make,'') LIKE ? ESCAPE '\\'
                                  OR COALESCE(e.model,'') LIKE ? ESCAPE '\\'
                                  OR COALESCE(e.lens,'') LIKE ? ESCAPE '\\')))"
                    .into(),
            );
            // ⚠️ 同一个 pattern 要绑定 **6 次**（文件名 / 标签名 / 地点名 / 品牌 / 机型 / 镜头），
            // 少一个就会「绑定值数量对不上」或条件错位 —— 加分支时这里必须同步。
            let pat = Value::Text(format!("%{escaped}%"));
            for _ in 0..6 {
                p.push(pat.clone());
            }
        }
    }
    if !f.formats.is_empty() {
        w.push(format!("images.format IN ({})", placeholders(f.formats.len())));
        p.extend(f.formats.iter().map(|s| Value::Text(s.clone())));
    }
    if let Some(raw) = f.is_raw {
        w.push("images.is_raw = ?".into());
        p.push(Value::Integer(raw as i64));
    }
    if let Some(r) = f.min_rating {
        w.push("images.rating >= ?".into());
        p.push(Value::Integer(r));
    }
    if let Some(fl) = f.flag {
        w.push("images.flag = ?".into());
        p.push(Value::Integer(fl));
    }
    if let Some(t) = f.taken_from {
        w.push("images.taken_at >= ?".into());
        p.push(Value::Integer(t));
    }
    if let Some(t) = f.taken_to {
        w.push("images.taken_at <= ?".into());
        p.push(Value::Integer(t));
    }
    if let Some(s) = f.min_size {
        w.push("images.size >= ?".into());
        p.push(Value::Integer(s));
    }
    if let Some(s) = f.max_size {
        w.push("images.size <= ?".into());
        p.push(Value::Integer(s));
    }

    // 标签：全含（GROUP BY + HAVING 计数）
    if !f.tags_all.is_empty() {
        w.push(format!(
            "images.id IN (SELECT it.image_id FROM image_tags it JOIN tags t ON t.id = it.tag_id
                            WHERE t.name IN ({}) GROUP BY it.image_id HAVING COUNT(DISTINCT t.name) = ?)",
            placeholders(f.tags_all.len())
        ));
        p.extend(f.tags_all.iter().map(|s| Value::Text(s.clone())));
        p.push(Value::Integer(f.tags_all.len() as i64));
    }
    // 标签：任一
    if !f.tags_any.is_empty() {
        w.push(format!(
            "EXISTS (SELECT 1 FROM image_tags it JOIN tags t ON t.id = it.tag_id
                      WHERE it.image_id = images.id AND t.name IN ({}))",
            placeholders(f.tags_any.len())
        ));
        p.extend(f.tags_any.iter().map(|s| Value::Text(s.clone())));
    }

    // 地点（离线城市名，侧栏「地点」分区点进来时用）
    if !f.places.is_empty() {
        w.push(format!(
            "EXISTS (SELECT 1 FROM image_places p
                      WHERE p.image_id = images.id AND p.place IN ({}))",
            placeholders(f.places.len())
        ));
        p.extend(f.places.iter().map(|s| Value::Text(s.clone())));
    }

    // 标签有无（顶部「标签」下拉：有标签 / 无标签）
    // ⚠️ 只算**视觉标签**（ai/ocr）。相机、地点这些 EXIF 标签几乎每张都有，
    // 算进去的话「无标签」只剩个位数，筛不出「AI 没认出来」的图。
    match f.has_tags {
        Some(true) => w.push(
            "EXISTS (SELECT 1 FROM image_tags it WHERE it.image_id = images.id AND it.source IN ('ai','ocr','face'))".into(),
        ),
        Some(false) => w.push(
            "NOT EXISTS (SELECT 1 FROM image_tags it WHERE it.image_id = images.id AND it.source IN ('ai','ocr','face'))".into(),
        ),
        None => {}
    }

    // 年份（顶部「年份」下拉）。⚠️ 时间戳是 epoch 秒，先转本地时间的年份再比
    if !f.years.is_empty() {
        w.push(format!(
            "CAST(strftime('%Y', COALESCE(images.taken_at, images.added_ts), 'unixepoch', 'localtime') AS INTEGER) IN ({})",
            placeholders(f.years.len())
        ));
        p.extend(f.years.iter().map(|y| Value::Integer(*y)));
    }

    // 相机机型（顶部「相机」下拉）
    // 人物（人像专栏点某个人时）。一张合影里同一个人只算一张图 → 用 IN 子查询。
    if let Some(pid) = f.person_id {
        w.push("images.id IN (SELECT image_id FROM faces WHERE person_id = ?)".into());
        p.push(Value::Integer(pid));
    }
    // 是否检出过人脸（标签条「人像」用它当筛选依据）
    match f.has_faces {
        Some(true) => w.push("EXISTS (SELECT 1 FROM faces f WHERE f.image_id = images.id)".into()),
        Some(false) => {
            w.push("NOT EXISTS (SELECT 1 FROM faces f WHERE f.image_id = images.id)".into())
        }
        None => {}
    }
    // 脸的数量（单人 = 恰好 1，合照 = ≥2）。用相关子查询，别 JOIN —— 合影会复制成多行
    if let Some(n) = f.face_count_eq {
        w.push("(SELECT COUNT(*) FROM faces f WHERE f.image_id = images.id) = ?".into());
        p.push(Value::Integer(n));
    }
    if let Some(n) = f.face_count_min {
        w.push("(SELECT COUNT(*) FROM faces f WHERE f.image_id = images.id) >= ?".into());
        p.push(Value::Integer(n));
    }
    if !f.cameras.is_empty() {
        w.push(format!(
            "EXISTS (SELECT 1 FROM image_exif e
                      WHERE e.image_id = images.id AND e.model IN ({}))",
            placeholders(f.cameras.len())
        ));
        p.extend(f.cameras.iter().map(|s| Value::Text(s.clone())));
    }

    // GPS（需 EXIF）
    match f.has_gps {
        Some(true) => w.push(
            "EXISTS (SELECT 1 FROM image_exif e WHERE e.image_id = images.id AND e.gps_lat IS NOT NULL)"
                .into(),
        ),
        Some(false) => w.push(
            "NOT EXISTS (SELECT 1 FROM image_exif e WHERE e.image_id = images.id AND e.gps_lat IS NOT NULL)"
                .into(),
        ),
        None => {}
    }

    if w.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", w.join(" AND "))
    }
}

/// 拼 `ORDER BY` 的内容（**不含**前导关键字）。
///
/// ## `TakenAt` 要特殊处理
/// 没有拍摄时间的照片（截图、下载图、扫描件）如果拿「入库时间」顶替，
/// 会被按入库时间插进真实拍摄时间中间 —— 用户选「按拍摄时间」看到的就是
/// 一堆乱序的图。所以一律**沉底**：
///
/// ```sql
/// (images.taken_at IS NULL) ASC, images.taken_at DESC
/// ```
/// 第一小节与升降无关（有时间的固定在前），第二小节才按方向排。
///
/// 末尾统一加 `images.id` 兜底：主键相同（同大小、同时间）时分页才不会漂。
fn order_clause(k: SortKey, dir: SortDir) -> String {
    match k {
        SortKey::TakenAt => format!(
            "(images.taken_at IS NULL) ASC, images.taken_at {}, images.id {}",
            dir.sql(),
            dir.sql()
        ),
        _ => format!("{} {}, images.id {}", sort_expr(k), dir.sql(), dir.sql()),
    }
}

/// 执行查询，返回图像列表。
pub fn list(conn: &Connection, q: &ImageQuery) -> Result<Vec<Image>> {
    let mut p: Vec<Value> = Vec::new();
    let where_sql = where_clause(&q.filter, &mut p);

    // ⭐ 额外算一列 `has_tags`：卡片要显示「无匹配」，但不想为它再发一次 IPC
    let mut sql = format!(
        "SELECT {}, EXISTS(SELECT 1 FROM image_tags it WHERE it.image_id = images.id) AS has_tags
           FROM images{} ORDER BY {}",
        super::images::COLS,
        where_sql,
        order_clause(q.sort, q.dir)
    );
    match q.limit {
        Some(l) => {
            sql.push_str(" LIMIT ? OFFSET ?");
            p.push(Value::Integer(l));
            p.push(Value::Integer(q.offset));
        }
        None if q.offset > 0 => {
            sql.push_str(" LIMIT -1 OFFSET ?");
            p.push(Value::Integer(q.offset));
        }
        None => {}
    }

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(p), |row| {
        let mut img = super::images::map_row(row)?;
        img.has_tags = row.get::<_, i64>(13)? != 0;
        Ok(img)
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 只统计命中数量（分页界面上显示"共 N 张"）。
pub fn count(conn: &Connection, f: &ImageFilter) -> Result<i64> {
    let mut p: Vec<Value> = Vec::new();
    let where_sql = where_clause(f, &mut p);
    let sql = format!("SELECT COUNT(*) FROM images{where_sql}");
    Ok(conn.query_row(&sql, rusqlite::params_from_iter(p), |r| r.get(0))?)
}

/// 库里出现过的拍摄年份及张数（顶部「年份」下拉用），按年份倒序。
pub fn list_years(conn: &Connection) -> Result<Vec<(i64, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT CAST(strftime('%Y', COALESCE(taken_at, added_ts), 'unixepoch', 'localtime') AS INTEGER) AS y,
                COUNT(*) AS n
           FROM images
          WHERE COALESCE(taken_at, added_ts) IS NOT NULL
          GROUP BY y
          ORDER BY y DESC",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Folder, Image, Library, LibraryKind, Volume};
    use crate::{open_test_db, repo};

    const TAKEN: [Option<i64>; 4] = [Some(100), Some(200), Some(300), None];
    const SIZES: [i64; 4] = [1000, 2000, 3000, 4000];

    /// 造 4 张图：d1 下 a1/a2/a3，d2 下 b1；带格式/大小/时间/评分/RAW，i1 有标签+GPS。
    /// 地点计数必须**跟着图库/目录切换**，和标签一个口径。
    ///
    /// 2026-10-28 用户报：「切图库时标签会变，地理标签不变」——
    /// `manager_places` 当时压根没有 scope 参数，永远统计全库。
    #[test]
    fn 地点计数按图库与目录限定范围() {
        use crate::models::{Library, LibraryKind};
        let (c, d1, d2) = setup();

        // setup 里 i1/i2 在 d1、i4 在 d2（按创建顺序递增）
        repo::places::upsert(&c, 1, Some("北京"), Some(39.9), Some(116.4)).unwrap();
        repo::places::upsert(&c, 2, Some("北京"), Some(39.9), Some(116.4)).unwrap();
        repo::places::upsert(&c, 4, Some("上海"), Some(31.2), Some(121.4)).unwrap();

        // 全库：北京 2、上海 1，按张数降序
        assert_eq!(
            repo::places::list_counts_scoped(&c, None, None).unwrap(),
            vec![("北京".to_string(), 2), ("上海".to_string(), 1)]
        );

        // 目录 scope：d1 里只有北京
        assert_eq!(
            repo::places::list_counts_scoped(&c, None, Some(d1)).unwrap(),
            vec![("北京".to_string(), 2)]
        );

        // 图库 scope：只把 d2 挂进去 → 只剩上海
        let lib = repo::libraries::insert(&c, &Library::new("库A", LibraryKind::Normal)).unwrap();
        repo::libraries::add_folder(&c, lib, d2).unwrap();
        assert_eq!(
            repo::places::list_counts_scoped(&c, Some(lib), None).unwrap(),
            vec![("上海".to_string(), 1)]
        );

        // 老的全库接口行为不变
        assert_eq!(repo::places::list_counts(&c).unwrap().len(), 2);
    }

    /// 「单人」= 恰好 1 张脸，「合照」= ≥2 张脸（用户 2026-11-02 的口径）。
    ///
    /// 这两个条件必须真的下推到 SQL：一张合影可能挂着 4 张脸，
    /// 用 JOIN 会让同一张图出 4 行，所以用相关子查询。
    #[test]
    fn 按脸数筛图() {
        let (c, _d1, _d2) = setup();
        let ids: Vec<i64> = {
            let mut s = c.prepare("SELECT id FROM images ORDER BY id").unwrap();
            let it = s.query_map([], |r| r.get::<_, i64>(0)).unwrap();
            it.map(|x| x.unwrap()).collect()
        };
        // 第 1 张 1 个脸、第 2 张 3 个脸、其余 0 个
        for (img, n) in [(ids[0], 1), (ids[1], 3)] {
            for _ in 0..n {
                c.execute(
                    "INSERT INTO faces(image_id, x, y, w, h, model) VALUES (?1, 0.1, 0.1, 0.1, 0.1, 'm')",
                    rusqlite::params![img],
                )
                .unwrap();
            }
        }
        let run = |flt: ImageFilter| {
            let q = ImageQuery {
                filter: flt,
                ..Default::default()
            };
            list(&c, &q).unwrap().len()
        };
        assert_eq!(run(ImageFilter::default()), 4, "不筛时 4 张");
        assert_eq!(
            run(ImageFilter {
                face_count_eq: Some(1),
                ..Default::default()
            }),
            1,
            "单人"
        );
        assert_eq!(
            run(ImageFilter {
                face_count_min: Some(2),
                ..Default::default()
            }),
            1,
            "合照"
        );
        assert_eq!(
            run(ImageFilter {
                has_faces: Some(true),
                ..Default::default()
            }),
            2,
            "人像 = 有脸的图"
        );
    }

    fn setup() -> (rusqlite::Connection, i64, i64) {
        let c = open_test_db().unwrap();
        let v = repo::volumes::insert(&c, &Volume::new("v")).unwrap();
        let d1 = repo::folders::insert(&c, &Folder::new(v, "a")).unwrap();
        let d2 = repo::folders::insert(&c, &Folder::new(v, "b")).unwrap();

        let mk = |dir: i64, name: &str, fmt: &str, size: i64, taken: Option<i64>, rating: i64, raw: bool| {
            let mut img = Image::new(dir, name);
            img.format = Some(fmt.into());
            img.size = Some(size);
            img.taken_at = taken;
            img.rating = rating;
            img.is_raw = raw;
            repo::images::insert(&c, &img).unwrap()
        };
        let i1 = mk(d1, "a1.jpg", "jpeg", SIZES[0], TAKEN[0], 5, false);
        mk(d1, "a2.png", "png", SIZES[1], TAKEN[1], 3, false);
        let i3 = mk(d1, "a3.cr3", "cr3", SIZES[2], TAKEN[2], 4, true);
        mk(d2, "b1.jpg", "jpeg", SIZES[3], TAKEN[3], 1, false);

        let t1 = repo::tags::ensure(&c, "风景", None).unwrap();
        let t2 = repo::tags::ensure(&c, "星空", None).unwrap();
        repo::tags::add_image_tag(&c, i1, t1, "ai", None).unwrap();
        repo::tags::add_image_tag(&c, i1, t2, "ai", None).unwrap();
        repo::tags::add_image_tag(&c, i3, t2, "ai", None).unwrap();

        c.execute(
            "INSERT INTO image_exif(image_id, gps_lat, gps_lon) VALUES (?1, 39.9, 116.4)",
            rusqlite::params![i1],
        )
        .unwrap();

        (c, d1, d2)
    }

    fn names(imgs: &[Image]) -> Vec<String> {
        imgs.iter().map(|i| i.rel_name.clone().unwrap()).collect()
    }

    #[test]
    fn 空过滤返回全部并按大小升序() {
        let (c, ..) = setup();
        assert_eq!(count(&c, &ImageFilter::default()).unwrap(), 4);
        let q = ImageQuery {
            sort: SortKey::Size,
            dir: SortDir::Asc,
            ..Default::default()
        };
        assert_eq!(names(&list(&c, &q).unwrap()), ["a1.jpg", "a2.png", "a3.cr3", "b1.jpg"]);
    }

    #[test]
    fn 按评分格式RAW大小过滤() {
        let (c, ..) = setup();

        let mut flt = ImageFilter { min_rating: Some(4), ..Default::default() };
        assert_eq!(count(&c, &flt).unwrap(), 2); // i1(5), i3(4)

        flt = ImageFilter { formats: vec!["jpeg".into()], ..Default::default() };
        assert_eq!(count(&c, &flt).unwrap(), 2); // i1, i4

        flt = ImageFilter { is_raw: Some(true), ..Default::default() };
        assert_eq!(count(&c, &flt).unwrap(), 1); // i3

        flt = ImageFilter { min_size: Some(2000), ..Default::default() };
        assert_eq!(count(&c, &flt).unwrap(), 3); // i2,i3,i4
    }

    #[test]
    fn 按拍摄时间区间与文件夹过滤() {
        let (c, d1, _d2) = setup();
        let flt = ImageFilter { taken_from: Some(150), ..Default::default() };
        assert_eq!(count(&c, &flt).unwrap(), 2); // i2(200), i3(300)；None 被排除

        let flt = ImageFilter { taken_to: Some(200), ..Default::default() };
        assert_eq!(count(&c, &flt).unwrap(), 2); // i1(100), i2(200)

        let flt = ImageFilter { dir_ids: vec![d1], ..Default::default() };
        assert_eq!(count(&c, &flt).unwrap(), 3);
    }

    #[test]
    fn 标签全含与任一() {
        let (c, ..) = setup();
        // 全含：风景 + 星空 → 只有 i1
        let flt = ImageFilter {
            tags_all: vec!["风景".into(), "星空".into()],
            ..Default::default()
        };
        assert_eq!(names(&list(&c, &ImageQuery { filter: flt, ..Default::default() }).unwrap()), ["a1.jpg"]);

        // 任一：星空 → i1, i3
        let flt = ImageFilter { tags_any: vec!["星空".into()], ..Default::default() };
        assert_eq!(count(&c, &flt).unwrap(), 2);
    }

    #[test]
    fn GPS过滤() {
        let (c, ..) = setup();
        let flt = ImageFilter { has_gps: Some(true), ..Default::default() };
        assert_eq!(names(&list(&c, &ImageQuery { filter: flt, ..Default::default() }).unwrap()), ["a1.jpg"]);

        let flt = ImageFilter { has_gps: Some(false), ..Default::default() };
        assert_eq!(count(&c, &flt).unwrap(), 3);
    }

    /// 地点 + 多词搜索（2026-10-19）：`image_places` 是侧栏「地点」分区的数据源，
    /// 搜索同时命中文件名 / 标签名 / 地点名，多个词之间是 AND。
    #[test]
    fn 地点过滤与多词搜索() {
        let (c, ..) = setup();
        // i1 有 GPS（setup 里写进 image_exif 的），造两条地点；i3 也挂一条
        let i3 = repo::images::get(&c, 3).unwrap().unwrap();
        repo::places::upsert(&c, 1, Some("北京"), Some(39.9), Some(116.4)).unwrap();
        repo::places::upsert(&c, i3.id, Some("上海"), Some(31.2), Some(121.4)).unwrap();

        // 地点过滤：精确匹配城市名
        let flt = ImageFilter {
            places: vec!["北京".into()],
            ..Default::default()
        };
        assert_eq!(
            names(&list(&c, &ImageQuery { filter: flt, ..Default::default() }).unwrap()),
            ["a1.jpg"]
        );
        let flt = ImageFilter {
            places: vec!["上海".into(), "北京".into()],
            ..Default::default()
        };
        assert_eq!(count(&c, &flt).unwrap(), 2);

        // 侧栏计数（张数降序；同为 1 张时按城市名的 UTF-8 序 —— 上海 U+4E0A 在 北京 之前）
        assert_eq!(
            repo::places::list_counts(&c).unwrap(),
            vec![("上海".to_string(), 1), ("北京".to_string(), 1)]
        );

        // 搜索命中地点：a1 的文件名里没有「北京」，只能靠 `image_places`
        let flt = ImageFilter {
            name_like: Some("北京".into()),
            ..Default::default()
        };
        assert_eq!(
            names(&list(&c, &ImageQuery { filter: flt, ..Default::default() }).unwrap()),
            ["a1.jpg"]
        );

        // 多词 AND：a1 同时带标签「风景」和地点「北京」
        let flt = ImageFilter {
            name_like: Some("风景 北京".into()),
            ..Default::default()
        };
        assert_eq!(count(&c, &flt).unwrap(), 1);
        // 换一个对不上的词 → 0（不能退化成 OR）
        let flt = ImageFilter {
            name_like: Some("风景 上海".into()),
            ..Default::default()
        };
        assert_eq!(count(&c, &flt).unwrap(), 0);
    }

    /// ⛔ 用户输入里的 `%` / `_` 是字面量，不当通配符（`a_b` 不许匹配到 `axb`）。
    #[test]
    fn 搜索里的通配符被转义() {
        let (c, ..) = setup();
        // `_` 当字面量：不转义的话 `a_` 会命中 a1.jpg / a2.png / a3.cr3
        let flt = ImageFilter {
            name_like: Some("a_".into()),
            ..Default::default()
        };
        assert_eq!(count(&c, &flt).unwrap(), 0);
        // `%` 同理（不转义会命中全部 4 张）
        let flt = ImageFilter {
            name_like: Some("a%".into()),
            ..Default::default()
        };
        assert_eq!(count(&c, &flt).unwrap(), 0);
        // ⚠️ 转义不能把正常检索也搞坏
        let flt = ImageFilter {
            name_like: Some("a1".into()),
            ..Default::default()
        };
        assert_eq!(count(&c, &flt).unwrap(), 1);
    }

    /// ⭐ 年份筛选：按**本地时间**的年份比，且没有 EXIF 时间的图按入库时间算。
    ///
    /// 这条容易写错的地方：`taken_at` 是 epoch 秒，直接比大小是「时间点」不是「年份」；
    /// 用 `strftime(...,'localtime')` 才跟用户看到的年份一致（东八区跨年那几小时会差一年）。
    #[test]
    fn 年份筛选按本地年且回退入库时间() {
        let c = open_test_db().unwrap();
        let v = repo::volumes::insert(&c, &Volume::new("v")).unwrap();
        let d = repo::folders::insert(&c, &Folder::new(v, "y")).unwrap();
        let epoch = |s: &str| -> i64 {
            c.query_row("SELECT CAST(strftime('%s', ?1) AS INTEGER)", [s], |r| r.get(0))
                .unwrap()
        };

        let mk = |name: &str, taken: Option<i64>, added: Option<i64>| {
            let mut img = Image::new(d, name);
            img.taken_at = taken;
            let id = repo::images::insert(&c, &img).unwrap();
            // `Image` 里没有 added_ts 字段（入库时才写），这里直接补
            if let Some(ts) = added {
                c.execute("UPDATE images SET added_ts = ?1 WHERE id = ?2", rusqlite::params![ts, id])
                    .unwrap();
            }
            id
        };
        mk("y2021.jpg", Some(epoch("2021-07-29 12:00:00")), None);
        mk("y2023.jpg", Some(epoch("2023-01-05 12:00:00")), None);
        // 没有拍摄时间：靠入库时间归到 2022
        mk("noexif.jpg", None, Some(epoch("2022-06-01 12:00:00")));

        let count_year = |years: Vec<i64>| {
            count(
                &c,
                &ImageFilter {
                    years,
                    ..Default::default()
                },
            )
            .unwrap()
        };
        assert_eq!(count_year(vec![2021]), 1, "2021");
        assert_eq!(count_year(vec![2023]), 1, "2023");
        assert_eq!(count_year(vec![2022]), 1, "没有 EXIF 的图应按入库时间进 2022");
        assert_eq!(count_year(vec![2021, 2023]), 2, "多选任一命中");
        assert_eq!(count_year(vec![]), 3, "空 = 不限");
        assert_eq!(count_year(vec![1999]), 0);
    }

    /// 搜索也要能命中**机型 / 镜头**（`image_exif`）——「全文」不只是文件名。
    #[test]
    fn 搜索命中机型与镜头() {
        let (c, ..) = setup();
        c.execute(
            // ⚠️ setup() 只给 image_id = 1 插了 image_exif 行，这里要更新**这一行**
            "UPDATE image_exif SET make = 'SONY', model = 'ILCE-7M3', lens = 'FE 35mm F1.8'
              WHERE image_id = 1",
            [],
        )
        .unwrap();

        let hit = |kw: &str| {
            count(
                &c,
                &ImageFilter {
                    name_like: Some(kw.into()),
                    ..Default::default()
                },
            )
            .unwrap()
        };
        assert_eq!(hit("SONY"), 1, "品牌");
        assert_eq!(hit("ILCE"), 1, "机型");
        assert_eq!(hit("35mm"), 1, "镜头");
        // ⚠️ 没有 EXIF 的图不该因为 `COALESCE` 把整表命中
        assert_eq!(hit("不存在的关键词"), 0);
    }

    /// 分辨率排序（2026-10-31 加，列表视图的「分辨率」列要用）。
    ///
    /// ⚠️ 用**像素总数**而不是宽/高单列：竖图 4000×3000 和横图 3000×4000 是
    /// 同一档，单比宽会把它们分成两类，看起来像排序坏了。
    #[test]
    fn 按分辨率排序() {
        let (c, ..) = setup();
        c.execute("UPDATE images SET width = 100, height = 100 WHERE rel_name = 'a1.jpg'", [])
            .unwrap();
        c.execute("UPDATE images SET width = 500, height = 500 WHERE rel_name = 'a2.png'", [])
            .unwrap();
        c.execute("UPDATE images SET width = 50, height = 50 WHERE rel_name = 'a3.cr3'", [])
            .unwrap();
        // b1.jpg 故意留着 width/height = NULL —— 必须当成 0 排在最后，不能崩

        let q = ImageQuery {
            sort: SortKey::Dims,
            dir: SortDir::Desc,
            ..Default::default()
        };
        assert_eq!(
            names(&list(&c, &q).unwrap()),
            ["a2.png", "a1.jpg", "a3.cr3", "b1.jpg"]
        );
    }

    /// 旗标排序：升/降序都要能用（列表的「标记」列）。
    #[test]
    fn 按旗标排序() {
        let (c, ..) = setup();
        c.execute("UPDATE images SET flag = 1 WHERE rel_name = 'a3.cr3'", [])
            .unwrap();

        let q = ImageQuery {
            sort: SortKey::Flag,
            dir: SortDir::Desc,
            ..Default::default()
        };
        let got = names(&list(&c, &q).unwrap());
        assert_eq!(got[0], "a3.cr3", "打了旗标的排最前");
    }

    #[test]
    fn 分页与图库过滤() {
        let (c, d1, _d2) = setup();
        // 按大小升序，limit 2 offset 1 → [2000, 3000]
        let q = ImageQuery { sort: SortKey::Size, dir: SortDir::Asc, limit: Some(2), offset: 1, ..Default::default() };
        assert_eq!(names(&list(&c, &q).unwrap()), ["a2.png", "a3.cr3"]);

        // 图库只含 d1
        let lid = repo::libraries::insert(&c, &Library::new("L", LibraryKind::Normal)).unwrap();
        repo::libraries::add_folder(&c, lid, d1).unwrap();
        let flt = ImageFilter { library_id: Some(lid), ..Default::default() };
        assert_eq!(count(&c, &flt).unwrap(), 3);
    }

    #[test]
    fn 组合过滤下推到SQL() {
        let (c, ..) = setup();
        // jpeg 且 评分>=3 且 有 GPS → 只有 i1
        let flt = ImageFilter {
            formats: vec!["jpeg".into()],
            min_rating: Some(3),
            has_gps: Some(true),
            ..Default::default()
        };
        assert_eq!(names(&list(&c, &ImageQuery { filter: flt, ..Default::default() }).unwrap()), ["a1.jpg"]);
    }

    /// ⭐ 没有拍摄时间的照片**沉底**，不能拿「入库时间」顶替。
    ///
    /// 顶替的后果：一堆截图/下载图（没有 EXIF）会按入库时间插进真实拍摄时间中间，
    /// 用户选「按拍摄时间」看到的就是乱序 —— 而界面上完全看不出原因。
    #[test]
    fn 按拍摄时间排序时没时间的沉底() {
        let (c, ..) = setup(); // b1 的 taken_at 是 None
        let desc = ImageQuery {
            sort: SortKey::TakenAt,
            dir: SortDir::Desc,
            ..Default::default()
        };
        assert_eq!(
            names(&list(&c, &desc).unwrap()),
            ["a3.cr3", "a2.png", "a1.jpg", "b1.jpg"],
            "降序：300 → 200 → 100 → 无时间"
        );

        let asc = ImageQuery {
            sort: SortKey::TakenAt,
            dir: SortDir::Asc,
            ..Default::default()
        };
        assert_eq!(
            names(&list(&c, &asc).unwrap()),
            ["a1.jpg", "a2.png", "a3.cr3", "b1.jpg"],
            "升序：100 → 200 → 300 → 无时间（仍沉底）"
        );
    }

    /// 文件类型排序：同格式聚在一起，格式名按字母序。
    #[test]
    fn 按文件类型排序() {
        let (c, ..) = setup(); // cr3, jpeg×2, png
        let q = ImageQuery {
            sort: SortKey::Format,
            dir: SortDir::Asc,
            ..Default::default()
        };
        assert_eq!(
            names(&list(&c, &q).unwrap()),
            ["a3.cr3", "a1.jpg", "b1.jpg", "a2.png"]
        );
    }
}
