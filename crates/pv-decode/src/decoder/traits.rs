//! 统一解码 trait。

use std::path::Path;

use pv_core::error::Result;
use pv_core::format::ImageFormat;

use super::info::ImageInfo;
use super::rgba::RgbaImage;
use super::target::DecodeTarget;

/// 统一解码接口。
///
/// **实现方不得把具体库的类型泄漏到签名里**，
/// 这样上层与具体解码器完全解耦。
///
/// `Send + Sync` 是必需约束：Tauri 的命令在多线程运行时执行，
/// 而解码器注册表是全局共享状态。LibRaw 这类 C++ 库若不满足，
/// 需用 `Mutex` 包装后再传入。
pub trait ImageDecoder: Send + Sync {
    /// 后端名称，用于日志与设置页展示
    fn backend_name(&self) -> &'static str;

    /// 该后端能处理的格式
    fn supports(&self, format: ImageFormat) -> bool;

    /// 探测文件信息（不解码像素，通常 < 5 ms）
    fn probe(&self, path: &Path) -> Result<ImageInfo>;

    /// 按目标尺寸解码
    fn decode(&self, path: &Path, target: DecodeTarget) -> Result<RgbaImage>;

    /// 该后端能解码的相机型号（RAW 后端实现）
    fn supported_cameras(&self) -> Option<Vec<String>> {
        None
    }
}
