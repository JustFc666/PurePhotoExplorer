//! 主题词表：查看、启用/停用、导出、导入。
//!
//! 词表本体在编译期嵌入（见 [`VOCAB_CSV`]），用户改动只以**禁用名单**的形式
//! 存进 `settings`（`ai.disabled_terms`）。所以「导入」不是替换词表，
//! 而是把导入文件里的启用状态合并进禁用名单 —— 词表升级时不冲突。

use serde::Serialize;
use tauri::State;

use crate::commands::dto::ErrorDto;
use crate::state::AppState;

use super::config::KEY_DISABLED;
use super::err;


/// 词表本体（编译期嵌入）。
const VOCAB_CSV: &str = include_str!("../../../../../../crates/pv-ai/vocab/zh_tags.csv");

/// 词表里的一行（CSV 的一行）。
///
/// `pub(super)` 是因为 [`parse_vocab`] 要把它交给兄弟模块 `config` ——
/// 阈值那侧需要 `vocab_count` 和 [`vocab_pairs`]。
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Row {
    pub(super) term: String,
    pub(super) l1: String,
    l2: String,
    pub(super) syn: String,
    en: String,
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
pub(super) fn parse_vocab() -> Vec<Row> {
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

pub(super) fn disabled_terms(state: &AppState) -> Vec<String> {
    state
        .with_manager(|s| {
            Ok(s.setting(KEY_DISABLED)
                .map_err(|e| e.to_string())?
                .and_then(|v| serde_json::from_str::<Vec<String>>(&v).ok())
                .unwrap_or_default())
        })
        .unwrap_or_default()
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
    let push = |out: &mut Vec<(String, String)>,
                seen: &mut std::collections::BTreeSet<String>,
                name: &str,
                l1: &str| {
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

/// 导入词表 CSV 的结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VocabImportDto {
    /// 状态真的变了的行数
    pub updated: usize,
    /// CSV 里有、但当前词表里查不到的词条（旧文件、手改错字）
    pub unknown: Vec<String>,
}

/// 拆一行 CSV（只处理双引号包裹与 `""` 转义 —— 词表就这么点复杂度）。
fn split_csv(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                if in_q && chars.peek() == Some(&'"') {
                    cur.push('"');
                    chars.next();
                } else {
                    in_q = !in_q;
                }
            }
            ',' if !in_q => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// 「启用」列的取值：是/否，也容忍 Excel 改出来的 1/0、true/false、y/n。
fn csv_bool(s: &str) -> bool {
    matches!(
        s.trim().to_ascii_lowercase().as_str(),
        "是" | "1" | "true" | "y" | "yes" | "on"
    )
}

/// 导入词表 CSV：**只更新启用 / 停用状态**。
///
/// 词条本身来自编译期词表（`crates/pv-ai/vocab/zh_tags.csv`），运行时改不了它 ——
/// 所以「导入」的语义就是「按 CSV 的『启用』列重设哪些词参与打标」，
/// 和 [`ai_vocab_export`] 正好闭环：导出 → Excel 里勾 → 导回来。
///
/// ⭐ 只处理**列出来的**词条：CSV 里没出现的词保持原状（部分文件也能用）。
#[tauri::command]
pub async fn ai_vocab_import(
    state: State<'_, AppState>,
    path: String,
) -> Result<VocabImportDto, ErrorDto> {
    let raw = std::fs::read_to_string(&path).map_err(|e| err(format!("读文件失败：{e}")))?;
    // Excel 存出来的 UTF-8 常带 BOM，不去掉的话第一列表头会变成「\u{feff}编码」
    let text = raw.trim_start_matches('\u{feff}');
    let mut lines = text.lines();
    let header = lines.next().ok_or_else(|| err("文件是空的".into()))?;
    let cols = split_csv(header);

    let find = |keys: &[&str]| cols.iter().position(|c| keys.iter().any(|k| c.contains(k)));
    let term_idx = find(&["主题词", "term"])
        .ok_or_else(|| err("找不到「主题词」列 —— 请用「导出词表」得到的 CSV 作模板".into()))?;
    let en_idx = find(&["启用", "enabled"])
        .ok_or_else(|| err("找不到「启用」列 —— 请用「导出词表」得到的 CSV 作模板".into()))?;

    let known: Vec<String> = parse_vocab().into_iter().map(|r| r.term).collect();

    state
        .with_manager(|s| {
            let mut disabled: Vec<String> = s
                .setting(KEY_DISABLED)
                .map_err(|e| e.to_string())?
                .and_then(|v| serde_json::from_str(&v).ok())
                .unwrap_or_default();

            let mut updated = 0usize;
            let mut unknown: Vec<String> = Vec::new();
            for line in lines {
                if line.trim().is_empty() {
                    continue;
                }
                let f = split_csv(line);
                let Some(cell) = f.get(term_idx) else { continue };
                let term = cell.trim();
                if term.is_empty() {
                    continue;
                }
                if !known.iter().any(|k| k == term) {
                    unknown.push(term.to_string());
                    continue;
                }
                let want_enabled = f.get(en_idx).map(|v| csv_bool(v)).unwrap_or(true);
                let is_disabled = disabled.iter().any(|t| t == term);
                if want_enabled && is_disabled {
                    disabled.retain(|t| t != term);
                    updated += 1;
                } else if !want_enabled && !is_disabled {
                    disabled.push(term.to_string());
                    updated += 1;
                }
            }

            let json = serde_json::to_string(&disabled).map_err(|e| e.to_string())?;
            s.set_setting(KEY_DISABLED, &json).map_err(|e| e.to_string())?;
            Ok(VocabImportDto { updated, unknown })
        })
        .map_err(err)
}

// ══════════════════════════════════════════════════════════════════════
//  一键执行（2026-10-28）
//
//  ⚠️ **这是过渡实现**：推理仍在 Python 脚本里（`tools/ai/*.py`）。
//  发行版会换成 Rust + `ort`（DirectML EP）直接推理 —— 那时只需换掉下面的
//  spawn 部分，前端按钮、日志面板、「停止」全都不用动。
//
//  为什么用「临时日志文件 + 轮询」而不是 Tauri 事件：
//  ① 脚本要跑几分钟到几十分钟，500ms 轮询足够；
//  ② stdout/stderr 直接重定向到文件，不用起两条线程读管道 ——
//     管道写满会把子进程卡死，这是最常踩的坑；
//  ③ 进程被 kill 或崩了，日志文件还在，用户能把尾部贴给我们。
// ══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv拆分处理引号与转义() {
        assert_eq!(split_csv("a,b,c"), vec!["a", "b", "c"]);
        // 引号里的逗号不算分隔
        assert_eq!(split_csv(r#""a,b",c"#), vec!["a,b", "c"]);
        // "" 转义成一个引号（注意末尾还要有一个闭引号）
        assert_eq!(split_csv("\"他说\"\"好\"\"\",x"), vec!["他说\"好\"", "x"]);
        // 空字段要保留，否则后面的列全错位
        assert_eq!(split_csv("a,,c"), vec!["a", "", "c"]);
    }

    #[test]
    fn 启用列取值宽容() {
        for yes in ["是", "1", "true", "y", "YES", " 是 "] {
            assert!(csv_bool(yes), "{yes} 应视为启用");
        }
        for no in ["否", "0", "false", "n", "", "停用"] {
            assert!(!csv_bool(no), "{no} 应视为停用");
        }
    }

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
