//! 改写 TIFF/EXIF 字节，让它与新写出的像素**保持一致**。
//!
//! ## 背景：本应用的像素从不应用 `Orientation`
//!
//! PurePhotoExplorer 的画布、裁剪、打印全都基于**文件里的原始像素**，`Orientation`
//! 只作为一条信息显示（信息面板的「方向」行），解码链（zune-jpeg / png /
//! 各 RAW 后端）不会拿它去旋转画面。这一点决定了下面两条规则的取舍。
//!
//! ## 规则一：用户旋转过 → `Orientation` 必须置 1
//!
//! 画布旋转是**真的重采样像素**（`apply_edits`），不是改方向标记。此时若把源
//! EXIF 原样搬走，别的软件会在已经转正的画面上**再转一次**，照片就横了。
//! 置 1 表示「像素即最终朝向」，与用户在画布上看到的一致。
//!
//! ## 规则二：用户没旋转 → 保持原值
//!
//! 纯「另存为」（没旋转）时像素一个字节都没变，方向信息就该原样保留 ——
//! 这正是用户要的「带上 EXIF」。无条件改成 1 会**凭空丢掉**
//! 「这张竖拍照片需要转 90°」这条信息，属于数据损失。
//!
//! ## 缩略图（IFD1）一律切断
//!
//! 它记录的是**编辑前**的小图。裁剪或旋转之后必然与实际画面不符，
//! 保留只会让资源管理器/部分软件显示错误的预览。顺带也省掉一大半体积
//! （缩略图通常占 EXIF 的一半以上），基本告别 JPEG APP1 的 64 KB 上限。
//!
//! ## 失败策略
//! 任何一处结构不符合预期（截断、偏移越界、字节序不认识）就**原样返回**：
//! 宁可带上一个"方向可能不对"的 EXIF，也不要写出一份损坏的元数据
//! —— 后者会让整个文件在被解析时出错。

/// 把 EXIF 调成与「已按 `rotation_steps` 旋转过的像素」一致。
///
/// `rotation_steps`：用户顺时针旋转的 90° 次数（`rem_euclid(4)` 后判断，
/// 0 表示没转过，此时方向标记原样保留）。
pub fn align_with_pixels(tiff: &mut [u8], rotation_steps: i32) -> Outcome {
    let Some(head) = Head::parse(tiff) else {
        return Outcome::Unparsable;
    };
    // IFD0 必须完整可解析（条目标签与切缩略图都要用它）。
    // 截断/越界说明这份 EXIF 不完整 —— 宁可不碰，也不要半改。
    let Some(entries) = head.ifd0_entries(tiff) else {
        return Outcome::Unparsable;
    };

    let mut turned = false;
    if rotation_steps.rem_euclid(4) != 0 {
        for e in entries {
            if e.tag != TAG_ORIENTATION {
                continue;
            }
            // 值内联在条目里（SHORT，2 字节），位于条目自身偏移 +8
            let at = e.offset + 8;
            let v = match head.big {
                true => u16::from_be_bytes([tiff[at], tiff[at + 1]]),
                false => u16::from_le_bytes([tiff[at], tiff[at + 1]]),
            };
            if v != 1 {
                let b = match head.big {
                    true => 1u16.to_be_bytes(),
                    false => 1u16.to_le_bytes(),
                };
                tiff[at..at + 2].copy_from_slice(&b);
                turned = true;
            }
        }
    }

    // 切断 IFD1（缩略图）：IFD0 条目区之后那 4 字节就是 next-IFD 偏移
    let mut dropped_thumb = false;
    if let Some(next_at) = head.next_ifd_offset_at(tiff) {
        let next = match head.big {
            true => u32::from_be_bytes(tiff[next_at..next_at + 4].try_into().unwrap()),
            false => u32::from_le_bytes(tiff[next_at..next_at + 4].try_into().unwrap()),
        };
        if next != 0 {
            tiff[next_at..next_at + 4].copy_from_slice(&[0, 0, 0, 0]);
            dropped_thumb = true;
        }
    }

    Outcome::Adjusted {
        orientation_reset: turned,
        thumbnail_dropped: dropped_thumb,
    }
}

/// 改写结果（给测试与日志用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// 结构不认识，**一个字节都没动**
    Unparsable,
    Adjusted {
        orientation_reset: bool,
        thumbnail_dropped: bool,
    },
}

/// Orientation 标签号。
const TAG_ORIENTATION: u16 = 0x0112;

/// TIFF 头信息。
struct Head {
    /// 大端序（`MM`）
    big: bool,
    /// IFD0 起始偏移
    ifd0: usize,
}

impl Head {
    fn parse(tiff: &[u8]) -> Option<Self> {
        let h = tiff.get(..8)?;
        let big = match &h[..2] {
            b"MM" => true,
            b"II" => false,
            _ => return None,
        };
        // 魔数 42（标准 TIFF）。厂商私有魔数（ORF `RO`、RW2 `U\0`）在上游
        // `normalize_tiff_magic` 已修过，这里只认标准值。
        let magic = match big {
            true => u16::from_be_bytes([h[2], h[3]]),
            false => u16::from_le_bytes([h[2], h[3]]),
        };
        if magic != 42 {
            return None;
        }
        let ifd0 = match big {
            true => u32::from_be_bytes([h[4], h[5], h[6], h[7]]),
            false => u32::from_le_bytes([h[4], h[5], h[6], h[7]]),
        } as usize;
        Some(Self { big, ifd0 })
    }

    /// IFD0 的条目数。
    fn count(&self, tiff: &[u8]) -> Option<usize> {
        let at = self.ifd0;
        let b = tiff.get(at..at + 2)?;
        Some(match self.big {
            true => u16::from_be_bytes([b[0], b[1]]),
            false => u16::from_le_bytes([b[0], b[1]]),
        } as usize)
    }

    /// next-IFD 指针所在的绝对偏移（= IFD0 条目区之后那 4 字节）。
    fn next_ifd_offset_at(&self, tiff: &[u8]) -> Option<usize> {
        let n = self.count(tiff)?;
        let at = self.ifd0.checked_add(2)?.checked_add(n.checked_mul(12)?)?;
        // 需要 4 字节可写
        tiff.get(at..at + 4)?;
        Some(at)
    }

    fn ifd0_entries(&self, tiff: &[u8]) -> Option<Vec<Entry>> {
        let n = self.count(tiff)?;
        // 条目区必须完整落在数据内，否则视为截断（改写有风险，放弃）
        let end = self.ifd0.checked_add(2)?.checked_add(n.checked_mul(12)?)?;
        if end + 4 > tiff.len() {
            return None;
        }
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let o = self.ifd0 + 2 + i * 12;
            let tag = match self.big {
                true => u16::from_be_bytes([tiff[o], tiff[o + 1]]),
                false => u16::from_le_bytes([tiff[o], tiff[o + 1]]),
            };
            out.push(Entry { tag, offset: o });
        }
        Some(out)
    }
}

struct Entry {
    tag: u16,
    /// 条目在字节流里的绝对偏移
    offset: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一份最小可用的 TIFF：IFD0 里放 Orientation(=6) + 一个 next-IFD 指针。
    fn make_tiff(orientation: u16, next_ifd: u32, big: bool) -> Vec<u8> {
        let mut t = Vec::new();
        let put16 = |t: &mut Vec<u8>, v: u16| {
            t.extend_from_slice(&match big { true => v.to_be_bytes(), false => v.to_le_bytes() });
        };
        let put32 = |t: &mut Vec<u8>, v: u32| {
            t.extend_from_slice(&match big { true => v.to_be_bytes(), false => v.to_le_bytes() });
        };
        t.extend_from_slice(if big { b"MM" } else { b"II" });
        put16(&mut t, 42);
        put32(&mut t, 8); // IFD0 紧跟头之后
        put16(&mut t, 1); // 1 个条目
        put16(&mut t, TAG_ORIENTATION);
        put16(&mut t, 3); // SHORT
        put32(&mut t, 1); // count
        put16(&mut t, orientation); // 值内联
        put16(&mut t, 0); // 补齐 4 字节
        put32(&mut t, next_ifd); // next IFD 指针
        t
    }

    fn read_orientation(t: &[u8], big: bool) -> u16 {
        let at = 8 + 2 + 8;
        match big {
            true => u16::from_be_bytes([t[at], t[at + 1]]),
            false => u16::from_le_bytes([t[at], t[at + 1]]),
        }
    }

    #[test]
    fn 旋转过时方向被重置为1且小端正确() {
        let mut t = make_tiff(6, 0, false);
        let o = align_with_pixels(&mut t, 1);
        assert_eq!(
            o,
            Outcome::Adjusted { orientation_reset: true, thumbnail_dropped: false }
        );
        assert_eq!(read_orientation(&t, false), 1, "Orientation 应被改成 1");
        // 头与魔数不能被动过
        assert_eq!(&t[..2], b"II");
        assert_eq!(&t[2..4], &42u16.to_le_bytes());
    }

    #[test]
    fn 旋转过时方向被重置为1且大端正确() {
        let mut t = make_tiff(8, 0, true);
        align_with_pixels(&mut t, 3);
        assert_eq!(read_orientation(&t, true), 1);
        assert_eq!(&t[..2], b"MM");
    }

    /// ⭐ **没旋转过就一个字节都不许动**。
    ///
    /// 这是「另存为」的主场景（用户只是想导出原图）。像素没变，
    /// 方向信息就该原样保留 —— 无条件改成 1 会把「这张竖拍照片需要转 90°」
    /// 凭空抹掉，属于数据损失。
    #[test]
    fn 未旋转时方向原样保留() {
        let mut t = make_tiff(6, 0, false);
        let before = t.clone();
        let o = align_with_pixels(&mut t, 0);
        assert_eq!(
            o,
            Outcome::Adjusted { orientation_reset: false, thumbnail_dropped: false }
        );
        assert_eq!(t, before, "没旋转时不该有任何字节变化");
        assert_eq!(read_orientation(&t, false), 6, "方向应保持原值");

        // 转满一圈（4 次）也等于没转
        let mut t2 = make_tiff(6, 0, false);
        let before2 = t2.clone();
        align_with_pixels(&mut t2, 4);
        assert_eq!(t2, before2, "转 4 次 = 没转");
    }

    #[test]
    fn 已经是1时报告未改动() {
        let mut t = make_tiff(1, 0, false);
        let before = t.clone();
        let o = align_with_pixels(&mut t, 2);
        assert_eq!(
            o,
            Outcome::Adjusted { orientation_reset: false, thumbnail_dropped: false }
        );
        assert_eq!(t, before, "值本来就是 1，不该写一遍");
    }

    /// 缩略图（IFD1）必须被切断 —— 那条链上的图是**旋转/裁剪前**的画面。
    #[test]
    fn 缩略图指针被清零() {
        let mut t = make_tiff(1, 0x1234, false);
        let o = align_with_pixels(&mut t, 0);
        assert_eq!(
            o,
            Outcome::Adjusted { orientation_reset: false, thumbnail_dropped: true }
        );
        let at = 8 + 2 + 12;
        assert_eq!(&t[at..at + 4], &[0, 0, 0, 0], "next-IFD 指针应清零");
    }

    /// 结构不认识时**一个字节都不许动** —— 写坏元数据比不写更糟。
    #[test]
    fn 结构异常时原样返回() {
        for bad in [
            b"XX\0\0\0\0\0\0".to_vec(),      // 字节序不认识
            b"II\x2b\0\x08\0\0\0".to_vec(),  // 魔数不是 42（BigTIFF）
            b"II*\0".to_vec(),               // 太短
            Vec::new(),
        ] {
            let mut t = bad.clone();
            assert_eq!(align_with_pixels(&mut t, 1), Outcome::Unparsable);
            assert_eq!(t, bad, "不该改动任何字节");
        }
    }

    /// 条目区被截断（IFD0 声称有 100 个条目但数据不够）→ 放弃方向改写。
    #[test]
    fn 截断的IFD0不改方向() {
        let mut t = Vec::new();
        t.extend_from_slice(b"II");
        t.extend_from_slice(&42u16.to_le_bytes());
        t.extend_from_slice(&8u32.to_le_bytes());
        t.extend_from_slice(&100u16.to_le_bytes()); // 声称 100 个条目
        let before = t.clone();
        assert_eq!(align_with_pixels(&mut t, 1), Outcome::Unparsable);
        assert_eq!(t, before);
    }
}
