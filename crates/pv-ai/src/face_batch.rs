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
pub fn detect_batch<F>(
    images: &[(i64, PathBuf)],
    det: &mut FaceDetector,
    rec: &mut FaceRecognizer,
    opts: &FaceBatchOpts,
    mut on_progress: F,
) -> (Vec<DetectedFace>, FaceBatchStats)
where
    F: FnMut(&FaceProgress),
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

        match detect_one(&reg, det, rec, path, *id, opts) {
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
                });
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
                });
            }
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
    Ok(out)
}
