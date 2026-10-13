//! PurePhotoExplorer 离线地名库 —— 由 **GPS 坐标反查最近的城市**。
//!
//! ## 为什么是离线库而不是调逆地理编码接口
//! 照片位置属于隐私数据。调在线服务意味着「每看一张带 GPS 的照片，
//! 就把用户在哪拍过发给了第三方」；而这里只要一份 1.8 MB 的静态表，
//! 查询在本地完成，**不联网、不上传**。代价是精度只到「最近的城市」，
//! 拿不到街道门牌 —— 对"这张照片在哪拍的"足够，且正好不泄露更细的位置。
//!
//! ## 数据
//! - 来源：GeoNames `cities5000`（人口 > 5000 的城镇，6.4 万条），CC BY 4.0；
//! - 由 `tools/gen_city_db.py` 裁成 18 字节定长记录 + 字符串池，
//!   `include_bytes!` 编进二进制（见该脚本注释）；
//! - 更新数据：重跑脚本覆盖 `data/cities.bin` 即可，本文件无需改动。
//!
//! ## 查询性能
//! 线性扫描 6.4 万条、每条只读 5 个字段算一个近似的平方距离，
//! 实测 **< 1 ms** —— 快到不需要空间索引（R 树/网格都会让代码复杂十倍，
//! 换来用户感知不到的收益）。
//!
//! ## 署名
//! GeoNames 采用 CC BY 4.0，应用内保留署名（帮助面板「关于」区）。

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// 紧凑数据表（1.8 MB），由 `tools/gen_city_db.py` 生成。
const DATA: &[u8] = include_bytes!("../data/cities.bin");

const MAGIC: &[u8; 4] = b"PVGE";
/// 头长度：magic(4) + ver(2) + 国家数(2) + 城市数(4) + 国家表长(4) + 池长(4)
const HEADER_LEN: usize = 20;
/// 单条城市记录：
/// lat f32(4) + lon f32(4) + pop u32(4) + 名偏移 u32(4) + 国家序号 u16(2) + 行政级别 u8(1)
const CITY_REC: usize = 19;

/// 最近城市超过这个距离就当"没有地点"。
///
/// 150 km 是刻意选的：再远就已经不是"这张照片在哪拍的"能回答的事了
/// （海上、沙漠、无人区），硬套一个几百公里外的地名反而误导。
pub const MAX_KM: f64 = 150.0;

/// 「同城」半径：这个范围内的条目视为同一个地方，只挑一条显示。
///
/// ## 为什么不是「取最近的」（2026-10-11 实测踩到）
/// 数据里同一个城市会被拆成很多条目，直接取最近会得到行政区甚至街区：
/// - 上海坐标 → **徐汇**（PPLA3，比上海更近）；
/// - 东京坐标 → **阿佐ヶ谷**（一个街区，且只有日文名）；
/// - 澳门坐标 → **珠海**（两地相距 1.5 km，珠海 170 万人 > 澳门 68 万人）。
///
/// 用户问的是"这张照片在哪座城市拍的"，要的是**上海 / 东京 / 中国澳门**。
///
/// ## 取舍规则（两条，按优先级）
/// 1. 同城内若存在**省/国家级**条目（PPLC / PPLA，见 [`MAJOR_LEVEL`]），
///    取级别最高的（并列比人口）—— 解决澳门被珠海压过、上海被徐汇压过；
/// 2. 否则取同城内**人口最大**的 —— 解决纽约被自己的区（布鲁克林，PPLA3）压过。
///
/// 10 km 内一条都没有，才退化成"取最近的"。这样"郊区的镇 vs 40 km 外的
/// 大城市"仍由距离说话。
const NEAR_KM: f64 = 10.0;

/// 初筛用 1 度 ≈ 111.195 km 换算，留 20% 余量避免边界漏掉精确值在 10 km 内的条目；
/// 最终仍以 haversine 复核。
const KM_PER_DEG: f64 = 111.195;
const NEAR_D2: f64 = (NEAR_KM * 1.2 / KM_PER_DEG) * (NEAR_KM * 1.2 / KM_PER_DEG);

/// 超过这个距离时，把距离一并显示给用户（近处没必要提）。
const SHOW_DISTANCE_KM: f64 = 20.0;

/// 查到的地点。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Place {
    /// 城市名（中国城市为中文；国外城市尽力转简体）
    pub name: String,
    /// 国家/地区中文名（中国香港 / 中国澳门 / 中国台湾 按规范写法）
    pub country: String,
    /// 到该城市的直线距离（公里）
    pub distance_km: f64,
    /// 城市人口（可用于判断"这是不是个像样的城市"）
    pub population: u32,
    /// 面向展示的文本，如 `上海，中国`；离得远时补上距离 `佛山，中国（38 km）`。
    ///
    /// 在构造时就算好（不是方法）：前端直接显示这个字段，
    /// 格式化规则只留在 Rust 这一处。
    pub label: String,
}

/// 展示文案的生成规则（[`Place::label`] 用）。
fn make_label(name: &str, country: &str, distance_km: f64) -> String {
    if distance_km <= SHOW_DISTANCE_KM {
        format!("{name}，{country}")
    } else {
        format!("{name}，{country}（{distance_km:.0} km）")
    }
}

/// 解析一次就缓存下来的表头信息（国家名表 245 条，没必要每次重扫）。
struct Db {
    city_count: usize,
    countries: Vec<&'static str>,
    cities_off: usize,
    pool_off: usize,
}

static DB: OnceLock<Db> = OnceLock::new();

fn db() -> &'static Db {
    DB.get_or_init(parse)
}

fn parse() -> Db {
    // 数据是构建时生成的常量，走到这里说明数据文件损坏（构建期问题，不是运行时输入）
    assert_eq!(&DATA[0..4], MAGIC, "cities.bin 魔数不符，请重跑 tools/gen_city_db.py");

    let country_count = u16::from_le_bytes([DATA[6], DATA[7]]) as usize;
    let city_count = u32::from_le_bytes([DATA[8], DATA[9], DATA[10], DATA[11]]) as usize;
    let country_len = u32::from_le_bytes([DATA[12], DATA[13], DATA[14], DATA[15]]) as usize;

    // 国家表：每条是 u16 长度 + UTF-8
    let mut countries = Vec::with_capacity(country_count);
    let mut p = HEADER_LEN;
    for _ in 0..country_count {
        let n = u16::from_le_bytes([DATA[p], DATA[p + 1]]) as usize;
        p += 2;
        countries.push(std::str::from_utf8(&DATA[p..p + n]).unwrap_or(""));
        p += n;
    }

    let cities_off = HEADER_LEN + country_len;
    Db {
        city_count,
        countries,
        cities_off,
        pool_off: cities_off + city_count * CITY_REC,
    }
}

/// 表里有多少条城市（自检/展示用）。
pub fn city_count() -> usize {
    db().city_count
}

/// 取第 `i` 条记录的字段
/// （`(纬度, 经度, 人口, 名字偏移, 国家序号, 行政级别)`）。
#[inline]
fn record(db: &Db, i: usize) -> (f64, f64, u32, usize, usize, u8) {
    let o = db.cities_off + i * CITY_REC;
    let d = DATA;
    (
        f32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]) as f64,
        f32::from_le_bytes([d[o + 4], d[o + 5], d[o + 6], d[o + 7]]) as f64,
        u32::from_le_bytes([d[o + 8], d[o + 9], d[o + 10], d[o + 11]]),
        u32::from_le_bytes([d[o + 12], d[o + 13], d[o + 14], d[o + 15]]) as usize,
        u16::from_le_bytes([d[o + 16], d[o + 17]]) as usize,
        d[o + 18],
    )
}

/// 从字符串池里取城市名（**只在最终命中那一条上调用一次**，其余 6 万条不碰字符串）。
fn pool_str(db: &Db, off: usize) -> &'static str {
    let p = db.pool_off + off;
    let n = DATA[p] as usize;
    std::str::from_utf8(&DATA[p + 1..p + 1 + n]).unwrap_or("")
}

/// 球面距离（公里）。
fn haversine(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const R_KM: f64 = 6371.0088;
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let a = (dlat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (dlon / 2.0).sin().powi(2);
    2.0 * R_KM * a.sqrt().clamp(-1.0, 1.0).asin()
}

/// 判定「省/国家级」的行政级别门槛（对应生成器里的 `PPLA = 2`、`PPLC = 3`）。
///
/// 只有同城内存在这一档的条目时，级别才压过人口 —— 否则纽约会败给
/// 自己的一个区（New York City 是 `PPL`，布鲁克林是 `PPLA3`），
/// 而那份数据里"谁级别高"完全不代表"谁更该被显示"。
const MAJOR_LEVEL: u8 = 2;

/// 找离 `(lat, lon)` 最近的城市；超过 [`MAX_KM`] 返回 `None`。
///
/// 两段式：先用**等距近似**（经度按 cos(纬度) 缩放）扫描，再对选中的那条
/// 算精确 haversine —— 近似只用来排序与初筛，最终距离一定是准的。
///
/// 同城多条目的取舍见 [`NEAR_KM`]。
pub fn nearest_place(lat: f64, lon: f64) -> Option<Place> {
    let db = db();
    let cos_lat = lat.to_radians().cos();

    let mut any_d2 = f64::INFINITY;
    let mut any_idx = usize::MAX;
    // 同城候选 A：省/国家级（level ≥ MAJOR_LEVEL），按级别 → 人口取大
    let mut major: Option<(u8, u32, usize)> = None;
    // 同城候选 B：所有同城条目里人口最大的（没有 A 时用）
    let mut biggest: Option<(u32, usize)> = None;

    for i in 0..db.city_count {
        let (clat, clon, pop, _, _, level) = record(db, i);
        let dlat = clat - lat;
        // 跨 ±180° 经线时取更短的一侧，否则会把近邻算成"绕地球一圈"
        let mut dlon = clon - lon;
        if dlon > 180.0 {
            dlon -= 360.0;
        } else if dlon < -180.0 {
            dlon += 360.0;
        }
        let x = dlon * cos_lat;
        let d2 = dlat * dlat + x * x;

        if d2 < any_d2 {
            any_d2 = d2;
            any_idx = i;
        }
        if d2 > NEAR_D2 {
            continue;
        }
        if level >= MAJOR_LEVEL
            && major.map_or(true, |(l, p, _)| level > l || (level == l && pop > p))
        {
            major = Some((level, pop, i));
        }
        if biggest.map_or(true, |(p, _)| pop > p) {
            biggest = Some((pop, i));
        }
    }

    // 同城内优先「省/国家级」，其次是「人口最大」，都没有才退化成「最近的」
    let near_idx = major
        .map(|(_, _, i)| i)
        .or_else(|| biggest.map(|(_, i)| i));
    if let Some(idx) = near_idx {
        let (clat, clon, pop, name_off, cc, _) = record(db, idx);
        let km = haversine(lat, lon, clat, clon);
        if km <= NEAR_KM {
            return Some(build(db, name_off, cc, km, pop));
        }
    }

    if any_idx == usize::MAX {
        return None;
    }
    let (clat, clon, pop, name_off, cc, _) = record(db, any_idx);
    let km = haversine(lat, lon, clat, clon);
    if km > MAX_KM {
        return None;
    }
    Some(build(db, name_off, cc, km, pop))
}

fn build(db: &Db, name_off: usize, cc: usize, distance_km: f64, population: u32) -> Place {
    let name = pool_str(db, name_off).to_string();
    let country = db.countries.get(cc).copied().unwrap_or("").to_string();
    let label = make_label(&name, &country, distance_km);
    Place {
        name,
        country,
        distance_km,
        population,
        label,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 数据表本身要自洽（文件被换坏时这条最先报）。
    #[test]
    fn 数据表可解析且规模合理() {
        let db = db();
        assert!(db.city_count > 50_000, "城市数 {} 偏少", db.city_count);
        assert!(db.countries.len() > 200, "国家/地区 {} 偏少", db.countries.len());
        // 池偏移必须落在数据内，否则后面切片会 panic
        assert!(db.pool_off < DATA.len(), "池偏移越界");
    }

    /// 上海：样本 IMG_7507.DNG 的实拍坐标，必须落在上海。
    ///
    /// ⚠️ 同时钉住「同城取人口最大」这条规则：数据里徐汇区离该坐标更近，
    /// 若退回成"取最近的"，这里会变成 `徐汇`（用户要的是"在哪座城市拍的"）。
    #[test]
    fn 上海坐标查到上海而非更近的区() {
        let p = nearest_place(31.204839, 121.440628).expect("上海市区应当有城市");
        assert_eq!(p.name, "上海", "实际 {}", p.name);
        assert_eq!(p.country, "中国");
        assert!(p.distance_km < 30.0, "距离 {} km 偏大", p.distance_km);
        assert!(
            p.population > 5_000_000,
            "命中的应是上海市（人口 {:?}），不是区级条目",
            p.population
        );
    }

    /// 国外城市要转成简体（GeoNames 原始数据里是 `紐約` / `東京`）。
    #[test]
    fn 国外城市名转简体() {
        let ny = nearest_place(40.7128, -74.0060).expect("纽约（曼哈顿）应有城市");
        assert_eq!(ny.name, "纽约", "实际 {}", ny.name);
        assert_eq!(ny.country, "美国");

        let tk = nearest_place(35.6762, 139.6503).expect("东京应有城市");
        assert_eq!(tk.name, "东京", "实际 {}", tk.name);
        assert_eq!(tk.country, "日本");
    }

    /// 荒野 / 海上：宁可没有地点，也不要硬套一个几百公里外的城市。
    #[test]
    fn 远离人烟时不返回地点() {
        // 南太平洋中部
        assert!(nearest_place(-30.0, -140.0).is_none());
        // 撒哈拉腹地
        assert!(nearest_place(23.0, 15.0).is_none());
    }

    /// 靠近阈值边界时距离要如实给出（前端靠它决定要不要显示"约 X km"）。
    #[test]
    fn 距离与标签格式() {
        let p = nearest_place(31.204839, 121.440628).unwrap();
        assert!(p.label.starts_with("上海"), "{}", p.label);
        assert!(p.label.contains("中国"), "{}", p.label);

        // 手工构造：远处应带距离后缀
        let far = Place {
            name: "佛山".into(),
            country: "中国".into(),
            distance_km: 38.4,
            population: 1000,
            label: make_label("佛山", "中国", 38.4),
        };
        assert_eq!(far.label, "佛山，中国（38 km）");
        // 近处不提距离（几百米内提"0 km"是噪音）
        assert_eq!(make_label("上海", "中国", 3.2), "上海，中国");
    }

    /// 跨 ±180° 经线要取短边（斐济一带），不能绕地球一圈。
    #[test]
    fn 跨日期变更线不绕地球() {
        // 179.9°E 与 -179.9°E 只差 0.2°
        let p = nearest_place(-17.6, 179.9);
        if let Some(place) = p {
            assert!(place.distance_km < 150.0);
        }
        // 真正的检验：同一个点从两个方向查，结果应当一致
        let a = nearest_place(-17.6, 179.95);
        let b = nearest_place(-17.6, -179.95);
        assert_eq!(
            a.map(|p| p.name),
            b.map(|p| p.name),
            "±180° 两侧应查到同一个最近城市"
        );
    }

    /// 中国港澳台按规范写法（不能出现"香港国"这类表述）。
    #[test]
    fn 港澳台用规范称谓() {
        for (lat, lon, expect) in [
            (22.3193, 114.1694, "中国香港"),
            (22.1987, 113.5439, "中国澳门"),
            (25.0330, 121.5654, "中国台湾"),
        ] {
            let p = nearest_place(lat, lon).expect("应有城市");
            assert_eq!(p.country, expect, "{} 落到了 {}", expect, p.country);
        }
    }

    /// 查询要够快 —— 每次打开带 GPS 的照片都会调一次，不能卡住看图。
    #[test]
    fn 查询耗时足够短() {
        let t = std::time::Instant::now();
        for _ in 0..50 {
            let _ = nearest_place(31.204839, 121.440628);
        }
        let per_ms = t.elapsed().as_secs_f64() * 1000.0 / 50.0;
        eprintln!("单次查询 {per_ms:.2} ms");
        assert!(per_ms < 10.0, "单次查询 {per_ms:.2} ms 太慢");
    }
}
