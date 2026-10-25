//! 基准：**RAW 出缩略图的现行成本**（量「全文件扫描找内嵌预览」到底多贵）。
//!
//! 目的：和 `bench_jpeg_scale` 的 JPEG 数据放在一起，判断
//! 「① RAW 按 IFD 直取预览」与「② JPEG DCT 缩放」哪条总收益更高。
//!
//! 跑法：
//! ```text
//! PV_BENCH_RAW="H:\...\_DSC6674.ARW" cargo test --release -p pv-decode --test bench_raw_thumb -- --ignored --nocapture
//! ```

use std::time::Instant;

use pv_decode::{make_thumbnail_with_bitmap, BitmapBackend, DecoderRegistry, EmbeddedPreviewPipeline};

#[test]
#[ignore = "基准测试，手动跑：PV_BENCH_RAW=<.arw> cargo test --release --test bench_raw_thumb -- --ignored --nocapture"]
fn raw缩略图现行成本() {
    let Ok(p) = std::env::var("PV_BENCH_RAW") else {
        eprintln!("跳过：请设置 PV_BENCH_RAW 指向一个 RAW 文件");
        return;
    };
    let path = std::path::PathBuf::from(&p);
    if !path.is_file() {
        eprintln!("文件不存在：{p}");
        return;
    }
    let mb = std::fs::metadata(&path).map(|m| m.len() as f64 / 1048576.0).unwrap_or(0.0);

    let mut reg = DecoderRegistry::new();
    reg.register(Box::new(BitmapBackend));
    reg.set_raw_pipeline(Box::new(EmbeddedPreviewPipeline::new()));

    // 冷/热各跑两次：第一次含磁盘寻道，后面受 OS 缓存影响
    let mut times = Vec::new();
    for i in 0..4 {
        let t = Instant::now();
        let (img, bytes) = make_thumbnail_with_bitmap(&reg, &path, 256).expect("RAW 缩略图失败");
        let ms = t.elapsed().as_millis();
        println!(
            "  第 {} 次：{ms:>5} ms（缩略图 {}×{}，JPEG {} 字节）",
            i + 1,
            img.width,
            img.height,
            bytes.len()
        );
        times.push(ms);
    }

    let first = times.first().copied().unwrap_or(0);
    let best = times.iter().copied().min().unwrap_or(0);
    println!("\n文件 {}（{mb:.1} MB）", path.display());
    println!("  首次（冷盘）：{first} ms");
    println!("  最快（热缓存）：{best} ms  ← 取这个作为「纯计算成本」的下界");
    println!("\n>>> 参考：若改成按 IFD 直取预览（只读 1–3MB），纯计算成本约 30–50 ms");
    if best > 0 {
        println!(">>> 收益上界约 {:.0}x（按热缓存成本算）", best as f64 / 40.0);
    }
}
