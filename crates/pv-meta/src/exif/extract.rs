//! 从解析好的 EXIF 字段表里**取字段**并填充 [`ExifData`]。
//!
//! 只做取值与组合，格式化一律委托给 [`super::format`]（纯函数，好单测）。

use exif::{Exif, In, Tag, Value};

use super::format;
use super::{ExifData, GpsData};

/// 把相机里的原始字段整理成前端要用的结构。
pub(super) fn extract(exif: &Exif) -> ExifData {
    ExifData {
        make: ascii(exif, Tag::Make),
        model: ascii(exif, Tag::Model),
        lens: lens(exif),
        date_time_original: ascii(exif, Tag::DateTimeOriginal).as_deref().and_then(format::date_time),
        exposure_time: rational(exif, Tag::ExposureTime)
            .and_then(|(n, d)| format::exposure_time(n, d)),
        f_number: rational(exif, Tag::FNumber).and_then(|(n, d)| format::f_number(n, d)),
        iso: iso(exif),
        focal_length: rational(exif, Tag::FocalLength)
            .and_then(|(n, d)| format::focal_length(n, d)),
        focal_length_35mm: uint(exif, Tag::FocalLengthIn35mmFilm).filter(|v| *v > 0),
        exposure_bias: srational(exif, Tag::ExposureBiasValue)
            .and_then(|(n, d)| format::exposure_bias(n, d)),
        exposure_program: uint(exif, Tag::ExposureProgram)
            .and_then(format::exposure_program)
            .map(String::from),
        metering_mode: uint(exif, Tag::MeteringMode)
            .and_then(format::metering_mode)
            .map(String::from),
        flash: uint(exif, Tag::Flash).map(format::flash),
        white_balance: uint(exif, Tag::WhiteBalance)
            .and_then(format::white_balance)
            .map(String::from),
        orientation: uint(exif, Tag::Orientation)
            .and_then(format::orientation)
            .map(String::from),
        software: ascii(exif, Tag::Software),
        gps: gps(exif),
    }
}

/// 拍摄地点：度分秒 + 方位 → 十进制度，另附海拔。
///
/// ## 为什么三个字段缺一不可
/// `GPSLatitude` 只给「31°12'17.42"」这个**绝对值**，南北半球由
/// `GPSLatitudeRef`（`N`/`S`）决定。少了 ref 就只能猜 —— 猜错会把
/// 上海（北纬）标到南半球去，**「标错地方」比「不显示」糟糕得多**，
/// 所以这里要求 ref 必须存在，否则整条坐标判缺失。
///
/// 海拔是可选装饰项：`GPSAltitude` 缺失时坐标照常显示。
fn gps(exif: &Exif) -> Option<GpsData> {
    let lat_ref = ascii(exif, Tag::GPSLatitudeRef)?;
    let lon_ref = ascii(exif, Tag::GPSLongitudeRef)?;
    let lat_dms = rationals(exif, Tag::GPSLatitude)?;
    let lon_dms = rationals(exif, Tag::GPSLongitude)?;

    // ref 只认首字母，且必须是 N/S、E/W —— 脏数据（空串、数字）宁可丢弃
    let lat = signed_decimal(&lat_dms, &lat_ref, 'S')?;
    let lon = signed_decimal(&lon_dms, &lon_ref, 'W')?;

    let altitude = match rational(exif, Tag::GPSAltitude) {
        Some((n, d)) => {
            // GPSAltitudeRef：0 = 海平面以上，1 = 以下；缺失按「以上」
            let below = uint(exif, Tag::GPSAltitudeRef).map(|v| v == 1).unwrap_or(false);
            format::gps_altitude(n, d, below)
        }
        None => None,
    };

    Some(GpsData {
        latitude: trim_decimal(lat),
        longitude: trim_decimal(lon),
        altitude,
        display: format!("{}, {}", format::gps_coord(lat, "S", "N"), format::gps_coord(lon, "W", "E")),
    })
}

/// 十进制度 + 方位字母 → 带符号数值。`neg` 是表示负向的那个字母。
fn signed_decimal(dms: &[(u32, u32)], ref_text: &str, neg: char) -> Option<f64> {
    let first = ref_text.chars().next()?.to_ascii_uppercase();
    if first != neg && first != opposite(neg) {
        return None;
    }
    let dec = format::gps_decimal(dms)?;
    Some(if first == neg { -dec.abs() } else { dec.abs() })
}

/// 与 `neg` 配对的另一方位字母（S↔N、W↔E）。
fn opposite(neg: char) -> char {
    match neg {
        'S' => 'N',
        'W' => 'E',
        _ => neg,
    }
}

/// 数值 → 6 位小数字符串（跟 `format::gps_coord` 的展示精度对齐）。
fn trim_decimal(v: f64) -> String {
    format!("{v:.6}")
}

/// ISO 感光度。优先标准标签，缺失时退回旧的 `ISOSpeed`。
fn iso(exif: &Exif) -> Option<u32> {
    uint(exif, Tag::PhotographicSensitivity)
        .or_else(|| uint(exif, Tag::ISOSpeed))
        .filter(|v| *v > 0 && *v <= 4_000_000)
}

/// 镜头：优先「型号」，缺失时退回「厂商」，两者都有且型号不含厂商名时合并。
fn lens(exif: &Exif) -> Option<String> {
    match (ascii(exif, Tag::LensMake), ascii(exif, Tag::LensModel)) {
        (Some(make), Some(model)) => {
            let brand = make.split_whitespace().next().unwrap_or("").to_lowercase();
            if !brand.is_empty() && model.to_lowercase().contains(&brand) {
                Some(model)
            } else {
                Some(format!("{make} {model}"))
            }
        }
        (Some(make), None) => Some(make),
        (None, Some(model)) => Some(model),
        (None, None) => None,
    }
}

/// 取 ASCII 字段（清理首尾空白与控制字符；空串视为缺失）。
fn ascii(exif: &Exif, tag: Tag) -> Option<String> {
    let field = exif.get_field(tag, In::PRIMARY)?;
    let raw = match &field.value {
        Value::Ascii(list) => list.first().cloned()?,
        // 少数相机把字符串写成 UNDEFINED/BYTE，兜底转成文本
        other => other.display_as(tag).to_string().into_bytes(),
    };
    clean(&raw)
}

/// 取无符号整数（取首个元素）。
fn uint(exif: &Exif, tag: Tag) -> Option<u32> {
    exif.get_field(tag, In::PRIMARY)?.value.get_uint(0)
}

/// 取无符号有理数 `(分子, 分母)`。
fn rational(exif: &Exif, tag: Tag) -> Option<(u32, u32)> {
    rationals(exif, tag)?.first().copied()
}

/// 取**全部**无符号有理数（GPS 的度分秒是连续 3 个 RATIONAL）。
fn rationals(exif: &Exif, tag: Tag) -> Option<Vec<(u32, u32)>> {
    match &exif.get_field(tag, In::PRIMARY)?.value {
        Value::Rational(v) => Some(v.iter().map(|r| (r.num, r.denom)).collect()),
        _ => None,
    }
}

/// 取有符号有理数 `(分子, 分母)`。
///
/// `ExposureBiasValue` 规范上是 SRATIONAL，但少量机型写成 RATIONAL，
/// 因此两种都接受（负值只可能来自 SRATIONAL）。
fn srational(exif: &Exif, tag: Tag) -> Option<(i32, i32)> {
    match &exif.get_field(tag, In::PRIMARY)?.value {
        Value::SRational(v) => v.first().map(|r| (r.num, r.denom)),
        Value::Rational(v) => v.first().map(|r| (r.num as i32, r.denom as i32)),
        _ => None,
    }
}

/// 清掉首尾空白 / NUL / 控制字符，空串归一为 `None`。
fn clean(bytes: &[u8]) -> Option<String> {
    let text: String = String::from_utf8_lossy(bytes)
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::clean;

    #[test]
    fn 清理字符串() {
        assert_eq!(clean(b"Canon\0"), Some("Canon".into()));
        assert_eq!(clean(b"  NIKON  "), Some("NIKON".into()));
        assert_eq!(clean(b"\0  \0"), None);
        assert_eq!(clean(b""), None);
    }
}
