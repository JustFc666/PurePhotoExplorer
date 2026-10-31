//! 真正干活的批处理：打标、人脸、一键流水线。
//!
//! 三个 `run_*_blocking` 都在 `spawn_blocking` 线程里跑，**不占 UI 线程**；
//! 进度与日志通过 [`super::Session`] 写回槽位（旧会话写不进新会话）。
//!
//! ⛔ 取消检查要放在**进度回调**和 **`on_done`** 两处：`on_done` 在
//! `on_progress` 之前回调，只查一处会多写一张标签。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{Manager, State};

use crate::commands::dto::ErrorDto;
use crate::state::AppState;

use super::config::{
    read_f32, DEF_MAX_TAGS, DEF_THRESHOLD, DEF_THRESHOLD_L1, KEY_MAX_TAGS, KEY_THRESHOLD,
    KEY_THRESHOLD_L1,
};
use super::{err, AiRunner, Running, Session};


/// 跑一次 Rust 打标（在 `spawn_blocking` 里执行，不占 UI 线程）。
///
/// 三阶段，索引锁只在两头各占一下：
/// 1. 锁内查图列表（毫秒级）
/// 2. **锁外**逐张解码 + 推理（1~2 秒/张，期间图库滚动不受影响）
/// 3. 回调里逐张写库（锁粒度 = 一张图，毫秒级）
///
/// 「停止」靠 `Session::cancelled()` 在**每张图**上检查：先判退、再判写库，
/// 所以按下停止之后不会再有新的标签落库。
pub(super) fn run_tag_batch_blocking(app: tauri::AppHandle, library: String, mark_finished: bool) {
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
pub(super) fn run_face_batch_blocking(app: tauri::AppHandle, library: String, mark_finished: bool) {
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
#[derive(Debug, Clone, Serialize)]
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
pub(super) fn run_pipeline_blocking(app: tauri::AppHandle, library: String) {
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
