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

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{Manager, State};

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
/// **一级类目**的准入门槛（比二级低：粗分类更容易命中）。
const KEY_THRESHOLD_L1: &str = "ai.l1Threshold";
const KEY_MAX_TAGS: &str = "ai.max_tags";
const KEY_FACES: &str = "ai.faces";
const KEY_DISABLED: &str = "ai.disabled_terms";

/// 默认阈值。
///
/// ⭐ 2026-10-22：取消「每张必须凑满 N 个」之后，阈值就是**唯一**准入门槛，
/// 因此从 0.35 提到 0.40 —— 实测节日误判 17→5、泛词 91→50。
const DEF_THRESHOLD: f64 = 0.40;
/// 一级类目默认门槛。与 `tools/ai/auto_tag.py` 的 `--l1-threshold` 缺省值保持一致（0.36）。
const DEF_THRESHOLD_L1: f64 = 0.36;
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
    /// 二级（具体词）门槛
    pub threshold: f64,
    /// 一级（类目）门槛，通常低于 `threshold`
    pub threshold_l1: f64,
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

/// 一步 = 跑一个 Python 脚本。
struct Step {
    script: &'static str,
    args: Vec<String>,
}

struct Running {
    /// 还没跑的步骤（第一个已经在 `child` 里跑着）
    steps: VecDeque<Step>,
    child: Option<Child>,
    log: PathBuf,
    /// 已结束的步骤是否全部成功
    ok: bool,
    python: PathBuf,
    root: PathBuf,
    /// `Some` = 这次是 **Rust 打标**（不走子进程），`ai_task_status` 据此分流
    tag: Option<TagProgress>,
}

/// 全局只允许一个 AI 任务：两个脚本同时写索引库会互相打架。
///
/// ⚠️ 用 `Arc<Mutex<…>>` 而不是裸 `Mutex`：Rust 打标在 `spawn_blocking` 里跑，
/// 线程拿不到 `State<'_, _>`（生命周期绑在调用栈上），必须 clone 出一个 Arc 进线程。
pub struct AiRunner(pub Arc<Mutex<Option<Running>>>);

impl Default for AiRunner {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(None)))
    }
}

/// 找 Python 解释器。
///
/// 顺序：设置项 `ai.pythonPath` → 本机常见 venv → `PATH` 里的 `python`。
/// ⚠️ 发行版不该依赖这个 —— 它是给「开发机 / 自用」准备的过渡路径。
fn resolve_python(state: &AppState) -> Option<PathBuf> {
    let from_setting = state
        .with_manager(|s| Ok(s.setting("ai.pythonPath").ok().flatten()))
        .ok()
        .flatten()
        .map(PathBuf::from)
        .filter(|p| p.is_file());
    if from_setting.is_some() {
        return from_setting;
    }
    let mut candidates = vec![PathBuf::from("J:/ai/tts/.venv/Scripts/python.exe")];
    if let Some(root) = project_root() {
        candidates.push(root.join("tools/ai/.venv/Scripts/python.exe"));
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// 项目根（脚本按相对路径找 `tools/ai/*.py` 与 `crates/pv-ai/vocab`）。
///
/// 先看编译期路径（开发机构建时有效），再从 exe 目录往上找带
/// `tools/ai/auto_tag.py` 的目录。⚠️ 装成发行版之后必然找不到 ——
/// 那正是要换成 Rust 推理的原因。
fn project_root() -> Option<PathBuf> {
    let from_build = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf());
    if let Some(p) = &from_build {
        if p.join("tools/ai/auto_tag.py").is_file() {
            return Some(p.clone());
        }
    }
    let exe = std::env::current_exe().ok()?;
    let mut dir = exe.parent()?.to_path_buf();
    for _ in 0..4 {
        if dir.join("tools/ai/auto_tag.py").is_file() {
            return Some(dir);
        }
        dir = dir.parent()?.to_path_buf();
    }
    None
}

fn spawn_next(run: &mut Running) -> Result<(), String> {
    let Some(step) = run.steps.pop_front() else {
        run.child = None;
        return Ok(());
    };
    let script = run.root.join("tools/ai").join(step.script);
    if !script.is_file() {
        return Err(format!("找不到脚本：{}", script.display()));
    }
    let out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&run.log)
        .map_err(|e| format!("建日志文件失败：{e}"))?;
    let err = out.try_clone().map_err(|e| format!("复制日志句柄失败：{e}"))?;

    let mut cmd = Command::new(&run.python);
    cmd.arg(&script)
        .args(&step.args)
        .current_dir(&run.root)
        // 脚本按 UTF-8 输出；不设的话 Windows 控制台默认 GBK，中文日志全糊
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUTF8", "1")
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err));
    // Windows 下别弹黑框
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let child = cmd.spawn().map_err(|e| format!("启动失败：{e}"))?;
    run.child = Some(child);
    Ok(())
}

/// 任务状态快照。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiTaskStatus {
    /// 还有步骤在跑
    pub running: bool,
    /// 全部步骤都结束了（成功或失败）
    pub done: bool,
    pub ok: bool,
    /// 日志尾部（最多 200 行）
    pub lines: Vec<String>,
}

/// 读日志尾部。跑久了文件可能几 MB，只取尾部，别每次轮询都全量解析。
fn tail_lines(path: &Path, max: usize) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(max);
    all[start..].iter().map(|l| (*l).to_string()).collect()
}

// Rust 打标的进度（与 Python 子进程共用 `Running`，靠 `tag` 字段区分走哪条路）。
struct TagProgress {
    total: usize,
    done: usize,
    /// 日志尾部（前端直接显示，最多 200 行）
    lines: VecDeque<String>,
    finished: bool,
    ok: bool,
}

impl Running {
    /// 造一个「Rust 打标」会话占位 —— 复用 `Running` 但 `child` 为 None、
    /// `steps` 为空，`ai_task_status` 看到 `tag.is_some()` 就走 Rust 分支。
    fn tag_session(log: PathBuf) -> Self {
        Self {
            steps: VecDeque::new(),
            child: None,
            log,
            ok: true,
            python: PathBuf::new(),
            root: PathBuf::new(),
            tag: Some(TagProgress {
                total: 0,
                done: 0,
                lines: VecDeque::new(),
                finished: false,
                ok: true,
            }),
        }
    }
}

/// 往打标进度里追加一行日志（同时落盘，方便事后查）。
fn tag_log(runner: &Mutex<Option<Running>>, line: String) {
    if let Ok(mut g) = runner.lock() {
        if let Some(r) = g.as_mut() {
            if let Some(t) = r.tag.as_mut() {
                t.lines.push_back(line.clone());
                while t.lines.len() > 200 {
                    t.lines.pop_front();
                }
            }
            // 日志文件也写一份：用户报问题时能直接把尾部贴过来
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&r.log)
            {
                use std::io::Write;
                let _ = writeln!(f, "{line}");
            }
        }
    }
}

/// 跑一次 Rust 打标（在 `spawn_blocking` 里执行，不占 UI 线程）。
///
/// 三阶段，索引锁只在两头各占一下：
/// 1. 锁内查图列表（毫秒级）
/// 2. **锁外**逐张解码 + 推理（1~2 秒/张，期间图库滚动不受影响）
/// 3. 回调里逐张写库（锁粒度 = 一张图，毫秒级）
fn run_tag_batch_blocking(app: tauri::AppHandle, library: String) {
    use pv_ai::batch::{tag_batch, Progress, TagOptions};

    // ⚠️ 先在**一条语句**里把 Arc 拷出来：`State<'_, T>` 借用 `app`，
    // 直接 `let runner = app.state::<AiRunner>()` 会把借用带到整个函数体，
    // 和下面 `state.with_manager_mut(…)` 的借用叠加后编译器判定生命周期不足。
    let runner: Arc<Mutex<Option<Running>>> = app.state::<AiRunner>().0.clone();
    let state = app.state::<AppState>();

    let t_start = std::time::Instant::now();
    tag_log(&runner, format!("开始打标：{library}"));

    // ── 1. 查图列表 ──
    let images: Vec<(i64, PathBuf)> = match state.with_manager(|s| {
        let libs = pv_index::repo::libraries::list(&s.conn()).map_err(|e| e.to_string())?;
        let Some(lib) = libs.into_iter().find(|l| l.name == library) else {
            return Err(format!("找不到图库「{library}」"));
        };
        let q = pv_store::ImageQuery::default();
        let imgs = s.query_in_library(&q, lib.id).map_err(|e| e.to_string())?;
        let mut out = Vec::with_capacity(imgs.len());
        for img in imgs {
            if let Ok(Some(p)) = s.abs_path(&img) {
                out.push((img.id, p));
            }
        }
        Ok(out)
    }) {
        Ok(v) => v,
        Err(e) => {
            tag_log(&runner, format!("失败：{e}"));
            if let Ok(mut g) = runner.lock() {
                if let Some(r) = g.as_mut() {
                    if let Some(t) = r.tag.as_mut() {
                        t.finished = true;
                        t.ok = false;
                    }
                    r.ok = false;
                }
            }
            return;
        }
    };

    let total = images.len();
    if let Ok(mut g) = runner.lock() {
        if let Some(r) = g.as_mut() {
            if let Some(t) = r.tag.as_mut() {
                t.total = total;
            }
        }
    }
    tag_log(&runner, format!("待处理 {total} 张"));

    // ── 2. 加载模型（165MB，只做一次）──
    let dir = tagger_dir();
    let mut tagger = match pv_ai::tagger::Tagger::load(&dir) {
        Ok(t) => t,
        Err(e) => {
            tag_log(&runner, format!("模型加载失败：{e}"));
            if let Ok(mut g) = runner.lock() {
                if let Some(r) = g.as_mut() {
                    if let Some(t) = r.tag.as_mut() {
                        t.finished = true;
                        t.ok = false;
                    }
                    r.ok = false;
                }
            }
            return;
        }
    };
    tag_log(
        &runner,
        format!("模型就绪 · 词表 {} 条 · {} 条 prompt", tagger.vocab().len(), tagger.prompts_len()),
    );

    // ── 3. 跑批 + 逐张写库 ──
    let opts = TagOptions {
        threshold: read_f32(&state, "ai.threshold", 0.39),
        threshold_l1: read_f32(&state, "ai.l1Threshold", 0.35),
        max_tags: read_f32(&state, "ai.max_tags", 6.0) as usize,
        long_side: 1024,
    };
    tag_log(
        &runner,
        format!(
            "阈值：二级 ≥{:.2} · 一级 ≥{:.2} · 每张最多 {} 个",
            opts.threshold, opts.threshold_l1, opts.max_tags
        ),
    );

    let mut last_log = 0usize;
    let stats = tag_batch(
        &images,
        &mut tagger,
        &opts,
        |p: &Progress| {
            if let Ok(mut g) = runner.lock() {
                if let Some(r) = g.as_mut() {
                    if let Some(t) = r.tag.as_mut() {
                        t.done = p.done;
                    }
                }
            }
            // 每 10 张刷一行，避免日志被 371 行「正在处理」淹没
            if p.done == p.total || p.done - last_log >= 10 {
                last_log = p.done;
                tag_log(
                    &runner,
                    format!("{}/{} · {}", p.done, p.total, p.name),
                );
            }
        },
        |done| {
            let _ = state.with_manager_mut(|s| {
                s.replace_ai_tags(done.image_id, &done.tags)
                    .map_err(|e| e.to_string())
            });
        },
    );

    for e in &stats.errors {
        tag_log(&runner, format!("跳过 {e}"));
    }
    let secs = t_start.elapsed().as_secs_f64();
    tag_log(
        &runner,
        format!(
            "完成：成功 {} 张 · 失败 {} 张 · 用时 {:.1} 秒（{:.0} ms/张）",
            stats.ok,
            stats.failed,
            secs,
            if stats.ok + stats.failed > 0 {
                secs * 1000.0 / (stats.ok + stats.failed) as f64
            } else {
                0.0
            }
        ),
    );

    // ⚠️ 末尾这个分号不能省：`if let` 作为尾表达式时，它内部的临时 `Result<MutexGuard>`
    // 会活到函数返回之后（比 `runner` 还久），编译器会报
    // "`runner` dropped here while still borrowed"。
    if let Ok(mut g) = runner.lock() {
        if let Some(r) = g.as_mut() {
            if let Some(t) = r.tag.as_mut() {
                t.finished = true;
                t.ok = stats.failed == 0;
            }
            r.ok = stats.failed == 0;
        }
    };
}


/// 跑一次人脸识别（检测 → 特征 → 聚类 → 写库），在 `spawn_blocking` 里执行。
///
/// ## 为什么分四段
/// 1. 查图列表（锁内，毫秒）
/// 2. **锁外**检测 + 特征（每张 1~3 秒，期间图库滚动不受影响）
/// 3. **锁外**聚类（纯内存，几千张脸不到 1 秒）
/// 4. 写库（锁内，事务性插入）
///
/// ## persons 会被重建
/// `--reset` 语义：先 `DELETE FROM persons` 再按新聚类建人物 ——
/// **用户给人物起的名字会丢**。这与 `tools/ai/face_cluster.py` 的行为一致，
/// UI 上要明确告知（别让用户莫名其妙发现名字没了）。
fn run_face_batch_blocking(app: tauri::AppHandle, library: String) {
    use pv_ai::cluster::{two_stage_cluster, ClusterOpts};
    use pv_ai::face::{FaceDetector, FaceRecognizer};
    use pv_ai::face_batch::{detect_batch, FaceBatchOpts, FaceProgress};

    // ⚠️ 一条语句里把 Arc 拷出来，理由同 tag：State 借用活不过函数体
    let runner: Arc<Mutex<Option<Running>>> = app.state::<AiRunner>().0.clone();
    let state = app.state::<AppState>();
    let t_start = std::time::Instant::now();

    tag_log(&runner, format!("开始识别人像：{library}"));

    let fail = |msg: String| {
        tag_log(&runner, format!("失败：{msg}"));
        if let Ok(mut g) = runner.lock() {
            if let Some(r) = g.as_mut() {
                if let Some(t) = r.tag.as_mut() {
                    t.finished = true;
                    t.ok = false;
                }
                r.ok = false;
            }
        }
    };

    // ── 1. 查图列表 ──
    let images: Vec<(i64, PathBuf)> = match state.with_manager(|s| {
        let libs = pv_index::repo::libraries::list(s.conn()).map_err(|e| e.to_string())?;
        let Some(lib) = libs.into_iter().find(|l| l.name == library) else {
            return Err(format!("找不到图库「{library}」"));
        };
        let q = pv_store::ImageQuery::default();
        let imgs = s.query_in_library(&q, lib.id).map_err(|e| e.to_string())?;
        let mut out = Vec::with_capacity(imgs.len());
        for img in imgs {
            if let Ok(Some(p)) = s.abs_path(&img) {
                out.push((img.id, p));
            }
        }
        Ok(out)
    }) {
        Ok(v) => v,
        Err(e) => return fail(e),
    };

    if let Ok(mut g) = runner.lock() {
        if let Some(r) = g.as_mut() {
            if let Some(t) = r.tag.as_mut() {
                t.total = images.len();
            }
        }
    }
    tag_log(&runner, format!("待处理 {} 张", images.len()));

    // ── 2. 模型 ──
    let dir = face_dir();
    let mut det = match FaceDetector::load(&dir) {
        Ok(d) => d,
        Err(e) => return fail(format!("检测模型加载失败：{e}")),
    };
    let mut rec = match FaceRecognizer::load(&dir) {
        Ok(r) => r,
        Err(e) => return fail(format!("识别模型加载失败：{e}")),
    };
    tag_log(&runner, "模型就绪 · YuNet + SFace".into());

    // ── 3. 检测 + 特征（锁外）──
    let opts = FaceBatchOpts::default();
    let mut last_log = 0usize;
    let (faces, stats) = detect_batch(
        &images,
        &mut det,
        &mut rec,
        &opts,
        |p: &FaceProgress| {
            if let Ok(mut g) = runner.lock() {
                if let Some(r) = g.as_mut() {
                    if let Some(t) = r.tag.as_mut() {
                        t.done = p.done;
                    }
                }
            }
            if p.done == p.total || p.done - last_log >= 10 {
                last_log = p.done;
                let tail = if p.faces > 0 {
                    format!("（{} 张脸）", p.faces)
                } else {
                    String::new()
                };
                tag_log(&runner, format!("{}/{} · {}{tail}", p.done, p.total, p.name));
            }
        },
    );

    tag_log(
        &runner,
        format!(
            "检测完成：{} 张图有脸 · 共 {} 张脸 · 失败 {} 张",
            stats.with_face, stats.faces, stats.failed
        ),
    );
    for e in &stats.errors {
        tag_log(&runner, format!("跳过 {e}"));
    }
    if faces.is_empty() {
        return fail("一张脸都没检测到（检查图库内容或降低检测门槛）".into());
    }

    // ── 4. 聚类（锁外）──
    tag_log(&runner, "聚类中…".into());
    let embs: Vec<Vec<f32>> = faces.iter().map(|f| f.embedding.clone()).collect();
    let c_opts = ClusterOpts::default();
    let labels = two_stage_cluster(&embs, &c_opts);
    let mut sizes: std::collections::HashMap<i32, usize> = std::collections::HashMap::new();
    for &l in &labels {
        *sizes.entry(l).or_insert(0) += 1;
    }
    let multi = sizes.values().filter(|&&c| c >= c_opts.min_core).count();
    let singles = sizes.values().filter(|&&c| c < c_opts.min_core).count();
    tag_log(
        &runner,
        format!("聚类完成：{} 个簇（≥{} 张的 {multi} 个）· 零散 {singles} 组", sizes.len(), c_opts.min_core),
    );

    // ── 5. 写库（锁内）──
    tag_log(&runner, "写库中…".into());
    let ver = format!(
        "sface_2021dec@{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    );

    let res = state.with_manager_mut(|s| {
        let conn = s.conn();
        // 4a. 重建 persons（Python 版同样的 --reset 语义）
        conn.execute("DELETE FROM persons", [])
            .map_err(|e| e.to_string())?;

        // 4b. 按图分组写 faces（先删后插，幂等），并把插入的 face_id 记下来
        let mut by_image: std::collections::BTreeMap<i64, Vec<usize>> =
            std::collections::BTreeMap::new();
        for (i, f) in faces.iter().enumerate() {
            by_image.entry(f.image_id).or_default().push(i);
        }
        let mut face_ids = vec![0i64; faces.len()];
        for (image_id, idxs) in &by_image {
            let arr: Vec<pv_index::models::Face> = idxs
                .iter()
                .map(|&i| {
                    let f = &faces[i];
                    pv_index::models::Face {
                        id: 0,
                        image_id: f.image_id,
                        person_id: None,
                        x: f.x,
                        y: f.y,
                        w: f.w,
                        h: f.h,
                        tier: Some(1600),
                        model: opts.model.to_string(),
                        score: Some(f.score),
                        quality: Some(f.quality),
                        embedding: pv_index::repo::faces::quantize(&f.embedding),
                    }
                })
                .collect();
            let ids = pv_index::repo::faces::replace_for_image(conn, *image_id, &arr)
                .map_err(|e| e.to_string())?;
            for (k, &i) in idxs.iter().enumerate() {
                face_ids[i] = ids[k];
            }
        }

        // 4c. 按簇建人物（只给 ≥ min_core 张的簇建，单例只留 faces 行）
        let mut by_label: std::collections::HashMap<i32, Vec<usize>> =
            std::collections::HashMap::new();
        for (i, &l) in labels.iter().enumerate() {
            by_label.entry(l).or_default().push(i);
        }
        let mut made = 0usize;
        for (_, idxs) in by_label {
            if idxs.len() < c_opts.min_core {
                continue;
            }
            // 封面用**质量最高**（脸最大）的那张，头像更清楚
            let cover = *idxs
                .iter()
                .max_by(|a, b| {
                    faces[**a]
                        .quality
                        .partial_cmp(&faces[**b].quality)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .unwrap();
            let pid = pv_index::repo::persons::insert(
                conn,
                &pv_index::models::Person::new(None, Some(ver.clone())),
            )
            .map_err(|e| e.to_string())?;
            pv_index::repo::persons::set_cover(conn, pid, Some(face_ids[cover]))
                .map_err(|e| e.to_string())?;
            let fids: Vec<i64> = idxs.iter().map(|&i| face_ids[i]).collect();
            pv_index::repo::faces::set_person(conn, &fids, Some(pid))
                .map_err(|e| e.to_string())?;
            pv_index::repo::persons::refresh_count(conn, pid).map_err(|e| e.to_string())?;
            made += 1;
        }
        pv_index::repo::persons::refresh_all(conn).map_err(|e| e.to_string())?;
        Ok((made, face_ids.len()))
    });

    let secs = t_start.elapsed().as_secs_f64();
    match res {
        Ok((made, written)) => {
            tag_log(
                &runner,
                format!(
                    "完成：写入 {written} 张脸 · 建了 {made} 个人物 · 用时 {secs:.1} 秒"
                ),
            );
            if let Ok(mut g) = runner.lock() {
                if let Some(r) = g.as_mut() {
                    if let Some(t) = r.tag.as_mut() {
                        t.finished = true;
                        t.ok = true;
                    }
                    r.ok = true;
                }
            }
        }
        Err(e) => fail(format!("写库失败：{e}")),
    }
}

/// 人脸模型目录：优先 exe 同级 `assets/ai/face`，退回编译期路径。
fn face_dir() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("assets/ai/face");
            if p.join("sface.onnx").is_file() {
                return p;
            }
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/ai/face")
}

/// 打标模型目录：优先 exe 同级 `assets/ai/tag`，退回编译期路径（开发机）。
fn tagger_dir() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("assets/ai/tag");
            if p.join("tag_meta.json").is_file() {
                return p;
            }
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/ai/tag")
}

fn read_f32(state: &AppState, key: &str, def: f32) -> f32 {
    state
        .with_manager(|s| Ok(s.setting(key).ok().flatten()))
        .ok()
        .flatten()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(def)
}

/// 启动一键任务。`task` 只有两种：`tag` / `face`。
///
/// 阈值等参数从设置里读，保证和界面上看到的一致 —— 别让用户改了滑块、
/// 跑起来却是旧参数。
#[tauri::command]
pub async fn ai_run_task(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    runner: State<'_, AiRunner>,
    task: String,
    library: String,
) -> Result<String, ErrorDto> {
    let mut guard = runner.0.lock().map_err(|_| err("任务状态锁坏了".into()))?;
    if let Some(r) = guard.as_mut() {
        if r.child.is_some() {
            return Err(err("已有任务在跑，先等它结束或点「停止」".into()));
        }
    }

    let python = resolve_python(&state).ok_or_else(|| {
        err("没找到 Python。这是开发版功能：请在设置里填 Python 路径".into())
    })?;
    let root = project_root()
        .ok_or_else(|| err("找不到项目目录（tools/ai 脚本所在位置）".into()))?;

    let (th, th_l1) = state
        .with_manager(|s| {
            let get = |k: &str| s.setting(k).ok().flatten();
            Ok((
                get("ai.threshold")
                    .and_then(|v| v.parse::<f64>().ok())
                    .unwrap_or(0.40),
                get("ai.l1Threshold")
                    .and_then(|v| v.parse::<f64>().ok())
                    .unwrap_or(0.36),
            ))
        })
        .map_err(|e| err(e.to_string()))?;

    /*
     * ⭐ `tag` 走 **Rust**（ort + DirectML），不再 spawn Python ——
     * 这是发行版的关键一步：用户机器上不需要装 Python/torch。
     * `face` 暂时还是 Python（S3 再换）。
     */
    if task == "tag" {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let log = std::env::temp_dir().join(format!("pve-ai-tag-{stamp}.log"));
        let _ = std::fs::write(&log, b"");
        let path = log.display().to_string();
        *guard = Some(Running::tag_session(log));
        // ⚠️ 必须在放锁之后起线程：批处理里要反复拿这把锁更新进度
        drop(guard);

        let app2 = app.clone();
        let lib2 = library.clone();
        tauri::async_runtime::spawn_blocking(move || run_tag_batch_blocking(app2, lib2));
        return Ok(path);
    }

    /*
     * ⭐ `face` 也走 Rust 了（YuNet + SFace + 两阶段聚类）——
     * 至此打标与人脸都脱离 Python，发行版不再需要 torch。
     */
    if task == "face" {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let log = std::env::temp_dir().join(format!("pve-ai-face-{stamp}.log"));
        let _ = std::fs::write(&log, b"");
        let path = log.display().to_string();
        *guard = Some(Running::tag_session(log));
        drop(guard);

        let app2 = app.clone();
        let lib2 = library.clone();
        tauri::async_runtime::spawn_blocking(move || run_face_batch_blocking(app2, lib2));
        return Ok(path);
    }

    #[allow(unreachable_code)]
    let steps: Vec<Step> = match task.as_str() {
        "face" => vec![
            Step {
                script: "face_probe.py",
                args: vec![
                    "--library".into(),
                    library,
                    "--long-side".into(),
                    "2048".into(),
                    "--min-face".into(),
                    "56".into(),
                    "--only-person-tags".into(),
                    "--write-db".into(),
                ],
            },
            // 探针写完 embedding 必须重新聚类，否则人像列表还是旧的
            Step {
                script: "face_cluster.py",
                args: vec!["--reset".into()],
            },
        ],
        other => return Err(err(format!("未知任务：{other}"))),
    };

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let log = std::env::temp_dir().join(format!("pve-ai-{task}-{stamp}.log"));
    // 开跑前清空，前端看到的就是这一次的完整日志
    let _ = std::fs::write(&log, b"");

    let mut running = Running {
        steps: steps.into_iter().collect(),
        child: None,
        log,
        ok: true,
        python,
        root,
        tag: None,
    };
    spawn_next(&mut running).map_err(err)?;
    let path = running.log.display().to_string();
    *guard = Some(running);
    Ok(path)
}

/// 轮询任务状态（前端每 500ms 一次）。
///
/// ⭐ 顺手负责**推进队列**：当前脚本退出后立刻起下一个 ——
/// 人脸那两步（probe → cluster）就是这样串起来的。
#[tauri::command]
pub async fn ai_task_status(runner: State<'_, AiRunner>) -> Result<AiTaskStatus, ErrorDto> {
    let mut guard = runner.0.lock().map_err(|_| err("任务状态锁坏了".into()))?;
    let Some(run) = guard.as_mut() else {
        return Ok(AiTaskStatus {
            running: false,
            done: true,
            ok: true,
            lines: Vec::new(),
        });
    };

    // Rust 打标：进度在 `tag` 里，和子进程那套完全分开
    if let Some(t) = run.tag.as_ref() {
        return Ok(AiTaskStatus {
            running: !t.finished,
            done: t.finished,
            ok: t.ok,
            lines: t.lines.iter().cloned().collect(),
        });
    }

    let mut finished = false;
    if let Some(child) = run.child.as_mut() {
        match child.try_wait() {
            Ok(Some(status)) => {
                finished = true;
                if !status.success() {
                    run.ok = false;
                    // 一步失败就别往下跑（人脸 probe 失败时聚类毫无意义）
                    run.steps.clear();
                }
            }
            Ok(None) => {}
            Err(_) => {
                finished = true;
                run.ok = false;
                run.steps.clear();
            }
        }
    } else {
        finished = true;
    }

    if finished && !run.steps.is_empty() {
        run.child = None;
        if let Err(msg) = spawn_next(run) {
            run.ok = false;
            run.steps.clear();
            let _ = std::fs::write(&run.log, format!("{msg}\n"));
        }
    } else if finished {
        run.child = None;
    }

    let running = run.child.is_some();
    let done = !running && run.steps.is_empty();
    Ok(AiTaskStatus {
        running,
        done,
        ok: run.ok,
        lines: tail_lines(&run.log, 200),
    })
}

/// 停止当前任务（连子进程一起杀，别留孤儿 Python）。
#[tauri::command]
pub async fn ai_task_stop(runner: State<'_, AiRunner>) -> Result<(), ErrorDto> {
    let mut guard = runner.0.lock().map_err(|_| err("任务状态锁坏了".into()))?;
    if let Some(run) = guard.as_mut() {
        if let Some(child) = run.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        run.steps.clear();
        run.child = None;
    }
    *guard = None;
    Ok(())
}

#[cfg(test)]
mod tests {
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
            threshold_l1: 0.36,
            max_tags: 8,
            faces: true,
        };
        let j = serde_json::to_string(&dto).unwrap();
        for k in ["vocabCount", "disabledCount", "thresholdL1", "maxTags"] {
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
