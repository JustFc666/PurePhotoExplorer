//! 探测 SFace 的输入输出布局（写对齐与特征提取之前先看清楚）。

use ort::ep::DirectML;
use ort::session::Session;
use ort::value::Tensor;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/ai/face/sface.onnx");
    let env = ort::init()
        .with_name("sface-probe")
        .with_execution_providers([DirectML::default().build()])
        .build()?;
    let mut session = Session::builder(&env)?.commit_from_file(model)?;

    for i in session.inputs() {
        println!("输入 {} → {:?}", i.name(), i.dtype());
    }
    for o in session.outputs() {
        println!("输出 {} → {:?}", o.name(), o.dtype());
    }

    // 112×112、RGB、0–1（OpenCV: blobFromImage(1/255, Scalar(), true, false)）
    let shape = [1i64, 3, 112, 112];
    let data = vec![0.5f32; 3 * 112 * 112];
    let t = Tensor::from_array((shape, data))?;
    let outputs = session.run(ort::inputs!["data" => t])?;
    for (name, v) in outputs.iter() {
        match v.try_extract_tensor::<f32>() {
            Ok((s, d)) => {
                let norm: f32 = d.iter().map(|x| x * x).sum::<f32>().sqrt();
                println!("  {name}: shape={s:?} len={} L2={norm:.4}", d.len());
            }
            Err(_) => println!("  {name}: 非 f32"),
        }
    }
    Ok(())
}
