//! 手工验证：列包内图片 + 解出第一张 + 报出缓存路径。
//!
//! ```text
//! cargo run -p pv-archive --example probe -- Temp/arctest/pack.cbz Temp/arctest/pack.7z
//! ```
//!
//! 输出四栏：条目数 / 前几个名字 / 解出的字节数 / 缓存落点。
//! 用来一眼看出「后端选错（zip 走了 CLI）」「中文名乱码」「缓存没命中」这几类问题。

use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("用法：probe <包路径>…");
        return;
    }
    let root = pv_archive::cache::default_cache_root().join("_probe");

    for pack in args {
        println!("── {pack}");
        match pv_archive::list_images(&pack) {
            Ok(items) => {
                println!("   图片条目 {} 个", items.len());
                for e in items.iter().take(3) {
                    println!("     · {} ({} 字节)", e.name, e.size);
                }
                if let Some(first) = items.first() {
                    let virtual_path = pv_archive::make_virtual(&pack, &first.name);
                    println!("   虚拟路径 {virtual_path}");
                    let t0 = std::time::Instant::now();
                    match pv_archive::materialize(&virtual_path, &root) {
                        Ok(p) => {
                            let len = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                            println!(
                                "   解出 {} 字节 → {}（{} ms）",
                                len,
                                p.display(),
                                t0.elapsed().as_millis()
                            );
                            // 第二次应当命中缓存，耗时接近 0
                            let t1 = std::time::Instant::now();
                            let p2 = pv_archive::materialize(&virtual_path, &root).unwrap();
                            println!(
                                "   再取一次（缓存）{} ms，同一文件={}",
                                t1.elapsed().as_millis(),
                                p2 == p
                            );
                            // 交给解码层验证：真的能当普通图片打开
                            let mut reg = pv_decode::DecoderRegistry::new();
                            reg.register(Box::new(pv_decode::BitmapBackend));
                            match pv_decode::probe_dimensions(&reg, &p2) {
                                Ok((w, h)) => println!("   解码探测 {w}×{h}"),
                                Err(e) => println!("   ⚠️ 解码探测失败：{e}"),
                            }
                        }
                        Err(e) => println!("   ⚠️ 解压失败：{e}"),
                    }
                }
            }
            Err(e) => println!("   ⚠️ {e}"),
        }
        println!();
    }
    let _ = Path::new(".");
}
