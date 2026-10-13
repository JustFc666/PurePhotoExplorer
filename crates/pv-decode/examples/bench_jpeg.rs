//! JPEG 解码性能基准 —— S1 验收依据。
//!
//! **为什么需要这个**：S1 阶段发现 jpeg-decoder 对 22 MB 大图需 49 秒，
//! 换 zune-jpeg 后降到亚秒级。此基准防止性能回归。
//!
//! 运行：
//! ```bash
//! cargo run -p pv-decode --example bench_jpeg --release
//! ```

use pv_decode::{BitmapBackend, DecodePurpose, DecodeTarget, ImageDecoder};
use std::time::Instant;

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| r"C:\Users\Fc\WorkBuddy\2026-10-02-22-48-04\test-image\DSC9515.JPG".to_string());
    let p = std::path::Path::new(&path);
    if !p.exists() {
        eprintln!("文件不存在: {}", p.display());
        std::process::exit(1);
    }

    let be = BitmapBackend;

    // 探测（应为 O(1)，不解码像素）
    let t = Instant::now();
    let info = match be.probe(p) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("探测失败: {e}");
            std::process::exit(1);
        }
    };
    let probe_us = t.elapsed().as_micros();
    println!("文件: {}", p.file_name().unwrap().to_string_lossy());
    println!(
        "尺寸: {}x{}  探测耗时: {} µs\n",
        info.width, info.height, probe_us
    );

    // 各档位解码
    println!("{:<12} {:>13} {:>10} {:>12}", "档位", "输出尺寸", "耗时", "相对预算");
    println!("{}", "-".repeat(52));

    // S1 性能预算（见开发文档 §7）
    //
    // 注意：所有档位都必须先解码全图（JPEG 熵解码无法跳过），
    // 所以预算按「全图解码 + SIMD 缩放」的实际成本定。
    // 缓存命中时（第二次打开同一张图）为个位数毫秒。
    let budgets: &[(&str, DecodePurpose, u128)] = &[
        ("缩略图", DecodePurpose::Thumbnail, 300),
        ("预览", DecodePurpose::Preview, 500),
        ("编辑", DecodePurpose::Edit, 1500),
        ("导出", DecodePurpose::Export, 3000),
    ];

    let mut all_pass = true;
    for (name, purpose, budget_ms) in budgets {
        let t = Instant::now();
        match be.decode(p, DecodeTarget::new(*purpose)) {
            Ok(img) => {
                let ms = t.elapsed().as_millis();
                let ok = ms <= *budget_ms;
                all_pass &= ok;
                println!(
                    "{name:<12} {:>6}x{:<6} {:>6} ms {:>9} / {budget_ms} ms {}",
                    img.width,
                    img.height,
                    ms,
                    ms,
                    if ok { "达标" } else { "超标" }
                );
            }
            Err(e) => {
                all_pass = false;
                println!("{name:<12} 解码失败: {e}");
            }
        }
    }

    println!();
    if all_pass {
        println!("全部档位达标");
    } else {
        println!("存在超标档位，需优化");
    }
}
