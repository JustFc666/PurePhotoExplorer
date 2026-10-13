//! 图像写出（编码）。与 `bitmap` 解码方向相反。
//!
//! ## 为什么单独开一个模块
//! 解码后端按「能力」注册（`ImageDecoder` trait），而**能解码 ≠ 能编码**：
//! WebP / HEIC / RAW 我们只读不写。所以编码能力单独判定，
//! 不挂在解码后端上，避免出现「后端说 supports() 但写不了」的错配。
//!
//! ## 当前支持写回的格式
//! | 格式 | 编码器 | 说明 |
//! |---|---|---|
//! | JPEG | `jpeg-encoder` | quality 92；丢弃 alpha（JPEG 无 alpha 通道） |
//! | PNG  | `png`          | 保留 alpha；压缩等级 6 |
//! | BMP  | 手写           | 24bit 真彩，不压缩 |
//!
//! 其余格式一律返回 [`PvError::NotWritable`]，文案里带上格式名与原因，
//! 前端可直接展示（不做二次翻译）。

use std::path::Path;

use pv_core::error::{PvError, Result};
use pv_core::format::ImageFormat;

use super::io_err;
use crate::decoder::RgbaImage;
/// JPEG 编码质量。92 是「肉眼无损」与体积的常见平衡点：
/// 低于 85 天空/平滑渐变会出现可见色带，高于 95 体积增长快而画质提升不可见。
const JPEG_QUALITY: u8 = 92;

/// PNG 压缩等级（0-9）。6 是 zlib 默认值，速度与体积平衡最好。
const PNG_COMPRESSION: png::Compression = png::Compression::Default;

/// 该格式是否支持**覆盖写回原文件**。
///
/// ⚠️ 与 [`can_encode`] 是**两件事**，别混用：
/// - `can_encode` = 「能不能编码成这个格式」→ 决定**另存为**的选项；
/// - 本函数 = 「能不能用这个格式覆盖原文件」→ 决定**覆盖保存**能不能用。
///
/// RAW / WebP / HEIC 都能编码（另存为没问题），但**不能覆盖原文件**：
/// 覆盖意味着要把解码后的 8bit sRGB 像素写回，而 RAW 的像素来自
/// 「内嵌 JPEG 预览」—— 那不是传感器数据，覆盖会**永久降级画质**。
/// HEIF 则是根本没有编码器。
pub fn can_overwrite_original(format: ImageFormat) -> bool {
    matches!(
        format,
        ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::Bmp
    )
}

/// 该格式能否被编码成字节流（=能否作为「另存为」的目标）。
///
/// 当前与 [`can_overwrite_original`] 相同，但**语义不同、后续会分叉**：
/// 接了 DNG 写出处之后，RAW 会变成「能另存为 DNG、但仍不能覆盖原 RAW」。
/// 所以这里独立成函数，就是为了那时只改一处。
pub fn can_encode(format: ImageFormat) -> bool {
    can_overwrite_original(format)
}

/// 从文件扩展名反推目标编码格式（供「另存为」按用户选的扩展名决定编码器）。
///
/// 认不出的扩展名返回 `None`，由调用方给出明确错误 ——
/// 静默退回原格式是最糟的：用户选了 `.png` 却拿到 JPEG 内容。
pub fn format_from_extension(ext: &str) -> Option<ImageFormat> {
    match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => Some(ImageFormat::Jpeg),
        "png" => Some(ImageFormat::Png),
        "bmp" => Some(ImageFormat::Bmp),
        _ => None,
    }
}

/// 不支持写回时的说明文案。
///
/// 文案只在这里写一处；前端原样展示，不再拼词。
pub fn not_writable_reason(format: ImageFormat) -> String {
    if format.is_raw() {
        format!(
            "{} 是相机 RAW 格式，覆盖原文件会破坏相机原始数据，因此只提供「另存为」。",
            format.display_name()
        )
    } else {
        format!(
            "暂支持覆盖保存 JPEG / PNG / BMP；{} 可用「另存为」导出。",
            format.display_name()
        )
    }
}

/// 把 RGBA 像素编码为指定格式的字节流。
///
/// 不落盘 —— 让调用方能自由决定写哪里（原文件 / 临时文件 / 内存）。
pub fn encode_rgba(img: &RgbaImage, format: ImageFormat) -> Result<Vec<u8>> {
    encode_rgba_with_exif(img, format, None)
}

/// 同上，但把 `exif`（**裸 TIFF 字节**）一并写进输出。
///
/// ## 为什么要单独一个函数而不是给 `encode_rgba` 加参数
/// 图像编码与元数据是两个独立关注点：绝大多数调用（缩略图、临时导出）
/// 不需要元数据，让它们继续用三参数的旧签名，读起来更清楚。
///
/// ## exif 传进来之前必须先对齐
/// 旋转是**改像素**实现的，所以源 EXIF 里的 `Orientation` 会与新像素矛盾。
/// 调用方要先过一遍 `pv_meta::align_exif_with_pixels`，否则看图器会二次旋转。
/// 这里只负责"塞进去"，不检查内容。
///
/// BMP 忽略 `exif`：BMP 没有标准的元数据段，硬塞会把文件写坏。
pub fn encode_rgba_with_exif(
    img: &RgbaImage,
    format: ImageFormat,
    exif: Option<&[u8]>,
) -> Result<Vec<u8>> {
    match format {
        ImageFormat::Jpeg => encode_jpeg(img, exif),
        ImageFormat::Png => encode_png(img, exif),
        ImageFormat::Bmp => encode_bmp(img),
        other => Err(PvError::NotWritable(not_writable_reason(other))),
    }
}

/// 编码并写入 `path`（覆盖）。
///
/// **先写同目录临时文件再原子替换** —— 直接 `File::create` 截断原文件后，
/// 一旦编码中途失败，用户的原图就变成半截废文件了。
/// 同目录是为了保证 `rename` 是同一文件系统内的原子操作（跨盘会退化成复制）。
///
/// 成功后会把临时文件重命名为目标名；失败则清理临时文件并保留原文件不动。
pub fn write_rgba_atomic(img: &RgbaImage, format: ImageFormat, path: &Path) -> Result<()> {
    write_rgba_atomic_with_exif(img, format, path, None)
}

/// 同上，但带上 EXIF（见 [`encode_rgba_with_exif`] 对 `exif` 的要求）。
pub fn write_rgba_atomic_with_exif(
    img: &RgbaImage,
    format: ImageFormat,
    path: &Path,
    exif: Option<&[u8]>,
) -> Result<()> {
    let bytes = encode_rgba_with_exif(img, format, exif)?;

    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "image".into());
    // 加进程 id 与纳秒时间戳，避免多实例/并发保存撞名
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let tmp = dir.join(format!(".{stem}.pv-tmp-{}-{nanos}", std::process::id()));

    // 写临时文件（用 write_all + sync_all，确保数据真的落盘再改名）
    std::fs::write(&tmp, &bytes).map_err(io_err(&tmp))?;
    if let Ok(f) = std::fs::File::open(&tmp) {
        // sync_all 失败不算致命（部分文件系统不支持），忽略
        let _ = f.sync_all();
    }

    // 原子替换
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(match e.kind() {
            std::io::ErrorKind::PermissionDenied => PvError::PermissionDenied(path.to_path_buf()),
            // Windows 上文件被其他程序占用时 rename 会失败，给出可操作提示
            _ if is_sharing_violation(&e) => PvError::Encode(format!(
                "文件被其他程序占用，无法写入。请先关闭正在使用「{}」的程序后重试。",
                path.file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            )),
            _ => PvError::Io {
                path: path.to_path_buf(),
                source: e,
            },
        });
    }
    Ok(())
}

/// Windows 的错误码 32 = ERROR_SHARING_VIOLATION（文件被占用）。
fn is_sharing_violation(e: &std::io::Error) -> bool {
    e.raw_os_error() == Some(32)
}

// ─────────────────────────── 各格式编码器 ───────────────────────────

fn encode_jpeg(img: &RgbaImage, exif: Option<&[u8]>) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut enc = jpeg_encoder::Encoder::new(&mut out, JPEG_QUALITY);
    // 基线 JPEG 不支持 4 通道；不设这个会直接报错
    enc.set_progressive(false);
    enc.encode(
        &img.data,
        img.width as u16,
        img.height as u16,
        jpeg_encoder::ColorType::Rgba,
    )
    .map_err(|e| PvError::Encode(format!("JPEG 编码失败: {e}")))?;

    if let Some(tiff) = exif {
        if !insert_jpeg_exif(&mut out, tiff) {
            // 图片保存优先于元数据：写不进去只是少了拍摄参数，不该让用户存不了图
            tracing::warn!(
                target: "photovault::encode",
                bytes = tiff.len(),
                "EXIF 超过 JPEG 单段上限，已跳过（图片本身正常写出）"
            );
        }
    }
    Ok(out)
}

/// 把 EXIF 作为 `APP1` 段插到 JPEG 的 `SOI` 之后；返回是否真的写入了。
///
/// ## 为什么插在 SOI 正后方
/// Exif 规范就是这么要求的（APP1 紧接 SOI）。此处的 JPEG 由 `jpeg-encoder`
/// 产出，它会在前面写 APP0(JFIF) —— 把 APP1 排在 APP0 之前仍然合法
/// （APP 段的顺序不作强制），且与相机出片、ExifTool 的行为一致。
///
/// ## 太长时返回 `false` 而不是报错
/// APP1 的长度字段是 **u16**（含自身 2 字节）→ 载荷最多 65533 字节。
/// 硬写会让长度字段溢出、整个 JPEG 都解析不了；而**图片本身是用户真正要的东西**，
/// 不能因为元数据塞不下就拒绝保存。所以这里放弃写入，由调用方记日志。
/// （正常流程很难触发：源文件里那条占体积的 IFD1 缩略图在
/// `pv_meta::align_exif_with_pixels` 里已被切断。）
fn insert_jpeg_exif(jpeg: &mut Vec<u8>, tiff: &[u8]) -> bool {
    const PREFIX: &[u8] = b"Exif\0\0";
    let payload = PREFIX.len() + tiff.len();
    if payload + 2 > u16::MAX as usize || !jpeg.starts_with(&[0xFF, 0xD8]) {
        return false;
    }

    let mut seg = Vec::with_capacity(payload + 4);
    seg.extend_from_slice(&[0xFF, 0xE1]);
    seg.extend_from_slice(&((payload + 2) as u16).to_be_bytes());
    seg.extend_from_slice(PREFIX);
    seg.extend_from_slice(tiff);

    // 手工重组而不是 `Vec::splice`：splice 的替换发生在迭代器被丢弃时，
    // 写成一句语句依赖 drop 时机，不够直白。
    let mut merged = Vec::with_capacity(jpeg.len() + seg.len());
    merged.extend_from_slice(&jpeg[..2]); // SOI
    merged.extend_from_slice(&seg);
    merged.extend_from_slice(&jpeg[2..]);
    *jpeg = merged;
    true
}

fn encode_png(img: &RgbaImage, exif: Option<&[u8]>) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, img.width, img.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(PNG_COMPRESSION);
        let mut writer = enc
            .write_header()
            .map_err(|e| PvError::Encode(format!("PNG 头写入失败: {e}")))?;

        // eXIf 必须在 IDAT 之前（PNG 规范），内容是**裸 TIFF**（没有 Exif\0\0 前缀）
        if let Some(tiff) = exif {
            writer
                .write_chunk(png::chunk::eXIf, tiff)
                .map_err(|e| PvError::Encode(format!("PNG eXIf 段写入失败: {e}")))?;
        }

        writer
            .write_image_data(&img.data)
            .map_err(|e| PvError::Encode(format!("PNG 编码失败: {e}")))?;
    }
    Ok(out)
}

/// BMP：手写 BITMAPFILEHEADER + BITMAPINFOHEADER + 24bit BGR 行（自底向上）。
///
/// 为什么手写：`image` crate 为了 BMP 一个格式引入太多东西，
/// 而 BMP 头结构固定、不到 60 行就能写对，不值得加依赖。
fn encode_bmp(img: &RgbaImage) -> Result<Vec<u8>> {
    let w = img.width;
    let h = img.height;

    // 每行按 4 字节对齐
    let row_raw = w as usize * 3;
    let row_padded = row_raw.div_ceil(4) * 4;
    let pixel_bytes = row_padded
        .checked_mul(h as usize)
        .ok_or_else(|| PvError::Encode("图像尺寸过大，BMP 行数据溢出".into()))?;
    let file_size = 14 + 40 + pixel_bytes;

    let mut out = Vec::with_capacity(file_size);

    // BITMAPFILEHEADER
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(file_size as u32).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // 保留
    out.extend_from_slice(&0u16.to_le_bytes()); // 保留
    out.extend_from_slice(&54u32.to_le_bytes()); // 像素数据偏移

    // BITMAPINFOHEADER
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes()); // 正数 = 自底向上
    out.extend_from_slice(&1u16.to_le_bytes()); // planes
    out.extend_from_slice(&24u16.to_le_bytes()); // bpp
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&(pixel_bytes as u32).to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes()); // 72 DPI
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // 调色板数
    out.extend_from_slice(&0u32.to_le_bytes()); // 重要颜色数

    // 像素：BMP 是自底向上 + BGR 序
    let pad = [0u8; 3];
    for y in (0..h).rev() {
        let row_start = y as usize * w as usize * 4;
        for x in 0..w as usize {
            let i = row_start + x * 4;
            out.push(img.data[i + 2]); // B
            out.push(img.data[i + 1]); // G
            out.push(img.data[i]); // R
        }
        out.extend_from_slice(&pad[..row_padded - row_raw]);
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一张有颜色梯度的测试图（避免纯色导致编码器压出 0 字节）。
    fn gradient(w: u32, h: u32) -> RgbaImage {
        let mut img = RgbaImage::new(w, h).unwrap();
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                img.data[i] = (x * 255 / w.max(1)) as u8;
                img.data[i + 1] = (y * 255 / h.max(1)) as u8;
                img.data[i + 2] = 128;
                img.data[i + 3] = 255;
            }
        }
        img
    }

    #[test]
    fn 可写格式判定正确() {
        assert!(can_encode(ImageFormat::Jpeg));
        assert!(can_encode(ImageFormat::Png));
        assert!(can_encode(ImageFormat::Bmp));
        // RAW 绝不能写
        assert!(!can_encode(ImageFormat::SonyArw));
        assert!(!can_encode(ImageFormat::CanonCr3));
        assert!(!can_encode(ImageFormat::Heif));
    }

    #[test]
    fn JPEG编码产生合法文件头() {
        let img = gradient(32, 16);
        let bytes = encode_jpeg(&img, None).unwrap();
        assert!(bytes.len() > 100, "编码结果过小: {}", bytes.len());
        assert_eq!(&bytes[..2], &[0xFF, 0xD8], "JPEG 应以 SOI 开头");
        assert_eq!(&bytes[bytes.len() - 2..], &[0xFF, 0xD9], "JPEG 应以 EOI 结尾");
    }

    #[test]
    fn PNG编码产生合法文件头() {
        let img = gradient(32, 16);
        let bytes = encode_png(&img, None).unwrap();
        assert!(bytes.len() > 50);
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "PNG 魔数");
    }

    /// 造一段最小的合法 TIFF（只有 IFD0 + 一个 Orientation 条目）。
    fn tiny_tiff() -> Vec<u8> {
        let mut t = Vec::new();
        t.extend_from_slice(b"II");
        t.extend_from_slice(&42u16.to_le_bytes());
        t.extend_from_slice(&8u32.to_le_bytes()); // IFD0 偏移
        t.extend_from_slice(&1u16.to_le_bytes()); // 1 个条目
        t.extend_from_slice(&0x0112u16.to_le_bytes()); // Orientation
        t.extend_from_slice(&3u16.to_le_bytes()); // SHORT
        t.extend_from_slice(&1u32.to_le_bytes()); // count
        t.extend_from_slice(&1u16.to_le_bytes()); // 值 = 1
        t.extend_from_slice(&0u16.to_le_bytes()); // 补齐
        t.extend_from_slice(&0u32.to_le_bytes()); // next IFD = 无
        t
    }

    /// 带 EXIF 的 JPEG：SOI 之后必须紧跟一个格式正确的 APP1 段。
    ///
    /// 这段字节是**外部看图器唯一能读到拍摄参数的地方**，长度字段写错
    /// 会让整个 JPEG 解析失败，所以逐字段核对。
    #[test]
    fn JPEG带EXIF时APP1段结构正确() {
        let img = gradient(16, 16);
        let tiff = tiny_tiff();
        let bytes = encode_jpeg(&img, Some(&tiff)).unwrap();

        assert_eq!(&bytes[..2], &[0xFF, 0xD8], "SOI 仍在最前");
        assert_eq!(&bytes[2..4], &[0xFF, 0xE1], "EXIF 应作为 APP1 紧跟 SOI");
        let seg_len = u16::from_be_bytes([bytes[4], bytes[5]]) as usize;
        assert_eq!(
            seg_len,
            2 + 6 + tiff.len(),
            "段长 = 自身 2 字节 + Exif 前缀 + TIFF"
        );
        assert_eq!(&bytes[6..12], b"Exif\0\0", "APP1 载荷以 Exif\\0\\0 开头");
        assert_eq!(&bytes[12..12 + tiff.len()], &tiff[..], "TIFF 原样搬入");
        // 段之后必须还是原来的 JPEG 内容（APP0 或 SOF）
        assert_eq!(bytes[12 + tiff.len()], 0xFF, "段后应是下一个标记");
    }

    /// 不带 EXIF 时不允许凭空多出段（否则老调用方拿到的文件会变样）。
    #[test]
    fn JPEG不带EXIF时无APP1段() {
        let img = gradient(16, 16);
        let bytes = encode_jpeg(&img, None).unwrap();
        assert_ne!(&bytes[2..4], &[0xFF, 0xE1], "不该有无中生有的 APP1");
    }

    /// 超大 EXIF 必须**放弃写入**而不是硬塞 —— 而且**图片本身仍要正常写出**。
    ///
    /// u16 长度字段溢出会让整个 JPEG 不可解析；但用户的图不能被元数据拖累。
    #[test]
    fn EXIF过大时跳过但仍写出合法JPEG() {
        let img = gradient(16, 16);
        let huge = vec![0u8; 70_000];
        let bytes = encode_jpeg(&img, Some(&huge)).unwrap();

        assert_eq!(&bytes[..2], &[0xFF, 0xD8], "仍应是合法 JPEG");
        assert_ne!(&bytes[2..4], &[0xFF, 0xE1], "不该写入超长的 APP1 段");
        // 能正常解码，说明没有把文件写坏
        let mut dec = zune_jpeg::JpegDecoder::new(std::io::Cursor::new(&bytes));
        dec.decode().expect("超长 EXIF 被跳过后，图片本身应完好");
    }

    /// PNG 的 EXIF 走 `eXIf` chunk，且必须在 IDAT 之前。
    #[test]
    fn PNG带EXIF时写入eXIf块() {
        let img = gradient(16, 16);
        let tiff = tiny_tiff();
        let bytes = encode_png(&img, Some(&tiff)).unwrap();

        let at = bytes
            .windows(4)
            .position(|w| w == b"eXIf")
            .expect("应存在 eXIf chunk");
        // chunk 结构：[4 字节长度][4 字节类型][数据][4 字节 CRC]
        let len = u32::from_be_bytes(bytes[at - 4..at].try_into().unwrap()) as usize;
        assert_eq!(len, tiff.len(), "eXIf 数据长度应等于 TIFF 长度");
        assert_eq!(&bytes[at + 4..at + 4 + len], &tiff[..], "内容是裸 TIFF（无 Exif 前缀）");

        let idat = bytes.windows(4).position(|w| w == b"IDAT").expect("应有 IDAT");
        assert!(at < idat, "eXIf 必须在 IDAT 之前");
    }

    /// BMP 没有标准的元数据段：传了 EXIF 也只能忽略（不能写坏文件）。
    #[test]
    fn BMP忽略EXIF但不报错() {
        let img = gradient(4, 4);
        let with = encode_rgba_with_exif(&img, ImageFormat::Bmp, Some(&tiny_tiff())).unwrap();
        let without = encode_rgba(&img, ImageFormat::Bmp).unwrap();
        assert_eq!(with, without, "BMP 输出应与不带 EXIF 时完全一致");
    }

    /// 带 EXIF 的原子写入：文件要能被正常读回（段插入不能破坏结构）。
    #[test]
    fn 带EXIF写入JPEG后仍可解码() {
        // ⚠️ 每个测试**各自的目录**：共用目录时，邻居正在写的 `pv-tmp*`
        // 会被本测试的「无残留临时文件」断言看见 → 偶发失败（2026-10-11 踩到）。
        let dir = std::env::temp_dir().join("pv-encode-test").join("原子写入");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("with-exif.jpg");
        let _ = std::fs::remove_file(&path);

        let img = gradient(24, 12);
        write_rgba_atomic_with_exif(&img, ImageFormat::Jpeg, &path, Some(&tiny_tiff())).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..2], &[0xFF, 0xD8]);
        // 从字节里再解出 EXIF 的那段，确认能取到 TIFF 头
        let at = bytes.windows(6).position(|w| w == b"Exif\0\0").expect("应有 EXIF");
        assert_eq!(&bytes[at + 6..at + 6 + 8][..2], b"II");

        // 真正解码一次，确认段插入没破坏 JPEG 结构（尺寸应保持）
        let mut dec = zune_jpeg::JpegDecoder::new(std::io::Cursor::new(&bytes));
        dec.decode().expect("带 EXIF 的 JPEG 应仍可解码");
        let (w, h) = dec.dimensions().expect("应能取到尺寸信息");
        assert_eq!((w as u32, h as u32), (24, 12));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn BMP编码头字段正确() {
        let img = gradient(4, 3);
        let bytes = encode_bmp(&img).unwrap();
        assert_eq!(&bytes[..2], b"BM");
        // 宽度 / 高度（i32 LE，偏移 18 / 22）
        let w = i32::from_le_bytes(bytes[18..22].try_into().unwrap());
        let h = i32::from_le_bytes(bytes[22..26].try_into().unwrap());
        assert_eq!((w, h), (4, 3));
        // bpp
        let bpp = u16::from_le_bytes(bytes[28..30].try_into().unwrap());
        assert_eq!(bpp, 24);
        // 行按 4 字节对齐：4*3=12 → 已是 4 的倍数
        assert_eq!(bytes.len(), 54 + 12 * 3);
    }

    #[test]
    fn BMP行不足4字节时补零对齐() {
        // 宽 3 → 每行 9 字节 → 补齐到 12
        let img = gradient(3, 2);
        let bytes = encode_bmp(&img).unwrap();
        assert_eq!(bytes.len(), 54 + 12 * 2);
    }

    #[test]
    fn RAW不能作为编码目标() {
        let img = gradient(4, 4);
        let err = encode_rgba(&img, ImageFormat::CanonCr3).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("RAW"), "应说明是 RAW: {msg}");
        // ⛔ 不能说「只读不写」—— 2026-10-10起 RAW 有出口了（另存为 JPEG/PNG），
        // 说成只读会让用户以为整个保存功能对 RAW 不可用。
        assert!(msg.contains("另存为"), "应指出有另存为这条路: {msg}");
        assert!(!msg.contains("只读不写"), "措辞过时了: {msg}");
        assert!(err.is_user_visible(), "该错误应可直接展示给用户");
    }

    #[test]
    fn 未知格式编码返回可操作提示() {
        let img = gradient(4, 4);
        let err = encode_rgba(&img, ImageFormat::Heif).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("JPEG / PNG / BMP"), "应告知支持哪些: {msg}");
        assert!(msg.contains("另存为"), "应给出路而不是只说不行: {msg}");
    }

    /// **覆盖原文件**与**能编码**是两件事，必须分别钉住。
    ///
    /// 2026-10-10 拆成`can_overwrite_original` / `can_encode` 两个函数：
    /// 今天值相同，但接了 DNG 写出处之后 RAW 会变成
    /// 「能另存为 DNG、仍不能覆盖原 RAW」。这条测试提醒那时**别把两者混用**。
    #[test]
    fn 覆盖与编码能力当前一致但语义独立() {
        for f in [ImageFormat::Jpeg, ImageFormat::Png, ImageFormat::Bmp] {
            assert!(can_overwrite_original(f));
            assert!(can_encode(f));
        }
        for f in [
            ImageFormat::CanonCr3,
            ImageFormat::NikonNef,
            ImageFormat::WebP,
            ImageFormat::Heif,
            ImageFormat::Gif,
            ImageFormat::Psd,
        ] {
            assert!(!can_overwrite_original(f), "{} 不该允许覆盖原文件", f.display_name());
        }
    }

    /// 扩展名 → 编码格式。认不出的必须返回 `None`，
    /// **绝不能静默退回某个默认格式** —— 用户选 .webp 却拿到 JPEG 内容，
    /// 看起来就像文件坏了。
    #[test]
    fn 扩展名映射只认三种且大小写不敏感() {
        assert_eq!(format_from_extension("jpg"), Some(ImageFormat::Jpeg));
        assert_eq!(format_from_extension("jpeg"), Some(ImageFormat::Jpeg));
        assert_eq!(format_from_extension("JPG"), Some(ImageFormat::Jpeg));
        assert_eq!(format_from_extension("png"), Some(ImageFormat::Png));
        assert_eq!(format_from_extension("bmp"), Some(ImageFormat::Bmp));
        for bad in ["webp", "heic", "tiff", "cr3", "", "jpg.exe"] {
            assert_eq!(format_from_extension(bad), None, "{bad} 不该被接受");
        }
    }

    #[test]
    fn 原子写入成功并可读回() {
        // ⚠️ 每个测试**各自的目录**：共用目录时，邻居正在写的 `pv-tmp*`
        // 会被本测试的「无残留临时文件」断言看见 → 偶发失败（2026-10-11 踩到）。
        let dir = std::env::temp_dir().join("pv-encode-test").join("带EXIF写入");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("out.png");
        let _ = std::fs::remove_file(&path);

        let img = gradient(16, 8);
        write_rgba_atomic(&img, ImageFormat::Png, &path).unwrap();

        assert!(path.is_file(), "文件应存在");
        // 不应残留临时文件
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("pv-tmp"))
            .collect();
        assert!(leftovers.is_empty(), "残留临时文件: {leftovers:?}");

        // 内容确实是 PNG
        let head = std::fs::read(&path).unwrap();
        assert_eq!(&head[..8], b"\x89PNG\r\n\x1a\n");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 写入不支持的格式时保留原文件() {
        // ⚠️ 每个测试**各自的目录**：共用目录时，邻居正在写的 `pv-tmp*`
        // 会被本测试的「无残留临时文件」断言看见 → 偶发失败（2026-10-11 踩到）。
        let dir = std::env::temp_dir().join("pv-encode-test").join("不支持格式");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("keep.bin");
        std::fs::write(&path, b"ORIGINAL").unwrap();

        let img = gradient(4, 4);
        // HEIF 不可写 → 应在编码阶段就失败，不碰原文件
        assert!(write_rgba_atomic(&img, ImageFormat::Heif, &path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"ORIGINAL", "原文件须保持不变");

        let _ = std::fs::remove_file(&path);
    }
}
