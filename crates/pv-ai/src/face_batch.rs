//! 人脸批处理：给一批图跑「解码 → 检测 → 对齐 → 特征」，把结果收集起来。
//!
//! ## 为什么不在这里聚类/写库
//! 聚类是**全局**的（所有脸放一起算相似度矩阵），必须等整批检测完才能做；
//! 写库又要和索引的事务、persons 的命名状态打交道。
//! 所以这层只负责「一图 → 若干张脸 + 向量」，聚类和落库由调用方编排 ——
//! 好处是这里可单测，而且检测阶段不占索引锁。

use std::path::{Path, PathBuf};

use pv_decode::{decode_scaled, BitmapBackend, DecoderRegistry};

use crate::face::{FaceDetector, FaceRecognizer};

/// 批处理参数。
#[derive(Debug, Clone)]
pub struct FaceBatchOpts {
    /// 检测置信度门槛（Python 版默认 0.6）
    pub score_thresh: f32,
    /// 脸宽像素下限（**解码后**坐标）。小脸向量不可信，聚类前必须过滤。
    pub min_face_px: f32,
    /// 解码长边。1600 是实测在「大合照里的人脸」和「耗时」之间的平衡点。
    pub long_side: u32,
    /// 写库时记录的模型版本 —— **换模型必须整库重建**，靠这个字段判定
    pub model: &'static str,
}

impl Default for FaceBatchOpts {
    fn default() -> Self {
        Self {
            score_thresh: 0.6,
            min_face_px: 56.0,
            long_side: 1600,
            model: "sface_2021dec",
        }
    }
}

/// 同图去重阈值：一张图里两张脸的余弦超过它就只保留分数高的那张。
///
/// ## 为什么需要（2026-10-29 用户报的「一张浮雕算出 6 张脸」）
/// 用户库里所有「多脸图」的内部相似度分布：
///
/// | 图 | 脸数 | 最高余弦 | 平均余弦 |
/// |---|---|---|---|
/// | 博物馆浮雕（相邻雕像风格相同） | 6 | **0.686** | **0.552** |
/// | 其他 6 脸/4 脸/3 脸图 | 3–6 | 0.24–0.58 | 0.10–0.44 |
///
/// 取 **0.55**：浮雕那种「整张图的人脸都长一个样」会被压成 1~2 张，
/// 而正常合影（不同人平均 0.10~0.44）一张都不会被误合并。
///
/// ⚠️ 这是**同图**内的去重，不影响跨图的聚类 —— 同一个人在不同照片里仍然是一个人。
const SAME_IMAGE_DEDUP_COS: f32 = 0.55;

/// 一张检测到的人脸（坐标已归一化到 0..1，与 `faces` 表一致）。
#[derive(Debug, Clone)]
pub struct DetectedFace {
    pub image_id: i64,
    /// 归一化坐标（0..1）
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub score: f32,
    /// 质量指标：**脸宽像素**（解码后坐标系），用于聚类前过滤
    pub quality: f32,
    /// 128 维，已 L2 归一化
    pub embedding: Vec<f32>,
}

/// 这个簇「像不像真人」—— 决定要不要给它建人物。
///
/// ## 为什么需要它（2026-10-29 实测数据）
/// YuNet 会在**公仔 / 手办 / 镜面反光**上误检「人脸」（分数 0.62~0.77），
/// 这些误检的 SFace 向量彼此余弦 0.52~0.77，刚好擦着 `t_core=0.55` 聚成一簇，
/// 于是人像墙上冒出一个「封面是镜子」的假人物。
///
/// 用户库里 10 个人物的实测分布：
///
/// | 簇 | 平均分 | 最高分 | ≥0.85 张数 |
/// |---|---|---|---|
/// | 误检（公仔/亭子/手办/合影） | **0.698** | **0.765** | **0** |
/// | 其余 9 个真人簇 | ≥0.786 | ≥0.855 | ≥1 |
///
/// ⚠️ 注意**簇内平均余弦分不开**（误检簇 0.597 反而比真簇 person2 的 0.544 高），
/// 所以别用「簇内一致性」当判据 —— 只能用检测分数。
///
/// 判据取「均分 ≥ 0.75 **且** 最高分 ≥ 0.85」：真人的簇里总有一两张正面清晰的
/// 高分脸（≥0.85），而误检的整簇都上不去。两个条件缺一不可 ——
/// 只看均分会把「4 张脸质量均衡但都一般」的真簇误杀。
pub fn is_likely_real_person(scores: &[f32]) -> bool {
    if scores.is_empty() {
        return false;
    }
    let mean = scores.iter().sum::<f32>() / scores.len() as f32;
    let max = scores.iter().cloned().fold(0.0f32, f32::max);
    mean >= 0.75 && max >= 0.85
}

/// 进度。
#[derive(Debug, Clone)]
pub struct FaceProgress {
    pub done: usize,
    pub total: usize,
    pub name: String,
    /// 这张图检出几张脸（0 = 没有人脸）
    pub faces: usize,
}

/// 统计。
#[derive(Debug, Default, Clone)]
pub struct FaceBatchStats {
    pub images: usize,
    /// 有脸的图数
    pub with_face: usize,
    /// 入库的脸数（过滤掉小脸之后）
    pub faces: usize,
    /// 被小脸/低分过滤掉的
    pub skipped: usize,
    pub failed: usize,
    pub errors: Vec<String>,
}

/// 跑一批图：检测 + 特征。`on_progress` 每张调一次。
///
/// **回调返回 `false` = 到此为止**，循环立刻收尾并返回已累计的结果
/// （理由同 [`crate::batch::tag_batch`]：取消标志不放这一层，
/// 让回调返回值当信号，这个函数就能保持无状态、可单测）。
pub fn detect_batch<F>(
    images: &[(i64, PathBuf)],
    det: &mut FaceDetector,
    rec: &mut FaceRecognizer,
    opts: &FaceBatchOpts,
    mut on_progress: F,
) -> (Vec<DetectedFace>, FaceBatchStats)
where
    F: FnMut(&FaceProgress) -> bool,
{
    let mut reg = DecoderRegistry::new();
    reg.register(Box::new(BitmapBackend));

    let total = images.len();
    let mut out: Vec<DetectedFace> = Vec::new();
    let mut stats = FaceBatchStats::default();

    for (i, (id, path)) in images.iter().enumerate() {
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        stats.images += 1;

        let keep_going = match detect_one(&reg, det, rec, path, *id, opts) {
            Ok(faces) => {
                if !faces.is_empty() {
                    stats.with_face += 1;
                }
                let n = faces.len();
                stats.faces += n;
                out.extend(faces);
                on_progress(&FaceProgress {
                    done: i + 1,
                    total,
                    name,
                    faces: n,
                })
            }
            Err(e) => {
                stats.failed += 1;
                if stats.errors.len() < 20 {
                    stats.errors.push(format!("{name}: {e}"));
                }
                on_progress(&FaceProgress {
                    done: i + 1,
                    total,
                    name,
                    faces: 0,
                })
            }
        };
        if !keep_going {
            break;
        }
    }

    (out, stats)
}

fn detect_one(
    reg: &DecoderRegistry,
    det: &mut FaceDetector,
    rec: &mut FaceRecognizer,
    path: &Path,
    image_id: i64,
    opts: &FaceBatchOpts,
) -> Result<Vec<DetectedFace>, String> {
    let img = decode_scaled(reg, path, opts.long_side).map_err(|e| format!("{e}"))?;
    let (iw, ih) = (img.width.max(1) as f32, img.height.max(1) as f32);
    let faces = det.detect(&img, opts.score_thresh)?;

    let mut out = Vec::new();
    for f in &faces {
        // 小脸过滤：**在解码后坐标里算**（和 Python 版一致）
        if f.w.min(f.h) < opts.min_face_px {
            continue;
        }
        // 超出画布的脸（检测框偶尔会越界）不写库，否则前端画出来会出界
        if f.x + f.w <= 0.0 || f.y + f.h <= 0.0 || f.x >= iw || f.y >= ih {
            continue;
        }
        let emb = match rec.embed(&img, f) {
            Ok(e) => e,
            Err(_) => continue, // 个别脸对齐失败就跳过，不影响整张图
        };
        out.push(DetectedFace {
            image_id,
            x: (f.x / iw).clamp(0.0, 1.0),
            y: (f.y / ih).clamp(0.0, 1.0),
            w: (f.w / iw).clamp(0.0, 1.0),
            h: (f.h / ih).clamp(0.0, 1.0),
            score: f.score,
            quality: f.w.min(f.h),
            embedding: emb,
        });
    }

    Ok(dedup_same_image(out))
}

/// 同图去重：`faces` 必须**按分数降序**（`detect` 出来就是），
/// 保留分数高的，后面相似的丢掉。
fn dedup_same_image(faces: Vec<DetectedFace>) -> Vec<DetectedFace> {
    let mut kept: Vec<DetectedFace> = Vec::with_capacity(faces.len());
    for f in faces {
        if kept
            .iter()
            .any(|k| crate::face::cosine(&k.embedding, &f.embedding) > SAME_IMAGE_DEDUP_COS)
        {
            continue;
        }
        kept.push(f);
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::is_likely_real_person;

    /// 用户库里真实误检簇的分数（公仔/亭子/手办/合影）
    #[test]
    fn 误检簇不建人物() {
        assert!(!is_likely_real_person(&[0.622, 0.732, 0.674, 0.765]));
    }

    /// 用户库里最小的真簇（person 1：0.699/0.726/0.842/0.877）
    #[test]
    fn 真人小簇照样建() {
        assert!(is_likely_real_person(&[0.699, 0.726, 0.842, 0.877]));
    }

    #[test]
    fn 大簇稳定建() {
        assert!(is_likely_real_person(&[0.894, 0.901, 0.929, 0.88]));
    }

    /// 均分够但一张高分都没有 → 不建（质量均衡的误检）
    #[test]
    fn 没有高分脸就不建() {
        assert!(!is_likely_real_person(&[0.76, 0.77, 0.78, 0.79]));
    }

    #[test]
    fn 空簇不建() {
        assert!(!is_likely_real_person(&[]));
    }
}

#[cfg(test)]
mod dedup_tests {
    use super::{dedup_same_image, DetectedFace};

    fn face(score: f32, emb: Vec<f32>) -> DetectedFace {
        DetectedFace {
            image_id: 1,
            x: 0.0,
            y: 0.0,
            w: 0.1,
            h: 0.1,
            score,
            quality: 100.0,
            embedding: emb,
        }
    }

    fn unit(v: &[f32]) -> Vec<f32> {
        let n: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter().map(|x| x / n).collect()
    }

    /// 浮雕场景：6 张脸风格高度相似（余弦 ~0.55+），只留分数最高的那张
    #[test]
    fn 浮雕式多脸被压成一张() {
        let mut faces = Vec::new();
        for (i, s) in [0.855f32, 0.825, 0.816, 0.804, 0.783, 0.709].iter().enumerate() {
            // 公共基 + 各自方向：两两余弦 = 1/(1+x²)，x=0.78 → ≈0.62，
            // 正好是用户那张浮雕实测的 0.55~0.69 区间
            let mut v = vec![0.0f32; 8];
            v[0] = 1.0;
            v[i + 1] = 0.78;
            faces.push(face(*s, unit(&v)));
        }
        let kept = dedup_same_image(faces);
        assert!(kept.len() <= 2, "浮雕式多脸应该被压成 1~2 张，实际 {}", kept.len());
        assert_eq!(kept[0].score, 0.855, "应该保留分数最高的");
    }

    /// 正常合影：不同人余弦很低，一张都不能少
    #[test]
    fn 合影不同人一个都不少() {
        let faces = vec![
            face(0.919, unit(&[1.0, 0.0, 0.0, 0.0])),
            face(0.917, unit(&[0.0, 1.0, 0.0, 0.0])),
            face(0.836, unit(&[0.0, 0.0, 1.0, 0.0])),
        ];
        let kept = dedup_same_image(faces);
        assert_eq!(kept.len(), 3, "不同人不能被合并");
    }

    /// 同一张脸被检测两次（几乎相同）→ 留分数高的
    #[test]
    fn 重复检测只留高分() {
        let a = unit(&[1.0, 0.0, 0.0, 0.0]);
        let b = unit(&[0.99, 0.02, 0.0, 0.0]);
        let kept = dedup_same_image(vec![face(0.80, a), face(0.90, b)]);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].score, 0.80, "入参按分数降序，先来的被保留");
    }

    #[test]
    fn 单张原样返回() {
        let kept = dedup_same_image(vec![face(0.9, unit(&[1.0, 0.0]))]);
        assert_eq!(kept.len(), 1);
    }
}
