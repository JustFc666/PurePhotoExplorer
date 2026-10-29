//! 全局 ORT 环境（**每个进程只能有一个**）。
//!
//! ⛔ `ort::init().build()` 第二次调用会直接报
//! `only one environment is allowed per process` —— ort 内部用 `static CURRENT_ENV` 守住了。
//!
//! 我们有三个模型（CN-CLIP 视觉塔 / YuNet / SFace），原来每个 `load` 各自 init 一次，
//! 于是「先打标再识别」或者「检测+识别同时加载」就炸。
//! 所以统一从这里取：第一次 build，之后复用同一个环境。
//!
//! ⚠️ DirectML EP 是**环境级**注册的：只要建成功了，后面所有 session 都能用。
//! 所以这里固定带上 DirectML，不需要每个模型再单独 register。

use std::sync::OnceLock;

use ort::environment::Environment;
use ort::ep::DirectML;

static ENV: OnceLock<Environment> = OnceLock::new();

/// 取（必要时创建）全局 ORT 环境。
pub fn ort_env() -> Result<&'static Environment, String> {
    if let Some(e) = ENV.get() {
        return Ok(e);
    }
    let built = ort::init()
        .with_name("pve-ai")
        .with_execution_providers([DirectML::default().build()])
        .build()
        .map_err(|e| format!("ORT 环境初始化失败：{e}"))?;
    Ok(ENV.get_or_init(|| built))
}
