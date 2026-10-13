//! 临时验证程序：模拟 Tauri 前端的完整调用链，实测 RAW 能否打开。
//!
//! 做的事：`DecoderRegistry::probe` → `decode`（Preview 档）
//! → 统计像素 → 打印与 Tauri `probe_image` 完全一致的字段。

use std::path::{Path, PathBuf};

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("test-image");

    let cases = [
        "canon-cr3-sample.CR3",
        "nikon-nrw-sample.NRW",
        "panasonic-rw2-sample.RW2",
        "fuji-raf-sample.RAF",
        "olympus-orf-sample.ORF",
        "sony-sr2-sample.SR2",
        "sony-srf-sample.SRF",
    ];

    // 与 state.rs 完全一致的配置
    let mut reg = pv_decode::DecoderRegistry::new();
    reg.register(Box::new(pv_decode::BitmapBackend));
    reg.set_raw_pipeline(Box::new(pv_decode::EmbeddedPreviewPipeline::new()));

    println!("{:=<96}", "");
    println!("{:<22} {:>12} {:>12} {:>8} {:>10}", "文件", "探测尺寸", "解码尺寸", "非黑%", "耗时ms");
    println!("{:-<96}", "");

    let mut ok = 0;
    for name in cases {
        let p: PathBuf = root.join(name);
        if std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0) < 4096 {
            println!("{name:<22} —— 样本不可用，跳过");
            continue;
        }

        let t0 = std::time::Instant::now();
        let info = match reg.probe(&p) {
            Ok(v) => v,
            Err(e) => {
                println!("{name:<22} 探测失败: {e}");
                continue;
            }
        };
        let img = match reg.decode(&p, pv_decode::DecodeTarget::new(pv_decode::DecodePurpose::Preview)) {
            Ok(v) => v,
            Err(e) => {
                println!("{name:<22} 解码失败: {e}");
                continue;
            }
        };
        let ms = t0.elapsed().as_millis();

        let total = (img.width * img.height) as usize;
        let non_black = img
            .data
            .chunks_exact(4)
            .filter(|px| px[0] > 5 || px[1] > 5 || px[2] > 5)
            .count();
        let pct = non_black as f64 * 100.0 / total as f64;

        let raw_tag = info
            .raw
            .as_ref()
            .map(|m| format!("{} 内嵌预览={}", m.raw_format, m.has_embedded_preview))
            .unwrap_or_else(|| "无 RAW 元数据".into());

        println!(
            "{name:<22} {:>5}×{:<6} {:>5}×{:<6} {:>7.0}% {:>9}",
            info.width, info.height, img.width, img.height, pct, ms
        );
        println!("{:<22}   └ {raw_tag}", "");
        ok += 1;
    }

    println!("{:=<96}", "");
    println!("成功 {ok}/7");

    // EXIF：信息面板的「拍摄参数」完全依赖它，逐样本确认
    println!("\n{:=<96}", "");
    println!("EXIF 拍摄参数（信息面板数据源）");
    println!("{:-<96}", "");
    for name in cases {
        let p: PathBuf = root.join(name);
        if std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0) < 4096 {
            continue;
        }
        let head = std::fs::read(&p)
            .map(|b| b[..b.len().min(16)].to_vec())
            .unwrap_or_default();
        let magic = head
            .iter()
            .take(8)
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(" ");
        match pv_meta::read_exif(&p) {
            Some(e) => println!(
                "{name:<22} ✓ {} {} | ISO {:?} {} {} {}",
                e.make.as_deref().unwrap_or("-"),
                e.model.as_deref().unwrap_or("-"),
                e.iso,
                e.exposure_time.as_deref().unwrap_or("-"),
                e.f_number.as_deref().unwrap_or("-"),
                e.focal_length.as_deref().unwrap_or("-"),
            ),
            None => println!("{name:<22} ✗ 未读到（魔数 {magic}）"),
        }
    }
}
