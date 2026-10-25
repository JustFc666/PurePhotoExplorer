//! 临时基准：量 `fs_list_images` 的**整条命令路径**（列举 + 探测，RAW 跳过）。
//!
//! ⚠️ 注册表必须跟 App 一样构造（注册 BitmapBackend + RAW 管线）——
//! 用裸 `DecoderRegistry::new()` 会让所有探测瞬间失败，得出完全错误的结论。
//!
//! 用法：`cargo run -q -p pv-store --example probe_bench -- <目录> [个数]`

#[cfg(not(windows))]
fn main() {
    eprintln!("仅 Windows");
}

#[cfg(windows)]
fn main() {
    use pv_decode::{BitmapBackend, DecoderRegistry, EmbeddedPreviewPipeline, probe_dimensions};
    use std::path::PathBuf;
    use std::time::Instant;

    let dir = std::env::args().nth(1).expect("用法：probe_bench <目录> [个数]");
    let limit: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(500);

    // 与 AppState::new() 完全一致的构造
    let mut reg = DecoderRegistry::new();
    reg.register(Box::new(BitmapBackend));
    reg.set_raw_pipeline(Box::new(EmbeddedPreviewPipeline::new()));

    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("读目录失败")
        .flatten()
        .filter(|e| e.path().is_file())
        .map(|e| e.path())
        .collect();
    files.sort();
    files.truncate(limit);

    println!("列举 {limit} 个（目录 {dir}）");

    // 1) 纯列举
    let t = Instant::now();
    let listed = pv_store::list_dir_images(std::path::Path::new(&dir)).unwrap_or_default();
    println!(
        "  list_dir_images      → {} 张，{:?}",
        listed.len(),
        t.elapsed()
    );

    // 2) 按 is_raw 跳过后的探测（= 现在 fs_list_images 的做法）
    let t = Instant::now();
    let mut ok = 0usize;
    let mut skipped = 0usize;
    let mut slow: Vec<(String, u128)> = Vec::new();
    for f in listed.iter().take(limit) {
        if f.is_raw {
            skipped += 1;
            continue;
        }
        let t1 = Instant::now();
        if probe_dimensions(&reg, std::path::Path::new(&f.path)).is_ok() {
            ok += 1;
        }
        let ms = t1.elapsed().as_millis();
        if ms > 20 {
            slow.push((f.name.clone(), ms));
        }
    }
    println!(
        "  探测（跳过 {skipped} 个 RAW）→ 成功 {ok}，{:?}",
        t.elapsed()
    );
    if !slow.is_empty() {
        slow.sort_by_key(|x| std::cmp::Reverse(x.1));
        println!("  超过 20ms 的 {} 张，最慢 5 张：", slow.len());
        for (name, ms) in slow.iter().take(5) {
            println!("    {ms:>6} ms  {name}");
        }
    }

    // 3) 不跳过 RAW 的话要多久（只量前 20 个， extrapolate ）
    let raws: Vec<&_> = listed.iter().filter(|f| f.is_raw).take(20).collect();
    if !raws.is_empty() {
        let t = Instant::now();
        for f in &raws {
            let _ = probe_dimensions(&reg, std::path::Path::new(&f.path));
        }
        let per = t.elapsed().as_millis() as f64 / raws.len() as f64;
        println!(
            "  ⚠️ 若不跳过 RAW：前 {} 张 {:.0} ms/张 → 全目录约 {:.0} 秒",
            raws.len(),
            per,
            per * listed.iter().filter(|f| f.is_raw).count() as f64 / 1000.0
        );
    }
}
