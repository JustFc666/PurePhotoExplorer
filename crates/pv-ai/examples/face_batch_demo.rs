//! 人脸批处理冒烟：`cargo run --release -p pv-ai --example face_batch_demo`
//!
//! 用 `test-image` 里的若干张图跑「检测 → 特征 → 聚类」全链路，
//! 只打印结果不写库。验证的是 `detect_batch` + `two_stage_cluster` 的配合。

use std::path::PathBuf;

use pv_ai::cluster::{two_stage_cluster, ClusterOpts};
use pv_ai::face::{FaceDetector, FaceRecognizer};
use pv_ai::face_batch::{detect_batch, FaceBatchOpts, FaceProgress};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../");
    let dir_path = format!("{root}assets/ai/face");
    let dir = std::path::Path::new(&dir_path);

    // 挑几张有人脸的（合照/人像），外加一张风景（应该 0 张脸）
    let names = [
        "test-image/mmexport1557578947455.jpg",
        "test-image/IMG_20230923_095924.jpg",
        "test-image/1cbe43343c7ffabe463c5d6df08bce4.jpg",
        "test-image/03ee756640.jpg",
        "test-image/1bde5d5265.jpg",
        "test-image/0a2ad9cec507ca3b593d52f4128e5651_已裁剪.jpg",
    ];
    let images: Vec<(i64, PathBuf)> = names
        .iter()
        .enumerate()
        .map(|(i, n)| (i as i64, PathBuf::from(format!("{root}{n}"))))
        .collect();

    let mut det = FaceDetector::load(dir)?;
    let mut rec = FaceRecognizer::load(dir)?;
    let opts = FaceBatchOpts::default();

    let t0 = std::time::Instant::now();
    let (faces, stats) = detect_batch(
        &images,
        &mut det,
        &mut rec,
        &opts,
        |p: &FaceProgress| {
            println!(
                "[{}/{}] {} → {} 张脸",
                p.done, p.total, p.name, p.faces
            );
        },
    );

    println!(
        "\n检测：{} 张图 · {} 张有脸 · 共 {} 张脸 · 失败 {} · 用时 {:.1}s",
        stats.images,
        stats.with_face,
        stats.faces,
        stats.failed,
        t0.elapsed().as_secs_f64()
    );
    for e in &stats.errors {
        println!("  失败：{e}");
    }

    if faces.is_empty() {
        println!("没有脸，跳过聚类");
        return Ok(());
    }

    let embs: Vec<Vec<f32>> = faces.iter().map(|f| f.embedding.clone()).collect();
    let c_opts = ClusterOpts::default();
    let t1 = std::time::Instant::now();
    let labels = two_stage_cluster(&embs, &c_opts);
    let mut sizes: std::collections::HashMap<i32, usize> = std::collections::HashMap::new();
    for &l in &labels {
        *sizes.entry(l).or_insert(0) += 1;
    }
    let mut sorted: Vec<(i32, usize)> = sizes.into_iter().collect();
    sorted.sort_by_key(|a| std::cmp::Reverse(a.1));

    println!(
        "\n聚类：{} 个簇 · 用时 {:.0} ms · t_core={:.2} t_join={:.2} min_core={}",
        sorted.len(),
        t1.elapsed().as_secs_f64() * 1000.0,
        c_opts.t_core,
        c_opts.t_join,
        c_opts.min_core
    );
    for (lb, n) in sorted.iter() {
        let mark = if *n >= c_opts.min_core { "→ 建人物" } else { "→ 单例" };
        println!("  簇 {lb}: {n} 张脸 {mark}");
    }
    Ok(())
}
