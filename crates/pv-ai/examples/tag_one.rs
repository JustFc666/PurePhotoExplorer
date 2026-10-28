//! 单张图打标验证：`cargo run -p pv-ai --example tag_one -- <图片路径>`
//!
//! 用来和 Python 版（`tools/ai/score_one.py`）对分：同一张图、同一套词表，
//! Rust 与 torch 的 top 标签和分数应该基本重合（差 < 0.01 就算对齐）。

use std::path::Path;
use std::time::Instant;

use pv_ai::tagger::Tagger;
use pv_decode::{decode_scaled, BitmapBackend, DecoderRegistry};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("用法: tag_one <图片路径>")?;
    // 第二个参数可覆盖模型目录（用来对比 fp16 / fp32 的精度）
    let dir = std::env::args()
        .nth(2)
        .unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/ai/tag").to_string());

    let mut reg = DecoderRegistry::new();
    reg.register(Box::new(BitmapBackend));

    let t0 = Instant::now();
    // 1024 长边：和 Python 版（PIL draft 448 后再由 CLIPProcessor 缩小）效果最接近。
    // ⛔ 别用小值（试过 640）：缩小前的细节更少，余弦会系统性偏低 0.01–0.02，
    // 阈值 0.40 附近就会整批漏打。
    let img = decode_scaled(&reg, Path::new(&path), 1024)?;
    let decode_ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("解码 {}×{} · {decode_ms:.1} ms", img.width, img.height);

    let t1 = Instant::now();
    let mut tagger = Tagger::load(Path::new(&dir))?;
    println!(
        "模型加载 {:.0} ms · 词表 {} 条 · {} 条 prompt",
        t1.elapsed().as_secs_f64() * 1000.0,
        tagger.vocab().len(),
        tagger.prompts_len()
    );

    let t2 = Instant::now();
    let emb = tagger.embed(&img)?;
    println!(
        "推理 {:.1} ms · 向量 {} 维",
        t2.elapsed().as_secs_f64() * 1000.0,
        emb.len()
    );

    println!("\nTop 12：");
    for (i, s) in tagger.score(&emb).iter().take(12).enumerate() {
        println!(
            "{:>2}. {:<10} {:.4}   [{}/{}]",
            i + 1,
            s.term,
            s.score,
            s.l1,
            s.l2
        );
    }
    Ok(())
}
