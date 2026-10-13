//! PurePhotoExplorer 核心领域模型
//!
//! **本 crate 是纯逻辑层：不碰文件 IO、不碰图像解码、不依赖任何原生库。**
//! 这样做的目的：
//! - 领域规则可单元测试，无需启动整个应用
//! - 上层（Tauri / 前端）不依赖 LibRaw 等 FFI 细节
//! - RAW 两阶段接口在此定死，实现方（M2 的 LibRaw 绑定）只能填空，不能改语义

// 测试函数名用中文（可读性优先），关闭 snake_case 检查
#![cfg_attr(test, allow(non_snake_case))]

pub mod error;
pub mod format;
pub mod raw;

pub use error::{PvError, Result};
pub use format::{probe_header, FormatProbe, FormatTier, ImageFormat, RawVariant};
pub use raw::{
    DemosaicAlgo, EncodedPreview, HighlightMode, LinearImage, RawColorSpace, RawDevelopParams,
    RawMetadata, RawPipeline, UnavailablePipeline, WbMode,
};
