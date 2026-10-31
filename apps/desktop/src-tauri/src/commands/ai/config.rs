//! AI 阈值 / 开关的读写。
//!
//! ⭐ **默认值的唯一真源在 `pv_ai::batch`**（`DEFAULT_THRESHOLD` 等）：
//! 这里的 `DEF_*` 只是把它转成 `f64` 给界面用。⛔ 别再写第二份字面量 ——
//! 2026-11 之前面板写 0.40/0.36、引擎按 0.39/0.35 跑，用户只能靠数标签才发现。

use serde::Serialize;
use tauri::State;

use crate::commands::dto::ErrorDto;
use crate::state::AppState;

use super::err;
use super::vocab::{disabled_terms, parse_vocab};

pub(super) const KEY_THRESHOLD: &str = "ai.threshold";
/// **一级类目**的准入门槛（比二级低：粗分类更容易命中）。
pub(super) const KEY_THRESHOLD_L1: &str = "ai.l1Threshold";
pub(super) const KEY_MAX_TAGS: &str = "ai.max_tags";
pub(super) const KEY_FACES: &str = "ai.faces";
pub(super) const KEY_DISABLED: &str = "ai.disabled_terms";

/// 默认阈值 —— **必须与引擎实际使用的缺省值一致**。
///
/// ⚠️ 2026-11 修：这两个数以前各写一份字面量（这里是 0.40 / 0.36，Rust 打标
/// 那边是 0.39 / 0.35），而 `f9127c1` 把打标切到 Rust 时只改了一处 ——
/// 结果是**没动过设置的用户，面板显示 0.40，实际按 0.39 跑**。
/// 阈值差 0.01 就意味着「界面说会打上的标签，实际没打」，属于静默偏差，
/// 用户只能靠数标签才发现。
/// 现在统一从 `pv_ai::batch` 的默认值派生（那里有校准依据）。
pub(super) const DEF_THRESHOLD: f64 = pv_ai::batch::DEFAULT_THRESHOLD as f64;
/// 一级类目默认门槛，同上。
pub(super) const DEF_THRESHOLD_L1: f64 = pv_ai::batch::DEFAULT_THRESHOLD_L1 as f64;
/// 每张标签数的**上限**（不是目标）：够格几个就几个，认不出就空着
pub(super) const DEF_MAX_TAGS: i64 = pv_ai::batch::DEFAULT_MAX_TAGS as i64;

/// ⚠️ 必须 `camelCase`：前端按 `vocabCount` / `maxTags` 读，
/// 字段名对不上时 JS 拿到 `undefined`，界面上直接显示成「NaN 词」。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiConfigDto {
    /// 模型标识（展示用）
    pub model: String,
    /// 词表条数（未扣禁用）
    pub vocab_count: usize,
    /// 被用户禁用的条数
    pub disabled_count: usize,
    /// 二级（具体词）门槛
    pub threshold: f64,
    /// 一级（类目）门槛，通常低于 `threshold`
    pub threshold_l1: f64,
    pub max_tags: i64,
    pub faces: bool,
}

/// 读取 AI 配置（缺省值写在前端可见的常量里，别让界面显示空）。
#[tauri::command]
pub async fn ai_config(state: State<'_, AppState>) -> Result<AiConfigDto, ErrorDto> {
    let vocab = parse_vocab();
    let disabled = disabled_terms(&state);
    let (threshold, threshold_l1, max_tags, faces) = state
        .with_manager(|s| {
            let get = |k: &str| s.setting(k).map_err(|e| e.to_string());
            Ok((
                get(KEY_THRESHOLD)
                    .ok()
                    .flatten()
                    .and_then(|v| v.parse::<f64>().ok())
                    .unwrap_or(DEF_THRESHOLD),
                get(KEY_THRESHOLD_L1)
                    .ok()
                    .flatten()
                    .and_then(|v| v.parse::<f64>().ok())
                    .unwrap_or(DEF_THRESHOLD_L1),
                get(KEY_MAX_TAGS)
                    .ok()
                    .flatten()
                    .and_then(|v| v.parse::<i64>().ok())
                    .unwrap_or(DEF_MAX_TAGS),
                get(KEY_FACES)
                    .ok()
                    .flatten()
                    .map(|v| v == "1")
                    .unwrap_or(true),
            ))
        })
        .map_err(err)?;

    Ok(AiConfigDto {
        model: "CN-CLIP ViT-B/16".into(),
        vocab_count: vocab.len(),
        disabled_count: disabled.len(),
        threshold,
        threshold_l1,
        max_tags,
        faces,
    })
}

/// 保存 AI 配置（只写传进来的字段）。
#[tauri::command]
pub async fn ai_save_config(
    state: State<'_, AppState>,
    threshold: Option<f64>,
    threshold_l1: Option<f64>,
    max_tags: Option<i64>,
    faces: Option<bool>,
) -> Result<(), ErrorDto> {
    state.with_manager(|s| {
        if let Some(v) = threshold {
            s.set_setting(KEY_THRESHOLD, &format!("{v:.2}")).map_err(|e| e.to_string())?;
        }
        if let Some(v) = threshold_l1 {
            s.set_setting(KEY_THRESHOLD_L1, &format!("{v:.2}")).map_err(|e| e.to_string())?;
        }
        if let Some(v) = max_tags {
            s.set_setting(KEY_MAX_TAGS, &v.to_string()).map_err(|e| e.to_string())?;
        }
        if let Some(v) = faces {
            s.set_setting(KEY_FACES, if v { "1" } else { "0" }).map_err(|e| e.to_string())?;
        }
        Ok(())
    })
    .map_err(err)
}

pub(super) fn read_f32(state: &AppState, key: &str, def: f32) -> f32 {
    state
        .with_manager(|s| Ok(s.setting(key).ok().flatten()))
        .ok()
        .flatten()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(def)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 配置字段必须是camelCase() {
        let dto = AiConfigDto {
            model: "m".into(),
            vocab_count: 385,
            disabled_count: 0,
            threshold: 0.35,
            threshold_l1: 0.36,
            max_tags: 8,
            faces: true,
        };
        let j = serde_json::to_string(&dto).unwrap();
        for k in ["vocabCount", "disabledCount", "thresholdL1", "maxTags"] {
            assert!(j.contains(k), "缺字段 {k}：{j}");
        }
    }
}
