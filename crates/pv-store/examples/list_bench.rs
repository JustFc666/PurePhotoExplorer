//! 临时基准：对比 `list_dir_images`（逐文件读魔数）与 `list_dir_images_fast`（扩展名优先）。
//!
//! 用法：`cargo run -q --release -p pv-store --example list_bench -- <目录>`

#[cfg(not(windows))]
fn main() {
    eprintln!("仅 Windows");
}

#[cfg(windows)]
fn main() {
    use std::time::Instant;

    let dir = std::env::args().nth(1).expect("用法：list_bench <目录>");

    // 慢路径（入库用的那套）
    let t0 = Instant::now();
    let slow = pv_store::list_dir_images(std::path::Path::new(&dir)).expect("慢路径失败");
    let slow_ms = t0.elapsed().as_secs_f64() * 1000.0;

    // 快路径（浏览用的）
    let t1 = Instant::now();
    let fast = pv_store::list_dir_images_fast(std::path::Path::new(&dir)).expect("快路径失败");
    let fast_ms = t1.elapsed().as_secs_f64() * 1000.0;

    println!("目录：{dir}");
    println!("  慢路径（逐文件读魔数）: {:>8.1} ms  → {} 个", slow_ms, slow.len());
    println!("  快路径（扩展名优先）  : {:>8.1} ms  → {} 个", fast_ms, fast.len());
    if slow.len() == fast.len() {
        println!("  ✅ 数量一致；加速 {:.1}x", slow_ms / fast_ms.max(0.01));
    } else {
        println!("  ⚠️ 数量不一致（{} vs {}）", slow.len(), fast.len());
    }
}
