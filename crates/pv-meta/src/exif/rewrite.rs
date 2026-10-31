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
            // 值内联在条目里：偏移 +8，宽度看类型
            let Some((at, width)) = orientation_slot(&e, tiff) else {
                // 类型/数量不是预期的 `SHORT×1` / `LONG×1` —— 说明我们对这份 EXIF
                // 的理解是错的，按失败策略**整份原样返回**，一个字节都不动。
                return Outcome::Unparsable;
            };
            if width.read(tiff, at, head.big) != Some(1) {
                width.write_one(tiff, at, head.big);
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

/// TIFF 数据类型 `SHORT`（2 字节无符号）。
const TYPE_SHORT: u16 = 3;
/// TIFF 数据类型 `LONG`（4 字节无符号）。
const TYPE_LONG: u16 = 4;

/// 条目里内联值的宽度（值域 ≤ 4 字节时直接放在条目末 4 字节里）。
#[derive(Debug, Clone, Copy)]
enum Width {
    /// 2 字节（`SHORT`）
    Short,
    /// 4 字节（`LONG`）
    Long,
}

impl Width {
    fn read(self, tiff: &[u8], at: usize, big: bool) -> Option<u32> {
        let b = tiff.get(at..at.checked_add(self.bytes())?)?;
        Some(match (self, big) {
            (Width::Short, true) => u16::from_be_bytes([b[0], b[1]]) as u32,
            (Width::Short, false) => u16::from_le_bytes([b[0], b[1]]) as u32,
            (Width::Long, true) => u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
            (Width::Long, false) => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        })
    }

    /// 写数值 `1`（本模块唯一会写的值：方向 = 「像素即最终朝向」）。
    fn write_one(self, tiff: &mut [u8], at: usize, big: bool) {
        match (self, big) {
            (Width::Short, true) => tiff[at..at + 2].copy_from_slice(&1u16.to_be_bytes()),
            (Width::Short, false) => tiff[at..at + 2].copy_from_slice(&1u16.to_le_bytes()),
            (Width::Long, true) => tiff[at..at + 4].copy_from_slice(&1u32.to_be_bytes()),
            (Width::Long, false) => tiff[at..at + 4].copy_from_slice(&1u32.to_le_bytes()),
        }
    }

    fn bytes(self) -> usize {
        match self {
            Width::Short => 2,
            Width::Long => 4,
        }
    }
}

/// `Orientation` 的值落在哪儿、占多宽；**类型或数量不合预期就返回 `None`**。
///
/// ## 为什么不能默认「2 字节」
/// 规范里 `Orientation` 是 `SHORT × 1`（值内联在条目 +8，占 4 字节里的前 2 字节），
/// 但有些写入方按 `LONG × 1` 写。若一律当 2 字节处理，**大端下会写坏值**：
/// `LONG 6` 是 `00 00 00 06`，按 2 字节读出 `0`（≠1）→ 把 `1` 写进前两字节 →
/// 变成 `00 01 00 06`。这种「悄悄改错元数据」比不写更糟。
///
/// `count != 1` 时值域里存的是**偏移量**而不是值，往里写就等于破坏那个指针，
/// 同样必须放弃。
fn orientation_slot(e: &Entry, tiff: &[u8]) -> Option<(usize, Width)> {
    if e.count != 1 {
        return None;
    }
    let width = match e.ty {
        TYPE_SHORT => Width::Short,
        TYPE_LONG => Width::Long,
        _ => return None,
    };
    let at = e.offset.checked_add(8)?;
    // 值域必须完整可读（写之前也在这里挡住越界）
    tiff.get(at..at.checked_add(width.bytes())?)?;
    Some((at, width))
}

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
            // 类型（+2）与数量（+4）：判断值域是内联还是偏移量，必须看这两个字段
            let ty = match self.big {
                true => u16::from_be_bytes([tiff[o + 2], tiff[o + 3]]),
                false => u16::from_le_bytes([tiff[o + 2], tiff[o + 3]]),
            };
            let count = match self.big {
                true => u32::from_be_bytes([tiff[o + 4], tiff[o + 5], tiff[o + 6], tiff[o + 7]]),
                false => u32::from_le_bytes([tiff[o + 4], tiff[o + 5], tiff[o + 6], tiff[o + 7]]),
            };
            out.push(Entry { tag, ty, count, offset: o });
        }
        Some(out)
    }
}

struct Entry {
    tag: u16,
    /// TIFF 数据类型（`SHORT` / `LONG` / …）
    ty: u16,
    /// 元素个数；`> 1` 表示值域里存的是**偏移量**而不是值本身
    count: u32,
    /// 条目在字节流里的绝对偏移
    offset: usize,
}

/// 读 IFD0 里 `Orientation` 的**原始值**（1–8）；没有该标签或结构不完整返回 `None`。
///
/// 与 [`align_with_pixels`] 共用同一套解析（含 [`orientation_slot`] 的类型校验），
/// 供「按 EXIF 批量摆正」判断该转多少度：`3` → 180°，`6` → 顺时针 90°，`8` → 逆时针 90°。
/// `2/4/5/7` 需要**镜像**，而画布只有旋转/裁剪，调用方应当跳过并如实报告。
pub fn read_orientation(tiff: &[u8]) -> Option<u16> {
    let head = Head::parse(tiff)?;
    let entries = head.ifd0_entries(tiff)?;
    for e in entries {
        if e.tag != TAG_ORIENTATION {
            continue;
        }
        // 类型/数量不合预期 → 当作读不到，让调用方跳过而不是猜
        let (at, width) = orientation_slot(&e, tiff)?;
        return Some(width.read(tiff, at, head.big)? as u16);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一份最小可用的 TIFF：IFD0 里放 Orientation(=6) + 一个 next-IFD 指针。
    fn make_tiff(orientation: u16, next_ifd: u32, big: bool) -> Vec<u8> {
        make_tiff_typed(orientation as u32, TYPE_SHORT, 1, next_ifd, big)
    }

    /// 同上，但**类型与数量可选** —— 真实世界里有按 `LONG × 1` 写方向的软件。
    fn make_tiff_typed(value: u32, ty: u16, count: u32, next_ifd: u32, big: bool) -> Vec<u8> {
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
        put16(&mut t, ty);
        put32(&mut t, count);
        put32(&mut t, value); // 值内联（≤4 字节）
        put32(&mut t, next_ifd); // next IFD 指针
        t
    }

    /// 直接按 `SHORT` 窥探条目值域前 2 字节 —— 不经过被测代码，用来验证「字节被改成了什么」。
    fn peek_orientation(t: &[u8], big: bool) -> u16 {
        let at = 8 + 2 + 8;
        match big {
            true => u16::from_be_bytes([t[at], t[at + 1]]),
            false => u16::from_le_bytes([t[at], t[at + 1]]),
        }
    }

    /// 按 `LONG` 窥探条目值域 4 字节。
    fn peek_orientation_long(t: &[u8], big: bool) -> u32 {
        let at = 8 + 2 + 8;
        let b = [t[at], t[at + 1], t[at + 2], t[at + 3]];
        match big {
            true => u32::from_be_bytes(b),
            false => u32::from_le_bytes(b),
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
        assert_eq!(peek_orientation(&t, false), 1, "Orientation 应被改成 1");
        // 头与魔数不能被动过
        assert_eq!(&t[..2], b"II");
        assert_eq!(&t[2..4], &42u16.to_le_bytes());
    }

    #[test]
    fn 旋转过时方向被重置为1且大端正确() {
        let mut t = make_tiff(8, 0, true);
        align_with_pixels(&mut t, 3);
        assert_eq!(peek_orientation(&t, true), 1);
        assert_eq!(&t[..2], b"MM");
    }

    /// 方向按 `LONG × 1` 写的文件（非规范但真实存在）也必须改对。
    ///
    /// 若一律按 2 字节处理，**大端会写坏**：`LONG 6` = `00 00 00 06`，
    /// 按 2 字节读出 `0`（≠1）→ 把 `1` 写进前两字节 → `00 01 00 06`。
    #[test]
    fn LONG类型的方向也能正确改写() {
        for big in [false, true] {
            let mut t = make_tiff_typed(6, TYPE_LONG, 1, 0, big);
            assert_eq!(read_orientation(&t), Some(6), "LONG 值本身要读得出");
            let o = align_with_pixels(&mut t, 1);
            assert_eq!(
                o,
                Outcome::Adjusted { orientation_reset: true, thumbnail_dropped: false },
                "big={big}"
            );
            assert_eq!(peek_orientation_long(&t, big), 1, "big={big}：整个 4 字节都应写成 1");
        }
    }

    /// 类型的不是 `SHORT` / `LONG`，或数量不是 1 → 对这份 EXIF 的理解是错的，
    /// **整份原样返回**（`count > 1` 时值域里存的是偏移量，往里写等于毁指针）。
    #[test]
    fn 方向类型不合预期时整份放弃() {
        for (ty, count) in [(1u16, 1u32), (5, 1), (TYPE_SHORT, 2), (TYPE_LONG, 3)] {
            let mut t = make_tiff_typed(6, ty, count, 0x99, false);
            let before = t.clone();
            assert_eq!(
                align_with_pixels(&mut t, 1),
                Outcome::Unparsable,
                "ty={ty} count={count}"
            );
            assert_eq!(t, before, "ty={ty} count={count}：不该改动任何字节");
            assert_eq!(read_orientation(&t), None, "读不到就别猜");
        }
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
        assert_eq!(peek_orientation(&t, false), 6, "方向应保持原值");

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
