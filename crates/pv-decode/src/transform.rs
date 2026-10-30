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

/// 水平镜像（左右翻转）。
pub fn flip_horizontal(src: &RgbaImage) -> RgbaImage {
    let (w, h) = (src.width, src.height);
    let mut out = RgbaImage {
        width: w,
        height: h,
        data: vec![0u8; src.data.len()],
    };
    let row = (w * 4) as usize;
    for y in 0..h {
        let base = y as usize * row;
        for x in 0..w {
            let si = base + x as usize * 4;
            let di = base + (w - 1 - x) as usize * 4;
            out.data[di..di + 4].copy_from_slice(&src.data[si..si + 4]);
        }
    }
    out
}

/// 垂直镜像（上下翻转）。
pub fn flip_vertical(src: &RgbaImage) -> RgbaImage {
    let (w, h) = (src.width, src.height);
    let mut out = RgbaImage {
        width: w,
        height: h,
        data: vec![0u8; src.data.len()],
    };
    let row = (w * 4) as usize;
    for y in 0..h {
        let si = y as usize * row;
        let di = (h - 1 - y) as usize * row;
        out.data[di..di + row].copy_from_slice(&src.data[si..si + row]);
    }
    out
}

/// 按 EXIF `Orientation`（1~8）把**存储像素**摆正成**显示像素**。
///
/// ## 为什么必须有这一层（2026-10-30）
/// 手机竖拍的照片，传感器写出来的是**横的像素** + 一个 `Orientation=6`
/// 的小标签，让看图软件替它转 90°。我们之前既不读这个标签、也不转 ——
/// 于是缩略图/看图/打印/人脸检测里，这类照片全是**躺倒**的
/// （测试库里 353 张 JPEG 有 17 张是这样）。
///
/// ## 八种取值的含义（`dst(x,y) ← src(?, ?)`）
/// | 值 | 含义 | 源坐标 |
/// |---|---|---|
/// | 1 | 正常 | (x, y) |
/// | 2 | 水平镜像 | (w-1-x, y) |
/// | 3 | 旋转 180° | (w-1-x, h-1-y) |
/// | 4 | 垂直镜像 | (x, h-1-y) |
/// | 5 | 沿主对角线转置 | (y, x) |
/// | 6 | 顺时针 90° | (y, h-1-x) |
/// | 7 | 沿副对角线转置 | (w-1-y, h-1-x) |
/// | 8 | 逆时针 90° | (w-1-y, x) |
///
/// ⚠️ 5~8 会把宽高**互换**，所以 `dst` 的尺寸要按 `swap` 算 ——
/// 忘了这一步就会出现半截黑边或越界。
///
/// ⛔ 不要用「链式调用 rotate90_cw + flip」来实现：5/7 的组合顺序很容易搞反，
/// 而错了以后**图看起来"还是那张脸"**（只是镜像了），肉眼极难发现。
pub fn apply_orientation(src: &RgbaImage, orientation: u16) -> RgbaImage {
    if orientation <= 1 || orientation > 8 {
        return src.clone();
    }
    let (w, h) = (src.width, src.height);
    let swap = matches!(orientation, 5..=8);
    let (dw, dh) = if swap { (h, w) } else { (w, h) };
    let mut out = RgbaImage {
        width: dw,
        height: dh,
        data: vec![0u8; src.data.len()],
    };
    let srow = (w * 4) as usize;
    let drow = (dw * 4) as usize;
    for y in 0..dh {
        for x in 0..dw {
            let (sx, sy) = match orientation {
                2 => (w - 1 - x, y),
                3 => (w - 1 - x, h - 1 - y),
                4 => (x, h - 1 - y),
                5 => (y, x),
                6 => (y, h - 1 - x),
                7 => (w - 1 - y, h - 1 - x),
                8 => (w - 1 - y, x),
                _ => (x, y),
            };
            let si = sy as usize * srow + sx as usize * 4;
            let di = y as usize * drow + x as usize * 4;
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
    // ── EXIF 方向（2026-10-30）─────────────────────────────────────

    /// 造一张「像素值 = 行*10 + 列」的 w×h 图，宽高不等、每格唯一，
    /// 方向转错立刻能看出来（镜像/转置的顺序错误在纯色图上不可见）。
    fn grid(w: u32, h: u32) -> RgbaImage {
        let mut data = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                data[i] = (y * 10 + x) as u8; // R = 编号
                data[i + 3] = 255;
            }
        }
        RgbaImage { width: w, height: h, data }
    }

    fn red_at(img: &RgbaImage, x: u32, y: u32) -> u8 {
        img.data[((y * img.width + x) * 4) as usize]
    }

    /// 8 种取值逐个对照 EXIF 规范。源图 3×2：
    /// ```
    /// 0 1 2
    /// 10 11 12
    /// ```
    #[test]
    fn 八种方向都正确() {
        let src = grid(3, 2); // w=3 h=2

        // 1 正常
        let o1 = apply_orientation(&src, 1);
        assert_eq!((o1.width, o1.height), (3, 2));
        assert_eq!(red_at(&o1, 0, 0), 0);
        assert_eq!(red_at(&o1, 2, 1), 12);

        // 2 水平镜像：左右互换，尺寸不变
        let o2 = apply_orientation(&src, 2);
        assert_eq!((o2.width, o2.height), (3, 2));
        assert_eq!(red_at(&o2, 0, 0), 2);
        assert_eq!(red_at(&o2, 2, 0), 0);

        // 3 旋转 180
        let o3 = apply_orientation(&src, 3);
        assert_eq!((o3.width, o3.height), (3, 2));
        assert_eq!(red_at(&o3, 0, 0), 12);
        assert_eq!(red_at(&o3, 2, 1), 0);

        // 4 垂直镜像：上下互换
        let o4 = apply_orientation(&src, 4);
        assert_eq!((o4.width, o4.height), (3, 2));
        assert_eq!(red_at(&o4, 0, 0), 10);
        assert_eq!(red_at(&o4, 0, 1), 0);

        // 5 沿主对角线转置：尺寸互换，src(x,y) 到 dst(y,x)
        let o5 = apply_orientation(&src, 5);
        assert_eq!((o5.width, o5.height), (2, 3));
        assert_eq!(red_at(&o5, 0, 0), 0);
        assert_eq!(red_at(&o5, 1, 0), 10, "src 第 0 列第 1 行 → dst 第 1 列第 0 行");
        assert_eq!(red_at(&o5, 0, 2), 2);

        // 6 顺时针 90°：src 左下角 → dst 左上角
        let o6 = apply_orientation(&src, 6);
        assert_eq!((o6.width, o6.height), (2, 3));
        assert_eq!(red_at(&o6, 0, 0), 10, "竖拍照片最常见的一种");
        assert_eq!(red_at(&o6, 1, 0), 0);
        assert_eq!(red_at(&o6, 0, 2), 12);
        assert_eq!(red_at(&o6, 1, 2), 2);

        // 7 沿副对角线转置
        let o7 = apply_orientation(&src, 7);
        assert_eq!((o7.width, o7.height), (2, 3));
        assert_eq!(red_at(&o7, 0, 0), 12);
        assert_eq!(red_at(&o7, 1, 2), 0);

        // 8 逆时针 90°：src 右上角 → dst 左上角
        let o8 = apply_orientation(&src, 8);
        assert_eq!((o8.width, o8.height), (2, 3));
        assert_eq!(red_at(&o8, 0, 0), 2);
        assert_eq!(red_at(&o8, 1, 0), 12);
        assert_eq!(red_at(&o8, 0, 2), 0);
        assert_eq!(red_at(&o8, 1, 2), 10);
    }

    /// 6 / 8 互逆：转过去再转回来必须是原图（逐字节相等）。
    #[test]
    fn 旋转九十度可逆() {
        let src = grid(5, 3);
        let back = apply_orientation(&apply_orientation(&src, 6), 8);
        assert_eq!((back.width, back.height), (5, 3));
        assert_eq!(back.data, src.data);
    }

    /// 非法/缺失值（0、9、255）按「正常」处理，不 panic。
    #[test]
    fn 非法方向当正常处理() {
        let src = grid(3, 2);
        for o in [0u16, 9, 255] {
            let out = apply_orientation(&src, o);
            assert_eq!((out.width, out.height), (3, 2));
            assert_eq!(out.data, src.data, "方向 {o} 应该原样返回");
        }
    }

    /// 1×1 和 1×N 这种退化尺寸不能越界 panic。
    #[test]
    fn 退化尺寸不越界() {
        for (w, h) in [(1u32, 1u32), (1, 5), (5, 1)] {
            let src = grid(w, h);
            for o in 1..=8u16 {
                let out = apply_orientation(&src, o);
                let expect = if o >= 5 { (h, w) } else { (w, h) };
                assert_eq!((out.width, out.height), expect, "{w}x{h} 方向 {o}");
                assert_eq!(out.data.len(), src.data.len());
            }
        }
    }
}
