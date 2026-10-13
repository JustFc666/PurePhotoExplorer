//! JPEG 解码（zune-jpeg）。
//!
//! ## 为什么用 zune-jpeg 而非 jpeg-decoder（2026-10-03 实测，22 MB 的 5847×3898 JPEG）
//!
//! | 库 | 缩略图档(731×488) | 全尺寸(5847×3898) |
//! |---|---|---|
//! | jpeg-decoder 0.3.2 | **49 秒** | 49 秒 |
//! | **zune-jpeg 0.5.15** | — | **193 毫秒** |
//!
//! jpeg-decoder 慢的根因：它的 `scale()` 只做 **IDCT 缩放**，
//! 而 JPEG 的**熵解码（Huffman）阶段无法跳过** —— 所有宏块系数必须先解出来。
//! 所以 IDCT 缩放只省 IDCT 的时间（占比很小），实测毫无改善。
//!
//! zune-jpeg 有高度优化的熵解码器 + SIMD，整图 193 ms。
//!
//! **结论**：JPEG 缩略图的正确做法是「快速解码全图 + SIMD 缩放」，
//! 而不是指望解码器做缩放。

use std::path::Path;

use fast_image_resize::{FilterType, ResizeAlg};
use pv_core::error::{PvError, Result};

use super::convert::rgb24_to_rgba;
use super::probe::read_jpeg_dims;
use super::resize::resize_rgb24;
use crate::decoder::{fit_within, DecodePurpose, DecodeTarget, RgbaImage};
use crate::bitmap::io_err;

/// 从内存里的 JPEG 字节解码为 RGBA（供 RAW 内嵌预览复用）。
///
/// RAW 的内嵌预览是**相机原样写入的 JPEG 字节**，没有独立文件，
/// 因此不能走 [`decode_jpeg`] 的路径读文件。除此之外解码流程完全一致
/// —— 都是 zune-jpeg 全量解码 + 按 `DecodeTarget` 降采样（见 [`finish_jpeg`]）。
pub(super) fn decode_jpeg_bytes(data: &[u8], target: DecodeTarget) -> Result<RgbaImage> {
    let mut decoder = zune_jpeg::JpegDecoder::new(std::io::Cursor::new(data));
    let pixels = decoder
        .decode()
        .map_err(|e| PvError::Decode(format!("内嵌预览 JPEG 解码失败: {e}")))?;
    let (w, h) = decoder
        .dimensions()
        .ok_or_else(|| PvError::Decode("内嵌预览尺寸不可用".into()))?;
    let (w, h) = (w as u32, h as u32);
    if w == 0 || h == 0 {
        return Err(PvError::Decode("内嵌预览尺寸为 0".into()));
    }
    finish_jpeg(pixels, w, h, target)
}

/// 解码后的收尾：按 `DecodeTarget` 降采样并转成 RGBA。
///
/// ## 为什么先缩放、后转 RGBA（2026-10-11 性能定稿）
/// 反过来做（先转整幅 RGBA 再缩放）会先分配并写满一整幅 `w×h×4`，
/// 而这些像素**绝大多数马上就被缩掉扔掉**。22 MB / 5847×3898 的实测：
/// 预览档这一步从 **76.7 ms 降到 15.2 ms**（RGB 空间缩放 + rayon）。
/// 详见 [`super::resize::resize_rgb24`] 的注释。
///
/// ⚠️ 拿的是 `Vec<u8>`（所有权）而不是 `&[u8]`：zune 解出来的缓冲直接 move 进
/// 缩放器，省掉一整幅 65 MB 的拷贝。
fn finish_jpeg(rgb: Vec<u8>, w: u32, h: u32, target: DecodeTarget) -> Result<RgbaImage> {
    // 目标边长；全尺寸档（导出）没有上限 → 直接转 RGBA 返回
    let Some(max_edge) = target.max_edge.filter(|m| *m > 0) else {
        return rgb24_to_rgba(&rgb, w, h);
    };
    let (tw, th) = fit_within(w, h, Some(max_edge));
    // 目标不小于原图 → 没有可省的像素，直接转
    if tw >= w && th >= h {
        return rgb24_to_rgba(&rgb, w, h);
    }

    // 缩略图是重缩小，用最快的算法（Nearest）；预览/编辑用 Lanczos3 保质量。
    // ⚠️ 别以为换成 Bilinear 会更快 —— 实测反而略慢（内存带宽瓶颈，不是滤波核）。
    let alg = match target.purpose {
        DecodePurpose::Thumbnail => ResizeAlg::Nearest,
        _ => ResizeAlg::Convolution(FilterType::Lanczos3),
    };
    let small = resize_rgb24(rgb, w, h, tw, th, alg)?;
    rgb24_to_rgba(&small, tw, th)
}

pub(super) fn decode_jpeg(path: &Path, target: DecodeTarget) -> Result<RgbaImage> {
    // 尺寸只能自己从 SOF 标记解析（见 read_jpeg_dims）
    let (orig_w, orig_h) = read_jpeg_dims(path)
        .ok_or_else(|| PvError::Decode("无法解析 JPEG 尺寸，文件可能损坏".into()))?;

    // 读入文件
    let data = std::fs::read(path).map_err(io_err(path))?;

    // zune-jpeg 全量解码（**JPEG 熵解码无法跳过，这是预览档的耗时下限**）
    let mut decoder = zune_jpeg::JpegDecoder::new(std::io::Cursor::new(&data));
    let pixels = decoder
        .decode()
        .map_err(|e| PvError::Decode(format!("JPEG 解码失败: {e}")))?;
    let (w, h) = decoder
        .dimensions()
        .unwrap_or((orig_w as usize, orig_h as usize));
    let w = w as u32;
    let h = h as u32;

    finish_jpeg(pixels, w, h, target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn JPEG目标尺寸在解码前确定() {
        // 缩放目标必须先算好，而不是解码完再缩
        let t = DecodeTarget::new(DecodePurpose::Preview);
        let (w, h) = fit_within(6000, 4000, t.max_edge);
        assert_eq!((w, h), (2048, 1365));
        assert!(t.max_edge.unwrap() < 6000, "浏览场景必须降采样");
    }

    #[test]
    fn 导出场景不做任何缩放() {
        let t = DecodeTarget::new(DecodePurpose::Export);
        assert_eq!(fit_within(6000, 4000, t.max_edge), (6000, 4000));
    }

    /// 取仓库 `test-image/` 下的真实样本（< 4KB 视为 Git LFS 指针）。
    fn sample(name: &str) -> Option<std::path::PathBuf> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-image")
            .join(name);
        if std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0) > 4096 {
            Some(p)
        } else {
            None
        }
    }

    /// **回归护栏**：RGB 空间缩放（新）必须与 RGBA 空间缩放（旧）**逐位一致**。
    ///
    /// 2026-10-11 的性能优化把「先转整幅 RGBA 再缩放」改成「先在 RGB 上缩放、
    /// 最后把小图转 RGBA」，预览档 301 ms → 218 ms。这是**画质敏感**改动：
    /// 一旦两条路径结果不同，就等于悄悄换了渲染结果，用户只会觉得"颜色变了"。
    ///
    /// 实测本机 22 MB 样本最大像素差为 **0**。这里把「允许的差异」钉死在 0，
    /// 以后若有人调整缩放实现（换滤波核、换通道数），会立刻被这条测试拦下。
    #[test]
    fn RGB空间缩放与RGBA空间缩放逐位一致() {
        use crate::bitmap::resize::resize_rgba_with;
        use super::super::convert::rgb24_to_rgba;

        let Some(p) = sample("DSC9515.JPG") else {
            eprintln!("跳过：找不到 DSC9515.JPG 样本");
            return;
        };
        // 新路径（生产代码）
        let new = decode_jpeg(&p, DecodeTarget::new(DecodePurpose::Preview)).unwrap();

        // 旧路径：解码 → 全图 RGBA → U8x4 Lanczos3 缩放
        let data = std::fs::read(&p).unwrap();
        let mut dec = zune_jpeg::JpegDecoder::new(std::io::Cursor::new(&data));
        let rgb = dec.decode().unwrap();
        let (w, h) = dec.dimensions().unwrap();
        let (w, h) = (w as u32, h as u32);
        let full = rgb24_to_rgba(&rgb, w, h).unwrap();
        let (tw, th) = fit_within(w, h, DecodeTarget::new(DecodePurpose::Preview).max_edge);
        let old = resize_rgba_with(&full, tw, th, ResizeAlg::Convolution(FilterType::Lanczos3)).unwrap();

        assert_eq!((new.width, new.height), (old.width, old.height), "尺寸必须一致");
        let max_diff = new
            .data
            .iter()
            .zip(old.data.iter())
            .map(|(a, b)| (*a as i32 - *b as i32).abs())
            .max()
            .unwrap_or(0);
        assert_eq!(max_diff, 0, "新路径与旧路径像素不一致（最大差 {max_diff}）");
    }
}
