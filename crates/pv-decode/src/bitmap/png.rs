//! PNG 解码。

use std::path::Path;

use pv_core::error::{PvError, Result};

use super::convert::{gray8_to_rgba, gray_alpha_to_rgba, rgb24_to_rgba};
use super::io_err;
use super::resize::resize_rgba;
use crate::decoder::{fit_within, DecodeTarget, RgbaImage};

pub(super) fn decode_png(path: &Path, target: DecodeTarget) -> Result<RgbaImage> {
    let f = std::fs::File::open(path).map_err(io_err(path))?;
    let mut decoder = png::Decoder::new(f);

    // ⭐ 必须显式声明输出变换（2026-11 修）。
    //
    // `png 0.17` 的默认变换是 `IDENTITY` —— 它**不剥 16 位样本的高字节**，
    // 于是 16 位 PNG 解出来 `data.len() == w*h*8`，而 `RgbaImage` 的不变量是
    // `w*h*4`。下游 `resize` / 旋转 / 裁剪 / 编码全都按 4 字节步长索引，
    // 结果是**花屏或半张图，而且没有任何一处报错** —— 属于最难查的那类错图。
    //
    // `normalize_to_color8()` 就是 `EXPAND | STRIP_16`，也正是 png 0.16 及更早
    // 版本的默认行为（0.17 为了"无损原样输出"把它改成了 opt-in）：
    //   · `STRIP_16`：16 位 → 8 位（取高字节）
    //   · `EXPAND`  ：调色板图 → RGB/RGBA；<8 位灰度 → 8 位；tRNS → alpha
    //
    // 顺带修掉另一件事：**调色板 PNG 原本落进 `other` 分支直接报"暂不支持"**
    // （`ColorType::Indexed` 没人处理）。展开之后 `info.color_type` 只可能是
    // Rgba / Rgb / Grayscale / GrayscaleAlpha 四种，下面的 match 正好全覆盖。
    //
    // ⚠️ 不要以为可以省掉：`OutputInfo` 携带的是**变换后**的色彩类型与位深，
    // 所以设置之后 `output_buffer_size()` / `buffer_size()` 会自动变成正确值。
    decoder.set_transformations(png::Transformations::normalize_to_color8());

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
        png::ColorType::Rgba => {
            // 不变量：这一支是**直接搬字节**，长度必须严格等于 `w*h*4`。
            // 一旦破掉，下游没有任何地方会报错，只会渲染出错图 ——
            // 所以在这里断死，把"静默错图"变成"测试当场炸"。
            debug_assert_eq!(
                info.buffer_size(),
                (w as usize) * (h as usize) * 4,
                "RgbaImage 长度不变量被破坏（16 位 PNG 没降位？检查 set_transformations）"
            );
            RgbaImage {
                width: w,
                height: h,
                data: buf[..info.buffer_size()].to_vec(),
            }
        }
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
