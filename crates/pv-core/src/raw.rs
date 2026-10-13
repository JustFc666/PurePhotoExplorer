//! RAW 两阶段管线接口（见开发文档 §4A）。
//!
//! **一期只实现 Stage 1（提取内嵌预览）**，但接口按两阶段设计，
//! 避免二期开发流程时重写解码层。
//!
//! 三条不可违反的约束：
//! 1. 实现方必须用 LibRaw，不用 rawler（rawler 输出未去马赛克 Bayer 阵列）
//! 2. Stage 2 输出必须是线性 f32 RGB（不是 sRGB）
//! 3. probe() 必须全量解析 EXIF，二期直接复用

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::error::{PvError, Result};

/// RAW 元数据。两阶段共用。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RawMetadata {
    pub make: String,
    pub model: String,
    pub lens: Option<String>,
    pub iso: Option<u32>,
    /// 快门时间，单位秒
    pub shutter: Option<f64>,
    pub aperture: Option<f64>,
    /// 焦距，单位 mm
    pub focal_mm: Option<f64>,
    pub width: u32,
    pub height: u32,
    /// 原始格式标识，如 "NEF" / "ARW" / "CR3" / "DNG"
    pub raw_format: String,
    pub bit_depth: u8,
    /// 传感器原生色彩空间
    pub color_space: RawColorSpace,
    /// 是否含内嵌预览（决定 Stage 1 走哪条路径）
    pub has_embedded_preview: bool,
    /// 拍摄时间
    pub captured_at: Option<chrono_like::DateTime>,
}

/// 轻量时间类型，避免为 pv-core 引入 chrono 依赖
pub mod chrono_like {
    /// Unix 时间戳（秒）
    pub type DateTime = i64;
}

/// 传感器色彩空间（Stage 2 的输入前提）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RawColorSpace {
    #[default]
    Unknown,
    Srgb,
    AdobeRgb,
    ProPhotoRgb,
    /// 相机原生（可能是 Canon ProPhoto / Nikon Wide / Sony Wide 等）
    CameraNative,
}

/// 线性 RGB 图像。Stage 2 的输出类型。
///
/// **刻意不做色彩空间转换**——转换交给上层 `pv-edit` 的渲染层，
/// 保证两阶段在"线性域处理"节点汇合（见文档 §4A.4）。
#[derive(Debug, Clone)]
pub struct LinearImage {
    pub width: u32,
    pub height: u32,
    /// 3 通道 float32，行优先，长度 = width * height * 3
    pub data: Vec<f32>,
    pub color_space: RawColorSpace,
}

impl LinearImage {
    pub fn new(width: u32, height: u32, color_space: RawColorSpace) -> Result<Self> {
        let len = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(3))
            .ok_or_else(|| PvError::Decode("图像尺寸溢出".into()))?;
        Ok(Self {
            width,
            height,
            data: vec![0.0; len],
            color_space,
        })
    }

    #[inline]
    pub fn pixel(&self, x: u32, y: u32) -> Option<[f32; 3]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = ((y as usize) * (self.width as usize) + x as usize) * 3;
        Some([self.data[i], self.data[i + 1], self.data[i + 2]])
    }
}

/// Stage 1 产物：相机已渲染好的预览图
#[derive(Debug, Clone)]
pub enum EncodedPreview {
    /// 相机内嵌的 JPEG 原样拷贝 —— **零解码成本，最快路径**
    Jpeg(Vec<u8>),
    /// 已解码的 RGBA
    Rgba {
        width: u32,
        height: u32,
        data: Vec<u8>,
    },
    /// 未识别预览格式，降级为像素
    Unknown(Vec<u8>),
}

impl EncodedPreview {
    /// 用于生成缩略图：JPEG 走解码，Rgba 直接缩放
    pub fn source_kind(&self) -> &'static str {
        match self {
            Self::Jpeg(_) => "jpeg",
            Self::Rgba { .. } => "rgba",
            Self::Unknown(_) => "unknown",
        }
    }
}

/// 白平衡模式
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum WbMode {
    /// 使用相机拍摄时的白平衡（ShotNG / AsShot）
    #[default]
    AsShot,
    /// 相机自动白平衡
    Auto,
    /// 相机标定白平衡
    Camera,
    /// 手动指定色温与色调
    Manual { kelvin: f32, tint: f32 },
}

/// 去马塞克算法
///
/// **许可注意**：LibRaw 部分 demosaic 实现（DCB、GPL 打包）受 GPL 约束，
/// 商业闭源发布应只用 AHD / VNG / PPG（见文档风险 R-03）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DemosaicAlgo {
    #[default]
    Ahd,
    Vng,
    Ppg,
    /// 仅在明确接受 GPL 传染风险时使用
    Dcb,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum HighlightMode {
    #[default]
    Clip,
    Soft,
    Hard,
}

/// 二期开发参数。一期不使用，但结构固定。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RawDevelopParams {
    pub wb_mode: WbMode,
    pub demosaic: DemosaicAlgo,
    pub highlight_mode: HighlightMode,
    /// 曝光补偿，单位 EV
    pub exposure: f32,
    pub contrast: f32,
    pub shadow_lift: f32,
    pub highlight_rolloff: f32,
    /// 手动去马赛克平铺宽度
    pub user_flip: Option<u32>,
    pub output_bps: u8,
}

/// RAW 管线抽象。
///
/// 实现方**必须**基于 LibRaw（理由见文档 §4A.1 约束 1）。
///
/// `Send + Sync` 是必需约束：Tauri 命令在多线程运行时执行，
/// 注册表是全局共享状态。LibRaw 的 C++ 对象通常不满足，
/// 实现方需用 `Mutex<LibRawHandle>` 包装。
pub trait RawPipeline: Send + Sync {
    /// 相机型号标识
    fn name(&self) -> &'static str;

    /// Stage 1 快路径：提取内嵌预览（不解码像素，5-15 ms）
    fn extract_preview(&self, path: &Path) -> Result<EncodedPreview>;

    /// Stage 1 降级路径：half-size 去马赛克（200-500 ms）
    fn extract_preview_slow(&self, path: &Path) -> Result<EncodedPreview>;

    /// Stage 2：完整开发流程。
    /// **一期必须返回 `Err(PvError::RawDevelopNotImplemented)`**
    fn develop(&self, _path: &Path, _params: &RawDevelopParams) -> Result<LinearImage> {
        Err(PvError::RawDevelopNotImplemented)
    }

    /// 元数据探测（全量 EXIF，两阶段共用）
    fn probe(&self, path: &Path) -> Result<RawMetadata>;

    /// 本实现支持的相机型号列表（设置页展示覆盖范围）
    fn supported_cameras(&self) -> Result<Vec<String>>;
}

/// 尚未接入 LibRaw 时的占位实现。
///
/// 存在的意义：让上层在 M1 就能完整跑通链路，
/// 而不是等到 M2 才有任何可运行的东西。
pub struct UnavailablePipeline {
    pub reason: String,
}

impl RawPipeline for UnavailablePipeline {
    fn name(&self) -> &'static str {
        "unavailable"
    }

    fn extract_preview(&self, _path: &Path) -> Result<EncodedPreview> {
        Err(PvError::UnsupportedFormat(format!(
            "RAW 解码器未接入: {}",
            self.reason
        )))
    }

    fn extract_preview_slow(&self, _path: &Path) -> Result<EncodedPreview> {
        Err(PvError::UnsupportedFormat(format!(
            "RAW 解码器未接入: {}",
            self.reason
        )))
    }

    fn probe(&self, _path: &Path) -> Result<RawMetadata> {
        Err(PvError::UnsupportedFormat(format!(
            "RAW 解码器未接入: {}",
            self.reason
        )))
    }

    fn supported_cameras(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
}

/// 便于上层统一处理"先快后慢"的降级逻辑
pub fn extract_preview_with_fallback<P: RawPipeline + ?Sized>(
    p: &P,
    path: &Path,
) -> Result<EncodedPreview> {
    match p.extract_preview(path) {
        Ok(v) => Ok(v),
        // 内嵌预览缺失或失败 → 降级到 half-size 解码
        Err(PvError::Corrupt(_)) | Err(PvError::Decode(_)) | Err(PvError::UnsupportedFormat(_)) => {
            tracing::debug!(file = %path_basename(path), "内嵌预览失败，降级到 half-size 解码");
            p.extract_preview_slow(path)
        }
        Err(e) => Err(e),
    }
}

/// 日志脱敏：只取文件名，不记完整路径（见文档 §9.3 日志默认脱敏）
fn path_basename(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "<unknown>".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn LinearImage_可安全构造() {
        let img = LinearImage::new(10, 10, RawColorSpace::AdobeRgb).unwrap();
        assert_eq!(img.data.len(), 300);
        assert_eq!(img.pixel(0, 0), Some([0.0, 0.0, 0.0]));
        assert_eq!(img.pixel(10, 0), None, "越界应返回 None 而非 panic");
        assert_eq!(img.pixel(0, 10), None);
    }

    #[test]
    fn LinearImage_尺寸溢出报错不panic() {
        let r = LinearImage::new(u32::MAX, u32::MAX, RawColorSpace::Srgb);
        assert!(r.is_err(), "超大尺寸应返回 Err 而非尝试分配");
    }

    #[test]
    fn 占位实现_所有方法返回明确错误() {
        let p = UnavailablePipeline {
            reason: "测试".into(),
        };
        let path = PathBuf::from("x.nef");
        assert!(matches!(
            p.extract_preview(&path),
            Err(PvError::UnsupportedFormat(_))
        ));
        assert!(matches!(p.probe(&path), Err(PvError::UnsupportedFormat(_))));
        assert!(p.supported_cameras().unwrap().is_empty());
    }

    #[test]
    fn develop_默认实现返回未实现而非panic() {
        let p = UnavailablePipeline {
            reason: "测试".into(),
        };
        let r = p.develop(Path::new("x.nef"), &RawDevelopParams::default());
        assert!(matches!(r, Err(PvError::RawDevelopNotImplemented)));
    }
}
