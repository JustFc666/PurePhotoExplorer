//! PNG 解码。

use std::path::Path;

use pv_core::error::{PvError, Result};

use super::convert::{gray8_to_rgba, gray_alpha_to_rgba, rgb24_to_rgba};
use super::io_err;
use super::resize::resize_rgba;
use crate::decoder::{fit_within, DecodeTarget, RgbaImage};

pub(super) fn decode_png(path: &Path, target: DecodeTarget) -> Result<RgbaImage> {
    let f = std::fs::File::open(path).map_err(io_err(path))?;
    let decoder = png::Decoder::new(f);
    let mut reader = decoder
        .read_info()
        .map_err(|e| PvError::Decode(format!("PNG 头解析失败: {e}")))?;

    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| PvError::Decode(format!("PNG 解码失败: {e}")))?;

    let w = info.width;
    let h = info.height;

    let rgba = match info.color_type {
        png::ColorType::Rgba => RgbaImage {
            width: w,
            height: h,
            data: buf[..info.buffer_size()].to_vec(),
        },
        png::ColorType::Rgb => rgb24_to_rgba(&buf[..info.buffer_size()], w, h)?,
        png::ColorType::Grayscale => gray8_to_rgba(&buf[..info.buffer_size()], w, h)?,
        png::ColorType::GrayscaleAlpha => gray_alpha_to_rgba(&buf[..info.buffer_size()], w, h)?,
        other => {
            return Err(PvError::UnsupportedFormat(format!(
                "暂不支持该 PNG 色彩类型: {other:?}"
            )))
        }
    };

    if !target.is_full_res() {
        if let Some(max) = target.max_edge {
            let (tw, th) = fit_within(w, h, Some(max));
            if tw < w || th < h {
                return resize_rgba(&rgba, tw, th);
            }
        }
    }
    Ok(rgba)
}
