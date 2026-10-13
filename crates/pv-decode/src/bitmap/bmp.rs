//! BMP 解码（**纯 Rust 手写，无第三方依赖**）。
//!
//! ## 支持范围
//! | 位深 | 压缩 | 说明 |
//! |---|---|---|
//! | 1 / 4 / 8 | BI_RGB | 调色板索引（未压缩）|
//! | 1 / 4 | BI_RLE4 / BI_RLE8 | 游程编码（Windows 老截图常见）|
//! | 16 | BI_RGB（默认 565）/ BI_BITFIELDS（显式掩码）| 带 shift 的 RGB |
//! | 24 | BI_RGB | 最通用的「真彩色」BGR |
//! | 32 | BI_RGB（BGRA，alpha 忽略）/ BI_BITFIELDS（含 alpha 掩码）| |
//!
//! ## 明确不支持（给清晰错误，不猜）
//! - `BI_JPEG` / `BI_PNG`（内嵌压缩流）—— 需要额外解码器；
//! - OS/2 1.x 旧头（`BITMAPCOREHEADER` 之外的 BA/CI/CP/IC/PT 变体仅作识别，
//!   实际解码仍按 BITMAPINFOHEADER 家族）。
//!
//! ## 行序
//! BMP 默认**自下而上**（`height > 0`，文件首行 = 图像底行）；`height < 0` 表示
//! **自上而下**。输出统一**翻转为顶行在前**，与 PNG 解码器 / `RgbaImage::pixel`
//! 的「y = 0 在顶」约定一致。

use std::path::Path;

use pv_core::error::{PvError, Result};
use pv_core::format::ImageFormat;

use super::io_err;
use super::resize::resize_rgba;
use crate::decoder::{fit_within, DecodeTarget, ImageInfo, RgbaImage};

/// BMP 文件头魔数家族（首两字节）。最常见是 `BM`，其余为 OS/2 图标/光标变种。
const BMP_MAGICS: &[&[u8]] = &[b"BM", b"BA", b"CI", b"CP", b"IC", b"PT"];

/// BI_RGB：无压缩、无掩码。
const BI_RGB: u32 = 0;
/// BI_RLE8：8 位调色板游程编码。
const BI_RLE8: u32 = 1;
/// BI_RLE4：4 位调色板游程编码。
const BI_RLE4: u32 = 2;
/// BI_BITFIELDS：16/32 位，RGB(A) 掩码紧跟 DIB 头。
const BI_BITFIELDS: u32 = 3;
/// BI_ALPHABITFIELDS：同 BITFIELDS 但额外带 alpha 掩码。
const BI_ALPHABITFIELDS: u32 = 6;

/// 解析后的 BMP 头（不含像素数据，像素按需从原缓冲切片）。
struct BmpHeader {
    width: u32,
    height: u32,
    bpp: u16,
    compression: u32,
    /// 自上而下（height<0）还是自下而上（height>0）。
    top_down: bool,
    /// 每行字节数（含 4 字节对齐填充）。
    stride: usize,
    /// 像素数据起始偏移（文件头里的 `bfOffBits`）。
    data_offset: usize,
    /// 1/4/8 位调色板（RGBA）。空表示无调色板。
    palette: Vec<[u8; 4]>,
    /// 16/32 位通道掩码（BI_RGB 时填默认）。
    r_mask: u32,
    g_mask: u32,
    b_mask: u32,
    a_mask: u32,
}

impl BmpHeader {
    fn is_rle(&self) -> bool {
        self.compression == BI_RLE8 || self.compression == BI_RLE4
    }
}

/// 解码入口（供 `BitmapBackend::decode` 调用）。
pub(super) fn decode_bmp(path: &Path, target: DecodeTarget) -> Result<RgbaImage> {
    let bytes = std::fs::read(path).map_err(io_err(path))?;
    let header = parse_bmp(&bytes)?;
    let rgba = to_rgba(&header, &bytes[header.data_offset..])?;

    // 非全尺寸请求（预览/缩略图）时按需等比缩放
    if !target.is_full_res() {
        if let Some(max) = target.max_edge {
            let (tw, th) = fit_within(rgba.width, rgba.height, Some(max));
            if tw < rgba.width || th < rgba.height {
                return resize_rgba(&rgba, tw, th);
            }
        }
    }
    Ok(rgba)
}

/// 仅探测尺寸与位深（不解码像素，O(1)）。
pub(super) fn probe_bmp(path: &Path) -> Result<ImageInfo> {
    let bytes = std::fs::read(path).map_err(io_err(path))?;
    let header = parse_bmp(&bytes).map_err(|e| PvError::Decode(format!("BMP 头解析失败: {e}")))?;
    Ok(ImageInfo {
        format: ImageFormat::Bmp,
        width: header.width,
        height: header.height,
        exif: None,
        raw: None,
        color_space: Some(format!("BMP {}bpp", header.bpp)),
    })
}

// ── 字节读取小工具（越界返回 0，静默降级而非 panic）──

fn rd_u8(b: &[u8], off: usize) -> u8 {
    b.get(off).copied().unwrap_or(0)
}
fn rd_u16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([rd_u8(b, off), rd_u8(b, off + 1)])
}
fn rd_i32(b: &[u8], off: usize) -> i32 {
    i32::from_le_bytes([rd_u8(b, off), rd_u8(b, off + 1), rd_u8(b, off + 2), rd_u8(b, off + 3)])
}
fn rd_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([
        rd_u8(b, off),
        rd_u8(b, off + 1),
        rd_u8(b, off + 2),
        rd_u8(b, off + 3),
    ])
}

/// 解析文件头 + DIB 头 + 调色板。
fn parse_bmp(bytes: &[u8]) -> Result<BmpHeader> {
    if bytes.len() < 14 + 40 {
        return Err(PvError::Decode("文件过短，不是合法 BMP".into()));
    }
    // 文件头魔数
    let magic = &bytes[0..2];
    if !BMP_MAGICS.iter().any(|m| *m == magic) {
        return Err(PvError::Decode(format!(
            "不是 BMP 文件（魔数 {:?}）",
            &bytes[0..2.min(bytes.len())]
        )));
    }

    let data_offset = rd_u32(bytes, 10) as usize;
    let dib_size = rd_u32(bytes, 14) as usize;

    let (width, height, bpp, compression, top_down, palette_bytes_len, palette_entry_size);
    if dib_size == 12 {
        // BITMAPCOREHEADER：全是 u16，无压缩字段
        width = rd_u16(bytes, 18) as u32;
        height = rd_u16(bytes, 20) as u32;
        bpp = rd_u16(bytes, 24);
        compression = BI_RGB;
        top_down = false;
        palette_bytes_len = (14 + 12) as usize;
        palette_entry_size = 3; // CORE 调色板是 BGR 三字节
    } else if dib_size >= 40 {
        let raw_w = rd_i32(bytes, 18);
        let raw_h = rd_i32(bytes, 22);
        width = raw_w.unsigned_abs();
        if raw_h < 0 {
            height = raw_h.unsigned_abs();
            top_down = true;
        } else {
            height = raw_h as u32;
            top_down = false;
        }
        bpp = rd_u16(bytes, 28);
        compression = rd_u32(bytes, 30);
        palette_bytes_len = 14 + dib_size;
        palette_entry_size = 4; // INFO 及以后：BGRA（末字节保留）
    } else {
        return Err(PvError::Decode(format!(
            "不支持的 BMP 头尺寸 {dib_size}（仅支持 12 / ≥40 字节的 DIB 头）"
        )));
    }

    if width == 0 || height == 0 {
        return Err(PvError::Decode("BMP 尺寸为 0，可能已损坏".into()));
    }

    // 调色板
    let palette = if bpp <= 8 {
        let colors_used = if dib_size >= 40 { rd_u32(bytes, 46) } else { 0 };
        let count = if colors_used != 0 {
            colors_used as usize
        } else {
            1usize << bpp
        };
        read_palette(bytes, palette_bytes_len, palette_entry_size, count)
    } else {
        Vec::new()
    };

    // 16/32 位掩码
    let (r_mask, g_mask, b_mask, a_mask) = compute_masks(bytes, dib_size, bpp, compression);

    let stride = ((width as usize * bpp as usize + 31) / 32) * 4;

    Ok(BmpHeader {
        width,
        height,
        bpp,
        compression,
        top_down,
        stride,
        data_offset,
        palette,
        r_mask,
        g_mask,
        b_mask,
        a_mask,
    })
}

/// 读取调色板为 RGBA 条目。
fn read_palette(bytes: &[u8], start: usize, entry_size: usize, count: usize) -> Vec<[u8; 4]> {
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let o = start + i * entry_size;
        if o + (entry_size - 1) >= bytes.len() {
            break;
        }
        let b = rd_u8(bytes, o);
        let g = rd_u8(bytes, o + 1);
        let r = rd_u8(bytes, o + 2);
        out.push([r, g, b, 255]);
        if out.len() >= 256 {
            break; // 8 位最多 256 项
        }
    }
    out
}

/// 计算 16/32 位的 RGB(A) 掩码。
///
/// - `BI_BITFIELDS` / `BI_ALPHABITFIELDS`：掩码显式写在 DIB 头后（偏移 40/44/48/52）。
/// - `BI_RGB` 走默认：32 位 = BGRA（`alpha` 忽略），16 位 = 565（Windows 高彩色最常见）。
fn compute_masks(
    bytes: &[u8],
    dib_size: usize,
    bpp: u16,
    compression: u32,
) -> (u32, u32, u32, u32) {
    let mr = rd_u32(bytes, 14 + 40);
    let mg = rd_u32(bytes, 14 + 44);
    let mb = rd_u32(bytes, 14 + 48);
    let ma = rd_u32(bytes, 14 + 52);

    let explicit = compression == BI_BITFIELDS || compression == BI_ALPHABITFIELDS;
    let v4_or_later = dib_size >= 108;

    if (bpp == 16 || bpp == 32) && (explicit || (v4_or_later && (mr | mg | mb) != 0)) {
        let a = if compression == BI_ALPHABITFIELDS || v4_or_later {
            ma
        } else {
            0
        };
        (mr, mg, mb, a)
    } else if bpp == 32 {
        (0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0)
    } else if bpp == 16 {
        (0xF800, 0x07E0, 0x001F, 0)
    } else {
        (0, 0, 0, 0)
    }
}

/// 按 BMP 头把原始像素（文件顺序）转成 RGBA 图像。
fn to_rgba(header: &BmpHeader, pixel_data: &[u8]) -> Result<RgbaImage> {
    let w = header.width as usize;
    let h = header.height as usize;
    let mut out = RgbaImage::new(header.width, header.height)?;

    // RLE 先解压成「未压缩的索引像素」（1/4/8 位打包布局）
    let owned;
    let raw: &[u8] = if header.is_rle() {
        owned = decode_rle(pixel_data, w, h, header.bpp, header.stride)?;
        &owned
    } else {
        pixel_data
    };

    for out_y in 0..h {
        // 文件行序 → 输出行序（翻转右下/左上）
        let file_row = if header.top_down { out_y } else { h - 1 - out_y };
        let row_off = file_row * header.stride;
        for x in 0..w {
            let px = sample_at(header, raw, row_off, x);
            let o = (out_y * w + x) * 4;
            out.data[o..o + 4].copy_from_slice(&px);
        }
    }
    Ok(out)
}

/// 取某一像素的 RGBA（已处理位深与调色板）。
fn sample_at(h: &BmpHeader, raw: &[u8], row_off: usize, x: usize) -> [u8; 4] {
    match h.bpp {
        24 => {
            let o = row_off + x * 3;
            [
                rd_u8(raw, o + 2),
                rd_u8(raw, o + 1),
                rd_u8(raw, o),
                255,
            ]
        }
        32 => {
            let o = row_off + x * 4;
            let v = u32::from_le_bytes([rd_u8(raw, o), rd_u8(raw, o + 1), rd_u8(raw, o + 2), rd_u8(raw, o + 3)]);
            let r = chan(v, h.r_mask);
            let g = chan(v, h.g_mask);
            let b = chan(v, h.b_mask);
            let a = if h.a_mask != 0 { chan(v, h.a_mask) } else { 255 };
            [r, g, b, a]
        }
        16 => {
            let o = row_off + x * 2;
            let v = u16::from_le_bytes([rd_u8(raw, o), rd_u8(raw, o + 1)]) as u32;
            let r = chan(v, h.r_mask);
            let g = chan(v, h.g_mask);
            let b = chan(v, h.b_mask);
            let a = if h.a_mask != 0 { chan(v, h.a_mask) } else { 255 };
            [r, g, b, a]
        }
        8 => {
            let idx = rd_u8(raw, row_off + x) as usize;
            palette_lookup(&h.palette, idx)
        }
        4 => {
            let byte = rd_u8(raw, row_off + x / 2);
            let nib = if x % 2 == 0 { byte >> 4 } else { byte & 0x0F };
            palette_lookup(&h.palette, nib as usize)
        }
        1 => {
            let byte = rd_u8(raw, row_off + x / 8);
            let bit = 7 - (x % 8);
            let idx = ((byte >> bit) & 1) as usize;
            palette_lookup(&h.palette, idx)
        }
        _ => [0, 0, 0, 255],
    }
}

/// 从掩码提取单通道并缩放到 8 位。
fn chan(v: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let val = (v & mask) >> shift;
    let max = mask >> shift; // 该通道掩码全 1 时的值
    if max == 0 {
        return 0;
    }
    ((val * 255 + max / 2) / max) as u8
}

#[inline]
fn palette_lookup(palette: &[[u8; 4]], idx: usize) -> [u8; 4] {
    palette
        .get(idx % palette.len())
        .copied()
        .unwrap_or([0, 0, 0, 255])
}

/// 解压 RLE（BI_RLE8 / BI_RLE4）。
///
/// 输出为未压缩的索引像素缓冲（与同色深未压缩 BMP 的逐行打包布局一致），
/// `to_rgba` 之后照常走 `sample_at` 的 8/4 位分支。
fn decode_rle(data: &[u8], w: usize, h: usize, bpp: u16, stride: usize) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; h * stride];
    let mut pos = 0usize;
    let mut x = 0usize;
    let mut y = 0usize;

    while pos + 1 < data.len() {
        let a = data[pos];
        let b = data[pos + 1];
        pos += 2;

        if a == 0 {
            match b {
                0 => {
                    // 换行
                    x = 0;
                    if y < h {
                        y += 1;
                    }
                }
                1 => break, // 位图结束
                2 => {
                    // 偏移（delta）
                    if pos + 1 >= data.len() {
                        break;
                    }
                    x += data[pos] as usize;
                    y += data[pos + 1] as usize;
                    pos += 2;
                }
                _ => {
                    // 绝对模式：b 个像素（4 位时为单位数）紧跟其后
                    let count = b as usize;
                    if bpp == 4 {
                        let nbytes = count.div_ceil(2);
                        let mut padded = nbytes;
                        if padded % 2 == 1 {
                            padded += 1; // 字节对齐填充
                        }
                        for k in 0..count {
                            if pos >= data.len() {
                                break;
                            }
                            let byte = data[pos + k / 2];
                            let nib = if k % 2 == 0 { byte >> 4 } else { byte & 0x0F };
                            write_nibble(&mut buf, stride, x + k, y, nib);
                        }
                        pos += padded;
                    } else {
                        for k in 0..count {
                            if pos >= data.len() {
                                break;
                            }
                            if x + k < w && y < h {
                                buf[y * stride + x + k] = data[pos];
                            }
                            pos += 1;
                        }
                        if count % 2 == 1 {
                            pos += 1; // 奇数字节后补一字节对齐
                        }
                    }
                    // 写完本段像素后推进 x；换行由随后的 EOL(0,0) 负责，
                    // 这里**不要**自动换行——否则会与 EOL 重复 +1 行。
                    x += count;
                }
            }
        } else {
            // 编码模式：a 个像素，值由 b 给出（4 位时为两 nibble 交替）
            if bpp == 4 {
                let hi = b >> 4;
                let lo = b & 0x0F;
                for k in 0..a as usize {
                    write_nibble(&mut buf, stride, x + k, y, if k % 2 == 0 { hi } else { lo });
                }
            } else {
                for k in 0..a as usize {
                    if x + k < w && y < h {
                        buf[y * stride + x + k] = b;
                    }
                }
            }
            // 同绝对模式：只推进 x，换行交给 EOL。
            x += a as usize;
        }
    }
    Ok(buf)
}

/// 向 4 位打包缓冲写入一个 nibble（高位在前）。
fn write_nibble(buf: &mut [u8], stride: usize, x: usize, y: usize, nib: u8) {
    if x >= (stride * 2) || y * stride + x / 2 >= buf.len() {
        return;
    }
    let off = y * stride + x / 2;
    if x % 2 == 0 {
        buf[off] = (buf[off] & 0x0F) | (nib << 4);
    } else {
        buf[off] = (buf[off] & 0xF0) | (nib & 0x0F);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pv_core::format::probe_header;

    #[test]
    fn bmp魔数探测() {
        // "BM" 开头应被识别为 BMP（探测只看文件头，不信任扩展名）
        let head: [u8; 4] = [0x42, 0x4D, 0, 0]; // "BM" + 2 字节填充
        assert_eq!(probe_header(&head).format, ImageFormat::Bmp);
    }

    #[test]
    fn 拒绝非bmp() {
        let junk = [0u8, 1, 2, 3, 4, 5];
        assert!(parse_bmp(&junk).is_err());
    }

    #[test]
    fn 文件头过短报错() {
        assert!(decode_bmp_impl(&[0x42, 0x4D, 0x00]).is_err());
    }

    // 直接喂字节给解析（绕过文件读取，便于单测）
    fn decode_bmp_impl(bytes: &[u8]) -> Result<RgbaImage> {
        let h = parse_bmp(bytes)?;
        to_rgba(&h, &bytes[h.data_offset..])
    }

    // 以下用一组标准 BMP 字节构造，覆盖各色深/压缩。
    // 像素布局：左上红(255,0,0) 右上绿(0,255,0) 左下蓝(0,0,255) 右下白(255,255,255)。

    fn rgb_pattern_4x4() -> Vec<[u8; 3]> {
        let mut px = vec![[128u8, 128, 128]; 16];
        px[0] = [255, 0, 0];
        px[3] = [0, 255, 0];
        px[12] = [0, 0, 255];
        px[15] = [255, 255, 255];
        px
    }

    /// 手写构造 24 位 BMP（自下而上）。
    fn make_bmp_24() -> Vec<u8> {
        let px = rgb_pattern_4x4();
        // 每行 4*3=12 字节，补齐到 4 的倍数 → 12
        let _stride = 12usize;
        let mut pixel_bytes = Vec::new();
        // 自下而上：文件行序与 y 相反
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
        out.extend_from_slice(&[0u8; 4]); // reserved
        out.extend_from_slice(&(pixel_offset as u32).to_le_bytes());
        out.extend_from_slice(&dib_size.to_le_bytes());
        out.extend_from_slice(&(4i32).to_le_bytes()); // width
        out.extend_from_slice(&(4i32).to_le_bytes()); // height (正=自下而上)
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&24u16.to_le_bytes()); // bpp
        out.extend_from_slice(&0u32.to_le_bytes()); // compression BI_RGB
        out.extend_from_slice(&(pixel_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&[0u8; 8]); // x/y ppm
        out.extend_from_slice(&0u32.to_le_bytes()); // colors used
        out.extend_from_slice(&0u32.to_le_bytes()); // important
        out.extend_from_slice(&pixel_bytes);
        out
    }

    #[test]
    fn 解码24位bmp() {
        let bytes = make_bmp_24();
        let img = decode_bmp_impl(&bytes).unwrap();
        assert_eq!((img.width, img.height), (4, 4));
        // 文件自下而上 → 输出 (0,0)=红
        assert_eq!(img.pixel(0, 0), Some([255, 0, 0, 255]), "左上应为红");
        assert_eq!(img.pixel(3, 0), Some([0, 255, 0, 255]), "右上应为绿");
        assert_eq!(img.pixel(0, 3), Some([0, 0, 255, 255]), "左下应为蓝");
        assert_eq!(img.pixel(3, 3), Some([255, 255, 255, 255]), "右下应为白");
        assert_eq!(img.pixel(1, 1), Some([128, 128, 128, 255]), "中间应为灰");
    }

    /// 32 位 BGRA（BI_RGB，alpha 忽略）。
    fn make_bmp_32() -> Vec<u8> {
        let px = rgb_pattern_4x4();
        let _stride = 16usize; // 4*4
        let mut pixel_bytes = Vec::new();
        for y in (0..4).rev() {
            for x in 0..4 {
                let [r, g, b] = px[y * 4 + x];
                pixel_bytes.extend_from_slice(&[b, g, r, 255]);
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
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(pixel_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&[0u8; 8]);
        out.extend_from_slice(&0u32.to_le_bytes()); // colors used
        out.extend_from_slice(&0u32.to_le_bytes()); // important
        out.extend_from_slice(&pixel_bytes);
        out
    }

    #[test]
    fn 解码32位bmp() {
        let bytes = make_bmp_32();
        let img = decode_bmp_impl(&bytes).unwrap();
        assert_eq!(img.pixel(0, 0), Some([255, 0, 0, 255]), "32 位左上应为红（alpha 忽略）");
        assert_eq!(img.pixel(3, 3), Some([255, 255, 255, 255]), "右下应为白");
    }

    /// 8 位调色板 BMP。构造调色板：0=黑,1=红,2=绿,3=蓝，外加 4=白、5=灰。
    fn make_bmp_8pal() -> Vec<u8> {
        // 调色板 6 项（实际用 2^8=256 上限，但只填前几项，其余黑）
        let mut palette = vec![[0u8, 0, 0, 255]; 256];
        palette[0] = [0, 0, 0, 255];
        palette[1] = [255, 0, 0, 255];
        palette[2] = [0, 255, 0, 255];
        palette[3] = [0, 0, 255, 255];
        palette[4] = [255, 255, 255, 255];
        palette[5] = [128, 128, 128, 255];
        // 索引图（4x4）：左上1 右上2 左下3 右下4 其余5
        let mut indices = vec![5u8; 16];
        indices[0] = 1;
        indices[3] = 2;
        indices[12] = 3;
        indices[15] = 4;
        let _stride = 4usize;
        let mut pixel_bytes = Vec::new();
        for y in (0..4).rev() {
            for x in 0..4 {
                pixel_bytes.push(indices[y * 4 + x]);
            }
        }
        let dib_size = 40u32;
        let palette_offset = 14 + dib_size;
        let pixel_offset = palette_offset + 256 * 4;
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
        out.extend_from_slice(&8u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(pixel_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&[0u8; 8]);
        out.extend_from_slice(&256u32.to_le_bytes()); // colors used
        out.extend_from_slice(&0u32.to_le_bytes());
        for p in &palette {
            out.extend_from_slice(&[p[2], p[1], p[0], p[3]]); // BGRX
        }
        out.extend_from_slice(&pixel_bytes);
        out
    }

    #[test]
    fn 解码8位调色板bmp() {
        let bytes = make_bmp_8pal();
        let img = decode_bmp_impl(&bytes).unwrap();
        assert_eq!(img.pixel(0, 0), Some([255, 0, 0, 255]), "调色板索引1=红");
        assert_eq!(img.pixel(3, 0), Some([0, 255, 0, 255]), "索引2=绿");
        assert_eq!(img.pixel(0, 3), Some([0, 0, 255, 255]), "索引3=蓝");
        assert_eq!(img.pixel(3, 3), Some([255, 255, 255, 255]), "索引4=白");
        assert_eq!(img.pixel(1, 1), Some([128, 128, 128, 255]), "索引5=灰");
    }

    /// 1 位 BMP：调色板 0=黑,1=白。
    fn make_bmp_1bit() -> Vec<u8> {
        let mut palette = vec![[0u8, 0, 0, 255]; 2];
        palette[0] = [0, 0, 0, 255];
        palette[1] = [255, 255, 255, 255];
        // 每行 4 像素 → 1 字节（MSB=最左像素）。bit7=pixel0,bit6=pixel1,bit5=pixel2,bit4=pixel3。
        // 自下而上存储：file_rows[0]=底行(y=3)，file_rows[3]=顶行(y=0)。
        //   0xC0 = 前两个白；0x30 = 后两个白。
        let file_rows: [u8; 4] = [
            0xC0, // 底行(y=3)：前两个白
            0xC0, // y=2：前两个白
            0x30, // y=1：后两个白
            0x30, // 顶行(y=0)：后两个白
        ];
        let _stride = 4usize; // ceil(4/8)=1 字节，补齐到 4
        let mut pixel_bytes = Vec::new();
        for r in 0..4 {
            pixel_bytes.push(file_rows[r]);
            pixel_bytes.extend_from_slice(&[0u8; 3]); // 行尾填充
        }
        let dib_size = 40u32;
        let palette_offset = 14 + dib_size;
        let pixel_offset = palette_offset + 2 * 4;
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
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(pixel_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&[0u8; 8]);
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        for p in &palette {
            out.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
        out.extend_from_slice(&pixel_bytes);
        out
    }

    #[test]
    fn 解码1位bmp() {
        let bytes = make_bmp_1bit();
        let img = decode_bmp_impl(&bytes).unwrap();
        // 文件自下而上：文件行序 [0b00001111(底),0b11110000,0b11110000,0b00001111(顶)]
        // 输出顶行 = 文件最后一行 = 0b00001111 → 后两像素白
        assert_eq!(img.pixel(0, 0), Some([0, 0, 0, 255]), "顶行最左黑");
        assert_eq!(img.pixel(3, 0), Some([255, 255, 255, 255]), "顶行最右白");
        assert_eq!(img.pixel(0, 3), Some([255, 255, 255, 255]), "底行最左白");
        assert_eq!(img.pixel(3, 3), Some([0, 0, 0, 255]), "底行最右黑");
    }

    /// 16 位 565 + BI_BITFIELDS。构造全红(31,0,0)、全绿、全蓝、白四角。
    fn make_bmp_16() -> Vec<u8> {
        // 565: R=0xF800 G=0x07E0 B=0x001F
        fn enc(r: u8, g: u8, b: u8) -> u16 {
            let r5 = (r as u16 * 31 / 255) & 0x1F;
            let g6 = (g as u16 * 63 / 255) & 0x3F;
            let b5 = (b as u16 * 31 / 255) & 0x1F;
            (r5 << 11) | (g6 << 5) | b5
        }
        let corners = [
            enc(255, 0, 0),
            enc(0, 255, 0),
            enc(0, 0, 255),
            enc(255, 255, 255),
        ];
        let _stride = 8usize; // 4*2
        let mut pixel_bytes = Vec::new();
        for y in (0..4).rev() {
            for x in 0..4 {
                let v = if (x == 0 && y == 0) || (x == 3 && y == 3) {
                    corners[if x == 0 && y == 0 { 0 } else { 3 }]
                } else if x == 3 && y == 0 {
                    corners[1]
                } else if x == 0 && y == 3 {
                    corners[2]
                } else {
                    enc(128, 128, 128)
                };
                pixel_bytes.extend_from_slice(&v.to_le_bytes());
            }
        }
        let dib_size = 40u32;
        let masks_offset = 14 + dib_size;
        let pixel_offset = masks_offset + 12; // 3 个掩码
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
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(&3u32.to_le_bytes()); // BI_BITFIELDS
        out.extend_from_slice(&(pixel_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&[0u8; 8]);
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        // 掩码：R G B
        out.extend_from_slice(&0xF800u32.to_le_bytes());
        out.extend_from_slice(&0x07E0u32.to_le_bytes());
        out.extend_from_slice(&0x001Fu32.to_le_bytes());
        out.extend_from_slice(&pixel_bytes);
        out
    }

    #[test]
    fn 解码16位565bmp() {
        let bytes = make_bmp_16();
        let img = decode_bmp_impl(&bytes).unwrap();
        let [r, g, b, _] = img.pixel(0, 0).unwrap();
        assert!(r > 230 && g < 30 && b < 30, "左上应接近红，实际 {r},{g},{b}");
        let [r, g, b, _] = img.pixel(3, 0).unwrap();
        assert!(r < 30 && g > 230 && b < 30, "右上应接近绿，实际 {r},{g},{b}");
        let [r, g, b, _] = img.pixel(0, 3).unwrap();
        assert!(r < 30 && g < 30 && b > 230, "左下应接近蓝，实际 {r},{g},{b}");
        let [r, g, b, _] = img.pixel(3, 3).unwrap();
        assert!(r > 230 && g > 230 && b > 230, "右下应接近白，实际 {r},{g},{b}");
    }

    /// 自上而下（height<0）的 24 位 BMP，验证行翻转。
    #[test]
    fn 解码自上而下24位bmp() {
        let px = rgb_pattern_4x4();
        let mut pixel_bytes = Vec::new();
        for y in 0..4 {
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
        out.extend_from_slice(&(-4i32).to_le_bytes()); // 负高=自上而下
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&24u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(pixel_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&[0u8; 8]);
        out.extend_from_slice(&0u32.to_le_bytes()); // colors used
        out.extend_from_slice(&0u32.to_le_bytes()); // important
        out.extend_from_slice(&pixel_bytes);
        let img = decode_bmp_impl(&out).unwrap();
        assert_eq!(img.pixel(0, 0), Some([255, 0, 0, 255]), "自上而下：首行即顶行=红");
        assert_eq!(img.pixel(3, 3), Some([255, 255, 255, 255]), "右下白");
    }

    /// RLE8 编码的 8 位 BMP：用游程铺满四角。
    #[test]
    fn 解码rle8_bmp() {
        // 构造 4x4 索引图：全 5（灰），但四个角用索引 1/2/3/4。
        // 为简单，整图用 run-length 编码：[5×15][1][5×...] 复杂；这里直接逐行 RLE。
        // 行0(自下而上=底行): 索引 3,3,3,3 → run: a=4,b=3
        // 行1: 5,5,5,5 → a=4,b=5
        // 行2: 5,5,5,5 → a=4,b=5
        // 行3(顶行): 1,1,1,1 → a=4,b=1
        // 但左上(x0,y0)实际是顶行第0个=索引1=红；右下(x3,y3)是底行第3个=索引3=蓝。
        let mut palette = vec![[0u8, 0, 0, 255]; 256];
        palette[1] = [255, 0, 0, 255];
        palette[2] = [0, 255, 0, 255];
        palette[3] = [0, 0, 255, 255];
        palette[4] = [255, 255, 255, 255];
        palette[5] = [128, 128, 128, 255];
        let mut pixel_data = Vec::new();
        // 自下而上：先写底行
        // 底行 (y=3): 3,3,3,3
        pixel_data.extend_from_slice(&[4, 3]); // a=4 run of 3
        pixel_data.extend_from_slice(&[0, 0]); // EOL
        // y=2: 5,5,5,5
        pixel_data.extend_from_slice(&[4, 5]);
        pixel_data.extend_from_slice(&[0, 0]);
        // y=1: 5,5,5,5
        pixel_data.extend_from_slice(&[4, 5]);
        pixel_data.extend_from_slice(&[0, 0]);
        // 顶行 y=0: 1,1,1,1
        pixel_data.extend_from_slice(&[4, 1]);
        pixel_data.extend_from_slice(&[0, 1]); // EOB

        let dib_size = 40u32;
        let palette_offset = 14 + dib_size;
        let pixel_offset = palette_offset + 256 * 4;
        let file_size = pixel_offset + pixel_data.len() as u32;
        let mut out = Vec::new();
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&file_size.to_le_bytes());
        out.extend_from_slice(&[0u8; 4]);
        out.extend_from_slice(&(pixel_offset as u32).to_le_bytes());
        out.extend_from_slice(&dib_size.to_le_bytes());
        out.extend_from_slice(&(4i32).to_le_bytes());
        out.extend_from_slice(&(4i32).to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&8u16.to_le_bytes());
        out.extend_from_slice(&1u32.to_le_bytes()); // BI_RLE8
        out.extend_from_slice(&(pixel_data.len() as u32).to_le_bytes());
        out.extend_from_slice(&[0u8; 8]);
        out.extend_from_slice(&256u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        for p in &palette {
            out.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
        out.extend_from_slice(&pixel_data);

        let img = decode_bmp_impl(&out).unwrap();
        assert_eq!(img.pixel(0, 0), Some([255, 0, 0, 255]), "RLE8 顶行=红");
        assert_eq!(img.pixel(3, 3), Some([0, 0, 255, 255]), "RLE8 底行=蓝");
    }
}
