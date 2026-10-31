//! AI 打标 / 人像识别的命令层。
//!
//! ## 这个目录里有什么
//! | 文件 | 职责 |
//! |---|---|
//! | [`config`] | 阈值/开关的读写（`ai_config` / `ai_save_config`）|
//! | [`vocab`] | 主题词表：查看、启停、导出、导入（`ai_vocab*`）|
//! | [`runner`] | 任务生命周期：启动、轮询、停止 |
//! | [`jobs`] | 真正干活的批处理：打标 / 人脸 / 一键流水线 |
//!
//! 本文件只放**四个子模块共享的东西**：错误包装、任务槽位类型
//! （[`Running`] / [`Session`] / [`TagProgress`]）和 `AiRunner` 托管状态。
//!
//! ## 为什么这么拆（2026-11）
//! 原来是一个 1888 行的 `ai.rs`：找「缩略图怎么取」要滚过阈值参数，
//! 改一个阈值要在一千多行里确认还有没有第二份字面量。按上面四格切开后，
//! 每个文件只回答一个问题。
//!
//! ## 词表从哪来
//! 编译期 `include_str!` 进来（`crates/pv-ai/vocab/zh_tags.csv`，由
//! `merge_thesaurus.py` 生成），所以**安装包里自带一份完整词表**，不依赖外部文件。
//!
//! ## 用户改了什么存在哪
//! 只存**覆盖项**：`settings` 表里的 `ai.disabled_terms`（JSON 数组）。
//! ⛔ 不要把整份词表复制进数据库 —— 词表升级后旧副本会成为脏数据，
//! 而「禁用名单」这种差量永远跟得上新词表。
//!
//! ## 推理在哪跑
//! 全部在 Rust（`pv-ai`：ort + DirectML / YuNet + SFace），发行版不需要
//! Python / torch。只有 `ai_run_task` 末尾那段**历史遗留**的 Python 链路还会
//! 找 `tools/ai/*.py`，正常三条路径都走不到它。

use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::Child;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tauri::Manager;

use crate::commands::dto::ErrorDto;

mod config;
mod jobs;
mod runner;
mod vocab;

/*
 * 对外路径保持不变：`commands::ai::ai_config` → `commands::ai_config`
 * 仍然有效（`commands/mod.rs` 那一层也是 `pub use ai::*`）。
 *
 * ⛔⛔ **必须用 `*`，不能改成逐项 `pub use config::{ai_config, ...}`**：
 * `#[tauri::command]` 除了函数本体，还会在**定义它的模块里**额外生成一个
 * 隐藏项（`__cmd__ai_config`），而 `generate_handler!` 展开时是按
 * 「父模块::__cmd__名字」去找的。逐项列名只搬函数、不搬隐藏项 →
 * `lib.rs` 里那十行注册全部报 `cannot find __cmd__ai_config`。
 * 2026-11 拆分层时踩过一次，错误信息只说找不到 `__cmd__`，很难看出原因。
 */
pub use config::*;
pub use jobs::*;
pub use runner::*;
pub use vocab::*;


/// 统一的错误包装（`ErrorDto` 在别处是按 PvError 构造的，这里没有 `new`）。
fn err(msg: String) -> ErrorDto {
    ErrorDto {
        code: "ai_error".into(),
        message: msg,
        user_visible: true,
    }
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

/// 一步 = 跑一个 Python 脚本。
struct Step {
    script: &'static str,
    args: Vec<String>,
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
