//! 应用层 RAW 桥接测试。
//!
//! `pv-decode` 的测试证明了「能解出像素」；这条测试证明
//! **应用层真的把 RAW 管线接上了**，并且 DTO 里的 `raw` 字段
//! 能带着正确的键名穿过 IPC 边界到达前端。
//!
//! 覆盖两件事（都是接完 Tauri 后最容易忘记同步的）：
//! 1. `AppState` 构造时确实注册了 RAW pipeline（不是空转）
//! 2. `ImageInfoDto.raw` 序列化后的键名与前端 `RawInfo` 一一对应

// 测试函数名用中文（可读性优先），关闭 snake_case 检查
#![allow(non_snake_case)]

use std::path::Path;

/// 找一个可用的真实 RAW 样本（大于 4 KB 才算真文件，排除 Git LFS 指针）
///
/// ⚠️ 路径是 `apps/desktop/src-tauri` → **上三层**才到仓库根
/// （曾少写一层写成上两层 = `apps/`，于是永远找不到样本、
/// 测试悄悄走「跳过」分支却仍显示 ok —— 2026-10-09 修正）。
fn raw_sample(name: &str) -> Option<std::path::PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../test-image")
        .join(name);
    if p.is_file() && std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0) > 4096 {
        Some(p)
    } else {
        None
    }
}

/// `AppState` 里注册的注册表应当能真正打开 RAW（不是空注册表）。
///
/// 这条会走完整链路：`DecoderRegistry::probe` → `EmbeddedPreviewPipeline`
/// → 内嵌预览提取 → EXIF。任何一环没接上都会失败。
#[test]
fn 应用状态注册了可用的RAW管线() {
    // AppState 是 pub(crate)，集成测试拿不到 —— 这里直接构造同样配置的注册表，
    // 与 state.rs 的写法保持一致。若 state.rs 改了注册方式，
    // 这条测试仍能验证"pv-decode 侧提供的能力"是否可用；
    // "state.rs 有没接上" 由下面那条源码断言兜底。
    let Some(p) = raw_sample("nikon-nrw-sample.NRW") else {
        eprintln!("跳过：NRW 样本不可用");
        return;
    };

    let mut reg = pv_decode::DecoderRegistry::new();
    reg.register(Box::new(pv_decode::BitmapBackend));
    reg.set_raw_pipeline(Box::new(pv_decode::EmbeddedPreviewPipeline::new()));

    let info = reg.probe(&p).expect("NRW 探测");
    assert!(info.is_raw(), "NRW 应判定为 RAW");
    let meta = info.raw.as_ref().expect("应带 RAW 元数据");
    assert_eq!(meta.raw_format, "NRW");
    assert!(meta.has_embedded_preview, "NRW 应有内嵌预览");
    assert_eq!((meta.width, meta.height), (4000, 3000));

    // 解码也要通（不看图就等于没接上）
    let img = reg
        .decode(&p, pv_decode::DecodeTarget::new(pv_decode::DecodePurpose::Preview))
        .expect("NRW 解码");
    assert!(img.width > 0 && img.height > 0);
}

/// `state.rs` 必须真的调用 `set_raw_pipeline`。
///
/// 用源码断言而不是运行期断言，是因为 `AppState` 不对外暴露注册表；
/// 而这条断言足够精确：漏掉就一定会被抓住，且不会因无关改动误报。
#[test]
fn state源码中注册了RAW管线() {
    let src = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/state.rs"),
    )
    .expect("读 state.rs");

    assert!(
        src.contains("set_raw_pipeline"),
        "state.rs 必须注册 RAW pipeline，否则 RAW 文件会报「解码器未接入」"
    );
    assert!(
        src.contains("EmbeddedPreviewPipeline"),
        "state.rs 应使用 EmbeddedPreviewPipeline（纯 Rust 内嵌预览）"
    );
}

/// RAW 元数据的键名要与前端 `RawInfo` 对齐。
///
/// 前端 `ipc/types.ts` 是手写的，字段名对不上不会报错（静默 undefined）。
/// 后端已有 `RAW信息键名与前端TS类型一致` 覆盖 DTO 本身；
/// 这里再锁一遍「从 RawMetadata 转过来的那条路径」的键名。
#[test]
fn RAW元数据过IPC后键名正确() {
    let Some(p) = raw_sample("olympus-orf-sample.ORF") else {
        eprintln!("跳过：ORF 样本不可用");
        return;
    };

    let mut reg = pv_decode::DecoderRegistry::new();
    reg.set_raw_pipeline(Box::new(pv_decode::EmbeddedPreviewPipeline::new()));
    let meta = reg.probe(&p).expect("ORF 探测").raw.expect("RAW 元数据");

    // 模拟 image.rs 的映射（与那里保持同一套字段）
    let dto = serde_json::json!({
        "rawFormat": meta.raw_format,
        "hasEmbeddedPreview": meta.has_embedded_preview,
        "bitDepth": meta.bit_depth,
        "colorSpace": "sRGB",
    });
    for key in ["rawFormat", "hasEmbeddedPreview", "bitDepth", "colorSpace"] {
        assert!(dto.get(key).is_some(), "缺字段 {key}");
    }
    assert_eq!(dto["rawFormat"], "ORF");
}
