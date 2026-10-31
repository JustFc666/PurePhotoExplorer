//! 批量旋转/翻转：**原地重写**像素，并把 EXIF 方向标记重置。
//!
//! 顺序固定为「先旋转、后翻转」——反过来在 90°/270° 下结果不同，
//! 用户看到的是预览里的那个顺序。

use std::path::Path;

use pv_decode::DecoderRegistry;
use tauri::State;

use super::{merr, BatchSummaryDto};
use crate::commands::dto::ErrorDto;
use crate::state::AppState;

/// 旋转/翻转并**写回原文件**（批量）。
///
/// ## 参数语义
/// - `rotate`：顺时针 90° 的步数，`1` = 顺 90，`-1`（或 `3`）= 逆 90，`2` = 180
/// - `flip_h` / `flip_v`：水平 / 垂直镜像
///
/// ## 顺序：先旋转，后翻转
/// 与「先把照片摆正、再左右翻一下」的直觉一致。同一次调用里两者可以叠加
/// （例如「顺 90° + 水平翻转」）。
///
/// ## 写回是破坏性的
/// - 只有 JPEG / PNG / BMP 能覆盖（`can_overwrite_original`）；
/// - RAW / HEIC / WebP / TIFF 等一律拒绝并给出原因 —— 尤其是 RAW：
///   画布像素来自内嵌 JPEG 预览，覆盖等于把 14bit 数据换成 8bit，不可逆；
/// - 先编码到内存、再原子替换（`write_rgba_atomic_with_exif`），
///   中途失败不会留下半截文件。
///
/// ## EXIF
/// 只要发生了任何几何变换（旋转或翻转），`Orientation` 一律置 1：
/// 像素已经是最终朝向，留着旧方向标记会让别的软件再转一次。
#[tauri::command]
pub async fn manager_transform_images(
    state: State<'_, AppState>,
    paths: Vec<String>,
    rotate: Option<i32>,
    flip_h: Option<bool>,
    flip_v: Option<bool>,
) -> Result<BatchSummaryDto, ErrorDto> {
    if paths.is_empty() {
        return Err(merr("没有选中任何图片"));
    }
    let steps = rotate.unwrap_or(0).rem_euclid(4);
    let fh = flip_h.unwrap_or(false);
    let fv = flip_v.unwrap_or(false);
    if steps == 0 && !fh && !fv {
        return Err(merr("没有指定任何调整（旋转或翻转）"));
    }
    let registry = state.registry_arc();

    let sum = tauri::async_runtime::spawn_blocking(move || {
        let mut out = BatchSummaryDto::new();
        for raw in &paths {
            let src_path = Path::new(raw);
            match transform_one(&registry, src_path, steps, fh, fv) {
                Ok(()) => out.done += 1,
                Err(e) => out.fail(src_path, e),
            }
        }
        out
    })
    .await
    .map_err(|e| merr(format!("批量旋转任务异常：{e}")))?;

    Ok(sum)
}

fn transform_one(
    registry: &DecoderRegistry,
    path: &Path,
    steps: i32,
    flip_h: bool,
    flip_v: bool,
) -> std::result::Result<(), String> {
    // 1) 格式能不能覆盖写回
    let info = registry.probe(path).map_err(|e| e.to_string())?;
    if !pv_decode::can_overwrite_original(info.format) {
        return Err(pv_decode::not_writable_reason(info.format));
    }

    // 2) 解码全尺寸（Export 档：写回不能拿 2048 预览降级原图）
    let src = registry
        .decode(path, pv_decode::DecodeTarget::new(pv_decode::DecodePurpose::Export))
        .map_err(|e| e.to_string())?;

    // 3) 先旋转、再翻转
    let mut img = pv_decode::rotate90_steps(&src, steps);
    if flip_h {
        img = flip_mirror(&img, true, false)?;
    }
    if flip_v {
        img = flip_mirror(&img, false, true)?;
    }

    // 4) EXIF：任何几何变换都让方向标记失效 → 置 1
    //
    // ⚠️ 这里以前是 `let _ = align_exif_with_pixels(...)`，把返回值丢了：
    //    `Unparsable` 意味着 EXIF 结构坏了、方向标记**没改成 1**，而像素已经转过了
    //    → 别的软件会再转一次。这不是能静默的分支，至少要留一条日志。
    //    （对齐 `edit.rs` 同名调用的处理；批量处理没有单张失败的理由 ——
    //     EXIF 坏不该阻断像素写回，所以是记日志而不是 `?`。）
    let exif = pv_meta::read_exif_raw(path).map(|mut t| {
        if pv_meta::align_exif_with_pixels(&mut t, 1) == pv_meta::ExifRewriteOutcome::Unparsable
        {
            tracing::warn!(
                target: "photovault::batch",
                path = %path.display(),
                "EXIF 结构无法解析：方向标记没能重置，像素已旋转 → 这张图可能被别的软件再转一次"
            );
        }
        t
    });

    // 5) 原子写回（编码到内存 → 临时文件 → rename）
    pv_decode::write_rgba_atomic_with_exif(&img, info.format, path, exif.as_deref())
        .map_err(|e| e.to_string())
}

/// 镜像翻转（RGBA 交织数据按行/列搬运，不走逐像素 `get/put_pixel`）。
fn flip_mirror(
    src: &pv_decode::RgbaImage,
    horizontal: bool,
    vertical: bool,
) -> std::result::Result<pv_decode::RgbaImage, String> {
    let (w, h) = (src.width as usize, src.height as usize);
    let row = w * 4;
    let s = &src.data;
    let mut out = pv_decode::RgbaImage::new(w as u32, h as u32).map_err(|e| e.to_string())?;
    let d = &mut out.data;
    for y in 0..h {
        let dy = if vertical { h - 1 - y } else { y };
        if horizontal {
            // 整行倒序：每个像素 4 字节一组反过来
            for x in 0..w {
                let si = y * row + x * 4;
                let di = dy * row + (w - 1 - x) * 4;
                d[di..di + 4].copy_from_slice(&s[si..si + 4]);
            }
        } else {
            d[dy * row..dy * row + row].copy_from_slice(&s[y * row..y * row + row]);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod flip_tests {
    use super::flip_mirror;
    use pv_decode::RgbaImage;

    /// 2×2 图，每像素首字节标记编号：`[1 2 / 3 4]`
    fn make() -> RgbaImage {
        let mut img = RgbaImage::new(2, 2).unwrap();
        for (i, v) in [1u8, 2, 3, 4].iter().enumerate() {
            img.data[i * 4..i * 4 + 4].copy_from_slice(&[*v, *v, *v, 255]);
        }
        img
    }

    fn at(img: &RgbaImage, idx: usize) -> u8 {
        img.data[idx * 4]
    }

    #[test]
    fn 水平翻转交换左右列() {
        // [1 2 / 3 4] → [2 1 / 4 3]
        let out = flip_mirror(&make(), true, false).unwrap();
        assert_eq!(
            [at(&out, 0), at(&out, 1), at(&out, 2), at(&out, 3)],
            [2, 1, 4, 3]
        );
    }

    #[test]
    fn 垂直翻转交换上下行() {
        // [1 2 / 3 4] → [3 4 / 1 2]
        let out = flip_mirror(&make(), false, true).unwrap();
        assert_eq!(
            [at(&out, 0), at(&out, 1), at(&out, 2), at(&out, 3)],
            [3, 4, 1, 2]
        );
    }

    #[test]
    fn 同时水平垂直等于旋转180() {
        // [1 2 / 3 4] → [4 3 / 2 1]
        let out = flip_mirror(&make(), true, true).unwrap();
        assert_eq!(
            [at(&out, 0), at(&out, 1), at(&out, 2), at(&out, 3)],
            [4, 3, 2, 1]
        );
    }

    #[test]
    fn 不翻转时原样返回() {
        let out = flip_mirror(&make(), false, false).unwrap();
        assert_eq!(
            [at(&out, 0), at(&out, 1), at(&out, 2), at(&out, 3)],
            [1, 2, 3, 4]
        );
    }
}
