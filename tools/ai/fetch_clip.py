# -*- coding: utf-8 -*-
"""下载 CN-CLIP（chinese-clip-vit-base-patch16）到本地目录。

只取导出 ONNX 必需的 4 个文件，跳过 753MB 的原版 .pt 权重和示例图。

    J:/ai/tts/.venv/Scripts/python.exe tools/ai/fetch_clip.py
"""

import os
import sys

# hf-mirror：huggingface.co 在国内直连基本不通
os.environ.setdefault("HF_ENDPOINT", "https://hf-mirror.com")
os.environ.setdefault("HF_HUB_DISABLE_TELEMETRY", "1")

from huggingface_hub import snapshot_download  # noqa: E402

REPO = "OFA-Sys/chinese-clip-vit-base-patch16"
DEST = r"J:\ai\models\chinese-clip-vit-base-patch16"

# 只要这些：配置 + 分词器词表 + 权重
ALLOW = ["config.json", "preprocessor_config.json", "vocab.txt", "pytorch_model.bin"]


def main() -> int:
    print(f"repo  : {REPO}")
    print(f"dest  : {DEST}")
    print(f"镜像  : {os.environ['HF_ENDPOINT']}")
    try:
        path = snapshot_download(
            repo_id=REPO,
            local_dir=DEST,
            allow_patterns=ALLOW,
            max_workers=4,
        )
    except Exception as e:  # 网络类错误的成因很杂，原样抛出来更有用
        print(f"下载失败：{e}", file=sys.stderr)
        return 1

    print(f"\n完成：{path}")
    for name in ALLOW:
        p = os.path.join(path, name)
        size = os.path.getsize(p) / 1e6 if os.path.exists(p) else -1
        print(f"  {size:8.1f} MB  {name}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
