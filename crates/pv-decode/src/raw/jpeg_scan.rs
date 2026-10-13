//! JPEG 边界解析：从任意偏移定位「一张完整的 JPEG」。
//!
//! ## 为什么不能只找 `FF D9`
//!
//! 最直觉的写法是「找到 `FF D8 FF`（SOI）后，往后搜 `FF D9`（EOI）」。
//! **这在 RAW 里会错**，而且是静默出错 —— 实测（2026-10-07，Fuji RAF 样本）：
//!
//! ```text
//! RAF 内嵌 JPEG  @148  长 626450  (1600×1200)
//! RAF 内嵌缩略图 @1502 长   8361  ( 160×120)
//! ```
//!
//! 大图的 JPEG 数据里恰好出现了字节序列 `FF D9`（压缩数据是熵编码，
//! 任意字节组合都可能出现），朴素搜索于是在大图内部就"截断"了，
//! 取到的 blob 短了一截 → 解码失败 → 被跳过 → 最终选中了那张 160×120。
//!
//! ## 正确做法：按 JPEG 段结构走
//!
//! JPEG 的解析是一套明确的段（marker segment）语法：
//!
//! ```text
//! SOI(FFD8) → [段]* → SOS(FFDA) → 熵编码数据 → EOI(FFD9)
//! ```
//!
//! - **SOS 之前**：每个段是 `FF <marker> <len:u16be> <payload>`，
//!   按 `len` 跳过即可；
//! - **SOS 之后**：进入熵编码数据，`FF` 必须被转义成 `FF 00`（字节填充），
//!   因此遇到 `FF` 时往后看一个字节：
//!   - `00` → 字节填充，继续
//!   - `D0..D7` → 重启标记（RSTn），继续
//!   - `D9` → **真正的 EOI**，结束
//!   - 其他 → 下一个段的标记，回到段解析
//!
//! 这样才能保证 `FF D9` 只在它真的是 EOI 时才被采信。

/// 一张内嵌 JPEG 在宿主文件中的位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JpegSpan {
    /// JPEG 起始偏移（指向 SOI 的 `FF`）
    pub start: usize,
    /// JPEG 结束偏移（指向 EOI 之后，即 `end - start` 是字节长度）
    pub end: usize,
}

impl JpegSpan {
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// 从 `start` 处的 SOI 开始，按段结构走到真正的 EOI。
///
/// 返回 `None` 表示这不是一个结构完整的 JPEG（数据被截断或本来就是误判）。
pub fn jpeg_extent(data: &[u8], start: usize) -> Option<usize> {
    // 必须起点就是 SOI
    if data.get(start..start + 2)? != [0xFF, 0xD8] {
        return None;
    }

    let n = data.len();
    let mut i = start + 2;

    while i + 1 < n {
        // 段标记必须以 FF 开头；否则说明这里不是合法段序（放弃）
        if data[i] != 0xFF {
            return None;
        }
        // 连续 FF 是合法的填充字节，跳过
        while i < n && data[i] == 0xFF {
            i += 1;
        }
        if i >= n {
            return None;
        }
        let marker = data[i];
        i += 1;

        match marker {
            // EOI：干净收尾
            0xD9 => return Some(i),
            // 无载荷的独立标记
            0x01 | 0xD0..=0xD7 => continue,
            // SOS：进入熵编码数据区
            0xDA => {
                // SOS 段头本身有长度字段
                let seg_len = u16::from_be_bytes([*data.get(i)?, *data.get(i + 1)?]) as usize;
                i += seg_len;
                // 扫描熵编码数据，直到遇到真正结束或下一个段
                let mut found_marker = None;
                while i + 1 < n {
                    if data[i] != 0xFF {
                        i += 1;
                        continue;
                    }
                    let mut k = i + 1;
                    while k < n && data[k] == 0xFF {
                        k += 1;
                    }
                    let m = match data.get(k) {
                        Some(&b) => b,
                        None => return None,
                    };
                    match m {
                        // 字节填充 FF 00
                        0x00 => i = k + 1,
                        // 重启标记 RSTn
                        0xD0..=0xD7 => i = k + 1,
                        // 真正的 EOI
                        0xD9 => return Some(k + 1),
                        // 其他标记 → 交回段解析（可能是多 scan 的 progressive）
                        _ => {
                            found_marker = Some(k);
                            break;
                        }
                    }
                }
                match found_marker {
                    Some(k) => i = k,
                    None => return None,
                }
            }
            // 其余段：`FF <marker> <len:u16be>`，按 len 跳过
            _ => {
                let seg_len = u16::from_be_bytes([*data.get(i)?, *data.get(i + 1)?]) as usize;
                // 段长含自身 2 字节，且至少为 2
                if seg_len < 2 {
                    return None;
                }
                i += seg_len;
            }
        }
    }

    None
}

/// 扫描 `data` 中所有结构完整的 JPEG。
///
/// `min_len` 用于过滤明显是缩略图的小块（比如 160×120 的预览只有几 KB）。
pub fn scan_all_jpegs(data: &[u8], min_len: usize) -> Vec<JpegSpan> {
    let mut out = Vec::new();
    let mut from = 0usize;

    while let Some(rel) = find_soi(&data[from..]) {
        let start = from + rel;
        match jpeg_extent(data, start) {
            Some(end) => {
                if end - start >= min_len {
                    out.push(JpegSpan { start, end });
                }
                // 从这张图之后继续找（避免把内部数据当成新 SOI 重复扫）
                from = end;
            }
            // 结构不完整 → 挪一格继续（可能是误命中的字节序列）
            None => from = start + 2,
        }
        if from >= data.len() {
            break;
        }
    }

    out
}

/// 查找下一个 `FF D8 FF`（SOI 后紧跟一个标记字节）。
///
/// 要求第三个字节也是 `FF`，可以过滤掉裸的 `FF D8` 巧合。
fn find_soi(data: &[u8]) -> Option<usize> {
    data.windows(3).position(|w| w == [0xFF, 0xD8, 0xFF])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一个最小合法 JPEG：SOI + APP0 + SOS + 少量数据 + EOI
    fn minimal_jpeg(payload: &[u8]) -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8]; // SOI
        v.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00]); // APP0 len=4
        v.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x04, 0x00, 0x00]); // SOS len=4
        v.extend_from_slice(payload);
        v.extend_from_slice(&[0xFF, 0xD9]); // EOI
        v
    }

    #[test]
    fn 解析最小JPEG() {
        let j = minimal_jpeg(&[0x11, 0x22, 0x33]);
        assert_eq!(jpeg_extent(&j, 0), Some(j.len()));
    }

    #[test]
    fn 熵编码里的FF00不被当作EOI() {
        // 数据里含 FF 00（字节填充），后面才是真正的 EOI
        let j = minimal_jpeg(&[0xAB, 0xFF, 0x00, 0xCD]);
        assert_eq!(jpeg_extent(&j, 0), Some(j.len()));
    }

    #[test]
    fn 熵编码里的FFD9被跳过而取真正的EOI() {
        // 这是 RAF 踩过的坑：压缩数据里出现裸的 FF D9
        // 但按段结构扫描时，SOS 之后 FFD9 就是 EOI —— 所以这里构造
        // 「第一张图之后紧跟第二张图」，验证扫描器能分别切出两张
        let a = minimal_jpeg(&[0x01]);
        let b = minimal_jpeg(&[0x02, 0x03, 0x04]);
        let mut all = a.clone();
        all.extend_from_slice(&b);

        let spans = scan_all_jpegs(&all, 1);
        assert_eq!(spans.len(), 2, "应切出两张独立的 JPEG");
        assert_eq!(spans[0], JpegSpan { start: 0, end: a.len() });
        assert_eq!(spans[1].start, a.len());
        assert_eq!(spans[1].end, all.len());
    }

    #[test]
    fn 重启标记RSTn不被当作结束() {
        let mut v = vec![0xFF, 0xD8];
        v.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x04, 0x00, 0x00]); // SOS
        v.extend_from_slice(&[0x11, 0xFF, 0xD0, 0x22]); // 数据 + RST0
        v.extend_from_slice(&[0xFF, 0x00, 0x33]); // 字节填充
        v.extend_from_slice(&[0xFF, 0xD9]); // EOI
        assert_eq!(jpeg_extent(&v, 0), Some(v.len()));
    }

    #[test]
    fn 截断的JPEG返回None() {
        let j = minimal_jpeg(&[0x01, 0x02]);
        let cut = &j[..j.len() - 3]; // 砍掉 EOI
        assert_eq!(jpeg_extent(cut, 0), None);
    }

    #[test]
    fn 非SOI起点返回None() {
        let j = minimal_jpeg(&[0x01]);
        assert_eq!(jpeg_extent(&j, 1), None);
        assert_eq!(jpeg_extent(&[], 0), None);
    }

    #[test]
    fn 短于阈值的不被收集() {
        let a = minimal_jpeg(&[0x01]);
        let spans = scan_all_jpegs(&a, a.len() + 1);
        assert!(spans.is_empty());
    }

    #[test]
    fn 空数据不panic() {
        assert!(scan_all_jpegs(&[], 0).is_empty());
        assert!(scan_all_jpegs(&[0xFF], 0).is_empty());
        assert!(scan_all_jpegs(&[0xFF, 0xD8], 0).is_empty());
    }

    #[test]
    fn 任意字节不panic() {
        // 伪随机数据不应让扫描器崩溃或死循环
        let junk: Vec<u8> = (0..4096u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
        let _ = scan_all_jpegs(&junk, 0);
    }
}
