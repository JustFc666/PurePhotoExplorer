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

/* ── 人脸四档（2026-11 从硬编码提到面板可调）──
 * ⛔ 键名统一 `face.` 前缀，与打标的 `ai.` 分开：人脸阈值改动必须重跑识别，
 * 混在一起容易被「重新打标」误当成已生效。
 */
pub(super) const KEY_FACE_SCORE: &str = "face.score_thresh";
pub(super) const KEY_FACE_MIN_PX: &str = "face.min_face_px";
pub(super) const KEY_FACE_T_CORE: &str = "face.t_core";
pub(super) const KEY_FACE_MIN_CORE: &str = "face.min_core";

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

/// 人脸四档缺省值 —— 同样派生自 `pv_ai`，⛔ 别在这里另写字面量。
pub(super) const DEF_FACE_SCORE: f64 = pv_ai::face_batch::DEFAULT_SCORE_THRESH as f64;
pub(super) const DEF_FACE_MIN_PX: f64 = pv_ai::face_batch::DEFAULT_MIN_FACE_PX as f64;
pub(super) const DEF_FACE_T_CORE: f64 = pv_ai::cluster::DEFAULT_T_CORE as f64;
pub(super) const DEF_FACE_MIN_CORE: i64 = pv_ai::cluster::DEFAULT_MIN_CORE as i64;

/// 缺省值本身也发给前端 —— 面板要在刻度上标「默认」，
/// 前端若自己写一份字面量，后端一调默认值就又会出现「面板说 0.60、实际 0.65」。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiDefaultsDto {
    pub face_score_thresh: f64,
    pub face_min_face_px: f64,
    pub face_t_core: f64,
    pub face_min_core: i64,
}

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
    /// 人脸检测置信度（YuNet）：低于它的不当作人脸
    pub face_score_thresh: f64,
    /// 脸宽像素下限（解码后坐标）：小脸向量不可信，直接丢
    pub face_min_face_px: f64,
    /// 聚类第一阶段阈值：越高越严（人物分得更细）
    pub face_t_core: f64,
    /// 建人物所需的最少张数
    pub face_min_core: i64,
    /// 上述四档的出厂默认值（面板用来标「默认」）
    pub defaults: AiDefaultsDto,
}

/// 读取 AI 配置（缺省值写在前端可见的常量里，别让界面显示空）。
#[tauri::command]
pub async fn ai_config(state: State<'_, AppState>) -> Result<AiConfigDto, ErrorDto> {
    let vocab = parse_vocab();
    let disabled = disabled_terms(&state);

    Ok(AiConfigDto {
        model: "CN-CLIP ViT-B/16".into(),
        vocab_count: vocab.len(),
        disabled_count: disabled.len(),
        threshold: read_f64(&state, KEY_THRESHOLD, DEF_THRESHOLD),
        threshold_l1: read_f64(&state, KEY_THRESHOLD_L1, DEF_THRESHOLD_L1),
        max_tags: read_i64(&state, KEY_MAX_TAGS, DEF_MAX_TAGS),
        faces: read_bool(&state, KEY_FACES, true),
        face_score_thresh: read_f64(&state, KEY_FACE_SCORE, DEF_FACE_SCORE),
        face_min_face_px: read_f64(&state, KEY_FACE_MIN_PX, DEF_FACE_MIN_PX),
        face_t_core: read_f64(&state, KEY_FACE_T_CORE, DEF_FACE_T_CORE),
        face_min_core: read_i64(&state, KEY_FACE_MIN_CORE, DEF_FACE_MIN_CORE),
        defaults: AiDefaultsDto {
            face_score_thresh: DEF_FACE_SCORE,
            face_min_face_px: DEF_FACE_MIN_PX,
            face_t_core: DEF_FACE_T_CORE,
            face_min_core: DEF_FACE_MIN_CORE,
        },
    })
}

/// 保存 AI 配置（只写传进来的字段）。
///
/// 参数多是因为「补丁式」写法：每个字段都是 `Option`，`None` = 不动它。
/// 拆成结构体会把前端 IPC 形状从扁平改成 `{ patch: {…} }`，为 1 条形式告警
/// 动 8 处调用不划算，所以这里就地豁免。
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn ai_save_config(
    state: State<'_, AppState>,
    threshold: Option<f64>,
    threshold_l1: Option<f64>,
    max_tags: Option<i64>,
    faces: Option<bool>,
    face_score_thresh: Option<f64>,
    face_min_face_px: Option<f64>,
    face_t_core: Option<f64>,
    face_min_core: Option<i64>,
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
        // 人脸四档：两位小数足够（0.01 的粒度，与滑块步长一致）
        if let Some(v) = face_score_thresh {
            s.set_setting(KEY_FACE_SCORE, &format!("{v:.2}")).map_err(|e| e.to_string())?;
        }
        if let Some(v) = face_min_face_px {
            // 像素取整存，免得面板显示 56.000000001
            s.set_setting(KEY_FACE_MIN_PX, &format!("{:.0}", v)).map_err(|e| e.to_string())?;
        }
        if let Some(v) = face_t_core {
            s.set_setting(KEY_FACE_T_CORE, &format!("{v:.2}")).map_err(|e| e.to_string())?;
        }
        if let Some(v) = face_min_core {
            s.set_setting(KEY_FACE_MIN_CORE, &v.to_string()).map_err(|e| e.to_string())?;
        }
        Ok(())
    })
    .map_err(err)
}

/// 读一条字符串设置：读不到 / 库报错都当「没设过」。
fn setting_str(state: &AppState, key: &str) -> Option<String> {
    state
        .with_manager(|s| Ok(s.setting(key).ok().flatten()))
        .ok()
        .flatten()
}

pub(super) fn read_f32(state: &AppState, key: &str, def: f32) -> f32 {
    setting_str(state, key)
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(def)
}

pub(super) fn read_f64(state: &AppState, key: &str, def: f64) -> f64 {
    setting_str(state, key)
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(def)
}

pub(super) fn read_i64(state: &AppState, key: &str, def: i64) -> i64 {
    setting_str(state, key)
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(def)
}

pub(super) fn read_bool(state: &AppState, key: &str, def: bool) -> bool {
    setting_str(state, key).map(|v| v == "1").unwrap_or(def)
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
            face_score_thresh: 0.6,
            face_min_face_px: 56.0,
            face_t_core: 0.55,
            face_min_core: 3,
            defaults: AiDefaultsDto {
                face_score_thresh: 0.6,
                face_min_face_px: 56.0,
                face_t_core: 0.55,
                face_min_core: 3,
            },
        };
        let j = serde_json::to_string(&dto).unwrap();
        for k in [
            "vocabCount",
            "disabledCount",
            "thresholdL1",
            "maxTags",
            "faceScoreThresh",
            "faceMinFacePx",
            "faceTCore",
            "faceMinCore",
            "defaults",
        ] {
            assert!(j.contains(k), "缺字段 {k}：{j}");
        }
    }
}
