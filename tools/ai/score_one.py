#!/usr/bin/env python
# -*- coding: utf-8 -*-
"""查单张图的完整打分（一级 15 个 + 二级全部），用于核对阈值与误判。

⭐ 打分走 `auto_tag.Scorer`（与批量打标**同一个类**），保证「工具算的」和「库里存的」一致。
   早期版本自己拼了一套链路，同一张图能差 0.02（0.3891 vs 0.4088），别再用那套。

用法：python tools/ai/score_one.py <文件名或路径> [--model dir] [--top 20]
"""

import argparse
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools" / "ai"))
import auto_tag as A  # noqa: E402


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("target")
    ap.add_argument("--model", default=None)
    ap.add_argument("--top", type=int, default=20)
    args = ap.parse_args()

    p = Path(args.target)
    if not p.is_file():
        p = ROOT / "test-image" / args.target
    if not p.is_file():
        sys.exit(f"找不到：{args.target}")

    sc = A.Scorer(args.model)
    sims, l1_sims, keep = sc.score_paths([p])
    if not keep:
        sys.exit("这张图读不出来")

    row = sims[0]
    score = [float(row[sc.owner_arr == j].max()) for j in range(len(sc.vocab))]
    l1_row = l1_sims[0]

    print(f"\n=== {p.name} ===")
    print(f"模型 {sc.model_dir.name} · 词表 {len(sc.vocab)} 条 · 一级 {len(sc.l1s)} 个\n")

    print("一级（全部）")
    for i in sorted(range(len(sc.l1s)), key=lambda k: -l1_row[k]):
        # 截图/文档 已改规则判定，这里的分只作参考，不参与打标
        mark = "   ← 已弃用（改按文件名/OCR 规则）" if sc.l1s[i] in A.RULE_L1 else ""
        print(f"   {sc.l1s[i]:<6}{l1_row[i]:.4f}{mark}")

    print(f"\n二级 Top {args.top}")
    order = sorted(range(len(sc.vocab)), key=lambda j: -score[j])[: args.top]
    for j in order:
        v = sc.vocab[j]
        print(f"   {v['term']:<6}{score[j]:.4f}   [{v['l1']}]")


if __name__ == "__main__":
    main()
