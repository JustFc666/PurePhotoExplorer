# -*- coding: utf-8 -*-
"""把 CN-CLIP 导出成 Rust (ort + DirectML) 能直接吃的两个产物。

    J:/ai/tts/.venv/Scripts/python.exe tools/ai/export_clip_onnx.py

产出（默认 `assets/ai/tag/`）：

| 文件 | 内容 |
|---|---|
| `clip_vision.onnx` | 视觉塔，输入 `pixel_values:[B,3,224,224]`，输出 `image_embeds:[B,512]`（已 L2 归一化） |
| `text_embeds.bin` | 词表 prompt 向量，`N×512` float32，行主序 |
| `tag_meta.json` | prompt 列表 / 每个 prompt 属于第几个词条 / 预处理参数 / 词表 |

## 为什么文本塔不进 ONNX
文本塔**只在建词表向量时跑一次**，之后 Rust 侧只做「图像向量 × 这堆向量」的内积。
把 60MB 的文本塔塞进安装包纯属浪费 —— 词表变了才需要重跑这个脚本。

## 为什么输出 L2 归一化
余弦相似度 = 归一化后的内积。在导出时归一化，Rust 侧只需一次点积，
省掉「先算模长再相除」的浮点开销与除零风险。文本向量同样归一化，两边同空间。
"""

import argparse
import csv
import json
import os
import sys
from pathlib import Path

import numpy as np
import torch
# 必须是 ChineseCLIP 系列：这个 checkpoint 的 model_type 是 chinese_clip，
# 用通用 CLIPModel 加载会**静默丢光文本塔权重**（text_model.* 全部随机初始化），
# 打出来的词向量毫无意义。和 tools/ai/auto_tag.py 保持一致。
from transformers import ChineseCLIPModel, ChineseCLIPProcessor

ROOT = Path(__file__).resolve().parents[2]
# 项目内已有一份（2026-10-20 下的），别再走网络：`assets/ai/chinese-clip` 就是 b16
MODEL_DIR = ROOT / "assets/ai/chinese-clip"
VOCAB = ROOT / "crates/pv-ai/vocab/zh_tags.csv"
OUT_DIR = ROOT / "assets/ai/tag"

# 与 tools/ai/auto_tag.py 的 TEMPLATES 保持一致 —— 词向量必须和打标脚本同一套 prompt，
# 否则「脚本打的标签」和「Rust 打的标签」阈值对不上。
TEMPLATES = ["{}", "一张{}的照片"]


def load_vocab() -> list[dict]:
    rows = list(csv.DictReader(VOCAB.open(encoding="utf-8-sig")))
    out = []
    for r in rows:
        term = (r.get("主题词") or "").strip()
        if not term:
            continue
        syn = [s.strip() for s in (r.get("同义词") or "").replace("，", ",").split(",") if s.strip()]
        out.append(
            {
                "term": term,
                "l1": (r.get("一级类目") or "").strip(),
                "l2": (r.get("二级类目") or "").strip(),
                "syn": syn,
            }
        )
    return out


def build_prompts(vocab: list[dict]) -> tuple[list[str], list[int]]:
    """返回 (prompts, owner)。owner[i] = 该 prompt 属于第几个词条（与 auto_tag.py 同构）。"""
    prompts, owner = [], []
    for i, v in enumerate(vocab):
        for t in TEMPLATES:
            prompts.append(t.format(v["term"]))
            owner.append(i)
        for s in v["syn"][:3]:
            prompts.append(TEMPLATES[1].format(s))
            owner.append(i)
    return prompts, owner


@torch.no_grad()
def text_matrix(model, proc, prompts: list[str], device: str) -> torch.Tensor:
    feats = []
    for i in range(0, len(prompts), 64):
        batch = prompts[i : i + 64]
        inp = proc(text=batch, return_tensors="pt", padding=True, truncation=True).to(device)
        f = model.get_text_features(**inp)
        feats.append(f / f.norm(dim=-1, keepdim=True))
    return torch.cat(feats)


class VisionOnly(torch.nn.Module):
    """只导出视觉塔 + 投影层，输出 L2 归一化的 512 维。"""

    def __init__(self, clip: ChineseCLIPModel):
        super().__init__()
        self.vision_model = clip.vision_model
        self.visual_projection = clip.visual_projection

    def forward(self, pixel_values):
        out = self.vision_model(pixel_values=pixel_values)
        emb = self.visual_projection(out.pooler_output)
        return emb / emb.norm(dim=-1, keepdim=True)


def export_vision(model, out_path: Path, dtype: torch.dtype) -> None:
    v = VisionOnly(model).eval().to(dtype)
    dummy = torch.zeros(1, 3, 224, 224, dtype=dtype)
    torch.onnx.export(
        v,
        dummy,
        str(out_path),
        input_names=["pixel_values"],
        output_names=["image_embeds"],
        # batch 必须动态：打标是成批跑的（一次 8 张比逐张快一大截），
        # 写死 1 的话 ORT 会直接拒绝 batch>1 的输入
        dynamic_axes={"pixel_values": {0: "batch"}, "image_embeds": {0: "batch"}},
        opset_version=17,
        do_constant_folding=True,
        dynamo=False,
    )


def verify_vision(model, out_path: Path, dtype: torch.dtype) -> float:
    """拿 ONNX 跑一遍，和 torch 比余弦。返回最小余弦（越接近 1 越好）。"""
    import onnxruntime as ort

    torch.manual_seed(0)
    x = torch.randn(2, 3, 224, 224)

    with torch.no_grad():
        ref = VisionOnly(model).eval()(x.to(model.dtype)).float().numpy()

    sess = ort.InferenceSession(str(out_path), providers=["CPUExecutionProvider"])
    got = sess.run(None, {"pixel_values": x.to(dtype).numpy()})[0].astype(np.float32)

    cos = (ref * got).sum(1) / (np.linalg.norm(ref, axis=1) * np.linalg.norm(got, axis=1) + 1e-9)
    return float(cos.min())


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", default=str(MODEL_DIR))
    ap.add_argument("--out", default=str(OUT_DIR))
    ap.add_argument("--fp32", action="store_true", help="导出 fp32（默认 fp16，DirectML 更快）")
    args = ap.parse_args()

    out_dir = Path(args.out)
    out_dir.mkdir(parents=True, exist_ok=True)
    # ⛔ 固定 CPU：导出不需要 GPU，而本机 torch 是 cu118 版、在 A 卡上
    # `cuda.is_available()` 会误报 True，真跑起来直接抛
    # "Input type (torch.FloatTensor) and weight type (torch.cuda.FloatTensor)"
    device = "cpu"
    dtype = torch.float32 if args.fp32 else torch.float16

    print(f"加载模型：{args.model}（设备 {device}）")
    model = ChineseCLIPModel.from_pretrained(args.model).to(device).eval()
    proc = ChineseCLIPProcessor.from_pretrained(args.model)

    # ── 1. 词表向量 ──
    vocab = load_vocab()
    prompts, owner = build_prompts(vocab)
    print(f"词表 {len(vocab)} 条 → {len(prompts)} 条 prompt")

    emb = text_matrix(model, proc, prompts, device).float().cpu().numpy()
    assert emb.shape == (len(prompts), 512), f"维度不对：{emb.shape}"
    # 文本向量用 fp32 存：只有几百 KB，没必要省这点
    emb.astype(np.float32).tofile(out_dir / "text_embeds.bin")
    print(f"文本向量：{emb.shape} → text_embeds.bin（{emb.nbytes / 1e6:.1f} MB）")

    # 预处理参数原样记下来，Rust 侧必须用同一套（差一点效果就偏）
    pp = proc.image_processor
    meta = {
        "model": "OFA-Sys/chinese-clip-vit-base-patch16",
        "dim": 512,
        "prompts": prompts,
        "owner": owner,
        "vocab": vocab,
        "preprocess": {
            "size": pp.size["shortest_edge"] if "shortest_edge" in pp.size else pp.size["height"],
            "crop": pp.crop_size["height"],
            "mean": [round(float(v), 6) for v in pp.image_mean],
            "std": [round(float(v), 6) for v in pp.image_std],
        },
        "embed_dtype": "float16" if dtype == torch.float16 else "float32",
        "text_embeds_dtype": "float32",
    }
    (out_dir / "tag_meta.json").write_text(
        json.dumps(meta, ensure_ascii=False, indent=1), encoding="utf-8"
    )
    print(f"元数据：tag_meta.json（预处理 {meta['preprocess']}）")

    # ── 2. 视觉塔 ONNX ──
    onnx_path = out_dir / "clip_vision.onnx"
    print(f"\n导出视觉塔（{dtype}）…")
    try:
        export_vision(model, onnx_path, dtype)
    except Exception as e:
        if dtype == torch.float16:
            print(f"fp16 导出失败（{e}），退回 fp32")
            dtype = torch.float32
            meta["embed_dtype"] = "float32"
            (out_dir / "tag_meta.json").write_text(
                json.dumps(meta, ensure_ascii=False, indent=1), encoding="utf-8"
            )
            export_vision(model, onnx_path, dtype)
        else:
            raise

    mb = onnx_path.stat().st_size / 1e6
    print(f"clip_vision.onnx：{mb:.1f} MB")

    # ── 3. 数值校验 ──
    cos = verify_vision(model, onnx_path, dtype)
    print(f"onnx vs torch 余弦（最小）：{cos:.6f}")
    if cos < 0.999:
        print("⚠️ 余弦偏低，检查 fp16 精度或导出算子", file=sys.stderr)
        return 2

    print("\n✅ 导出完成：", out_dir)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
