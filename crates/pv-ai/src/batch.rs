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

/// 二级（具体词）门槛默认值。
///
/// ⭐ 这里是**唯一真源**：`TagOptions::default()` 与设置面板显示（`commands/ai`
/// 的 `DEF_*`）都从这里派生 —— 2026-11 之前它们各写一份字面量，改校准时漏改
/// 设置面板那份，就出现「面板显示 0.40、实际按 0.39 跑」的静默偏差。
///
/// ## 为什么是 0.41（2026-11-01 改，原值 0.39）
///
/// 0.39 当年是为了「防漏打」从 Python 侧的 0.40 下调的，代价是**精度崩了**。
/// 全新库（402 张图）实测：1360 条 AI 标签里 **636 条（47%）挤在 0.39–0.40**
/// 这一档；因为 `max_tags` 是「凑够几个算几个」，阈值一低就变成「每张硬凑 6 个」，
/// 尾巴全是随机名次 —— 用户报「海狸照片被打上熊/猴子/婴儿/熊猫」就是这个。
///
/// 同库复算：0.39 → 1360 条（熊猫 18 条，分数 0.390–0.408）；0.41 → 371 条
/// （熊猫 0 条）；0.42 → 200 条。`assets/ai/阈值分析.md` 的人工标定也是
/// 0.41 = 80.7% 精度、0.42 = 84.8%，且两者覆盖都是 52/60（抬到 0.41 不丢图）。
///
/// ⛔ 别再往 0.39 退：那条路上每张图都会被填满，末尾标签的可信度接近掷骰子。
pub const DEFAULT_THRESHOLD: f32 = 0.41;
/// 一级（类目）门槛默认值，见 [`DEFAULT_THRESHOLD`]。
pub const DEFAULT_THRESHOLD_L1: f32 = 0.35;
/// 每张最多几个**二级**标签。
pub const DEFAULT_MAX_TAGS: usize = 6;

impl Default for TagOptions {
    fn default() -> Self {
        Self {
            threshold: DEFAULT_THRESHOLD,
            threshold_l1: DEFAULT_THRESHOLD_L1,
            max_tags: DEFAULT_MAX_TAGS,
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
/// - `on_progress` 每张调一次（调用方负责限流，别每张都刷 UI）；
///   **返回 `false` 表示「到此为止」**，循环立刻收尾并返回已累计的统计。
/// - `on_done` 每张**成功**调一次，把标签交给调用方写库
///
/// 单张失败不影响整批（坏文件、权限、RAW 变体都可能失败）。
///
/// ## 为什么进度回调要能叫停
/// 取消标志没法放进这一层（这里是无状态的纯函数，要能单测）。而
/// 「停止」只可能是调用方的事 —— 所以最省事的通道就是让回调的返回值
/// 本身成为信号，不必引入 `Arc<AtomicBool>` 之类的共享状态。
///
/// ⚠️ **先 `on_done` 再 `on_progress`**（成功那一支）：写库发生在 `on_done`，
/// 所以调用方在 `on_progress` 里看到的 `done` 已经包含刚落库的这张。
/// 反过来说，**取消检查必须两处都放** —— 只放 `on_progress` 的话，
/// 最后一张仍会被写进索引，用户点了停止却多出一张标签。
pub fn tag_batch<F, G>(
    images: &[(i64, PathBuf)],
    tagger: &mut Tagger,
    opts: &TagOptions,
    mut on_progress: F,
    mut on_done: G,
) -> BatchStats
where
    F: FnMut(&Progress) -> bool,
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

        let keep_going = match tag_one(&reg, tagger, path, opts) {
            Ok(tags) => {
                stats.ok += 1;
                on_done(TaggedImage { image_id: *id, tags });
                on_progress(&Progress {
                    done: i + 1,
                    total,
                    name,
                })
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
                })
            }
        };
        if !keep_going {
            break;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 防止 clippy 把断言当常量直接优化掉：这里故意绕一层函数调用，
    /// 让「默认值」在运行时才被读到。
    fn 默认口径() -> (f32, f32) {
        (DEFAULT_THRESHOLD, DEFAULT_THRESHOLD_L1)
    }

    /// 默认阈值是**发布口径**，不能被随手调低。
    ///
    /// 0.39 的实测后果：全库 1360 条标签里 47% 落在阈值上方 0.01 以内，
    /// 每张图被 `max_tags` 强制填满，尾巴出现「海狸 → 熊猫」这类随机词。
    #[test]
    fn 默认阈值不得低于标定下限() {
        let (th, th_l1) = 默认口径();
        assert!(
            th >= 0.40,
            "默认二级阈值被调回 {th}：会让每张图凑满标签，噪声尾巴复发"
        );
        assert!(th <= 0.45, "默认二级阈值 {th} 过高：覆盖会从 52/60 跳水");
        assert!(th_l1 < th, "一级（类目）门槛必须低于二级（具体词）");
    }
}
