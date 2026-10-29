//! CN-CLIP 零样本打标（Rust + ONNX Runtime + DirectML）。
//!
//! ## 和 Python 版的关系
//! `tools/ai/auto_tag.py` 是**验证用**的脚本（torch）。这里是**发行版**的正式实现：
//! 同一套词表、同一套 prompt、同一个模型，但推理走 `ort` + DirectML，
//! 不依赖 Python / torch / CUDA。
//!
//! 两边必须**结果可比**，所以下面三件事严格对齐（改了会静默偏分）：
//! 1. prompt 构造（`["{}", "一张{}的照片"]` + 同义词各一条）—— 由导出脚本算好，这里只读
//! 2. 预处理（短边 224 → 中心裁剪 224 → normalize mean/std → RGB → CHW）
//! 3. 打分（余弦 = 归一化向量内积；**同一词条的多条 prompt 取 max**，不是 mean）
//!
//! ## 为什么词条取 max 而不是 mean
//! 同义词是「另一种叫法」，不是「更强的相关」。取 mean 会让「樱花」的具体分
//! 被「花」这种宽义词稀释；取 max 才是「任一种叫法像就算像」。

use std::path::Path;

use fast_image_resize::images::Image as FirImage;
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
use half::f16;
use ort::session::Session;
use ort::value::Tensor;
use pv_decode::RgbaImage;
use serde::Deserialize;

/// CLIP 的固定输入边长。
const SIDE: usize = 224;

/// 预处理参数（必须与 `tools/ai/export_clip_onnx.py` 写进 meta 的一致）。
#[derive(Debug, Clone, Deserialize)]
struct Preprocess {
    /// 短边缩到多少（CLIP 的 `shortest_edge`）
    size: usize,
    /// 中心裁剪边长
    crop: usize,
    mean: [f32; 3],
    std: [f32; 3],
}

#[derive(Debug, Clone, Deserialize)]
pub struct VocabEntry {
    pub term: String,
    pub l1: String,
    pub l2: String,
    #[serde(default)]
    pub syn: Vec<String>,
}

/// `tag_meta.json` 的镜像 —— 这是导出脚本与本实现之间的**契约**。
#[derive(Debug, Deserialize)]
struct TagMeta {
    dim: usize,
    prompts: Vec<String>,
    /// `owner[i]` = 第 i 条 prompt 属于第几个词条
    owner: Vec<usize>,
    vocab: Vec<VocabEntry>,
    preprocess: Preprocess,
    #[serde(default)]
    embed_dtype: String,
}

/// 一个词条的得分。
#[derive(Debug, Clone)]
pub struct Scored {
    pub term: String,
    pub l1: String,
    pub l2: String,
    /// 该词条所有 prompt 里的最高余弦
    pub score: f32,
}

pub struct Tagger {
    session: Session,
    meta: TagMeta,
    /// `prompts.len() * dim` 行主序，已 L2 归一化
    text: Vec<f32>,
    fp16: bool,
}

/// 词表里的**全部一级类目名**（去重 + 排序），只读 `tag_meta.json`，不加载模型。
///
/// 用途：清掉老库里「一级类目」那种派生标签 —— 见
/// `pv_index::repo::tags::drop_redundant_l1`。
pub fn vocab_l1_names(dir: &Path) -> Result<Vec<String>, String> {
    let path = dir.join("tag_meta.json");
    let meta: TagMeta = serde_json::from_str(
        &std::fs::read_to_string(&path).map_err(|e| format!("读 {path:?} 失败：{e}"))?,
    )
    .map_err(|e| format!("解析 {path:?} 失败：{e}"))?;
    let mut out: Vec<String> = meta.vocab.into_iter().map(|e| e.l1).collect();
    out.sort();
    out.dedup();
    Ok(out)
}

impl Tagger {
    /// 从导出的产物目录加载（含 `clip_vision.onnx` / `text_embeds.bin` / `tag_meta.json`）。
    pub fn load(dir: &Path) -> Result<Self, String> {
        let meta_path = dir.join("tag_meta.json");
        let meta: TagMeta = serde_json::from_str(
            &std::fs::read_to_string(&meta_path)
                .map_err(|e| format!("读 tag_meta.json 失败：{e}"))?,
        )
        .map_err(|e| format!("解析 tag_meta.json 失败：{e}"))?;

        let raw = std::fs::read(dir.join("text_embeds.bin"))
            .map_err(|e| format!("读 text_embeds.bin 失败：{e}"))?;
        let text: Vec<f32> = raw
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        let want = meta.prompts.len() * meta.dim;
        if text.len() != want {
            return Err(format!(
                "词向量长度不对：{} ≠ prompts×dim（{want}）",
                text.len()
            ));
        }

        let onnx = dir.join("clip_vision.onnx");
        // ⛔ 不能自己 ort::init()：进程里只允许一个环境，三个模型各自 init 会炸
        let env = crate::ort_env::ort_env()?;
        let session = Session::builder(&env)
            .and_then(|mut b| b.commit_from_file(&onnx))
            .map_err(|e| format!("加载视觉塔失败（{onnx:?}）：{e}"))?;

        let fp16 = meta.embed_dtype != "float32";
        Ok(Self {
            session,
            meta,
            text,
            fp16,
        })
    }

    /// 词表（顺序与 `score()` 返回的一致）。
    pub fn vocab(&self) -> &[VocabEntry] {
        &self.meta.vocab
    }

    pub fn prompts_len(&self) -> usize {
        self.meta.prompts.len()
    }

    /// 单张图 → 512 维归一化向量。
    pub fn embed(&mut self, img: &RgbaImage) -> Result<Vec<f32>, String> {
        let chw = self.preprocess(img);
        let shape = [1i64, 3, SIDE as i64, SIDE as i64];

        // ⚠️ 两个 dtype 分支必须分开写：`Tensor<f16>` 和 `Tensor<f32>` 是不同类型，
        // 而 `session.run` 的返回值又持有可变借用 —— 抽成泛型函数反而更容易写错。
        let mut out: Vec<f32> = if self.fp16 {
            let data: Vec<f16> = chw.iter().map(|v| f16::from_f32(*v)).collect();
            let t = Tensor::from_array((shape, data))
                .map_err(|e| format!("构造输入张量失败：{e}"))?;
            let outputs = self
                .session
                .run(ort::inputs!["pixel_values" => t])
                .map_err(|e| format!("推理失败：{e}"))?;
            let (sh, d) = outputs[0]
                .try_extract_tensor::<f16>()
                .map_err(|e| format!("取输出失败：{e}"))?;
            let n: usize = sh.iter().map(|x| *x as usize).product();
            d[..n].iter().map(|v| v.to_f32()).collect()
        } else {
            let t = Tensor::from_array((shape, chw))
                .map_err(|e| format!("构造输入张量失败：{e}"))?;
            let outputs = self
                .session
                .run(ort::inputs!["pixel_values" => t])
                .map_err(|e| format!("推理失败：{e}"))?;
            let (sh, d) = outputs[0]
                .try_extract_tensor::<f32>()
                .map_err(|e| format!("取输出失败：{e}"))?;
            let n: usize = sh.iter().map(|x| *x as usize).product();
            d[..n].to_vec()
        };

        // 模型输出已经归一化，这里只做长度防御（避免以后换模型忘了归一化）
        let norm = out.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            for v in &mut out {
                *v /= norm;
            }
        }
        Ok(out)
    }

    /// 向量 × 词表 → 每个词条的最高分（按分数降序）。
    pub fn score(&self, emb: &[f32]) -> Vec<Scored> {
        let dim = self.meta.dim;
        let n = self.meta.prompts.len();
        let mut best = vec![f32::NEG_INFINITY; self.meta.vocab.len()];

        for p in 0..n {
            let row = &self.text[p * dim..(p + 1) * dim];
            let dot: f32 = row.iter().zip(emb).map(|(a, b)| a * b).sum();
            let o = self.meta.owner[p];
            if dot > best[o] {
                best[o] = dot;
            }
        }

        let mut v: Vec<Scored> = self
            .meta
            .vocab
            .iter()
            .zip(best)
            .map(|(e, s)| Scored {
                term: e.term.clone(),
                l1: e.l1.clone(),
                l2: e.l2.clone(),
                score: s,
            })
            .collect();
        v.sort_by(|a, b| b.score.total_cmp(&a.score));
        v
    }

    /// 预处理：短边缩到 224 → 中心裁剪 → normalize → RGB → CHW。
    ///
    /// ⚠️ 每一步都要和 `CLIPImageProcessor` 对上：
    /// - 先缩**短边**（不是长边），保持比例
    /// - 中心裁剪（不是左上角）
    /// - normalize 用 meta 里的 mean/std（ImageNet 那套，不是 0.5）
    fn preprocess(&self, img: &RgbaImage) -> Vec<f32> {
        let pp = &self.meta.preprocess;
        let (w, h) = (img.width.max(1), img.height.max(1));

        // 1) 短边 → pp.size，其他边等比
        let scale = pp.size as f32 / w.min(h) as f32;
        let nw = ((w as f32 * scale).round() as u32).max(1);
        let nh = ((h as f32 * scale).round() as u32).max(1);

        let src = FirImage::from_vec_u8(w, h, img.data.clone(), PixelType::U8x4)
            .expect("源图构造失败（尺寸/缓冲不匹配）");
        let mut dst = FirImage::new(nw, nh, PixelType::U8x4);
        // PIL 的 BICUBIC 是 Catmull-Rom（a=-0.5），fast_image_resize 里对应 CatmullRom。
        // ⛔ 别用 Lanczos3：锐过头，和 Python 版分数会有系统性偏差
        Resizer::new()
            .resize(
                &src,
                &mut dst,
                &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::CatmullRom)),
            )
            .expect("缩放失败");
        let rgb = dst.into_vec();

        // 2) 中心裁剪 pp.crop × pp.crop（缩完一定 ≥ crop）
        let cw = pp.crop as usize;
        let ox = (nw as usize - cw) / 2;
        let oy = (nh as usize - cw) / 2;

        // 3) normalize + HWC(RGBA) → CHW(f32)
        let side = SIDE;
        let mut chw = vec![0f32; 3 * side * side];
        for y in 0..side {
            for x in 0..side {
                let si = ((oy + y) * nw as usize + (ox + x)) * 4;
                let o = y * side + x;
                for c in 0..3 {
                    let px = rgb[si + c] as f32 / 255.0;
                    chw[c * side * side + o] = (px - pp.mean[c]) / pp.std[c];
                }
            }
        }
        chw
    }
}
