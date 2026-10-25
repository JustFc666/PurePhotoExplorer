//! ⭐ 护栏：GPS 度分秒写成 **SRATIONAL** 的机型也要能解析。
//!
//! 规范里 `GPSLatitude` / `GPSLongitude` 是 RATIONAL（无符号），
//! 但 ASUS_I001DA 这类手机写成了 SRATIONAL。只认 RATIONAL 时，
//! kamadak 明明取到了字段，我们却返回 None —— 用户看到的是
//! 「Windows 属性里有经纬度，软件里没有」。
//!
//! 样本：`test-image/P_20210729_111300.jpg`（30°20'11.04"N, 108°58'54.79"E）

use std::path::Path;

#[test]
fn 手机把GPS写成SRATIONAL也能解析() {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-image/P_20210729_111300.jpg");
    if !p.is_file() {
        eprintln!("跳过：找不到样本 {}", p.display());
        return;
    }

    let exif = pv_meta::read_exif(&p).expect("读不到 EXIF");
    let gps = exif.gps.expect("GPS 不该是 None（SRATIONAL 要兼容）");
    let (lat, lon) = gps.lat_lon().expect("坐标要能转成数值");

    // 30 + 20/60 + 11.04/3600 = 30.336400
    assert!((lat - 30.3364).abs() < 1e-4, "纬度算错：{lat}");
    // 108 + 58/60 + 54.79/3600 = 108.981886
    assert!((lon - 108.981886).abs() < 1e-4, "经度算错：{lon}");
    assert!(gps.display.contains('N') && gps.display.contains('E'), "展示文本：{}", gps.display);
}
