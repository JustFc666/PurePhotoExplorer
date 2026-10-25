//! 扫描时的格式判定（**只看魔数，不信扩展名**）。
//!
//! 复用 [`pv_core::format::probe_header`]，因此本 crate 不需要 image 解码依赖。

use crate::Result;
use pv_core::format::{probe_header, ImageFormat};
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// 读文件头的字节数：ISOBMFF 系（CR3/HEIF/AVIF）要看偏移 8 的 `ftyp` brand，
/// 64 字节足够覆盖本项目所有格式的判定。
const HEAD_LEN: usize = 64;

/// 把探测到的格式映射成**索引用的短标签**；不可索引的格式返回 `None`。
///
/// 不可索引 = 本项目没有解码器（JXL）或本就不处理（视频/未知）。
pub fn indexable_tag(f: ImageFormat) -> Option<&'static str> {
    use ImageFormat::*;
    Some(match f {
        Jpeg => "jpeg",
        Png => "png",
        Gif => "gif",
        Bmp => "bmp",
        WebP => "webp",
        Tiff => "tiff",
        Avif => "avif",
        Heif => "heif",
        Psd => "psd",
        CanonCr2 => "cr2",
        CanonCr3 => "cr3",
        NikonNef => "nef",
        SonyArw => "arw",
        FujiRaf => "raf",
        AdobeDng => "dng",
        PentaxPef => "pef",
        PanasonicRw2 => "rw2",
        OlympusOrf => "orf",
        PhaseOneIiq => "iiq",
        // TIFF 容器歧义：`probe_header` 分不清普通 TIFF 与 RAW 时会落到这里，
        // 先按 raw 记；真正的降级由解码注册表在解码时处理（后续步骤）。
        OtherRaw => "raw",
        Jxl | Video | Unknown => return None,
    })
}

/// 探测一个文件是否是可索引的图像，返回 `(格式标签, 是否 RAW)`。
pub fn probe_file(path: &Path) -> Result<Option<(String, bool)>> {
    let mut f = File::open(path)?;
    let mut head = [0u8; HEAD_LEN];
    let n = f.read(&mut head)?;
    let fmt = probe_header(&head[..n]).format;
    Ok(indexable_tag(fmt).map(|t| (t.to_string(), fmt.is_raw())))
}

/// 由**扩展名**推格式标签 —— 浏览模式的快路径，**不打开文件**。
///
/// ## ⛔ 这不违反「只看魔数不信扩展名」
/// 两条路的用途不同，判据自然不同：
///
/// | | [`probe_file`]（入库） | 本函数（浏览） |
/// |---|---|---|
/// | 决定什么 | **要不要进索引**，写进 `library.db` | 只是「先列出候选」，零持久化 |
/// | 判错的代价 | 索引里多一条垃圾 / 少一张真图 | 那张卡片的格式标签标错，缩略图照常 |
/// | 代价 | 每个文件一次 `open` + `read` | 纯字符串比较 |
///
/// 为什么值得单独开一条路：**每个文件一次 `open` 在机械盘上是 8ms 级的随机读**。
/// 实测一个 1224 个文件的目录，`walk_dir` 冷盘要 **9.47 秒**（热缓存 60ms）——
/// 用户看到的就是「正在处理…」转十秒。而其中 100% 的文件扩展名都在白名单里，
/// 那 1224 次 `open` 一次都不必要。
///
/// 缩略图解码走的是 `pv_core::format::detect`（魔数），所以**显示不会错**；
/// 真入库时仍然走 [`probe_file`]，索引仍然准。
///
/// 返回 `None` = 扩展名不认识，调用方应回落到 [`probe_file`]（这样「无扩展名的图」
/// 和「扩展名写法奇怪但其实是图」的漏不掉）。
pub fn guess_by_extension(name: &str) -> Option<(&'static str, bool)> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "jpg" | "jpeg" | "jpe" | "jfif" => ("jpeg", false),
        "png" => ("png", false),
        "gif" => ("gif", false),
        "bmp" => ("bmp", false),
        "webp" => ("webp", false),
        // ⚠️ 普通 TIFF 也会被魔数判成 `OtherRaw` → "raw"（16 字节分不清 TIFF 与相机 RAW，
        // 真正的降级在解码层做）。这里跟着写 "raw"，保证「浏览」与「入库」标签一致。
        // 副作用：`.tif` 在界面上显示为 RAW，且不会被探尺寸 —— 与入库后的行为相同。
        "tif" | "tiff" => ("raw", true),
        "avif" => ("avif", false),
        // ⚠️ HEIC / HEIF 都归到 `heif`：`indexable_tag` 对 Heif 统一返回 "heif"，
        // 两边必须一致，否则同一个文件「浏览时叫 heic、入库后叫 heif」。
        "heic" | "heif" => ("heif", false),
        "psd" => ("psd", false),
        // ⚠️ TIFF 容器家族（dng/cr2/nef/arw/pef/iiq）**魔数判定都是 `OtherRaw` → "raw"**：
        // `probe_header` 只有 16 字节，分不清「普通 TIFF」和「相机 RAW」，
        // 更分不清是哪家相机（`pv_core::format::detect` 的既有行为，别试图改它）。
        // 所以这里必须写 "raw" 而不是各自的 "dng"/"arw"，否则同一个文件会出现
        // 「浏览时叫 dng、加库后叫 raw」——护栏测试 `快速遍历与魔数遍历结果一致` 会抓。
        "cr2" | "nef" | "arw" | "dng" | "pef" | "iiq" => ("raw", true),
        // 这几种有**独立的魔数**，能分清，扩展名映射与魔数一致：
        "cr3" => ("cr3", true),
        "raf" => ("raf", true),
        "rw2" => ("rw2", true),
        "orf" => ("orf", true),
        // SR2/SRF 的魔数也落在 `OtherRaw`
        "sr2" | "srf" => ("raw", true),
        _ => return None,
    })
}
