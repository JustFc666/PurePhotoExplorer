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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

/// 默认阈值 —— **必须与引擎实际使用的缺省值一致**。
///
/// ⚠️ 2026-11 修：这两个数以前各写一份字面量（这里是 0.40 / 0.36，Rust 打标
/// 那边是 0.39 / 0.35），而 `f9127c1` 把打标切到 Rust 时只改了一处 ——
/// 结果是**没动过设置的用户，面板显示 0.40，实际按 0.39 跑**。
/// 阈值差 0.01 就意味着「界面说会打上的标签，实际没打」，属于静默偏差，
/// 用户只能靠数标签才发现。
/// 现在统一从 `pv_ai::batch` 的默认值派生（那里有校准依据）。
const DEF_THRESHOLD: f64 = pv_ai::batch::DEFAULT_THRESHOLD as f64;
/// 一级类目默认门槛，同上。
const DEF_THRESHOLD_L1: f64 = pv_ai::batch::DEFAULT_THRESHOLD_L1 as f64;
/// 每张标签数的**上限**（不是目标）：够格几个就几个，认不出就空着
const DEF_MAX_TAGS: i64 = pv_ai::batch::DEFAULT_MAX_TAGS as i64;

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

pub struct Running {
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
    /// 会话编号（见 [`Session`]）。
    id: u64,
    /// 停止请求：Rust 批处理每张图查一次。
    ///
    /// ⛔ **不能靠 kill 子进程来停 Rust 任务** —— 它跑在同一个进程的
    /// `spawn_blocking` 线程里，没有 pid 可杀。`ai_task_stop` 以前只 kill
    /// `child`（Rust 路径恒为 `None`）就把状态清空，界面立刻显示「已停止」，
    /// 而线程还在解码、推理、**往索引里写标签**，能持续几十分钟。
    cancel: Arc<AtomicBool>,
}

/// 会话编号发号器。
static SESSION_SEQ: AtomicU64 = AtomicU64::new(1);

fn next_session_id() -> u64 {
    SESSION_SEQ.fetch_add(1, Ordering::Relaxed)
}

/// 后台线程手里的会话凭据。
///
/// ## 为什么需要它
/// Rust 批处理只能拿到 `Arc<Mutex<Option<Running>>>`，而槽位会被
/// 「停止 → 再开始」整个换掉。旧线程如果照常往槽位里写，就会把进度写进
/// **新任务**的 `TagProgress` —— 用户看到进度从 3/500 开始乱跳，或者新任务
/// 一开始就显示「已完成」。
///
/// 所以：编号对不上 = 我的会话已经不在了 → 所有写入静默丢弃、循环尽快退出。
#[derive(Clone)]
struct Session {
    runner: Arc<Mutex<Option<Running>>>,
    id: u64,
    cancel: Arc<AtomicBool>,
}

impl Session {
    /// 从全局槽位取当前会话凭据（`None` = 没有任务，直接返回即可）。
    fn take(app: &tauri::AppHandle) -> Option<Self> {
        let runner: Arc<Mutex<Option<Running>>> = app.state::<AiRunner>().0.clone();
        let g = runner.lock().ok()?;
        let r = g.as_ref()?;
        Some(Self {
            runner: runner.clone(),
            id: r.id,
            cancel: r.cancel.clone(),
        })
        // `g` 在这里释放：后面全程都要反复拿这把锁，不能带着借用往下走
    }

    /// 用户点「停止」了吗（也包含「我的会话已被顶掉」）。
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// 在**本次会话**的状态上做一次修改。
    ///
    /// 编号不符或槽位已空时什么也不做 —— 这就是「旧线程写不进新会话」的保证。
    fn with(&self, f: impl FnOnce(&mut Running)) {
        let Ok(mut g) = self.runner.lock() else {
            return;
        };
        if let Some(r) = g.as_mut().filter(|r| r.id == self.id) {
            f(r);
        }
    }

    /// 修改打标进度（没在跑 Rust 打标时是空操作）。
    fn with_tag(&self, f: impl FnOnce(&mut TagProgress)) {
        self.with(|r| {
            if let Some(t) = r.tag.as_mut() {
                f(t);
            }
        });
    }

    /// 整个任务失败：进度标红 + 结束，前端立刻能显示原因。
    fn fail(&self, msg: String) {
        self.log(format!("失败：{msg}"));
        self.with(|r| {
            if let Some(t) = r.tag.as_mut() {
                t.finished = true;
                t.ok = false;
            }
            r.ok = false;
        });
    }

    /// 往进度里追加一行日志（同时落盘，方便事后查）。
    fn log(&self, line: String) {
        self.with(|r| {
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
        });
    }
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
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiTaskStatus {
    /// 还有步骤在跑
    pub running: bool,
    /// 全部步骤都结束了（成功或失败）
    pub done: bool,
    pub ok: bool,
    /// 日志尾部（最多 200 行）
    pub lines: Vec<String>,
    /// 当前阶段：`"tag"` / `"face"`（一键 AI 识别时前端据此高亮进度行）
    pub phase: String,
    /// 打标进度
    pub tag_done: usize,
    pub tag_total: usize,
    /// 人脸进度
    pub face_done: usize,
    pub face_total: usize,
}

/// 读日志尾部（最多 `max` 行）。
///
/// ⭐ **真的只读尾部**：前端每 500ms 轮询一次，而日志跑久了能到几 MB
/// （Python 那条路把子进程 stdout 全量重定向进来）。以前这里 `read_to_string`
/// 整份读，等于每 500ms 重复读几 MB 再切一遍行 —— 注释写着「别全量解析」，
/// 实现却是全量的。
///
/// 现在从末尾取 64KB 窗口（200 行中文约 12KB，留足余量），不够再翻 4 倍。
/// 窗口起点落在行中间时丢掉那半行 —— 宁可少显示一行，也不要半截乱码。
fn tail_lines(path: &Path, max: usize) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    if max == 0 {
        return Vec::new();
    }
    let Ok(mut f) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let Ok(len) = f.metadata().map(|m| m.len()) else {
        return Vec::new();
    };
    let mut window = 64 * 1024u64;
    loop {
        let start = len.saturating_sub(window);
        if f.seek(SeekFrom::Start(start)).is_err() {
            return Vec::new();
        }
        let mut buf = Vec::new();
        if f.read_to_end(&mut buf).is_err() {
            return Vec::new();
        }
        let text = String::from_utf8_lossy(&buf);
        let mut lines: Vec<&str> = text.lines().collect();
        // 从文件中间切进来的首行是半行（UTF-8 还可能切在半字符上）
        if start > 0 && !lines.is_empty() {
            lines.remove(0);
        }
        if lines.len() >= max || start == 0 {
            let from = lines.len().saturating_sub(max);
            return lines[from..].iter().map(|l| (*l).to_string()).collect();
        }
        window = window.saturating_mul(4);
    }
}

// Rust 打标的进度（与 Python 子进程共用 `Running`，靠 `tag` 字段区分走哪条路）。
struct TagProgress {
    /// 当前阶段：`"tag"` / `"face"`（pipeline 任务用来高亮对应的进度行）
    phase: String,
    /// 打标阶段的进度
    tag_done: usize,
    tag_total: usize,
    /// 人脸阶段的进度
    face_done: usize,
    face_total: usize,
    /// 兼容单阶段任务：等于当前阶段的那一对
    total: usize,
    done: usize,
    /// 整体是否结束（pipeline 要等两步都跑完）
    finished: bool,
    ok: bool,
    /// 日志尾部（前端直接显示，最多 200 行）
    lines: VecDeque<String>,
}

impl Running {
    /// 这个会话还在跑吗？
    ///
    /// ⚠️ **Rust 任务的 `child` 恒为 `None`**：光看 `child.is_some()` 会认为
    /// 「没有任务在跑」，于是用户在打标进行中再点一次「开始」，
    /// 后端就真的又起一个 `spawn_blocking` 线程 —— 两个线程同时往索引写标签。
    /// `ai_run_task` 的防重入检查必须用这个。
    fn is_active(&self) -> bool {
        self.child.is_some() || self.tag.as_ref().is_some_and(|t| !t.finished)
    }

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
            id: next_session_id(),
            cancel: Arc::new(AtomicBool::new(false)),
            tag: Some(TagProgress {
                phase: "tag".into(),
                tag_done: 0,
                tag_total: 0,
                face_done: 0,
                face_total: 0,
                total: 0,
                done: 0,
                finished: false,
                ok: true,
                lines: VecDeque::new(),
            }),
        }
    }
}

/// 跑一次 Rust 打标（在 `spawn_blocking` 里执行，不占 UI 线程）。
///
/// 三阶段，索引锁只在两头各占一下：
/// 1. 锁内查图列表（毫秒级）
/// 2. **锁外**逐张解码 + 推理（1~2 秒/张，期间图库滚动不受影响）
/// 3. 回调里逐张写库（锁粒度 = 一张图，毫秒级）
///
/// 「停止」靠 `Session::cancelled()` 在**每张图**上检查：先判退、再判写库，
/// 所以按下停止之后不会再有新的标签落库。
fn run_tag_batch_blocking(app: tauri::AppHandle, library: String, mark_finished: bool) {
    use pv_ai::batch::{tag_batch, Progress, TagOptions};

    // ⚠️ 会话凭据要在**一条语句**里取出：`State<'_, T>` 借用 `app`，
    // 带着它往下走会和 `state.with_manager_mut(…)` 的借用叠加，判定生命周期不足。
    let Some(sess) = Session::take(&app) else {
        return;
    };
    let state = app.state::<AppState>();

    let t_start = std::time::Instant::now();
    sess.log(format!("开始打标：{library}"));

    // ── 1. 查图列表（`library` 为空 = 全部图库）──
    let images: Vec<(i64, PathBuf)> = match state.with_manager(|s| collect_images(s, &library)) {
        Ok(v) => v,
        Err(e) => return sess.fail(e),
    };

    let total = images.len();
    sess.with_tag(|t| {
        t.phase = "tag".into();
        t.tag_total = total;
        t.total = total;
    });
    sess.log(format!(
        "范围：{} · 待处理 {total} 张",
        if library.is_empty() { "全部图库" } else { &library }
    ));

    // ── 2. 加载模型（165MB，只做一次）──
    let dir = tagger_dir();
    let mut tagger = match pv_ai::tagger::Tagger::load(&dir) {
        Ok(t) => t,
        Err(e) => return sess.fail(format!("模型加载失败：{e}")),
    };
    sess.log(format!(
        "模型就绪 · 词表 {} 条 · {} 条 prompt",
        tagger.vocab().len(),
        tagger.prompts_len()
    ));

    // ── 3. 跑批 + 逐张写库 ──
    // 缺省值一律取 `DEF_*`（从 `pv_ai::batch` 派生），⛔ 别再在这里写字面量 ——
    // 写两个字面量就会出现「面板显示一个数、实际按另一个数跑」。
    let opts = TagOptions {
        threshold: read_f32(&state, KEY_THRESHOLD, DEF_THRESHOLD as f32),
        threshold_l1: read_f32(&state, KEY_THRESHOLD_L1, DEF_THRESHOLD_L1 as f32),
        max_tags: read_f32(&state, KEY_MAX_TAGS, DEF_MAX_TAGS as f32) as usize,
        long_side: 1024,
    };
    sess.log(format!(
        "阈值：二级 ≥{:.2} · 一级 ≥{:.2} · 每张最多 {} 个",
        opts.threshold, opts.threshold_l1, opts.max_tags
    ));

    let mut last_log = 0usize;
    let stats = tag_batch(
        &images,
        &mut tagger,
        &opts,
        |p: &Progress| {
            sess.with_tag(|t| {
                t.phase = "tag".into();
                t.tag_done = p.done;
                t.done = p.done;
            });
            // 每 10 张刷一行，避免日志被 371 行「正在处理」淹没
            if p.done == p.total || p.done - last_log >= 10 {
                last_log = p.done;
                sess.log(format!("{}/{} · {}", p.done, p.total, p.name));
            }
            // ⛔ 返回 false = 用户点了停止 → 批处理立刻收尾，不再解下一张
            !sess.cancelled()
        },
        |done| {
            // ⚠️ 先判后写：`on_done` 在 `on_progress` **之前**回调，
            // 只靠进度回调里的取消检查会多写一张标签。
            if sess.cancelled() {
                return;
            }
            let _ = state.with_manager_mut(|s| {
                s.replace_ai_tags(done.image_id, &done.tags)
                    .map_err(|e| e.to_string())
            });
        },
    );

    for e in &stats.errors {
        sess.log(format!("跳过 {e}"));
    }
    let secs = t_start.elapsed().as_secs_f64();

    if sess.cancelled() {
        sess.log(format!(
            "已停止：处理 {} 张（成功 {} · 失败 {}）· 用时 {secs:.1} 秒",
            stats.ok + stats.failed,
            stats.ok,
            stats.failed
        ));
        sess.with(|r| {
            if let Some(t) = r.tag.as_mut() {
                // 停止后 pipeline 不该再跑人脸阶段，所以这里无条件结束会话
                t.finished = true;
                // 没跑完就不算成功：前端按 `ok` 区分「完成」与「中断」
                t.ok = false;
            }
            r.ok = false;
        });
        return;
    }

    sess.log(format!(
        "完成：成功 {} 张 · 失败 {} 张 · 用时 {:.1} 秒（{:.0} ms/张）",
        stats.ok,
        stats.failed,
        secs,
        if stats.ok + stats.failed > 0 {
            secs * 1000.0 / (stats.ok + stats.failed) as f64
        } else {
            0.0
        }
    ));

    sess.with(|r| {
        if let Some(t) = r.tag.as_mut() {
            t.finished = mark_finished;
            t.ok = stats.failed == 0;
        }
        r.ok = stats.failed == 0;
    });
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
fn run_face_batch_blocking(app: tauri::AppHandle, library: String, mark_finished: bool) {
    use pv_ai::cluster::{two_stage_cluster, ClusterOpts};
    use pv_ai::face::{FaceDetector, FaceRecognizer};
    use pv_ai::face_batch::{detect_batch, FaceBatchOpts, FaceProgress};

    // ⚠️ 会话凭据要在**一条语句**里取出：`State<'_, T>` 借用 `app`，
    // 带着它往下走会和 `state.with_manager(…)` 的借用叠加，判定生命周期不足。
    let Some(sess) = Session::take(&app) else {
        return;
    };
    let state = app.state::<AppState>();
    let t_start = std::time::Instant::now();

    sess.log(format!("开始识别人像：{library}"));

    /*
     * ── 1. 查图列表 ──
     *
     * **混合策略**：打过标的图按标签筛掉明显不是人像的（截图/图表/扫描件），
     * 没打过标的图照常处理 —— 既不依赖打标，又能在打过标后少做无用功。
     */
    let images: Vec<(i64, PathBuf)> = match state.with_manager(|s| {
        let all = collect_images(s, &library)?;
        let ids: Vec<i64> = all.iter().map(|(id, _)| *id).collect();
        let skip = s.non_portrait_ids(&ids).map_err(|e| e.to_string())?;
        let kept: Vec<(i64, PathBuf)> = all
            .into_iter()
            .filter(|(id, _)| !skip.contains(id))
            .collect();
        Ok(kept)
    }) {
        Ok(v) => v,
        Err(e) => return sess.fail(e),
    };

    sess.with_tag(|t| {
        t.phase = "face".into();
        t.face_total = images.len();
        t.total = images.len();
    });
    sess.log(format!(
        "范围：{} · 待处理 {} 张",
        if library.is_empty() { "全部图库" } else { &library },
        images.len()
    ));

    // ── 2. 模型 ──
    let dir = face_dir();
    let mut det = match FaceDetector::load(&dir) {
        Ok(d) => d,
        Err(e) => return sess.fail(format!("检测模型加载失败：{e}")),
    };
    let mut rec = match FaceRecognizer::load(&dir) {
        Ok(r) => r,
        Err(e) => return sess.fail(format!("识别模型加载失败：{e}")),
    };
    sess.log("模型就绪 · YuNet + SFace".into());

    // ── 3. 检测 + 特征（锁外）──
    let opts = FaceBatchOpts::default();
    let mut last_log = 0usize;
    let (faces, stats) = detect_batch(
        &images,
        &mut det,
        &mut rec,
        &opts,
        |p: &FaceProgress| {
            sess.with_tag(|t| {
                t.phase = "face".into();
                t.face_done = p.done;
                t.done = p.done;
            });
            if p.done == p.total || p.done - last_log >= 10 {
                last_log = p.done;
                let tail = if p.faces > 0 {
                    format!("（{} 张脸）", p.faces)
                } else {
                    String::new()
                };
                sess.log(format!("{}/{} · {}{tail}", p.done, p.total, p.name));
            }
            // ⛔ 返回 false = 用户点了停止 → 不再解下一张
            !sess.cancelled()
        },
    );

    /*
     * 停止发生在检测阶段：已经解过的图不必丢掉 —— 它们的人脸行还没写库，
     * 但聚类只用内存向量，半批数据聚出来的簇会严重失真（10 张脸的簇
     * 和 300 张脸的簇是完全不同的东西）。所以这里直接结束，不留半成品。
     */
    if sess.cancelled() {
        sess.log(format!(
            "已停止：已检测 {} 张（{} 张脸）· 未写库 · 用时 {:.1} 秒",
            stats.with_face + stats.failed,
            stats.faces,
            t_start.elapsed().as_secs_f64()
        ));
        sess.with(|r| {
            if let Some(t) = r.tag.as_mut() {
                t.finished = true;
                t.ok = false;
            }
            r.ok = false;
        });
        return;
    }

    sess.log(format!(
        "检测完成：{} 张图有脸 · 共 {} 张脸 · 失败 {} 张",
        stats.with_face, stats.faces, stats.failed
    ));
    for e in &stats.errors {
        sess.log(format!("跳过 {e}"));
    }
    if faces.is_empty() {
        return sess.fail("一张脸都没检测到（检查图库内容或降低检测门槛）".into());
    }

    // ── 4. 聚类（锁外）──
    sess.log("聚类中…".into());
    let embs: Vec<Vec<f32>> = faces.iter().map(|f| f.embedding.clone()).collect();
    let c_opts = ClusterOpts::default();
    let labels = two_stage_cluster(&embs, &c_opts);
    let mut sizes: std::collections::HashMap<i32, usize> = std::collections::HashMap::new();
    for &l in &labels {
        *sizes.entry(l).or_insert(0) += 1;
    }
    let multi = sizes.values().filter(|&&c| c >= c_opts.min_core).count();
    let singles = sizes.values().filter(|&&c| c < c_opts.min_core).count();
    sess.log(format!(
        "聚类完成：{} 个簇（≥{} 张的 {multi} 个）· 零散 {singles} 组",
        sizes.len(),
        c_opts.min_core
    ));

    // ── 5. 写库（锁内）──
    sess.log("写库中…".into());
    let ver = format!(
        "sface_2021dec@{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    );

    let res = state.with_manager_mut(|s| {
        let conn = s.conn();

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

        /*
         * 4c. 清理**孤儿人物** —— 本次替换 faces 后，原来挂着这些人脸的人物可能一张脸
         * 都不剩了（比如某个「人物」在这批图里只有 2 张脸，重跑后聚类变了/被过滤了）。
         *
         * ⛔ 不能用 `DELETE FROM persons`（全表清空）：重跑「二次元」11 张图会把
         * 其他图库里的 30/41 张脸的大人物一起删掉 —— 那是真实数据事故。
         * 只删「没有任何 face 指向」的人物，其他图库的人物原封不动。
         */
        conn.execute(
            "DELETE FROM persons WHERE id NOT IN              (SELECT DISTINCT person_id FROM faces WHERE person_id IS NOT NULL)",
            [],
        )
        .map_err(|e| e.to_string())?;

        // 4d. 按簇建人物（只给 ≥ min_core 张的簇建，单例只留 faces 行）
        let mut by_label: std::collections::HashMap<i32, Vec<usize>> =
            std::collections::HashMap::new();
        for (i, &l) in labels.iter().enumerate() {
            by_label.entry(l).or_default().push(i);
        }
        let mut made = 0usize;
        let mut dropped = 0usize;
        for (_, idxs) in by_label {
            if idxs.len() < c_opts.min_core {
                continue;
            }
            /*
             * 建人物前先验「这一簇像不像真人」：
             * YuNet 会在公仔/手办/镜面反光上误检，那些假脸的分数上不去
             * （整簇 0.62~0.77，一张 ≥0.85 的都没有），真人簇总有高分正脸。
             * ⛔ 只影响「建不建人物」—— faces 行照写，其他人物不受影响。
             */
            let scores: Vec<f32> = idxs.iter().map(|&i| faces[i].score).collect();
            if !pv_ai::face_batch::is_likely_real_person(&scores) {
                dropped += 1;
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
        Ok((made, dropped, face_ids.len()))
    });

    let secs = t_start.elapsed().as_secs_f64();
    match res {
        Ok((made, dropped, written)) => {
            let extra = if dropped > 0 {
                format!(" · 跳过 {dropped} 个疑似误检簇")
            } else {
                String::new()
            };
            sess.log(format!(
                "完成：写入 {written} 张脸 · 建了 {made} 个人物{extra} · 用时 {secs:.1} 秒"
            ));
            sess.with(|r| {
                if let Some(t) = r.tag.as_mut() {
                    t.finished = mark_finished;
                    t.ok = true;
                }
                r.ok = true;
            });
        }
        Err(e) => sess.fail(format!("写库失败：{e}")),
    }
}

/// 人脸模型目录。
fn face_dir() -> PathBuf {
    asset_dir("face", "sface.onnx")
}

/// 找 `assets/ai/<sub>`（`marker` 用来判断目录对不对）。
///
/// ⭐ **从 exe 所在目录逐级往上找**，不用 `env!("CARGO_MANIFEST_DIR")`：
/// - 发行版：exe 旁边就是 `assets/ai/face`（第 0 级命中）
/// - 开发机：`release/` 或 `target/debug/examples/` 往上几级才到项目根
///
/// ⛔ 之前用编译期路径 `join("../../assets/ai/face")` —— 但本文件属于
/// `apps/desktop/src-tauri`，往上**两级**是 `apps/`，少了一级，
/// 于是拼出 `apps/assets/ai/face/yunet.onnx` 这种不存在的路径。
fn asset_dir(sub: &str, marker: &str) -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(start) = exe.parent() {
            if let Some(p) = asset_dir_from(start, sub, marker) {
                return p;
            }
        }
    }
    // 兜底：编译期路径（从 apps/desktop/src-tauri 往上**三级**才到项目根）
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../assets/ai")
        .join(sub)
}

/// 收集待处理的图（`library` 为空 = 全部图库）。
fn collect_images(
    s: &pv_store::Store,
    library: &str,
) -> Result<Vec<(i64, std::path::PathBuf)>, String> {
    let q = pv_store::ImageQuery::default();
    let imgs = if library.is_empty() {
        s.query(&q).map_err(|e| e.to_string())?
    } else {
        let libs = pv_index::repo::libraries::list(s.conn()).map_err(|e| e.to_string())?;
        let Some(lib) = libs.into_iter().find(|l| l.name == library) else {
            return Err(format!("找不到图库「{library}」"));
        };
        s.query_in_library(&q, lib.id).map_err(|e| e.to_string())?
    };
    let mut out = Vec::with_capacity(imgs.len());
    for img in imgs {
        if let Ok(Some(p)) = s.abs_path(&img) {
            out.push((img.id, p));
        }
    }
    Ok(out)
}

/// 图库的 AI 处理状态（前端据此决定按钮文案与是否二次确认）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryAiStatus {
    /// 该范围内打过 AI 标签的图数
    pub tagged: usize,
    /// 该范围内已有人脸的图数
    pub faced: usize,
    /// 范围内的总图数
    pub total: usize,
}

/// 查某个范围（空 = 全部）已经处理到什么程度。
#[tauri::command]
pub async fn ai_library_status(
    state: State<'_, AppState>,
    library: String,
) -> Result<LibraryAiStatus, ErrorDto> {
    state
        .with_manager(|s| {
            let imgs = collect_images(s, &library)?;
            let ids: Vec<i64> = imgs.iter().map(|(id, _)| *id).collect();
            let (tagged, faced) = s.ai_coverage(&ids).map_err(|e| e.to_string())?;
            Ok(LibraryAiStatus {
                tagged,
                faced,
                total: ids.len(),
            })
        })
        .map_err(err)
}

/// **一键 AI 识别**：先打标，再识别人像，串行跑完。
///
/// ## 为什么不做成前端串行
/// 前端串行要靠轮询发现「第一步结束了」再发第二个请求，中间有任何抖动
/// （用户切页、请求超时）就会卡住。放在后端一个函数里，
/// 两阶段的进度写进同一个 `Running`，前端只看一个任务。
///
/// ⚠️ 第一阶段传 `mark_finished = false`：跑完不能把 `finished` 置真，
/// 否则前端会在打标结束时就认为整体完成，第二阶段的日志和进度全看不到。
fn run_pipeline_blocking(app: tauri::AppHandle, library: String) {
    run_tag_batch_blocking(app.clone(), library.clone(), false);

    /*
     * 打标阶段结束后要不要接着跑人脸？
     *
     * 三种收尾状态都会把 `finished` 置真：
     * - 失败（`sess.fail`）→ `ok = false`
     * - 用户点了停止 → `ok = false`
     * - 跑完（`mark_finished = false` 只把 `ok` 置真，`finished` 仍为假）
     *
     * 所以「已结束且不成功」= 失败或中断，两种都不该再跑人脸 —— 尤其是中断：
     * 用户按下停止的意图是「现在别算了」，接着起一个人脸阶段等于无视他的操作。
     */
    let aborted = {
        let runner: Arc<Mutex<Option<Running>>> = app.state::<AiRunner>().0.clone();
        let g = runner.lock().ok();
        let bad = g
            .as_ref()
            .and_then(|o| o.as_ref())
            .and_then(|r| r.tag.as_ref())
            .map(|t| t.finished && !t.ok)
            .unwrap_or(true); // 槽位已空 = 会话被顶掉，同样不该继续
        drop(g);
        bad
    };
    if aborted {
        return;
    }

    run_face_batch_blocking(app, library, true);
}

/// 词表里的一级类目名（只读 `tag_meta.json`，**不加载模型**）。
///
/// 读不到就返回空 —— 调用方（`drop_redundant_l1_tags`）拿空列表是空操作，
/// 绝不会因为词表缺失而误删标签。
pub(crate) fn vocab_l1_names() -> Vec<String> {
    pv_ai::tagger::vocab_l1_names(&tagger_dir()).unwrap_or_default()
}

/// 打标模型目录。
fn tagger_dir() -> PathBuf {
    asset_dir("tag", "tag_meta.json")
}

/// 从 `start` 逐级往上找 `assets/ai/<sub>`（独立成函数是为了能单测）。
fn asset_dir_from(start: &std::path::Path, sub: &str, marker: &str) -> Option<PathBuf> {
    let mut d = start.to_path_buf();
    // 6 级够覆盖 target/debug/examples → 项目根
    for _ in 0..6 {
        let p = d.join("assets/ai").join(sub);
        if p.join(marker).is_file() {
            return Some(p);
        }
        if !d.pop() {
            break;
        }
    }
    None
}

#[cfg(test)]
mod asset_tests {
    use super::asset_dir_from;
    use std::path::{Path, PathBuf};

    /// 项目根（本 crate 在 `apps/desktop/src-tauri`，往上三级）
    fn root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
    }

    /// 开发机从 `release/` 跑 exe 时，必须能往上找到项目根里的模型。
    ///
    /// ⛔ 之前用编译期 `join("../../assets/...")`，少了一级，拼出
    /// `apps/assets/...`（不存在）—— 用户点「开始」直接报模型加载失败。
    /// 这个测试就是防它再来一次。
    #[test]
    fn 从_release_目录能往上找到模型目录() {
        let start = root().join("release");
        let got = asset_dir_from(&start, "face", "yunet.onnx");
        assert!(got.is_some(), "从 release/ 往上没找到 assets/ai/face");
        assert!(got.unwrap().join("yunet.onnx").is_file());
    }

    #[test]
    fn 从_examples_目录也能找到() {
        let start = root().join("target/debug/examples");
        let got = asset_dir_from(&start, "tag", "tag_meta.json");
        assert!(got.is_some(), "从 target/debug/examples 往上没找到 assets/ai/tag");
    }

    #[test]
    fn 找不到就返回_none_而不是瞎猜() {
        let start = Path::new("C:/definitely/not/here");
        assert!(asset_dir_from(start, "face", "yunet.onnx").is_none());
    }
}

fn read_f32(state: &AppState, key: &str, def: f32) -> f32 {
    state
        .with_manager(|s| Ok(s.setting(key).ok().flatten()))
        .ok()
        .flatten()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(def)
}

/// 启动一键任务。`task` 只有几种：`tag` / `face` / `pipeline`。
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
    /*
     * ⚠️ 防重入必须用 `is_active()`，⛔ 不能只看 `child.is_some()`
     * （2026-11 修）：Rust 任务的 `child` **恒为 `None`**，以前这么写等于
     * 完全没防 —— 用户在打标进行中再点一次「开始」，后端就真的又起一个
     * `spawn_blocking` 线程，两个线程同时往索引里 replace 标签。
     */
    if guard.as_ref().is_some_and(Running::is_active) {
        return Err(err("已有任务在跑，先等它结束或点「停止」".into()));
    }

    /*
     * ⛔ **不要在这里要求 Python / 项目目录**（2026-10-31 修）。
     *
     * 打标与人脸都已经换成 Rust（ort + DirectML / YuNet + SFace），
     * 下面三条路径全都不碰 Python。但这两行以前写死在函数开头 ——
     * venv 一被删（本机 `J:/ai/tts/.venv` 就没了），**所有 AI 任务**都会在
     * 真正开始之前直接报「没找到 Python」，表现就是「点了 AI 识别没反应」。
     * 现在只在最后那段历史遗留的 Python 链路里才要求它们。
     */

    // 阈值不在这里读：Rust 打标在 `run_tag_batch_blocking` 里自己读设置，
    // `tools/ai/*.py` 也是自己去库里读 —— 这里读出来只会是一份没人用的死值。

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
        tauri::async_runtime::spawn_blocking(move || run_tag_batch_blocking(app2, lib2, true));
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
        tauri::async_runtime::spawn_blocking(move || run_face_batch_blocking(app2, lib2, true));
        return Ok(path);
    }

    /*
     * ⭐ `pipeline` = 一键 AI 识别：先打标、再识别人像，后端串行跑完。
     * 前端只看一个任务，两阶段进度都在 `TagProgress` 里。
     */
    if task == "pipeline" {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let log = std::env::temp_dir().join(format!("pve-ai-pipe-{stamp}.log"));
        let _ = std::fs::write(&log, b"");
        let path = log.display().to_string();
        *guard = Some(Running::tag_session(log));
        drop(guard);

        let app2 = app.clone();
        let lib2 = library.clone();
        tauri::async_runtime::spawn_blocking(move || run_pipeline_blocking(app2, lib2));
        return Ok(path);
    }

    /*
     * ⬇️ 以下是**历史遗留的 Python 链路**：`tag` / `face` / `pipeline` 三条
     * Rust 路径都已经 return 了，走不到这里。留着是为了以后真要加一个
     * 纯 Python 的新任务时有地方挂 —— 也正因如此，`resolve_python`
     * 和 `project_root` 的检查只能放在这里，不能提到函数开头。
     */
    #[allow(unreachable_code)]
    let python = resolve_python(&state).ok_or_else(|| {
        err("没找到 Python。这是开发版功能：请在设置里填 Python 路径".into())
    })?;
    #[allow(unreachable_code)]
    let root = project_root()
        .ok_or_else(|| err("找不到项目目录（tools/ai 脚本所在位置）".into()))?;

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
        // Python 链路靠 kill 停，取消标志用不上；但仍要发号，`Session` 才能区分会话
        id: next_session_id(),
        cancel: Arc::new(AtomicBool::new(false)),
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
            ..Default::default()
        });
    };

    // Rust 打标：进度在 `tag` 里，和子进程那套完全分开
    if let Some(t) = run.tag.as_ref() {
        return Ok(AiTaskStatus {
            running: !t.finished,
            done: t.finished,
            ok: t.ok,
            lines: t.lines.iter().cloned().collect(),
            phase: t.phase.clone(),
            tag_done: t.tag_done,
            tag_total: t.tag_total,
            face_done: t.face_done,
            face_total: t.face_total,
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
        ..Default::default()
    })
}

/// 停止当前任务。
///
/// ## 两条完全不同的路
/// - **Python 子进程**（历史链路）：有 pid，`kill` + `wait` 收掉孤儿进程。
/// - **Rust 批处理**（现在 `tag` / `face` / `pipeline` 全走这条）：
///   跑在本进程的 `spawn_blocking` 线程里，**没有 pid 可杀**，唯一的停止方式
///   是让它自己看到取消标志。
///
/// ⛔ 以前这里只 kill `child`（Rust 路径恒 `None`）就把槽位清空 —— 界面立刻
/// 显示「已停止」，而线程还在解码、推理、**往索引里写标签**，能持续几十分钟。
/// 现在必须先 `cancel.store(true)` 让线程**自己**在下一张图之前收手。
#[tauri::command]
pub async fn ai_task_stop(runner: State<'_, AiRunner>) -> Result<(), ErrorDto> {
    let mut guard = runner.0.lock().map_err(|_| err("任务状态锁坏了".into()))?;
    if let Some(run) = guard.as_mut() {
        if let Some(child) = run.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        // Rust 路径的实际停止信号；`spawn_blocking` 线程在每张图上查它。
        // ⚠️ 顺序要紧：先置标志，线程才有可能在下一次检查时看到。
        run.cancel.store(true, Ordering::Relaxed);
        run.steps.clear();
        run.child = None;
    }
    /*
     * 槽位清空后，`Session::with` 的 `r.id == self.id` 校验会让旧线程的所有
     * 写入静默丢弃 —— 所以这里不需要（也没法）等线程真正退出，界面不会因为
     * 旧线程的进度回写而「停止之后又自己跳回来」。
     */
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

    /// 轮询每 500ms 一次，日志能有几 MB —— 只读尾部、且不能露出被切半的行。
    #[test]
    fn 日志只读尾部且不留半行() {
        let dir = std::env::temp_dir();
        let big = dir.join(format!("pve-tail-big-{}.log", std::process::id()));
        let mut body = String::new();
        for i in 0..20_000 {
            body.push_str(&format!("第 {i} 行 · 每行都带点中文让它有点长度\n"));
        }
        std::fs::write(&big, &body).unwrap();

        // ~1MB，远超 64KB 窗口 —— 走「取尾部 + 丢掉半行」那条路
        let got = tail_lines(&big, 200);
        assert_eq!(got.len(), 200);
        assert_eq!(
            got.last().unwrap(),
            "第 19999 行 · 每行都带点中文让它有点长度"
        );
        assert_eq!(got[0], "第 19800 行 · 每行都带点中文让它有点长度");

        // 小文件整份读，第一行不能被当成半行丢掉
        let small = dir.join(format!("pve-tail-small-{}.log", std::process::id()));
        std::fs::write(&small, "a\nb\nc\n").unwrap();
        assert_eq!(tail_lines(&small, 10), vec!["a", "b", "c"]);
        assert_eq!(tail_lines(&small, 2), vec!["b", "c"]);

        // 文件不存在 → 空，不 panic
        assert!(tail_lines(&dir.join("pve-tail-nope-不存在.log"), 10).is_empty());

        let _ = std::fs::remove_file(&big);
        let _ = std::fs::remove_file(&small);
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

    /*
     * ── 任务生命周期护栏（2026-11 修的两处真 bug）──
     *
     * 这两个测试都不需要 `AppHandle`：它们只覆盖纯状态判断，
     * 而 bug 恰恰就出在状态判断上。
     */

    fn 槽位() -> Arc<Mutex<Option<Running>>> {
        Arc::new(Mutex::new(None))
    }

    fn 临时日志() -> PathBuf {
        std::env::temp_dir().join(format!("pve-guard-{}.log", std::process::id()))
    }

    /// ⛔ 防重入不能只看 `child.is_some()` —— Rust 任务的 `child` 恒为 `None`，
    /// 于是「打标进行中再点开始」会起第二个线程，两个线程同时 replace 标签。
    #[test]
    fn 打标会话没结束就算活跃() {
        let mut r = Running::tag_session(临时日志());
        assert!(r.child.is_none(), "Rust 任务本来就没有子进程");
        assert!(r.is_active(), "← 这就是旧逻辑漏掉的那种「正在跑」");
        r.tag.as_mut().unwrap().finished = true;
        assert!(!r.is_active(), "结束后要放行下一次任务");
    }

    /// ⛔ 「停止 → 再开始」会把槽位整个换掉，旧线程必须写不进新会话 ——
    /// 否则新任务的进度会被旧线程回写成 3/500，甚至一开就显示「已完成」。
    #[test]
    fn 旧会话写不进新会话() {
        let s = 槽位();
        *s.lock().unwrap() = Some(Running::tag_session(临时日志()));

        // 会话 A 的凭据（等价于 `Session::take` 拿到的东西）
        let (a_id, a_cancel) = {
            let g = s.lock().unwrap();
            let r = g.as_ref().unwrap();
            (r.id, r.cancel.clone())
        };
        let a = Session {
            runner: s.clone(),
            id: a_id,
            cancel: a_cancel.clone(),
        };

        // 用户点停止 → 立刻再开始（槽位换成会话 B）
        a_cancel.store(true, Ordering::Relaxed);
        *s.lock().unwrap() = Some(Running::tag_session(临时日志()));
        assert!(a.cancelled(), "旧会话看得到停止标志");

        // 旧线程照写：必须一行都进不了新会话
        a.log("旧线程的一行".into());
        a.with_tag(|t| t.done = 3);
        {
            let g = s.lock().unwrap();
            let t = g.as_ref().unwrap().tag.as_ref().unwrap();
            assert!(t.lines.is_empty(), "新会话日志被旧线程污染：{:?}", t.lines);
            assert_eq!(t.done, 0, "新会话进度被旧线程改写");
        }

        // 同一会话内部照常可写（别把护栏做成「谁都不能写」）
        let b = Session {
            runner: s.clone(),
            id: s.lock().unwrap().as_ref().unwrap().id,
            cancel: Arc::new(AtomicBool::new(false)),
        };
        b.log("新线程的一行".into());
        let g = s.lock().unwrap();
        assert_eq!(g.as_ref().unwrap().tag.as_ref().unwrap().lines.len(), 1);
    }
}
