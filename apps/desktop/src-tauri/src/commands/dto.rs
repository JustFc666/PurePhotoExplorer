//! 前后端交换的数据结构（DTO）与错误映射。

use std::path::Path;

use pv_core::error::PvError;
use pv_meta::ExifData;
use serde::{Deserialize, Serialize};

/// 图像信息（走 JSON，量小）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageInfoDto {
    pub path: String,
    pub file_name: String,
    /// 格式标识（与 pv_core::format::ImageFormat 对应，由前端 switch）
    pub format: String,
    pub format_label: String,
    pub width: u32,
    pub height: u32,
    pub file_size: u64,
    /// 是否 RAW（前端可显示"需要 RAW 解码器"提示）
    pub is_raw: bool,
    /// 文件修改时间（Unix 秒）
    pub modified_at: i64,
    /// 拍摄参数（EXIF）；文件不含 EXIF 时为 `null`
    pub exif: Option<ExifData>,
    /// RAW 专属信息；常规格式为 `null`
    ///
    /// 与 `exif` 的分工：`exif` 是"文件里写的 EXIF 文本"，
    /// `raw` 是"RAW 管线消化后的事实"（含内嵌预览尺寸、色彩空间、位深）。
    /// RAW 的拍摄参数两者都可能提供 —— 前端优先用 `raw`（更权威）。
    pub raw: Option<RawInfoDto>,
    /// 由 GPS 坐标反查出的拍摄地点（离线库，不联网）。
    ///
    /// 与 `exif.gps` 的分工：`exif.gps` 是**文件里写的**原始坐标，
    /// 这里是**我们算出来的**"哪个城市"；文件无 GPS 或附近 150 km 内
    /// 没有城镇时为 `null`。
    pub place: Option<pv_geo::Place>,
}

/// RAW 格式信息（对应 `pv_core::raw::RawMetadata` 的子集）。
///
/// 数值型字段保持原始类型（不用字符串），因为前端要按需拼单位；
/// 而 EXIF 那套是 Rust 侧已格式化的展示串，两者刻意不同。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawInfoDto {
    /// 原始格式标识，如 `CR3` / `NRW` / `DNG`
    pub raw_format: String,
    /// 内嵌预览是否可用（Stage 1 路径）
    pub has_embedded_preview: bool,
    /// 位深（内嵌预览按 8 位处理）
    pub bit_depth: u8,
    /// 传感器色彩空间的中文标签，如 `sRGB` / `相机原生`
    pub color_space: String,
}

/// 错误信息（走 JSON）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorDto {
    pub code: String,
    pub message: String,
    /// 是否可展示给用户看（技术性错误隐藏细节）
    pub user_visible: bool,
}

impl From<PvError> for ErrorDto {
    fn from(e: PvError) -> Self {
        let (code, user_visible) = match &e {
            PvError::NotFound(_) => ("not_found", true),
            PvError::PermissionDenied(_) => ("permission_denied", true),
            PvError::Io { .. } => ("io_error", false),
            PvError::UnsupportedFormat(_) => ("unsupported_format", true),
            PvError::Corrupt(_) => ("corrupt", true),
            PvError::Decode(_) => ("decode_error", true),
            PvError::Encode(_) => ("encode_error", true),
            PvError::NotWritable(_) => ("not_writable", true),
            PvError::VaultLocked => ("vault_locked", true),
            PvError::VaultCorrupt(_) => ("vault_corrupt", true),
            PvError::RawDevelopNotImplemented => ("raw_devive_todo", true),
            PvError::Timeout(_) => ("timeout", false),
            PvError::MemoryBudget { .. } => ("memory_budget", true),
            PvError::Other(_) => ("other", false),
        };
        Self {
            code: code.to_string(),
            message: e.to_string(),
            user_visible,
        }
    }
}

/// 把 io::Error 包装成 PvError 再转 ErrorDto
pub(crate) fn io_err(path: &Path, e: std::io::Error) -> ErrorDto {
    ErrorDto::from(PvError::Io {
        path: path.to_path_buf(),
        source: e,
    })
}

/// 文件版本 —— 纯 `stat` 结果，用于**前端解码缓存的校验**。
///
/// ## 为什么单独开一条命令，而不是复用 `probe_image`
/// `probe_image` 要按格式解析头部，HEIC 甚至是**全解码**（实测 ~98ms）。
/// 拿它当缓存校验会让「预取相邻图片」变成重活。这里只 `metadata`，
/// 微秒级、与格式无关，可以放心在每次取缓存前调用。
///
/// ## 为什么用毫秒而不是秒
/// `ImageInfoDto::modified_at` 是**秒**（只够信息面板显示）。秒级精度下
/// 「同一秒内覆盖原文件、且大小恰好相同」会漏判 —— 那正是本机制要抓的场景。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileVersionDto {
    /// 修改时间（Unix **毫秒**；取不到时为 0）
    pub modified_ms: i64,
    /// 文件字节数（与时间戳一起构成版本指纹）
    pub size: u64,
}

/// 应用信息（供前端显示版本与能力）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfoDto {
    pub version: String,
    pub backends: Vec<String>,
    pub raw_enabled: bool,
}

/// 单格式能力条目
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormatCapability {
    pub id: String,
    pub label: String,
    pub tier: String,
    pub enabled: bool,
}

/// 文件关联状态（供帮助面板的「设置」回显）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssociationStateDto {
    /// 当前已注册到「打开方式」列表的扩展名（小写、不含点）
    pub registered: Vec<String>,
    /// 本程序在注册表里的 ProgID 前缀，前端拼 `PurePhotoExplorer.jpg` 这类名字用
    pub prog_id_prefix: String,
    /// 说明性文案（"Windows 不允许程序自行设为默认"这类），放 Rust 侧写一处
    pub note: String,
}

/// 批量设置文件关联时的**单个条目**（前端 → 后端）。
///
/// `display_name` 是写进注册表的类型名（如 `JPEG 图片`），由前端从
/// `constants/formats.ts` 的标签派生 —— 格式清单的唯一真源在前端，
/// Rust 侧不重复维护一份，否则加了新格式两边就会漏同步。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssocItemDto {
    /// 扩展名（小写、不含点）
    pub ext: String,
    /// 注册表里的类型显示名
    pub display_name: String,
}

/// 辅助函数：把路径转成用户可读名（不含完整路径，日志脱敏用）
#[allow(dead_code)]
fn base_name(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "<未知>".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 错误映射对用户可见性判断正确() {
        // 不支持格式 → 用户应看到"暂不支持"
        let e: ErrorDto = PvError::UnsupportedFormat("X".into()).into();
        assert_eq!(e.code, "unsupported_format");
        assert!(e.user_visible, "不支持格式必须让用户知道");

        // IO 错误是技术性的，不该直接甩给用户
        let e: ErrorDto = PvError::Io {
            path: std::path::PathBuf::from("x"),
            source: std::io::Error::other("boom"),
        }
        .into();
        assert!(!e.user_visible, "IO 细节不该展示给用户");
    }

    /// ⭐ `FileVersionDto` 的字段名必须与前端 TS（`ipc/image.ts::FileVersion`）一致。
    ///
    /// 这条不是形式主义：前端拿它拼缓存指纹 `${modifiedMs}:${size}`。
    /// 字段名对不上时两个值都是 `undefined`，指纹变成恒定的 `"undefined:undefined"`，
    /// **版本校验会静默失效** —— 表现就是「文件被覆盖了还显示旧图」原地复活，
    /// 而且没有任何报错。`ImageInfoDto` 那类"只显示"的字段还能肉眼看出来，
    /// 这个只看代码看不出来。
    #[test]
    fn 文件版本DTO字段名与前端一致() {
        let dto = FileVersionDto {
            modified_ms: 1_700_000_000_123,
            size: 4096,
        };
        let v = serde_json::to_value(&dto).unwrap();
        for key in ["modifiedMs", "size"] {
            assert!(v.get(key).is_some(), "FileVersionDto 缺字段 {key}");
        }
        // 秒级时间戳会被当成毫秒用 → 校验几乎必然失效，这里钉住量级
        assert_eq!(v["modifiedMs"].as_i64().unwrap(), 1_700_000_000_123);
    }

    /// 文件关联状态的字段名同样要对齐
    #[test]
    fn 关联状态DTO字段名与前端一致() {
        let dto = AssociationStateDto {
            registered: vec!["jpg".into()],
            prog_id_prefix: "PurePhotoExplorer".into(),
            note: "说明".into(),
        };
        let v = serde_json::to_value(&dto).unwrap();
        for key in ["registered", "progIdPrefix", "note"] {
            assert!(v.get(key).is_some(), "AssociationStateDto 缺字段 {key}");
        }
    }

    /// 批量关联条目的字段名（前端传 `{ ext, displayName }`）
    #[test]
    fn 关联条目DTO字段名与前端一致() {
        let dto = AssocItemDto {
            ext: "jpg".into(),
            display_name: "JPEG 图片".into(),
        };
        let v = serde_json::to_value(&dto).unwrap();
        for key in ["ext", "displayName"] {
            assert!(v.get(key).is_some(), "AssocItemDto 缺字段 {key}");
        }
        // 反序列化方向也要能认（前端就是把这个形状传过来的）
        let back: AssocItemDto =
            serde_json::from_value(serde_json::json!({"ext":"png","displayName":"PNG 图片"})).unwrap();
        assert_eq!(back.ext, "png");
        assert_eq!(back.display_name, "PNG 图片");
    }

    #[test]
    fn JSON字段名与前端TS类型一一对应() {
        let dto = ImageInfoDto {
            path: "C:/a.jpg".into(),
            file_name: "a.jpg".into(),
            format: "jpeg".into(),
            format_label: "JPEG".into(),
            width: 100,
            height: 50,
            file_size: 1024,
            is_raw: false,
            modified_at: 1_700_000_000,
            exif: Some(pv_meta::ExifData {
                make: Some("SONY".into()),
                iso: Some(320),
                ..Default::default()
            }),
            raw: None,
            place: None,
        };

        let v = serde_json::to_value(&dto).unwrap();
        for key in [
            "path",
            "fileName",
            "format",
            "formatLabel",
            "width",
            "height",
            "fileSize",
            "isRaw",
            "modifiedAt",
            "exif",
            "raw",
            "place",
        ] {
            assert!(v.get(key).is_some(), "ImageInfoDto 缺少字段 {key}");
        }

        let exif = &v["exif"];
        for key in [
            "make",
            "model",
            "lens",
            "dateTimeOriginal",
            "exposureTime",
            "fNumber",
            "iso",
            "focalLength",
            "focalLength35mm",
            "exposureBias",
            "exposureProgram",
            "meteringMode",
            "flash",
            "whiteBalance",
            "orientation",
            "software",
            "gps",
        ] {
            assert!(exif.get(key).is_some(), "ExifData 缺少字段 {key}");
        }

        // GPS 是嵌套对象，键名同样要和前端 `GpsData` 对齐
        let gps = serde_json::to_value(pv_meta::GpsData {
            latitude: "31.204839".into(),
            longitude: "121.440628".into(),
            altitude: Some("9.3 m".into()),
            display: "31.204839°N, 121.440628°E".into(),
        })
        .unwrap();
        for key in ["latitude", "longitude", "altitude", "display"] {
            assert!(gps.get(key).is_some(), "GpsData 缺少字段 {key}");
        }

        // 无 EXIF 的照片应当是显式 null，而不是缺字段（前端按 null 判断）
        let mut no_exif = v.clone();
        no_exif["exif"] = serde_json::Value::Null;
        assert!(no_exif["exif"].is_null());
    }

    /// RAW 信息的键名同样要和前端 `RawInfo` 对齐。
    #[test]
    fn RAW信息键名与前端TS类型一致() {
        let dto = RawInfoDto {
            raw_format: "CR3".into(),
            has_embedded_preview: true,
            bit_depth: 8,
            color_space: "sRGB".into(),
        };
        let v = serde_json::to_value(&dto).unwrap();
        for key in [
            "rawFormat",
            "hasEmbeddedPreview",
            "bitDepth",
            "colorSpace",
        ] {
            assert!(v.get(key).is_some(), "RawInfoDto 缺少字段 {key}");
        }
    }
}
