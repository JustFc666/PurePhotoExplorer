//! RAW 内嵌预览提取（纯 Rust，不依赖 LibRaw）。
//!
//! ## 为什么走「提取内嵌预览」这条路
//!
//! 项目文档原本规划用 LibRaw 做 RAW 解码（`pv_core::raw::RawPipeline`），
//! 但实测（2026-10-07）Rust 生态的 LibRaw 绑定在 Windows/MSVC 上不可用：
//!
//! | crate | 结论 |
//! |---|---|
//! | `rsraw` | `build.rs` 硬编码 `panic!("MSVC is not supported")`，直接不可用 |
//! | `libraw` / `rawlib` | 需系统预装 `libraw_r.dll` + pkg-config，本机没有 |
//!
//! 而且 LibRaw 全量编译会把 `dcb_demosaic.cpp` / `xtrans_demosaic.cpp`
//! 等 **GPL 代码**一并引入，与文档 §4A / 风险 R-03 的许可要求冲突。
//!
//! ## 内嵌预览的性价比
//!
//! 相机在写 RAW 时通常会额外存一张（或几张）已渲染好的 JPEG 预览。
//! 实测本机样本中最大预览的分辨率：
//!
//! ```text
//! canon-cr3   6000×4000   ← 全尺寸，画质等同相机直出 JPEG
//! nikon-nrw   4000×3000   ← 全尺寸
//! panasonic-rw2 1920×1280
//! fuji-raf    1600×1200
//! olympus-orf 1600×1200
//! sony-sr2/srf  640×480
//! ```
//!
//! **现代机型的预览就是全尺寸**，浏览体验与真解码几乎无差别，
//! 而且是相机厂商自己渲染的，观感最"正"（白平衡、色彩都按厂商意图来）。
//!
//! 代价：预览偏小的机型（Sony SR2）放大后会糊。真解码（demosaic +
//! 线性域处理）留给 `RawPipeline::develop`，接口已留好，随时可换实现。
//!
//! ## 定位策略：通用扫描
//!
//! 不逐家适配容器格式（TIFF IFD / ISOBMFF / RAF 头），而是
//! **扫描整个文件，用严格 JPEG 边界解析切出每一张内嵌 JPEG**，
//! 再按像素面积取最大的一张。
//!
//! 理由：各家容器差异大（CR3 的预览藏在 CMT 描述的 mdat 区，
//! NRW 的大预览不在 IFD0 而在次级 IFD），逐个适配容易漏；
//! 而扫描器只需一份正确实现就能全覆盖 —— 实测 7/7 命中，
//! 且 CR3/NRW 都能取到全尺寸预览。
//!
//! 性能：26 MB 的 CR3 全扫约几十毫秒，且只在打开 RAW 时触发一次。

mod jpeg_scan;
mod pipeline;

use std::path::Path;

use pv_core::error::{PvError, Result};
use pv_core::raw::EncodedPreview;

pub use jpeg_scan::JpegSpan;
pub use pipeline::EmbeddedPreviewPipeline;

/// 候选预览的最小字节数。低于此值的基本都是 160×120 级别的缩略图。
///
/// 设 16 KB 是因为：最小的可用预览（Sony 640×480）也有 134 KB，
/// 而各家都会额外塞一张 160×120 的小图（通常 5–12 KB）。
const MIN_PREVIEW_BYTES: usize = 16 * 1024;

/// 扫描上限。超过部分不再查找预览。
///
/// 预览总是位于文件前部或独立区块，128 MB 足够覆盖所有已知机型；
/// 设上限是为了避免在异常大的文件上做无谓的全量扫描。
const MAX_SCAN_BYTES: u64 = 128 * 1024 * 1024;

/// 从 RAW 文件中提取尺寸最大的内嵌预览。
///
/// 返回的 [`EncodedPreview::Jpeg`] 是**相机原始字节的原样拷贝**，
/// 调用方直接交给 JPEG 解码器即可（不要在这里解码，保持 Stage 1 零成本）。
pub fn extract_embedded_preview(path: &Path) -> Result<EncodedPreview> {
    let data = read_capped(path)?;
    if data.is_empty() {
        return Err(PvError::Corrupt("文件为空".into()));
    }

    let spans = jpeg_scan::scan_all_jpegs(&data, MIN_PREVIEW_BYTES);
    let best = pick_largest(&data, &spans).ok_or_else(|| {
        PvError::UnsupportedFormat("该 RAW 不含可用的内嵌预览".into())
    })?;

    Ok(EncodedPreview::Jpeg(data[best.start..best.end].to_vec()))
}

/// 探测 RAW 内嵌预览的尺寸（不解码整张图，只读 JPEG 的 SOF 段）。
///
/// 返回 `(宽, 高)`。用于在没有 EXIF 机型尺寸时兜底显示。
pub fn probe_preview_size(path: &Path) -> Result<(u32, u32)> {
    let data = read_capped(path)?;
    let spans = jpeg_scan::scan_all_jpegs(&data, MIN_PREVIEW_BYTES);
    let best = pick_largest(&data, &spans).ok_or_else(|| {
        PvError::UnsupportedFormat("该 RAW 不含可用的内嵌预览".into())
    })?;
    let blob = &data[best.start..best.end];
    read_jpeg_size(blob).ok_or_else(|| PvError::Decode("内嵌预览的 JPEG 头无法解析".into()))
}

/// 在候选里挑像素面积最大的那张。
///
/// 之所以不直接按字节长度排（长的通常也大），是因为**字节长度会骗人**：
/// 高压缩率的大图可能比低压缩率的小图字节更少。所以这里读每张图的
/// SOF 尺寸来比面积，读不出尺寸的才退回按字节长度。
fn pick_largest<'a>(data: &[u8], spans: &'a [JpegSpan]) -> Option<&'a JpegSpan> {
    let mut best: Option<(&JpegSpan, u64)> = None;
    for span in spans {
        let blob = &data[span.start..span.end];
        let area = read_jpeg_size(blob)
            .map(|(w, h)| w as u64 * h as u64)
            // 读不出尺寸就按字节长度估（乘个系数保持量纲可比）
            .unwrap_or(span.len() as u64);
        match best {
            Some((_, cur)) if cur >= area => {}
            _ => best = Some((span, area)),
        }
    }
    best.map(|(s, _)| s)
}

/// 从 JPEG 字节流里读出图像尺寸（解析 SOF 段）。
///
/// 只走到 SOF 就返回，不解码任何像素 —— 对 26 MB 的内嵌大图也是微秒级。
fn read_jpeg_size(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return None;
    }
    let mut i = 2usize;
    while i + 9 < data.len() {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }
        let mut marker = data[i + 1];
        // 跳过填充
        while marker == 0xFF && i + 2 < data.len() {
            i += 1;
            marker = data[i + 1];
        }
        // SOF0..SOF15（跳过 DHT=C4 / JPG=C8 / DAC=CC）
        if (0xC0..=0xCF).contains(&marker)
            && marker != 0xC4
            && marker != 0xC8
            && marker != 0xCC
        {
            let h = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
            let w = u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32;
            return Some((w, h));
        }
        // SOS 之后再没有 SOF 了
        if marker == 0xDA || marker == 0xD9 {
            return None;
        }
        let seg_len = u16::from_be_bytes([
            *data.get(i + 2)?,
            *data.get(i + 3)?,
        ]) as usize;
        if seg_len < 2 {
            return None;
        }
        i += 2 + seg_len;
    }
    None
}

/// 按 [`MAX_SCAN_BYTES`] 上限读取文件。
fn read_capped(path: &Path) -> Result<Vec<u8>> {
    use std::io::Read;
    let f = std::fs::File::open(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => PvError::NotFound(path.to_path_buf()),
        std::io::ErrorKind::PermissionDenied => PvError::PermissionDenied(path.to_path_buf()),
        _ => PvError::Io {
            path: path.to_path_buf(),
            source: e,
        },
    })?;
    let mut buf = Vec::new();
    f.take(MAX_SCAN_BYTES).read_to_end(&mut buf).map_err(|e| PvError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    Ok(buf)
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
            eprintln!("跳过：样例不可用 {}", p.display());
            None
        }
    }

    #[test]
    fn 从各厂商RAW提取到内嵌预览() {
        // 7 个真实样本，全部应能提取到结构完整的 JPEG
        let cases = [
            ("canon-cr3-sample.CR3", 6000, 4000),
            ("fuji-raf-sample.RAF", 1600, 1200),
            ("nikon-nrw-sample.NRW", 4000, 3000),
            ("olympus-orf-sample.ORF", 1600, 1200),
            ("panasonic-rw2-sample.RW2", 1920, 1280),
            ("sony-sr2-sample.SR2", 640, 424),
            ("sony-srf-sample.SRF", 640, 480),
        ];
        let mut checked = 0;
        for (name, exp_w, exp_h) in cases {
            let Some(p) = sample(name) else { continue };
            let preview = extract_embedded_preview(&p)
                .unwrap_or_else(|e| panic!("{name} 提取预览失败: {e}"));
            let bytes = match &preview {
                EncodedPreview::Jpeg(b) => b,
                other => panic!("{name} 应产出 Jpeg 预览，实际 {}", other.source_kind()),
            };
            let (w, h) = read_jpeg_size(bytes)
                .unwrap_or_else(|| panic!("{name} 的预览 JPEG 尺寸读不出"));
            assert_eq!(
                (w, h),
                (exp_w, exp_h),
                "{name} 应选中最大的预览（不是缩略图）"
            );
            checked += 1;
        }
        assert!(checked > 0, "至少应有一个真实样本可用");
        eprintln!("已校验 {checked} 个 RAW 样本");
    }

    #[test]
    fn 提取到的是原样JPEG字节() {
        let Some(p) = sample("olympus-orf-sample.ORF") else { return };
        let bytes = match extract_embedded_preview(&p).unwrap() {
            EncodedPreview::Jpeg(b) => b,
            _ => panic!("应为 Jpeg"),
        };
        assert_eq!(&bytes[0..2], &[0xFF, 0xD8], "应以 SOI 开头");
        assert_eq!(&bytes[bytes.len() - 2..], &[0xFF, 0xD9], "应以 EOI 结尾");
    }

    #[test]
    fn probe尺寸与提取结果一致() {
        let Some(p) = sample("nikon-nrw-sample.NRW") else { return };
        let (w, h) = probe_preview_size(&p).unwrap();
        assert_eq!((w, h), (4000, 3000));
    }

    #[test]
    fn 非RAW文件不panic() {
        let dir = std::env::temp_dir().join("pv-raw-test");
        std::fs::create_dir_all(&dir).unwrap();

        let junk = dir.join("junk.bin");
        std::fs::write(&junk, vec![0x5Au8; 100_000]).unwrap();
        assert!(extract_embedded_preview(&junk).is_err());

        let empty = dir.join("empty.bin");
        std::fs::write(&empty, b"").unwrap();
        assert!(extract_embedded_preview(&empty).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 不存在的文件返回NotFound() {
        let r = extract_embedded_preview(Path::new("Z:/绝对不存在/x.nef"));
        assert!(matches!(r, Err(PvError::NotFound(_))));
    }

    #[test]
    fn 解析SOF取尺寸() {
        // SOI + SOF0(w=320,h=240) + EOI
        let mut j = vec![0xFF, 0xD8];
        j.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08]); // SOF0, len=17, precision=8
        j.extend_from_slice(&[0x00, 0xF0]); // height = 240
        j.extend_from_slice(&[0x01, 0x40]); // width = 320
        j.extend_from_slice(&[0x03, 0x01, 0x11, 0x00, 0x02, 0x11, 0x01, 0x03, 0x11, 0x01]);
        assert_eq!(read_jpeg_size(&j), Some((320, 240)));
    }
}
