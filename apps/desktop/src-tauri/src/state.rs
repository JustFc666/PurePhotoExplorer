//! 全局应用状态。
//!
//! - 解码器注册表在启动时构建一次，之后只读共享（`Arc` + `Send + Sync`）。
//! - 管理器的 `Store`（S2）**惰性打开**：首次调用 `manager_open` 前为 `None`。
//!   `rusqlite::Connection` 不是 `Sync`，故整体放在 `Mutex` 里串行访问
//!   （管理器的调用频率低，串行没有瓶颈；也顺带避免了 `&mut` 与并发写冲突）。

use std::path::Path;
use std::sync::{Arc, Mutex};

use pv_decode::{BitmapBackend, DecoderRegistry, EmbeddedPreviewPipeline};
use pv_store::{Store, WindowsVolumeResolver};

/// 全局解码器注册表。
pub struct AppState {
    registry: Arc<DecoderRegistry>,
    /// 管理器：`None` = 尚未打开数据目录
    manager: Mutex<Option<Store>>,
}

impl AppState {
    pub fn new() -> Self {
        let mut registry = DecoderRegistry::new();
        registry.register(Box::new(BitmapBackend));
        // RAW：纯 Rust 内嵌预览提取（不依赖 LibRaw，见 crates/pv-decode/src/raw）。
        registry.set_raw_pipeline(Box::new(EmbeddedPreviewPipeline::new()));
        Self {
            registry: Arc::new(registry),
            manager: Mutex::new(None),
        }
    }

    /// 只读访问解码器注册表
    pub(crate) fn registry(&self) -> &DecoderRegistry {
        &self.registry
    }

    /// `Arc` 克隆：批量命令要把它挪进 `spawn_blocking` 闭包（需要 `'static`）。
    pub(crate) fn registry_arc(&self) -> Arc<DecoderRegistry> {
        Arc::clone(&self.registry)
    }

    /// 打开（或替换）管理器。返回实际使用的数据目录。
    pub(crate) fn open_manager(&self, data_dir: &Path) -> Result<(), String> {
        let store = Store::open(data_dir, Box::new(WindowsVolumeResolver))
            .map_err(|e| format!("打开管理器失败：{e}"))?;
        let mut g = self.manager.lock().map_err(|_| "状态锁中毒".to_string())?;
        *g = Some(store);
        Ok(())
    }

    /// 只读使用管理器。
    pub(crate) fn with_manager<T>(
        &self,
        f: impl FnOnce(&Store) -> Result<T, String>,
    ) -> Result<T, String> {
        let g = self.manager.lock().map_err(|_| "状态锁中毒".to_string())?;
        let store = g
            .as_ref()
            .ok_or_else(|| "管理器尚未打开（请先调用 manager_open）".to_string())?;
        f(store)
    }

    /// 需要 `&mut` 的操作（如扫描）。
    pub(crate) fn with_manager_mut<T>(
        &self,
        f: impl FnOnce(&mut Store) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut g = self.manager.lock().map_err(|_| "状态锁中毒".to_string())?;
        let store = g
            .as_mut()
            .ok_or_else(|| "管理器尚未打开（请先调用 manager_open）".to_string())?;
        f(store)
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
