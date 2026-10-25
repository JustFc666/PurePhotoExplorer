#!/usr/bin/env python
# -*- coding: utf-8 -*-
"""给一个目录里所有图片打一遍分，导出 JSON（不写库、不依赖索引）。

用途：调阈值 / 做标注对比。输出每一项：
    {name, path, l1: [{name, score}], l2: [{term, l1, score}]}

打分走 `auto_tag.Scorer`（与批量打标同一条链路，分数是**原始相似度**）。

用法：python tools/ai/dump_scores.py [目录] [--limit N]
"""

import argparse
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools" / "ai"))
import auto_tag as A  # noqa: E402

EXTS = {".jpg", ".jpeg", ".png", ".webp", ".bmp", ".tif", ".tiff", ".avif", ".gif", ".psd", ".heic"}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dir", nargs="?", default=str(ROOT / "test-image"))
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--out", default=str(ROOT / "assets" / "ai" / "scores_all.json"))
    args = ap.parse_args()

    d = Path(args.dir)
    files = sorted(p for p in d.iterdir() if p.is_file() and p.suffix.lower() in EXTS)
    if args.limit:
        files = files[: args.limit]
    print(f"扫描 {len(files)} 张")

    sc = A.Scorer()
    sims, l1_sims, keep = sc.score_paths(files)
    files = [files[i] for i in keep]
    print(f"成功 {len(files)} 张")

    rows = []
    for i, f in enumerate(files):
        row = sims[i]
        l2 = [
            {"term": v["term"], "l1": v["l1"], "score": round(float(row[sc.owner_arr == j].max()), 4)}
            for j, v in enumerate(sc.vocab)
        ]
        l2.sort(key=lambda x: -x["score"])
        l1 = [
            {"name": n, "score": round(float(l1_sims[i][k]), 4)} for k, n in enumerate(sc.l1s)
        ]
        l1.sort(key=lambda x: -x["score"])
        rows.append({"name": f.name, "path": str(f), "l1": l1, "l2": l2})

    out = Path(args.out)
    out.write_text(json.dumps(rows, ensure_ascii=False), encoding="utf-8")
    print(f"写出 {out}（{len(rows)} 张）")


if __name__ == "__main__":
    main()
