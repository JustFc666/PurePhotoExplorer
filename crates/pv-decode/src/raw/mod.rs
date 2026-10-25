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

/// **快速通道**只读文件开头这么多字节。
///
/// 实测（2026-10-17，各格式官方样本 + 相机实拍）内嵌预览的位置极不均衡：
///
/// | 格式 | 预览起点 | 占文件 |
/// |---|---|---|
/// | 索尼 ARW | 135 KB | **0.3%** |
/// | 佳能 CR3 | 354 KB | 1.3% |
/// | 奥林巴斯 ORF | 24 KB | 0.2% |
/// | 富士 RAF | 148 B | 0.0% |
/// | 松下 RW2 | 1.5 KB | 0.0% |
/// | 索尼 SR2/SRF | ~1 KB | 0.0% |
/// | Adobe DNG | 3.17 MB | 12.1% |
/// | **尼康 NRW** | **18.3 MB** | **87.5%** ← 必须走兜底 |
///
/// 所以取 **4 MB**（刚好覆盖 DNG 那个 3.17MB 的），其余格式全在前 0.4 MB 内。
/// 一张 47MB 的 ARW 因此只读 4MB → **约 12x 的 I/O 减少**；
/// 像 NRW 这种「预览在尾部」的，快速通道找不到够用的，自动退回全量扫描。
const FAST_PREFIX_BYTES: u64 = 4 * 1024 * 1024;

/// 快速通道的「够清晰」下限（长边像素）。
///
/// 各格式的正式预览都 ≥1600，而相机常在文件前部塞一张小程序图 ——
/// 它同样过得了 [`MIN_PREVIEW_BYTES`]（16KB），只看「够不够 256」会误收。
const FAST_MIN_GOOD_EDGE: u32 = 1024;

/// 快速通道的「像样预览」字节下限。
///
/// 实测（各格式官方样本 + ARW 实拍）前缀里挑出的候选字节数：
/// ARW 490KB、CR3 1.8MB、DNG 3.9MB、RW2 450KB、ORF 420KB、
/// RAF 626KB、SR2 **134KB**（最小）、SRF 142KB；
/// 而尼康 NRW 前缀里那张「假预览」只有 **25KB**（长边却 ≥1024，光看尺寸骗得过）。
///
/// 取 100KB：合法预览全部通过，25KB 那种劣质大图被挡掉。
/// ⛔ 判据是**同时**满足：字节数够、长边达到 callers 的期望、且 ≥1024 —— 三者缺一不可。
const FAST_MIN_PREVIEW_BYTES: usize = 100 * 1024;

/// 快速通道的判定（抽成纯函数，便于单测边界）。
///
/// 三个条件**同时**满足才接受：
/// 1. 字节数 ≥ [`FAST_MIN_PREVIEW_BYTES`] —— 排除高压缩的劣质大图
/// 2. 长边 ≥ `need_edge` —— 达到调用方期望的尺寸
/// 3. 长边 ≥ [`FAST_MIN_GOOD_EDGE`] —— 排除相机的小程序图
///
/// 例外：`whole_file` = 前缀已覆盖整个文件，此时前缀里最大的那张就是全局最大，
/// 再「读剩下的」也没有更多内容可读，直接接受。
fn fast_preview_good_enough(
    edge: u32,
    bytes: usize,
    need_edge: u32,
    whole_file: bool,
) -> bool {
    whole_file
        || (bytes >= FAST_MIN_PREVIEW_BYTES
            && edge >= need_edge
            && edge >= FAST_MIN_GOOD_EDGE)
}

/// 从 RAW 文件中提取内嵌预览。
///
/// 返回的 [`EncodedPreview::Jpeg`] 是**相机原始字节的原样拷贝**，
/// 调用方直接交给 JPEG 解码器即可（不要在这里解码，保持 Stage 1 零成本）。
///
/// `need_edge` 见 [`pv_core::raw::RawPipeline::extract_preview`]：
/// 传 0 表示要最大的那张（全量扫描）；传具体尺寸则允许走快速通道。
pub fn extract_embedded_preview(path: &Path, need_edge: u32) -> Result<EncodedPreview> {
    // ── 快速通道：只读文件开头一段 ──
    // 只有当「找到的这张已经够用」才提前返回；`need_edge == 0` 表示要最大的，不走近道。
    if need_edge > 0 {
        if let Some(bytes) = fast_preview(path, need_edge) {
            return Ok(EncodedPreview::Jpeg(bytes));
        }
    }

    // ── 兜底：整文件扫描（预览在尾部、或快速通道没找到够大的）──
    let data = read_up_to(path, MAX_SCAN_BYTES)?;
    if data.is_empty() {
        return Err(PvError::Corrupt("文件为空".into()));
    }
    let spans = jpeg_scan::scan_all_jpegs(&data, MIN_PREVIEW_BYTES);
    let best = pick_largest(&data, &spans)
        .ok_or_else(|| PvError::UnsupportedFormat("该 RAW 不含可用的内嵌预览".into()))?;
    Ok(EncodedPreview::Jpeg(data[best.start..best.end].to_vec()))
}

/// 只读前缀就能拿到一张「够用」的预览时返回它，否则返回 `None`（让调用方走全扫描）。
///
/// 任何 I/O 错误都吞成 `None`：快速通道只是优化，失败就该退回正规路径，
/// 不能因为它把本来能解出来的文件判死。
fn fast_preview(path: &Path, need_edge: u32) -> Option<Vec<u8>> {
    let data = read_up_to(path, FAST_PREFIX_BYTES).ok()?;
    // ⚠️ 判「前缀是否已覆盖整个文件」必须看**文件本身的大小**。
    //    踩过：用 `data.len() <= FAST_PREFIX_BYTES` 判断时，大文件读满 4MB 也满足
    //    `<=`，于是 20MB 的 NRW 被当成「前缀即全文」，短路接受了那张 25KB 的假预览。
    let file_len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(u64::MAX);
    let whole_file = file_len <= FAST_PREFIX_BYTES;
    let spans = jpeg_scan::scan_all_jpegs(&data, MIN_PREVIEW_BYTES);
    let best = pick_largest(&data, &spans)?;
    let blob = &data[best.start..best.end];
    let (w, h) = read_jpeg_size(blob)?;
    if fast_preview_good_enough(w.max(h), blob.len(), need_edge, whole_file) {
        Some(blob.to_vec())
    } else {
        None
    }
}

/// 探测 RAW 内嵌预览的尺寸（不解码整张图，只读 JPEG 的 SOF 段）。
///
/// 返回 `(宽, 高)`。用于在没有 EXIF 机型尺寸时兜底显示。
///
/// 先走**快速通道**（只读前 4MB），门槛与 [`extract_embedded_preview`] 完全一致 ——
/// 所以结果与全量扫描**逐位相同**（`probe尺寸与提取结果一致` 与
/// `快速通道与全扫描结果一致` 两条测试一起钉住）。
///
/// ⚠️ 别为了「省 I/O」放宽门槛：NRW 前缀里那张 640×480 的假预览如果被接受，
/// 信息面板就会把 4000×3000 的机器报成 640×480。
pub fn probe_preview_size(path: &Path) -> Result<(u32, u32)> {
    // need_edge = 1：只要「够清晰 + 字节数够」，不额外要求尺寸
    if let Some(blob) = fast_preview(path, 1) {
        if let Some(size) = read_jpeg_size(&blob) {
            return Ok(size);
        }
    }
    let data = read_up_to(path, MAX_SCAN_BYTES)?;
    let spans = jpeg_scan::scan_all_jpegs(&data, MIN_PREVIEW_BYTES);
    let best = pick_largest(&data, &spans)
        .ok_or_else(|| PvError::UnsupportedFormat("该 RAW 不含可用的内嵌预览".into()))?;
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
/// 从文件开头读最多 `limit` 字节。
///
/// 用 `take(limit)` 而不是先 `metadata().len()` 再判断：既省一次 stat，
/// 也不会因为文件在读取期间变小而读出错。
fn read_up_to(path: &Path, limit: u64) -> Result<Vec<u8>> {
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
    f.take(limit).read_to_end(&mut buf).map_err(|e| PvError::Io {
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
            // 0 = 要最大的那张 → 走全量扫描（这条测试校验的就是「选对了最大的」）
            let preview = extract_embedded_preview(&p, 0)
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
        let bytes = match extract_embedded_preview(&p, 0).unwrap() {
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

    /// ⭐ 快速通道（只读前 4MB）与全量扫描必须**给出同一张预览**。
    ///
    /// 这是「不读 47MB 也能拿到正确预览」的核心护栏：
    /// 对每个样本，`need_edge=256`（允许快速通道）与 `need_edge=0`（强制全扫描）
    /// 得到的字节要完全一致。尼康 NRW 的预览在 87.5% 处 —— 快速通道找不到够大的，
    /// 会自动退回全扫描，因此也一致。
    #[test]
    fn 快速通道与全扫描结果一致() {
        let mut checked = 0;
        let mut cases: Vec<std::path::PathBuf> = Vec::new();
        for name in [
            "canon-cr3-sample.CR3",
            "fuji-raf-sample.RAF",
            "nikon-nrw-sample.NRW",
            "olympus-orf-sample.ORF",
            "panasonic-rw2-sample.RW2",
            "sony-sr2-sample.SR2",
            "sony-srf-sample.SRF",
            "IMG_7507.DNG",
        ] {
            if let Some(p) = sample(name) {
                cases.push(p);
            }
        }
        // 相机实拍的 ARW 不在仓库里（太大），需要时用环境变量点进来：
        //   PV_TEST_RAW="H:\...\_DSC6674.ARW" cargo test --release -p pv-decode 快速通道
        if let Ok(p) = std::env::var("PV_TEST_RAW") {
            let p = std::path::PathBuf::from(p);
            if p.is_file() {
                cases.push(p);
            }
        }

        for p in &cases {
            let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let fast = match extract_embedded_preview(p, 256) {
                Ok(EncodedPreview::Jpeg(b)) => b,
                Ok(other) => panic!("{name} 应产出 Jpeg，实际 {}", other.source_kind()),
                Err(e) => panic!("{name} 快速通道失败：{e}"),
            };
            let full = match extract_embedded_preview(p, 0).unwrap() {
                EncodedPreview::Jpeg(b) => b,
                _ => panic!("{name} 应为 Jpeg"),
            };
            // ⭐ 核心断言：字节必须完全一致（不是「长度接近」）
            assert_eq!(
                fast, full,
                "{name}：快速通道拿到的预览与全扫描不一致（{} vs {} 字节），\
                 说明门槛判断放过了不合适的那张",
                fast.len(),
                full.len()
            );
            eprintln!("  {name}: {} 字节 → 一致 ✓", fast.len());
            checked += 1;
        }
        assert!(checked > 0, "至少应有一个真实样本可用");
        eprintln!("已校验 {checked} 个样本：快速通道 == 全扫描");
    }

    /// 快速通道门槛的边界（纯函数，不碰磁盘）。
    #[test]
    fn 快速通道门槛的边界条件() {
        const BIG: usize = 400 * 1024; // 像样预览的字节数
        const TINY: usize = 25 * 1024; // NRW 那张假预览

        // 缩略图档（256）：ARW 实拍的 1616×490KB → 走快速通道（这是主场景）
        assert!(fast_preview_good_enough(1616, BIG, 256, false));
        assert!(fast_preview_good_enough(1024, BIG, 256, false));

        // ⛔ 回归：NRW 前缀里那张 25KB「大尺寸劣质图」不能充数
        assert!(!fast_preview_good_enough(1600, TINY, 256, false));

        // ⛔ 回归：相机的小程序图（320/640）哪怕字节数够也不行
        assert!(!fast_preview_good_enough(320, BIG, 256, false));
        assert!(!fast_preview_good_enough(640, BIG, 256, false));

        // 预览档（2048）：1616 不够大 → 走全扫描（宁可贵一点，也不 settle 到更小的）
        assert!(!fast_preview_good_enough(1616, BIG, 2048, false));
        // 导出档的 4096 同理
        assert!(!fast_preview_good_enough(1616, BIG, 4096, false));

        // 前缀即全文 → 前缀里最大的就是全局最大，再读也没用，接受
        assert!(fast_preview_good_enough(320, TINY, 4096, true));
    }


    #[test]
    fn 非RAW文件不panic() {
        let dir = std::env::temp_dir().join("pv-raw-test");
        std::fs::create_dir_all(&dir).unwrap();

        // 两条路径都要覆盖：256 走「快速通道」，0 走「全量扫描」
        let junk = dir.join("junk.bin");
        std::fs::write(&junk, vec![0x5Au8; 100_000]).unwrap();
        assert!(extract_embedded_preview(&junk, 256).is_err());
        assert!(extract_embedded_preview(&junk, 0).is_err());

        let empty = dir.join("empty.bin");
        std::fs::write(&empty, b"").unwrap();
        assert!(extract_embedded_preview(&empty, 256).is_err());
        assert!(extract_embedded_preview(&empty, 0).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 不存在的文件返回NotFound() {
        // ⭐ 快速通道不能把 I/O 错误吞成「这个文件没有预览」：
        // 它只把错误吞成 `None` 让调用方走兜底，**兜底必须把真正的错误报上来**。
        assert!(
            matches!(
                extract_embedded_preview(Path::new("Z:/绝对不存在/x.nef"), 256),
                Err(PvError::NotFound(_))
            ),
            "走快速通道时也应报 NotFound，而不是「无内嵌预览」"
        );
        assert!(matches!(
            extract_embedded_preview(Path::new("Z:/绝对不存在/x.nef"), 0),
            Err(PvError::NotFound(_))
        ));
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
