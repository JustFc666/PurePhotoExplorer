//! SIMD 缩放（fast_image_resize）。

use fast_image_resize::images::Image as FirImage;
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
use pv_core::error::{PvError, Result};

use crate::decoder::RgbaImage;

/// SIMD 缩放（默认 Lanczos3，缩图质量优于双线性）
pub(super) fn resize_rgba(src: &RgbaImage, tw: u32, th: u32) -> Result<RgbaImage> {
    resize_rgba_with(src, tw, th, ResizeAlg::Convolution(FilterType::Lanczos3))
}

/// 指定缩放算法的 SIMD 缩放
pub(super) fn resize_rgba_with(
    src: &RgbaImage,
    tw: u32,
    th: u32,
    alg: ResizeAlg,
) -> Result<RgbaImage> {
    if tw == 0 || th == 0 {
        return Err(PvError::Decode("目标尺寸为 0".into()));
    }
    if src.width == tw && src.height == th {
        return Ok(src.clone());
    }

    let src_view = FirImage::from_vec_u8(src.width, src.height, src.data.clone(), PixelType::U8x4)
        .map_err(|e| PvError::Decode(format!("源图构造失败: {e}")))?;

    let mut dst_view = FirImage::new(tw, th, PixelType::U8x4);

    let options = ResizeOptions::new().resize_alg(alg);

    Resizer::new()
        .resize(&src_view, &mut dst_view, &options)
        .map_err(|e| PvError::Decode(format!("缩放失败: {e}")))?;

    // fast_image_resize 5.x 的 into_vec 直接返回 Vec<u8>
    let out = dst_view.into_vec();

    Ok(RgbaImage {
        width: tw,
        height: th,
        data: out,
    })
}

/// **RGB24 空间**的 SIMD 缩放（解压后的直出格式，拿走所有权以免拷贝）。
///
/// ## 为什么要单独有这条路径（2026-10-11 实测，22 MB / 5847×3898 JPEG 预览档）
/// 原做法是「先 `rgb24_to_rgba` 转出整幅 RGBA，再在 RGBA 空间缩放」。对
/// 预览档（→2048）而言，那意味着先分配并写满一整幅 86 MB 的 RGBA，
/// 其中绝大部分像素**马上就被缩掉扔掉**：
///
/// | 做法 | 缩放 + 转换耗时 |
/// |---|---|
/// | RGBA 空间（U8x4，单线程） | 76.7 ms |
/// | RGB 空间（U8x3，单线程） | 26.5 ms |
/// | **RGB 空间 + rayon** | **15.2 ms** |
///
/// U8x3 比 U8x4 快这么多，不只是少一个通道 —— `fast_image_resize` 的
/// U8x4 路径要处理 alpha，走的是更慢的通用分支。所以**能不移到 RGBA 就别移**：
/// 先在三通道上缩放，只在最后把小图转成 RGBA。
///
/// ⛔ 别把入参改回 `&[u8]`：这里要的是**所有权**（zune 解出来的 Vec 直接move进来），
/// 改成借用就得先 clone 一整幅 65 MB，收益全被吃掉。
pub(super) fn resize_rgb24(
    src: Vec<u8>,
    sw: u32,
    sh: u32,
    tw: u32,
    th: u32,
    alg: ResizeAlg,
) -> Result<Vec<u8>> {
    if tw == 0 || th == 0 {
        return Err(PvError::Decode("目标尺寸为 0".into()));
    }
    let src_view = FirImage::from_vec_u8(sw, sh, src, PixelType::U8x3)
        .map_err(|e| PvError::Decode(format!("源图构造失败: {e}")))?;
    let mut dst_view = FirImage::new(tw, th, PixelType::U8x3);
    Resizer::new()
        .resize(&src_view, &mut dst_view, &ResizeOptions::new().resize_alg(alg))
        .map_err(|e| PvError::Decode(format!("缩放失败: {e}")))?;
    Ok(dst_view.into_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 同尺寸缩放直接返回原图() {
        let img = RgbaImage::new(4, 4).unwrap();
        let out = resize_rgba(&img, 4, 4).unwrap();
        assert_eq!(out.width, 4);
        assert_eq!(out.height, 4);
    }

    #[test]
    fn 缩放到零尺寸返回Err() {
        let img = RgbaImage::new(4, 4).unwrap();
        assert!(resize_rgba(&img, 0, 4).is_err());
        assert!(resize_rgba(&img, 4, 0).is_err());
    }

    #[test]
    fn 实际缩放尺寸正确() {
        let src = RgbaImage::new(100, 50).unwrap();
        let out = resize_rgba(&src, 50, 25).unwrap();
        assert_eq!((out.width, out.height), (50, 25));
        assert_eq!(out.data.len(), 50 * 25 * 4);
    }
}
