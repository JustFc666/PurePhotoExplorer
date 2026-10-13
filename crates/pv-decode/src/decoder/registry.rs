//! 解码后端注册表：按格式路由 + RAW 两阶段管线挂载。

use std::path::Path;

use pv_core::error::{PvError, Result};
use pv_core::format::{classify_non_image, ImageFormat};
use pv_core::raw::RawPipeline;

use super::info::ImageInfo;
use super::rgba::RgbaImage;
use super::target::DecodeTarget;
use super::traits::ImageDecoder;

/// 解码后端注册表。
///
/// 按格式路由到对应后端；后端不可用时降级而非报错。
pub struct DecoderRegistry {
    backends: Vec<Box<dyn ImageDecoder>>,
    /// RAW 两阶段管线（可选，S3 才接入）
    raw_pipeline: Option<Box<dyn RawPipeline>>,
}

impl DecoderRegistry {
    pub fn new() -> Self {
        Self {
            backends: Vec::new(),
            raw_pipeline: None,
        }
    }

    pub fn register(&mut self, backend: Box<dyn ImageDecoder>) {
        tracing::info!(backend = backend.backend_name(), "注册解码后端");
        self.backends.push(backend);
    }

    pub fn set_raw_pipeline(&mut self, pipeline: Box<dyn RawPipeline>) {
        self.raw_pipeline = Some(pipeline);
    }

    /// 选取支持该格式的后端
    fn backend_for(&self, format: ImageFormat) -> Option<&dyn ImageDecoder> {
        self.backends
            .iter()
            .find(|b| b.supports(format))
            .map(|b| b.as_ref())
    }

    /// 探测（RAW 优先走 RawPipeline，因为它有更完整的 EXIF 解析）
    pub fn probe(&self, path: &Path) -> Result<ImageInfo> {
        // 先按文件头判定格式：只有 RAW 系列才需要交给 RawPipeline。
        //
        // 注意不能像早期实现那样「无条件先试 raw.probe()」——
        // 那会让每张普通 JPEG 都多付一次 RAW 探测的成本（读文件 + 解析 EXIF），
        // 而且 RAW 探测失败的错误会掩盖真实的格式问题。
        let head = read_head(path, 512)?;
        let probe = pv_core::format::probe_header(&head);
        let format = probe.format;

        if format.is_raw() {
            if let Some(raw) = self.raw_pipeline.as_ref() {
                // TIFF 容器特例：常规 TIFF 与多数相机 RAW（NEF/ARW/DNG…）
                // 共用 TIFF 容器，`detect()` 仅靠魔数无法区分，一律返回
                // `OtherRaw`。**真实 RAW 有内嵌预览，RAW 管线会成功**；
                // 常规 TIFF 没有内嵌预览，RAW 管线会失败 —— 此时降级到
                // TIFF 解码器，而不是把一张普通 TIFF 误当成坏 RAW。
                if is_tiff_container(&head) {
                    match raw.probe(path) {
                        Ok(m) if m.has_embedded_preview => {
                            return Ok(ImageInfo {
                                format,
                                width: m.width,
                                height: m.height,
                                exif: None,
                                raw: Some(m),
                                color_space: None,
                            });
                        }
                        // 无内嵌预览（常规 TIFF）→ 当普通 TIFF 处理
                        _ => return crate::bitmap::probe_tiff(path),
                    }
                }
                let m = raw.probe(path)?;
                return Ok(ImageInfo {
                    // 保留魔数判定出的具体格式（CR3/RAF/OtherRaw…），
                    // 便于上层区分展示；早期实现一律写 OtherRaw 会丢失这个信息。
                    format,
                    width: m.width,
                    height: m.height,
                    // RAW 的拍摄参数已经在 raw 字段里（结构化），
                    // 这里不再重复塞原始文本
                    exif: None,
                    raw: Some(m),
                    color_space: None,
                });
            }
            return Err(PvError::UnsupportedFormat("RAW 解码器未接入".into()));
        }

        // 常规格式：路由到对应后端
        let backend = self.backend_for(format).ok_or_else(|| {
            format_unsupported_error(format, &head)
        })?;
        backend.probe(path)
    }

    /// 解码
    pub fn decode(&self, path: &Path, target: DecodeTarget) -> Result<RgbaImage> {
        let head = read_head(path, 512)?;
        let probe = pv_core::format::probe_header(&head);
        let format = probe.format;

        // RAW 走两阶段管线
        if format.is_raw() {
            let raw = self.raw_pipeline.as_ref().ok_or_else(|| {
                PvError::UnsupportedFormat(
                    "RAW 解码器未接入（需 MSVC 工具链，见 M0 决策项）".into(),
                )
            })?;

            // TIFF 容器特例：常规 TIFF（非相机 RAW）没有内嵌预览，
            // 两阶段管线会失败 → 降级到 TIFF 解码器（见 probe() 的同款逻辑）。
            if is_tiff_container(&head) {
                match pv_core::raw::extract_preview_with_fallback(raw.as_ref(), path) {
                    Ok(preview) => return decode_encoded_preview(&preview, target),
                    // 无可用预览 → 当普通 TIFF 解码
                    Err(_) => return crate::bitmap::decode_tiff(path, target),
                }
            }

            let preview = pv_core::raw::extract_preview_with_fallback(raw.as_ref(), path)?;
            return decode_encoded_preview(&preview, target);
        }

        let backend = self.backend_for(format).ok_or_else(|| {
            format_unsupported_error(format, &head)
        })?;
        backend.decode(path, target)
    }
}

/// 构造「格式不支持」的错误，尽量给出可操作的说明。
///
/// 关键改进：当文件根本不是图像（Git LFS 指针、纯文本、空文件）时，
/// 不再笼统报「未知格式」—— 那种提示让用户无从下手。
/// 这里换成具体原因 + 该怎么做。
fn format_unsupported_error(format: ImageFormat, head: &[u8]) -> PvError {
    if format == ImageFormat::Unknown {
        if let Some(kind) = classify_non_image(head) {
            return PvError::UnsupportedFormat(kind.hint().to_string());
        }
    }
    PvError::UnsupportedFormat(format!("暂不支持该格式：{}", format.display_name()))
}

impl Default for DecoderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// 判断文件头是否为 TIFF 容器（小端 "II*\0" 或大端 "MM\0*"）。
///
/// 常规 TIFF 与多数相机 RAW（NEF/ARW/DNG…）共用 TIFF 容器，
/// 仅靠魔数无法区分，因此 `detect()` 一律返回 `OtherRaw`。
/// 这里只在「RAW 管线失败」时作为降级钩子，把常规 TIFF 路由到
/// TIFF 解码器，而不影响真实 RAW 的优先路径。
fn is_tiff_container(head: &[u8]) -> bool {
    head.len() >= 4
        && (head.starts_with(&[0x49, 0x49, 0x2A, 0x00]) // II*\0
            || head.starts_with(&[0x4D, 0x4D, 0x00, 0x2A])) // MM\0*
}

/// 读取文件头（不读整个文件）
fn read_head(path: &Path, n: usize) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => PvError::NotFound(path.to_path_buf()),
        std::io::ErrorKind::PermissionDenied => PvError::PermissionDenied(path.to_path_buf()),
        _ => PvError::Io {
            path: path.to_path_buf(),
            source: e,
        },
    })?;
    let mut buf = vec![0u8; n];
    let read = f.read(&mut buf).map_err(|e| PvError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    buf.truncate(read);
    Ok(buf)
}

/// 把 RAW 内嵌预览转成 RGBA。
///
/// 内嵌 JPEG 是**相机原样写入的字节**，直接喂给 JPEG 解码器即可 ——
/// Stage 1 的价值就在这里：零解析成本，且拿到的是厂商已渲染好的画面。
fn decode_encoded_preview(
    preview: &pv_core::raw::EncodedPreview,
    target: DecodeTarget,
) -> Result<RgbaImage> {
    match preview {
        pv_core::raw::EncodedPreview::Jpeg(bytes) => {
            crate::bitmap::decode_jpeg_from_bytes(bytes, target).map_err(|e| {
                PvError::Decode(format!("RAW 内嵌预览（JPEG）解码失败：{e}"))
            })
        }
        pv_core::raw::EncodedPreview::Rgba {
            width,
            height,
            data,
        } => {
            if data.len() != (*width as usize) * (*height as usize) * 4 {
                return Err(PvError::Corrupt("RAW 预览像素长度与尺寸不匹配".into()));
            }
            Ok(RgbaImage {
                width: *width,
                height: *height,
                data: data.clone(),
            })
        }
        pv_core::raw::EncodedPreview::Unknown(_) => Err(PvError::UnsupportedFormat(
            "内嵌预览为未知格式，无法解码".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 空注册表不崩溃() {
        let reg = DecoderRegistry::new();
        assert!(reg.backend_for(ImageFormat::Jpeg).is_none());
    }

    #[test]
    fn 文件不存在返回NotFound() {
        let r = read_head(Path::new("Z:/绝对不存在的路径/x.jpg"), 32);
        assert!(matches!(r, Err(PvError::NotFound(_))));
    }

    /// Git LFS 指针文件应给出「可操作」的错误，而不是笼统的「未知格式」。
    ///
    /// 背景（2026-10-07）：`test-image/` 下的 dng/cr2/nef/arw 样本
    /// 全是 132 字节的 LFS 指针，用户看到「未知格式」完全不知道发生了什么。
    #[test]
    fn LFS指针文件报错说明原因() {
        let dir = std::env::temp_dir().join("pv_lfs_err_test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("fake.dng");
        std::fs::write(
            &p,
            b"version https://git-lfs.github.com/spec/v1\noid sha256:abc\nsize 6372698\n",
        )
        .unwrap();

        let reg = DecoderRegistry::new();
        let err = reg.probe(&p).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Git LFS"), "错误应点明 LFS 指针，实际：{msg}");
        assert!(msg.contains("git lfs pull"), "应给出解决办法，实际：{msg}");

        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn 空文件报错说明是空文件() {
        let dir = std::env::temp_dir().join("pv_empty_err_test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("empty.png");
        std::fs::write(&p, b"").unwrap();

        let reg = DecoderRegistry::new();
        let msg = reg.probe(&p).unwrap_err().to_string();
        assert!(msg.contains("0 字节"), "应点明空文件，实际：{msg}");

        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn 纯文本文件报错说明是文本() {
        let dir = std::env::temp_dir().join("pv_text_err_test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("notes.jpg");
        std::fs::write(&p, b"hello world, this is not an image at all\n".repeat(3)).unwrap();

        let reg = DecoderRegistry::new();
        let msg = reg.probe(&p).unwrap_err().to_string();
        assert!(msg.contains("文本"), "应点明是文本，实际：{msg}");

        let _ = std::fs::remove_file(&p);
    }
}
