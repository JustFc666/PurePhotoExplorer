//! PurePhotoExplorer AI：CN-CLIP 零样本打标（Rust + ONNX Runtime + DirectML）。
//!
//! ⚠️ 目前**只做打标**。人脸（YuNet/SFace）的 Rust 实现在 S3 补上。
//!
//! 为什么不再走 Python：发行版要「装上就能用」，而 Python 方案要求用户
//! 自己装 torch（2.5GB，还得按显卡选 CUDA/ROCm 版本）—— 对分发是灾难。
//! 现在一份 `onnxruntime` + `DirectML.dll` 就覆盖 AMD/NVIDIA/Intel 所有 DX12 显卡。

pub mod batch;
pub mod face;
pub mod tagger;
