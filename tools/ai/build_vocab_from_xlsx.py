#!/usr/bin/env python
# -*- coding: utf-8 -*-
"""从 `tag模板.xlsx`（一级 / 二级两列）生成打标词表 `zh_tags.csv`。

## 为什么单独一个脚本
`merge_thesaurus.py` 是从「原 524 行表」构建的（含大量合并规则）。
用户 2026-10-22 换了更精简的模板（15 一级 / 106 二级），结构不同，
直接由 xlsx 生成更直观；**改模板就重跑本脚本**，不要手改 csv。

## 会顺手处理的两类脏数据
- 完全重复的 (一级, 二级)：去重；
- 同一个二级挂在不同一级下（如 `足球` 既在「运动」又在「兵器」）：
  **保留第一次出现的**，并在输出里列出冲突，让人来决定。
  兵器下的「足球」显然是笔误（应为足球相关兵器？），不能猜，只提示。

用法：python tools/ai/build_vocab_from_xlsx.py [--xlsx path] [--dry]
"""

import argparse
import csv
from collections import OrderedDict
from pathlib import Path

import openpyxl

# ── 互斥组（2026-10-22 人工定义）──────────────────────────────
# 同一组里的二级**只能留一个**（取分最高的）。理由：这些概念在画面里互相排斥，
# 但 CLIP 会把它们一起打上（实测「单人 0.4057 / 合照 0.4103」只差 0.0046）。
# ⚠️ 分组键必须唯一；一个二级只能属于一个互斥组。
# ── 二级合并（2026-10-22 用户定）──────────────────────────────
# `教堂` 并进 `寺庙`：两者在照片里几乎无法区分，且用户不需要分开统计。
# 合并后：模板的「建筑/教堂」行消失，`教堂` 变成「寺庙」的同义词。
MERGE_INTO: dict[str, str] = {}  # 模板里已直接写成「寺庙/教堂」，不再需要程序合并

EXCLUSIVE = {
    # ✅ 真正互斥（一张图只能有一个）
    "人像-人数": ["单人", "合照"],
    "人像-景别": ["全身", "半身", "面部特写"],
    "人像-年龄": ["婴儿", "儿童"],
    "运动-项目": ["跑步", "游泳", "足球", "登山", "露营", "骑行", "冲浪", "滑翔伞", "篮球"],
    "宠物-种类": ["猫", "狗"],
    "文档-类型": ["证件", "银行卡", "发票"],
    "截图-类型": ["聊天截图", "游戏截图", "软件截图"],
    # 绘画 / 雕塑：同一件作品不会既是画又是雕塑（博物馆/二次元不参与，它们能同框）
    "艺术-类型": ["绘画", "雕塑"],
    "兵器-类型": ["武器", "刀剑", "坦克", "军舰"],
    # 野生动物：一张图的主体动物通常只有一种
    "动物-主体": [
        "鱼", "熊猫", "狮子", "老虎", "狼", "狐狸", "熊", "大象", "长颈鹿",
        "斑马", "鹿", "猴子", "海豚", "海星", "贝壳", "昆虫", "蝴蝶", "鸟", "蜜蜂",
    ],
    # ⛔ 用户已删除：美食-菜系、自然-天象（同框合理，不互斥）
}

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_XLSX = ROOT / "tag模板.xlsx"
OUT = ROOT / "crates" / "pv-ai" / "vocab" / "zh_tags.csv"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--xlsx", default=str(DEFAULT_XLSX))
    ap.add_argument("--dry", action="store_true")
    args = ap.parse_args()

    ws = openpyxl.load_workbook(args.xlsx, data_only=True).active
    pairs, seen, dup_same, conflicts = [], set(), [], {}

    # 模板第 3 列（可选）= 互斥组：用户在 Excel 里填了就优先于代码里的 EXCLUSIVE
    xlsx_group: dict[str, str] = {}

    for r in ws.iter_rows(min_row=2, values_only=True):
        if not r or not r[0] or not r[1]:
            continue
        l1, l2 = str(r[0]).strip(), str(r[1]).strip()
        if len(r) > 2 and r[2]:
            xlsx_group[l2] = str(r[2]).strip()
        # 合并：教堂 → 寺庙（用目标名继续走后面的去重，等于"这一行不要了"）
        if l2 in MERGE_INTO:
            merged_into = MERGE_INTO[l2]
            print(f"合并：{l2} → {merged_into}")
            l2 = merged_into
        if (l1, l2) in seen:
            dup_same.append((l1, l2))
            continue
        # 同一个二级挂到第二个一级下 → 记冲突，保留第一次
        owner = next((a for a, b in seen if b == l2), None)
        if owner is not None:
            conflicts.setdefault(l2, [owner]).append(l1)
            continue
        seen.add((l1, l2))
        pairs.append((l1, l2))

    groups = OrderedDict()
    for l1, l2 in pairs:
        groups.setdefault(l1, []).append(l2)

    print(f"一级 {len(groups)} 个 · 二级 {len(pairs)} 条")
    if dup_same:
        print(f"去重（完全重复）{len(dup_same)} 条：{dup_same}")
    for l2, owners in conflicts.items():
        print(f"⚠️ 「{l2}」同时挂在 {owners} 下 → 只保留「{owners[0]}」，请确认模板是否笔误")

    term_group = {t: g for g, ts in EXCLUSIVE.items() for t in ts}
    term_group.update(xlsx_group)  # 模板里填的优先
    # 合并过来的词记成同义词，方便检索/匹配
    merged_syn: dict[str, list[str]] = {}
    for src_, dst in MERGE_INTO.items():
        merged_syn.setdefault(dst, []).append(src_)

    rows = []
    for i, (l1, l2) in enumerate(pairs, start=1):
        # 「寺庙/教堂」这种合并名：展示用全名，匹配时拆成两个词当同义词
        parts = [x.strip() for x in l2.split("/") if x.strip()]
        syn = ";".join(merged_syn.get(l2, []) + (parts if len(parts) > 1 else []))
        rows.append([f"T{i:03d}", l1, l2, l2, syn, "", term_group.get(l2, "")])

    if args.dry:
        for r in rows[:5]:
            print("  ", r)
        print("（--dry，未写）")
        return

    with open(OUT, "w", encoding="utf-8-sig", newline="") as f:
        w = csv.writer(f)
        w.writerow(["编码", "一级类目", "二级类目", "主题词", "同义词", "英文对照", "互斥组"])
        w.writerows(rows)
    print(f"已写 {OUT}（{len(rows)} 条）")

    # 便于人工审阅的 Excel 副本（3 列：一级 / 二级 / 互斥组）
    try:
        # ⛔ 别在这里再 import openpyxl：函数内 import 会让整个 main 里的 openpyxl 变成局部变量，
        # 上面读取 xlsx 那行就会 UnboundLocalError。
        from openpyxl.styles import Font, PatternFill

        wb = openpyxl.Workbook()
        ws = wb.active
        ws.title = "词表"
        ws.append(["一级", "二级", "互斥组"])
        for c in ws[1]:
            c.font = Font(bold=True)
        for l1, l2 in pairs:
            ws.append([l1, l2, term_group.get(l2, "")])
        # 互斥行浅色底，便于一眼看出
        fill = PatternFill("solid", fgColor="FFF2CC")
        for row in ws.iter_rows(min_row=2):
            if row[2].value:
                for c in row:
                    c.fill = fill
        ws.column_dimensions["A"].width = 12
        ws.column_dimensions["B"].width = 16
        ws.column_dimensions["C"].width = 16
        ws.freeze_panes = "A2"

        ws2 = wb.create_sheet("互斥组说明")
        ws2.append(["互斥组", "成员", "规则"])
        for c in ws2[1]:
            c.font = Font(bold=True)
        for g, ts in EXCLUSIVE.items():
            ws2.append([g, " / ".join(ts), "只保留分数最高的一个"])
        ws2.column_dimensions["A"].width = 14
        ws2.column_dimensions["B"].width = 70
        ws2.column_dimensions["C"].width = 22

        xlsx = ROOT / "tag模板-互斥标注.xlsx"
        wb.save(xlsx)
        print(f"已写 {xlsx}")
    except Exception as e:  # noqa: BLE001
        print(f"（Excel 副本跳过：{e}）")


if __name__ == "__main__":
    main()
