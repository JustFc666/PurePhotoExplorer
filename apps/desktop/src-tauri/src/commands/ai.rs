//! AI 打标：配置、主题词表（查看 / 启停 / 导出）。
//!
//! ## 词表从哪来
//! 编译期 `include_str!` 进来（`crates/pv-ai/vocab/zh_tags.csv`，由
//! `merge_thesaurus.py` 生成），所以**安装包里自带一份完整词表**，不依赖外部文件。
//!
//! ## 用户改了什么存在哪
//! 只存**覆盖项**：`settings` 表里的 `ai.disabled_terms`（JSON 数组）。
//! ⛔ 不要把整份词表复制进数据库 —— 词表升级后旧副本会成为脏数据，
//! 而"禁用名单"这种差量永远跟得上新词表。
//!
//! ## 和 Python 脚本的关系
//! 现阶段推理在 `tools/ai/auto_tag.py`（验证用）。它读同一份 CSV；禁用名单
//! 通过 `ai.disabled_terms` 共享，所以 UI 里禁掉的词，脚本下次跑也不会用。

use serde::Serialize;
use tauri::State;

use crate::commands::dto::ErrorDto;
use crate::state::AppState;

/// 词表本体（编译期嵌入）。
const VOCAB_CSV: &str = include_str!("../../../../../crates/pv-ai/vocab/zh_tags.csv");

/// 统一的错误包装（`ErrorDto` 在别处是按 PvError 构造的，这里没有 `new`）。
fn err(msg: String) -> ErrorDto {
    ErrorDto {
        code: "ai_error".into(),
        message: msg,
        user_visible: true,
    }
}

const KEY_THRESHOLD: &str = "ai.threshold";
const KEY_MAX_TAGS: &str = "ai.max_tags";
const KEY_FACES: &str = "ai.faces";
const KEY_DISABLED: &str = "ai.disabled_terms";

/// 默认阈值。
///
/// ⭐ 2026-10-22：取消「每张必须凑满 N 个」之后，阈值就是**唯一**准入门槛，
/// 因此从 0.35 提到 0.40 —— 实测节日误判 17→5、泛词 91→50。
const DEF_THRESHOLD: f64 = 0.40;
/// 每张标签数的**上限**（不是目标）：够格几个就几个，认不出就空着
const DEF_MAX_TAGS: i64 = 6;

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
    pub threshold: f64,
    pub max_tags: i64,
    pub faces: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VocabTermDto {
    pub term: String,
    pub l1: String,
    pub l2: String,
    pub syn: String,
    pub en: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct Row {
    term: String,
    l1: String,
    l2: String,
    syn: String,
    en: String,
}

/// 拆一行 CSV（只处理本词表用到的形态：逗号分隔 + 双引号包裹/转义）。
///
/// ⚠️ 不用 `split(',')`：英文/同义词里会出现逗号与引号，硬拆会把列错位，
/// 而错位不会报错 —— 只会让「一级类目」变成半截英文，极难发现。
fn split_csv_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if in_quotes && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// 解析嵌入的词表（BOM + 表头都要跳过）。
fn parse_vocab() -> Vec<Row> {
    let text = VOCAB_CSV.trim_start_matches('\u{feff}');
    let mut rows = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if i == 0 || line.trim().is_empty() {
            continue; // 表头
        }
        let f = split_csv_line(line);
        if f.len() < 6 {
            continue;
        }
        rows.push(Row {
            term: f[3].trim().to_string(),
            l1: f[1].trim().to_string(),
            l2: f[2].trim().to_string(),
            syn: f[4].trim().to_string(),
            en: f[5].trim().to_string(),
        });
    }
    rows
}

fn disabled_terms(state: &AppState) -> Vec<String> {
    state
        .with_manager(|s| {
            Ok(s.setting(KEY_DISABLED)
                .map_err(|e| e.to_string())?
                .and_then(|v| serde_json::from_str::<Vec<String>>(&v).ok())
                .unwrap_or_default())
        })
        .unwrap_or_default()
}

/// 读取 AI 配置（缺省值写在前端可见的常量里，别让界面显示空）。
#[tauri::command]
pub async fn ai_config(state: State<'_, AppState>) -> Result<AiConfigDto, ErrorDto> {
    let vocab = parse_vocab();
    let disabled = disabled_terms(&state);
    let (threshold, max_tags, faces) = state
        .with_manager(|s| {
            let get = |k: &str| s.setting(k).map_err(|e| e.to_string());
            Ok((
                get(KEY_THRESHOLD)
                    .ok()
                    .flatten()
                    .and_then(|v| v.parse::<f64>().ok())
                    .unwrap_or(DEF_THRESHOLD),
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
        max_tags,
        faces,
    })
}

/// 保存 AI 配置（只写传进来的字段）。
#[tauri::command]
pub async fn ai_save_config(
    state: State<'_, AppState>,
    threshold: Option<f64>,
    max_tags: Option<i64>,
    faces: Option<bool>,
) -> Result<(), ErrorDto> {
    state.with_manager(|s| {
        if let Some(v) = threshold {
            s.set_setting(KEY_THRESHOLD, &format!("{v:.2}")).map_err(|e| e.to_string())?;
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

/// 展开成 `(标签名, 一级类目)` 映射，供一级类目**去重计数**使用。
///
/// ⭐ 三个要点：
/// 1. 主题词本身 + 同义词都要映射（标签库里可能存的是同义词）；
/// 2. **一级名自己也要映射到自己** —— 只过了一级门槛、没挂上二级标签的图，
///    也要算进「人像」里，否则点一级会少掉一批（用户 2026-10-23 报的"没显示全部"）；
/// 3. 同名只保留第一次出现，避免同义词串到别的类目。
pub(crate) fn vocab_pairs() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut seen_l1: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let push = |out: &mut Vec<(String, String)>, seen: &mut std::collections::BTreeSet<String>, name: &str, l1: &str| {
        if !name.is_empty() && seen.insert(name.to_string()) {
            out.push((name.to_string(), l1.to_string()));
        }
    };
    for r in parse_vocab() {
        push(&mut out, &mut seen, &r.term, &r.l1);
        for s in r.syn.split([';', '；', '、', '|']) {
            push(&mut out, &mut seen, s.trim(), &r.l1);
        }
        if seen_l1.insert(r.l1.clone()) {
            push(&mut out, &mut seen, &r.l1, &r.l1);
        }
    }
    out
}

/// 列出全部主题词（含启用状态）。
#[tauri::command]
pub async fn ai_vocab(state: State<'_, AppState>) -> Result<Vec<VocabTermDto>, ErrorDto> {
    let disabled = disabled_terms(&state);
    Ok(parse_vocab()
        .into_iter()
        .map(|r| VocabTermDto {
            enabled: !disabled.contains(&r.term),
            term: r.term,
            l1: r.l1,
            l2: r.l2,
            syn: r.syn,
            en: r.en,
        })
        .collect())
}

/// 启用/禁用某个主题词（写入 `ai.disabled_terms`）。
#[tauri::command]
pub async fn ai_vocab_toggle(
    state: State<'_, AppState>,
    term: String,
    enabled: bool,
) -> Result<(), ErrorDto> {
    state.with_manager(|s| {
        let mut list: Vec<String> = s
            .setting(KEY_DISABLED)
            .map_err(|e| e.to_string())?
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default();
        let pos = list.iter().position(|t| t == &term);
        match (enabled, pos) {
            (false, None) => list.push(term.clone()),
            (true, Some(i)) => {
                list.remove(i);
            }
            _ => {}
        }
        let json = serde_json::to_string(&list).map_err(|e| e.to_string())?;
        s.set_setting(KEY_DISABLED, &json).map_err(|e| e.to_string())
    })
    .map_err(err)
}

/// 导出词表 CSV 到指定路径（用户在保存对话框里选的）。
///
/// 导出的列和源文件一致，末尾多一列 `enabled`，方便在 Excel 里筛完再导回来。
#[tauri::command]
pub async fn ai_vocab_export(state: State<'_, AppState>, path: String) -> Result<String, ErrorDto> {
    let disabled = disabled_terms(&state);
    let mut out = String::from("编码,一级类目,二级类目,主题词,同义词,英文对照,启用\n");
    for (i, r) in parse_vocab().iter().enumerate() {
        let enabled = if disabled.contains(&r.term) { "否" } else { "是" };
        // 含逗号/引号/换行的字段要加引号并转义
        let esc = |s: &str| -> String {
            if s.contains(',') || s.contains('"') || s.contains('\n') {
                format!("\"{}\"", s.replace('"', "\"\""))
            } else {
                s.to_string()
            }
        };
        out.push_str(&format!(
            "T{:03},{},{},{},{},{},{}\n",
            i + 1,
            esc(&r.l1),
            esc(&r.l2),
            esc(&r.term),
            esc(&r.syn),
            esc(&r.en),
            enabled
        ));
    }
    std::fs::write(&path, out).map_err(|e| err(format!("写文件失败：{e}")))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 词表能被解析且条数合理() {
        let rows = parse_vocab();
        // ⭐ 2026-10-24：断言下限从 300 改成 100 —— 词表已由 385 词精简到
        // **110 词**（与 `tag模板.xlsx` 一致，用户 10-23/10-24 刻意维护的规模）。
        // 这里只保底「词表没被清空」，具体条数会随模板变，别再写死。
        assert!(rows.len() >= 100, "词表条数异常：{}", rows.len());
        assert!(rows.iter().all(|r| !r.term.is_empty() && !r.l1.is_empty()));
        // 一级类目必须都是中文短词（防止 CSV 列错位把英文塞进来）
        assert!(
            rows.iter().all(|r| r.l1.chars().count() <= 6),
            "一级类目出现异常值：{:?}",
            rows.iter().map(|r| &r.l1).find(|l| l.chars().count() > 6)
        );
        // 一级类目要有足够多样性（界面按一级/二级两级钻取，全挤一类等于没分类）
        let l1s: std::collections::HashSet<&str> = rows.iter().map(|r| r.l1.as_str()).collect();
        assert!(l1s.len() >= 12, "一级类目过少：{}", l1s.len());
        // 抽查已知词（2026-10-24 随词表精简同步：猫已归到「宠物」，
        // 文档类的主题词是「证件」而不是「护照」）
        assert!(rows.iter().any(|r| r.term == "猫" && r.l1 == "宠物"));
        assert!(rows.iter().any(|r| r.term == "证件" && r.l1 == "文档"));
    }

    /// ⚠️ 前端按 `vocabCount` / `maxTags` 读配置。字段名一旦不是 camelCase，
    /// JS 拿到 `undefined` → 界面显示「NaN 词」，而且不会报错。
    #[test]
    fn 配置字段必须是camelCase() {
        let dto = AiConfigDto {
            model: "m".into(),
            vocab_count: 385,
            disabled_count: 0,
            threshold: 0.35,
            max_tags: 8,
            faces: true,
        };
        let j = serde_json::to_string(&dto).unwrap();
        for k in ["vocabCount", "disabledCount", "maxTags"] {
            assert!(j.contains(k), "缺字段 {k}：{j}");
        }
    }

    #[test]
    fn 带引号逗号的字段不会错列() {
        let f = split_csv_line(r#"1,人物,人群,"猫,狗",有毛,"cat, dog",是"#);
        assert_eq!(f.len(), 7);
        assert_eq!(f[3], "猫,狗");
        assert_eq!(f[5], "cat, dog");
        // 双引号转义
        let g = split_csv_line(r#"1,a,b,"他说""喵""",,c,是"#);
        assert_eq!(g[3], r#"他说"喵""#);
    }
}
