//! 格式判定与元信息探测（不解码像素）。
//!
//! JPEG 尺寸靠解析 SOFn 标记（O(1)），PNG 尺寸靠 `read_info()`。

use std::path::Path;

use pv_core::error::{PvError, Result};
use pv_core::format::{probe_header, ImageFormat};

use super::io_err;
use crate::decoder::ImageInfo;

/// 读文件头判定真实格式（不信任扩展名，FR-1.5）
pub(super) fn probe_format(path: &Path) -> Result<ImageFormat> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(io_err(path))?;
    let mut head = [0u8; 32];
    let n = f.read(&mut head).map_err(io_err(path))?;
    if n == 0 {
        return Err(PvError::Decode("文件为空".into()));
    }
    Ok(probe_header(&head[..n]).format)
}

/**
 * 探测 JPEG 尺寸。
 *
 * **重要**：`jpeg_decoder` 的 `Decoder::info()` 在调用 `decode()` 之前
 * 会返回 `None`（元信息在解码后才填充），所以不能靠它探测。
 *
 * 这里直接解析 JPEG 的 SOFn 标记拿尺寸 —— O(1)，不解码像素。
 * 这是唯一能在不解码前提下拿到尺寸的方式。
 */
pub(super) fn probe_jpeg(path: &Path) -> Result<ImageInfo> {
    match read_jpeg_dims(path) {
        Some((w, h)) => Ok(ImageInfo {
            format: ImageFormat::Jpeg,
            width: w,
            height: h,
            exif: read_jpeg_exif(path),
            raw: None,
            color_space: Some("YCbCr (JPEG)".to_string()),
        }),
        None => Err(PvError::Decode(
            "无法解析 JPEG 尺寸，文件可能已损坏或不是 JPEG".into(),
        )),
    }
}

/// 解析 JPEG 的 SOF0–SOF15 标记，取出图像尺寸。
///
/// 遍历 JPEG 段：每个段以 `FF xx` + 2 字节长度开头，
/// 长度包含长度字段本身但不含标记。
pub(super) fn read_jpeg_dims(path: &Path) -> Option<(u32, u32)> {
    use std::io::Read;
    let f = std::fs::File::open(path).ok()?;
    let mut r = std::io::BufReader::with_capacity(64 * 1024, f);

    let mut b = [0u8; 2];
    // 跳过 SOI (FFD8)
    r.read_exact(&mut b).ok()?;

    loop {
        let mut marker = [0u8; 2];
        if r.read_exact(&mut marker).is_err() {
            return None;
        }
        // 跳过填充字节与 RSTn 标记
        if marker[0] != 0xFF {
            continue;
        }
        let m = marker[1];
        // 无长度字段的标记
        if m == 0xD8 || m == 0x01 || (0xD0..=0xD7).contains(&m) {
            continue;
        }
        // 到达图像数据
        if m == 0xDA || m == 0xD9 {
            return None;
        }
        // 软复位 SOF
        if m == 0xFF {
            continue;
        }

        let mut lenbuf = [0u8; 2];
        if r.read_exact(&mut lenbuf).is_err() {
            return None;
        }
        let seg_len = u16::from_be_bytes(lenbuf) as usize;
        if seg_len < 2 {
            return None;
        }

        // SOF0..SOF15，排除 DHT(C4) / JPG(C8) / DAC(CC)
        let is_sof = matches!(m, 0xC0..=0xCF) && m != 0xC4 && m != 0xC8 && m != 0xCC;

        if is_sof {
            // SOFn 结构：长度(2) + 精度(1) + 高(2) + 宽(2) + 分量数(1)
            let mut sof = [0u8; 6];
            if r.read_exact(&mut sof).is_err() {
                return None;
            }
            let height = u16::from_be_bytes([sof[1], sof[2]]) as u32;
            let width = u16::from_be_bytes([sof[3], sof[4]]) as u32;
            if width > 0 && height > 0 {
                return Some((width, height));
            }
            return None;
        }

        // 跳过本段剩余内容
        let mut skip = [0u8; 4096];
        let mut remaining = seg_len - 2;
        while remaining > 0 {
            let n = remaining.min(skip.len());
            if r.read_exact(&mut skip[..n]).is_err() {
                return None;
            }
            remaining -= n;
        }
    }
}

/// 提取 JPEG 的 EXIF 文本（APP1 段）
///
/// S1 阶段只做原始字符串保留，S2 再解析成结构化字段。
fn read_jpeg_exif(path: &Path) -> Option<String> {
    use std::io::Read;
    let f = std::fs::File::open(path).ok()?;
    let mut r = std::io::BufReader::with_capacity(64 * 1024, f);

    let mut b = [0u8; 2];
    r.read_exact(&mut b).ok()?; // SOI

    // 只扫描前若干段，EXIF 通常在最前面
    for _ in 0..8 {
        let mut marker = [0u8; 2];
        if r.read_exact(&mut marker).is_err() {
            return None;
        }
        if marker[0] != 0xFF {
            continue;
        }
        let m = marker[1];
        if m == 0xD8 || m == 0xDA {
            return None;
        }

        let mut lenbuf = [0u8; 2];
        if r.read_exact(&mut lenbuf).is_err() {
            return None;
        }
        let seg_len = u16::from_be_bytes(lenbuf) as usize;
        if seg_len < 2 {
            return None;
        }

        if m == 0xE1 {
            // APP1：Exif\0\0 或 XMP
            let mut head = [0u8; 6];
            if r.read_exact(&mut head).is_err() {
                return None;
            }
            if &head[..4] == b"Exif" {
                let rest = seg_len - 2 - 6;
                let mut buf = vec![0u8; rest.min(64 * 1024)];
                if r.read_exact(&mut buf).is_ok() {
                    return Some(String::from_utf8_lossy(&buf).chars().take(2048).collect());
                }
            }
            return None;
        }

        let mut skip = [0u8; 4096];
        let mut remaining = seg_len - 2;
        while remaining > 0 {
            let n = remaining.min(skip.len());
            if r.read_exact(&mut skip[..n]).is_err() {
                return None;
            }
            remaining -= n;
        }
    }
    None
}

pub(super) fn probe_png(path: &Path) -> Result<ImageInfo> {
    let f = std::fs::File::open(path).map_err(io_err(path))?;
    let decoder = png::Decoder::new(f);
    let reader = decoder
        .read_info()
        .map_err(|e| PvError::Decode(format!("PNG 头解析失败: {e}")))?;
    let info = reader.info();
    Ok(ImageInfo {
        format: ImageFormat::Png,
        width: info.width,
        height: info.height,
        exif: None,
        raw: None,
        color_space: Some(format!("{:?}", info.color_type)),
    })
}
