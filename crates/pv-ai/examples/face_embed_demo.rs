//! 人脸特征冒烟：`cargo run --release -p pv-ai --example face_embed_demo -- <图片>`
//!
//! 打印检出的人脸两两余弦。同一张合照里的**不同人**应该都很低（< 0.4）；
//! 如果出现普遍 > 0.8 或负数，说明对齐/归一化写错了。

use std::path::Path;
use std::time::Instant;

use pv_ai::face::{cosine, FaceDetector, FaceRecognizer};
use pv_decode::{decode_scaled, BitmapBackend, DecoderRegistry};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).cloned().unwrap_or_else(|| {
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../test-image/mmexport1557578947455.jpg")
            .to_string()
    });
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/ai/face");

    let mut reg = DecoderRegistry::new();
    reg.register(Box::new(BitmapBackend));
    let img = decode_scaled(&reg, Path::new(&path), 1600)?;

    let mut det = FaceDetector::load(Path::new(dir))?;
    let mut rec = FaceRecognizer::load(Path::new(dir))?;

    let faces = det.detect(&img, 0.6)?;
    println!("检出 {} 张脸（图 {}×{}）\n", faces.len(), img.width, img.height);

    let mut embs = Vec::new();
    for (i, f) in faces.iter().enumerate() {
        let t = Instant::now();
        match rec.embed(&img, f) {
            Ok(e) => {
                println!(
                    "#{} score={:.3} box=({:.0},{:.0}) {:.0}×{:.0} · 对齐+特征 {:.1} ms · dim={}",
                    i + 1,
                    f.score,
                    f.x,
                    f.y,
                    f.w,
                    f.h,
                    t.elapsed().as_secs_f64() * 1000.0,
                    e.len()
                );
                embs.push(e);
            }
            Err(e) => println!("#{} 特征失败：{e}", i + 1),
        }
    }

    if embs.len() >= 2 {
        println!("\n两两余弦：");
        for i in 0..embs.len() {
            for j in (i + 1)..embs.len() {
                println!("  #{} × #{} = {:.3}", i + 1, j + 1, cosine(&embs[i], &embs[j]));
            }
        }
    }
    Ok(())
}
