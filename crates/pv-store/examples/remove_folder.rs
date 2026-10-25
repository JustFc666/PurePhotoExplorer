//! 运维工具：**列出 / 移除图库里的目录**（从命令行）。
//!
//! ```text
//! cargo run -p pv-store --example remove_folder -- --list          # 只看，不改
//! cargo run -p pv-store --example remove_folder -- <folder_id>     # 移除该目录及其子树
//! cargo run -p pv-store --example remove_folder -- [数据目录] <folder_id>
//! ```
//! 数据目录缺省为 `%LOCALAPPDATA%\PurePhotoExplorer`。
//!
//! ## 为什么要有它
//! App 里「右键 → 从图库移除」在误扫了一大片目录之后**点不过来**
//! （本机 `F:\` 下面有 2000+ 个子目录行）。这是同一个
//! [`Store::remove_folder`] 的命令行入口 —— **同一条代码路径、同一套护栏测试**，
//! 不会因为换了入口就漏删附属表或误删原图。
//!
//! ⚠️ **只删索引与缩略图缓存，绝不碰原始图片文件。**
//! ⚠️ 跑之前请**关掉 App**：它会把索引整体读进内存，外部改了它不会知道。

#[cfg(not(windows))]
fn main() {
    eprintln!("本示例目前仅支持 Windows（卷身份用 Win32 实现）");
}

#[cfg(windows)]
fn main() {
    use pv_store::{Store, WindowsVolumeResolver};
    use std::path::PathBuf;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let list_only = args.iter().any(|a| a == "--list");
    let nums: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();

    // 一个纯数字参数 = folder_id，其余当数据目录
    let (data_dir, folder_id): (PathBuf, Option<i64>) = match nums.as_slice() {
        [] => (default_data_dir(), None),
        [one] => match one.parse::<i64>() {
            Ok(id) => (default_data_dir(), Some(id)),
            Err(_) => (PathBuf::from(one.as_str()), None),
        },
        [dir, id] => (
            PathBuf::from(dir.as_str()),
            Some(id.parse().expect("第二个参数要是 folder_id")),
        ),
        _ => {
            eprintln!("参数太多。用法见文件头注释。");
            std::process::exit(2);
        }
    };

    println!("数据目录：{}", data_dir.display());
    let mut store = Store::open(&data_dir, Box::new(WindowsVolumeResolver)).expect("打开服务层失败");

    let folders = store.folders().expect("读目录表失败");

    // 子树规模：用 parent_id 建 children 索引后在内存里汇总
    let mut children: std::collections::HashMap<i64, Vec<i64>> = std::collections::HashMap::new();
    for f in &folders {
        if let Some(p) = f.parent_id {
            children.entry(p).or_default().push(f.id);
        }
    }
    let subtree = |root: i64| -> (u64, i64) {
        let mut dirs = 0u64;
        let mut imgs = 0i64;
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            dirs += 1;
            if let Some(n) = folders.iter().find(|f| f.id == id) {
                imgs += n.image_count;
            }
            if let Some(k) = children.get(&id) {
                stack.extend(k.iter().copied());
            }
        }
        (dirs, imgs)
    };

    println!("\n=== 图库里「显式加入」的目录（is_root）===");
    for f in folders.iter().filter(|f| f.is_root) {
        let (dirs, imgs) = subtree(f.id);
        let mount = f.mount_path.as_deref().unwrap_or("?");
        let rel = f.rel_path.replace('/', "\\");
        println!(
            "  id={:<6} 子树 {dirs:>5} 个目录 / {imgs:>6} 张   {mount}{rel}",
            f.id
        );
    }

    let Some(id) = folder_id else {
        if !list_only {
            println!("\n（没给 folder_id，只列出来看看。要移除就传 id）");
        }
        return;
    };

    let (dirs, imgs) = subtree(id);
    println!("\n即将移除 id={id}：{dirs} 个目录行 / {imgs} 张索引记录（只删索引与缓存，不删原图）");
    let r = store.remove_folder(id).expect("移除失败");
    println!(
        "完成：目录 {} · 照片 {} · 缓存文件 {}{}",
        r.folders,
        r.images,
        r.thumbs,
        if r.thumbs_orphan > 0 {
            format!(" · 离线未清缓存 {}", r.thumbs_orphan)
        } else {
            String::new()
        }
    );
    let left = store.count(&pv_store::ImageFilter::default()).unwrap_or(-1);
    println!("剩余索引记录：{left} 张");
}

#[cfg(windows)]
fn default_data_dir() -> std::path::PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("PurePhotoExplorer")
}
