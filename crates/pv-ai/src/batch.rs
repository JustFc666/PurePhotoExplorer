//! 批量打标：给一批图跑「解码 → 向量 → 打分 → 筛选」。
//!
//! ## 为什么用回调而不是直接写库
//! 这一层**只管算**：读图、推理、按阈值筛出标签，把结果交出去。
//! 写库（SQLite）由调用方注入 —— 好处是这里可以单测（不依赖数据库），
//! 而且「锁」只发生在调用方的写库回调里，模型推理期间不占索引锁。
//!
//! ⚠️ 这一点很关键：一张图解码 + 推理要 1~2 秒，如果整个循环都在持锁，
//! 图库滚动会卡住（用户的直觉是「AI 跑批时界面还能用」）。

use std::path::{Path, PathBuf};

use pv_decode::{decode_scaled, BitmapBackend, DecoderRegistry};

use crate::tagger::Tagger;

/// 一次批处理的参数。
#[derive(Debug, Clone)]
pub struct TagOptions {
    /// 二级（具体词）门槛
    pub threshold: f32,
    /// 一级（类目）门槛，通常低于 `threshold`
    pub threshold_l1: f32,
    /// 每张最多几个**二级**标签
    pub max_tags: usize,
    /// 解码长边。1024 与 Python 版（PIL draft）效果最接近；
    /// ⛔ 别降到 640 —— 细节不足会让余弦整体低 0.01，阈值附近整批漏打。
    pub long_side: u32,
}

impl Default for TagOptions {
    fn default() -> Self {
        Self {
            // ⚠️ 阈值和 Python 版（0.40 / 0.36）**不同**：
            // Rust 侧的解码与缩放实现和 PIL 有细微差异，实测余弦系统性低约 0.01，
            // 沿用 Python 的值会成批漏打。这两个数是用 Rust 侧分布重新校准的。
            threshold: 0.39,
            threshold_l1: 0.35,
            max_tags: 6,
            long_side: 1024,
        }
    }
}

/// 一张图的结果。
#[derive(Debug, Clone)]
pub struct TaggedImage {
    pub image_id: i64,
    /// `(词条, 分数)`，已按分数降序、已过阈值、已按 `max_tags` 截断
    pub tags: Vec<(String, f32)>,
}

/// 进度回调的参数。
#[derive(Debug, Clone)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    /// 刚处理完的那张（给界面显示「正在处理 xxx」）
    pub name: String,
}

/// 批处理统计。
#[derive(Debug, Default, Clone)]
pub struct BatchStats {
    pub ok: usize,
    pub failed: usize,
    /// 失败原因（最多保留 20 条，够用户判断是格式问题还是权限问题）
    pub errors: Vec<String>,
}

/// 跑一批图。
///
/// - `on_progress` 每张调一次（调用方负责限流，别每张都刷 UI）
/// - `on_done` 每张**成功**调一次，把标签交给调用方写库
///
/// 单张失败不影响整批（坏文件、权限、RAW 变体都可能失败）。
pub fn tag_batch<F, G>(
    images: &[(i64, PathBuf)],
    tagger: &mut Tagger,
    opts: &TagOptions,
    mut on_progress: F,
    mut on_done: G,
) -> BatchStats
where
    F: FnMut(&Progress),
    G: FnMut(TaggedImage),
{
    let mut reg = DecoderRegistry::new();
    reg.register(Box::new(BitmapBackend));

    let total = images.len();
    let mut stats = BatchStats::default();

    for (i, (id, path)) in images.iter().enumerate() {
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();

        match tag_one(&reg, tagger, path, opts) {
            Ok(tags) => {
                stats.ok += 1;
                on_done(TaggedImage { image_id: *id, tags });
                on_progress(&Progress {
                    done: i + 1,
                    total,
                    name,
                });
            }
            Err(e) => {
                stats.failed += 1;
                if stats.errors.len() < 20 {
                    stats.errors.push(format!("{name}: {e}"));
                }
                on_progress(&Progress {
                    done: i + 1,
                    total,
                    name,
                });
            }
        }
    }

    stats
}

/// 单张：解码 → 向量 → 打分 → 按阈值筛。
fn tag_one(
    reg: &DecoderRegistry,
    tagger: &mut Tagger,
    path: &Path,
    opts: &TagOptions,
) -> Result<Vec<(String, f32)>, String> {
    let img = decode_scaled(reg, path, opts.long_side).map_err(|e| format!("{e}"))?;
    let emb = tagger.embed(&img)?;
    let scored = tagger.score(&emb);

    // `score()` 已按分数降序，一趟扫完：过门槛取前 max_tags
    let mut out: Vec<(String, f32)> = Vec::new();

    for s in &scored {
        if s.score < opts.threshold {
            continue;
        }
        if out.len() >= opts.max_tags {
            break;
        }
        out.push((s.term.clone(), s.score));
    }

    /*
     * ⚠️ **一级类目不再写库**（2026-10-29 实测后改的）。
     *
     * 之前的做法是从命中的二级词聚合出一级（有「单人」就算「人像」），
     * 结果一级分数和它下面那个二级**完全一样**（生日 0.410 / 活动 0.410），
     * 标签条上并列显示看着像重复打了两遍。
     *
     * 一级的正确位置是**侧栏筛选器**：查「活动」时按词表的 `l1` 映射去匹配
     * 二级标签即可，不需要在每张图上多存一份派生数据。
     */
    Ok(out)
}
