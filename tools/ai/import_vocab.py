#!/usr/bin/env python
# -*- coding: utf-8 -*-
"""把用户改过的「① 词表_可编辑.csv」导回 `crates/pv-ai/vocab/zh_tags.csv`。

## 支持用户怎么改
- **删词**：把 `启用` 列改 `否`（推荐，可追溯），或直接删掉整行；
- **加词**：在末尾加行，填 一级/二级/主题词（编码留空会自动补，同义词/英文可空）；
- **改分类**：改 一级类目 / 二级类目；
- **改同义词**：用 `;` 分隔。

## 不做什么
- 不动 `merge_thesaurus.py` 的 MERGE/DROP（那是「原表 → 词表」的构建规则）。
  本脚本直接改**产出的词表**；以后再跑 merge 脚本会被覆盖，
  所以改完请把变更同步给 merge 脚本（或直接以本表为准）。

用法：
    python tools/ai/import_vocab.py "release/tag导出/① 词表_385_可编辑.csv" [--dry]
"""

import argparse
import csv
import shutil
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TARGET = ROOT / "crates" / "pv-ai" / "vocab" / "zh_tags.csv"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("src", help="用户改过的 CSV")
    ap.add_argument("--dry", action="store_true", help="只检查，不写回")
    args = ap.parse_args()

    rows = list(csv.DictReader(open(args.src, encoding="utf-8-sig")))
    if not rows:
        sys.exit("空文件")

    # 表头兼容：用户可能在 Excel 里改过多余列
    def col(r, *names):
        for n in names:
            if n in r and r[n] is not None:
                return (r[n] or "").strip()
        return ""

    out, used_codes = [], set()
    dropped = 0
    for r in rows:
        term = col(r, "主题词")
        if not term:
            continue
        if col(r, "启用") in ("否", "N", "n", "0", "false", "FALSE"):
            dropped += 1
            continue
        l1 = col(r, "一级类目", "一级")
        l2 = col(r, "二级类目", "二级")
        if not l1 or not l2:
            sys.exit(f"「{term}」缺一级/二级类目")
        code = col(r, "编码")
        if not code or code in used_codes:
            code = f"U{len(out) + 1:03d}"
        used_codes.add(code)
        out.append([code, l1, l2, term, col(r, "同义词"), col(r, "英文对照")])

    if not out:
        sys.exit("过滤后一条都不剩，检查「启用」列")

    print(f"读入 {len(rows)} 行 → 保留 {len(out)} 条（停用 {dropped} 条）")

    # 简单校验：主题词不能重复
    seen, dup = set(), []
    for r in out:
        if r[3] in seen:
            dup.append(r[3])
        seen.add(r[3])
    if dup:
        sys.exit(f"主题词重复：{sorted(set(dup))[:10]}")

    if args.dry:
        for r in out[:5]:
            print("  ", r)
        print("（--dry，未写回）")
        return

    backup = TARGET.with_suffix(".csv.bak")
    shutil.copy2(TARGET, backup)
    with open(TARGET, "w", encoding="utf-8-sig", newline="") as f:
        w = csv.writer(f)
        w.writerow(["编码", "一级类目", "二级类目", "主题词", "同义词", "英文对照"])
        w.writerows(out)
    print(f"已写回 {TARGET}（备份 {backup.name}）")
    print("下一步：重跑 tools/ai/auto_tag.py --reset --write 让新词表生效")


if __name__ == "__main__":
    main()
