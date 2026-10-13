//! PurePhotoExplorer 解码抽象层。
//!
//! **本 crate 是格式接入的唯一收口**：上层只依赖 `ImageDecoder` trait，
//! 新增格式（LibRaw / libheif / Wuffs）只需实现 trait，不改上层。
//!
//! 一期落地纯 Rust 后端（JPEG / PNG / GIF / WebP / HEIC / PSD），
//! 无需 MSVC 工具链即可跑通单图浏览。
//!
//! RAW 走 `raw` 子模块的纯 Rust 管线（提取内嵌预览），
//! 理由与取舍见该模块文档。

// 测试函数名用中文（可读性优先），关闭 snake_case 检查
#![cfg_attr(test, allow(non_snake_case))]

use std::path::Path;

use pv_core::error::PvError;

pub mod bitmap;
pub mod decoder;
pub mod encode;
pub mod raw;
pub mod transform;

pub use bitmap::{
    decode_animation, decode_gif, decode_webp_animation, Animation, AnimationFrame, BitmapBackend,
};
pub use decoder::{
    fit_within, DecodePurpose, DecodeTarget, DecoderRegistry, ImageDecoder, ImageInfo, RgbaImage,
};
pub use encode::{
    can_encode, can_overwrite_original, encode_rgba, encode_rgba_with_exif, format_from_extension,
    not_writable_reason, write_rgba_atomic, write_rgba_atomic_with_exif,
};
pub use raw::EmbeddedPreviewPipeline;
pub use transform::{
    crop_normalized, crop_to_aspect, rotate180, rotate90_ccw, rotate90_cw, rotate90_steps,
};

/// 把 io 错误转成领域错误（带路径上下文）。
///
/// 放在 crate 根：解码（`bitmap`）与编码（`encode`）都要用，
/// 归到任一侧都会让另一侧出现「绕路引用」。
pub(crate) fn io_err(path: &Path) -> impl Fn(std::io::Error) -> PvError {
    let p = path.to_path_buf();
    move |e| match e.kind() {
        std::io::ErrorKind::NotFound => PvError::NotFound(p.clone()),
        std::io::ErrorKind::PermissionDenied => PvError::PermissionDenied(p.clone()),
        _ => PvError::Io {
            path: p.clone(),
            source: e,
        },
    }
}
