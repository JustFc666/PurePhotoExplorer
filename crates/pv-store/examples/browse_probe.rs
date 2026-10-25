//! 临时排查：复现 `fs_list_images` 的后端路径，看 `list_dir_images` 返回多少。
//!
//! 用法：`cargo run -q -p pv-store --example browse_probe -- <目录>`

#[cfg(not(windows))]
fn main() {
    eprintln!("仅 Windows");
}

#[cfg(windows)]
fn main() {
    use pv_store::list_dir_images;
    use std::path::Path;

    let raw = std::env::args().nth(1).expect("用法：browse_probe <目录>");
    println!("入参（原样）: {raw:?}");

    for label in ["原样", "正斜杠"] {
        let s = if label == "原样" { raw.clone() } else { raw.replace('\\', "/") };
        let p = Path::new(&s);
        let exists = p.is_dir();
        match list_dir_images(p) {
            Ok(v) => println!(
                "  [{label}] is_dir={exists} → {} 张（前 3 张文件名：{:?}）",
                v.len(),
                v.iter().take(3).map(|x| x.name.clone()).collect::<Vec<_>>()
            ),
            Err(e) => println!("  [{label}] is_dir={exists} → 错误：{e}"),
        }
    }
}
