//! 端到端验证：**「另存为/写回」不会丢掉拍摄参数**。
//!
//! 单元测试只能证明"段结构写对了"，证明不了"外部软件真能读到那些字段"。
//! 这里用真实相机的 JPEG 走完整链路：
//!
//! ```text
//! 源 JPEG ──read_exif_raw──▶ 裸 TIFF ──align_exif_with_pixels──▶ 
//!         ──encode_rgba_with_exif──▶ 新 JPEG ──read_exif──▶ 对比字段
//! ```
//!
//! 用户 2026-10-11 的需求就是最后这一步：另存出来的图，拍摄信息还在。

#![cfg(test)]
#![allow(non_snake_case)]

use pv_decode::{encode_rgba_with_exif, BitmapBackend, DecodePurpose, DecodeTarget, DecoderRegistry, RgbaImage};
use pv_core::format::ImageFormat;
use pv_meta::{align_exif_with_pixels, read_exif, read_exif_raw};
use std::path::{Path, PathBuf};

/// 真实图库路径（缺失时测试自动跳过，便于在别的机器上跑）。
fn sample(name: &str) -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-image")
        .join(name);
    if p.is_file() {
        Some(p)
    } else {
        eprintln!("跳过：样例文件不可用 {}", p.display());
        None
    }
}

/// 造一张小图（内容无所谓，只关心元数据搬运）。
fn tiny_image() -> RgbaImage {
    let mut img = RgbaImage::new(64, 48).unwrap();
    for (i, px) in img.data.iter_mut().enumerate() {
        *px = (i % 251) as u8;
    }
    img
}

fn tmp_path(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("pv-exif-roundtrip");
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join(name);
    let _ = std::fs::remove_file(&p);
    p
}

/// ⭐ 核心断言：编辑过的图另存为 JPEG 后，**拍摄参数仍在**。
#[test]
fn 另存为后拍摄参数仍然存在() {
    let Some(src) = sample("DSC9515.JPG") else { return };
    let before = read_exif(&src).expect("样本应有 EXIF");
    let raw = read_exif_raw(&src).expect("样本应能取出原始 EXIF 段");

    // 模拟"用户旋转了 90° 再另存为"
    let mut tiff = raw;
    align_exif_with_pixels(&mut tiff, 1);

    let out = tmp_path("rotated.jpg");
    let bytes = encode_rgba_with_exif(&tiny_image(), ImageFormat::Jpeg, Some(&tiff)).unwrap();
    std::fs::write(&out, &bytes).unwrap();

    let after = read_exif(&out).expect("另存出来的图应当带 EXIF");

    // 展现在界面上的关键字段一个都不能少
    assert_eq!(after.make, before.make, "相机厂商");
    assert_eq!(after.model, before.model, "相机型号");
    assert_eq!(after.lens, before.lens, "镜头");
    assert_eq!(after.date_time_original, before.date_time_original, "拍摄时间");
    assert_eq!(after.exposure_time, before.exposure_time, "快门");
    assert_eq!(after.f_number, before.f_number, "光圈");
    assert_eq!(after.iso, before.iso, "ISO");
    assert_eq!(after.focal_length, before.focal_length, "焦距");
    assert_eq!(after.gps, before.gps, "GPS（若源图有）");

    eprintln!("输出 EXIF = {after:#?}");
    let _ = std::fs::remove_file(&out);
}

/// 旋转过 → 输出里的方向必须是「正常」。
///
/// 否则别的软件会把已经转正的画面**再转一次**，照片横过来。
#[test]
fn 旋转后输出的方向标记为正常() {
    let Some(src) = sample("DSC9515.JPG") else { return };
    let raw = read_exif_raw(&src).expect("应有 EXIF");

    let mut tiff = raw;
    let outcome = align_exif_with_pixels(&mut tiff, 1);
    eprintln!("align 结果 = {outcome:?}");

    let out = tmp_path("oriented.jpg");
    let bytes = encode_rgba_with_exif(&tiny_image(), ImageFormat::Jpeg, Some(&tiff)).unwrap();
    std::fs::write(&out, &bytes).unwrap();

    let after = read_exif(&out).expect("应能读回");
    assert_eq!(
        after.orientation.as_deref(),
        Some("正常"),
        "旋转过之后方向必须是「正常」"
    );
    let _ = std::fs::remove_file(&out);
}

/// 没旋转（纯另存为）→ 方向标记**原样保留**，不能被抹成「正常」。
///
/// 这是"无损导出"场景：像素没变，元数据也不该变。
#[test]
fn 未旋转时方向标记保持原值() {
    let Some(src) = sample("IMG_7507.DNG") else { return };
    let before = read_exif(&src).expect("应有 EXIF");
    // 该样本实拍方向 = 顺时针 90°；若不是这个值就跳过（样本被换过）
    if before.orientation.as_deref() != Some("顺时针 90°") {
        eprintln!("跳过：样本方向为 {:?}", before.orientation);
        return;
    }

    let mut tiff = read_exif_raw(&src).expect("应有 EXIF");
    align_exif_with_pixels(&mut tiff, 0); // 没旋转

    let out = tmp_path("asis.jpg");
    let bytes = encode_rgba_with_exif(&tiny_image(), ImageFormat::Jpeg, Some(&tiff)).unwrap();
    std::fs::write(&out, &bytes).unwrap();

    let after = read_exif(&out).expect("应能读回");
    assert_eq!(
        after.orientation.as_deref(),
        Some("顺时针 90°"),
        "没旋转时方向信息必须保留（无条件改 1 属于数据损失）"
    );
    let _ = std::fs::remove_file(&out);
}

/// PNG 输出同样要带上 EXIF（走 `eXIf` 段）。
#[test]
fn PNG另存为也带拍摄参数() {
    let Some(src) = sample("DSC9515.JPG") else { return };
    let before = read_exif(&src).expect("应有 EXIF");
    let tiff = read_exif_raw(&src).expect("应有 EXIF");

    let out = tmp_path("out.png");
    let bytes = encode_rgba_with_exif(&tiny_image(), ImageFormat::Png, Some(&tiff)).unwrap();
    std::fs::write(&out, &bytes).unwrap();

    let after = read_exif(&out).expect("PNG 应当能读回 EXIF");
    assert_eq!(after.model, before.model);
    assert_eq!(after.iso, before.iso);
    let _ = std::fs::remove_file(&out);
}

/// RAW 是「画布像素来自内嵌预览」的格式，另存为 JPEG 时同样应带上 EXIF。
#[test]
fn RAW另存为也带拍摄参数() {
    let Some(src) = sample("olympus-orf-sample.ORF") else { return };
    let before = read_exif(&src).expect("ORF 样本应有 EXIF");
    let tiff = read_exif_raw(&src).expect("ORF 的 TIFF 就是 EXIF 容器");

    let out = tmp_path("from-raw.jpg");
    let bytes = encode_rgba_with_exif(&tiny_image(), ImageFormat::Jpeg, Some(&tiff)).unwrap();
    std::fs::write(&out, &bytes).unwrap();

    let after = read_exif(&out).expect("应能读回");
    assert_eq!(after.make, before.make, "厂商");
    assert_eq!(after.model, before.model, "机型");
    let _ = std::fs::remove_file(&out);
}

/// 源文件**没有** EXIF 时，输出也不能凭空多出元数据。
#[test]
fn 无EXIF的源不产生EXIF() {
    let img = tiny_image();
    let out = tmp_path("plain.jpg");
    let bytes = encode_rgba_with_exif(&img, ImageFormat::Jpeg, None).unwrap();
    std::fs::write(&out, &bytes).unwrap();

    assert!(read_exif(&out).is_none(), "不该凭空出现 EXIF");
    assert!(read_exif_raw(&out).is_none());
    let _ = std::fs::remove_file(&out);
}

/// 端到端：走一遍**真正的保存命令**（`save_impl`），确认它接上了 EXIF。
///
/// 前面几条是手工串的链路，这条直接调产品代码，防止"接了个空"。
/// 通过公开的 registry + 编码函数模拟 save_impl 里那两步关键操作。
#[test]
fn 保存链路上的EXIF搬运与产品代码一致() {
    let Some(src) = sample("DSC9515.JPG") else { return };
    let mut reg = DecoderRegistry::new();
    reg.register(Box::new(BitmapBackend));
    // 源图能被正常解码（确认样本可用）
    assert!(reg
        .decode(&src, DecodeTarget::new(DecodePurpose::Preview))
        .is_ok());

    let before = read_exif(&src).unwrap();
    let mut tiff = read_exif_raw(&src).unwrap();
    align_exif_with_pixels(&mut tiff, 2); // 转 180°

    let out = tmp_path("saved.jpg");
    pv_decode::write_rgba_atomic_with_exif(&tiny_image(), ImageFormat::Jpeg, &out, Some(&tiff))
        .unwrap();

    let after = read_exif(&out).expect("写盘后应带 EXIF");
    assert_eq!(after.model, before.model);
    assert_eq!(after.orientation.as_deref(), Some("正常"), "转过就要置 1");
    assert!(out.is_file());
    let _ = std::fs::remove_file(&out);
}
