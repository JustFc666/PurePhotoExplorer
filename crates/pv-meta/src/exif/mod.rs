//! EXIF 读取：文件 → [`ExifData`]。
//!
//! ## 支持的容器
//! - **JPEG / PNG / HEIF·HEIC·AVIF / WebP**：交给 kamadak-exif 的容器解析
//!   （它只读 EXIF 段，不会把整张图读进内存）；
//! - **TIFF 家族（含 RAW）**：TIFF、NEF、CR2、CR3 之外的 ARW/DNG，
//!   以及魔数不标准的 **ORF（`IIRO`/`MMOR`）、RW2（`IIU\0`）** ——
//!   这些文件本身就是 TIFF 结构，只是「版本标识」被厂商改了；
//!   本模块把 2..4 字节改回 `*\0` 后按裸 TIFF 解析（改的是标识，不动任何数据偏移）。
//!
//! ## 为什么 RAW 现在就能读 EXIF
//! RAW 一期还解不了码（LibRaw 未接入），但拍摄参数就写在文件头部的 IFD 里，
//! 顺手读出来给用户看是**零成本的信息增量**：用户能立刻知道这张 RAW 的机型/镜头/参数。
//!
//! ## 内存与失败策略
//! - TIFF 家族按 [`MAX_SCAN`] 截断读取，避免为读几百字节元数据而把 200 MB 的 DNG 读进内存；
//! - 截断/部分字段损坏时用 `continue_on_error`，**能读多少读多少**；
//! - 任何失败都只记为调试日志并返回 `None` —— EXIF 是锦上添花，**绝不能影响看图**。

mod extract;
mod format;
mod rewrite;

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use exif::Reader;
use serde::{Deserialize, Serialize};

pub use format::{
    date_time, exposure_bias, exposure_program, exposure_time, f_number, flash, focal_length,
    gps_altitude, gps_coord, gps_decimal, metering_mode, orientation, white_balance,
};
pub use rewrite::{align_with_pixels as align_exif_with_pixels, Outcome as ExifRewriteOutcome};

/// 拍摄地点（EXIF GPS）。
///
/// ⚠️ **字段一律是字符串，不用 `f64`**：`ExifData` 派生了 `Eq`（用于
/// `is_empty()` 的“一个字段都没读到”判断），而 `f64` 不满足 `Eq`。
/// 反正展示文本要 6 位小数、地图链接要原始数字，两种都是字符串更省事 ——
/// 数值只在 [`format::gps_decimal`] 内部以 `f64` 存在。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpsData {
    /// 纬度（十进制度，**南纬为负**），如 `31.204839` —— 供地图链接使用
    pub latitude: String,
    /// 经度（十进制度，**西经为负**），如 `121.440628`
    pub longitude: String,
    /// 海拔（米，海平面以下为负）；多数设备不记录，缺失为 `None`
    pub altitude: Option<String>,
    /// 面向展示的文本，如 `31°12'17.4"N, 121°26'26.3"E`
    pub display: String,
}

impl GpsData {
    /// 解析出数值坐标 `(纬度, 经度)`，供反查地名等计算使用。
    ///
    /// 字段本身是字符串（见 [`GpsData`] 的说明），所以这里 parse 一次 ——
    /// 只在真正需要计算的调用方（`pv-geo`）里用，展示路径完全不碰。
    pub fn lat_lon(&self) -> Option<(f64, f64)> {
        Some((self.latitude.parse().ok()?, self.longitude.parse().ok()?))
    }
}

/// EXIF 容器最大扫描字节数。
///
/// EXIF IFD 总是紧邻文件头（IFD0 在前，EXIF IFD 紧跟其后，再往后才是图像数据），
/// 8 MiB 足够覆盖所有常见机型；超出部分只会是像素数据或厂商私有段落。
const MAX_SCAN: u64 = 8 * 1024 * 1024;

/// 拍摄参数（面向展示：数值已格式化成 `f/2.8`、`1/250 s` 这类人类可读形态）。
///
/// 所有字段缺失时为 `None`（前端按「有值才显示」渲染）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExifData {
    /// 相机厂商，如 `NIKON CORPORATION`
    pub make: Option<String>,
    /// 相机型号，如 `NIKON Z 6_2`
    pub model: Option<String>,
    /// 镜头，如 `NIKKOR Z 24-70mm f/2.8 S`
    pub lens: Option<String>,
    /// 拍摄时间（已转成 `2024-05-01 18:32:10`）
    pub date_time_original: Option<String>,
    /// 快门速度，如 `1/250 s`
    pub exposure_time: Option<String>,
    /// 光圈，如 `f/2.8`
    pub f_number: Option<String>,
    /// ISO 感光度
    pub iso: Option<u32>,
    /// 焦距，如 `50 mm`
    pub focal_length: Option<String>,
    /// 35mm 等效焦距
    pub focal_length_35mm: Option<u32>,
    /// 曝光补偿，如 `-0.3 EV`
    pub exposure_bias: Option<String>,
    /// 曝光程序，如 `光圈优先`
    pub exposure_program: Option<String>,
    /// 测光模式，如 `评价测光`
    pub metering_mode: Option<String>,
    /// 闪光灯，如 `未闪光`
    pub flash: Option<String>,
    /// 白平衡，如 `自动`
    pub white_balance: Option<String>,
    /// 图像方向，如 `顺时针 90°`
    pub orientation: Option<String>,
    /// 写入该文件的软件，如 `Adobe Photoshop Lightroom`
    pub software: Option<String>,
    /// 拍摄地点（GPS）。
    ///
    /// ⚠️ 只有开了定位的手机（以及少数带 GPS 模块的相机）才会写，
    /// **绝大多数照片没有** —— 前端按 `null` 整行隐藏，不留空行。
    pub gps: Option<GpsData>,
}

impl ExifData {
    /// 是否一个字段都没读到。
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// EXIF 读取错误（**仅内部使用**：对外一律降级为 `None`）。
#[derive(Debug, thiserror::Error)]
pub enum MetaError {
    #[error("读取文件失败：{0}")]
    Io(#[from] std::io::Error),
    #[error("解析 EXIF 失败：{0}")]
    Exif(#[from] exif::Error),
}

/// 读取文件的 EXIF 拍摄参数。
///
/// 返回 `None` 表示「这个文件没有可用的拍摄参数」（无 EXIF、容器不支持或数据损坏），
/// 调用方不需要区分原因 —— 界面上就是不显示「拍摄参数」分区。
pub fn read_exif(path: &Path) -> Option<ExifData> {
    match read_exif_inner(path) {
        Ok(data) if !data.is_empty() => Some(data),
        Ok(_) => None,
        Err(e) => {
            tracing::debug!(path = %path.display(), error = %e, "读取 EXIF 失败，已忽略");
            None
        }
    }
}

/// 读文件里的**原始 EXIF 段**（裸 TIFF 字节），供「写回/另存为时带上拍摄参数」使用。
///
/// ## 与 [`read_exif`] 的分工
/// `read_exif` 把字段解析成 [`ExifData`] 给界面**显示**；本函数**不做解析**，
/// 只把那段 TIFF 原样取出来 —— 写回时要的是能直接塞进 JPEG `APP1` /
/// PNG `eXIf` 的字节。
///
/// 拿到的数据**不能直接写入**：旋转是把像素转正后写出的，而源 EXIF 里的
/// `Orientation` 还指着旧方向，直接搬会导致双重旋转。写之前必须先过一遍
/// [`align_exif_with_pixels`]。
///
/// 返回 `None` 表示这个文件没有可搬运的 EXIF（或结构不认识）——
/// 调用方直接不带元数据写出去即可，不是错误。
pub fn read_exif_raw(path: &Path) -> Option<Vec<u8>> {
    read_exif_raw_inner(path).ok().flatten()
}

/// 可搬运的 EXIF 体积上限。
///
/// JPEG 的 APP1 段长度字段是 `u16`（含自身 2 字节），所以**载荷不能超过
/// 65533 字节**；这里留一点余量取 60 KB。典型相机的 EXIF（去掉缩略图后）
/// 只有几 KB，撞不上这个上限。
///
/// 类型是 `u64`（不是 `usize`）：它直接喂给 `Read::take`。
const EXIF_EMBED_MAX: u64 = 60 * 1024;

fn read_exif_raw_inner(path: &Path) -> Result<Option<Vec<u8>>, MetaError> {
    let mut head = [0u8; 4];
    let read = {
        let mut f = BufReader::new(File::open(path)?);
        f.read(&mut head)?
    };
    if read < 4 {
        return Ok(None);
    }

    // ── TIFF 家族（含各类 RAW）：文件本身就是 EXIF 容器 ──
    if is_tiff_like(&head) {
        let mut f = File::open(path)?;
        let mut data = Vec::new();
        f.by_ref().take(EXIF_EMBED_MAX).read_to_end(&mut data)?;
        normalize_tiff_magic(&mut data);
        return Ok(tiff_body(data));
    }

    // 其余容器都只需要文件前部（EXIF 段总在紧邻文件头处）
    let mut f = File::open(path)?;
    let mut data = Vec::new();
    f.by_ref().take(MAX_SCAN).read_to_end(&mut data)?;

    // ── JPEG：APP1 段 ──
    if head.starts_with(&[0xFF, 0xD8]) {
        if let Some(pos) = find_bytes(&data, &[0xFF, 0xD8, 0xFF]) {
            if let Some(tiff) = find_app1_in_jpeg(&data, pos) {
                return Ok(tiff_body(tiff));
            }
        }
    }

    // ── PNG：eXIf chunk（PNG 1.5+ / ISO 规范，内容也是裸 TIFF）──
    if head.starts_with(&[0x89, b'P', b'N', b'G']) {
        if let Some(tiff) = find_png_exif(&data) {
            return Ok(tiff_body(tiff));
        }
    }

    // ── 兜底：扫 `Exif\0\0` 标记 ──
    // 覆盖 HEIF/HEIC（Exif item）、WebP（RIFF 里的 EXIF chunk）、CR3 等 ——
    // 它们的元数据都是「`Exif\0\0` + 裸 TIFF」这一个套路。
    Ok(find_exif_marker(&data).and_then(tiff_body))
}

/// 校验 TIFF 头并修正厂商标识；不含合法 TIFF 头就返回 `None`。
fn tiff_body(mut data: Vec<u8>) -> Option<Vec<u8>> {
    if data.len() < 8 {
        return None;
    }
    normalize_tiff_magic(&mut data);
    // 魔数必须是 42（标准）；厂商私有值（ORF/RW2）在上一行已被修回来
    let magic = match data[..2] {
        [b'I', b'I'] => u16::from_le_bytes([data[2], data[3]]),
        [b'M', b'M'] => u16::from_be_bytes([data[2], data[3]]),
        _ => return None,
    };
    if magic != 42 {
        return None;
    }
    Some(data)
}

/// 在 PNG 里找 `eXIf` chunk，返回其载荷（裸 TIFF）。
fn find_png_exif(data: &[u8]) -> Option<Vec<u8>> {
    let mut p = 8; // 跳过 8 字节签名
    while p + 8 <= data.len() {
        let len = u32::from_be_bytes(data[p..p + 4].try_into().ok()?) as usize;
        let ty = &data[p + 4..p + 8];
        let body = p + 8;
        if ty == b"eXIf" {
            return data.get(body..body + len).map(|s| s.to_vec());
        }
        // IEND 之后没有东西了；len 异常（越界）直接放弃
        if ty == b"IEND" || body.checked_add(len + 4)? > data.len() {
            return None;
        }
        p = body + len + 4;
    }
    None
}

/// 扫描 `Exif\0\0` 标记，返回其后的裸 TIFF。
///
/// 两种布局都要认：
/// - **JPEG / PNG / WebP**：标记后**直接**是 TIFF 头；
/// - **HEIF/ISOBMFF 的 Exif item**：标记后先有 4 字节「TIFF 头相对 item 起点的偏移」，
///   再是 TIFF 头。
///
/// ⚠️ 必须用 TIFF 魔数（`II*\0` / `MM\0*`）二次确认 —— 否则像素数据里偶然出现
/// 这 6 个字节都会被当成元数据段，搬进输出文件就是一坨垃圾。
fn find_exif_marker(data: &[u8]) -> Option<Vec<u8>> {
    const MARK: &[u8] = b"Exif\0\0";
    let mut from = 0usize;
    while let Some(rel) = find_bytes(&data[from..], MARK) {
        let after = from + rel + MARK.len();
        for skip in [0usize, 4] {
            if let Some(tiff) = data.get(after + skip..) {
                if looks_like_tiff(tiff) {
                    return Some(tiff.to_vec());
                }
            }
        }
        from = after;
        if from >= data.len() {
            break;
        }
    }
    None
}

/// 8 字节内能确认是 TIFF 头吗（含厂商私有魔数的变体）。
fn looks_like_tiff(d: &[u8]) -> bool {
    let Some(h) = d.get(..8) else { return false };
    let magic_ok = match &h[..2] {
        b"II" => matches!(&h[2..4], [42, 0] | b"RO" | b"U\0"),
        b"MM" => matches!(&h[2..4], [0, 42] | b"OR" | b"\0U"),
        _ => false,
    };
    if !magic_ok {
        return false;
    }
    // IFD0 偏移必须在文件内（挡住随机数据）
    let off = match &h[..2] {
        b"I" => u32::from_le_bytes(h[4..8].try_into().unwrap_or([0; 4])) as usize,
        _ => u32::from_be_bytes(h[4..8].try_into().unwrap_or([0; 4])) as usize,
    };
    off >= 8 && off < (1 << 28)
}

fn read_exif_inner(path: &Path) -> Result<ExifData, MetaError> {    let mut file = BufReader::new(File::open(path)?);

    let mut head = [0u8; 4];
    let read = file.read(&mut head)?;
    if read < 4 {
        // 读不到 4 字节头（空文件/极小文件）→ 谈不上 EXIF
        return Ok(ExifData::default());
    }

    // ── 主路径：按文件头判定容器 ──
    let primary = if is_tiff_like(&head) {
        read_tiff_like(path).ok()
    } else {
        // JPEG / PNG / HEIF / WebP：容器解析（内部只读 EXIF 段）。
        // 注意：探嗅已把游标推到第 4 字节，必须回到文件头，
        // 否则容器识别（看头几字节魔数）与段偏移都会错位。
        file.seek(SeekFrom::Start(0))?;
        Reader::new().read_from_container(&mut file).ok()
    };

    if let Some(exif) = primary {
        let data = extract::extract(&exif);
        if !data.is_empty() {
            return Ok(data);
        }
    }

    // ── 兜底 A：CR3 等 ISOBMFF 容器里的 `CMT1`–`CMT4` box ──
    //
    // CR3 的拍摄参数不在文件头的 TIFF 里，而是藏在 uuid box（CMT1–CMT4）中：
    // 每个 box 内容都是一段独立裸 TIFF（`II*\0` 开头），且**只带一部分 IFD**
    // （CMT1=机型、CMT2=曝光参数…），所以必须逐个解析再合并。
    if let Some(data) = read_cr3_cmt(path) {
        if !data.is_empty() {
            return Ok(data);
        }
    }

    // ── 兜底 B：从内嵌 JPEG 的 APP1 段读 EXIF ──
    //
    // 不少 RAW（实测 Fujifilm RAF）把 EXIF 只写进内嵌预览 JPEG 的 APP1 里，
    // 文件头本身没有 TIFF 结构。既然解码层已经会扫内嵌 JPEG，
    // 这里再用同一份扫描结果取 EXIF 是顺手的 —— 而且相机写的 APP1
    // 往往比厂商私有字段更完整。
    if let Some(data) = read_exif_from_embedded_jpeg(path) {
        if !data.is_empty() {
            return Ok(data);
        }
    }

    Ok(ExifData::default())
}

/// 文件头是否是 TIFF 家族（标准 TIFF 或厂商改过魔数的变体）。
///
/// 只认字节序标记 `II` / `MM` —— ORF 的 `IIRO`、RW2 的 `IIU\0` 都能被它捞住。
fn is_tiff_like(head: &[u8; 4]) -> bool {
    head.starts_with(b"II") || head.starts_with(b"MM")
}

/// 按裸 TIFF 解析（先截断读取，必要时修正魔数）。
fn read_tiff_like(path: &Path) -> Result<exif::Exif, MetaError> {
    let mut file = File::open(path)?;
    let mut data = Vec::new();
    file.by_ref().take(MAX_SCAN).read_to_end(&mut data)?;

    if data.len() < 8 {
        return Err(MetaError::Exif(exif::Error::InvalidFormat("TIFF 头部过短")));
    }
    normalize_tiff_magic(&mut data);

    // 截断或个别字段越界时不要整体失败 —— 能读到的字段照样有用
    parse_tiff_bytes(&data)
        .ok_or(MetaError::Exif(exif::Error::InvalidFormat("TIFF 解析无结果")))
}

/// 把厂商私有魔数改回标准 TIFF 魔数 `*\0` / `\0*`。
///
/// ORF 写 `IIRO`（`RO` = 0x4F52），RW2 写 `IIU\0`（0x55），
/// 但 IFD 偏移与条目布局完全遵循 TIFF 规范，所以改掉这 2 个字节即可解析。
fn normalize_tiff_magic(data: &mut [u8]) {
    match (data[0], data[1]) {
        (b'I', b'I') => {
            data[2] = b'*';
            data[3] = 0;
        }
        (b'M', b'M') => {
            data[2] = 0;
            data[3] = b'*';
        }
        _ => {}
    }
}

/// 从 CR3（ISOBMFF）的 `CMT1`–`CMT4` box 里取 EXIF。
///
/// ## 为什么 CR3 需要单独处理
///
/// CR3 是 ISOBMFF 结构（`ftyp crx ` + `moov` + 若干 `uuid` box + `mdat`），
/// 文件头不是 TIFF，所以 `is_tiff_like` 判定为 false；
/// 而 kamadak-exif 的 `read_from_container` 只认 JPEG/PNG/HEIF 那几套，
/// 不认识 CR3 —— 于是主路径两条都落空。
///
/// ## Canon 的非标准布局（本函数存在的全部理由）
///
/// Canon 把拍摄参数拆到 4 个 `uuid`（类型标识 CMT1–CMT4）里，
/// 每个 box 内容都是一段独立的裸 TIFF，但**布局与 EXIF 规范不同**：
///
/// ```text
/// CMT1  392 B   IFD0 里只有 Make / Model / DateTime
/// CMT2 1064 B   IFD0 里**平铺**着 ExposureTime/FNumber/ISO/FocalLength…
///               ← 规范要求这些属于 ExifIFD，由 IFD0 的 0x8769 指针进入；
///                 CMT2 既没有 0x8769，IFD0 里也没有 Make/Model
/// ```
///
/// 于是 `read_raw` 解析 CMT2 时会因为「IFD0 里没有相机标识」
/// 判定这不是一份 EXIF，把字段全部丢弃 —— 实测现象就是
/// "能显示 Canon EOS M50，但快门/光圈/ISO 全空"。
///
/// 解法：**把 CMT2 的 IFD0 改写成合成 TIFF** ——
/// 新建骨架 IFD0（只放 Make/Model/ExifIFD 指针），
/// 用 0x8769 指回原来那段平铺 IFD；其余字节原样保留（偏移基准不变）。
/// 这样既符合规范，又不用重排任何数据。
fn read_cr3_cmt(path: &Path) -> Option<ExifData> {
    const SCAN: u64 = 1024 * 1024; // CMT box 总在文件最前部，1 MiB 足够

    let mut file = File::open(path).ok()?;
    let mut data = Vec::new();
    file.by_ref().take(SCAN).read_to_end(&mut data).ok()?;

    // CMT1：拿机型（也是本函数的兜底结果，CMT2 合成需要它做 IFD0）
    let cmt1 = parse_cmt_box(&data, b"CMT1");
    let name = cmt1_name(cmt1.as_ref());
    // CMT2：拿曝光参数 —— 它的 IFD0 是"平铺"的，必须先合成成规范布局
    let exposure = parse_cmt_box(&data, b"CMT2")
        .map(|(tiff, _)| tiff)
        .and_then(|tiff| synthesize_exposure_tiff(&tiff, &name))
        .map(|exif| extract::extract(&exif));
    // CMT3/CMT4：厂商私有，能捞多少捞多少
    let extra3 = parse_cmt_box(&data, b"CMT3").map(|(_, d)| d);
    let extra4 = parse_cmt_box(&data, b"CMT4").map(|(_, d)| d);

    let mut merged = cmt1.map(|(_, d)| d).unwrap_or_default();
    for part in [exposure, extra3, extra4].into_iter().flatten() {
        merge_exif(&mut merged, &part);
    }

    if merged.is_empty() {
        None
    } else {
        Some(merged)
    }
}

/// 从 CMT1 解析结果里取 `(make, model)`，供合成 IFD0 使用。
fn cmt1_name(cmt1: Option<&(Vec<u8>, ExifData)>) -> (Option<String>, Option<String>) {
    match cmt1 {
        Some((_, d)) => (d.make.clone(), d.model.clone()),
        None => (None, None),
    }
}

/// 定位并解析单个 CMT box。
///
/// 返回 `(box 内的原始 TIFF 字节, 解析结果)` —— 原始字节给
/// [`synthesize_exposure_tiff`] 用（CMT2 需要重排 IFD 才能解析）。
fn parse_cmt_box(data: &[u8], tag: &[u8; 4]) -> Option<(Vec<u8>, ExifData)> {
    let at = find_bytes(data, tag)?;
    if at < 4 {
        return None;
    }
    // CMT 的 box 长度在它前面 4 字节（大端）
    let len = u32::from_be_bytes([data[at - 4], data[at - 3], data[at - 2], data[at - 1]]) as usize;
    let body = at + 4;
    if len <= 8 || body + 8 > data.len() {
        return None;
    }
    if !is_tiff_like(&[data[body], data[body + 1], data[body + 2], data[body + 3]]) {
        return None;
    }
    let end = (at - 4 + len).min(data.len());
    if end <= body + 8 {
        return None;
    }
    let mut tiff = data[body..end].to_vec();
    normalize_tiff_magic(&mut tiff);
    let exif = parse_tiff_bytes(&tiff)?;
    Some((tiff, extract::extract(&exif)))
}

/// 把 CMT2 那种「IFD0 平铺曝光参数」的 TIFF 改写成规范布局。
///
/// 做法（不搬动任何已有数据）：
///
/// ```text
/// 原文件:  [TIFF头][IFD0: 39 个曝光条目][值数据区...]
/// 合成后:  [TIFF头][新IFD0: Make/Model + ExifIFD 指针][原IFD0 39 个条目][值数据区...]
///                              ↓ 0x8769
/// ```
///
/// 新 IFD0 插在头部之后、原 IFD0 之前，导致**原 IFD0 及其值区的
/// 绝对偏移整体后移** —— 所以插入时必须把所有内部偏移都加上增量。
/// 好在这类 TIFF 的偏移只出现在三处：IFD0 的 `next IFD` 指针、
/// 每个条目（值 > 4 字节时）的「值偏移」、以及 Make/Model 的字符串。
/// 逐项修正即可。
///
/// `name` 是 CMT1 里读到的 `(make, model)` —— **必须写进新 IFD0**，
/// 否则 kamadak-exif 会认为"IFD0 里没有相机标识"而丢弃整份 EXIF。
fn synthesize_exposure_tiff(
    orig: &[u8],
    name: &(Option<String>, Option<String>),
) -> Option<exif::Exif> {
    if orig.len() < 8 || !orig.starts_with(b"II") {
        // 只处理小端（CR3 实测都是 II）；其他情况放弃合成，
        // 交给常规路径（大不了少几个曝光字段）
        return None;
    }
    let ifd0_off = u32::from_le_bytes([orig[4], orig[5], orig[6], orig[7]]) as usize;
    if ifd0_off + 2 > orig.len() {
        return None;
    }
    let count = u16::from_le_bytes([orig[ifd0_off], orig[ifd0_off + 1]]) as usize;
    let entries_end = ifd0_off + 2 + count * 12;
    if count == 0 || entries_end + 4 > orig.len() {
        return None;
    }

    // ── 规划新 IFD0：Make / Model（可选）+ ExifIFD 指针 ──
    let make_bytes = name.0.as_ref().map(|s| {
        let mut b = s.as_bytes().to_vec();
        b.push(0);
        b
    });
    let model_bytes = name.1.as_ref().map(|s| {
        let mut b = s.as_bytes().to_vec();
        b.push(0);
        b
    });
    let mut new_entries: Vec<[u8; 12]> = Vec::new();
    // 这些"长值"要放在新 IFD0 之后（新 IFD0 结束位置 = 8 + 2 + n*12 + 4）
    let n_entries = (make_bytes.is_some() as usize) + (model_bytes.is_some() as usize) + 1;
    let new_ifd_len = 2 + n_entries * 12 + 4;
    let mut tail: Vec<u8> = Vec::new(); // 长值区
    let tail_base = 8 + new_ifd_len; // 长值在新文件里的起始绝对偏移

    // Make (0x010F)
    if let Some(b) = &make_bytes {
        let off = tail_base + tail.len();
        let mut e = [0u8; 12];
        e[0..2].copy_from_slice(&0x010Fu16.to_le_bytes());
        e[2..4].copy_from_slice(&2u16.to_le_bytes()); // ASCII
        e[4..8].copy_from_slice(&(b.len() as u32).to_le_bytes());
        e[8..12].copy_from_slice(&(off as u32).to_le_bytes());
        new_entries.push(e);
        tail.extend_from_slice(b);
    }
    // Model (0x0110)
    if let Some(b) = &model_bytes {
        let off = tail_base + tail.len();
        let mut e = [0u8; 12];
        e[0..2].copy_from_slice(&0x0110u16.to_le_bytes());
        e[2..4].copy_from_slice(&2u16.to_le_bytes());
        e[4..8].copy_from_slice(&(b.len() as u32).to_le_bytes());
        e[8..12].copy_from_slice(&(off as u32).to_le_bytes());
        new_entries.push(e);
        tail.extend_from_slice(b);
    }
    // ExifIFD 指针：指向原 IFD0 搬过来之后的位置
    let shift = new_ifd_len + tail.len(); // 原数据整体后移量
    let orig_ifd0_new = ifd0_off + shift;
    let mut ptr = [0u8; 12];
    ptr[0..2].copy_from_slice(&EXIF_IFD_TAG.to_le_bytes());
    ptr[2..4].copy_from_slice(&4u16.to_le_bytes()); // LONG
    ptr[4..8].copy_from_slice(&1u32.to_le_bytes());
    ptr[8..12].copy_from_slice(&(orig_ifd0_new as u32).to_le_bytes());
    new_entries.push(ptr);

    // ── 组装 ──
    let mut out = Vec::with_capacity(orig.len() + shift);
    out.extend_from_slice(b"II");
    out.extend_from_slice(&42u16.to_le_bytes());
    out.extend_from_slice(&8u32.to_le_bytes());
    out.extend_from_slice(&(n_entries as u16).to_le_bytes());
    for e in &new_entries {
        out.extend_from_slice(e);
    }
    out.extend_from_slice(&0u32.to_le_bytes()); // next IFD = 无
    out.extend_from_slice(&tail); // 长值区

    // 原数据：头部之后到 IFD0 之前（若有）原样搬
    if ifd0_off > 8 {
        out.extend_from_slice(&orig[8..ifd0_off]);
    }
    // IFD0 条目（修正外部值偏移）
    let mut ifd = orig[ifd0_off..entries_end].to_vec();
    for i in 0..count {
        let e = 2 + i * 12;
        let typ = u16::from_le_bytes([ifd[e + 2], ifd[e + 3]]);
        let cnt = u32::from_le_bytes([ifd[e + 4], ifd[e + 5], ifd[e + 6], ifd[e + 7]]);
        if type_size(typ) * cnt as usize > 4 {
            let v = u32::from_le_bytes([ifd[e + 8], ifd[e + 9], ifd[e + 10], ifd[e + 11]]);
            let nv = v.checked_add(shift as u32)?;
            ifd[e + 8..e + 12].copy_from_slice(&nv.to_le_bytes());
        }
    }
    // next IFD 指针
    let next = u32::from_le_bytes([
        orig[entries_end],
        orig[entries_end + 1],
        orig[entries_end + 2],
        orig[entries_end + 3],
    ]);
    let new_next = if next == 0 {
        0
    } else {
        next.checked_add(shift as u32)?
    };
    out.extend_from_slice(&ifd);
    out.extend_from_slice(&new_next.to_le_bytes());
    // 值数据区（原样）
    out.extend_from_slice(&orig[entries_end + 4..]);

    parse_tiff_bytes(&out)
}

/// EXIF 的 `ExifIFD` 指针 tag（规范定义在 IFD0 中）。
const EXIF_IFD_TAG: u16 = 0x8769;

/// TIFF 类型码 → 单元素字节数（用于判断值是否存在条目外）。
fn type_size(typ: u16) -> usize {
    match typ {
        1 | 2 | 6 | 7 => 1, // BYTE / ASCII / SBYTE / UNDEFINED
        3 | 8 => 2,         // SHORT / SSHORT
        4 | 9 | 11 => 4,    // LONG / SLONG / FLOAT
        5 | 10 | 12 => 8,   // RATIONAL / SRATIONAL / DOUBLE
        _ => 0,
    }
}

/// 把 `src` 里的字段并入 `dst`（`dst` 已有的值不被覆盖）。
///
/// 顺序即优先级：先解析的 CMT1（基础）与 CMT2（曝光）字段更标准，
/// 后续 box 只在"前面没读到"时补位。
fn merge_exif(dst: &mut ExifData, src: &ExifData) {
    macro_rules! fill {
        ($($f:ident),* $(,)?) => {
            $( if dst.$f.is_none() { dst.$f = src.$f.clone(); } )*
        };
    }
    fill!(
        make,
        model,
        lens,
        date_time_original,
        exposure_time,
        f_number,
        iso,
        focal_length,
        focal_length_35mm,
        exposure_bias,
        exposure_program,
        metering_mode,
        flash,
        white_balance,
        orientation,
        software,
        gps,
    );
}

/// 从内嵌 JPEG 的 `APP1` 段读 EXIF。
///
/// 适用场景：RAW 文件头没有 TIFF 结构，EXIF 只存在于内嵌预览 JPEG 里
/// （实测 Fujifilm RAF 就是如此）。扫描逻辑与 `pv-decode` 的
/// `raw::jpeg_scan` 同源，但这里只需要「找到带 APP1 的那几段 JPEG」，
/// 不需要完整走到 EOI —— 省掉一半工作量。
///
/// 为什么不复用 `pv-decode::raw`：会引入循环依赖（pv-decode 依赖 pv-meta）。
/// 这里只做最小实现，且比那边更简单（不解析熵编码）。
fn read_exif_from_embedded_jpeg(path: &Path) -> Option<ExifData> {
    const SCAN: u64 = 8 * 1024 * 1024; // 预览总在文件前部

    let mut file = File::open(path).ok()?;
    let mut data = Vec::new();
    file.by_ref().take(SCAN).read_to_end(&mut data).ok()?;

    let mut from = 0usize;
    while let Some(rel) = find_bytes(&data[from..], &[0xFF, 0xD8, 0xFF]) {
        let start = from + rel;
        if let Some(app1) = find_app1_in_jpeg(&data, start) {
            // APP1 载荷以 `Exif\0\0` 开头，去掉这 6 字节后是裸 TIFF
            if let Some(exif) = parse_tiff_bytes(&app1) {
                return Some(extract::extract(&exif));
            }
        }
        from = start + 3;
        if from >= data.len() {
            break;
        }
    }
    None
}

/// 在一张 JPEG 里找 `APP1` 段，返回其载荷（**去掉控制头与 `Exif\0\0` 之后**）。
///
/// 只沿段链走到 SOS/DQT 之前就停 —— 拿到 APP1 就够了，无需解析熵编码数据。
fn find_app1_in_jpeg(data: &[u8], start: usize) -> Option<Vec<u8>> {
    // start 指向 SOI 的 FF，跳过 SOI 两个字节
    let mut i = start + 2;
    let n = data.len();

    while i + 3 < n {
        if data[i] != 0xFF {
            return None; // 段链断裂
        }
        let marker = data[i + 1];
        match marker {
            // 填充与无载荷标记
            0xFF | 0x01 | 0xD0..=0xD7 => {
                i += 2;
                continue;
            }
            // 到 SOS 就说明 APP1 已在前面处理过（或压根没有）
            0xDA | 0xD9 => return None,
            _ => {}
        }
        let len = u16::from_be_bytes([*data.get(i + 2)?, *data.get(i + 3)?]) as usize;
        if len < 2 || i + 2 + len > n {
            return None;
        }
        if marker == 0xE1 {
            let payload = &data[i + 4..i + 2 + len];
            // `Exif\0\0` 前缀
            if payload.starts_with(b"Exif\0\0") && payload.len() > 8 {
                return Some(payload[6..].to_vec());
            }
            return None; // APP1 里是 XMP 等其他内容
        }
        i += 2 + len;
    }
    None
}

/// 解析已切好的 TIFF 字节（复用主路径的容错策略）。
fn parse_tiff_bytes(tiff: &[u8]) -> Option<exif::Exif> {
    let mut reader = Reader::new();
    reader.continue_on_error(true);
    let result = match reader.read_raw(tiff.to_vec()) {
        Ok(exif) => Ok(exif),
        // 部分字段越界也会带回已解析的内容，尽量用
        Err(e) => e.distill_partial_result(|errors| {
            tracing::debug!(count = errors.len(), "TIFF 部分字段解析失败，已跳过");
        }),
    };
    result.ok()
}

/// 在 `data` 中查找最早的 `needle`。
fn find_bytes(data: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || data.len() < needle.len() {
        return None;
    }
    data.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实图库路径（缺失时测试自动跳过，便于在别的机器上跑）
    fn sample(name: &str) -> Option<std::path::PathBuf> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-image")
            .join(name);
        if p.is_file() && std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0) > 4096 {
            Some(p)
        } else {
            eprintln!("跳过：样例文件不可用 {}", p.display());
            None
        }
    }

    #[test]
    fn 读取JPEG的拍摄参数() {
        let Some(p) = sample("DSC9515.JPG") else { return };
        let e = read_exif(&p).expect("真实相机 JPEG 应当能读到 EXIF");
        assert!(e.make.is_some(), "应有相机厂商");
        assert!(e.model.is_some(), "应有相机型号");
        assert!(e.date_time_original.is_some(), "应有拍摄时间");
        assert!(e.exposure_time.is_some(), "应有快门速度");
        assert!(e.f_number.is_some(), "应有光圈");
        assert!(e.iso.is_some(), "应有 ISO");
        assert!(e.focal_length.is_some(), "应有焦距");
        eprintln!("JPEG EXIF = {e:#?}");
    }

    #[test]
    fn 读取RAW也能拿到拍摄参数() {
        // ORF / RW2 属于「非标准魔数的 TIFF」，是 normalize_tiff_magic 的重点覆盖对象
        for name in [
            "olympus-orf-sample.ORF",
            "panasonic-rw2-sample.RW2",
            "sony-sr2-sample.SR2",
            "nikon-nrw-sample.NRW",
        ] {
            let Some(p) = sample(name) else { continue };
            match read_exif(&p) {
                Some(e) => {
                    assert!(!e.is_empty());
                    eprintln!("{name} → make={:?} model={:?}", e.make, e.model);
                }
                None => eprintln!("{name} 未读到 EXIF（可接受，但值得关注）"),
            }
        }
    }

    #[test]
    fn 读取耗时不影响看图体验() {
        let Some(p) = sample("DSC9515.JPG") else { return };
        let t = std::time::Instant::now();
        let _ = read_exif(&p);
        let jpeg_ms = t.elapsed().as_millis();

        let Some(raw) = sample("olympus-orf-sample.ORF") else { return };
        let t = std::time::Instant::now();
        let _ = read_exif(&raw);
        let raw_ms = t.elapsed().as_millis();

        // 不设阈值断言（CI 机器差异大），只打印出来便于判断
        eprintln!("EXIF 读取耗时：JPEG {jpeg_ms} ms / ORF {raw_ms} ms");
    }

    /// GPS：只有开定位的手机/带 GPS 模块的相机才写，且**必须**连方位一起读到。
    ///
    /// 这条测试钉住两件容易被写错的事：
    /// 1. **度分秒 → 十进制度**的换算（写成"度.分秒"拼接会差出一个数量级）；
    /// 2. **南/西半球取负** —— 少了 ref 或符号搞反，坐标会指到地球另一侧，
    ///    而界面上看起来"有地点"完全正常，是最难发现的一类错误。
    #[test]
    fn 读取带GPS的照片得到正确坐标() {
        let Some(p) = sample("IMG_7507.DNG") else { return };
        let e = read_exif(&p).expect("该 DNG 应能读到 EXIF");
        let gps = e.gps.as_ref().expect("该样本带 GPS");

        // 实测基准（Python 独立解析同一文件核对过）：31°12'17.42"N 121°26'26.26"E
        let lat: f64 = gps.latitude.parse().unwrap();
        let lon: f64 = gps.longitude.parse().unwrap();
        assert!((lat - 31.204839).abs() < 1e-5, "纬度 {lat}");
        assert!((lon - 121.440628).abs() < 1e-5, "经度 {lon}");
        assert_eq!(gps.altitude.as_deref(), Some("9.3 m"), "海拔 124672/13399");
        // 展示文本是度分秒（2026-10-11 用户选定），十进制另有字段给地图链接
        assert_eq!(gps.display, "31°12'17.4\"N, 121°26'26.3\"E");
        eprintln!("GPS = {gps:#?}");
    }

    /// 没有 GPS 的照片必须返回 `None`（前端据此整行隐藏）。
    #[test]
    fn 无GPS的照片不显示地点() {
        let Some(p) = sample("DSC9515.JPG") else { return };
        let e = read_exif(&p).expect("该 JPEG 应能读到 EXIF");
        assert!(e.gps.is_none(), "不应凭空编造地点：{:?}", e.gps);
    }

    #[test]
    fn 空文件与垃圾数据不panic() {
        let dir = std::env::temp_dir().join("pv-meta-test");
        std::fs::create_dir_all(&dir).unwrap();

        let empty = dir.join("empty.bin");
        std::fs::write(&empty, b"").unwrap();
        assert_eq!(read_exif(&empty), None);

        let garbage = dir.join("garbage.bin");
        std::fs::write(&garbage, vec![0xABu8; 512]).unwrap();
        assert_eq!(read_exif(&garbage), None);

        // 伪造一个「像 TIFF 但被截断」的文件，确认走 continue_on_error 不炸
        let broken = dir.join("broken.tif");
        std::fs::write(&broken, b"II*\0\x08\0\0\0\x01\0").unwrap();
        let _ = read_exif(&broken);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 魔数修正只动第3和第4字节() {
        let mut orf = b"IIRO\x08\0\0\0".to_vec();
        normalize_tiff_magic(&mut orf);
        assert_eq!(&orf[0..2], b"II");
        assert_eq!(&orf[2..4], b"*\0");
        assert_eq!(&orf[4..8], b"\x08\0\0\0", "IFD 偏移不能被破坏");

        let mut big = b"MMOR\0\x08\0\0".to_vec();
        normalize_tiff_magic(&mut big);
        assert_eq!(&big[2..4], b"\0*");
    }

    #[test]
    fn 用不到EXIF的格式返回None() {
        // 纯文本文件既不是容器也不是 TIFF
        assert!(read_exif(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml").as_path()).is_none());
    }

    /// CR3 走的是「CMT1–CMT4 box + 合成 TIFF」这条专有路径。
    ///
    /// 这条测试的价值在于：CR3 的容器结构一旦变化（或合成逻辑写错），
    /// 表现是「相机型号有、曝光参数全空」这种半残状态 ——
    /// 只看「读到了 EXIF」是发现不了的，所以必须显式断言曝光字段。
    #[test]
    fn CR3走CMT合成路径且曝光参数完整() {
        let Some(p) = sample("canon-cr3-sample.CR3") else { return };
        let e = read_exif(&p).expect("CR3 应当能读到 EXIF");

        assert_eq!(e.make.as_deref(), Some("Canon"), "厂商");
        assert_eq!(e.model.as_deref(), Some("Canon EOS M50"), "型号");
        // 这三个在 CMT2 里，必须经合成路径才拿得到
        assert!(e.exposure_time.is_some(), "CR3 应有快门速度（CMT2 合成失败会丢）");
        assert!(e.f_number.is_some(), "CR3 应有光圈（CMT2 合成失败会丢）");
        assert!(e.iso.is_some(), "CR3 应有 ISO（CMT2 合成失败会丢）");
        assert!(e.focal_length.is_some(), "CR3 应有焦距");
        assert!(e.lens.is_some(), "CR3 应有镜头");
        eprintln!("CR3 EXIF = {e:#?}");
    }

    /// 合成 TIFF 必须保持内部偏移自洽（偏移后移量算错会读到垃圾）。
    #[test]
    fn 合成后的TIFF偏移自洽() {
        let Some(p) = sample("canon-cr3-sample.CR3") else { return };
        let data = std::fs::read(&p).unwrap();
        let at = find_bytes(&data, b"CMT2").expect("应有 CMT2");
        let len = u32::from_be_bytes([data[at - 4], data[at - 3], data[at - 2], data[at - 1]]) as usize;
        let mut tiff = data[at + 4..at + 4 + len - 8].to_vec();
        normalize_tiff_magic(&mut tiff);

        let exif = synthesize_exposure_tiff(&tiff, &(Some("Canon".into()), Some("Test".into())))
            .expect("合成应成功");

        // 合成后 ExifIFD 的字段必须真的取得到值（取不到说明偏移算错）
        use exif::{In, Tag, Value};
        let t = exif
            .get_field(Tag::ExposureTime, In::PRIMARY)
            .expect("合成后应能取到 ExposureTime");
        let n = exif
            .get_field(Tag::FNumber, In::PRIMARY)
            .expect("合成后应能取到 FNumber");
        // 偏移算错的话这两个值会是完全不着边际的数（读到别的字段的数据）
        let (tn, td) = match &t.value {
            Value::Rational(v) => (v[0].num, v[0].denom),
            other => panic!("ExposureTime 应是 RATIONAL，实际 {other:?}"),
        };
        assert!(tn > 0 && td > 0 && tn < td, "快门 {tn}/{td} 应小于 1 秒");
        let (fn_, fd) = match &n.value {
            Value::Rational(v) => (v[0].num, v[0].denom),
            other => panic!("FNumber 应是 RATIONAL，实际 {other:?}"),
        };
        // 样本实拍 f/8
        let f = fn_ as f64 / fd as f64;
        assert!((1.0..=64.0).contains(&f), "光圈 {f} 应在合理范围");
    }

    /// RAF 的 EXIF 只存在于内嵌 JPEG 的 APP1 里（文件头是 `FUJIFILM` 私有结构）。
    #[test]
    fn RAF从内嵌JPEG的APP1读到EXIF() {
        let Some(p) = sample("fuji-raf-sample.RAF") else { return };
        let e = read_exif(&p).expect("RAF 应能经 APP1 兜底读到 EXIF");
        assert!(e.make.is_some(), "应读到厂商");
        assert!(e.exposure_time.is_some(), "应读到快门");
        eprintln!("RAF EXIF = {e:#?}");
    }

    /// APP1 解析必须能识别段边界，不能把非 APP1 段当成 APP1。
    #[test]
    fn APP1段解析只认Exif前缀() {
        // 造一张只有 APP0(JFIF) + SOS 的 JPEG，不应误报 APP1
        let mut jpeg = vec![0xFF, 0xD8];
        // APP0, len=16
        jpeg.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x10]);
        jpeg.extend_from_slice(b"JFIF\0");
        jpeg.extend_from_slice(&[0x01, 0x02, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00]);
        // SOS
        jpeg.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00]);
        assert!(find_app1_in_jpeg(&jpeg, 0).is_none(), "无 APP1 应返回 None");

        // 再插入一个 XMP 型 APP1（以 http://ns.adobe.com 开头），也不应被当作 EXIF
        let mut with_xmp = vec![0xFF, 0xD8];
        let xmp = b"http://ns.adobe.com/xap/1.0/\0<x/>";
        let seg_len = (xmp.len() + 2) as u16;
        with_xmp.extend_from_slice(&[0xFF, 0xE1]);
        with_xmp.extend_from_slice(&seg_len.to_be_bytes());
        with_xmp.extend_from_slice(xmp);
        assert!(
            find_app1_in_jpeg(&with_xmp, 0).is_none(),
            "XMP 型 APP1 不是 EXIF，应返回 None"
        );
    }
}
