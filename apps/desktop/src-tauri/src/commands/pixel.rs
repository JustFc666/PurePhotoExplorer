//! 像素传输协议（PVI1 静图 / PVA1 动图）—— **统一走二进制，不走 base64**。
//!
//! ⚠️ **2026-10-09 关键变更**：此前所有像素都 base64 成 JSON 字符串回传，
//! 是为了绕开「大 raw 响应退化成 JSON 数字数组」的坑。那个坑的**真正根因是
//! CSP 没放行 `ipc: http://ipc.localhost`** —— `fetch` 被拦后 Tauri 会回退到
//! `window.ipc.postMessage` 通道，而该通道对大响应走 `webview.eval()`：
//! 把数据拼进 JS 源码再交给 WebView2 执行（Rust→COM 还要转 UTF-16），
//! 95MB 的动图在这儿要花好几秒。
//!
//! `tauri.conf.json` 的 CSP 补上 `connect-src ipc: http://ipc.localhost` 之后，
//! invoke 恢复走自定义协议的 `fetch`：响应按 `content-type` 分派 ——
//! `application/octet-stream` → `response.arrayBuffer()`（**真二进制**），
//! `application/json` → `response.json()`。于是：
//! - 字节直接过桥，**零 base64 膨胀**（省 33% 体积与一端编码一端解码的 CPU）；
//! - 前端 `toBytes()` 拿到 `ArrayBuffer`，零拷贝切视图。
//!
//! 所以现在两个包都是**裸字节**，`tauri::ipc::Response::new(..)` 返回。
//! 前端 `ipc/protocol.ts::toBytes()` 仍兼容字符串/数组等形态 ——
//! 万一将来 CSP 又被改坏、退化成旧通道，功能不坏，只是变慢（有兜底）。
//!
//! ## PVI1（静图，单帧）
//! ```text
//! magic:  4 bytes  = "PVI1"
//! width:  4 bytes  u32
//! height: 4 bytes  u32
//! flags:  1 byte   保留
//! ─── 以下为像素数据，紧随其后 ───
//! rgba:   width * height * 4 bytes
//! ```
//!
//! ## PVA1（动图，多帧）
//! 见 [`pack_animation`] 上方注释。

use pv_decode::RgbaImage;

pub const PIXEL_MAGIC: &[u8; 4] = b"PVI1";
pub const PIXEL_HEADER_LEN: usize = 13;

/// 把像素数据打包成传输格式
pub fn pack_pixels(img: &RgbaImage) -> Vec<u8> {
    let mut buf = Vec::with_capacity(PIXEL_HEADER_LEN + img.data.len());
    buf.extend_from_slice(PIXEL_MAGIC);
    buf.extend_from_slice(&img.width.to_le_bytes());
    buf.extend_from_slice(&img.height.to_le_bytes());
    buf.push(0); // flags 预留
    buf.extend_from_slice(&img.data);
    buf
}

// ─────────────────────────── 动图传输协议（PVA1）───────────────────────────
//
// 静图一帧一个 `decode_image` 调用即可；动图有几百帧，逐帧调用会引入
// 几百次 IPC 往返，拼成一个 JSON 又要每帧 base64 一遍。这里用**二进制大块**：
// 头部（含每帧延时表）之后紧跟**首尾相连的 RGBA 帧数据**。
//
// 布局（全部小端）：
// ```text
// magic:   4 bytes  = "PVA1"
// version: 2 bytes  u16
// flags:   2 bytes  u16   bit0 = 被上限截断
// width:   4 bytes  u32   逻辑屏幕宽
// height:  4 bytes  u32   逻辑屏幕高
// loop:    4 bytes  i32   -1 = 无限循环；>=0 = 额外循环次数
// count:   4 bytes  u32   **本块**含多少帧
// start:   4 bytes  u32   本块第一帧在整段动画里的序号
// total:   4 bytes  u32   整段动画的总帧数
// ─── 以下为延时表 ───
// delay:   count × 4 bytes  u32，每帧显示时长（毫秒）
// ─── 以下为像素区 ───
// rgba:    count × width*height*4 bytes，**逐帧首尾相连**（帧长固定故无需偏移表）
// ```
//
// 固定头 32 字节 + `4*count` 延时表。帧长固定（都是整张逻辑屏幕），
// 所以前端只需 `base + i*stride` 就能零拷贝切出第 i 帧。
//
// ## 为什么要 `start` / `total`（v2 新增，2026-10-09）
// 动图可能几十 MB（299 帧 × 250×250 实测 71MB，传输约 1.2s）。
// 与其让用户干等，不如**分两段传**：先传开头几秒的帧让画面立刻动起来，
// 剩下的在后台接着传。`start`/`total` 就是让前端知道"这一块接在哪儿、
// 还差多少"——没有这两个字段，分块传输就没法做（见 `commands/image.rs`）。

/// 动图包魔数
pub const ANIM_MAGIC: &[u8; 4] = b"PVA1";
/// 动图包版本（改动布局时必须 +1，前端据此拒绝不认识的版本）
pub const ANIM_VERSION: u16 = 2;
/// 固定头长度（magic4+ver2+flags2+w4+h4+loop4+count4+start4+total4）
pub const ANIM_HEADER_FIXED: usize = 32;
/// flags 位：因上限（帧数/总内存）被截断
pub const ANIM_FLAG_TRUNCATED: u16 = 1 << 0;

/// 打包动图块所需的全部信息。
///
/// 用结构体而不是一长串位置参数：帧的起止、总量、循环方式互相独立，
/// 位置参数一多就容易传错顺序（而且编译器不会提醒，因为类型都相同）。
pub struct AnimPack<'a> {
    pub width: u32,
    pub height: u32,
    /// `None` = 无限循环
    pub loop_count: Option<u16>,
    pub truncated: bool,
    /// 本块第一帧在整段动画里的序号
    pub start_index: u32,
    /// 整段动画的总帧数（不是本块的帧数）
    pub total_count: u32,
    /// `(延时毫秒, RGBA 数据)`，长度必须都为 `width*height*4`
    pub frames: Vec<(u32, &'a [u8])>,
}

/// 把一个**帧区间**打包成一个二进制块（协议见上方注释）。
pub fn pack_animation(p: AnimPack<'_>) -> Vec<u8> {
    let AnimPack {
        width,
        height,
        loop_count,
        truncated,
        start_index,
        total_count,
        frames,
    } = p;

    let stride = width as usize * height as usize * 4;
    let mut buf = Vec::with_capacity(ANIM_HEADER_FIXED + frames.len() * 4 + frames.len() * stride);

    buf.extend_from_slice(ANIM_MAGIC);
    buf.extend_from_slice(&ANIM_VERSION.to_le_bytes());
    buf.extend_from_slice(
        &if truncated { ANIM_FLAG_TRUNCATED } else { 0 }.to_le_bytes(),
    );
    buf.extend_from_slice(&width.to_le_bytes());
    buf.extend_from_slice(&height.to_le_bytes());
    // 统一用 i32 承载循环次数：-1 表示无限，省得前端再处理 Option
    buf.extend_from_slice(&loop_count.map(i32::from).unwrap_or(-1).to_le_bytes());
    buf.extend_from_slice(&(frames.len() as u32).to_le_bytes());
    buf.extend_from_slice(&start_index.to_le_bytes());
    buf.extend_from_slice(&total_count.to_le_bytes());

    for (delay, _) in &frames {
        buf.extend_from_slice(&delay.to_le_bytes());
    }
    for (_, data) in &frames {
        buf.extend_from_slice(data);
    }
    buf
}

/// 动图块头部（解包结果）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnimHeader {
    pub width: u32,
    pub height: u32,
    /// `-1` = 无限循环
    pub loop_count: i32,
    pub truncated: bool,
    /// 本块第一帧在整段动画里的序号
    pub start_index: u32,
    /// 整段动画的总帧数
    pub total_count: u32,
}

/// 解包动图块（Rust 侧自检实现，镜像前端 `parseAnimation`）。
///
/// 与 [`pack_animation`] 成对，用于测试协议自洽 —— 前端是 TypeScript
/// 的 DataView 实现，两边必须保持一致。
///
/// `allow(dead_code)`：生产路径只打包不解包（解包在前端），
/// 这里留着是给测试与将来的调试工具用。
#[allow(dead_code, clippy::type_complexity)]
pub fn decode_animation_blob(
    buf: &[u8],
) -> std::result::Result<(AnimHeader, Vec<(u32, &[u8])>), &'static str> {
    if buf.len() < ANIM_HEADER_FIXED {
        return Err("动图数据长度不足");
    }
    if &buf[0..4] != ANIM_MAGIC {
        return Err("动图魔数不匹配");
    }
    let version = u16::from_le_bytes([buf[4], buf[5]]);
    if version != ANIM_VERSION {
        return Err("动图协议版本不支持");
    }
    let flags = u16::from_le_bytes([buf[6], buf[7]]);
    let header = AnimHeader {
        width: u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
        height: u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]),
        loop_count: i32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]),
        truncated: flags & ANIM_FLAG_TRUNCATED != 0,
        // 这两个字段在 count 之后，先占位再补
        start_index: 0,
        total_count: 0,
    };
    let count = u32::from_le_bytes([buf[20], buf[21], buf[22], buf[23]]) as usize;
    let start_index = u32::from_le_bytes([buf[24], buf[25], buf[26], buf[27]]);
    let total_count = u32::from_le_bytes([buf[28], buf[29], buf[30], buf[31]]);
    let header = AnimHeader {
        start_index,
        total_count,
        ..header
    };

    let stride = header.width as usize * header.height as usize * 4;
    let table_start = ANIM_HEADER_FIXED;
    let pixel_start = table_start + count * 4;
    if buf.len() < pixel_start + count * stride {
        return Err("动图数据长度与声明的帧数/尺寸不匹配");
    }

    let mut frames = Vec::with_capacity(count);
    for i in 0..count {
        let d = table_start + i * 4;
        let delay = u32::from_le_bytes([buf[d], buf[d + 1], buf[d + 2], buf[d + 3]]);
        let p = pixel_start + i * stride;
        frames.push((delay, &buf[p..p + stride]));
    }
    Ok((header, frames))
}

/// 解包像素数据（Rust 侧的自检实现，镜像前端逻辑）
///
/// **保留这个函数的原因**：前端用 TypeScript 的 DataView 解析同一协议。
/// 两边实现必须保持一致，此函数让 Rust 侧的测试能验证协议自洽，
/// 且在未来写调试工具时可直接复用。
pub fn decode_pixels(buf: &[u8]) -> std::result::Result<(u32, u32, &[u8]), &'static str> {
    if buf.len() < PIXEL_HEADER_LEN {
        return Err("数据长度不足");
    }
    if &buf[0..4] != PIXEL_MAGIC {
        return Err("魔数不匹配，协议版本可能已变更");
    }
    let w = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
    let h = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);

    let expected = (w as usize) * (h as usize) * 4;
    let actual = buf.len() - PIXEL_HEADER_LEN;
    if actual != expected {
        return Err("像素数据长度与尺寸不匹配");
    }
    Ok((w, h, &buf[PIXEL_HEADER_LEN..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 像素包头长度正确() {
        let img = RgbaImage::new(3, 2).unwrap();
        let packed = pack_pixels(&img);
        assert_eq!(packed.len(), PIXEL_HEADER_LEN + 3 * 2 * 4);
        assert_eq!(&packed[0..4], PIXEL_MAGIC);
    }

    #[test]
    fn 像素包头可正确解析() {
        let img = RgbaImage::new(100, 50).unwrap();
        let packed = pack_pixels(&img);
        let (w, h, pixels) = decode_pixels(&packed).expect("应能解包");
        assert_eq!((w, h), (100, 50));
        assert_eq!(pixels.len(), 100 * 50 * 4);
    }

    #[test]
    fn 像素数据紧随包头之后() {
        let mut img = RgbaImage::new(2, 1).unwrap();
        img.data[0] = 255; // 首个像素的 R 通道
        let packed = pack_pixels(&img);
        let (_, _, pixels) = decode_pixels(&packed).unwrap();
        assert_eq!(pixels[0], 255, "包头后应紧接像素数据");
    }

    #[test]
    fn 打包解包往返像素完全一致() {
        let mut img = RgbaImage::new(8, 8).unwrap();
        for i in 0..img.data.len() {
            img.data[i] = (i % 256) as u8;
        }
        let packed = pack_pixels(&img);
        let (w, h, pixels) = decode_pixels(&packed).unwrap();
        assert_eq!((w, h), (8, 8));
        assert_eq!(pixels, &img.data[..], "往返后像素必须逐字节一致");
    }

    #[test]
    fn 魔数错误被拒绝() {
        let mut img = RgbaImage::new(2, 2).unwrap();
        img.data[0] = 1;
        let mut packed = pack_pixels(&img);
        packed[0] = b'X'; // 破坏魔数
        assert!(decode_pixels(&packed).is_err(), "魔数不匹配必须拒绝");
    }

    #[test]
    fn 数据长度不足被拒绝() {
        assert!(decode_pixels(&[1, 2, 3]).is_err());
        assert!(decode_pixels(&[]).is_err());
    }

    #[test]
    fn 尺寸与像素长度不匹配被拒绝() {
        // 手工构造：声明 100x100 但只给很少的像素
        let mut buf = PIXEL_MAGIC.to_vec();
        buf.extend_from_slice(&100u32.to_le_bytes());
        buf.extend_from_slice(&100u32.to_le_bytes());
        buf.push(0);
        buf.extend_from_slice(&[0u8; 10]);
        assert!(decode_pixels(&buf).is_err(), "长度不匹配必须拒绝");
    }

    #[test]
    fn 尺寸溢出不panic() {
        let r = RgbaImage::new(u32::MAX, u32::MAX);
        assert!(r.is_err());
    }

    // ─────────────── 动图协议 PVA1 ───────────────

    /// 造 n 帧假数据：每帧第 0 字节 = 帧序号，便于校验「帧没串位」
    fn fake_frames(from: usize, n: usize, w: u32, h: u32) -> Vec<(u32, Vec<u8>)> {
        (from..from + n)
            .map(|i| {
                let mut d = vec![0u8; (w as usize) * (h as usize) * 4];
                d[0] = i as u8;
                (100 + i as u32, d)
            })
            .collect()
    }

    /// 便捷打包：整段动画（start=0, total=帧数）
    fn pack_all(
        w: u32,
        h: u32,
        loop_count: Option<u16>,
        trunc: bool,
        frames: &[(u32, Vec<u8>)],
    ) -> Vec<u8> {
        pack_animation(AnimPack {
            width: w,
            height: h,
            loop_count,
            truncated: trunc,
            start_index: 0,
            total_count: frames.len() as u32,
            frames: frames.iter().map(|(d, p)| (*d, p.as_slice())).collect(),
        })
    }

    #[test]
    fn 动图包固定头为32字节且布局正确() {
        let frames = fake_frames(0, 3, 4, 2);
        let blob = pack_all(4, 2, Some(5), false, &frames);

        assert_eq!(&blob[0..4], ANIM_MAGIC);
        assert_eq!(u16::from_le_bytes([blob[4], blob[5]]), ANIM_VERSION);
        assert_eq!(u16::from_le_bytes([blob[6], blob[7]]), 0, "未截断时 flags 应为 0");
        assert_eq!(u32::from_le_bytes([blob[8], blob[9], blob[10], blob[11]]), 4);
        assert_eq!(u32::from_le_bytes([blob[12], blob[13], blob[14], blob[15]]), 2);
        assert_eq!(i32::from_le_bytes([blob[16], blob[17], blob[18], blob[19]]), 5);
        assert_eq!(u32::from_le_bytes([blob[20], blob[21], blob[22], blob[23]]), 3, "本块帧数");
        assert_eq!(u32::from_le_bytes([blob[24], blob[25], blob[26], blob[27]]), 0, "起始序号");
        assert_eq!(u32::from_le_bytes([blob[28], blob[29], blob[30], blob[31]]), 3, "总帧数");

        // 总长 = 固定头 + 延时表(3×4) + 像素(3×4×2×4)
        assert_eq!(blob.len(), ANIM_HEADER_FIXED + 3 * 4 + 3 * (4 * 2 * 4));
    }

    #[test]
    fn 动图打包解包往返一致() {
        let frames = fake_frames(0, 5, 4, 2);
        let blob = pack_all(4, 2, None, false, &frames);

        let (hdr, out) = decode_animation_blob(&blob).expect("应能解包");
        assert_eq!((hdr.width, hdr.height), (4, 2));
        assert_eq!(hdr.loop_count, -1, "None 应编码成 -1（无限循环）");
        assert_eq!((hdr.start_index, hdr.total_count), (0, 5));
        assert!(!hdr.truncated);
        assert_eq!(out.len(), 5);

        for (i, (delay, data)) in out.iter().enumerate() {
            assert_eq!(*delay, 100 + i as u32, "第 {i} 帧延时");
            assert_eq!(data.len(), 4 * 2 * 4, "第 {i} 帧长度");
            assert_eq!(data[0], i as u8, "第 {i} 帧内容串位了");
        }
    }

    /// **分块传输的核心**：第二块的帧号必须接在第一块之后，不能从 0 重来。
    #[test]
    fn 分块打包时起始序号与总帧数正确() {
        // 整段 10 帧，这里只打第 4..7 帧
        let all = fake_frames(0, 10, 4, 2);
        let slice = &all[4..7];
        let blob = pack_animation(AnimPack {
            width: 4,
            height: 2,
            loop_count: Some(2),
            truncated: false,
            start_index: 4,
            total_count: 10,
            frames: slice.iter().map(|(d, p)| (*d, p.as_slice())).collect(),
        });

        let (hdr, out) = decode_animation_blob(&blob).unwrap();
        assert_eq!(hdr.start_index, 4, "本块应标称从第 4 帧开始");
        assert_eq!(hdr.total_count, 10, "总帧数是整段的，不是本块的");
        assert_eq!(out.len(), 3, "本块只含 3 帧");
        // 帧内容确实是对应原动画的第 4/5/6 帧
        for (k, (delay, data)) in out.iter().enumerate() {
            assert_eq!(*delay, 100 + (4 + k) as u32, "第 {k} 个块的延时应对应原第 {} 帧", 4 + k);
            assert_eq!(data[0], (4 + k) as u8, "帧内容必须来自原动画第 {} 帧", 4 + k);
        }
    }

    #[test]
    fn 动图截断位被正确编码() {
        let frames = fake_frames(0, 1, 2, 2);
        let blob = pack_all(2, 2, Some(0), true, &frames);
        let (hdr, _) = decode_animation_blob(&blob).unwrap();
        assert!(hdr.truncated, "truncated=true 必须体现在 flags 上");
        assert_eq!(hdr.loop_count, 0, "有限循环 0 次应编码成 0（与 -1 区分开）");
    }

    #[test]
    fn 动图零帧包可解析() {
        let blob = pack_animation(AnimPack {
            width: 2,
            height: 2,
            loop_count: None,
            truncated: false,
            start_index: 3,
            total_count: 3,
            frames: Vec::new(),
        });
        let (hdr, out) = decode_animation_blob(&blob).unwrap();
        assert_eq!((hdr.width, hdr.height), (2, 2));
        assert_eq!(hdr.loop_count, -1);
        // 「从第 3 帧开始、共 3 帧、本块空」—— 尾部块的自然形态
        assert_eq!((hdr.start_index, hdr.total_count), (3, 3));
        assert!(out.is_empty());
        assert_eq!(blob.len(), ANIM_HEADER_FIXED, "零帧时只有固定头");
    }

    #[test]
    fn 动图包魔数与版本错误被拒绝() {
        let frames = fake_frames(0, 1, 2, 2);

        let mut bad_magic = pack_all(2, 2, None, false, &frames);
        bad_magic[0] = b'X';
        assert!(decode_animation_blob(&bad_magic).is_err(), "魔数不符必须拒绝");

        let mut bad_ver = pack_all(2, 2, None, false, &frames);
        bad_ver[4] = 99;
        assert!(decode_animation_blob(&bad_ver).is_err(), "版本不符必须拒绝");
    }

    #[test]
    fn 动图包长度不足被拒绝() {
        assert!(decode_animation_blob(&[]).is_err());
        let frames = fake_frames(0, 2, 4, 4);
        let mut blob = pack_all(4, 4, None, false, &frames);
        blob.truncate(blob.len() - 10); // 砍掉半帧像素
        assert!(decode_animation_blob(&blob).is_err(), "像素区缺字节必须拒绝");
    }

    #[test]
    fn 动图无base64膨胀体积() {
        // 250×250 × 299 帧的真实量级：裸字节应恰为 头 + 像素，不多一分
        let (w, h, n) = (250u32, 250u32, 299usize);
        let frames = fake_frames(0, n, w, h);
        let blob = pack_all(w, h, None, false, &frames);
        let expect = ANIM_HEADER_FIXED + n * 4 + n * (w as usize) * (h as usize) * 4;
        assert_eq!(blob.len(), expect);
    }
}
