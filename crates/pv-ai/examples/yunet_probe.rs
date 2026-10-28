//! 探测 YuNet 的输入输出布局（写后处理之前先看清楚）。

use ort::ep::DirectML;
use ort::session::Session;
use ort::value::Tensor;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/ai/face/yunet.onnx");
    let env = ort::init()
        .with_name("yunet-probe")
        .with_execution_providers([DirectML::default().build()])
        .build()?;
    let mut session = Session::builder(&env)?.commit_from_file(model)?;

    for i in session.inputs() {
        println!("输入 {} → {:?}", i.name(), i.dtype());
    }
    for o in session.outputs() {
        println!("输出 {} → {:?}", o.name(), o.dtype());
    }

    // 640×640、RGB、0–255（OpenCV 的 blobFromImage(1.0, Scalar(), true, false)）
    let shape = [1i64, 3, 640, 640];
    let data = vec![0f32; 3 * 640 * 640];
    let t = Tensor::from_array((shape, data))?;
    let outputs = session.run(ort::inputs!["input" => t])?;

    println!("\n实际输出张量：");
    for (name, v) in outputs.iter() {
        match v.try_extract_tensor::<f32>() {
            Ok((s, d)) => println!("  {name}: shape={s:?} len={}", d.len()),
            Err(_) => println!("  {name}: 非 f32"),
        }
    }
    Ok(())
}
