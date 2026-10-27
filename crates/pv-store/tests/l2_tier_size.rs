//! 护栏：L2 缩略图**读回时不能被重新缩成 256**。
//!
//! ## 这个 bug 的代价
//! `DecodePurpose::Thumbnail` 的 `suggested_max_edge()` 是 **256**。
//! `Store::thumbnail()` 的 L2 命中分支原先用 `DecodeTarget::new(Thumbnail)` 解码，
//! 于是缓存里的 720×1280 被缩成 **144×256** —— md/lg/xl **所有档位**全部退化成 256 长边。
//! 表现是「大图预览糊、人脸头像糊」，但不报错、不崩，查了很久（2026-10-27）。
//!
//! 正确写法：`DecodeTarget::exact(Thumbnail, None)` —— 缓存文件本身就是目标尺寸。

use pv_core::format::ImageFormat;
use pv_decode::{encode_rgba, DecodePurpose, DecodeTarget, RgbaImage};

/// 造一张纯色 RGBA。
fn solid(w: u32, h: u32) -> RgbaImage {
    RgbaImage {
        width: w,
        height: h,
        data: vec![128u8; (w * h * 4) as usize],
    }
}

#[test]
fn l2_读回保持缓存图原始尺寸() {
    // 512 长边的缓存文件（模拟 lg/md 档）
    let img = solid(512, 512);
    let bytes = encode_rgba(&img, ImageFormat::Jpeg)
        .expect("编码失败");

    // 正确：不限制尺寸 → 保持 512
    let good = pv_decode::decode_jpeg_from_bytes(
        &bytes,
        DecodeTarget::exact(DecodePurpose::Thumbnail, None),
    )
    .expect("解码失败");
    assert_eq!(
        (good.width, good.height),
        (512, 512),
        "L2 读回必须保持缓存图尺寸，不能被缩成 256"
    );

    // 反证：这就是修复前的行为（new() 取 Thumbnail 的默认 256）
    let bad = pv_decode::decode_jpeg_from_bytes(
        &bytes,
        DecodeTarget::new(DecodePurpose::Thumbnail),
    )
    .expect("解码失败");
    assert_eq!(
        (bad.width, bad.height),
        (256, 256),
        "`new(Thumbnail)` 的默认边长是 256 —— 正是当初把 xl 缩成 144×256 的原因"
    );
}

#[test]
fn 竖图也不会被压成256() {
    // 真实案例：720×1280 的竖图，xl 档缓存是 720×1280
    let img = solid(720, 1280);
    let bytes = encode_rgba(&img, ImageFormat::Jpeg)
        .expect("编码失败");
    let out = pv_decode::decode_jpeg_from_bytes(
        &bytes,
        DecodeTarget::exact(DecodePurpose::Thumbnail, None),
    )
    .expect("解码失败");
    assert_eq!((out.width, out.height), (720, 1280));
}
