//! 各色彩空间 → RGBA 转换。
//!
//! RGB→RGBA 走 rayon 按行分块并行（22 MB 图会产生 68 MB 数据，纯内存带宽瓶颈）。

use rayon::prelude::*;

use pv_core::error::{PvError, Result};

use crate::decoder::RgbaImage;

/// RGB24 → RGBA（按行分块并行）
///
/// 用 `chunks_exact_mut` 做安全切片，不使用裸指针。
pub(super) fn rgb24_to_rgba(src: &[u8], w: u32, h: u32) -> Result<RgbaImage> {
    let n = (w as usize) * (h as usize);
    if src.len() < n * 3 {
        return Err(PvError::Decode("RGB 数据长度不足，文件可能截断".into()));
    }
    let mut out = RgbaImage::new(w, h)?;

    const CHUNK_PX: usize = 4096; // 每块 4096 像素 ≈ 16 KB 源数据

    // 源数据按像素三元组切块，目标数据按像素四元组切块
    out.data
        .par_chunks_mut(CHUNK_PX * 4)
        .zip(src.par_chunks(CHUNK_PX * 3))
        .for_each(|(dst, src_chunk)| {
            for (i, px) in dst.chunks_exact_mut(4).enumerate() {
                let s = &src_chunk[i * 3..i * 3 + 3];
                px[0] = s[0];
                px[1] = s[1];
                px[2] = s[2];
                px[3] = 255;
            }
        });

    Ok(out)
}

pub(super) fn gray8_to_rgba(src: &[u8], w: u32, h: u32) -> Result<RgbaImage> {
    if src.len() < (w as usize) * (h as usize) {
        return Err(PvError::Decode("灰度数据长度不足".into()));
    }
    let mut out = RgbaImage::new(w, h)?;
    for i in 0..(w as usize) * (h as usize) {
        let g = src[i];
        out.data[i * 4] = g;
        out.data[i * 4 + 1] = g;
        out.data[i * 4 + 2] = g;
        out.data[i * 4 + 3] = 255;
    }
    Ok(out)
}

pub(super) fn gray_alpha_to_rgba(src: &[u8], w: u32, h: u32) -> Result<RgbaImage> {
    let n = (w as usize) * (h as usize);
    if src.len() < n * 2 {
        return Err(PvError::Decode("灰度带 Alpha 数据长度不足".into()));
    }
    let mut out = RgbaImage::new(w, h)?;
    for i in 0..n {
        out.data[i * 4] = src[i * 2];
        out.data[i * 4 + 1] = src[i * 2];
        out.data[i * 4 + 2] = src[i * 2];
        out.data[i * 4 + 3] = src[i * 2 + 1];
    }
    Ok(out)
}

/// CMYK32 → RGBA
///
/// 一期未接入（zune-jpeg 现阶段输出 RGB），保留供后续 CMYK JPEG 支持。
/// 按标准公式转换：`R = (255 - C) * (255 - K) / 255`。
///
/// 注：带 Adobe APP14 标记的 CMYK JPEG 实际是反序存储的，
/// 此时青↔品红会互换。CMYK JPEG 极少见，待收集到真实样本后再校正。
#[allow(dead_code)]
pub(super) fn cmyk32_to_rgba(src: &[u8], w: u32, h: u32) -> Result<RgbaImage> {
    let n = (w as usize) * (h as usize);
    if src.len() < n * 4 {
        return Err(PvError::Decode("CMYK 数据长度不足".into()));
    }
    let mut out = RgbaImage::new(w, h)?;
    for i in 0..n {
        let c = src[i * 4] as u32;
        let m = src[i * 4 + 1] as u32;
        let y = src[i * 4 + 2] as u32;
        let k = src[i * 4 + 3] as u32;
        // K 参与乘法，K=255 时为纯黑
        let kf = 255 - k;
        out.data[i * 4] = ((255 - c) * kf / 255) as u8;
        out.data[i * 4 + 1] = ((255 - m) * kf / 255) as u8;
        out.data[i * 4 + 2] = ((255 - y) * kf / 255) as u8;
        out.data[i * 4 + 3] = 255;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn RGB转RGBA正确填充Alpha() {
        let rgb = vec![255u8, 0, 0, 0, 255, 0, 0, 0, 255];
        let out = rgb24_to_rgba(&rgb, 3, 1).unwrap();
        assert_eq!(out.pixel(0, 0), Some([255, 0, 0, 255]), "红");
        assert_eq!(out.pixel(1, 0), Some([0, 255, 0, 255]), "绿");
        assert_eq!(out.pixel(2, 0), Some([0, 0, 255, 255]), "蓝");
    }

    #[test]
    fn 数据长度不足时返回Err不panic() {
        let short = vec![0u8; 5];
        assert!(rgb24_to_rgba(&short, 10, 10).is_err());
        assert!(gray8_to_rgba(&short, 10, 10).is_err());
        assert!(cmyk32_to_rgba(&short, 10, 10).is_err());
    }

    #[test]
    fn 灰度转RGBA三通道相等() {
        let gray = vec![128u8; 4];
        let out = gray8_to_rgba(&gray, 2, 2).unwrap();
        assert_eq!(out.pixel(0, 0), Some([128, 128, 128, 255]));
    }

    #[test]
    fn CMYK无墨时为纯白() {
        let cmyk = vec![0u8, 0, 0, 0];
        let out = cmyk32_to_rgba(&cmyk, 1, 1).unwrap();
        assert_eq!(out.pixel(0, 0), Some([255, 255, 255, 255]));
    }

    #[test]
    fn CMYK满墨时为纯黑() {
        let cmyk = vec![255u8, 255, 255, 255];
        let out = cmyk32_to_rgba(&cmyk, 1, 1).unwrap();
        assert_eq!(out.pixel(0, 0), Some([0, 0, 0, 255]));
    }

    #[test]
    fn CMYK纯青应映射到蓝绿通道() {
        // C=255, M=Y=K=0 → R=0, G=255, B=255（青）
        let cmyk = vec![255u8, 0, 0, 0];
        let out = cmyk32_to_rgba(&cmyk, 1, 1).unwrap();
        assert_eq!(out.pixel(0, 0), Some([0, 255, 255, 255]));
    }
}
