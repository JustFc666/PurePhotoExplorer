//! 批处理层冒烟：`cargo run -p pv-ai --example tag_batch_demo`
//!
//! 只验「循环 + 阈值筛选 + 回调」这套，不进数据库（写库由调用方注入）。
//! 专门放了**一张坏图**进去，确认单张失败不会拖垮整批。

use std::path::PathBuf;

use pv_ai::batch::{tag_batch, TagOptions};
use pv_ai::tagger::Tagger;

fn main() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../");
    let dir = format!("{root}assets/ai/tag");
    let mut tagger = match Tagger::load(std::path::Path::new(&dir)) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("模型加载失败：{e}\n先跑 python tools/ai/export_clip_onnx.py");
            return;
        }
    };
    println!("模型就绪 · 词表 {} 条", tagger.vocab().len());

    // 第三张特意用不存在的路径：验证「单张失败不影响整批」
    let names = [
        "test-image/mmexport1557578947455.jpg",
        "test-image/IMG_20230923_095924.jpg",
        "test-image/不存在的文件.jpg",
    ];
    let images: Vec<(i64, PathBuf)> = names
        .iter()
        .enumerate()
        .map(|(i, n)| (i as i64, PathBuf::from(format!("{root}{n}"))))
        .collect();

    let opts = TagOptions::default();
    println!(
        "阈值：二级 ≥{:.2} · 一级 ≥{:.2} · 每张最多 {} 个\n",
        opts.threshold, opts.threshold_l1, opts.max_tags
    );

    let t0 = std::time::Instant::now();
    let stats = tag_batch(
        &images,
        &mut tagger,
        &opts,
        // 返回 false = 提前收尾（命令行走完，没有「停止」按钮）
        |p| {
            println!("[{}/{}] {}", p.done, p.total, p.name);
            true
        },

        |d| {
            let terms: Vec<String> = d
                .tags
                .iter()
                .map(|(t, s)| format!("{t}({s:.3})"))
                .collect();
            println!("   image {} → {}", d.image_id, terms.join(" · "));
        },
    );

    println!(
        "\n成功 {} · 失败 {} · 用时 {:.1}s",
        stats.ok,
        stats.failed,
        t0.elapsed().as_secs_f64()
    );
    for e in &stats.errors {
        println!("  失败明细：{e}");
    }
}
