//! 单文件的探测结果 + 元数据。

use pv_core::format::ImageFormat;
use pv_core::raw::RawMetadata;

/// 单个文件的探测结果 + 元数据
#[derive(Debug, Clone, Default)]
pub struct ImageInfo {
    pub format: ImageFormat,
    pub width: u32,
    pub height: u32,
    /// EXIF / XMP 原始文本（若有）
    pub exif: Option<String>,
    /// RAW 专用元数据（非常规格式为 None）
    pub raw: Option<RawMetadata>,
    /// 色彩空间标识字符串
    pub color_space: Option<String>,
}

impl ImageInfo {
    pub fn is_raw(&self) -> bool {
        self.format.is_raw()
    }
}
