//! GIF 解码（含**动图多帧合成**）。
//!
//! ## 为什么不是「读第一帧就完事」
//! GIF 的帧是**增量**的：每帧只携带自己在逻辑屏幕上的一个子矩形，
//! 还可能带处置方式（dispose）指示「贴完之后要不要把我这块擦掉 /
//! 还原成上一帧的样子」。所以要看对动图，必须维护一张画布、
//! 逐帧「贴上去 → 快照一份 → 按 dispose 清理」，不能只解一帧。
//!
//! 合成算法与 `gif` crate 官方示例一致：
//! 1. 若本帧 dispose 为 `Previous`，先把当前画布备份（下一帧要还原回来）；
//! 2. 把 `frame.buffer` 贴到 `(left, top)`；
//! 3. 把**贴完的状态**快照成一个输出帧；
//! 4. 最后按 dispose 清理画布：`Background` = 擦成透明，`Previous` = 还原备份。
//!
//! ## 内存保护
//! 帧数上限与总字节上限在 [`super::anim`]（GIF 与动画 WebP 共用），
//! 任一触发就截断（`truncated = true`），由上层提示用户，而不是让进程 OOM。

use std::path::Path;

use pv_core::error::{PvError, Result};
use pv_core::format::ImageFormat;

use super::anim::{
    Animation, AnimationFrame, DEFAULT_DELAY_MS, MAX_FRAMES, MAX_TOTAL_BYTES,
};
use super::io_err;
use super::resize::resize_rgba;
use crate::decoder::{fit_within, DecodeTarget, ImageInfo, RgbaImage};

/// 探测 GIF 尺寸（只读逻辑屏幕头，不解码帧）
pub(super) fn probe_gif(path: &Path) -> Result<ImageInfo> {
    let f = std::fs::File::open(path).map_err(io_err(path))?;
    // 探测不需要改选项（read_info 按值取走），所以无需 mut
    let opts = ::gif::DecodeOptions::new();
    let decoder = opts
        .read_info(f)
        .map_err(|e| PvError::Decode(format!("GIF 头解析失败: {e}")))?;
    let (w, h) = (decoder.width() as u32, decoder.height() as u32);
    if w == 0 || h == 0 {
        return Err(PvError::Decode("GIF 逻辑屏幕尺寸为 0".into()));
    }
    Ok(ImageInfo {
        format: ImageFormat::Gif,
        width: w,
        height: h,
        exif: None,
        raw: None,
        color_space: Some("索引色 (GIF)".to_string()),
    })
}

/// 解码 GIF 的全部帧。
///
/// `pub`（而非 `pub(super)`）是为了让 `bitmap/mod.rs` 能 `pub use` 出去 ——
/// 模块 `gif` 本身是私有的，所以对外仍然只经 `pv_decode::bitmap::decode_gif` 暴露。
pub fn decode_gif(path: &Path, target: DecodeTarget) -> Result<Animation> {
    let file = std::fs::File::open(path).map_err(io_err(path))?;

    let mut opts = ::gif::DecodeOptions::new();
    // 让 gif crate 直接输出 RGBA —— 免得自己查调色板、还要处理透明索引
    opts.set_color_output(::gif::ColorOutput::RGBA);
    let mut decoder = opts
        .read_info(file)
        .map_err(|e| PvError::Decode(format!("GIF 头解析失败: {e}")))?;

    let sw = decoder.width() as usize;
    let sh = decoder.height() as usize;
    if sw == 0 || sh == 0 {
        return Err(PvError::Decode("GIF 逻辑屏幕尺寸为 0".into()));
    }

    let loop_count = match decoder.repeat() {
        ::gif::Repeat::Infinite => None,
        ::gif::Repeat::Finite(n) => Some(n),
    };

    let canvas_len = sw * sh * 4; // 已由尺寸非 0 保证不会为 0
    let mut canvas = vec![0u8; canvas_len];
    let mut backup = vec![0u8; canvas_len];

    // 目标(帧)尺寸：只依赖原始尺寸与 target，所以**在循环外算一次**。
    // 合成必须按原始像素做（帧的局部偏移是按原始坐标给的），缩放只在快照那一步。
    let (tw, th) = fit_within(sw as u32, sh as u32, target.max_edge);

    let mut frames: Vec<AnimationFrame> = Vec::new();
    let mut total_bytes = 0usize;
    let mut truncated = false;

    while let Some(frame) = decoder
        .read_next_frame()
        .map_err(|e| PvError::Decode(format!("GIF 帧解码失败: {e}")))?
    {
        if frames.len() >= MAX_FRAMES || total_bytes >= MAX_TOTAL_BYTES {
            truncated = true;
            break;
        }

        let fw = frame.width as usize;
        let fh = frame.height as usize;
        let fx = frame.left as usize;
        let fy = frame.top as usize;

        // 损坏文件的兜底：帧缓冲长度必须与声明的帧尺寸吻合
        if frame.buffer.len() < fw * fh * 4 || fx >= sw || fy >= sh {
            truncated = true;
            break;
        }

        // ① Previous 处置需要「贴之前」的画布
        if frame.dispose == ::gif::DisposalMethod::Previous {
            backup.copy_from_slice(&canvas);
        }

        // ② 逐行把帧贴到画布（帧可能超出右/下边界，按可见部分裁）
        let copy_w = fw.min(sw - fx);
        let copy_h = fh.min(sh - fy);
        for y in 0..copy_h {
            let src = y * fw * 4;
            let dst = ((fy + y) * sw + fx) * 4;
            let n = copy_w * 4;
            canvas[dst..dst + n].copy_from_slice(&frame.buffer[src..src + n]);
        }

        // ③ 快照成输出帧（按目标尺寸缩放；GIF 一般不超 2048，通常不触发缩放）
        let full = RgbaImage {
            width: sw as u32,
            height: sh as u32,
            data: canvas.clone(),
        };
        let image = if (tw, th) == (sw as u32, sh as u32) {
            full
        } else {
            resize_rgba(&full, tw, th)?
        };
        total_bytes += image.data.len();

        frames.push(AnimationFrame {
            delay_ms: if frame.delay == 0 {
                DEFAULT_DELAY_MS
            } else {
                // gif crate 的 delay 单位是厘秒（1/100 s）
                frame.delay as u32 * 10
            },
            image,
        });

        // ④ 按 dispose 清理画布，为下一帧做准备
        match frame.dispose {
            ::gif::DisposalMethod::Background => {
                for y in 0..copy_h {
                    let start = ((fy + y) * sw + fx) * 4;
                    let n = copy_w * 4;
                    canvas[start..start + n].fill(0);
                }
            }
            ::gif::DisposalMethod::Previous => canvas.copy_from_slice(&backup),
            // Any / Keep：保持画布不动（GIF 里 DisposalMethod 只有这四种）
            _ => {}
        }
    }

    if frames.is_empty() {
        return Err(PvError::Decode("GIF 不含可解码的帧".to_string()));
    }

    Ok(Animation {
        // ⚠️ 必须是**帧像素**尺寸（缩放后），不是原始画布尺寸 —— 见字段文档。
        // 原始 sw/sh 只用于合成，不对外暴露：上层拿它算 stride。
        width: tw,
        height: th,
        frames,
        truncated,
        loop_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::DecodePurpose;

/// 造一张 2×2 的单帧 GIF 用于测试。
///
/// **用 gif crate 的编码器生成，而不是手写字节**：
/// GIF 的像素数据是 LZW 压缩的，手写极易出错 —— 那样一旦测试挂了，
/// 分不清是「解码器坏了」还是「样本本来就是坏的」。用官方编码器产出，
/// 样本一定合法，测试失败必然指向解码逻辑。
fn write_single_frame_gif(path: &Path) {
    // 2×2：红、绿
    //     蓝、白
    let mut rgba: Vec<u8> = vec![
        255, 0, 0, 255, 0, 255, 0, 255, //
        0, 0, 255, 255, 255, 255, 255, 255,
    ];
    let frame = ::gif::Frame::from_rgba(2, 2, &mut rgba);

    let mut out = Vec::new();
    {
        // 空全局调色板：让每帧自带局部调色板
        let mut enc = ::gif::Encoder::new(&mut out, 2, 2, &[]).unwrap();
        enc.write_frame(&frame).unwrap();
    }
    std::fs::write(path, out).unwrap();
}

    #[test]
    fn 探测GIF尺寸() {
        let dir = std::env::temp_dir().join("pv_gif_probe");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.gif");
        write_single_frame_gif(&p);

        let info = probe_gif(&p).unwrap();
        assert_eq!(info.format, ImageFormat::Gif);
        assert_eq!((info.width, info.height), (2, 2));

        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn 解码单帧GIF得到RGBA() {
        let dir = std::env::temp_dir().join("pv_gif_decode");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("b.gif");
        write_single_frame_gif(&p);

        let anim = decode_gif(&p, DecodeTarget::new(DecodePurpose::Preview)).unwrap();
        assert_eq!(anim.frames.len(), 1);
        assert!(anim.is_static());
        let f = &anim.frames[0];
        assert_eq!((f.image.width, f.image.height), (2, 2));
        // RGBA，2×2 → 16 字节
        assert_eq!(f.image.data.len(), 16);
        // delay = 0 → 兜底 100ms
        assert_eq!(f.delay_ms, DEFAULT_DELAY_MS);

        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn 非GIF文件报错而不是panic() {
        let dir = std::env::temp_dir().join("pv_gif_bad");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("not.gif");
        std::fs::write(&p, b"this is definitely not a gif file").unwrap();

        let r = decode_gif(&p, DecodeTarget::new(DecodePurpose::Preview));
        assert!(r.is_err(), "非 GIF 应返回错误");

        let _ = std::fs::remove_file(&p);
    }
}
