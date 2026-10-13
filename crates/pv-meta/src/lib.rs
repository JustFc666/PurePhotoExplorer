//! PurePhotoExplorer 元数据层
//!
//! **职责单一：从图像文件里读出拍摄参数（EXIF），只读，绝不修改原图。**
//!
//! 为什么单独成 crate 而不是塞进 `pv-decode`：
//! - 解码是"文件 → 像素"，元数据是"文件 → 结构化描述"，两者失败模式与缓存策略都不同
//!   （RAW 一期还解不了码，但 EXIF 现在就能读出来给用户看）；
//! - 后续 S2/S3 的检索（按相机、镜头、拍摄日期筛选）直接复用本 crate 的 `ExifData`。
//!
//! 分层：
//! - [`ExifData`] 统一的数据模型（可序列化，直接给前端用）
//! - `exif` 子模块：`read_exif`（含容器格式兜底）+ 字段提取 + 数值格式化
//! - [`read_exif_raw`] / [`align_exif_with_pixels`]：**写回**用的原始 EXIF 字节
//!   与其一致性修正（另存为时把拍摄参数带给新文件）

// 测试函数名用中文（可读性优先），关闭 snake_case 检查
#![cfg_attr(test, allow(non_snake_case))]

pub mod exif;

pub use exif::{
    align_exif_with_pixels, read_exif, read_exif_raw, ExifData, ExifRewriteOutcome, GpsData,
    MetaError,
};
