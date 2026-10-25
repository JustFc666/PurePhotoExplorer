//! 基准：**libjpeg-turbo 的 DCT 缩放**（`mozjpeg`）vs **现行 zune 全解码 + SIMD 缩放**。
//!
//! 背景：`bitmap/jpeg.rs` 头注释记着 2026-10-03 的实测 —— jpeg-decoder 的
//! `Decoder::scale()` 在 22MB/5847×3898 上要 49 秒（zune 193ms），于是被否。
//! 但那次只试了**纯 Rust 的 jpeg-decoder**，没试 libjpeg-turbo 的实现：
//! Mozilla 的 bug 2033250 用 libjpeg-turbo 的 IDCT 缩放在 1/8 档量到 2.38x
//! 且峰值内存 84MB→4.1MB。本测试回答：**我们这张图上到底有没有这个收益**。
//!
//! 跑法（默认忽略，不参与常规测试）：
//! ```text
//! PV_BENCH_JPEG="E:\...\big.jpg" cargo test -p pv-decode --test bench_jpeg_scale -- --ignored --nocapture
//! ```
//!
//! ⚠️ 只量 JPEG → 缩略图（256 长边）这一档，也就是列表卡片实际用的档位；
//! 缩放滤波器统一用项目自己的 `Lanczos3`，保证对比只有「解码方式」一个变量。

use std::path::PathBuf;
use std::time::Instant;

use fast_image_resize::images::Image;
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
use pv_decode::{decode_scaled, BitmapBackend, DecoderRegistry};

/// 缩略图长边（与 `ThumbTier::Thumbnail.long_edge()` 一致）
const MAX_EDGE: u32 = 256;
/// 每组跑几次，取最小值（最小值最接近真实成本，少受调度抖动影响）
const ROUNDS: usize = 3;

fn bench_path() -> Option<PathBuf> {
    let p = PathBuf::from(std::env::var("PV_BENCH_JPEG").ok()?);
    if p.is_file() {
        Some(p)
    } else {
        eprintln!("PV_BENCH_JPEG 指向的文件不存在：{}", p.display());
        None
    }
}

/// 等比缩到长边 `max_edge`。
fn fit(w: u32, h: u32, max_edge: u32) -> (u32, u32) {
    if w == 0 || h == 0 {
        return (1, 1);
    }
    let s = (max_edge as f64 / w.max(h) as f64).min(1.0);
    (((w as f64 * s).round() as u32).max(1), ((h as f64 * s).round() as u32).max(1))
}

/// mozjpeg 走 DCT 缩放到 `1/n`，再 SIMD 缩到 256。返回最终长边（防优化）。
fn dct_path(path: &std::path::Path, scale_num: u8) -> u128 {
    let mut dec = mozjpeg::Decompress::new_path(path).expect("mozjpeg 打开失败");
    // 输出 = 原图 × scale_num/8：1 → 1/8、2 → 1/4、4 → 1/2
    dec.scale(scale_num);
    let mut dec = dec.rgb().expect("mozjpeg 进入 rgb 失败");
    let (sw, sh) = (dec.width() as u32, dec.height() as u32);
    let rgb = dec.read_scanlines::<u8>().expect("mozjpeg 读取失败");

    let src = Image::from_vec_u8(sw, sh, rgb, PixelType::U8x3).expect("构造源图失败");
    let (dw, dh) = fit(sw, sh, MAX_EDGE);
    let mut dst = Image::new(dw, dh, PixelType::U8x3);
    let mut resizer = Resizer::new();
    resizer
        .resize(
            &src,
            &mut dst,
            &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3)),
        )
        .expect("缩放失败");
    dw as u128
}

fn best_of<F: FnMut() -> u128>(mut f: F) -> u128 {
    let mut best = u128::MAX;
    for _ in 0..ROUNDS {
        let t = Instant::now();
        let r = f();
        assert!(r > 0);
        best = best.min(t.elapsed().as_millis());
    }
    best
}

#[test]
#[ignore = "基准测试，手动跑：PV_BENCH_JPEG=<大图> cargo test --test bench_jpeg_scale -- --ignored --nocapture"]
fn dct缩放_vs_全解码缩放() {
    let Some(path) = bench_path() else {
        eprintln!("跳过：请设置 PV_BENCH_JPEG 指向一张大 JPEG");
        return;
    };
    let size_mb = std::fs::metadata(&path).map(|m| m.len() as f64 / 1048576.0).unwrap_or(0.0);

    let mut reg = DecoderRegistry::new();
    reg.register(Box::new(BitmapBackend));
    let (w, h) = pv_decode::probe_dimensions(&reg, &path).expect("探测尺寸失败");

    println!("\n=== {}（{size_mb:.1} MB）===", path.display());
    println!("原图 {w}×{h}，目标长边 {MAX_EDGE}（下采样 {:.1}x）", w as f64 / MAX_EDGE as f64);
    println!("DCT 1/8 后 {w_e}×{h_e}，1/4 后 {w_q}×{h_q}\n",
        w_e = w / 8, h_e = h / 8, w_q = w / 4, h_q = h / 4);

    // ── A：现行路径（zune 全解码 → RGB24 → SIMD 缩放 → RGBA）──
    let a = best_of(|| {
        let img = decode_scaled(&reg, &path, MAX_EDGE).expect("现行路径解码失败");
        assert!(img.width <= MAX_EDGE && img.height <= MAX_EDGE);
        img.width as u128
    });

    // ── B/C：mozjpeg DCT 缩放（1/8、1/4）+ SIMD 缩放到 256 ──
    let b = best_of(|| dct_path(&path, 1));
    let c = best_of(|| dct_path(&path, 2));

    let fmt = |name: &str, ms: u128| {
        let ratio = a as f64 / ms as f64;
        println!("  {name:<34} {ms:>5} ms   {ratio:>5.2}x");
    };
    println!("缩略图档（{MAX_EDGE} 长边）耗时，每组 {ROUNDS} 次取最小：");
    println!("  {:<34} {:>5}    基准", "A 现行（zune 全解码 + SIMD）", a);
    fmt("B mozjpeg DCT 1/8 + SIMD", b);
    fmt("C mozjpeg DCT 1/4 + SIMD", c);

    let best_dct = b.min(c);
    let ratio = a as f64 / best_dct as f64;
    println!("\n>>> 最好的一组加速比：{ratio:.2}x");
    if ratio < 1.3 {
        println!(">>> 结论：**不值得**引入 C 依赖（收益 < 1.3x）");
    } else {
        println!(">>> 结论：**值得考虑**（>= 1.3x）；还需看编译成本与包体积增量");
    }

    // ── 画质：DCT 缩放是**有损**的（只解低频系数），必须量一下差多少 ──
    let a_img = decode_scaled(&reg, &path, MAX_EDGE).expect("现行路径解码失败");
    println!("\n=== 画质（DCT vs 全解码，{}×{}）===", a_img.width, a_img.height);
    println!("  平均绝对误差（0-255，逐通道，只比重叠区域）：");
    for (label, sn) in [("1/8", 1u8), ("1/4", 2u8)] {
        let (bw, bh, brgb) = dct_rgb(&path, sn);
        let mad = mean_abs_diff_overlap(&a_img, bw, bh, &brgb);
        let verdict = if mad < 3.0 {
            "几乎无差"
        } else if mad < 6.0 {
            "轻微，可接受"
        } else {
            "⚠️ 肉眼可辨，慎用"
        };
        println!("    DCT {label}（{bw}×{bh}）：MAD = {mad:>5.2}  → {verdict}");
    }
    println!("  参考：同一张图在两条无损路径之间的 MAD 通常 <1");

    // 落盘给人眼看（1/8 与全解码两张）
    let out = std::env::temp_dir();
    let _ = image_write_rgba(&a_img, &out.join("bench_A_full_decode.png"));
    let b8 = dct_rgb(&path, 1);
    let _ = image_write_rgb(&b8, &out.join("bench_B_dct18.png"));
    println!(
        "  两张缩略图已写到 {}（bench_A_full_decode.png / bench_B_dct18.png）",
        out.display()
    );
}

/// 只比两张图的**重叠区域**（等比缩放的取整规则不同，可能差 1px）。
fn mean_abs_diff_overlap(
    a: &pv_decode::RgbaImage,
    bw: u32,
    bh: u32,
    b_rgb: &[u8],
) -> f64 {
    let w = a.width.min(bw) as usize;
    let h = a.height.min(bh) as usize;
    if w == 0 || h == 0 {
        return f64::NAN;
    }
    let mut sum = 0u64;
    for y in 0..h {
        for x in 0..w {
            let ai = (y * a.width as usize + x) * 4;
            let bi = (y * bw as usize + x) * 3;
            for c in 0..3 {
                sum += (a.data[ai + c] as i32 - b_rgb[bi + c] as i32).unsigned_abs() as u64;
            }
        }
    }
    sum as f64 / (w * h * 3) as f64
}

/// mozjpeg 走 DCT 缩放到 `1/scale_num/8`，再缩到 256，返回 (宽, 高, RGB)。
fn dct_rgb(path: &std::path::Path, scale_num: u8) -> (u32, u32, Vec<u8>) {
    let mut dec = mozjpeg::Decompress::new_path(path).expect("mozjpeg 打开失败");
    dec.scale(scale_num);
    let mut dec = dec.rgb().expect("mozjpeg 进入 rgb 失败");
    let (sw, sh) = (dec.width() as u32, dec.height() as u32);
    let rgb = dec.read_scanlines::<u8>().expect("mozjpeg 读取失败");

    let src = Image::from_vec_u8(sw, sh, rgb, PixelType::U8x3).expect("构造源图失败");
    let (dw, dh) = fit(sw, sh, MAX_EDGE);
    let mut dst = Image::new(dw, dh, PixelType::U8x3);
    let mut resizer = Resizer::new();
    resizer
        .resize(
            &src,
            &mut dst,
            &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3)),
        )
        .expect("缩放失败");
    (dw, dh, dst.buffer().to_vec())
}

/// 把 RGBA 写成 PNG（依赖 dev-dependency 里的 `png`）。
fn image_write_rgba(img: &pv_decode::RgbaImage, out: &std::path::Path) -> std::io::Result<()> {
    let f = std::fs::File::create(out)?;
    let w = std::io::BufWriter::new(f);
    let mut enc = png::Encoder::new(w, img.width, img.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc.write_header()?;
    wr.write_image_data(&img.data)?;
    Ok(())
}

fn image_write_rgb(img: &(u32, u32, Vec<u8>), out: &std::path::Path) -> std::io::Result<()> {
    let f = std::fs::File::create(out)?;
    let w = std::io::BufWriter::new(f);
    let mut enc = png::Encoder::new(w, img.0, img.1);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc.write_header()?;
    wr.write_image_data(&img.2)?;
    Ok(())
}
