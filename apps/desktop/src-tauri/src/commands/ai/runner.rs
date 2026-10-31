//! AI 任务的生命周期：启动、轮询、停止。
//!
//! 三条任务（`tag` / `face` / `pipeline`）都已经走 Rust，跑在
//! `spawn_blocking` 线程里 —— **没有子进程可以 kill**，所以「停止」靠
//! [`super::Running::cancel`] 这个标志让批处理自己收手（见 [`super::Session`]）。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use crate::commands::dto::ErrorDto;
use crate::state::AppState;

use super::jobs::{run_face_batch_blocking, run_pipeline_blocking, run_tag_batch_blocking};
use super::{err, next_session_id, AiRunner, Running, Step};


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
    use super::*;
    use super::super::Session;
    use std::sync::Mutex;

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
