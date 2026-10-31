//! 缩略图与尺寸探测（网格排版 / 缩略图生成用）。
//!
//! - [`probe_dimensions`]：拿宽高（**不缩放、不编码**，供网格排版）
//! - [`make_thumbnail`]：解码到「最长边 ≤ `max_edge`」并编码为 JPEG 字节
//!
//! ## 为什么用 JPEG 而不是 WebP
//! 项目里 `image-webp` 的编码器只有**无损**（VP8L），缩略图体积会比有损 q82 大 5–10 倍，
//! 磁盘缓存（默认 5 GB / 万级）根本放不下。`jpeg-encoder` 已在依赖里，照片场景最合适。
//! 代价：JPEG **无 alpha**，透明像素会**合成到白底**（见 [`flatten_on_white`]）。

use std::path::Path;

use pv_core::error::Result;
use pv_core::format::ImageFormat;

use crate::decoder::{DecodePurpose, DecodeTarget, DecoderRegistry, RgbaImage};

/// 缩略图编码格式（当前固定 JPEG）。
pub const THUMB_FORMAT: ImageFormat = ImageFormat::Jpeg;

/// 取图像宽高（不缩放、不编码）。
///
/// ⛔ **RAW 一律拒绝**，理由有两层：
///
/// 1. **代价**：RAW 的 `probe` 走的是「找内嵌 JPEG 预览」这条路，要把文件读进来
///    （一张 ARW 47MB）。一个 611 个 ARW 的目录就是 ~28GB 读取，界面会假死几分钟
///    —— 2026-10-17 实测 287 秒。
/// 2. **正确性**：即使探出来，拿到的是**内嵌预览**的尺寸（1616×1080），
///    不是传感器尺寸（6000×4000）。显示它是**误导**，宁缺毋假。
///
/// 这条防护放在这里而不是各调用点：过去两个调用点各自「记得跳过 RAW」，
/// 其中一处回滚时忘了就复活了这个 bug。**解码层知道这个事实，就该在这里拦住**。
pub fn probe_dimensions(reg: &DecoderRegistry, path: &Path) -> Result<(u32, u32)> {
    let info = reg.probe(path)?;
    if info.format.is_raw() || info.raw.is_some() {
        return Err(pv_core::error::PvError::UnsupportedFormat(
            "RAW 不支持探测尺寸（内嵌预览尺寸不等于传感器尺寸，且代价极高）".into(),
        ));
    }
    Ok((info.width, info.height))
}

/// 生成缩略图，**同时返回位图与 JPEG 字节**：
/// - 位图给 **L1 内存缓存**（保留 alpha，UI 自己决定底色）
/// - JPEG 字节给 **L2 磁盘缓存**（无 alpha → 合成白底）
///
/// `max_edge` 会被收敛到 ≥ 1，避免 0 触发除零。
pub fn make_thumbnail_with_bitmap(
    reg: &DecoderRegistry,
    path: &Path,
    max_edge: u32,
) -> Result<(RgbaImage, Vec<u8>)> {
    let target = DecodeTarget::exact(DecodePurpose::Thumbnail, Some(max_edge.max(1)));
    let img = reg.decode(path, target)?;
    let flat = flatten_on_white(&img);
    let bytes = crate::encode::encode_rgba(&flat, THUMB_FORMAT)?;
    Ok((img, bytes))
}

/// 只要 JPEG 字节（见 [`make_thumbnail_with_bitmap`]）。
pub fn make_thumbnail(reg: &DecoderRegistry, path: &Path, max_edge: u32) -> Result<Vec<u8>> {
    Ok(make_thumbnail_with_bitmap(reg, path, max_edge)?.1)
}

/// 直接解码到「最长边 ≤ `max_edge`」，**只返回位图，不编码、不碰任何缓存**。
///
/// 给「只看一眼、不落盘」的场景用（文件浏览器里的临时预览）。
/// 比 [`make_thumbnail_with_bitmap`] 少一次 JPEG 编码 —— 那次编码只是为了写
/// **L2 磁盘缓存**，临时预览不需要它，所以这条路反而更快。
pub fn decode_scaled(reg: &DecoderRegistry, path: &Path, max_edge: u32) -> Result<RgbaImage> {
    let target = DecodeTarget::exact(DecodePurpose::Thumbnail, Some(max_edge.max(1)));
    reg.decode(path, target)
}

/// 把 RGBA 合成到**白底**并置为不透明（JPEG 无 alpha 通道）。
///
/// 不透明像素原样返回；越界/极端值不会 panic（用 u32 中间量）。
///
/// ⚠️ **两处共用，别各写一份**：缩略图转 JPEG（本模块）与「另存为 JPEG」
/// （`encode::encode_jpeg`）。两处实现一旦分叉，用户会看到「缩略图里透明的
/// 地方是白的、另存出来却是黑的」，而这种不一致极难归因。
pub(crate) fn flatten_on_white(img: &RgbaImage) -> RgbaImage {
    let mut out = img.clone();
    for px in out.data.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a == 255 {
            continue; // 本就不透明，跳过
        }
        for ch in &mut px[..3] {
            let v = *ch as u32;
            // 预乘到白底：out = v*a + 255*(255-a)，再 /255
            *ch = ((v * a + 255 * (255 - a)) / 255) as u8;
        }
        px[3] = 255;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn sample(name: &str) -> Option<PathBuf> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-image")
            .join(name);
        if p.is_file() && std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0) > 4096 {
            Some(p)
        } else {
            None
        }
    }

    fn registry() -> DecoderRegistry {
        let mut reg = DecoderRegistry::new();
        reg.register(Box::new(crate::BitmapBackend));
        reg
    }

    /// 带 RAW 管线的注册表（和 App 里的 `AppState` 一致）
    fn registry_with_raw() -> DecoderRegistry {
        let mut reg = DecoderRegistry::new();
        reg.register(Box::new(crate::BitmapBackend));
        reg.set_raw_pipeline(Box::new(crate::EmbeddedPreviewPipeline::new()));
        reg
    }

    /// ⛔ RAW 的尺寸探测必须**直接拒绝**。
    ///
    /// 护栏价值：这条 bug 出现过两次 —— 探测一张 47MB 的 ARW 要把整个文件读进
    /// 来找内嵌预览，611 张就是 ~28GB、界面假死 287 秒。之前靠每个调用点
    /// 「记得跳过 RAW」，其中一处回滚时忘了就复活了。现在拦在解码层。
    #[test]
    fn RAW不探测尺寸_直接拒绝() {
        let Some(p) = sample("canon-cr3-sample.CR3") else {
            eprintln!("跳过：找不到 RAW 样本");
            return;
        };
        let reg = registry_with_raw();
        let r = probe_dimensions(&reg, &p);
        assert!(
            matches!(r, Err(pv_core::error::PvError::UnsupportedFormat(_))),
            "RAW 应返回 UnsupportedFormat，实际 {r:?}"
        );
    }

    /// 普通位图照旧能探（证明上一条不是把整个函数弄坏了）
    #[test]
    fn 非RAW仍可探测尺寸() {
        let Some(p) = sample("prime-gaming-nekomata.webp") else {
            return;
        };
        assert_eq!(probe_dimensions(&registry_with_raw(), &p).unwrap(), (7680, 4320));
    }

    #[test]
    fn 透明像素合成到白底() {
        let mut img = RgbaImage::new(2, 1).unwrap();
        img.data[0..4].copy_from_slice(&[255, 0, 0, 128]); // 半透明纯红
        img.data[4..8].copy_from_slice(&[0, 0, 0, 0]); // 全透明
        let flat = flatten_on_white(&img);
        assert_eq!(&flat.data[0..4], &[255, 127, 127, 255]);
        assert_eq!(&flat.data[4..8], &[255, 255, 255, 255]);
    }

    #[test]
    fn 探测宽高_已知样本() {
        let Some(p) = sample("prime-gaming-nekomata.webp") else {
            eprintln!("跳过：找不到样本");
            return;
        };
        let reg = registry();
        assert_eq!(probe_dimensions(&reg, &p).unwrap(), (7680, 4320));
    }

    #[test]
    fn 缩略图是JPEG且长边受限() {
        let Some(p) = sample("iloveu.webp") else {
            eprintln!("跳过：找不到样本");
            return;
        };
        let reg = registry();
        let bytes = make_thumbnail(&reg, &p, 64).unwrap();

        // 头是 JPEG SOI、尾是 EOI
        assert!(bytes.starts_with(&[0xFF, 0xD8]), "应是 JPEG（SOI）");
        assert!(bytes.ends_with(&[0xFF, 0xD9]), "应有 JPEG 结束标记（EOI）");
        assert!(bytes.len() > 100, "不应是空图");

        // 落临时文件解码回来，验证长边确实 ≤ 64
        let tmp = std::env::temp_dir().join(format!("pv_thumb_{}.jpg", std::process::id()));
        std::fs::write(&tmp, &bytes).unwrap();
        let back = reg
            .decode(&tmp, DecodeTarget::new(DecodePurpose::Export))
            .unwrap();
        let _ = std::fs::remove_file(&tmp);
        assert!(
            back.width.max(back.height) <= 64,
            "缩略图长边应 ≤ 64，实际 {}x{}",
            back.width,
            back.height
        );
    }

    #[test]
    fn 缩略图明显小于原图() {
        let Some(p) = sample("prime-gaming-nekomata.webp") else {
            eprintln!("跳过：找不到样本");
            return;
        };
        let reg = registry();
        let bytes = make_thumbnail(&reg, &p, 256).unwrap();
        let orig = std::fs::metadata(&p).unwrap().len();
        assert!(
            (bytes.len() as u64) < orig / 10,
            "256px 缩略图应远小于原图：{} vs {orig}",
            bytes.len()
        );
    }
}
