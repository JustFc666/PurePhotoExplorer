//! 纯 Rust 的 [`RawPipeline`] 实现。
//!
//! Stage 1（提取内嵌预览）已完整可用；Stage 2（完整开发流程）
//! 按设计返回 `RawDevelopNotImplemented`，接口留好待后续接入。
//!
//! ## probe 为什么用 `pv-meta` 而不是自己解析
//!
//! 拍摄参数（EXIF）的解析逻辑已经在 `pv-meta` 里做完了 —— 它支持
//! 「非标准 TIFF 魔数」的 ORF/RW2，也有完整的字段格式化。
//! 这里只是把 `ExifData`（面向展示的字符串）映射成
//! [`RawMetadata`]（带数值的结构化数据），**不重复实现解析**。
//!
//! 这样「文件 → 结构化描述」的能力始终只有一份实现，
//! 修 bug 或扩字段都只改 `pv-meta`。

use std::path::Path;

use pv_core::error::{PvError, Result};
use pv_core::raw::{EncodedPreview, RawColorSpace, RawMetadata, RawPipeline};

use super::extract_embedded_preview;

/// 基于内嵌预览的 RAW 管线。
///
/// 无状态 —— 每次调用独立打开文件，可安全地在多线程间共享
/// （满足 `RawPipeline: Send + Sync`）。
pub struct EmbeddedPreviewPipeline;

impl EmbeddedPreviewPipeline {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EmbeddedPreviewPipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl RawPipeline for EmbeddedPreviewPipeline {
    fn name(&self) -> &'static str {
        "embedded-preview-pure-rust"
    }

    fn extract_preview(&self, path: &Path) -> Result<EncodedPreview> {
        extract_embedded_preview(path)
    }

    fn extract_preview_slow(&self, path: &Path) -> Result<EncodedPreview> {
        // 纯 Rust 方案没有「降级去马赛克」这条路径：
        // 内嵌预览已经是全部能力。返回明确错误而不是假装成功，
        // 让上层的 `extract_preview_with_fallback` 如实向上传递。
        let _ = path;
        Err(PvError::RawDevelopNotImplemented)
    }

    /// Stage 2 未实现（本期范围外）。
    ///
    /// 用 trait 的默认实现即可 —— 它返回 `RawDevelopNotImplemented`。
    /// 这里不覆盖，是为了让「没实现」这件事只有一处声明。

    fn probe(&self, path: &Path) -> Result<RawMetadata> {
        let exif = pv_meta::read_exif(path);

        // 预览尺寸作为图像尺寸的兜底：
        // RAW 的完整传感器尺寸需要解析厂商私有 IFD，本期不做；
        // 而用户看到的本来也是预览，用预览尺寸更贴合实际观感。
        //
        // **必须如实反映有无预览**：早先这里写死 `has_embedded_preview: true`
        // 且尺寸默认 `(0, 0)`，会让「无内嵌预览」的 RAW 在信息面板上
        // 显示成「内嵌预览 可用 / 尺寸 0 × 0」，随后解码又失败 —— 自相矛盾。
        let preview_size = super::probe_preview_size(path).ok();
        let (width, height) = preview_size.unwrap_or((0, 0));
        let has_preview = preview_size.is_some();

        let (make, model, lens, iso, shutter, aperture, focal) = match &exif {
            Some(e) => (
                e.make.clone().unwrap_or_default(),
                e.model.clone().unwrap_or_default(),
                e.lens.clone(),
                e.iso,
                parse_exposure_time(e.exposure_time.as_deref()),
                parse_f_number(e.f_number.as_deref()),
                parse_focal_length(e.focal_length.as_deref()),
            ),
            None => Default::default(),
        };

        Ok(RawMetadata {
            make,
            model,
            lens,
            iso,
            shutter,
            aperture,
            focal_mm: focal,
            width,
            height,
            raw_format: raw_format_label(path),
            // 内嵌预览是 8-bit JPEG；传感器的真实位深（12/14/16）需要
            // 解析厂商 IFD，本期不读取。
            bit_depth: 8,
            color_space: RawColorSpace::Srgb,
            has_embedded_preview: has_preview,
            captured_at: None,
        })
    }

    fn supported_cameras(&self) -> Result<Vec<String>> {
        // 本实现不依赖机型白名单 —— 只要相机写了内嵌 JPEG 预览就能显示。
        // 返回空列表表示「不限定机型」，设置页据此显示为「通用」。
        Ok(Vec::new())
    }
}

/// 从格式化的快门字符串（`1/250 s`）还原为秒数。
///
/// `pv-meta` 输出的是人类可读形态，这里反解回数值供结构化消费。
fn parse_exposure_time(s: Option<&str>) -> Option<f64> {
    let s = s?.trim().trim_end_matches(" s").trim();
    if let Some((num, den)) = s.split_once('/') {
        let n: f64 = num.trim().parse().ok()?;
        let d: f64 = den.trim().parse().ok()?;
        if d == 0.0 {
            return None;
        }
        Some(n / d)
    } else {
        s.parse().ok()
    }
}

/// 从 `f/2.8` 还原光圈值。
fn parse_f_number(s: Option<&str>) -> Option<f64> {
    s?.trim().trim_start_matches("f/").trim().parse().ok()
}

/// 从 `50 mm` 还原焦距。
fn parse_focal_length(s: Option<&str>) -> Option<f64> {
    s?.trim().trim_end_matches(" mm").trim().parse().ok()
}

/// 按扩展名给个可读的格式标签。
///
/// 注意：**仅用于展示**。真正的格式判定一律走文件头魔数
/// （见 `pv_core::format::probe_header`），这里取扩展名是因为
/// 厂商 RAW 的容器差异在魔数层面无法区分（NEF/ARW/DNG 都是 TIFF）。
fn raw_format_label(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_uppercase())
        .unwrap_or_else(|| "RAW".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sample(name: &str) -> Option<PathBuf> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-image")
            .join(name);
        if p.is_file() && std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0) > 4096 {
            Some(p)
        } else {
            None
        }
    }

    #[test]
    fn probe返回拍摄参数与预览尺寸() {
        let Some(p) = sample("nikon-nrw-sample.NRW") else { return };
        let m = EmbeddedPreviewPipeline::new().probe(&p).unwrap();
        assert_eq!(m.raw_format, "NRW");
        assert_eq!((m.width, m.height), (4000, 3000), "尺寸应来自内嵌预览");
        assert!(m.has_embedded_preview);
        eprintln!("NRW metadata = {m:#?}");
    }

    /// 无内嵌预览时必须**如实**报告，不能谎报「可用」+ 尺寸 0×0。
    ///
    /// 回归背景（2026-10-07）：原实现写死 `has_embedded_preview: true`
    /// 且尺寸默认 (0,0)，信息面板会显示「内嵌预览 可用 / 尺寸 0 × 0」，
    /// 但随后解码必然失败 —— 前后矛盾，用户会认为程序坏了。
    #[test]
    fn 无内嵌预览时如实报告() {
        // 造一个「TIFF 头 + 空 IFD」的裸容器，其中不含任何 JPEG
        let dir = std::env::temp_dir().join("pv_no_preview_test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("no-preview.dng");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"II");
        bytes.extend_from_slice(&42u16.to_le_bytes());
        bytes.extend_from_slice(&8u32.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes()); // IFD 条目数 0
        bytes.extend_from_slice(&0u32.to_le_bytes()); // next IFD
        bytes.extend_from_slice(&[0u8; 4096]); // 填充，超过最小预览门槛
        std::fs::write(&p, &bytes).unwrap();

        let m = EmbeddedPreviewPipeline::new().probe(&p).unwrap();
        assert!(
            !m.has_embedded_preview,
            "没有内嵌 JPEG 时必须报告 false，而不是写死 true"
        );
        assert_eq!(
            (m.width, m.height),
            (0, 0),
            "无预览时尺寸应为 0，由前端显示为「未知」"
        );

        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn 各样本probe都能拿到尺寸() {
        for name in [
            "canon-cr3-sample.CR3",
            "fuji-raf-sample.RAF",
            "olympus-orf-sample.ORF",
            "panasonic-rw2-sample.RW2",
            "sony-sr2-sample.SR2",
            "sony-srf-sample.SRF",
        ] {
            let Some(p) = sample(name) else { continue };
            let m = EmbeddedPreviewPipeline::new().probe(&p).unwrap();
            assert!(m.width > 0 && m.height > 0, "{name} 应能探测到预览尺寸");
            eprintln!("{name} → {}×{} fmt={}", m.width, m.height, m.raw_format);
        }
    }

    #[test]
    fn develop明确返回未实现() {
        let p = EmbeddedPreviewPipeline::new();
        let r = p.develop(
            Path::new("x.nef"),
            &pv_core::raw::RawDevelopParams::default(),
        );
        assert!(matches!(r, Err(PvError::RawDevelopNotImplemented)));
    }

    #[test]
    fn 快门字符串反解() {
        assert_eq!(parse_exposure_time(Some("1/250 s")), Some(0.004));
        assert_eq!(parse_exposure_time(Some("2 s")), Some(2.0));
        assert_eq!(parse_exposure_time(None), None);
        assert_eq!(parse_exposure_time(Some("1/0 s")), None, "除零不应 panic");
    }

    #[test]
    fn 光圈与焦距反解() {
        assert_eq!(parse_f_number(Some("f/2.8")), Some(2.8));
        assert_eq!(parse_f_number(Some("f/11")), Some(11.0));
        assert_eq!(parse_f_number(None), None);
        assert_eq!(parse_focal_length(Some("50 mm")), Some(50.0));
        assert_eq!(parse_focal_length(Some("24 mm")), Some(24.0));
    }

    #[test]
    fn 格式标签取扩展名大写() {
        assert_eq!(raw_format_label(Path::new("a/x.nef")), "NEF");
        assert_eq!(raw_format_label(Path::new("a/x.CR3")), "CR3");
        assert_eq!(raw_format_label(Path::new("a/noext")), "RAW");
    }
}
