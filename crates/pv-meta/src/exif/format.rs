//! EXIF 数值 → 展示字符串的**纯函数**（无 IO、无外部状态，全部可单测）。
//!
//! **为什么不用 kamadak-exif 的 `display_value()`**：
//! 1. 它输出英文短语（`multi-spot`、`aperture priority`、`auto white balance`），
//!    而本应用界面是中文；
//! 2. 它的数值格式不符合摄影习惯 —— 快门输出裸 `1/250`（无单位）、
//!    曝光补偿输出 `-1/3`（分数）而不是 `-0.3 EV`。
//!
//! 因此这里统一按中文摄影习惯输出：`1/250 s`、`f/2.8`、`50 mm`、`-0.3 EV`。

/// 去掉小数末尾多余的 0（`2.0` → `2`，`0.30` → `0.3`）。
fn trim_fixed(v: f64, digits: usize) -> String {
    let s = format!("{:.*}", digits, v);
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

/// 接近整数判定（用于区分 `4/3` 这类非整倍率）。
fn is_near_int(v: f64) -> bool {
    (v - v.round()).abs() < 0.005
}

/// 快门速度：曝光时间 = `num / denom` 秒。
///
/// - `1/250` → `1/250 s`
/// - `30/1`  → `30 s`
/// - `13/10` → `1.3 s`
/// - `1/3`   → `1/3 s`（分母不整除时退化为小数秒）
pub fn exposure_time(num: u32, denom: u32) -> Option<String> {
    if num == 0 || denom == 0 {
        return None;
    }
    let secs = num as f64 / denom as f64;
    if !(0.0..=3600.0).contains(&secs) {
        return None;
    }

    // 长曝光（≥ 1 秒）：直接写秒数
    if secs >= 1.0 {
        return Some(format!("{} s", trim_fixed(secs, 1)));
    }

    // 短曝光：优先写分数形式（摄影惯例），不能整除时退化写小数
    let reciprocal = denom as f64 / num as f64;
    if is_near_int(reciprocal) {
        Some(format!("1/{} s", reciprocal.round() as u64))
    } else {
        Some(format!("{} s", trim_fixed(secs, 3)))
    }
}

/// 光圈值：`num / denom` → `f/2.8`（整数光圈显示为 `f/8`）。
pub fn f_number(num: u32, denom: u32) -> Option<String> {
    if num == 0 || denom == 0 {
        return None;
    }
    let f = num as f64 / denom as f64;
    if !(0.5..=512.0).contains(&f) {
        return None;
    }
    Some(format!("f/{}", trim_fixed(f, 1)))
}

/// 焦距：`num / denom` mm → `50 mm`。
pub fn focal_length(num: u32, denom: u32) -> Option<String> {
    if num == 0 || denom == 0 {
        return None;
    }
    let mm = num as f64 / denom as f64;
    if !(0.0..=10_000.0).contains(&mm) {
        return None;
    }
    Some(format!("{} mm", trim_fixed(mm, 1)))
}

/// 曝光补偿（有符号）：`num / denom` EV → `-0.3 EV` / `+1 EV` / `0 EV`。
pub fn exposure_bias(num: i32, denom: i32) -> Option<String> {
    if denom == 0 {
        return None;
    }
    let ev = num as f64 / denom as f64;
    if ev.abs() < 0.005 {
        return Some("0 EV".into());
    }
    let sign = if ev > 0.0 { "+" } else { "-" };
    Some(format!("{sign}{} EV", trim_fixed(ev.abs(), 2)))
}

/// EXIF 时间戳 `2024:05:01 18:32:10` → `2024-05-01 18:32:10`。
///
/// 全 0（部分相机未写入）或格式不合法时返回 `None`。
pub fn date_time(raw: &str) -> Option<String> {
    let s = raw.trim();
    let b = s.as_bytes();
    if b.len() < 19 {
        return None;
    }
    // 必须是 ASCII 且分隔符位置固定（EXIF 规范固定为 "YYYY:MM:DD HH:MM:SS"）
    let digits_at = |ranges: &[(usize, usize)]| {
        ranges.iter().all(|&(a, z)| {
            b[a..z].iter().all(|c| c.is_ascii_digit())
        })
    };
    if b[4] != b':' || b[7] != b':' || b[10] != b' ' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    if !digits_at(&[(0, 4), (5, 7), (8, 10), (11, 13), (14, 16), (17, 19)]) {
        return None;
    }
    if b[0..4] == *b"0000" {
        return None;
    }
    Some(format!(
        "{}-{}-{} {}",
        &s[0..4],
        &s[5..7],
        &s[8..10],
        &s[11..19]
    ))
}

/// 曝光程序（Exif 0x8822）。
pub fn exposure_program(v: u32) -> Option<&'static str> {
    Some(match v {
        0 => "未定义",
        1 => "手动",
        2 => "程序自动",
        3 => "光圈优先",
        4 => "快门优先",
        5 => "创意程序",
        6 => "运动程序",
        7 => "人像模式",
        8 => "风景模式",
        _ => return None,
    })
}

/// 测光模式（Exif 0x9207）。
pub fn metering_mode(v: u32) -> Option<&'static str> {
    Some(match v {
        0 => "未知",
        1 => "平均测光",
        2 => "中央重点平均",
        3 => "点测光",
        4 => "多点测光",
        5 => "评价测光",
        6 => "局部测光",
        255 => "其他",
        _ => return None,
    })
}

/// 白平衡（Exif 0xa403）。
pub fn white_balance(v: u32) -> Option<&'static str> {
    Some(match v {
        0 => "自动",
        1 => "手动",
        _ => return None,
    })
}

/// 图像方向（Exif 0x0112）。**注意：当前看图器尚未按此自动旋正**，仅作信息展示。
pub fn orientation(v: u32) -> Option<&'static str> {
    Some(match v {
        1 => "正常",
        2 => "水平镜像",
        3 => "旋转 180°",
        4 => "垂直镜像",
        5 => "水平镜像 + 顺时针 90°",
        6 => "顺时针 90°",
        7 => "顺时针 90° + 垂直镜像",
        8 => "逆时针 90°",
        _ => return None,
    })
}

/// 闪光灯（Exif 0x9209）—— 按位组合出人类可读描述。
pub fn flash(v: u32) -> String {
    let mut parts: Vec<&str> = Vec::new();
    parts.push(if v & 1 == 1 { "已闪光" } else { "未闪光" });

    // bit 3-4：闪光灯模式
    match (v >> 3) & 3 {
        1 => parts.push("强制"),
        2 => parts.push("强制关闭"),
        3 => parts.push("自动"),
        _ => {}
    }
    // bit 6：防红眼
    if (v >> 6) & 1 == 1 {
        parts.push("防红眼");
    }
    // bit 5：1 表示机身上没有闪光灯
    if (v >> 5) & 1 == 1 {
        parts.push("无闪光灯");
    }
    parts.join("，")
}

/// GPS 度分秒 → 十进制度。
///
/// EXIF 里纬经度写成 **3 个 RATIONAL**（度 / 分 / 秒），如 `31°12'17.42"`，
/// 这里换成地图通用的十进制度 `31.204839`。
///
/// - `dms` 至少要 1 个元素（度）；分、秒缺失时按 0 处理；
/// - 分母为 0 的项视为缺失（损坏数据不该让整条坐标失效）；
/// - 度必须 ≥ 0（`GPSLatitudeRef` / `GPSLongitudeRef` 才负责符号）。
pub fn gps_decimal(dms: &[(u32, u32)]) -> Option<f64> {
    let take = |i: usize| -> Option<f64> {
        let (n, d) = *dms.get(i)?;
        if d == 0 {
            return None;
        }
        Some(n as f64 / d as f64)
    };
    let deg = take(0)?;
    let min = take(1).unwrap_or(0.0);
    let sec = take(2).unwrap_or(0.0);
    let dec = deg + min / 60.0 + sec / 3600.0;
    if !(-180.0..=180.0).contains(&dec) {
        return None;
    }
    Some(dec)
}

/// 十进制度 → 度分秒展示文本，如 `31°12'17.4"N`、`121°26'26.3"E`。
///
/// 摄影与地图行业习惯用度分秒（DMS）而不是十进制小数，用户 2026-10-11 选定这种。
///
/// ⚠️ **秒要处理进位**：秒四舍五入到 0.1 后可能变成 `60.0`（如 59.96"），
/// 此时必须进到分；分到 60 再进到度。漏了这一步会出现
/// `31°12'60.0"N` 这种非法读数。
pub fn gps_coord(mut dec: f64, neg_ref: &str, pos_ref: &str) -> String {
    let hemi = if dec < 0.0 { neg_ref } else { pos_ref };
    dec = dec.abs();

    let mut deg = dec.floor();
    let min_total = (dec - deg) * 60.0;
    let mut min = min_total.floor();
    // 先四舍五入到 0.1 秒，再逐级进位
    let mut sec = ((min_total - min) * 60.0 * 10.0).round() / 10.0;

    if sec >= 60.0 {
        sec -= 60.0;
        min += 1.0;
    }
    if min >= 60.0 {
        min -= 60.0;
        deg += 1.0;
    }

    format!("{}°{}'{:.1}\"{}", deg as u64, min as u64, sec, hemi)
}

/// GPS 海拔：RATIONAL 米数 + 参考（0=海平面以上，1=以下）→ `9.3 m` / `-9.3 m`。
///
/// 分母为 0 或数值离谱（> 100 km，超过民航巡航高度）时视为无效 ——
/// 少数机型会把 0 或垃圾值写进这个字段。
pub fn gps_altitude(num: u32, denom: u32, below_sea: bool) -> Option<String> {
    if denom == 0 {
        return None;
    }
    let m = num as f64 / denom as f64;
    if !(0.0..=100_000.0).contains(&m) {
        return None;
    }
    let signed = if below_sea { -m } else { m };
    Some(format!("{} m", trim_fixed(signed, 1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 快门速度按摄影惯例格式化() {
        assert_eq!(exposure_time(1, 250).as_deref(), Some("1/250 s"));
        assert_eq!(exposure_time(1, 8000).as_deref(), Some("1/8000 s"));
        assert_eq!(exposure_time(30, 1).as_deref(), Some("30 s"));
        assert_eq!(exposure_time(13, 10).as_deref(), Some("1.3 s"));
        assert_eq!(exposure_time(1, 3).as_deref(), Some("1/3 s"));
        // 10/2500 = 1/250（非最简分数也要还原成 1/250）
        assert_eq!(exposure_time(10, 2500).as_deref(), Some("1/250 s"));
        assert_eq!(exposure_time(0, 250), None, "分子为 0 视为无效");
        assert_eq!(exposure_time(1, 0), None, "分母为 0 视为无效");
    }

    #[test]
    fn 光圈格式化() {
        assert_eq!(f_number(28, 10).as_deref(), Some("f/2.8"));
        assert_eq!(f_number(8, 1).as_deref(), Some("f/8"));
        assert_eq!(f_number(18, 10).as_deref(), Some("f/1.8"));
        assert_eq!(f_number(0, 1), None);
    }

    #[test]
    fn 焦距格式化() {
        assert_eq!(focal_length(50, 1).as_deref(), Some("50 mm"));
        assert_eq!(focal_length(105, 1).as_deref(), Some("105 mm"));
        assert_eq!(focal_length(9, 2).as_deref(), Some("4.5 mm"));
        assert_eq!(focal_length(0, 1), None);
    }

    #[test]
    fn 曝光补偿带符号() {
        assert_eq!(exposure_bias(-1, 3).as_deref(), Some("-0.33 EV"));
        assert_eq!(exposure_bias(-3, 10).as_deref(), Some("-0.3 EV"));
        assert_eq!(exposure_bias(1, 1).as_deref(), Some("+1 EV"));
        assert_eq!(exposure_bias(0, 1).as_deref(), Some("0 EV"));
        assert_eq!(exposure_bias(1, 0), None);
    }

    #[test]
    fn 拍摄时间转成常见格式() {
        assert_eq!(
            date_time("2024:05:01 18:32:10").as_deref(),
            Some("2024-05-01 18:32:10")
        );
        assert_eq!(date_time("    :  :     :  :  "), None, "空值应被丢弃");
        assert_eq!(date_time("0000:00:00 00:00:00"), None, "全 0 应被丢弃");
        assert_eq!(date_time("2024-05-01"), None, "长度不足应被丢弃");
    }

    #[test]
    fn 枚举标签映射为中文() {
        assert_eq!(exposure_program(3), Some("光圈优先"));
        assert_eq!(metering_mode(5), Some("评价测光"));
        assert_eq!(white_balance(0), Some("自动"));
        assert_eq!(orientation(6), Some("顺时针 90°"));
        assert_eq!(metering_mode(12345), None, "未知取值不应瞎编");
    }

    #[test]
    fn 闪光灯按位解析() {
        assert_eq!(flash(0), "未闪光");
        assert_eq!(flash(1), "已闪光");
        // 0x19 = 已闪光 + 自动
        assert_eq!(flash(0x19), "已闪光，自动");
        // 0x59 = 已闪光 + 自动 + 防红眼
        assert_eq!(flash(0x59), "已闪光，自动，防红眼");
        // 0x20 = 未闪光 + 机身上无闪光灯
        assert_eq!(flash(0x20), "未闪光，无闪光灯");
    }

    #[test]
    fn GPS度分秒转十进制度() {
        // 样本 IMG_7507.DNG 实测值：31°12'17.42" → 31.204839
        let d = gps_decimal(&[(31, 1), (12, 1), (1742, 100)]).unwrap();
        assert!((d - 31.204839).abs() < 1e-5, "实际 {d}");

        // 分、秒缺失时按 0 处理（只有度也算一份可用的粗坐标）
        assert_eq!(gps_decimal(&[(31, 1)]), Some(31.0));
        assert_eq!(gps_decimal(&[(31, 1), (30, 1)]), Some(31.5));

        // 损坏数据：分母为 0、或度本身超出 ±180 都判无效
        assert_eq!(gps_decimal(&[(31, 0)]), None, "分母为 0");
        assert_eq!(gps_decimal(&[]), None, "空列表");
        assert_eq!(gps_decimal(&[(200, 1)]), None, "纬度不可能 200°");
    }

    #[test]
    fn GPS坐标转度分秒带方位() {
        // 样本 IMG_7507.DNG 实测值
        assert_eq!(gps_coord(31.204839, "S", "N"), "31°12'17.4\"N");
        assert_eq!(gps_coord(121.440628, "W", "E"), "121°26'26.3\"E");
        // 南半球 / 西经：取绝对值 + 换方位字母
        assert_eq!(gps_coord(-33.8688, "S", "N"), "33°52'7.7\"S");
        assert_eq!(gps_coord(-70.6693, "W", "E"), "70°40'9.5\"W");
        // 0 不带负号（南纬/西经 0° 就是赤道 / 本初子午线）
        assert_eq!(gps_coord(0.0, "S", "N"), "0°0'0.0\"N");
    }

    /// 秒的四舍五入必须能进位 —— 否则会印出 `31°12'60.0"` 这种非法读数。
    #[test]
    fn GPS度分秒进位正确() {
        // 59.96" → 60.0" → 应进到分
        assert_eq!(gps_coord(31.0 + 12.0 / 60.0 + 59.96 / 3600.0, "S", "N"), "31°13'0.0\"N");
        // 分秒同时顶格：59'59.96" → 度进位
        assert_eq!(gps_coord(31.0 + 59.0 / 60.0 + 59.96 / 3600.0, "S", "N"), "32°0'0.0\"N");
        // 未达进位的临界值不许多走
        assert_eq!(gps_coord(31.0 + 12.0 / 60.0 + 59.94 / 3600.0, "S", "N"), "31°12'59.9\"N");
    }

    #[test]
    fn GPS海拔带符号() {
        // 样本实测 124672/13399 ≈ 9.3 m
        assert_eq!(gps_altitude(124672, 13399, false).as_deref(), Some("9.3 m"));
        assert_eq!(gps_altitude(41, 10, false).as_deref(), Some("4.1 m"));
        // 死海沿岸：低于海平面
        assert_eq!(gps_altitude(430, 1, true).as_deref(), Some("-430 m"));
        assert_eq!(gps_altitude(5, 0, false), None, "分母为 0");
        assert_eq!(gps_altitude(0, 1, false).as_deref(), Some("0 m"));
        assert_eq!(gps_altitude(200_000, 1, false), None, "离谱值应丢弃");
    }
}
