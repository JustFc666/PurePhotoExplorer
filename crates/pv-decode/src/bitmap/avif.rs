//! AVIF 解码（AV1-in-ISOBMFF，纯 Rust）。
//!
//! ## 为什么用 `avif-decode`
//! AVIF = AV1 帧塞进 HEIF 容器。底层 AV1 解码最稳的是 dav1d（C）/ libaom，
//! 但都要编译 C。Kornel 的 `avif-decode` 把 dav1d 移植成了纯 Rust 的
//! `rav1d`（`avif-decode` 直接依赖它），于是**零 C 依赖、纯 Rust 可编译**。
//!
//! 唯一代价：`rav1d` 默认开 `asm` 特性，需要 `nasm` 才能编译
//!（本机已装 `nasm` 并加入 PATH）。若不想依赖 nasm，可改用 `zenavif`，
//! 但其输出是复杂的 `PixelBuffer`，映射成本更高，故不取。
//!
//! `avif-decode` 直接输出简单的 `Image` 枚举
//!（`Rgb8` / `Rgba8` / `Rgb16` / `Rgba16` / `Gray8` / `Gray16`），
//! 本模块负责统一映射到我们的 RGBA。16-bit 取高字节（与 TIFF / HEIC 一致）。

use std::path::Path;

use pv_core::error::{PvError, Result};
use pv_core::format::ImageFormat;
use rgb::{Rgb, Rgba};

use crate::decoder::{fit_within, DecodeTarget, ImageInfo, RgbaImage};
use super::resize::resize_rgba;

/// 探测 AVIF 尺寸。
///
/// AVIF 没有「只读头拿尺寸」的轻量路径（`avif-decode` 必须解码），
/// 这里直接解码取尺寸 —— 对预览/信息面板足够快。
pub(crate) fn probe_avif(path: &Path) -> Result<ImageInfo> {
    let img = decode_avif_pixels(path)?;
    Ok(ImageInfo {
        format: ImageFormat::Avif,
        width: img.width,
        height: img.height,
        exif: None,
        raw: None,
        color_space: Some("AVIF".to_string()),
    })
}

/// 解码 AVIF 为 RGBA（按 `target` 缩放）。
pub(crate) fn decode_avif(path: &Path, target: DecodeTarget) -> Result<RgbaImage> {
    let full = decode_avif_pixels(path)?;
    let (tw, th) = fit_within(full.width, full.height, target.max_edge);
    if (tw, th) == (full.width, full.height) {
        Ok(full)
    } else {
        resize_rgba(&full, tw, th)
    }
}

/// 读文件 → 解码 AVIF → 转 RGBA（全分辨率）。
fn decode_avif_pixels(path: &Path) -> Result<RgbaImage> {
    let bytes = std::fs::read(path).map_err(crate::io_err(path))?;
    let image = avif_decode::Decoder::from_avif(&bytes)
        .and_then(|d| d.to_image())
        .map_err(|e| PvError::Decode(format!("AVIF 解码失败（可能非标准 AVIF 或损坏）：{e}")))?;

    let (w, h, data) = match image {
        avif_decode::Image::Rgb8(img) => {
            let (w, h) = (img.width() as u32, img.height() as u32);
            let mut data = vec![0u8; (w as usize) * (h as usize) * 4];
            for (i, p) in img.buf().iter().enumerate() {
                let Rgb { r, g, b } = *p;
                let o = i * 4;
                data[o..o + 4].copy_from_slice(&[r, g, b, 255]);
            }
            (w, h, data)
        }
        avif_decode::Image::Rgba8(img) => {
            let (w, h) = (img.width() as u32, img.height() as u32);
            let mut data = vec![0u8; (w as usize) * (h as usize) * 4];
            for (i, p) in img.buf().iter().enumerate() {
                let Rgba { r, g, b, a } = *p;
                let o = i * 4;
                data[o..o + 4].copy_from_slice(&[r, g, b, a]);
            }
            (w, h, data)
        }
        avif_decode::Image::Gray8(img) => {
            let (w, h) = (img.width() as u32, img.height() as u32);
            let mut data = vec![0u8; (w as usize) * (h as usize) * 4];
            for (i, p) in img.buf().iter().enumerate() {
                let v = (*p).value();
                let o = i * 4;
                data[o..o + 4].copy_from_slice(&[v, v, v, 255]);
            }
            (w, h, data)
        }
        // ── 16-bit：取高字节 ──
        avif_decode::Image::Rgb16(img) => {
            let (w, h) = (img.width() as u32, img.height() as u32);
            let mut data = vec![0u8; (w as usize) * (h as usize) * 4];
            for (i, p) in img.buf().iter().enumerate() {
                let Rgb { r, g, b } = *p;
                let o = i * 4;
                data[o..o + 4].copy_from_slice(&[
                    (r >> 8) as u8,
                    (g >> 8) as u8,
                    (b >> 8) as u8,
                    255,
                ]);
            }
            (w, h, data)
        }
        avif_decode::Image::Rgba16(img) => {
            let (w, h) = (img.width() as u32, img.height() as u32);
            let mut data = vec![0u8; (w as usize) * (h as usize) * 4];
            for (i, p) in img.buf().iter().enumerate() {
                let Rgba { r, g, b, a } = *p;
                let o = i * 4;
                data[o..o + 4].copy_from_slice(&[
                    (r >> 8) as u8,
                    (g >> 8) as u8,
                    (b >> 8) as u8,
                    (a >> 8) as u8,
                ]);
            }
            (w, h, data)
        }
        avif_decode::Image::Gray16(img) => {
            let (w, h) = (img.width() as u32, img.height() as u32);
            let mut data = vec![0u8; (w as usize) * (h as usize) * 4];
            for (i, p) in img.buf().iter().enumerate() {
                let v = ((*p).value() >> 8) as u8;
                let o = i * 4;
                data[o..o + 4].copy_from_slice(&[v, v, v, 255]);
            }
            (w, h, data)
        }
    };

    let mut img = RgbaImage::new(w, h)?;
    img.data = data;
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::DecodePurpose;

    /// 取仓库 `test-image/` 下的真实样本（小于 4KB 视为 Git LFS 指针）
    fn sample(name: &str) -> Option<std::path::PathBuf> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-image");
        let p = dir.join(name);
        if p.is_file() && std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0) > 4096 {
            Some(p)
        } else {
            None
        }
    }

    #[test]
    fn 真实AVIF探测尺寸() {
        let Some(p) = sample("10007.avif") else {
            eprintln!("跳过：找不到 10007.avif 样本");
            return;
        };
        let info = probe_avif(&p).unwrap();
        assert!(info.width > 0 && info.height > 0, "尺寸应大于 0，实际 {}×{}", info.width, info.height);
        assert_eq!(info.format, ImageFormat::Avif);
        eprintln!("10007.avif 尺寸 = {}×{}", info.width, info.height);
    }

    #[test]
    fn 真实AVIF解码为RGBA() {
        let Some(p) = sample("10007.avif") else {
            eprintln!("跳过：找不到 10007.avif 样本");
            return;
        };
        let img = decode_avif(&p, DecodeTarget::new(DecodePurpose::Export)).unwrap();
        assert!(img.width > 0 && img.height > 0);
        assert_eq!(img.data.len(), (img.width as usize) * (img.height as usize) * 4);
        // 不能整幅同色（解码失败常表现如此）
        let distinct = img
            .data
            .chunks_exact(4)
            .step_by(997)
            .map(|px| (px[0], px[1], px[2]))
            .collect::<std::collections::HashSet<_>>();
        assert!(distinct.len() > 5, "采样颜色过少（{}），疑似解码异常", distinct.len());
    }

    #[test]
    fn AVIF按目标尺寸缩放() {
        let Some(p) = sample("10007.avif") else { return };
        let img = decode_avif(&p, DecodeTarget::new(DecodePurpose::Thumbnail)).unwrap();
        assert_eq!(img.width.max(img.height), 256);
    }

    #[test]
    fn 非AVIF文件报错而不是panic() {
        let dir = std::env::temp_dir().join("pv_avif_bad");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("not.avif");
        std::fs::write(&p, b"this is definitely not an avif file").unwrap();
        assert!(decode_avif(&p, DecodeTarget::new(DecodePurpose::Preview)).is_err());
        let _ = std::fs::remove_file(&p);
    }
}
