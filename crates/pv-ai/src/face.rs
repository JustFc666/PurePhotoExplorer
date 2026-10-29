//! 人脸检测与特征提取（YuNet + SFace，ONNX Runtime + DirectML）。
//!
//! ## 和 Python 版的关系
//! `tools/ai/face_probe.py` 用 `cv2.FaceDetectorYN` + `cv2.FaceRecognizerSF`，
//! 后处理都封在 OpenCV 里。这里是 Rust 复刻：
//! - 检测：YuNet 是 anchor-free，每个网格直接回归「中心偏移 + 宽高 + 5 个关键点」
//! - 对齐：用 5 点做**相似变换**（4 自由度：缩放/旋转/平移）到固定的目标位置
//! - 特征：SFace 输出 128 维，比较用余弦（OpenCV 官方阈值 0.363，本机实测 0.40）
//!
//! ## 输入尺寸是写死的
//! 这个 yunet.onnx 的输入是 `[1,3,640,640]`（不是动态）。所以整图**拉伸**到 640×640，
//! 检测完再按比例把坐标映射回原图 —— 拉伸会让脸轻微变形，但对检测影响很小，
//! 而「保持比例 + padding」会多一层坐标换算，容易出错。
//!
//! ⚠️ 拉伸 vs padding 会影响**关键点**的精度，进而影响对齐后的特征。
//! 如果实测识别率不如 Python 版，第一件事就是换成「等比例缩放 + 填充」。

use ort::ep::DirectML;
use ort::session::Session;
use ort::value::Tensor;
use pv_decode::RgbaImage;

/// 检测网络输入边长（模型写死）。
const DET_SIDE: u32 = 640;
/// 三个输出分支的步长。
const STRIDES: [u32; 3] = [8, 16, 32];
/// NMS 的 IoU 阈值（与 Python 版 `FaceDetectorYN.create` 的第 5 个参数一致）。
const NMS_IOU: f32 = 0.3;

/// 检测到的一张脸。
#[derive(Debug, Clone)]
pub struct Face {
    /// 左上角 + 宽高（**原图坐标系**）
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub score: f32,
    /// 5 个关键点：左眼、右眼、鼻尖、左嘴角、右嘴角（原图坐标系）
    pub kps: [[f32; 2]; 5],
}

pub struct FaceDetector {
    session: Session,
}

impl FaceDetector {
    pub fn load(dir: &std::path::Path) -> Result<Self, String> {
        let path = dir.join("yunet.onnx");
        // ⛔ 不能自己 ort::init()：进程里只允许一个环境（三个模型共用）
        let env = crate::ort_env::ort_env()?;
        let session = Session::builder(&env)
            .and_then(|mut b| b.commit_from_file(&path))
            .map_err(|e| format!("加载 YuNet 失败（{path:?}）：{e}"))?;
        Ok(Self { session })
    }

    /// 检测一张图里的所有脸。`score_thresh` 建议 0.6（Python 版默认）。
    ///
    /// 返回的坐标已经映射回**传入图像**的坐标系。
    pub fn detect(&mut self, img: &RgbaImage, score_thresh: f32) -> Result<Vec<Face>, String> {
        let (ow, oh) = (img.width.max(1) as f32, img.height.max(1) as f32);

        // 1) 拉伸到 640×640，RGBA → RGB，0-255 直接喂（YuNet 不做 mean/std）
        let chw = to_chw_rgb(img, DET_SIDE, DET_SIDE);
        let t = Tensor::from_array(([1i64, 3, DET_SIDE as i64, DET_SIDE as i64], chw))
            .map_err(|e| format!("构造输入失败：{e}"))?;

        // 2) 前向
        let mut raw: Vec<(String, Vec<f32>)> = Vec::new();
        {
            let outputs = self
                .session
                .run(ort::inputs!["input" => t])
                .map_err(|e| format!("YuNet 推理失败：{e}"))?;
            for name in ["cls_8", "obj_8", "bbox_8", "kps_8", "cls_16", "obj_16", "bbox_16", "kps_16", "cls_32", "obj_32", "bbox_32", "kps_32"] {
                let v = outputs
                    .get(name)
                    .ok_or_else(|| format!("缺少输出 {name}"))?;
                let (_, d) = v
                    .try_extract_tensor::<f32>()
                    .map_err(|e| format!("取 {name} 失败：{e}"))?;
                raw.push((name.to_string(), d.to_vec()));
            }
        }
        let pick = |key: &str| -> &Vec<f32> {
            &raw.iter().find(|(n, _)| n == key).expect("上面按名字取过").1
        };

        // 3) 解码：anchor-free，每个网格回归「中心 + log(宽高) + 5 点偏移」
        let mut cands: Vec<Face> = Vec::new();
        for s in STRIDES {
            let grid = (DET_SIDE / s) as usize;
            let cls = pick(&format!("cls_{s}"));
            let obj = pick(&format!("obj_{s}"));
            let bbox = pick(&format!("bbox_{s}"));
            let kps = pick(&format!("kps_{s}"));

            for i in 0..grid {
                for j in 0..grid {
                    let idx = i * grid + j;
                    // 官方是 sqrt(cls * obj)；两路都过 sigmoid 了，直接乘开方
                    let score = (cls[idx] * obj[idx]).max(0.0).sqrt();
                    if score < score_thresh {
                        continue;
                    }
                    let b = &bbox[idx * 4..idx * 4 + 4];
                    let cx = (j as f32 + b[0]) * s as f32;
                    let cy = (i as f32 + b[1]) * s as f32;
                    // 宽高是 log 尺度；clamp 一下防止异常权重把 exp 撑爆
                    let w = b[2].clamp(-8.0, 8.0).exp() * s as f32;
                    let h = b[3].clamp(-8.0, 8.0).exp() * s as f32;
                    if !(w.is_finite() && h.is_finite()) || w <= 1.0 || h <= 1.0 {
                        continue;
                    }
                    let mut k = [[0f32; 2]; 5];
                    for (n, pt) in k.iter_mut().enumerate() {
                        pt[0] = (kps[idx * 10 + n * 2] + j as f32) * s as f32;
                        pt[1] = (kps[idx * 10 + n * 2 + 1] + i as f32) * s as f32;
                    }
                    cands.push(Face {
                        x: cx - w / 2.0,
                        y: cy - h / 2.0,
                        w,
                        h,
                        score,
                        kps: k,
                    });
                }
            }
        }

        // 4) NMS（分数降序，IoU 超阈值就丢）
        cands.sort_by(|a, b| b.score.total_cmp(&a.score));
        let mut keep: Vec<Face> = Vec::new();
        for c in cands {
            if keep.iter().all(|k| iou(k, &c) < NMS_IOU) {
                keep.push(c);
            }
        }

        // 5) 坐标映射回原图（拉伸的逆变换）
        let sx = ow / DET_SIDE as f32;
        let sy = oh / DET_SIDE as f32;
        for f in &mut keep {
            f.x *= sx;
            f.y *= sy;
            f.w *= sx;
            f.h *= sy;
            for pt in &mut f.kps {
                pt[0] *= sx;
                pt[1] *= sy;
            }
        }
        Ok(keep)
    }
}

fn iou(a: &Face, b: &Face) -> f32 {
    let x1 = a.x.max(b.x);
    let y1 = a.y.max(b.y);
    let x2 = (a.x + a.w).min(b.x + b.w);
    let y2 = (a.y + a.h).min(b.y + b.h);
    let iw = (x2 - x1).max(0.0);
    let ih = (y2 - y1).max(0.0);
    let inter = iw * ih;
    let union = a.w * a.h + b.w * b.h - inter;
    if union <= 0.0 {
        0.0
    } else {
        inter / union
    }
}

/// RGBA → RGB，**双线性**拉伸到 `tw × th`，输出 CHW 的 f32（0–255）。
///
/// ⚠️ 必须双线性（2026-10-29 实测）：用最近邻时框位置还行（±10px），但
/// 分数比 OpenCV 低 0.03~0.2、框尺寸差 10~18px —— OpenCV 的 `resize` 是双线性，
/// 输入像素差一点，检测头的回归结果就偏一截。
///
/// 坐标映射按 OpenCV 的约定：`src = (dst + 0.5) * scale - 0.5`（像素中心对齐），
/// 不是直接 `dst * scale` —— 后者会有半个像素的系统性偏移。
fn to_chw_rgb(img: &RgbaImage, tw: u32, th: u32) -> Vec<f32> {
    let plane = (tw * th) as usize;
    let mut out = vec![0f32; 3 * plane];
    let (sw, sh) = (img.width.max(1) as f32, img.height.max(1) as f32);
    let (rw, rh) = (sw / tw as f32, sh / th as f32);

    for y in 0..th {
        let fy = (y as f32 + 0.5) * rh - 0.5;
        let y0 = fy.floor().max(0.0) as usize;
        let y1 = (y0 + 1).min(sh as usize - 1);
        let wy = (fy - y0 as f32).clamp(0.0, 1.0);

        for x in 0..tw {
            let fx = (x as f32 + 0.5) * rw - 0.5;
            let x0 = fx.floor().max(0.0) as usize;
            let x1 = (x0 + 1).min(sw as usize - 1);
            let wx = (fx - x0 as f32).clamp(0.0, 1.0);

            let i00 = (y0 * sw as usize + x0) * 4;
            let i01 = (y0 * sw as usize + x1) * 4;
            let i10 = (y1 * sw as usize + x0) * 4;
            let i11 = (y1 * sw as usize + x1) * 4;
            let di = y as usize * tw as usize + x as usize;

            for c in 0..3 {
                let p00 = img.data[i00 + c] as f32;
                let p01 = img.data[i01 + c] as f32;
                let p10 = img.data[i10 + c] as f32;
                let p11 = img.data[i11 + c] as f32;
                let top = p00 + (p01 - p00) * wx;
                let bot = p10 + (p11 - p10) * wx;
                out[c * plane + di] = top + (bot - top) * wy;
            }
        }
    }
    out
}

// ══════════════════════════════════════════════════════════════════════
//  SFace：5 点对齐 → 128 维特征
// ══════════════════════════════════════════════════════════════════════

/// SFace 的标准人脸关键点（112×112 画布上的理想位置）。
///
/// ⭐ 这 5 个坐标是 OpenCV `FaceRecognizerSF::alignCrop` 里写死的**目标位置**，
/// 顺序：左眼、右眼、鼻尖、左嘴角、右嘴角（与 YuNet 输出的 5 点顺序一致）。
/// ⛔ 不能自己改：SFace 训练时就是用这套对齐，换一套特征会整体失准。
const TARGET_KPS: [[f32; 2]; 5] = [
    [38.2946, 51.6963],
    [73.5318, 51.5014],
    [56.0252, 71.7366],
    [41.5493, 92.3655],
    [70.7299, 92.2041],
];

/// 对齐后的边长（SFace 固定输入）。
const REC_SIDE: usize = 112;

pub struct FaceRecognizer {
    session: Session,
}

impl FaceRecognizer {
    pub fn load(dir: &std::path::Path) -> Result<Self, String> {
        let path = dir.join("sface.onnx");
        let env = crate::ort_env::ort_env()?;
        let session = Session::builder(env)
            .and_then(|mut b| b.commit_from_file(&path))
            .map_err(|e| format!("加载 SFace 失败（{path:?}）：{e}"))?;
        Ok(Self { session })
    }

    /// 一张脸 → 128 维 **L2 归一化**向量。
    ///
    /// 模型输出本身没归一化（实测模长 13.7），必须自己归一化：
    /// 否则余弦会变成「模长×夹角」，同一个人也会判成不同人。
    pub fn embed(&mut self, img: &RgbaImage, face: &Face) -> Result<Vec<f32>, String> {
        let m = estimate_similar(&face.kps, &TARGET_KPS);
        let chw = warp_aligned(img, &m);

        let t = Tensor::from_array(([1i64, 3, REC_SIDE as i64, REC_SIDE as i64], chw))
            .map_err(|e| format!("构造 SFace 输入失败：{e}"))?;

        let mut out = {
            let outputs = self
                .session
                .run(ort::inputs!["data" => t])
                .map_err(|e| format!("SFace 推理失败：{e}"))?;
            let (_, d) = outputs[0]
                .try_extract_tensor::<f32>()
                .map_err(|e| format!("取 SFace 输出失败：{e}"))?;
            d.to_vec()
        };

        let norm = out.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 1e-6 {
            for v in &mut out {
                *v /= norm;
            }
        }
        Ok(out)
    }
}

/// 求 4 参数相似变换 `dst ≈ s·R·src + t`，返回 `[a, -b, tx, b, a, ty]`。
///
/// 用最小二乘闭式解（Procrustes）。这里**不直接仿射拟合 6 个参数** ——
/// 相似变换只有 4 个自由度，6 参数最小二乘会把「人脸被压扁」也算成合理形变。
fn estimate_similar(src: &[[f32; 2]; 5], dst: &[[f32; 2]; 5]) -> [f32; 6] {
    let n = 5.0f32;
    let (mut sx, mut sy) = (0.0f32, 0.0f32);
    let (mut dx, mut dy) = (0.0f32, 0.0f32);
    for i in 0..5 {
        sx += src[i][0];
        sy += src[i][1];
        dx += dst[i][0];
        dy += dst[i][1];
    }
    let (sx, sy, dx, dy) = (sx / n, sy / n, dx / n, dy / n);

    let mut num_a = 0.0f32; // Σ(x·u + y·v)
    let mut num_b = 0.0f32; // Σ(x·v − y·u)
    let mut den = 0.0f32; // Σ(x² + y²)
    for i in 0..5 {
        let x = src[i][0] - sx;
        let y = src[i][1] - sy;
        let u = dst[i][0] - dx;
        let v = dst[i][1] - dy;
        num_a += x * u + y * v;
        num_b += x * v - y * u;
        den += x * x + y * y;
    }
    if den.abs() < 1e-9 {
        return [1.0, 0.0, dx - sx, 0.0, 1.0, dy - sy];
    }
    let a = num_a / den;
    let b = num_b / den;
    let tx = dx - (a * sx - b * sy);
    let ty = dy - (b * sx + a * sy);
    [a, -b, tx, b, a, ty]
}

/// 按 `m` 把原图重采样成 112×112 RGB（CHW，**0–255**）。
///
/// 逐像素求**逆变换**再双线性采样 —— 正向映射会留下空洞。
///
/// ⛔ 两个关键点（都踩过）：
/// 1. **不做 1/255**。OpenCV `FaceRecognizerSF::feature` 的 blobFromImage
///    用的是 `scale=1.0`，即把 0–255 的 float 直接喂进去。我按常识除了 255，
///    结果特征模长只有 2.5（正确 13.7），三张不同脸余弦高达 0.97 —— 全废。
/// 2. 逆矩阵是 `1/(a²+b²)·[[a,b],[-b,a]]`。我一度把 det 写成 `a²-b²`、v 的符号也抄反，
///    采样位置全乱（人脸对齐图看着还行，但特征完全不对）。
fn warp_aligned(img: &RgbaImage, m: &[f32; 6]) -> Vec<f32> {
    // m = [a, -b, tx, b, a, ty] ⇒ 2×3 矩阵 [[a, -b, tx], [b, a, ty]]
    let [a, _negb, tx, b, _a2, ty] = *m;
    let det = a * a + b * b;
    let inv_det = if det.abs() < 1e-9 { 0.0 } else { 1.0 / det };

    let plane = REC_SIDE * REC_SIDE;
    let mut out = vec![0f32; 3 * plane];
    let (w, h) = (img.width as f32, img.height as f32);
    let ww = img.width as usize;

    for y in 0..REC_SIDE {
        for x in 0..REC_SIDE {
            let (px, py) = (x as f32, y as f32);
            // M⁻¹ · [px, py, 1]
            let u = inv_det * (a * (px - tx) + b * (py - ty));
            let v = inv_det * (-b * (px - tx) + a * (py - ty));

            let di = y * REC_SIDE + x;
            if u < -1.0 || v < -1.0 || u > w || v > h {
                continue; // 画布外填 0（黑边）
            }
            let x0 = u.floor().max(0.0);
            let y0 = v.floor().max(0.0);
            let x1 = (x0 + 1.0).min(w - 1.0);
            let y1 = (y0 + 1.0).min(h - 1.0);
            let wx = (u - x0).clamp(0.0, 1.0);
            let wy = (v - y0).clamp(0.0, 1.0);
            let (ix0, iy0) = (x0 as usize, y0 as usize);
            let (ix1, iy1) = (x1 as usize, y1 as usize);

            for c in 0..3 {
                let p00 = img.data[(iy0 * ww + ix0) * 4 + c] as f32;
                let p01 = img.data[(iy0 * ww + ix1) * 4 + c] as f32;
                let p10 = img.data[(iy1 * ww + ix0) * 4 + c] as f32;
                let p11 = img.data[(iy1 * ww + ix1) * 4 + c] as f32;
                let top = p00 + (p01 - p00) * wx;
                let bot = p10 + (p11 - p10) * wx;
                out[c * plane + di] = top + (bot - top) * wy;
            }
        }
    }
    out
}

/// 余弦相似度（两个向量都已 L2 归一化，所以就是点积）。
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}
