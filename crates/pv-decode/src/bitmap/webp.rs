//! WebP 解码（有损 VP8 / 无损 VP8L / **动画**）。
//!
//! ## 为什么选 `image-webp`
//! 它是 image-rs 官方的纯 Rust 实现，**零依赖**（`cargo tree` 下没有别的包），
//! 支持有损、无损、alpha、以及**动画**。相比两条替代路线：
//! - `libwebp-sys`：要编 C 库（本机无 cmake）；
//! - `webp` crate：同样绑 libwebp。
//!
//! ## 动画为什么能直接复用动图管线
//! `WebPDecoder::read_frame` **内部已经做了增量合成**（alpha 混合 + `dispose`
//! 清理 + 背景色回填），返回的是「贴完这一帧之后的完整画面」，
//! 与 [`super::gif`] 的输出语义一致 —— 所以两者共用 [`super::anim`] 的类型，
//! 上层（PVA1 打包、渐进加载、播放器）完全不用区分格式。
//!
//! ## 尺寸上限（必须自己做）
//! 文件头可以把画布声明成 16383×16383，`output_buffer_size()` 就是 1GB ——
//! **库的 `set_memory_limit` 只管 ICC/EXIF/XMP，不管像素分配**（源码里写明了
//! "Some allocations currently ignore this limit"）。所以分配前必须自己挡。

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use image_webp::{DecodingError, LoopCount, WebPDecoder};
use pv_core::error::{PvError, Result};
use pv_core::format::ImageFormat;

use super::anim::{
    Animation, AnimationFrame, DEFAULT_DELAY_MS, MAX_FRAMES, MAX_TOTAL_BYTES,
};
use super::io_err;
use super::resize::resize_rgba;
use crate::decoder::{fit_within, DecodeTarget, ImageInfo, RgbaImage};

/// 单帧输出缓冲的上限（256 MB ≈ 8192×8192×4）。
///
/// 本项目的样本就是 7680×4320 的 8K 无损图（RGBA 126MB），必须容得下；
/// 而声明成 16K 画布的恶意文件（1GB）要在**分配之前**就被挡住。
const MAX_FRAME_BYTES: usize = 256 * 1024 * 1024;

/// 打开并建好解码器（顺带设好非像素类分配的上限）。
fn open(path: &Path) -> Result<WebPDecoder<BufReader<File>>> {
    let f = File::open(path).map_err(io_err(path))?;
    let mut dec = WebPDecoder::new(BufReader::new(f))
        .map_err(|e| PvError::Decode(format!("WebP 头解析失败：{e}")))?;
    // 只管 ICC/EXIF/XMP 的读取量，像素分配由下面的 alloc_buffer 把关
    dec.set_memory_limit(MAX_FRAME_BYTES);
    Ok(dec)
}

/// 按声明的画布尺寸分配输出缓冲，超限直接拒绝（不让 `vec!` 去 OOM）。
fn alloc_buffer(dec: &WebPDecoder<BufReader<File>>) -> Result<Vec<u8>> {
    let need = dec
        .output_buffer_size()
        .ok_or_else(|| PvError::Decode("WebP 画布尺寸溢出（疑似损坏文件）".into()))?;
    if need > MAX_FRAME_BYTES {
        return Err(PvError::Decode(format!(
            "WebP 画布过大：需要 {} MB，超过上限 {} MB",
            need / 1048576,
            MAX_FRAME_BYTES / 1048576
        )));
    }
    Ok(vec![0u8; need])
}

/// 把 `image-webp` 的输出（RGB 或 RGBA）统一成 RGBA。
///
/// 长度对齐 4 通道时直接接管所有权（不复制）；否则逐像素补 alpha=255。
fn into_rgba(width: u32, height: u32, buf: Vec<u8>) -> RgbaImage {
    let pixels = width as usize * height as usize;
    if buf.len() == pixels * 4 {
        return RgbaImage {
            width,
            height,
            data: buf,
        };
    }
    let mut data = Vec::with_capacity(pixels * 4);
    for px in buf.chunks_exact(3) {
        data.extend_from_slice(&[px[0], px[1], px[2], 255]);
    }
    RgbaImage {
        width,
        height,
        data,
    }
}

/// 按目标尺寸缩放（只在需要时调，避免无谓的整幅拷贝）
fn scale(img: RgbaImage, target: DecodeTarget) -> Result<RgbaImage> {
    let (tw, th) = fit_within(img.width, img.height, target.max_edge);
    if (tw, th) == (img.width, img.height) {
        Ok(img)
    } else {
        resize_rgba(&img, tw, th)
    }
}

/// 探测 WebP 尺寸与属性。
///
/// ⚠️ **本函数只读头，不解码像素** —— 与 [`super::heif::probe_heif`] 那种
/// 「库没暴露轻量接口、被迫全解」的情况不同。实测 8MB 样本 **138µs**。
pub(super) fn probe_webp(path: &Path) -> Result<ImageInfo> {
    let dec = open(path)?;
    let (w, h) = dec.dimensions();
    if w == 0 || h == 0 {
        return Err(PvError::Decode("WebP 画布尺寸为 0".into()));
    }
    Ok(ImageInfo {
        format: ImageFormat::WebP,
        width: w,
        height: h,
        // EXIF 在 RIFF 的 `EXIF` 块里，不是 JPEG 的 APP1 段。
        // 解析它要另写一套容器定位，本期不做（信息面板显示空即可）。
        exif: None,
        raw: None,
        // 有损/无损都是 sRGB；ICC 块未解析，所以不谎称更精确的色彩空间
        color_space: Some("sRGB".to_string()),
    })
}

/// 解码 WebP 的**第一帧**（静态图入口）。
///
/// `read_image` 内部对动画文件也只取第一帧（源码里临时把动画游标拨到首帧
/// 再恢复），所以这个入口对动画 WebP 同样安全。
pub(super) fn decode_webp(path: &Path, target: DecodeTarget) -> Result<RgbaImage> {
    let mut dec = open(path)?;
    let (w, h) = dec.dimensions();
    if w == 0 || h == 0 {
        return Err(PvError::Decode("WebP 画布尺寸为 0".into()));
    }

    let mut buf = alloc_buffer(&dec)?;
    dec.read_image(&mut buf)
        .map_err(|e| PvError::Decode(format!("WebP 解码失败：{e}")))?;

    scale(into_rgba(w, h, buf), target)
}

/// 解码 WebP 的**全部帧**。
///
/// 静态 WebP 也走这里（返回只有一帧的 `Animation`）—— 这样上层按格式分发时
/// 不必再判断「是不是动图」，`is_static()` 自然为 `true`，
/// 前端播放器就会把它当普通图片处理。
pub fn decode_webp_animation(path: &Path, target: DecodeTarget) -> Result<Animation> {
    let mut dec = open(path)?;
    let (w, h) = dec.dimensions();
    if w == 0 || h == 0 {
        return Err(PvError::Decode("WebP 画布尺寸为 0".into()));
    }

    // 帧像素尺寸（= 缩放后）—— `Animation.width/height` 必须是这个，
    // 上层按它算 stride / 分块预算，报原始画布尺寸会让包长对不上。
    let (fw, fh) = fit_within(w, h, target.max_edge);

    // 静态图（含「只有一帧的动画」）：一次解码搞定，不做帧循环
    if !dec.is_animated() || dec.num_frames() <= 1 {
        let mut buf = alloc_buffer(&dec)?;
        dec.read_image(&mut buf)
            .map_err(|e| PvError::Decode(format!("WebP 解码失败：{e}")))?;
        return Ok(Animation {
            width: fw,
            height: fh,
            frames: vec![AnimationFrame {
                delay_ms: DEFAULT_DELAY_MS,
                image: scale(into_rgba(w, h, buf), target)?,
            }],
            truncated: false,
            loop_count: None,
        });
    }

    // 无限循环 → `None`；`Times(n)` → `Some(n)`。
    // ⚠️ 两边对「次数」的定义略有差异（GIF 的 repeat 是**额外**重复次数，
    // WebP 的 `loop_count` 是总轮数），但本项目前端只用它判断「是否无限」，
    // 不按精确次数控制播放，所以不做 +1/-1 的换算。
    let loop_count = match dec.loop_count() {
        LoopCount::Forever => None,
        LoopCount::Times(n) => Some(n.get()),
    };

    // 复用同一块缓冲：`read_frame` 每帧都会整幅覆写
    let mut buf = alloc_buffer(&dec)?;
    let mut frames: Vec<AnimationFrame> = Vec::new();
    let mut total_bytes = 0usize;
    let mut truncated = false;

    loop {
        if frames.len() >= MAX_FRAMES || total_bytes >= MAX_TOTAL_BYTES {
            truncated = true;
            break;
        }

        let delay = match dec.read_frame(&mut buf) {
            Ok(ms) => ms,
            // 正常结束：没有更多帧了
            Err(DecodingError::NoMoreFrames) => break,
            Err(e) => return Err(PvError::Decode(format!("WebP 帧解码失败：{e}"))),
        };

        // 合成结果就在 buf 里（库已处理 alpha 混合与 dispose），拷一份快照
        let image = scale(into_rgba(w, h, buf.clone()), target)?;
        total_bytes += image.data.len();

        frames.push(AnimationFrame {
            delay_ms: if delay == 0 { DEFAULT_DELAY_MS } else { delay },
            image,
        });
    }

    if frames.is_empty() {
        return Err(PvError::Decode("WebP 不含可解码的帧".to_string()));
    }

    Ok(Animation {
        width: fw,
        height: fh,
        frames,
        truncated,
        loop_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::DecodePurpose;
    use std::path::PathBuf;

    /// 取仓库 `test-image/` 下的样本（小于 4KB 视为 Git LFS 指针，不算）
    fn sample(name: &str) -> Option<PathBuf> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-image");
        assert!(
            dir.is_dir(),
            "找不到 test-image（推算 {}）",
            dir.display()
        );
        let p = dir.join(name);
        if p.is_file() && std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0) > 4096 {
            Some(p)
        } else {
            None
        }
    }

    const SAMPLE: &str = "prime-gaming-nekomata.webp";

    #[test]
    fn 真实WebP探测尺寸() {
        let Some(p) = sample(SAMPLE) else {
            eprintln!("跳过：找不到 WebP 样本");
            return;
        };
        let t = std::time::Instant::now();
        let info = probe_webp(&p).unwrap();
        let dt = t.elapsed();

        assert_eq!((info.width, info.height), (7680, 4320));
        assert_eq!(info.format, ImageFormat::WebP);
        assert!(!info.is_raw(), "WebP 不是 RAW");
        // 探测必须**不解码像素**：全解要 ~187ms，只读头是亚毫秒级
        assert!(
            dt.as_millis() < 50,
            "探测应只读头，实测 {dt:?} —— 是不是退化成全解码了？"
        );
        eprintln!("WebP 探测（只读头）{dt:?}");
    }

    #[test]
    fn 真实WebP解码出正确像素() {
        let Some(p) = sample(SAMPLE) else {
            eprintln!("跳过：找不到 WebP 样本");
            return;
        };
        let t = std::time::Instant::now();
        // Export = 原图，不缩放，才能验证真实解码尺寸
        let img = decode_webp(&p, DecodeTarget::new(DecodePurpose::Export)).unwrap();
        let dt = t.elapsed();

        assert_eq!((img.width, img.height), (7680, 4320));
        assert_eq!(img.data.len(), 7680 * 4320 * 4, "应是 RGBA");
        // 这张图是带透明通道的素材：四角必定透明，中心必定有内容。
        // 靠这两点能同时排除「解成全黑」「alpha 丢了」「上下/左右颠倒」。
        let px = |x: usize, y: usize| {
            let o = (y * 7680 + x) * 4;
            [img.data[o], img.data[o + 1], img.data[o + 2], img.data[o + 3]]
        };
        assert_eq!(px(10, 10)[3], 0, "左上角应完全透明");
        assert_eq!(px(7670, 4310)[3], 0, "右下角应完全透明");
        let center = px(3840, 2160);
        assert_eq!(center[3], 255, "中心应不透明");
        assert!(
            center.iter().take(3).any(|&c| c > 16),
            "中心不是黑色内容：{center:?}"
        );
        eprintln!("WebP 解码 7680×4320 用时 {dt:?}");
    }

    #[test]
    fn WebP按目标尺寸缩放() {
        let Some(p) = sample(SAMPLE) else {
            eprintln!("跳过：找不到 WebP 样本");
            return;
        };
        // 预览档 max_edge = 2048：7680×4320 应等比缩到 2048×1152
        let img = decode_webp(&p, DecodeTarget::new(DecodePurpose::Preview)).unwrap();
        assert_eq!((img.width, img.height), (2048, 1152));
        assert_eq!(img.data.len(), 2048 * 1152 * 4);
    }

    /// 静态 WebP 走动画入口时应返回**单帧**结果，前端才能按普通图片处理。
    #[test]
    fn 静态WebP走动画入口返回单帧() {
        let Some(p) = sample(SAMPLE) else {
            eprintln!("跳过：找不到 WebP 样本");
            return;
        };
        let anim = decode_webp_animation(&p, DecodeTarget::new(DecodePurpose::Preview)).unwrap();
        assert_eq!(anim.frames.len(), 1, "静态 WebP 应只有一帧");
        assert!(anim.is_static());
        assert!(!anim.truncated);
        // ⚠️ `width/height` 是**帧像素**尺寸（PVA1 靠它算 stride），不是原始画布
        assert_eq!((anim.width, anim.height), (2048, 1152), "尺寸应是帧像素尺寸");
        assert_eq!(anim.frames[0].delay_ms, DEFAULT_DELAY_MS);
    }

    #[test]
    fn 非WebP文件报错而不是panic() {
        let dir = std::env::temp_dir().join("pv-webp-test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("not.webp");
        std::fs::write(&p, b"this is definitely not a webp file").unwrap();

        assert!(probe_webp(&p).is_err(), "非 WebP 必须返回错误");
        assert!(decode_webp(&p, DecodeTarget::new(DecodePurpose::Preview)).is_err());
        assert!(decode_webp_animation(&p, DecodeTarget::new(DecodePurpose::Preview)).is_err());
    }

    #[test]
    fn 空文件与截断文件不panic() {
        let dir = std::env::temp_dir().join("pv-webp-test");
        std::fs::create_dir_all(&dir).unwrap();

        let empty = dir.join("empty.webp");
        std::fs::write(&empty, b"").unwrap();
        assert!(probe_webp(&empty).is_err());

        // 只有 RIFF/WEBP 签名，没有后续块
        let stub = dir.join("stub.webp");
        let mut b = b"RIFF".to_vec();
        b.extend_from_slice(&4u32.to_le_bytes());
        b.extend_from_slice(b"WEBP");
        std::fs::write(&stub, &b).unwrap();
        assert!(probe_webp(&stub).is_err());
    }
}
