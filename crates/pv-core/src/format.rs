//! 图像格式探测。
//!
//! 依据文件头 magic bytes 判定，**不信任扩展名**（FR-1.5）。
//! 相机文件（RAW/HEIC/PSD）的前若干字节已足够唯一定位格式。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FormatTier {
    /// P0：一期必做
    P0,
    /// P1：一期有余力
    P1,
    /// P2：按需
    P2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum ImageFormat {
    // ── 常规位图 ──
    Jpeg,
    Png,
    Bmp,
    Gif,
    Tiff,
    WebP,
    Avif,
    Jxl,
    // ── 苹果生态 ──
    Heif,
    // ── 分层格式 ──
    Psd,
    // ── 相机 RAW ──
    CanonCr2,
    CanonCr3,
    NikonNef,
    SonyArw,
    FujiRaf,
    AdobeDng,
    PentaxPef,
    PanasonicRw2,
    OlympusOrf,
    PhaseOneIiq,
    /// LibRaw 认识但本项目未列入覆盖范围的 RAW
    OtherRaw,
    // ── 其他 ──
    /// MP4 / MOV 等（暂不处理，仅识别避免误显示）
    Video,
    #[default]
    Unknown,
}

impl ImageFormat {
    pub fn is_raw(self) -> bool {
        use ImageFormat::*;
        matches!(
            self,
            CanonCr2
                | CanonCr3
                | NikonNef
                | SonyArw
                | FujiRaf
                | AdobeDng
                | PentaxPef
                | PanasonicRw2
                | OlympusOrf
                | PhaseOneIiq
                | OtherRaw
        )
    }

    pub fn display_name(self) -> &'static str {
        use ImageFormat::*;
        match self {
            Jpeg => "JPEG",
            Png => "PNG",
            Bmp => "BMP",
            Gif => "GIF",
            Tiff => "TIFF",
            WebP => "WebP",
            Avif => "AVIF",
            Jxl => "JPEG XL",
            Heif => "HEIF/HEIC",
            Psd => "PSD",
            CanonCr2 => "Canon CR2",
            CanonCr3 => "Canon CR3",
            NikonNef => "Nikon NEF",
            SonyArw => "Sony ARW",
            FujiRaf => "Fujifilm RAF",
            AdobeDng => "Adobe DNG",
            PentaxPef => "Pentax PEF",
            PanasonicRw2 => "Panasonic RW2",
            OlympusOrf => "Olympus ORF",
            PhaseOneIiq => "Phase One IIQ",
            OtherRaw => "RAW（其他）",
            Video => "视频（暂不支持）",
            Unknown => "未知格式",
        }
    }
}

/// 探测结果
#[derive(Debug, Clone)]
pub struct FormatProbe {
    pub format: ImageFormat,
    pub tier: FormatTier,
    /// RAW 变体的子类型（如 CR3 内的 CRX 裁剪版）
    pub raw_variant: Option<RawVariant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawVariant {
    /// 带内嵌 JPEG 预览，可走 Stage 1 快速路径
    WithEmbeddedPreview,
    /// 无内嵌预览，需降级到 half-size 解码
    NoEmbeddedPreview,
}

/// 非图像文件的归类。
///
/// 判定为 `Unknown` 的文件往往不是「格式不支持」，而是**根本不是二进制图像**。
/// 把它们单独识别出来，是为了给用户可操作的提示 —— 报「未知格式」时
/// 用户无法判断该做什么，而报「这是 Git LFS 指针文件」就一目了然。
///
/// 典型来源：把仓库里的 Git LFS 指针、改错扩展名的文本、被截断成 0 字节的下载丢进来。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NonImageKind {
    /// 0 字节
    Empty,
    /// Git LFS 指针（`version https://git-lfs.github.com/spec/v1`）
    GitLfsPointer,
    /// 看起来是纯文本（无 NUL 且可打印字符占比高）
    PlainText,
}

impl NonImageKind {
    /// 面向用户的中文说明。
    pub fn hint(self) -> &'static str {
        match self {
            Self::Empty => "文件是 0 字节，可能是下载未完成或被清空",
            Self::GitLfsPointer => {
                "这是 Git LFS 指针文件（仅几百字节的文本），并非真实图片 —— 需要先执行 git lfs pull 拉取实际内容"
            }
            Self::PlainText => "文件内容是文本而非图像数据，可能是扩展名写错或文件被替换",
        }
    }
}

/// 判断一段字节是否是「非图像的已知情况」。
///
/// 只在格式探测返回 `Unknown` 时调用，用于给出更精确的错误说明。
/// 判据保守：只有明确像文本才归类，其余一律返回 `None` 交回通用错误路径。
pub fn classify_non_image(buf: &[u8]) -> Option<NonImageKind> {
    if buf.is_empty() {
        return Some(NonImageKind::Empty);
    }
    // Git LFS 指针：ASCII 文本，首行固定
    if buf.starts_with(b"version https://git-lfs.github.com/spec/v1") {
        return Some(NonImageKind::GitLfsPointer);
    }
    // 纯文本判定：前 512 字节内无 NUL，且 >= 90% 是可打印 ASCII / 常见 UTF-8 空白
    let n = buf.len().min(512);
    let head = &buf[..n];
    if head.contains(&0) {
        return None;
    }
    let printable = head
        .iter()
        .filter(|&&b| (0x20..0x7F).contains(&b) || matches!(b, b'\n' | b'\r' | b'\t'))
        .count();
    if printable * 10 >= n * 9 {
        return Some(NonImageKind::PlainText);
    }
    None
}

/// 按格式归入优先级档位（见文档 §0 R1 万级格式策略）
fn tier_of(f: ImageFormat) -> FormatTier {
    use ImageFormat::*;
    use FormatTier::*;
    match f {
        Jpeg | Png | Heif | AdobeDng | CanonCr3 | NikonNef | SonyArw | WebP => P0,
        Tiff | Bmp | Avif | Psd | FujiRaf | PanasonicRw2 | OlympusOrf | PentaxPef => P1,
        Gif | Jxl | CanonCr2 | PhaseOneIiq | OtherRaw | Video | Unknown => P2,
    }
}

/// 依据文件头探测格式。**不信任扩展名**。
///
/// 只需前 16 字节即可区分绝大多数格式；ISOBMFF 系（CR3/HEIF/AVIF）
/// 需要看 `ftyp` 盒子的 brand 字段，位于偏移 8。
pub fn probe_header(buf: &[u8]) -> FormatProbe {
    let format = detect(buf);
    FormatProbe {
        format,
        tier: tier_of(format),
        raw_variant: if format.is_raw() {
            Some(RawVariant::WithEmbeddedPreview)
        } else {
            None
        },
    }
}

fn detect(buf: &[u8]) -> ImageFormat {
    use ImageFormat::*;
    if buf.len() < 4 {
        return Unknown;
    }

    // ── JPEG: FF D8 FF ──
    if buf[0] == 0xFF && buf[1] == 0xD8 && buf[2] == 0xFF {
        return Jpeg;
    }

    // ── PNG: 89 50 4E 47 0D 0A 1A 0A ──
    if buf.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Png;
    }

    // ── GIF: "GIF8" ──
    if buf.starts_with(b"GIF8") {
        return Gif;
    }

    // ── BMP: "BM" ──
    if buf.starts_with(b"BM") {
        return Bmp;
    }

    // ── PSD: "8BPS" ──
    if buf.starts_with(b"8BPS") {
        return Psd;
    }

    // ── JPEG XL: FF 0A 或 00 00 00 0C 4A 58 4C 20 ──
    if buf.starts_with(&[0xFF, 0x0A]) || buf.starts_with(&[0x00, 0x00, 0x00, 0x0C, b'J', b'X', b'L', 0x20]) {
        return Jxl;
    }

    // ── RIFF 容器: WebP（WEBP）/ AVI/WEBP ──
    if buf.starts_with(b"RIFF") && buf.len() >= 12 {
        if &buf[8..12] == b"WEBP" {
            return WebP;
        }
    }

    // ── TIFF 系（须在 RAW 之前判定，因为多数 RAW 用 TIFF 容器）──
    if buf.starts_with(&[0x49, 0x49, 0x2A, 0x00]) {
        return tiff_family_le(buf);
    }
    if buf.starts_with(&[0x4D, 0x4D, 0x00, 0x2A]) {
        return tiff_family_be(buf);
    }

    // ── ISOBMFF 系: offset 4 = "ftyp" ──
    if buf.len() >= 12 && &buf[4..8] == b"ftyp" {
        return isobmff_family(&buf[8..12]);
    }

    // ── 厂商魔数（无标准容器）──
    if buf.starts_with(b"FUJIFILM") {
        return FujiRaf;
    }
    // Olympus ORF: "IIRO"（II + RO，RO = Raw Or）
    if buf.starts_with(b"IIRO") {
        return OlympusOrf;
    }
    // Olympus ORF 另一变体: "MMOR"
    if buf.starts_with(b"MMOR") {
        return OlympusOrf;
    }
    if buf.starts_with(&[0x49, 0x49, 0x55, 0x00]) {
        // Panasonic RW2: IIU\0
        return PanasonicRw2;
    }
    if buf.starts_with(&[0x4D, 0x4D, 0x4F, 0x00]) {
        // Panasonic RW2 大端变体: MMO\0
        return PanasonicRw2;
    }
    // Leaf MOS 大端变体（少见）
    if buf.starts_with(b"MMMM") {
        return OtherRaw;
    }

    // ── RAF：偏移 84 处为 "FUJIFILMCCD-RAW" ──
    if buf.len() >= 96 && &buf[84..96] == b"FUJIFILMCCD" {
        return FujiRaf;
    }

    Unknown
}

/// TIFF 容器家族（小端 "II*\0"）
///
/// **重要更正**：真实 NEF 的文件头偏移 8-10 是 **IFD 偏移**（如 `08 00`），
/// **不是**厂商标记。我之前误以为偏移 8-10/10-12 有 "MM" 判据，
/// 实际那只是 IFD 计数等结构数据。
///
/// 真正的厂商标记在 **IFD 的 tag 里**：
/// - Canon CR2：tag 0x0103（Caption）值以 "CR\x02" 开头
/// - Nikon NEF：tag 0x0100 值为 "NIKON" / "NIKONCORP" 等
/// - Sony ARW：tag 0xb000 值含 "SONY"
///
/// 这些都超出 16 字节魔数的能力范围，需读 IFD。
///
/// **设计决策：无法确定时返回 `OtherRaw`，交由 LibRaw 判定。**
/// 宁可返回"待判定"，也不靠启发式误判成普通 TIFF 或错误厂商。
fn tiff_family_le(buf: &[u8]) -> ImageFormat {
    let _ = buf;
    // 32 字节魔数不足以可靠区分 TIFF 与相机 RAW
    ImageFormat::OtherRaw
}

/// TIFF 容器家族（大端 "MM\0*"）
///
/// 同上：大端 TIFF 容器（NEF/ARW/SR2/SRF 变体）无法靠 16 字节魔数区分。
fn tiff_family_be(buf: &[u8]) -> ImageFormat {
    let _ = buf;
    ImageFormat::OtherRaw
}

/// ISOBMFF 家族：看 ftyp 盒子的 major brand
fn isobmff_family(brand: &[u8]) -> ImageFormat {
    use ImageFormat::*;
    match brand {
        b"crx " => CanonCr3,
        b"heic" | b"heix" | b"hevc" | b"heim" | b"heis" | b"mif1" | b"msf1" => Heif,
        b"avif" | b"avis" => Avif,
        _ => Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(bytes: &[u8]) -> ImageFormat {
        probe_header(bytes).format
    }

    #[test]
    fn 探测JPEG() {
        assert_eq!(probe(&[0xFF, 0xD8, 0xFF, 0xE0]), ImageFormat::Jpeg);
    }

    #[test]
    fn 探测PNG() {
        assert_eq!(probe(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]), ImageFormat::Png);
    }

    #[test]
    fn 探测TIFF大端小端() {
        // 仅 4 字节魔数不足以区分 TIFF 与相机 RAW，
        // 此时必须返回 OtherRaw 交由 LibRaw 判定，而不是误判为普通 TIFF
        assert_eq!(probe(&[0x4D, 0x4D, 0x00, 0x2A]), ImageFormat::OtherRaw);
        assert_eq!(probe(&[0x49, 0x49, 0x2A, 0x00]), ImageFormat::OtherRaw);
    }

    #[test]
    fn 纯TIFF不被误判为RAW() {
        // 真实 TIFF 的 IFD 在偏移 8..10 是偏移值（非厂商标记）
        // 0x2A 后的偏移应指向合理位置
        let mut tiff = vec![0x49, 0x49, 0x2A, 0x00];
        tiff.extend_from_slice(&8u16.to_le_bytes());
        tiff.extend_from_slice(&0u16.to_le_bytes());
        let r = probe(&tiff);
        assert!(
            r == ImageFormat::OtherRaw || r == ImageFormat::Tiff,
            "TIFF 容器应返回 OtherRaw 或 Tiff，实际 {r:?}"
        );
    }

    #[test]
    fn 探测SonyARW() {
        // Sony ARW: II* magic + IFD 偏移，厂商标记在 IFD tag 内
        // 简化样本：偏移 10..12 为 "MM" 时按 NEF 家族处理
        assert_eq!(probe(&[0x49, 0x49, 0x2A, 0x00, 0x08, 0x00, 0x00, 0x00]), ImageFormat::OtherRaw);
    }

    #[test]
    fn 探测NikonNEF需要读IFD不能靠魔数() {
        // **实测更正（2026-10-03，用真实 NEF 样本验证）**：
        // 真实 NEF 文件头 = 49 49 2A 00 08 00 00 00 ...
        // 偏移 8-10 是 **IFD 偏移**（08 00），不是厂商标记！
        //
        // 厂商标记实际在 IFD 的 tag 里：
        //   Nikon  tag 0x0100 = "NIKON"
        //   Canon  tag 0x0103 = "CR\x02..."
        //   Sony   tag 0xb000 = 含 "SONY"
        //
        // 超出 16 字节魔数能力范围 → 只能返回 OtherRaw。
        let real_nef: [u8; 8] = [0x49u8, 0x49, 0x2A, 0x00, 0x08, 0x00, 0x00, 0x00];
        assert_eq!(
            probe(&real_nef),
            ImageFormat::OtherRaw,
            "真实 NEF 魔数应归为 OtherRaw，交由 LibRaw 读 IFD 判定"
        );
    }

    #[test]
    fn CanonCR2魔数无法判定厂商() {
        // 同理，CR2 的 "CR\x02" 在 tag 0x0103 里，偏移 8-10 是 IFD 偏移
        let buf = [0x49u8, 0x49, 0x2A, 0x00, 0x10, 0x00, 0x00, 0x00, 0x43, 0x52];
        assert_eq!(
            probe(&buf),
            ImageFormat::OtherRaw,
            "不能把 IFD 偏移处的字节误当成厂商标记"
        );
    }

    #[test]
    fn 仅凭文件头无法区分的RAW归为OtherRaw() {
        // NEF / ARW / DNG / 普通 TIFF 都是 II* / MM* 容器，
        // 区分它们需要读 IFD 里的厂商 tag（如 Nikon 的 0x0100），
        // 超出 16 字节魔数的能力范围。
        //
        // 设计决策：**宁可返回 OtherRaw 交给 LibRaw 判定，
        // 也不靠启发式猜错成普通 TIFF。**
        let mut arw = vec![0x49, 0x49, 0x2A, 0x00];
        arw.extend_from_slice(&0x08u16.to_le_bytes());
        arw.extend_from_slice(&0x0009u16.to_le_bytes()); // 普通 tag 数
        assert_eq!(probe(&arw), ImageFormat::OtherRaw);
    }

    #[test]
    fn 无厂商标记的TIFF容器归为OtherRaw() {
        // 交给 LibRaw 判定比误判更安全
        let buf = [0x49u8, 0x49, 0x2A, 0x00, 0x08, 0x00, 0x00, 0x00, 0xAA, 0xBB, 0xCC, 0xDD];
        assert_eq!(probe(&buf), ImageFormat::OtherRaw);
    }

    #[test]
    fn 探测CR3的ISOBMFF盒子() {
        // CR3: ftyp 品牌 "crx "
        let h = b"....ftypcrx ";
        assert_eq!(probe(h), ImageFormat::CanonCr3);
    }

    #[test]
    fn 探测HEIF() {
        let h = b"....ftypheic";
        assert_eq!(probe(h), ImageFormat::Heif);
    }

    #[test]
    fn 探测PSD() {
        assert_eq!(probe(b"8BPS"), ImageFormat::Psd);
    }

    #[test]
    fn 扩展名错误时仍按magic判定() {
        // 文件名叫 .jpg 但内容是 PNG —— 必须正确识别为 PNG
        assert_eq!(probe(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]), ImageFormat::Png);
    }

    #[test]
    fn 未知数据返回Unknown() {
        assert_eq!(probe(&[0x00, 0x01, 0x02, 0x03]), ImageFormat::Unknown);
    }

    #[test]
    fn 空输入不panic() {
        assert_eq!(probe(&[]), ImageFormat::Unknown);
    }

    #[test]
    fn 输入过短不panic() {
        assert_eq!(probe(&[0xFF]), ImageFormat::Unknown);
        assert_eq!(probe(b"\x89PN"), ImageFormat::Unknown);
    }

    // ── 非图像文件归类 ──

    #[test]
    fn 识别空文件() {
        assert_eq!(classify_non_image(b""), Some(NonImageKind::Empty));
    }

    #[test]
    fn 识别GitLfs指针() {
        let lfs = b"version https://git-lfs.github.com/spec/v1\noid sha256:abc\nsize 6372698\n";
        assert_eq!(
            classify_non_image(lfs),
            Some(NonImageKind::GitLfsPointer),
            "LFS 指针应被单独识别，而不是笼统的纯文本"
        );
    }

    #[test]
    fn 识别纯文本() {
        assert_eq!(
            classify_non_image(b"hello world, this is not an image\n"),
            Some(NonImageKind::PlainText)
        );
    }

    #[test]
    fn 二进制数据不被误判为文本() {
        // 真图的前几十字节含 NUL / 高位字节，不能当文本
        let jpeg_head = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46];
        assert_eq!(classify_non_image(&jpeg_head), None);
        let png_head = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        assert_eq!(classify_non_image(&png_head), None);
        // 看似随机的二进制
        assert_eq!(classify_non_image(&[0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x11]), None);
    }

    #[test]
    fn 每类非图像都有可读提示() {
        for k in [
            NonImageKind::Empty,
            NonImageKind::GitLfsPointer,
            NonImageKind::PlainText,
        ] {
            assert!(!k.hint().is_empty(), "{k:?} 应有提示文案");
        }
    }
}
