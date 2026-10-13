//! 全局应用状态。
//!
//! 解码器注册表在应用启动时构建一次，之后只读共享（`Arc` + `Send + Sync`）。

use std::sync::Arc;

use pv_decode::{BitmapBackend, DecoderRegistry, EmbeddedPreviewPipeline};

/// 全局解码器注册表。
pub struct AppState {
    registry: Arc<DecoderRegistry>,
}

impl AppState {
    pub fn new() -> Self {
        let mut registry = DecoderRegistry::new();
        registry.register(Box::new(BitmapBackend));
        // RAW：纯 Rust 内嵌预览提取（不依赖 LibRaw，见 crates/pv-decode/src/raw）。
        // Stage 1 已可用（看图 + 拍摄参数）；`develop()` 仍返回未实现，
        // 待后续接真解码时只需替换这个 Box 的实现。
        registry.set_raw_pipeline(Box::new(EmbeddedPreviewPipeline::new()));
        Self {
            registry: Arc::new(registry),
        }
    }

    /// 只读访问解码器注册表
    pub(crate) fn registry(&self) -> &DecoderRegistry {
        &self.registry
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
