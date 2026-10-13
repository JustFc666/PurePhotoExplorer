//! 统一解码接口。
//!
//! **这个 trait 是整个项目最重要的预留点**（见开发文档 §13 开发顺序规划）。
//!
//! 上层（单图浏览 / 资源管理器 / 编辑 / 导出）只依赖 [`ImageDecoder`]，
//! 不依赖任何具体解码实现。因此后续接入 LibRaw / libheif 时
//! **只需新增实现，不改上层一行代码**。
//!
//! 与 `pv_core::raw::RawPipeline` 的分工：
//! - [`ImageDecoder`]：常规位图（JPEG/PNG/WebP/…）+ 统一调度
//! - `RawPipeline`：相机 RAW 的两阶段专用流程（见 §4A）
//!
//! ## 子模块
//! - [`rgba`]     解码产物 `RgbaImage`
//! - [`target`]   解码目标尺寸 `DecodeTarget` / `DecodePurpose` + `fit_within`
//! - [`info`]     探测结果 `ImageInfo`
//! - [`traits`]   统一解码 trait `ImageDecoder`
//! - [`registry`] 后端注册表 `DecoderRegistry`

mod info;
mod registry;
mod rgba;
mod target;
mod traits;

pub use info::ImageInfo;
pub use registry::DecoderRegistry;
pub use rgba::RgbaImage;
pub use target::{fit_within, DecodePurpose, DecodeTarget};
pub use traits::ImageDecoder;
