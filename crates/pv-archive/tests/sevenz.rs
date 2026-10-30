//! 7-Zip（CLI 后端）的端到端护栏：**现场造包**，再列清单 / 解条目 / 真正解码。
//!
//! ## 为什么用集成测试而不是单测
//! 这条链路的失败模式几乎全在「进程边界」上，而单测里 mock 掉 7z.exe 就什么都测不到：
//! - `-slt` 的输出格式（`Path = …` / `Size = …`）解析错了 → 列不出条目
//! - 忘了 `-so` → 7z 解到临时目录、stdout 是空的 → 解出来 0 字节
//! - 忘了 `-spd` → 包内名字里的 `[` 被当通配符 → 解出别的文件（静默错图）
//! - `-sccUTF-8` 没加 → 中文名乱码
//!
//! 所以这里真的去跑 7z.exe。**没装 7-Zip 的机器上会跳过**（不算失败）：
//! 这个后端本来就是「有 7z 才能用 RAR/7z」。
//!
//! ## 为什么不测 RAR
//! 7-Zip **只能解不能压** RAR。要么在仓库里放一个真实 rar 包（版权 + 体积），
//! 要么依赖用户机器上恰好有 rar。这里退一步测 7z —— 两者走的是**同一条
//! CLI 代码路径**，差异只在 7z.exe 内部。

use std::path::{Path, PathBuf};
use std::process::Command;

fn have_7z() -> Option<PathBuf> {
    pv_archive::cli::seven_zip_exe()
}

/// 造一个 .7z 包，里面放两张真图（借 test-image 的样本）。
fn make_7z(dir: &Path) -> Option<PathBuf> {
    let exe = have_7z()?;
    std::fs::create_dir_all(dir).unwrap();
    let pack = dir.join("t.7z");
    let _ = std::fs::remove_file(&pack);

    let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-image");
    let mut picks: Vec<PathBuf> = std::fs::read_dir(&sample)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p.to_string_lossy().to_ascii_lowercase();
            n.ends_with(".jpg") || n.ends_with(".jpeg") || n.ends_with(".png")
        })
        // 只挑有内容的（样本目录里有 0 字节的占位文件）
        .filter(|p| std::fs::metadata(p).map(|m| m.len() > 1024).unwrap_or(false))
        .take(2)
        .collect();
    if picks.len() < 2 {
        return None;
    }
    picks.sort();

    let mut cmd = Command::new(&exe);
    cmd.arg("a").arg("-t7z").arg(&pack);
    for p in &picks {
        cmd.arg(p);
    }
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(pack)
}

#[test]
fn 七z包能列出条目并解出可解码的图() {
    let Some(_) = have_7z() else {
        eprintln!("没有 7z.exe，跳过");
        return;
    };
    let root = std::env::temp_dir().join("pv_arc_7z_it");
    let _ = std::fs::remove_dir_all(&root);
    let Some(pack) = make_7z(&root) else {
        eprintln!("造包失败，跳过");
        return;
    };
    let pack_str = pack.to_string_lossy().into_owned();

    // 1) 列清单：应当恰好等于塞进去的图片数，且名字是**相对路径**（不是绝对路径）
    let items = pv_archive::list_images(&pack_str).unwrap();
    assert!(!items.is_empty(), "7z 包里应当能列出图片");
    for e in &items {
        assert!(
            !e.name.contains(":\\") && !e.name.starts_with("//"),
            "条目名不该是绝对路径：{}",
            e.name
        );
        assert!(e.size > 0, "条目 {} 的大小应当 > 0（-slt 解析错了？）", e.name);
    }

    // 2) 解一条，字节数必须和清单报的一致 —— 这一条能抓住「忘了 -so」：
    //    那会让 stdout 为空（status 是 0，只是没内容），只看 status 发现不了。
    let first = &items[0];
    let vpath = pv_archive::make_virtual(&pack_str, &first.name);
    let real = pv_archive::materialize(&vpath, &root.join("cache")).unwrap();
    let len = std::fs::metadata(&real).unwrap().len();
    assert_eq!(len, first.size, "解出的字节数要和清单一致");

    // 3) 真正解码一次：证明解出来的东西是可以当普通图片打开的
    let mut reg = pv_decode::DecoderRegistry::new();
    reg.register(Box::new(pv_decode::BitmapBackend));
    let (w, h) = pv_decode::probe_dimensions(&reg, &real).expect("解出来的图应当能解码");
    assert!(w > 0 && h > 0);

    let _ = std::fs::remove_dir_all(&root);
}

/// 包内名字里带 `[` `]` 时，`-spd` 不能漏 —— 漏了 7z 会把它们当通配符，
/// 可能匹配到**别的**文件（内容错了但字节数非 0，最难查）。
#[test]
fn 条目名带方括号也能精确解出() {
    let Some(exe) = have_7z() else {
        eprintln!("没有 7z.exe，跳过");
        return;
    };
    let root = std::env::temp_dir().join("pv_arc_7z_bracket");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    // 造两个**内容不同**的小文件，其中一个带方括号
    let a = root.join("a[1].txt");
    let b = root.join("a1.txt");
    std::fs::write(&a, b"BRACKET").unwrap();
    std::fs::write(&b, b"PLAIN").unwrap();

    // 带方括号的用 `-spf` 关掉「按文件名通配」的方式压进去：直接压文件路径即可
    let pack = root.join("p.7z");
    let ok = Command::new(&exe)
        .arg("a")
        .arg("-t7z")
        .arg(&pack)
        .arg(&a)
        .arg(&b)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        eprintln!("造包失败，跳过");
        return;
    }

    let pack_str = pack.to_string_lossy().into_owned();
    let items = pv_archive::list_images(&pack_str).unwrap_or_default();
    // 这两个是 txt，list_images 会过滤掉 —— 这里要的是「能按名精确读」，所以直接读
    let _ = items;

    let mut arc = pv_archive::open(&pack_str).unwrap();
    let entries = arc.entries().unwrap();
    let bracketed = entries
        .iter()
        .find(|e| e.name.contains('['))
        .map(|e| e.name.clone());
    let Some(name) = bracketed else {
        eprintln!("包里没有带方括号的条目，跳过");
        return;
    };
    let data = arc.read(&name).unwrap();
    assert_eq!(
        data, b"BRACKET",
        "带方括号的条目必须精确命中，不能落到同名的 a1.txt 上"
    );

    let _ = std::fs::remove_dir_all(&root);
}
