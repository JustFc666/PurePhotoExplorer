//! S1 端到端冒烟测试：生成真实图片文件 → 探测 → 解码 → 校验像素。
//!
//! **为什么需要这个**：纯逻辑单元测试无法证明
//! "解码器真的能读出正确像素"。这里生成真实的 JPEG/PNG 字节流，
//! 走完整链路并校验像素值，是 S1 阶段的验收依据。

#![cfg(test)]
// 测试函数名用中文（可读性优先），关闭 snake_case 检查
#![allow(non_snake_case)]

use pv_core::error::Result;
use pv_decode::{BitmapBackend, DecodePurpose, DecodeTarget, DecoderRegistry, ImageDecoder};
use std::path::Path;
use std::time::Instant;

/// 构造一张 4x4 的测试 PNG（RGB 手写编码，不依赖外部图像库生成）
fn make_test_png(path: &std::path::Path) -> Result<()> {
    use png::{BitDepth, ColorType};

    let w = 4u32;
    let h = 4u32;
    let mut data = Vec::new();
    // 构造 16 个像素：左上红、右上绿、左下蓝、右下白，中间灰
    for y in 0..h {
        for x in 0..w {
            let c: [u8; 3] = match (x, y) {
                (0, 0) => [255, 0, 0],
                (3, 0) => [0, 255, 0],
                (0, 3) => [0, 0, 255],
                (3, 3) => [255, 255, 255],
                _ => [128, 128, 128],
            };
            data.extend_from_slice(&c);
        }
    }

    let file = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(ColorType::Rgb);
    enc.set_depth(BitDepth::Eight);
    let mut writer = enc.write_header().unwrap();
    writer.write_image_data(&data).unwrap();
    writer.finish().unwrap();
    Ok(())
}

fn tmp_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(name)
}

#[test]
fn 端到端_生成PNG并解码出正确像素() {
    let p = tmp_path("pv_e2e_test.png");
    make_test_png(&p).expect("生成测试 PNG");

    let be = BitmapBackend;
    let backend: &dyn ImageDecoder = &be;

    // 1) 探测
    let info = backend.probe(&p).expect("探测 PNG");
    assert_eq!(info.format, pv_core::format::ImageFormat::Png);
    assert_eq!(info.width, 4, "探测宽度应为 4");
    assert_eq!(info.height, 4, "探测高度应为 4");

    // 2) 解码（全尺寸）
    let img = backend
        .decode(&p, DecodeTarget::new(DecodePurpose::Export))
        .expect("解码 PNG");
    assert_eq!((img.width, img.height), (4, 4));
    assert_eq!(img.data.len(), 4 * 4 * 4, "RGBA 长度应为 64");

    // 3) 校验关键像素（验证通道序与转换正确性）
    assert_eq!(img.pixel(0, 0), Some([255, 0, 0, 255]), "左上应为红");
    assert_eq!(img.pixel(3, 0), Some([0, 255, 0, 255]), "右上应为绿");
    assert_eq!(img.pixel(0, 3), Some([0, 0, 255, 255]), "左下应为蓝");
    assert_eq!(img.pixel(3, 3), Some([255, 255, 255, 255]), "右下应为白");
    assert_eq!(img.pixel(1, 1), Some([128, 128, 128, 255]), "中间应为灰");

    let _ = std::fs::remove_file(&p);
}

#[test]
fn 端到端_注册表可路由到PNG后端() {
    let p = tmp_path("pv_e2e_registry.png");
    make_test_png(&p).expect("生成测试 PNG");

    let mut reg = DecoderRegistry::new();
    reg.register(Box::new(BitmapBackend));

    // 探测
    let info = reg.probe(&p).expect("注册表探测应成功");
    assert_eq!(info.format, pv_core::format::ImageFormat::Png);
    assert_eq!(info.width, 4);

    // 解码
    let img = reg
        .decode(&p, DecodeTarget::new(DecodePurpose::Preview))
        .expect("注册表解码应成功");
    assert_eq!((img.width, img.height), (4, 4));

    let _ = std::fs::remove_file(&p);
}

#[test]
fn 端到端_损坏文件返回Err不panic() {
    // 造一个假 PNG：魔数正确但内容是垃圾
    let p = tmp_path("pv_e2e_corrupt.png");
    let mut fake = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    fake.extend_from_slice(&[0xFF; 200]); // 垃圾数据
    std::fs::write(&p, &fake).unwrap();

    let be = BitmapBackend;
    // 关键：不能 panic，且必须返回 Err
    let r = be.decode(&p, DecodeTarget::new(DecodePurpose::Preview));
    assert!(r.is_err(), "损坏文件应返回 Err 而非崩溃或返回垃圾数据");

    let _ = std::fs::remove_file(&p);
}

#[test]
fn 端到端_扩展名错误时按内容识别() {
    // 内容是 PNG，扩展名写成 .jpg
    let p = tmp_path("pv_e2e_mislabelled.jpg");
    make_test_png(&p).expect("生成测试 PNG");

    // 注册表按文件头路由，不看扩展名
    let mut reg = DecoderRegistry::new();
    reg.register(Box::new(BitmapBackend));

    let info = reg.probe(&p).expect("应按魔数识别为 PNG");
    assert_eq!(
        info.format,
        pv_core::format::ImageFormat::Png,
        "扩展名是 .jpg 但内容是 PNG，必须按魔数识别"
    );

    // 后端内部也必须按内容识别 —— 扩展名错误时仍要能正常解码。
    // 这条曾经失败过（早期实现按扩展名分发），说明"不信任扩展名"
    // 必须贯穿到后端内部，不能只在注册表层面做。
    let img = BitmapBackend
        .decode(&p, DecodeTarget::new(DecodePurpose::Export))
        .expect("后端应按魔数识别并成功解码，不该被扩展名误导");
    assert_eq!((img.width, img.height), (4, 4));
    assert_eq!(img.pixel(0, 0), Some([255, 0, 0, 255]), "像素应正确");

    let _ = std::fs::remove_file(&p);
}

#[test]
fn 端到端_空文件返回Err() {
    let p = tmp_path("pv_e2e_empty.png");
    std::fs::write(&p, b"").unwrap();

    let mut reg = DecoderRegistry::new();
    reg.register(Box::new(BitmapBackend));

    // 空文件连魔数都没有，探测应返回错误
    let r = reg.probe(&p);
    assert!(r.is_err(), "空文件应返回 Err");

    let _ = std::fs::remove_file(&p);
}

#[test]
fn 端到端_大图降采样到目标尺寸() {
    // 生成 2000x1000 的 PNG，验证缩放链路
    let p = tmp_path("pv_e2e_large.png");
    {
        use png::{BitDepth, ColorType};
        let (w, h) = (2000u32, 1000u32);
        let mut data = Vec::with_capacity((w * h * 3) as usize);
        for i in 0..(w * h) {
            data.push((i % 256) as u8);
            data.push(((i / 7) % 256) as u8);
            data.push(((i / 13) % 256) as u8);
        }
        let file = std::fs::File::create(&p).unwrap();
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
        enc.set_color(ColorType::Rgb);
        enc.set_depth(BitDepth::Eight);
        let mut writer = enc.write_header().unwrap();
        writer.write_image_data(&data).unwrap();
        writer.finish().unwrap();
    }

    let be = BitmapBackend;
    // 请求 256px 缩略图（应触发缩放）
    let img = be
        .decode(&p, DecodeTarget::new(DecodePurpose::Thumbnail))
        .expect("大图降采样解码");

    assert!(
        img.width <= 256 && img.height <= 256,
        "缩略图应被约束到 256 以内，实际 {}x{}",
        img.width,
        img.height
    );
    assert_eq!(img.data.len(), (img.width * img.height * 4) as usize, "长度与尺寸匹配");

    let _ = std::fs::remove_file(&p);
}

#[test]
fn 端到端_大图导出保持全尺寸() {
    let p = tmp_path("pv_e2e_export.png");
    {
        use png::{BitDepth, ColorType};
        let (w, h) = (800u32, 600u32);
        let data = vec![128u8; (w * h * 3) as usize];
        let file = std::fs::File::create(&p).unwrap();
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
        enc.set_color(ColorType::Rgb);
        enc.set_depth(BitDepth::Eight);
        let mut writer = enc.write_header().unwrap();
        writer.write_image_data(&data).unwrap();
        writer.finish().unwrap();
    }

    let be = BitmapBackend;
    // 缩略图用途会缩到 256
    let thumb = be
        .decode(&p, DecodeTarget::new(DecodePurpose::Thumbnail))
        .unwrap();
    // 编辑用途会缩到 4096（800 不够大，不缩）
    let edit = be.decode(&p, DecodeTarget::new(DecodePurpose::Edit)).unwrap();
    // 导出用途保持全尺寸
    let export = be
        .decode(&p, DecodeTarget::new(DecodePurpose::Export))
        .unwrap();

    assert!(thumb.width <= 256, "缩略图被缩放");
    assert_eq!((edit.width, edit.height), (800, 600), "编辑尺寸不缩");
    assert_eq!((export.width, export.height), (800, 600), "导出必须全尺寸");

    let _ = std::fs::remove_file(&p);
}

/// RAW 端到端：真实样本 → 探测 → 解码 → 校验像素。
///
/// **背景**（2026-10-07）：LibRaw 绑定在 Windows/MSVC 上不可用
/// （rsraw 的 build.rs 直接 panic），改为纯 Rust 提取内嵌预览。
/// 这条测试锁定整条 RAW 链路：注册表路由 → 内嵌预览提取 →
/// JPEG 解码 → RGBA 像素，确保"能看图"这件事真的成立。
///
/// 覆盖 8 个样本（CR3/DNG/RAF/NRW/ORF/RW2/SR2/SRF），
/// 校验每张都能解出正确的预览尺寸与非空像素。
#[test]
fn 真实RAW样本_探测与解码() {
    use pv_decode::EmbeddedPreviewPipeline;

    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates 下")
        .parent()
        .expect("仓库根下")
        .join("test-image");

    // (文件名, 期望宽, 期望高)
    let cases = [
        ("canon-cr3-sample.CR3", 6000u32, 4000u32),
        // DNG 也走同一条路（TIFF 容器内嵌 JPEG 预览）。
        // 2026-10-09 加上：用户提供了真实样本，实测 4032×3024 可解。
        ("IMG_7507.DNG", 4032, 3024),
        ("fuji-raf-sample.RAF", 1600, 1200),
        ("nikon-nrw-sample.NRW", 4000, 3000),
        ("olympus-orf-sample.ORF", 1600, 1200),
        ("panasonic-rw2-sample.RW2", 1920, 1280),
        ("sony-sr2-sample.SR2", 640, 424),
        ("sony-srf-sample.SRF", 640, 480),
    ];

    let mut reg = DecoderRegistry::new();
    reg.register(Box::new(BitmapBackend));
    reg.set_raw_pipeline(Box::new(EmbeddedPreviewPipeline::new()));

    let mut checked = 0usize;
    for (name, exp_w, exp_h) in cases {
        let p = dir.join(name);
        if std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0) < 4096 {
            eprintln!("跳过：样本不可用 {name}");
            continue;
        }

        // 1) 注册表探测：应识别为 RAW 且尺寸来自内嵌预览
        let info = reg
            .probe(&p)
            .unwrap_or_else(|e| panic!("{name} 探测失败: {e}"));
        assert!(info.is_raw(), "{name} 应被判定为 RAW");
        assert_eq!(
            (info.width, info.height),
            (exp_w, exp_h),
            "{name} 探测尺寸应等于最大内嵌预览"
        );
        assert!(info.raw.is_some(), "{name} 应带 RAW 元数据");

        // 2) 解码（预览档）：应解出正确尺寸的 RGBA
        let img = reg
            .decode(&p, DecodeTarget::new(DecodePurpose::Preview))
            .unwrap_or_else(|e| panic!("{name} 解码失败: {e}"));
        assert_eq!(
            img.data.len(),
            (img.width * img.height * 4) as usize,
            "{name} RGBA 长度必须与尺寸匹配"
        );
        assert!(img.width > 0 && img.height > 0, "{name} 尺寸不应为 0");

        // 3) 像素不应全黑（验证内嵌 JPEG 真的解码出了内容）
        let total = (img.width * img.height) as usize;
        let non_black = img
            .data
            .chunks_exact(4)
            .filter(|px| px[0] > 5 || px[1] > 5 || px[2] > 5)
            .count();
        assert!(
            non_black > total / 2,
            "{name} 多数像素应非全黑，实际非黑 {:.1}%",
            non_black as f64 * 100.0 / total as f64
        );

        eprintln!(
            "{name} ✓ {}×{} → 解码 {}×{}（非黑 {:.0}%）",
            info.width,
            info.height,
            img.width,
            img.height,
            non_black as f64 * 100.0 / total as f64
        );
        checked += 1;
    }

    assert!(checked >= 7, "7 个样本应全部跑通，实际 {checked} 个");
}

/// RAW 缩略图档应被约束到目标尺寸（列表/胶片条场景）。
#[test]
fn 真实RAW_缩略图档被降采样() {
    use pv_decode::EmbeddedPreviewPipeline;

    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("test-image")
        .join("canon-cr3-sample.CR3");
    if std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0) < 4096 {
        eprintln!("跳过：CR3 样本不可用");
        return;
    }

    let mut reg = DecoderRegistry::new();
    reg.register(Box::new(BitmapBackend));
    reg.set_raw_pipeline(Box::new(EmbeddedPreviewPipeline::new()));

    let thumb = reg
        .decode(&p, DecodeTarget::new(DecodePurpose::Thumbnail))
        .expect("CR3 缩略图解码");
    assert!(
        thumb.width <= 256 && thumb.height <= 256,
        "缩略图应约束到 256 以内，实际 {}×{}",
        thumb.width,
        thumb.height
    );
}

/// 未注册 RAW 管线时，RAW 应返回明确错误而不是 panic。
#[test]
fn 未接RAW管线时返回明确错误() {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("test-image")
        .join("olympus-orf-sample.ORF");
    if std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0) < 4096 {
        eprintln!("跳过：ORF 样本不可用");
        return;
    }

    let mut reg = DecoderRegistry::new();
    reg.register(Box::new(BitmapBackend));
    // 故意不注册 raw pipeline

    let r = reg.decode(&p, DecodeTarget::new(DecodePurpose::Preview));
    assert!(r.is_err(), "未接管线时应返回 Err");
}

/// 真实相机 JPEG 的端到端验证（2026-10-03 加入）
///
/// **背景**：用户在 test-image 放入 DSC9515.JPG（22 MB / 5847×3898），
/// 实测发现 jpeg-decoder 解码需 49 秒，换 zune-jpeg 后降到 231 ms。
/// 这条测试锁定三个关键行为：
/// 1. 能正确探测大图的尺寸（靠解析 SOF 标记，不依赖解码器）
/// 2. 解码耗时在合理范围内（防止性能回归）
/// 3. 像素转换正确（不是全黑）
///
/// ⚠️ **性能断言只在 release 下有意义**：
/// debug 构建无优化，22 MB 图需 11 秒；release 需 231 ms。
/// 因此 debug 下跳过耗时断言，让它专注验证正确性部分。
#[test]
fn 真实大图JPEG_探测与解码() {
    let p = tmp_path("pv_dsc9515.jpg");
    // 真实样本在 test-image/ 下
    let real = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates 下")
        .parent()
        .expect("仓库根下")
        .join("test-image")
        .join("DSC9515.JPG");
    if !real.exists() {
        eprintln!("跳过：真实样本不存在 {}", real.display());
        return;
    }
    std::fs::copy(&real, &p).expect("复制真实样本");

    let be = BitmapBackend;

    // 探测：应为 O(1)，不触发解码
    let t = Instant::now();
    let info = be.probe(&p).expect("探测应成功");
    let probe_us = t.elapsed().as_micros();
    assert_eq!(info.format, pv_core::format::ImageFormat::Jpeg);
    assert_eq!(info.width, 5847, "宽度应从 SOF 标记正确解析");
    assert_eq!(info.height, 3898, "高度应从 SOF 标记正确解析");
    assert!(
        probe_us < 50_000,
        "探测应 < 50ms（O(1) 解析 SOF），实际 {} µs",
        probe_us
    );

    // 预览档解码：应有尺寸的像素且被正确缩放
    let t = Instant::now();
    let img = be
        .decode(&p, DecodeTarget::new(DecodePurpose::Preview))
        .expect("预览解码应成功");
    let ms = t.elapsed().as_millis();

    assert_eq!(img.width, 2048, "预览档最长边应为 2048");
    assert!(img.height < 2048, "高度应等比缩小");
    assert_eq!(
        img.data.len(),
        (img.width * img.height * 4) as usize,
        "RGBA 长度必须与尺寸匹配"
    );

    // 像素不应全黑（验证 RGB→RGBA 转换正确）
    let total_px = img.data.len() / 4;
    let non_black = img
        .data
        .chunks_exact(4)
        .filter(|p| p[0] > 5 || p[1] > 5 || p[2] > 5)
        .count();
    assert!(
        non_black > total_px / 2,
        "多数像素应非全黑，实际非黑占比 {:.1}%",
        non_black as f64 * 100.0 / total_px as f64
    );

    // 性能断言：仅 release 下检查（debug 无优化，11 秒属正常）
    if cfg!(not(debug_assertions)) {
        assert!(
            ms < 1000,
            "预览档解码应 < 1000ms（回归检查：曾达 49000ms），实际 {}ms",
            ms
        );
    } else {
        eprintln!("提示：debug 构建下 {}ms，release 约 230ms（性能断言已跳过）", ms);
    }

    let _ = std::fs::remove_file(&p);
}

/// 构造一张 4x4 的 24 位 BMP（自下而上），四角红/绿/蓝/白。
///
/// 不依赖任何外部图像库，纯手写字节，便于端到端验证 BMP 解码链路。
fn make_test_bmp(path: &Path) -> std::io::Result<()> {
    let mut px = vec![[128u8, 128, 128]; 16];
    px[0] = [255, 0, 0];
    px[3] = [0, 255, 0];
    px[12] = [0, 0, 255];
    px[15] = [255, 255, 255];

    let stride = 12usize; // 4*3，已对齐到 4
    let mut pixel_bytes = Vec::new();
    for y in (0..4).rev() {
        for x in 0..4 {
            let [r, g, b] = px[y * 4 + x];
            pixel_bytes.extend_from_slice(&[b, g, r]);
        }
    }
    let dib_size = 40u32;
    let pixel_offset = 14 + dib_size;
    let file_size = pixel_offset + pixel_bytes.len() as u32;
    let mut out = Vec::new();
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&file_size.to_le_bytes());
    out.extend_from_slice(&[0u8; 4]);
    out.extend_from_slice(&(pixel_offset as u32).to_le_bytes());
    out.extend_from_slice(&dib_size.to_le_bytes());
    out.extend_from_slice(&(4i32).to_le_bytes());
    out.extend_from_slice(&(4i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(pixel_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&[0u8; 8]);
    out.extend_from_slice(&0u32.to_le_bytes()); // colors used
    out.extend_from_slice(&0u32.to_le_bytes()); // important
    out.extend_from_slice(&pixel_bytes);
    std::fs::write(path, &out)
}

#[test]
fn 端到端_生成BMP并解码出正确像素() {
    let p = tmp_path("pv_e2e_test.bmp");
    make_test_bmp(&p).expect("生成测试 BMP");

    let be = BitmapBackend;
    let backend: &dyn ImageDecoder = &be;

    // 1) 探测（走文件头魔数 "BM" → ImageFormat::Bmp）
    let info = backend.probe(&p).expect("探测 BMP");
    assert_eq!(info.format, pv_core::format::ImageFormat::Bmp);
    assert_eq!(info.width, 4, "探测宽度应为 4");
    assert_eq!(info.height, 4, "探测高度应为 4");
    assert!(info.color_space.is_some(), "应带位深信息");

    // 2) 解码（导出档 = 全尺寸）
    let img = backend
        .decode(&p, DecodeTarget::new(DecodePurpose::Export))
        .expect("解码 BMP");
    assert_eq!((img.width, img.height), (4, 4));
    assert_eq!(img.data.len(), 4 * 4 * 4, "RGBA 长度应为 64");

    // 3) 校验关键像素（验证自下而上行翻转 + BGR→RGBA 顺序）
    assert_eq!(img.pixel(0, 0), Some([255, 0, 0, 255]), "左上应为红");
    assert_eq!(img.pixel(3, 0), Some([0, 255, 0, 255]), "右上应为绿");
    assert_eq!(img.pixel(0, 3), Some([0, 0, 255, 255]), "左下应为蓝");
    assert_eq!(img.pixel(3, 3), Some([255, 255, 255, 255]), "右下应为白");

    // 4) 预览档解码（验证目标尺寸链路不崩）
    let preview = be
        .decode(&p, DecodeTarget::new(DecodePurpose::Preview))
        .expect("预览档解码");
    assert_eq!((preview.width, preview.height), (4, 4), "小图预览不放大");

    let _ = std::fs::remove_file(&p);
}

/// 取仓库 `test-image/` 下的真实样本（< 4KB 视为 Git LFS 指针，跳过）。
fn sample_path(name: &str) -> Option<std::path::PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates 下")
        .parent()
        .expect("仓库根下")
        .join("test-image");
    let p = dir.join(name);
    if std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0) >= 4096 {
        Some(p)
    } else {
        eprintln!("跳过：样本不可用 {name}");
        None
    }
}

/// **回归护栏**：普通 TIFF 必须能打开 —— 尽管它在魔数层面被判成 `OtherRaw`。
///
/// **背景（2026-10-11）**：`detect()` 把所有 TIFF 容器（小端 `II*\0` /
/// 大端 `MM\0*`）都判为 `OtherRaw`，因为相机 RAW（NEF/ARW/DNG…）也是
/// TIFF 容器、仅凭魔数分不开。于是注册表先走 RAW 管线；普通 TIFF 没有
/// 内嵌 JPEG 预览，RAW 管线必然失败 —— **若无降级钩子，用户打开 `.tif`
/// 只会看到「RAW 解码器未接入」**。
///
/// 本测试固定住「RAW 管线失败 → 降级到 TIFF 解码器」这条路径。
#[test]
fn 真实TIFF_经注册表降级仍能解码() {
    use pv_decode::EmbeddedPreviewPipeline;

    // door.tif = 大端容器、未压缩；rgb.tiff = 小端容器、LZW
    let cases = [("door.tif", 750u32, 1500u32), ("rgb.tiff", 800, 519)];

    // 关键：注册 RAW 管线，模拟真实运行环境
    let mut reg = DecoderRegistry::new();
    reg.register(Box::new(BitmapBackend));
    reg.set_raw_pipeline(Box::new(EmbeddedPreviewPipeline::new()));

    let mut checked = 0usize;
    for (name, w, h) in cases {
        let Some(p) = sample_path(name) else { continue };

        let info = reg
            .probe(&p)
            .unwrap_or_else(|e| panic!("{name} 探测失败（应降级到 TIFF）: {e}"));
        assert_eq!(
            (info.width, info.height),
            (w, h),
            "{name} 尺寸应来自 TIFF 容器而非 RAW 预览"
        );

        let img = reg
            .decode(&p, DecodeTarget::new(DecodePurpose::Preview))
            .unwrap_or_else(|e| panic!("{name} 解码失败（应降级到 TIFF）: {e}"));
        assert!(img.width > 0 && img.height > 0, "{name} 应解出像素");
        assert_eq!(
            img.data.len(),
            (img.width as usize) * (img.height as usize) * 4,
            "{name} RGBA 长度应与尺寸匹配"
        );
        // 整幅同色 = 解码失败（能过尺寸断言但像素是空的）
        let distinct = img
            .data
            .chunks_exact(4)
            .step_by(701)
            .map(|px| (px[0], px[1], px[2]))
            .collect::<std::collections::HashSet<_>>();
        assert!(distinct.len() > 10, "{name} 采样颜色过少，疑似解码异常");

        checked += 1;
    }
    assert!(checked > 0, "两个 TIFF 样本都不可用，测试没有实际覆盖");
}

/// TIFF 降级不得**误伤真实 RAW**：相机 RAW 仍必须走内嵌预览路径。
#[test]
fn 真实RAW不被TIFF降级劫持() {
    use pv_decode::EmbeddedPreviewPipeline;

    let Some(p) = sample_path("nikon-nrw-sample.NRW") else { return };

    let mut reg = DecoderRegistry::new();
    reg.register(Box::new(BitmapBackend));
    reg.set_raw_pipeline(Box::new(EmbeddedPreviewPipeline::new()));

    let info = reg.probe(&p).expect("NRW 探测");
    assert!(info.is_raw(), "真实 RAW 仍应判为 RAW（带内嵌预览元数据）");
    assert!(
        info.raw.is_some(),
        "真实 RAW 必须保留 RAW 元数据，不能被降级成普通 TIFF"
    );
    assert_eq!((info.width, info.height), (4000, 3000));
}

/// AVIF 走**正常后端路由**（魔数就判成 `Avif`，不进 RAW 分支）。
#[test]
fn 真实AVIF_经注册表探测与解码() {
    let Some(p) = sample_path("10007.avif") else { return };

    let mut reg = DecoderRegistry::new();
    reg.register(Box::new(BitmapBackend));

    let info = reg.probe(&p).expect("AVIF 探测");
    assert_eq!(info.format, pv_core::format::ImageFormat::Avif);
    assert!(info.width > 0 && info.height > 0);

    let img = reg
        .decode(&p, DecodeTarget::new(DecodePurpose::Preview))
        .expect("AVIF 解码");
    assert_eq!(
        img.data.len(),
        (img.width as usize) * (img.height as usize) * 4,
        "RGBA 长度应与尺寸匹配"
    );

    // 缩略图档应被降采样
    let thumb = reg
        .decode(&p, DecodeTarget::new(DecodePurpose::Thumbnail))
        .expect("AVIF 缩略图");
    assert_eq!(thumb.width.max(thumb.height), 256);
}
