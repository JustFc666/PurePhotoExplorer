//! 人脸检测冒烟：`cargo run -p pv-ai --example face_detect_demo -- <图片>`
//!
//! 关注三件事：能不能检出、框的位置合不合理、耗时多少。

use std::path::Path;
use std::time::Instant;

use pv_ai::face::FaceDetector;
use pv_decode::{decode_scaled, BitmapBackend, DecoderRegistry};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).cloned().unwrap_or_else(|| {
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../test-image/mmexport1557578947455.jpg")
            .to_string()
    });
    let dir = args.get(2).cloned().unwrap_or_else(|| {
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/ai/face").to_string()
    });

    let mut reg = DecoderRegistry::new();
    reg.register(Box::new(BitmapBackend));

    let t0 = Instant::now();
    let img = decode_scaled(&reg, Path::new(&path), 1600)?;
    println!("解码 {}×{} · {:.0} ms", img.width, img.height, t0.elapsed().as_secs_f64() * 1000.0);

    let t1 = Instant::now();
    let mut det = FaceDetector::load(Path::new(&dir))?;
    println!("模型加载 {:.0} ms", t1.elapsed().as_secs_f64() * 1000.0);

    let t2 = Instant::now();
    let faces = det.detect(&img, 0.6)?;
    println!(
        "检测 {:.1} ms · 找到 {} 张脸",
        t2.elapsed().as_secs_f64() * 1000.0,
        faces.len()
    );
    for (i, f) in faces.iter().enumerate() {
        println!(
            "  #{} score={:.3} box=({:.0},{:.0}) {:.0}×{:.0} · 左眼({:.0},{:.0}) 右眼({:.0},{:.0})",
            i + 1,
            f.score,
            f.x,
            f.y,
            f.w,
            f.h,
            f.kps[0][0],
            f.kps[0][1],
            f.kps[1][0],
            f.kps[1][1]
        );
    }
    Ok(())
}
