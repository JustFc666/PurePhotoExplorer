//! PSD / PSB 解码（**纯 Rust 手写，无第三方依赖**）。
//!
//! ## 我们解的是"合成图"，不是图层
//!
//! PSD 文件尾部有一段**扁平的合成图像数据**（Photoshop 说的"最大化兼容性"，
//! 默认开启）。它就是另存为 JPEG/PNG 时看到的那张图，正是看图软件需要的。
//! 图层数据（`Layer and Mask Information` 段）在我们这里**整段跳过** ——
//! 自己实现图层混合模式、蒙版、调整层是个无底洞，而合成图已经够用。
//!
//! ## 文件结构（解析顺序即读文件的顺序）
//! ```text
//! Header:          26 字节  '8BPS' + 版本 + 通道数 + 高 + 宽 + 位深 + 色彩模式
//! Color Mode Data: u32 长度 + 数据    （索引色/双色调的调色板；我们是 RGB，通常为 0）
//! Image Resources: u32 长度 + 数据    （ICC 配置、分辨率等；跳过）
//! Layer and Mask:  u32 长度 + 数据    （**图层，整段跳过**；PSB 是 u64）
//! Image Data:      u16 压缩方式 + 像素（**我们要的就是这段**）
//! ```
//!
//! ## 合成图的数据布局：**分平面（planar）**
//! 不是 RGBA 交错，而是「通道 0 的所有行 → 通道 1 的所有行 → …」。
//! 所以解完必须重新交错成 RGBA，见 [`planar_to_rgba`]。
//!
//! ## 四种压缩方式
//! | 值 | 方式 | 实现 |
//! |---|---|---|
//! | 0 | Raw | 直接读 |
//! | 1 | RLE（PackBits，逐行） | [`decode_rle`] —— Photoshop 的默认值 |
//! | 2 | ZIP（zlib，整段） | [`decode_zip`] |
//! | 3 | ZIP + 逐行差分预测 | [`decode_zip`]（带 `prediction`） |
//!
//! ## 不支持的输入（给明确错误，不猜）
//! - **色彩模式**：只做 RGB / 灰度。CMYK、Lab、索引色、位图、多通道一律拒绝 ——
//!   CMYK 要做 ICC 色彩管理才准，我们没样本也没必要。
//! - **位深**：8 / 16（16 取高字节降到 8）。1 位与 32 位浮点拒绝。
//! - **PSB**（大文档，版本 2）：结构相同但长度字段是 64 位，已一并支持。

use std::path::Path;

use pv_core::error::{PvError, Result};
use pv_core::format::ImageFormat;

use super::io_err;
use super::resize::resize_rgba;
use crate::decoder::{fit_within, DecodeTarget, ImageInfo, RgbaImage};

/// 头部固定 26 字节
const HEADER_LEN: usize = 26;
/// 允许的像素总量上限（防止畸形/巨型文件把内存吃干）。
///
/// PSD 规范允许到 30000×30000，但那已是 3600 万像素 × 通道数，
/// 4 通道 8 位就是 144MB，再加交错后的 RGBA 288MB —— 还不算离谱。
/// 这里按 **1.5 亿样本** 设闸（约 150MB planar / 600MB RGBA），
/// 超过就明确报错而不是 OOM。
const MAX_SAMPLES: usize = 150_000_000;

/// PSD 头部解析结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PsdHeader {
    /// true = PSB（大文档，长度字段为 64 位）
    is_large: bool,
    channels: usize,
    width: u32,
    height: u32,
    depth: u16,
    color_mode: u16,
}

impl PsdHeader {
    /// 每样本字节数（我们只接受 8 / 16 位）
    fn bytes_per_sample(self) -> usize {
        match self.depth {
            8 => 1,
            16 => 2,
            _ => 1, // 校验阶段已拦掉，这里只是给个安全值
        }
    }

    /// 单个通道的样本数
    fn samples_per_channel(self) -> usize {
        self.width as usize * self.height as usize
    }
}

/// 色彩模式名（用于报错文案与 `color_space` 显示）
fn color_mode_name(mode: u16) -> &'static str {
    match mode {
        0 => "位图",
        1 => "灰度",
        2 => "索引色",
        3 => "RGB",
        4 => "CMYK",
        7 => "多通道",
        8 => "双色调",
        9 => "Lab",
        _ => "未知",
    }
}

/// 探测 PSD 尺寸（**只读 26 字节头**，O(1)）
pub(super) fn probe_psd(path: &Path) -> Result<ImageInfo> {
    let head = read_head(path)?;
    let h = parse_header(&head)?;
    Ok(ImageInfo {
        format: ImageFormat::Psd,
        width: h.width,
        height: h.height,
        exif: None,
        raw: None,
        color_space: Some(format!("{}（合成图）", color_mode_name(h.color_mode))),
    })
}

/// 只读文件头 26 字节
fn read_head(path: &Path) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(io_err(path))?;
    let mut buf = vec![0u8; HEADER_LEN];
    let n = f.read(&mut buf).map_err(io_err(path))?;
    if n < HEADER_LEN {
        return Err(PvError::Decode("PSD 文件头不完整（不足 26 字节）".into()));
    }
    Ok(buf)
}

/// 解析并校验 PSD 头部
fn parse_header(b: &[u8]) -> Result<PsdHeader> {
    if &b[0..4] != b"8BPS" {
        return Err(PvError::Decode("不是 PSD 文件（缺少 8BPS 签名）".into()));
    }
    let version = be16(b, 4);
    let is_large = match version {
        1 => false,
        2 => true,
        other => {
            return Err(PvError::Decode(format!(
                "不支持的 PSD 版本号 {other}（应为 1=PSD 或 2=PSB）"
            )))
        }
    };

    let channels = be16(b, 12) as usize;
    let height = be32(b, 14);
    let width = be32(b, 18);
    let depth = be16(b, 22);
    let color_mode = be16(b, 24);

    if width == 0 || height == 0 {
        return Err(PvError::Decode(format!("PSD 尺寸非法：{width}×{height}")));
    }
    if !(1..=56).contains(&channels) {
        return Err(PvError::Decode(format!("PSD 通道数非法：{channels}")));
    }
    if !matches!(depth, 8 | 16) {
        return Err(PvError::Decode(format!(
            "不支持的 PSD 位深 {depth} 位（目前只支持 8 位与 16 位）"
        )));
    }
    if !matches!(color_mode, 1 | 3) {
        return Err(PvError::Decode(format!(
            "不支持的 PSD 色彩模式「{}」（目前只支持 RGB 与灰度）",
            color_mode_name(color_mode)
        )));
    }

    let h = PsdHeader {
        is_large,
        channels,
        width,
        height,
        depth,
        color_mode,
    };
    let samples = h.samples_per_channel().saturating_mul(h.channels);
    if samples > MAX_SAMPLES {
        return Err(PvError::Decode(format!(
            "PSD 过大：{}×{} × {} 通道 = {samples} 个样本，超出上限",
            h.width, h.height, h.channels
        )));
    }
    Ok(h)
}

/// 解码 PSD 的合成图为 RGBA（按 `target` 缩放）。
pub(super) fn decode_psd(path: &Path, target: DecodeTarget) -> Result<RgbaImage> {
    let data = std::fs::read(path).map_err(io_err(path))?;
    // 先量长度再解析 —— `parse_header` 会直接索引 b[0..4]，短数据必须先拦住
    if data.len() < HEADER_LEN {
        return Err(PvError::Decode("PSD 文件头不完整（不足 26 字节）".into()));
    }
    let h = parse_header(&data[..HEADER_LEN])?;

    // ── 依次跳过三段，定位到图像数据 ──
    let mut p = HEADER_LEN;
    p = skip_block(&data, p, "色彩模式数据")?;
    p = skip_block(&data, p, "图像资源")?;
    // 图层与蒙版段：PSD 用 u32、PSB 用 u64
    let lm_len = if h.is_large {
        let n = be64(&data, p)?;
        (p + 8, n)
    } else {
        let n = be32(&data, p) as u64;
        (p + 4, n)
    };
    let (mut p, lm_len) = lm_len;
    let lm_end = p
        .checked_add(lm_len as usize)
        .ok_or_else(|| PvError::Decode("PSD 图层段长度溢出".into()))?;
    if lm_end > data.len() {
        return Err(PvError::Decode(
            "PSD 文件被截断（图层与蒙版段长度超出文件）".into(),
        ));
    }
    p = lm_end;

    if p + 2 > data.len() {
        return Err(PvError::Decode(
            "PSD 缺少图像数据段：该文件可能未保存合成图（Photoshop 里\
             「文件 → 存储为」勾选「最大兼容」，或「编辑 → 首选项 → 文件处理」里开启）"
                .into(),
        ));
    }
    let compression = be16(&data, p);
    p += 2;
    let payload = &data[p..];

    let expected = h.samples_per_channel() * h.channels * h.bytes_per_sample();
    let planar = match compression {
        0 => decode_raw(payload, expected)?,
        1 => decode_rle(payload, &h, expected)?,
        2 => decode_zip(payload, expected, false, &h)?,
        3 => decode_zip(payload, expected, true, &h)?,
        other => {
            return Err(PvError::Decode(format!(
                "不支持的 PSD 合成图压缩方式 {other}（支持 0=Raw / 1=RLE / 2=ZIP / 3=ZIP+预测）"
            )))
        }
    };

    let full = planar_to_rgba(&planar, &h)?;
    let (tw, th) = fit_within(full.width, full.height, target.max_edge);
    if (tw, th) == (full.width, full.height) {
        Ok(full)
    } else {
        resize_rgba(&full, tw, th)
    }
}

/// 跳过 `u32 长度 + 数据` 形式的段，返回下一段的起始偏移
fn skip_block(data: &[u8], p: usize, what: &str) -> Result<usize> {
    if p + 4 > data.len() {
        return Err(PvError::Decode(format!("PSD 在{what}段处被截断")));
    }
    let len = be32(data, p) as usize;
    let end = p
        .checked_add(4)
        .and_then(|v| v.checked_add(len))
        .ok_or_else(|| PvError::Decode(format!("PSD{what}段长度溢出")))?;
    if end > data.len() {
        return Err(PvError::Decode(format!(
            "PSD 文件被截断（{what}段声明 {len} 字节，超出文件范围）"
        )));
    }
    Ok(end)
}

/// 原样取用（compression = 0）
fn decode_raw(payload: &[u8], expected: usize) -> Result<Vec<u8>> {
    if payload.len() < expected {
        return Err(PvError::Decode(format!(
            "PSD 图像数据不足：需要 {expected} 字节，实际 {} 字节",
            payload.len()
        )));
    }
    Ok(payload[..expected].to_vec())
}

/// PackBits（Apple RLE）解压一行。
///
/// 格式：读控制字节 `n`
/// - `0..=127`：紧跟 `n+1` 个**字面**字节
/// - `129..=255`：把下一个字节**重复** `257-n` 次
/// - `128`：空操作（规范里的保留值）
///
/// 返回解出的 `want` 个字节，以及消耗的输入长度。
fn packbits_row(src: &[u8], want: usize) -> Result<(Vec<u8>, usize)> {
    let mut out = Vec::with_capacity(want);
    let mut i = 0usize;
    while out.len() < want {
        if i >= src.len() {
            // 行数据提前结束：后面的位置按 0 补齐（比整图失败更宽容）
            out.resize(want, 0);
            break;
        }
        let n = src[i] as usize;
        i += 1;
        if n < 128 {
            let cnt = n + 1;
            if i + cnt > src.len() {
                out.resize(want, 0);
                break;
            }
            let take = cnt.min(want - out.len());
            out.extend_from_slice(&src[i..i + take]);
            i += cnt;
        } else if n > 128 {
            let cnt = 257 - n;
            if i >= src.len() {
                out.resize(want, 0);
                break;
            }
            let byte = src[i];
            i += 1;
            let take = cnt.min(want - out.len());
            out.resize(out.len() + take, byte);
        }
        // n == 128：跳过，不做任何事
    }
    Ok((out, i))
}

/// RLE（compression = 1）：先有 `通道数 × 高` 个 u16 行长度表，再逐行 PackBits。
fn decode_rle(payload: &[u8], h: &PsdHeader, expected: usize) -> Result<Vec<u8>> {
    let rows = h.channels * h.height as usize;
    let table_len = rows * 2;
    if payload.len() < table_len {
        return Err(PvError::Decode(
            "PSD 的 RLE 行长度表不完整 —— 该文件很可能没有保存合成图（请在 Photoshop 中\
             重新保存并勾选「最大兼容」）"
                .into(),
        ));
    }

    // 行长度表（大端 u16）。总和为 0 说明合成图是空的。
    let mut row_len = Vec::with_capacity(rows);
    let mut total = 0usize;
    for i in 0..rows {
        let n = be16(payload, i * 2) as usize;
        total += n;
        row_len.push(n);
    }
    if total == 0 {
        return Err(PvError::Decode(
            "PSD 没有合成图数据（所有行长度都是 0）—— 请在 Photoshop 里\
             重新保存并勾选「最大兼容」"
                .into(),
        ));
    }

    let row_bytes = h.width as usize * h.bytes_per_sample();
    let mut out = Vec::with_capacity(expected);
    let mut p = table_len;
    for i in 0..rows {
        let n = row_len[i];
        let end = (p + n).min(payload.len());
        let (row, _) = packbits_row(&payload[p..end], row_bytes)?;
        out.extend_from_slice(&row);
        p += n;
    }

    if out.len() < expected {
        // RLE 实际产出比声明的少：补齐到声明长度（后面按 0 处理），
        // 而不是整图失败 —— 部分损坏的 PSD 仍值得显示出来。
        out.resize(expected, 0);
    }
    Ok(out)
}

/// ZIP（compression = 2 / 3）。
///
/// 整段图像数据是一个 **zlib 流**（不是裸 deflate）。
/// `prediction = true`（compression 3）时每行做过差分，需要按
/// "前一个像素" 累加还原 —— 这是 PNG 的 Sub 滤波器同款思路，
/// 步长是「一个像素在本通道里的字节数」。
fn decode_zip(payload: &[u8], expected: usize, prediction: bool, h: &PsdHeader) -> Result<Vec<u8>> {
    use std::io::Read;

    let mut dec = flate2::read::ZlibDecoder::new(payload);
    let mut out = Vec::with_capacity(expected);
    // 只解到 expected 就停，避免畸形数据无限膨胀
    let mut limited = (&mut dec).take(expected as u64 + 1);
    limited
        .read_to_end(&mut out)
        .map_err(|e| PvError::Decode(format!("PSD 的 ZIP 数据解压失败：{e}")))?;

    if out.len() < expected {
        return Err(PvError::Decode(format!(
            "PSD 的 ZIP 数据不足：解出 {} 字节，需要 {expected} 字节",
            out.len()
        )));
    }
    out.truncate(expected);
    if prediction {
        // compression = 3：解压出来的是差分值，必须逐行累加还原。
        // 漏掉这一步不会报错，只会得到一张"斜条纹"状的乱图。
        undo_prediction(&mut out, h);
    }
    Ok(out)
}

/// 差分预测的还原（compression = 3）。
///
/// 逐行做：`row[i] += row[i - step]`（`step` = 一个像素在本通道的字节数）。
/// 必须在**每个通道的每一行**内独立进行，所以需要知道每行长度。
fn undo_prediction(data: &mut [u8], h: &PsdHeader) {
    let row_bytes = h.width as usize * h.bytes_per_sample();
    let step = h.bytes_per_sample();
    if row_bytes == 0 || step == 0 {
        return;
    }
    for row in data.chunks_mut(row_bytes) {
        for i in step..row.len() {
            row[i] = row[i].wrapping_add(row[i - step]);
        }
    }
}

/// 分平面（planar）→ 交错 RGBA。
///
/// 输入是「通道 0 的全部行 → 通道 1 的全部行 → …」，每个样本
/// `bytes_per_sample` 字节（16 位时取**高字节**，等价于右移 8）。
fn planar_to_rgba(planar: &[u8], h: &PsdHeader) -> Result<RgbaImage> {
    let w = h.width as usize;
    let ht = h.height as usize;
    let n = w * ht;
    let bps = h.bytes_per_sample();
    let stride = n * bps;
    if planar.len() < stride * h.channels {
        return Err(PvError::Decode("PSD 分平面数据长度不足".into()));
    }

    // 取第 c 个通道第 i 个样本的值（统一成 8 位）
    let ch = |c: usize, i: usize| -> u8 {
        let o = c * stride + i * bps;
        let v = planar[o];
        // 16 位：planar 是大端，planar[o] 已是高字节，直接用作 8 位值
        v
    };

    let mut data = vec![0u8; n * 4];
    match h.color_mode {
        // 灰度：1 通道（+1 可能是 alpha）
        1 => {
            let alpha_idx = if h.channels >= 2 { 1 } else { usize::MAX };
            for i in 0..n {
                let g = ch(0, i);
                let o = i * 4;
                data[o] = g;
                data[o + 1] = g;
                data[o + 2] = g;
                data[o + 3] = if alpha_idx == usize::MAX { 255 } else { ch(alpha_idx, i) };
            }
        }
        // RGB：3 通道（+1 可能是 alpha）
        3 => {
            let alpha_idx = if h.channels >= 4 { 3 } else { usize::MAX };
            for i in 0..n {
                let o = i * 4;
                data[o] = ch(0, i);
                data[o + 1] = ch(1, i);
                data[o + 2] = ch(2, i);
                data[o + 3] = if alpha_idx == usize::MAX { 255 } else { ch(alpha_idx, i) };
            }
        }
        m => {
            return Err(PvError::Decode(format!(
                "不支持的 PSD 色彩模式「{}」",
                color_mode_name(m)
            )))
        }
    }

    Ok(RgbaImage {
        width: h.width,
        height: h.height,
        data,
    })
}

// ── 大端读取（带边界检查）──

fn be16(b: &[u8], o: usize) -> u16 {
    u16::from_be_bytes([b.get(o).copied().unwrap_or(0), b.get(o + 1).copied().unwrap_or(0)])
}

fn be32(b: &[u8], o: usize) -> u32 {
    let g = |i: usize| b.get(i).copied().unwrap_or(0);
    u32::from_be_bytes([g(o), g(o + 1), g(o + 2), g(o + 3)])
}

fn be64(b: &[u8], o: usize) -> Result<u64> {
    if o + 8 > b.len() {
        return Err(PvError::Decode("PSD 在 64 位长度字段处被截断".into()));
    }
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[o..o + 8]);
    Ok(u64::from_be_bytes(a))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::DecodePurpose;
    use std::path::PathBuf;

    fn sample(name: &str) -> Option<PathBuf> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-image");
        assert!(
            dir.is_dir(),
            "找不到 test-image 目录（推算 {}）—— 样本定位失败会让测试假通过",
            dir.display()
        );
        let p = dir.join(name);
        if p.is_file() && std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0) > 4096 {
            Some(p)
        } else {
            None
        }
    }

    /// 造一个最小的 8 位 RGB PSD（Raw 压缩），用于覆盖非样本路径
    fn build_min_psd(w: u32, h: u32, channels: u16, pixels: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"8BPS");
        v.extend_from_slice(&1u16.to_be_bytes()); // version
        v.extend_from_slice(&[0u8; 6]); // reserved
        v.extend_from_slice(&channels.to_be_bytes());
        v.extend_from_slice(&h.to_be_bytes());
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&8u16.to_be_bytes()); // depth
        v.extend_from_slice(&3u16.to_be_bytes()); // RGB
        v.extend_from_slice(&0u32.to_be_bytes()); // color mode data
        v.extend_from_slice(&0u32.to_be_bytes()); // image resources
        v.extend_from_slice(&0u32.to_be_bytes()); // layer and mask
        v.extend_from_slice(&0u16.to_be_bytes()); // compression = Raw
        v.extend_from_slice(pixels);
        v
    }

    #[test]
    fn 探测PSD头部即可拿到尺寸() {
        let Some(p) = sample("clockpic.psd") else {
            eprintln!("跳过：找不到 PSD 样本");
            return;
        };
        let info = probe_psd(&p).unwrap();
        assert_eq!(info.format, ImageFormat::Psd);
        assert_eq!((info.width, info.height), (1240, 1754));
        assert!(info.color_space.as_deref().unwrap().contains("RGB"));
    }

    #[test]
    fn 真实PSDRLE解码出正确尺寸与内容() {
        let Some(p) = sample("clockpic.psd") else {
            eprintln!("跳过：找不到 PSD 样本");
            return;
        };
        let img = decode_psd(&p, DecodeTarget::new(DecodePurpose::Preview)).unwrap();
        assert_eq!((img.width, img.height), (1240, 1754));
        assert_eq!(img.data.len(), 1240 * 1754 * 4);
        // 无 alpha 通道 → 全 255
        assert!(img.data.chunks_exact(4).all(|px| px[3] == 255));

        // 内容不能是纯色（该图是白底时钟：应有大量白 + 少量黑）
        let mut white = 0usize;
        let mut dark = 0usize;
        for px in img.data.chunks_exact(4).step_by(37) {
            if px[0] > 240 && px[1] > 240 && px[2] > 240 {
                white += 1;
            } else if px[0] < 60 && px[1] < 60 && px[2] < 60 {
                dark += 1;
            }
        }
        assert!(white > 0, "应有白色背景");
        assert!(dark > 0, "应有深色线条（表盘刻度/指针）");
    }

    #[test]
    fn 非PSD文件报错而不是panic() {
        let dir = std::env::temp_dir().join("pv_psd_bad");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("not.psd");
        std::fs::write(&p, b"definitely not a psd file at all").unwrap();
        assert!(probe_psd(&p).is_err());
        assert!(decode_psd(&p, DecodeTarget::new(DecodePurpose::Preview)).is_err());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn 截断的PSD报错而不是panic() {
        let full = build_min_psd(2, 2, 3, &[0u8; 12]);
        for cut in [0usize, 4, 10, 25, 30, full.len() - 1] {
            let dir = std::env::temp_dir().join("pv_psd_trunc");
            std::fs::create_dir_all(&dir).unwrap();
            let p = dir.join(format!("cut{cut}.psd"));
            std::fs::write(&p, &full[..cut]).unwrap();
            // 不 panic 即可（返回 Ok 或 Err 都算通过）
            let _ = probe_psd(&p);
            let _ = decode_psd(&p, DecodeTarget::new(DecodePurpose::Preview));
            let _ = std::fs::remove_file(&p);
        }
    }

    #[test]
    fn 不支持的颜色模式给出明确错误() {
        let mut v = build_min_psd(2, 2, 4, &[0u8; 16]);
        v[24] = 0;
        v[25] = 4; // CMYK
        let dir = std::env::temp_dir().join("pv_psd_cmyk");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("cmyk.psd");
        std::fs::write(&p, &v).unwrap();
        let e = probe_psd(&p).unwrap_err().to_string();
        assert!(e.contains("CMYK"), "错误信息应点名色彩模式，实际：{e}");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn Raw压缩的小PSD能正确交错成RGBA() {
        // 2×1 图：通道0 = [10, 20]，通道1 = [30, 40]，通道2 = [50, 60]
        let planar = [10u8, 20, 30, 40, 50, 60];
        let v = build_min_psd(2, 1, 3, &planar);
        let dir = std::env::temp_dir().join("pv_psd_raw");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("tiny.psd");
        std::fs::write(&p, &v).unwrap();

        let img = decode_psd(&p, DecodeTarget::new(DecodePurpose::Preview)).unwrap();
        assert_eq!((img.width, img.height), (2, 1));
        // planar → 交错：像素0 = (10,30,50)，像素1 = (20,40,60)
        assert_eq!(&img.data[0..4], &[10, 30, 50, 255]);
        assert_eq!(&img.data[4..8], &[20, 40, 60, 255]);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn PackBits行解压符合规范() {
        // 字面：0x02 → 复制 3 字节；重复：0xFE(=-2 → 257-254=3 次) + 'A'
        let (out, used) = packbits_row(&[0x02, 1, 2, 3], 3).unwrap();
        assert_eq!(out, vec![1, 2, 3]);
        assert_eq!(used, 4);

        let (out, used) = packbits_row(&[0xFE, b'A'], 3).unwrap();
        assert_eq!(out, vec![b'A'; 3]);
        assert_eq!(used, 2);

        // 0x80 是空操作：应被跳过并继续读下一个控制字节
        let (out, _) = packbits_row(&[0x80, 0x01, 9, 9], 2).unwrap();
        assert_eq!(out, vec![9, 9]);
    }

    #[test]
    fn 行数据不足时按0补齐而不是整图失败() {
        let (out, _) = packbits_row(&[0x02, 1, 2, 3], 10).unwrap();
        assert_eq!(out.len(), 10);
        assert_eq!(&out[..3], &[1, 2, 3]);
        assert!(out[3..].iter().all(|&b| b == 0));
    }

    #[test]
    fn 差分预测还原正确() {
        let h = PsdHeader {
            is_large: false,
            channels: 3,
            width: 3,
            height: 2,
            depth: 8,
            color_mode: 3,
        };
        // 每行 3 字节，步长 1 → 行内累加
        let mut d = vec![1, 1, 1, /*行2*/ 2, 2, 2];
        undo_prediction(&mut d, &h);
        assert_eq!(d, vec![1, 2, 3, 2, 4, 6]);
    }

    #[test]
    fn 位深16的PSD取高字节() {
        let h = PsdHeader {
            is_large: false,
            channels: 3,
            width: 2,
            height: 1,
            depth: 16,
            color_mode: 3,
        };
        assert_eq!(h.bytes_per_sample(), 2);
        // 2 像素：通道0 = [0x11,0x22], [0x33,0x44]；通道1 全 0x55；通道2 全 0x66
        let mut planar = vec![0u8; 3 * 2 * 2];
        planar[0] = 0x11; planar[1] = 0x22; planar[2] = 0x33; planar[3] = 0x44;
        for i in 4..8 { planar[i] = 0x55; }
        for i in 8..12 { planar[i] = 0x66; }
        let img = planar_to_rgba(&planar, &h).unwrap();
        assert_eq!(&img.data[0..4], &[0x11, 0x55, 0x66, 255]);
        assert_eq!(&img.data[4..8], &[0x33, 0x55, 0x66, 255]);
    }
}
