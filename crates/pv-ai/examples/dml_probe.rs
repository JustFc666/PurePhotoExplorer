//! **S0 spike**：ort + DirectML 能不能在本机真跑推理。
//!
//! ```text
//! cargo run --release -p pv-ai --example dml_probe
//! ```
//!
//! 判据（都过才算通）：
//! 1. `DirectML::is_available()` 为 true —— ORT **编译期**带了 DirectML provider
//! 2. 环境初始化成功 —— 运行时能加载 `DirectML.dll`、拿到 DX12 设备
//! 3. 建图成功 + **真的跑出结果**，且重复推理耗时明显低于纯 CPU
//!
//! ⚠️ 第 1 步就 false 说明用的是 **CPU 版** onnxruntime.dll：
//! DirectML provider 是编译进 ORT 的，不是运行时插的，只有换 dll 才有效。
//!
//! ⚠️ ORT 在 DML 初始化失败时可能**静默回退 CPU**（只在日志里 warning），
//! 所以必须看耗时，不能只看「没报错」。

use std::time::Instant;

use ort::ep::{DirectML, ExecutionProvider};
use ort::session::Session;
use ort::value::Tensor;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/ai/face/yunet.onnx");
    println!("模型: {model}");
    println!("ORT API 版本: {}", ort::MINOR_VERSION);
    println!("DirectML 编译期支持: {:?}", DirectML::is_available());

    let env = ort::init()
        .with_name("pve-dml-probe")
        .with_execution_providers([DirectML::default().build()])
        .build()?;

    let mut session = Session::builder(&env)?.commit_from_file(model)?;
    let ins: Vec<String> = session
        .inputs()
        .iter()
        .map(|i| i.name().to_string())
        .collect();
    let outs = session.outputs().len();
    println!("输入 {ins:?} · 输出 {outs} 个");

    // YuNet 输入固定 1×3×H×W（这个版本写死 640×640，不是动态）
    let (h, w) = (640usize, 640usize); // YuNet 2023mar 固定 640×640
    let shape = [1i64, 3, h as i64, w as i64];

    // 首次含图优化 / 显存分配，单独计时
    let tensor = Tensor::from_array((shape, vec![0.5f32; 3 * h * w]))?;
    let t0 = Instant::now();
    let _ = session.run(ort::inputs!["input" => tensor])?;
    println!(
        "首次推理: {:.1} ms（含预热）",
        t0.elapsed().as_secs_f64() * 1000.0
    );

    // 稳态：再跑 5 次取平均 —— 这个数字才能判断是不是真在 GPU 上
    const N: u32 = 5;
    let t1 = Instant::now();
    for _ in 0..N {
        let tensor = Tensor::from_array((shape, vec![0.5f32; 3 * h * w]))?;
        let _ = session.run(ort::inputs!["input" => tensor])?;
    }
    let avg = t1.elapsed().as_secs_f64() * 1000.0 / N as f64;
    println!("稳态平均: {avg:.1} ms / 次（{N} 次）");

    if avg > 60.0 {
        println!("\n⚠️ 平均 {avg:.0}ms 偏高，很可能已回退 CPU EP —— 查 DirectML.dll 与显卡驱动");
    } else {
        println!("\n✅ S0 通过：ort + DirectML 跑通真实推理");
    }
    Ok(())
}
