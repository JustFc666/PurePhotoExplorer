//! 纯 Rust 常规位图后端（JPEG / PNG / GIF / WebP / HEIC / PSD / BMP / TIFF / AVIF）。
//!
//! **为什么先做这个**：它不需要 MSVC 工具链，
//! 因此 S1 单图浏览阶段完全不被 R-00 阻断。
//!
//! ## 子模块
//! - [`probe`]   格式判定与元信息探测（SOF 解析 / PNG 头 / GIF 逻辑屏幕）
//! - [`jpeg`]    JPEG 解码（zune-jpeg）
//! - [`png`]     PNG 解码
//! - [`anim`]    动图的**通用类型与资源上限**（GIF / 动画 WebP 共用）
//! - [`gif`]     GIF 解码（含**动图多帧合成**）
//! - [`webp`]    WebP 解码（image-webp：有损/无损/动画，**零依赖纯 Rust**）
//! - [`heif`]    HEIC 解码（heif-oxide + rust_h265，**全纯 Rust**）
//! - [`psd`]     PSD/PSB 解码（**手写**：解的是文件尾部的"合成图"）
//! - [`bmp`]     BMP 解码（**手写**：1/4/8/16/24/32 位 + 调色板 + RLE + 位域掩码）
//! - [`tiff`]    TIFF 解码（image-rs 官方 `tiff` crate，纯 Rust）
//! - [`avif`]    AVIF 解码（avif-decode → rav1d，纯 Rust，需 nasm）
//! - [`convert`] 各色彩空间 → RGBA
//! - [`resize`]  SIMD 缩放

mod anim;
mod avif;
mod bmp;
mod convert;
mod gif;
mod heif;
mod jpeg;
mod png;
mod probe;
mod psd;
mod resize;
mod tiff;
mod webp;

use std::path::Path;

use pv_core::error::{PvError, Result};
use pv_core::format::ImageFormat;

use crate::decoder::{DecodeTarget, ImageDecoder, ImageInfo, RgbaImage};

/// 动图解码入口（供 IPC 层取所有帧）——格式在这里收口。
///
/// 新增动图格式时**只改这个函数**：上层（命令层 / PVA1 / 播放器）拿到的
/// 永远是同一套 [`Animation`]，不必知道它来自 GIF 还是 WebP。
pub use anim::{Animation, AnimationFrame};
pub use gif::decode_gif;
pub use webp::decode_webp_animation;
// TIFF / AVIF 解码入口。TIFF 特别在注册表（`decoder/registry.rs`）里被
// 当作「RAW 管线对 TIFF 容器失败」的降级目标引用；AVIF 走正常后端路由，
// 这里只是把入口收拢到 `bitmap` 根，与 GIF/WebP 一致。
pub(crate) use tiff::{decode_tiff, probe_tiff};

/// 按文件头分发到对应格式的动图解码器。
///
/// 静图格式也会走这里（返回单帧 `Animation`）—— 命令层不必先探测再分支。
pub fn decode_animation(path: &Path, target: DecodeTarget) -> Result<Animation> {
    match probe::probe_format(path)? {
        ImageFormat::WebP => webp::decode_webp_animation(path, target),
        // GIF 是默认分支：`decode_gif` 自己会校验魔数并在格式不符时报错
        _ => gif::decode_gif(path, target),
    }
}

/// 从内存字节解码 JPEG，供 RAW 内嵌预览复用。
///
/// 内嵌预览没有独立文件路径，只能走字节入口；
/// 其余解码逻辑与文件版完全共用（见 `bitmap::jpeg`）。
pub fn decode_jpeg_from_bytes(data: &[u8], target: DecodeTarget) -> Result<RgbaImage> {
    jpeg::decode_jpeg_bytes(data, target)
}

/// 纯 Rust 位图后端
pub struct BitmapBackend;

impl ImageDecoder for BitmapBackend {
    fn backend_name(&self) -> &'static str {
        "bitmap-pure-rust"
    }

    fn supports(&self, format: ImageFormat) -> bool {
        matches!(
            format,
            ImageFormat::Jpeg
                | ImageFormat::Png
                | ImageFormat::Gif
                | ImageFormat::WebP
                | ImageFormat::Heif
                | ImageFormat::Psd
                | ImageFormat::Bmp
                | ImageFormat::Tiff
                | ImageFormat::Avif
        )
    }

    fn probe(&self, path: &Path) -> Result<ImageInfo> {
        match probe::probe_format(path)? {
            ImageFormat::Jpeg => probe::probe_jpeg(path),
            ImageFormat::Png => probe::probe_png(path),
            ImageFormat::Gif => gif::probe_gif(path),
            ImageFormat::WebP => webp::probe_webp(path),
            ImageFormat::Heif => heif::probe_heif(path),
            ImageFormat::Psd => psd::probe_psd(path),
            ImageFormat::Bmp => bmp::probe_bmp(path),
            ImageFormat::Tiff => tiff::probe_tiff(path),
            ImageFormat::Avif => avif::probe_avif(path),
            other => Err(PvError::UnsupportedFormat(format!(
                "本后端暂不支持 {}",
                other.display_name()
            ))),
        }
    }

    fn decode(&self, path: &Path, target: DecodeTarget) -> Result<RgbaImage> {
        // 一律按文件头判定，不看扩展名（FR-1.5）
        match probe::probe_format(path)? {
            ImageFormat::Jpeg => jpeg::decode_jpeg(path, target),
            ImageFormat::Png => png::decode_png(path, target),
            // 静态入口只取第一帧；动图播放走 `decode_animation`（IPC 的 decode_animation）
            ImageFormat::Gif => gif::decode_gif(path, target).map(|a| {
                a.frames
                    .into_iter()
                    .next()
                    .map(|f| f.image)
                    .expect("decode_gif 保证至少一帧")
            }),
            ImageFormat::WebP => webp::decode_webp(path, target),
            ImageFormat::Heif => heif::decode_heif(path, target),
            ImageFormat::Psd => psd::decode_psd(path, target),
            ImageFormat::Bmp => bmp::decode_bmp(path, target),
            ImageFormat::Tiff => tiff::decode_tiff(path, target),
            ImageFormat::Avif => avif::decode_avif(path, target),
            other => Err(PvError::UnsupportedFormat(format!(
                "本后端暂不支持 {}",
                other.display_name()
            ))),
        }
    }
}

/// 把 io 错误转成领域错误（带路径上下文）。
///
/// 实现在 crate 根（编码模块也要用），此处 re-export 保持子模块内
/// `use super::io_err` 的原有写法不变。
pub(super) use crate::io_err;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 后端声明的支持范围与实现一致() {
        let b = BitmapBackend;
        for f in [
            ImageFormat::Jpeg,
            ImageFormat::Png,
            ImageFormat::Gif,
            ImageFormat::WebP,
            ImageFormat::Heif,
            ImageFormat::Psd,
            ImageFormat::Bmp,
            ImageFormat::Tiff,
            ImageFormat::Avif,
        ] {
            assert!(b.supports(f), "{} 应由本后端承担", f.display_name());
        }
        assert!(!b.supports(ImageFormat::SonyArw), "RAW 不归本后端管");
    }
}
