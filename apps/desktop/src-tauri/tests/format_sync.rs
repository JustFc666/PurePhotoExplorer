//! **回归护栏**：Rust 能力表与前端格式清单必须一致。
//!
//! 「能打开哪些格式」这件事有两份清单：
//! 1. 前端 `apps/desktop/src/constants/formats.ts` —— 打开对话框的过滤器、
//!    帮助面板的「打开方式」勾选项都从它派生（用户**看得见**的那份）；
//! 2. Rust `commands/app.rs::supported_formats()` —— `enabled` 表示**真有解码器**。
//!
//! 这两边**没有代码级的绑定**，只能靠测试盯着。曾经 HEIC 就是这样不一致：
//! 魔数探测早就有了、前端把它列进「所有**支持的**图片」，但解码器没接 ——
//! 用户能选中，选完只看到「暂不支持」。同一形态还出现了两次（`bmp`）。
//!
//! 所以这里做**单向**断言：**能力表里标记为可用的每个格式，都必须出现在
//! 前端清单里**。反向断言不成立，因为两者粒度不同（`jpg`/`jpeg` 是同一格式
//! 的两个扩展名、`heif` 与 `heic` 同属 HEIF，能力表只列一个）。

use std::path::{Path, PathBuf};

use photovault_lib::ipc_protocol::supported_formats;

/// 仓库根的 `test-image` 同级 —— 前端源码所在目录
fn frontend_formats_path() -> PathBuf {
    // CARGO_MANIFEST_DIR = apps/desktop/src-tauri
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("apps/desktop")
        .join("src/constants/formats.ts")
}

/// 从 `SUPPORTED_FORMATS` 数组里抓出扩展名。
///
/// 刻意不用正则：那段是规整的 `  ['ext', '标签'],` 一行一项，
/// 逐行取引号里的第一段就够了，也免得为一条测试引入 regex 依赖。
fn parse_supported_exts(text: &str) -> Vec<String> {
    let start = text
        .find("export const SUPPORTED_FORMATS")
        .expect("formats.ts 里应有 SUPPORTED_FORMATS —— 是不是改名了？");
    let body = &text[start..];
    let end = body.find("\n]").expect("SUPPORTED_FORMATS 数组应有收尾的 `]`");
    let block = &body[..end];

    let mut exts = Vec::new();
    for line in block.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('[') {
            continue;
        }
        // 形如 `['jpg', 'JPEG'],` —— 取第一个单引号对
        let Some(q1) = trimmed.find('\'') else { continue };
        let rest = &trimmed[q1 + 1..];
        let Some(q2) = rest.find('\'') else { continue };
        exts.push(rest[..q2].to_ascii_lowercase());
    }
    exts
}

#[test]
fn 能力表里可用的格式都出现在前端清单里() {
    let path = frontend_formats_path();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("应能读到 {}：{e}", path.display()));
    let listed = parse_supported_exts(&text);
    assert!(
        listed.len() >= 10,
        "只解析出 {} 个扩展名，解析逻辑可能失效了：{listed:?}",
        listed.len()
    );

    let missing: Vec<String> = supported_formats()
        .into_iter()
        .filter(|f| f.enabled)
        .map(|f| f.id.to_ascii_lowercase())
        .filter(|id| !listed.contains(id))
        .collect();

    assert!(
        missing.is_empty(),
        "这些格式在 Rust 能力表里标了 enabled=true，却没进前端清单 {missing:?} ——\n\
         用户会在打开对话框里看不到它们（或者反过来被列进\"支持的图片\"却打不开）。\n\
         请把 {} 里补上对应的扩展名，或把能力表的 enabled 改回 false。",
        path.display()
    );
}

/// 解析函数本身要靠谱 —— 否则上面那条测试可能因为「一个都没解析出来」而假通过。
#[test]
fn 前端清单解析器能认出已知格式() {
    let text = std::fs::read_to_string(frontend_formats_path()).expect("应能读到 formats.ts");
    let exts = parse_supported_exts(&text);
    for want in ["jpg", "jpeg", "png", "gif", "webp", "heic", "heif", "psd", "cr3", "dng"] {
        assert!(
            exts.contains(&want.to_string()),
            "解析结果里缺 {want}，实际：{exts:?}"
        );
    }
    // 不该把标签当成扩展名
    assert!(!exts.iter().any(|e| e == "jpeg 图片" || e == "JPEG"), "抓错了字段：{exts:?}");
}
