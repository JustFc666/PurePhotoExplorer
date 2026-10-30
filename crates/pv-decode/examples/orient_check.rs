//! 验证 EXIF 方向是否真的落到了解码结果上。
//!
//! 用法：`cargo run -q -p pv-decode --example orient_check -- <图片路径> [...]`
//!
//! 输出每张图的「文件声明方向 / 解码后宽高」。竖拍照片（方向 6/8）
//! 解码后应当是**高 > 宽**；如果还是宽 > 高，说明摆正没生效。

use std::path::Path;

use pv_decode::{
    decode_scaled, BitmapBackend, DecodeTarget, DecodePurpose, DecoderRegistry,
    EmbeddedPreviewPipeline,
};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("用法：orient_check <图片> [更多图片…]");
        return;
    }
    let mut reg = DecoderRegistry::new();
    reg.register(Box::new(BitmapBackend));
    reg.set_raw_pipeline(Box::new(EmbeddedPreviewPipeline));
    for a in args {
        let p = Path::new(&a);
        let raw_dims = pv_decode::probe_dimensions(&reg, p)
            .map(|(w, h)| format!("{w}x{h}"))
            .unwrap_or_else(|e| format!("探测失败 {e}"));
        let o = pv_meta::read_orientation(p).unwrap_or(1);
        // 走 128 长边的缩略图路径，和界面用的是同一条
        let out = decode_scaled(&reg, p, 128);
        let shown = match &out {
            Ok(img) => format!("{}x{}", img.width, img.height),
            Err(e) => format!("解码失败 {e}"),
        };
        // 顺带确认「不摆正」的那条路确实是原始的
        let asis = reg
            .decode_asis(p, DecodeTarget::new(DecodePurpose::Thumbnail))
            .map(|i| format!("{}x{}", i.width, i.height))
            .unwrap_or_else(|e| format!("失败 {e}"));
        println!(
            "{:<34} 方向={o:<2} 原始={raw_dims:<12} 摆正后={shown:<12} asis={asis}",
            p.file_name().unwrap_or_default().to_string_lossy()
        );
    }
}
