//! HEIF/HEIC 解码（`heif-oxide` 容器解析 + `rust_h265` HEVC 解码，**全纯 Rust**）。
//!
//! ## 为什么是这个库（2026-10-09 选型，改前必读）
//!
//! HEIC = ISOBMFF 容器 + HEVC 编码。三条可选路线，实测后只留下第三条：
//!
//! | 路线 | 结论 |
//! |---|---|
//! | Windows WIC（系统解码器） | ❌ **本机解不出帧**。`CreateDecoderFromFilename` 成功（说明 HEIF 容器解析器在），但 `GetFrame(0)` 返回 `0x88982F44` = `WINCODEC_ERR_FRAMEMISSING`；系统未安装 `Microsoft.HEVCVideoExtension`，而该 HEIC 里 53 个 `hvc1` item **全是 HEVC**（搜遍全文件无一个 JPEG/PNG，6 个 `mime` item 是 `application/rdf+xml` 的 XMP，不是缩略图）→ 没有"内嵌预览"这条降级路 |
//! | libheif + libde265（C 库） | ❌ 本机无 cmake / nasm，需要自行编译 C 库 |
//! | **`heif-oxide`（纯 Rust）** | ✅ 实测 3024×4032 / **98ms** / 色彩与方向全对 |
//!
//! ⚠️ 顺带排除了一条更省事的路：用「内嵌缩略图」当预览。**这个 HEIC 里没有**，
//! 而且即使有，实现上也要写 ISOBMFF 的 `iloc`/`iinf` 解析 —— 不如直接全解。
//!
//! ## 这个库替我们做掉了什么
//! - **grid 分块**：iPhone/小米的照片不是一张码流，而是 512×512 的 tile 网格
//!   （本样本 53 块），要解码后再拼。库内部并发解码 tile。
//! - **方向**：`irot` / `imir` / `clap` 按 `ipma` 声明的顺序应用。
//! - **色彩**：BT.601/709/2020 矩阵、full/limited range、**Display P3 → sRGB**
//!   （iPhone 默认色彩空间）。**输出即 display-ready sRGB**，我们不必再转。
//! - 8-bit / 10-bit（10-bit 给 16-bit 缓冲，我们取高字节降到 8-bit）。
//!
//! ## 不支持
//! AVIF（`av01`）、JPEG-in-HEIF、图像序列（`moov` 轨道）—— 库会给明确错误。
//! 我们只管**静图**：HEIC 没有动图概念，所以不涉及多帧。

use std::path::Path;

use pv_core::error::{PvError, Result};
use pv_core::format::ImageFormat;

use super::resize::resize_rgba;
use crate::decoder::{fit_within, DecodeTarget, ImageInfo, RgbaImage};

/// HEIC 只声明"有损压缩的 RGB"这一条色彩说明。
///
/// 不用报 `Display P3` 之类 —— `heif-oxide` 已经把像素转成 sRGB 了，
/// 这里说的是**我们交出去的像素是什么**，不是源文件的色彩空间。
const COLOR_SPACE: &str = "sRGB（HEVC 解码后已转换）";

/// 探测 HEIC 尺寸。
///
/// ⚠️ **这里是全解码，不是"只读头"**（与其他格式的 probe 不同）。
/// `heif-oxide` 没暴露轻量的容器查询接口（`boxes` 模块是私有的），
/// 而自己写一套 ISOBMFF 解析去拿 `ispe` 有两个坑：
/// 1. 主图往往是 `grid` item，"尺寸"要由 `grid` 的行列数 × tile 尺寸算出来；
/// 2. 方向（`irot`）会把宽高对调，而正确关联 item↔property 需要解析 `ipma`。
/// 猜一个尺寸再去解码，会出现"信息面板显示 4032×3024、实际画出来 3024×4032"。
///
/// 代价可控：本样本 98ms（3024×4032），比多数大 JPEG 还快 —— 毕竟 HEIC 的
/// 解码器是纯 Rust 的，没有多余的进程/COM 往返。
pub(super) fn probe_heif(path: &Path) -> Result<ImageInfo> {
    let img = decode_any(path)?;
    Ok(ImageInfo {
        format: ImageFormat::Heif,
        width: img.width,
        height: img.height,
        // HEIC 的 EXIF 在容器的 `Exif` item 里，不是我们熟悉的 APP1 段。
        // 解析它要再写一遍 ISOBMFF 定位，本期先不做（信息面板显示空即可）。
        exif: None,
        raw: None,
        color_space: Some(COLOR_SPACE.to_string()),
    })
}

/// 解码 HEIC 为 RGBA（按 `target` 缩放）。
pub(super) fn decode_heif(path: &Path, target: DecodeTarget) -> Result<RgbaImage> {
    let img = decode_any(path)?;
    let (w, h) = (img.width, img.height);
    let full = RgbaImage {
        width: w,
        height: h,
        // to_rgba8 会补 alpha=255（HEIC 多数无 alpha）；10-bit 源取高字节
        data: img.to_rgba8(),
    };

    let (tw, th) = fit_within(w, h, target.max_edge);
    if (tw, th) == (w, h) {
        Ok(full)
    } else {
        resize_rgba(&full, tw, th)
    }
}

/// 统一的解码入口：把 `heif-oxide` 的错误翻译成用户看得懂的话。
fn decode_any(path: &Path) -> Result<heif_oxide::DecodedImage> {
    heif_oxide::decode_file(path).map_err(|e| {
        PvError::Decode(format!(
            "HEIC/HEIF 解码失败：{}（仅支持 HEVC 编码的静图；AVIF、JPEG-in-HEIF 与图像序列暂不支持）",
            e
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::DecodePurpose;
    use std::path::PathBuf;

    /// 取仓库 `test-image/` 下的真实样本（小于 4KB 视为 Git LFS 指针）
    fn sample(name: &str) -> Option<PathBuf> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-image");
        assert!(
            dir.is_dir(),
            "找不到 test-image 目录（推算 {}）—— 样本定位逻辑坏了，测试会假通过",
            dir.display()
        );
        let p = dir.join(name);
        if p.is_file() && std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0) > 4096 {
            Some(p)
        } else {
            None
        }
    }

    #[test]
    fn 非HEIC文件报错而不是panic() {
        let dir = std::env::temp_dir().join("pv_heif_bad");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("not.heic");
        std::fs::write(&p, b"this is definitely not a heif file").unwrap();

        assert!(
            decode_heif(&p, DecodeTarget::new(DecodePurpose::Preview)).is_err(),
            "非 HEIC 文件必须返回错误"
        );
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn 真实HEIC探测尺寸() {
        let Some(p) = sample("IMG_6896.HEIC") else {
            eprintln!("跳过：找不到 HEIC 样本");
            return;
        };
        let info = probe_heif(&p).unwrap();
        assert_eq!(info.format, ImageFormat::Heif);
        // 竖拍手机照片：3024×4032（容器的 ispe 与解码结果一致，说明方向已应用）
        assert_eq!((info.width, info.height), (3024, 4032));
        assert!(!info.is_raw(), "HEIC 不是 RAW");
    }

    #[test]
    fn 真实HEIC解码出正确像素() {
        let Some(p) = sample("IMG_6896.HEIC") else {
            eprintln!("跳过：找不到 HEIC 样本");
            return;
        };
        // 用 Export（原图，不缩放）才能验证真实解码尺寸；
        // Preview 会按 max_edge=2048 等比缩到 1536×2048（另有测试覆盖）
        let t = std::time::Instant::now();
        let img = decode_heif(&p, DecodeTarget::new(DecodePurpose::Export)).unwrap();
        let dt = t.elapsed();

        assert_eq!((img.width, img.height), (3024, 4032));
        assert_eq!(img.data.len(), 3024 * 4032 * 4);
        assert!(img.data.chunks_exact(4).all(|px| px[3] == 255), "HEIC 无 alpha，应全为 255");

        // 不能是纯黑/纯白（解码失败常表现为整幅同色）
        let distinct = img.data.chunks_exact(4).step_by(4001).map(|px| (px[0], px[1], px[2])).collect::<std::collections::HashSet<_>>();
        assert!(distinct.len() > 20, "采样出的颜色过少（{}），疑似解码异常", distinct.len());

        eprintln!("HEIC 解码 3024×4032 用时 {:?}", dt);
    }

    #[test]
    fn HEIC按目标尺寸缩放() {
        let Some(p) = sample("IMG_6896.HEIC") else {
            eprintln!("跳过：找不到 HEIC 样本");
            return;
        };
        // Thumbnail = 256 长边
        let img = decode_heif(&p, DecodeTarget::new(DecodePurpose::Thumbnail)).unwrap();
        assert_eq!(img.width.max(img.height), 256);
        assert_eq!(img.data.len(), (img.width * img.height * 4) as usize);
        // 长宽比应与原图一致（4032:3024 = 4:3）
        let ratio = img.width as f64 / img.height as f64;
        assert!((ratio - 3024.0 / 4032.0).abs() < 0.01, "缩放后长宽比应保持，实际 {ratio}");
    }
}
