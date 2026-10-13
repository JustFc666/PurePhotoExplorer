//! TIFF 解码（基线 TIFF，image-rs 官方纯 Rust `tiff` crate）。
//!
//! ## 为什么用 `tiff` 而不是手写
//! TIFF 的压缩/光度组合太杂（LZW / Deflate / PackBits、调色板、16-bit、
//! 分平面、horizontal predictor…），手写 LZW 极易出 bug。本项目已用 image-rs
//! 的 `png` / `gif` / `image-webp`（都是纯 Rust、无 C 构建），`tiff` 同门，
//! 直接复用最稳。
//!
//! `tiff` 的 `read_image()` 返回**扁平字节** + `ColorType`（`U8(Vec<u8>)` /
//! `U16(Vec<u16>)`），由本模块负责转成我们的 RGBA。
//!
//! 支持：Gray / RGB / GrayA / RGBA（8/16 位）；压缩 1/5/8/32773 由 `tiff` 处理。
//! 不支持：调色板 / CMYK / YCbCr（返回明确错误，不会静默出乱图）。

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use pv_core::error::{PvError, Result};
use pv_core::format::ImageFormat;
use tiff::decoder::{Decoder, DecodingResult};
use tiff::ColorType;

use crate::decoder::{fit_within, DecodeTarget, ImageInfo, RgbaImage};
use super::resize::resize_rgba;

/// 探测 TIFF 尺寸（只读 IFD，不解码像素，O(1)）。
///
/// `pub(crate)`：注册表（`decoder/registry.rs`）在 RAW 管线对 TIFF 容器
/// 失败时，会把它路由到这里。
pub(crate) fn probe_tiff(path: &Path) -> Result<ImageInfo> {
    let file = File::open(path).map_err(crate::io_err(path))?;
    let mut dec = Decoder::new(BufReader::new(file))
        .map_err(|e| PvError::Decode(format!("TIFF 打开失败：{e}")))?;
    let (w, h) = dec
        .dimensions()
        .map_err(|e| PvError::Decode(format!("TIFF 尺寸解析失败：{e}")))?;
    Ok(ImageInfo {
        format: ImageFormat::Tiff,
        width: w,
        height: h,
        exif: None,
        raw: None,
        color_space: Some("TIFF".to_string()),
    })
}

/// 解码 TIFF 为 RGBA（按 `target` 缩放）。
///
/// `pub(crate)`：注册表（`decoder/registry.rs`）在 RAW 管线对 TIFF 容器
/// 失败时，会把它路由到这里。
pub(crate) fn decode_tiff(path: &Path, target: DecodeTarget) -> Result<RgbaImage> {
    let file = File::open(path).map_err(crate::io_err(path))?;
    let mut dec = Decoder::new(BufReader::new(file))
        .map_err(|e| PvError::Decode(format!("TIFF 打开失败：{e}")))?;
    let (w, h) = dec
        .dimensions()
        .map_err(|e| PvError::Decode(format!("TIFF 尺寸解析失败：{e}")))?;
    let color = dec
        .colortype()
        .map_err(|e| PvError::Decode(format!("TIFF 色彩类型解析失败：{e}")))?;
    let result = dec
        .read_image()
        .map_err(|e| {
            PvError::Decode(format!(
                "TIFF 像素解码失败（可能用了本程序未支持的压缩/色彩，如 JPEG 压缩、调色板、CMYK、YCbCr）：{e}"
            ))
        })?;

    let full = bytes_to_rgba(&result, color, w, h)?;

    let (tw, th) = fit_within(w, h, target.max_edge);
    if (tw, th) == (w, h) {
        Ok(full)
    } else {
        resize_rgba(&full, tw, th)
    }
}

/// 把 `tiff` 的扁平像素缓冲 + `ColorType` 转成我们的 RGBA。
fn bytes_to_rgba(
    buf: &DecodingResult,
    color: ColorType,
    w: u32,
    h: u32,
) -> Result<RgbaImage> {
    let mut img = RgbaImage::new(w, h)?;
    let n = (w as usize) * (h as usize);

    match (color, buf) {
        // ── 8 位 ──
        (ColorType::Gray(8), DecodingResult::U8(d)) => {
            for i in 0..n {
                let g = d[i];
                img.data[i * 4..i * 4 + 4].copy_from_slice(&[g, g, g, 255]);
            }
        }
        (ColorType::GrayA(8), DecodingResult::U8(d)) => {
            for i in 0..n {
                let g = d[i * 2];
                let a = d[i * 2 + 1];
                img.data[i * 4..i * 4 + 4].copy_from_slice(&[g, g, g, a]);
            }
        }
        (ColorType::RGB(8), DecodingResult::U8(d)) => {
            for i in 0..n {
                let o = i * 4;
                let s = i * 3;
                img.data[o..o + 3].copy_from_slice(&[d[s], d[s + 1], d[s + 2]]);
                img.data[o + 3] = 255;
            }
        }
        (ColorType::RGBA(8), DecodingResult::U8(d)) => {
            img.data.copy_from_slice(d);
        }
        // ── 16 位：取高字节（与 HEIC 10-bit → 8-bit 同策略）──
        (ColorType::Gray(16), DecodingResult::U16(d)) => {
            for i in 0..n {
                let g = (d[i] >> 8) as u8;
                img.data[i * 4..i * 4 + 4].copy_from_slice(&[g, g, g, 255]);
            }
        }
        (ColorType::GrayA(16), DecodingResult::U16(d)) => {
            for i in 0..n {
                let g = (d[i * 2] >> 8) as u8;
                let a = (d[i * 2 + 1] >> 8) as u8;
                img.data[i * 4..i * 4 + 4].copy_from_slice(&[g, g, g, a]);
            }
        }
        (ColorType::RGB(16), DecodingResult::U16(d)) => {
            for i in 0..n {
                let o = i * 4;
                let s = i * 3;
                img.data[o] = (d[s] >> 8) as u8;
                img.data[o + 1] = (d[s + 1] >> 8) as u8;
                img.data[o + 2] = (d[s + 2] >> 8) as u8;
                img.data[o + 3] = 255;
            }
        }
        (ColorType::RGBA(16), DecodingResult::U16(d)) => {
            for i in 0..n {
                let o = i * 4;
                let s = i * 4;
                img.data[o] = (d[s] >> 8) as u8;
                img.data[o + 1] = (d[s + 1] >> 8) as u8;
                img.data[o + 2] = (d[s + 2] >> 8) as u8;
                img.data[o + 3] = (d[s + 3] >> 8) as u8;
            }
        }
        // ── 暂不支持的色彩 ──
        (ColorType::Palette(_), _) => {
            return Err(PvError::UnsupportedFormat(
                "TIFF 调色板（Palette）模式暂不支持".into(),
            ));
        }
        (ColorType::CMYK(_), _) => {
            return Err(PvError::UnsupportedFormat(
                "TIFF CMYK 模式暂不支持".into(),
            ));
        }
        (ColorType::YCbCr(_), _) => {
            return Err(PvError::UnsupportedFormat(
                "TIFF YCbCr 色彩暂不支持（需做色度转换）".into(),
            ));
        }
        // 其它位深 / 数据类型不匹配
        _ => {
            return Err(PvError::UnsupportedFormat(format!(
                "暂不支持的 TIFF 像素布局：色彩 {color:?} / 数据 {:?}",
                buf_variant(buf)
            )));
        }
    }
    Ok(img)
}

fn buf_variant(buf: &DecodingResult) -> &'static str {
    match buf {
        DecodingResult::U8(_) => "U8",
        DecodingResult::U16(_) => "U16",
        DecodingResult::U32(_) => "U32",
        DecodingResult::U64(_) => "U64",
        DecodingResult::F32(_) => "F32",
        DecodingResult::F64(_) => "F64",
        DecodingResult::I8(_) => "I8",
        DecodingResult::I16(_) => "I16",
        DecodingResult::I32(_) => "I32",
        DecodingResult::I64(_) => "I64",
    }
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
    fn 真实TIFF_uncompressed探测尺寸() {
        let Some(p) = sample("door.tif") else {
            eprintln!("跳过：找不到 door.tif 样本");
            return;
        };
        let info = probe_tiff(&p).unwrap();
        assert_eq!((info.width, info.height), (750, 1500));
        assert_eq!(info.format, ImageFormat::Tiff);
    }

    #[test]
    fn 真实TIFF_uncompressed解码() {
        let Some(p) = sample("door.tif") else {
            eprintln!("跳过：找不到 door.tif 样本");
            return;
        };
        let img = decode_tiff(&p, DecodeTarget::new(DecodePurpose::Export)).unwrap();
        assert_eq!((img.width, img.height), (750, 1500));
        assert_eq!(img.data.len(), 750 * 1500 * 4);
        // 不能整幅同色（解码失败常表现如此）
        let distinct = img.data.chunks_exact(4).step_by(7001).map(|px| (px[0], px[1], px[2])).collect::<std::collections::HashSet<_>>();
        assert!(distinct.len() > 10, "采样颜色过少（{}），疑似解码异常", distinct.len());
    }

    #[test]
    fn 真实TIFF_LZW探测与解码() {
        let Some(p) = sample("rgb.tiff") else {
            eprintln!("跳过：找不到 rgb.tiff 样本");
            return;
        };
        let info = probe_tiff(&p).unwrap();
        assert_eq!((info.width, info.height), (800, 519));
        let img = decode_tiff(&p, DecodeTarget::new(DecodePurpose::Export)).unwrap();
        assert_eq!((img.width, img.height), (800, 519));
        assert_eq!(img.data.len(), 800 * 519 * 4);
        // 左上角像素应与原图一致（LZW 无损）
        let [r, g, b, a] = img.pixel(0, 0).unwrap();
        assert_eq!(a, 255);
        eprintln!("rgb.tiff (0,0) = ({r},{g},{b})");
    }

    #[test]
    fn TIFF按目标尺寸缩放() {
        let Some(p) = sample("rgb.tiff") else { return };
        let img = decode_tiff(&p, DecodeTarget::new(DecodePurpose::Thumbnail)).unwrap();
        assert_eq!(img.width.max(img.height), 256);
    }

    #[test]
    fn 非TIFF文件报错而不是panic() {
        let dir = std::env::temp_dir().join("pv_tiff_bad");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("not.tiff");
        std::fs::write(&p, b"this is definitely not a tiff file").unwrap();
        assert!(decode_tiff(&p, DecodeTarget::new(DecodePurpose::Preview)).is_err());
        let _ = std::fs::remove_file(&p);
    }
}
