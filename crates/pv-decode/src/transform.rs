//! 像素几何变换：旋转 / 裁剪。编码（写回原图）前的必要步骤。
//!
//! ## 为什么不做任意角度的精细重采样
//! 前端的角度微调（±45°）是**预览**用的。真正写回原图时，若按任意角度
//! 重采样并裁剪，会带来两个问题：
//! 1. 旋转后的外接矩形比原图大，需要「扩边」或「裁掉四角」，
//!    两种都会永久损失像素 —— 且用户在主界面看到的裁剪框是**容器自适应**的，
//!    与最终写出的像素尺寸对不上，属于「所见非所得」。
//! 2. 二次重采样（旋转一次、编码再压一次）画质损失叠加。
//!
//! 所以这里的约定是：**写回只应用 90° 整数倍旋转 + 按比例裁剪**。
//! 角度微调留在预览层，不落到文件上。前端在保存前会就此给出提示。
//!
//! ## 旋转方向约定
//! `rotate90` 的 `clockwise` 语义与前端一致：
//! - 顺时针 90°：原图左上角 → 新图右上角
//! - 逆时针 90°：原图左上角 → 新图左下角

use pv_core::error::{PvError, Result};

use crate::decoder::RgbaImage;

/// 顺时针旋转 90°。
pub fn rotate90_cw(src: &RgbaImage) -> RgbaImage {
    let (w, h) = (src.width, src.height);
    let mut out = RgbaImage {
        width: h,
        height: w,
        data: vec![0u8; src.data.len()],
    };
    // 目标 (x, y) ← 源 (y, h-1-x)：源第 x 列自底向上，成为目标第 x 行
    for y in 0..h {
        for x in 0..w {
            let si = ((y * w + x) * 4) as usize;
            // 目标坐标：(h-1-y, x)
            let dx = h - 1 - y;
            let dy = x;
            let di = ((dy * h + dx) * 4) as usize;
            out.data[di..di + 4].copy_from_slice(&src.data[si..si + 4]);
        }
    }
    out
}

/// 逆时针旋转 90°。
pub fn rotate90_ccw(src: &RgbaImage) -> RgbaImage {
    let (w, h) = (src.width, src.height);
    let mut out = RgbaImage {
        width: h,
        height: w,
        data: vec![0u8; src.data.len()],
    };
    // 目标 (x, y) ← 源 (w-1-y, x)
    for y in 0..h {
        for x in 0..w {
            let si = ((y * w + x) * 4) as usize;
            let dx = y;
            let dy = w - 1 - x;
            let di = ((dy * h + dx) * 4) as usize;
            out.data[di..di + 4].copy_from_slice(&src.data[si..si + 4]);
        }
    }
    out
}

/// 按 `steps` 个 90° 顺时针旋转（`steps` 对 4 取模，负数按逆时针处理）。
pub fn rotate90_steps(src: &RgbaImage, steps: i32) -> RgbaImage {
    match steps.rem_euclid(4) {
        0 => src.clone(),
        1 => rotate90_cw(src),
        2 => rotate180(src),
        _ => rotate90_ccw(src),
    }
}

/// 旋转 180°（等价于连续两次顺时针 90°，但一趟循环更快）。
pub fn rotate180(src: &RgbaImage) -> RgbaImage {
    let (w, h) = (src.width, src.height);
    let mut out = RgbaImage {
        width: w,
        height: h,
        data: vec![0u8; src.data.len()],
    };
    let pixels = (w as usize) * (h as usize);
    for i in 0..pixels {
        let si = i * 4;
        let di = (pixels - 1 - i) * 4;
        out.data[di..di + 4].copy_from_slice(&src.data[si..si + 4]);
    }
    out
}

/// 按归一化矩形裁剪（**归一化坐标**：0~1，相对图像宽/高）。
///
/// 与 [`crop_to_aspect`] 的区别：那个按固定宽高比**居中**裁，
/// 这个按用户鼠标框出的**任意位置与尺寸**裁。两者互斥，由前端决定用哪个。
///
/// 坐标会被夹到 `[0, 1]` 并保证不越界；裁出的尺寸至少 1×1 像素。
/// 入参非法（非有限值 / 宽高为 0）时返回错误，而不是悄悄产出空图。
pub fn crop_normalized(
    src: &RgbaImage,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> Result<RgbaImage> {
    let (iw, ih) = (src.width, src.height);
    if iw == 0 || ih == 0 {
        return Err(PvError::Encode("图像尺寸为 0，无法裁剪".into()));
    }
    if !(x.is_finite() && y.is_finite() && w.is_finite() && h.is_finite()) {
        return Err(PvError::Encode("裁剪区域参数非法".into()));
    }
    if w <= 0.0 || h <= 0.0 {
        return Err(PvError::Encode("裁剪区域宽高必须为正".into()));
    }

    // 夹到图像范围内（用户框到画布外时不会越界取像素）
    let nx = x.clamp(0.0, 1.0);
    let ny = y.clamp(0.0, 1.0);
    let nw = w.min(1.0 - nx);
    let nh = h.min(1.0 - ny);
    if nw <= 0.0 || nh <= 0.0 {
        return Err(PvError::Encode("裁剪区域超出图像范围".into()));
    }

    let ox = ((nx * iw as f64).round() as u32).min(iw - 1);
    let oy = ((ny * ih as f64).round() as u32).min(ih - 1);
    let cw = ((nw * iw as f64).round() as u32).clamp(1, iw - ox);
    let ch = ((nh * ih as f64).round() as u32).clamp(1, ih - oy);

    // 整图被框住时直接返回副本，省一次拷贝逻辑
    if ox == 0 && oy == 0 && cw == iw && ch == ih {
        return Ok(src.clone());
    }

    let mut out = RgbaImage {
        width: cw,
        height: ch,
        data: vec![0u8; (cw as usize) * (ch as usize) * 4],
    };
    let dst_stride = (cw as usize) * 4;
    let src_stride = (iw as usize) * 4;
    for row in 0..ch as usize {
        let s = (oy as usize + row) * src_stride + (ox as usize) * 4;
        let d = row * dst_stride;
        out.data[d..d + dst_stride].copy_from_slice(&src.data[s..s + dst_stride]);
    }
    Ok(out)
}

/// 从图像**中心**裁剪出宽高比为 `target`（宽/高）的最大矩形。
///
/// 与前端裁剪框的「覆盖模式」语义一致：先求原图内能容纳的最大目标比例矩形，
/// 再从中心裁切（等价于 `object-fit: cover` 后取中心）。
///
/// `target <= 0` 或非有限值时原样返回。
pub fn crop_to_aspect(src: &RgbaImage, target: f64) -> Result<RgbaImage> {
    if !target.is_finite() || target <= 0.0 {
        return Ok(src.clone());
    }
    let (w, h) = (src.width, src.height);
    if w == 0 || h == 0 {
        return Err(PvError::Encode("图像尺寸为 0，无法裁剪".into()));
    }

    // 目标矩形的最大尺寸：受限于宽或高
    let (cw, ch) = if (w as f64) / (h as f64) > target {
        // 原图更宽 → 高度用满
        ((h as f64) * target, h as f64)
    } else {
        // 原图更高 → 宽度用满
        (w as f64, (w as f64) / target)
    };
    let cw = (cw.round() as u32).clamp(1, w);
    let ch = (ch.round() as u32).clamp(1, h);

    // 已经等于目标尺寸就不用裁
    if cw == w && ch == h {
        return Ok(src.clone());
    }

    let ox = (w - cw) / 2;
    let oy = (h - ch) / 2;

    let mut out = RgbaImage {
        width: cw,
        height: ch,
        data: vec![0u8; (cw as usize) * (ch as usize) * 4],
    };
    let dst_stride = (cw as usize) * 4;
    let src_stride = (w as usize) * 4;
    for y in 0..ch as usize {
        let s = (oy as usize + y) * src_stride + (ox as usize) * 4;
        let d = y * dst_stride;
        out.data[d..d + dst_stride].copy_from_slice(&src.data[s..s + dst_stride]);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一张每像素编码自身坐标的图，便于精确验证旋转映射。
    /// 低字节 = x，次低字节 = y。
    fn coords(w: u32, h: u32) -> RgbaImage {
        let mut img = RgbaImage::new(w, h).unwrap();
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                img.data[i] = x as u8;
                img.data[i + 1] = y as u8;
                img.data[i + 2] = 0;
                img.data[i + 3] = 255;
            }
        }
        img
    }

    #[test]
    fn 顺时针90度宽高互换且映射正确() {
        let src = coords(4, 3);
        let out = rotate90_cw(&src);
        assert_eq!((out.width, out.height), (3, 4), "宽高应互换");

        // 原图左上 (0,0) 旋转后应在目标右上角
        assert_eq!(out.pixel(2, 0), Some([0, 0, 0, 255]), "原(0,0)→目标(2,0)");
        // 原图右上 (3,0) → 目标右下 (2,3)
        assert_eq!(out.pixel(2, 3), Some([3, 0, 0, 255]));
        // 原图左下 (0,2) → 目标左上 (0,0)
        assert_eq!(out.pixel(0, 0), Some([0, 2, 0, 255]));
    }

    #[test]
    fn 逆时针90度宽高互换且映射正确() {
        let src = coords(4, 3);
        let out = rotate90_ccw(&src);
        assert_eq!((out.width, out.height), (3, 4));

        // 原图左上 (0,0) → 目标左下 (0,3)
        assert_eq!(out.pixel(0, 3), Some([0, 0, 0, 255]));
        // 原图右上 (3,0) → 目标左上 (0,0)
        assert_eq!(out.pixel(0, 0), Some([3, 0, 0, 255]));
    }

    #[test]
    fn 顺逆各一次回到原图() {
        let src = coords(5, 3);
        let back = rotate90_ccw(&rotate90_cw(&src));
        assert_eq!((back.width, back.height), (5, 3));
        assert_eq!(back.data, src.data);
    }

    #[test]
    fn 旋转180度等价两次顺时针90度() {
        let src = coords(4, 3);
        let a = rotate180(&src);
        let b = rotate90_cw(&rotate90_cw(&src));
        assert_eq!((a.width, a.height), (b.width, b.height));
        assert_eq!(a.data, b.data);
    }

    #[test]
    fn 四次旋转回到原图() {
        let src = coords(6, 4);
        let out = rotate90_steps(&src, 4);
        assert_eq!(out.data, src.data);
    }

    #[test]
    fn 负步数按逆时针处理() {
        let src = coords(4, 3);
        let a = rotate90_steps(&src, -1);
        let b = rotate90_ccw(&src);
        assert_eq!(a.data, b.data);
    }

    #[test]
    fn 裁剪到正方形从中心取() {
        // 6x4 → 1:1 应裁成 4x4，左右各去掉 1 列
        let src = coords(6, 4);
        let out = crop_to_aspect(&src, 1.0).unwrap();
        assert_eq!((out.width, out.height), (4, 4));
        // 中心对齐：源 x 从 1 开始 → 目标 (0,0) 的源 x 应为 1
        assert_eq!(out.pixel(0, 0), Some([1, 0, 0, 255]));
        assert_eq!(out.pixel(3, 0), Some([4, 0, 0, 255]));
    }

    #[test]
    fn 裁剪到宽比例时高度变小() {
        // 4x4 → 16:9 应裁成 4x2（高用满宽时 4/16*9=2.25→2）
        let src = coords(4, 4);
        let out = crop_to_aspect(&src, 16.0 / 9.0).unwrap();
        assert_eq!(out.width, 4);
        assert!(out.height < 4, "高应减少，实际 {}", out.height);
    }

    #[test]
    fn 已是目标比例时原样返回() {
        let src = coords(4, 4);
        let out = crop_to_aspect(&src, 1.0).unwrap();
        assert_eq!((out.width, out.height), (4, 4));
        assert_eq!(out.data, src.data);
    }

    #[test]
    fn 非法比例原样返回() {
        let src = coords(4, 3);
        assert_eq!(crop_to_aspect(&src, 0.0).unwrap().data, src.data);
        assert_eq!(crop_to_aspect(&src, -1.0).unwrap().data, src.data);
        assert_eq!(crop_to_aspect(&src, f64::NAN).unwrap().data, src.data);
    }

    #[test]
    fn 一像素图不panic() {
        let src = coords(1, 1);
        assert_eq!(rotate90_cw(&src).data.len(), 4);
        assert_eq!(rotate180(&src).data.len(), 4);
        assert_eq!(crop_to_aspect(&src, 1.0).unwrap().data.len(), 4);
    }

    #[test]
    fn 归一化裁剪取到正确位置与尺寸() {
        // 8x8 图上取左上四分之一
        let src = coords(8, 8);
        let out = crop_normalized(&src, 0.0, 0.0, 0.25, 0.25).unwrap();
        assert_eq!((out.width, out.height), (2, 2));
        // 左上角仍是 (0,0)
        assert_eq!(out.pixel(0, 0), Some([0, 0, 0, 255]));
        assert_eq!(out.pixel(1, 1), Some([1, 1, 0, 255]));
    }

    #[test]
    fn 归一化裁剪非左上起点偏移正确() {
        // 从 x=0.5, y=0.25 起取 2x4（8x8 图上）
        let src = coords(8, 8);
        let out = crop_normalized(&src, 0.5, 0.25, 0.25, 0.5).unwrap();
        assert_eq!((out.width, out.height), (2, 4));
        // 左上角对应源 (4, 2)
        assert_eq!(out.pixel(0, 0), Some([4, 2, 0, 255]));
    }

    #[test]
    fn 归一化裁剪越界被夹住不panic() {
        let src = coords(8, 8);
        // 起点 0.9、宽 0.5 会超出右边界，应被夹到只剩 0.1
        let out = crop_normalized(&src, 0.9, 0.9, 0.5, 0.5).unwrap();
        assert!(out.width >= 1 && out.height >= 1);
        assert!(out.width <= 8 && out.height <= 8);
    }

    #[test]
    fn 归一化裁剪非法参数返回错误() {
        let src = coords(8, 8);
        assert!(crop_normalized(&src, 0.0, 0.0, 0.0, 0.5).is_err(), "零宽应报错");
        assert!(crop_normalized(&src, 0.0, 0.0, 0.5, -1.0).is_err(), "负高应报错");
        assert!(
            crop_normalized(&src, f64::NAN, 0.0, 0.5, 0.5).is_err(),
            "NaN 应报错"
        );
    }

    #[test]
    fn 归一化裁剪整图时等于原图() {
        let src = coords(4, 3);
        let out = crop_normalized(&src, 0.0, 0.0, 1.0, 1.0).unwrap();
        assert_eq!((out.width, out.height), (4, 3));
        assert_eq!(out.data, src.data);
    }
}
