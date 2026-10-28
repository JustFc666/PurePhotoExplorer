//! **S1 spike**：CN-CLIP 视觉塔（fp16, 172MB）能不能在 DirectML 上跑，多快。
//!
//! ```text
//! cargo run --release -p pv-ai --example clip_probe
//! ```
//!
//! 前置：先跑 `python tools/ai/export_clip_onnx.py` 生成
//! `assets/ai/tag/clip_vision.onnx` + `text_embeds.bin` + `tag_meta.json`。
//!
//! 关注三件事：
//! 1. fp16 模型在 DML 上能不能建图（有些算子 fp16 会失败）
//! 2. 单张 224×224 的耗时（ViT-B/16 在 7800XT 上应在 10–20ms）
//! 3. 输出是不是 512 维
//!
//! ⚠️ 第一次跑会把 172MB 权重读进显存，首个 forward 明显慢（含分配 + 图优化），
//! 所以要看「稳态平均」而不是「首次」。

use std::time::Instant;

use half::f16;
use ort::ep::DirectML;
use ort::session::Session;
use ort::value::Tensor;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/ai/tag/clip_vision.onnx"
    );
    if !std::path::Path::new(model).exists() {
        eprintln!("缺模型：{model}\n先跑 python tools/ai/export_clip_onnx.py");
        return Ok(());
    }

    let env = ort::init()
        .with_name("pve-clip-probe")
        .with_execution_providers([DirectML::default().build()])
        .build()?;
    let mut session = Session::builder(&env)?.commit_from_file(model)?;

    let ins: Vec<String> = session
        .inputs()
        .iter()
        .map(|i| i.name().to_string())
        .collect();
    let outs: Vec<String> = session
        .outputs()
        .iter()
        .map(|o| o.name().to_string())
        .collect();
    println!("输入 {ins:?} · 输出 {outs:?}");

    let shape = [1i64, 3, 224, 224];
    let mk = || -> Vec<f16> { vec![f16::ZERO; 3 * 224 * 224] };

    let t0 = Instant::now();
    // 注意作用域：run 的返回值持有 session 的可变借用，必须先 drop 再跑下一轮
    let first = {
        let r = session.run(ort::inputs!["pixel_values" => Tensor::from_array((shape, mk()))?]);
        match r {
            Ok(o) => Ok(o.len()),
            Err(e) => Err(e.to_string()),
        }
    };
    let first_ms = t0.elapsed().as_secs_f64() * 1000.0;
    match first {
        Ok(n) => println!("首次推理: {first_ms:.1} ms（含预热）· 输出张量 {n} 个"),
        Err(e) => {
            println!("❌ DirectML 跑 fp16 模型失败：{e}");
            println!("   → 可能某些算子不支持 fp16，用 `--fp32` 重新导出再试");
            return Ok(());
        }
    }

    const N: u32 = 5;
    let t1 = Instant::now();
    for _ in 0..N {
        let _ = session.run(ort::inputs!["pixel_values" => Tensor::from_array((shape, mk()))?])?;
    }
    let avg = t1.elapsed().as_secs_f64() * 1000.0 / N as f64;
    println!("稳态平均: {avg:.1} ms / 张（{N} 次）");

    // 1000 张的粗估
    println!("→ 1000 张约 {:.1} 分钟（不含解码）", avg * 1000.0 / 60_000.0);
    Ok(())
}
