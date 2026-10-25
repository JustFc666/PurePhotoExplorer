#!/usr/bin/env python
"""人脸聚类：从 `faces` 表读向量 → 两阶段 Chinese Whispers → 写 `persons`.

## 为什么两阶段（2026-10-24 实测定稿）
混合图库（含小红书美颜图）上单阈值必崩：0.40 会把 **221 张脸**糊成一簇，
而那些脸互相根本不是一个人。两阶段先用严阈值聚出"高纯度核心簇"，
再让散脸按**离簇心多远**并入 —— 只有簇心参与二次判定，比两两比较抗噪声。

## 安全默认
库里**已经有命名人物**时默认拒绝执行（命名是用户资产，不能被重聚类悄悄冲掉）；
确实要重建就加 `--reset`（会把所有人物清空重来）。

用法：
    python tools/ai/face_cluster.py                 # 聚类并写库
    python tools/ai/face_cluster.py --dry           # 只报告，不写库
    python tools/ai/face_cluster.py --reset         # 清掉所有人物重来
"""
from __future__ import annotations

import argparse
import sqlite3
import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
from face_probe import APP_DB, chinese_whispers, two_stage_cluster  # noqa: E402

# 只有 ≥3 张脸的簇才建「人物」：实测 105 个簇里 65 个是单例，
# 全建出来只是噪声（侧栏挂一堆"未命名 ⑰"没人会点）。
MIN_FACES = 3
CLUSTER_VERSION = "cw-0.55-0.50-v1"


def load_faces(con: sqlite3.Connection, model: str, min_quality: float):
    rows = con.execute(
        """SELECT id, embedding FROM faces
            WHERE model = ? AND COALESCE(quality, 0) >= ?
            ORDER BY id""",
        (model, min_quality),
    ).fetchall()
    ids = [r[0] for r in rows]
    if not ids:
        return [], np.zeros((0, 0), dtype=np.float32)
    mat = np.stack([np.frombuffer(r[1], dtype=np.int8).astype(np.float32) / 127.0 for r in rows])
    # int8 反量化后模长 ≈ 1（0.997~1.003），重新归一化一下更稳
    mat /= np.linalg.norm(mat, axis=1, keepdims=True) + 1e-9
    return ids, mat


def filter_cohesion(sim, labels: np.ndarray, thr: float, rounds: int = 2) -> np.ndarray:
    """簇内一致性过滤 —— 拆掉「链式连通」造成的假簇。

    Chinese Whispers 只看"有没有边"，于是 A~B、B~C、C~D 各 0.6 相似，
    但 A 与 D 只有 0.35，四个不同的人也会被连成一簇（婴儿/儿童脸尤其常见）。
    这里对每张脸算**它与簇内其它脸的平均相似度**，低于 thri 就踢出去，迭代几轮。

    ⛔ 不要用"与质心的相似度"：质心是平均值，会把两个不同的人"平均"成
       一个谁都不像的向量，反而放过误配。
    """
    out = labels.copy()
    for _ in range(rounds):
        changed = False
        for lb in sorted(set(out.tolist())):
            if lb < 0:
                continue
            idx = np.where(out == lb)[0]
            if len(idx) < 2:
                continue
            block = sim[np.ix_(idx, idx)]
            # 每张脸与簇内其它脸的平均相似度（减掉对角线上的 1.0）
            # ⚠️ 实测对比过中位数判据：中位数更"宽容"（簇内多数脸彼此像 → 中位数被抬高），
            #    同样门槛下剔除的脸更少、簇内最小相似度反而更差，所以用平均值。
            mean_sim = (block.sum(axis=1) - 1.0) / (len(idx) - 1)
            bad = mean_sim < thr
            if bad.any():
                out[idx[bad]] = -1
                changed = True
        if not changed:
            break
    return out


def split_loose(sim, labels: np.ndarray, min_ok: float = 0.45, thr: float = 0.6) -> np.ndarray:
    """把"内部有一对脸完全不像"的簇拆开。

    ⚠️ 为什么还需要这一步：`filter_cohesion` 用的是**平均相似度**，在几十张脸的大簇里
    混进一两个离群点，平均值仍可能过线（实测出现过 -0.27 的一对脸还留在同一个人物里）。
    这里对"最小两两相似度 < min_ok"的簇，用更高阈值（0.6）在**簇内**重跑一次 CW，
    最大的子簇沿用原 id，其余另起 id（拆出来的小簇若不足 3 张，后面会被 keep 过滤掉）。
    """
    out = labels.copy()
    next_lb = int(labels[labels >= 0].max()) + 1 if (labels >= 0).any() else 0
    for lb in sorted(set(out.tolist()) - {-1}):
        idx = np.where(out == lb)[0]
        if len(idx) < 2:
            continue
        sub = sim[np.ix_(idx, idx)]
        if sub.min() >= min_ok:
            continue
        sub_labels = chinese_whispers(sub, thr)
        buckets: dict[int, list[int]] = {}
        for i, sl in enumerate(sub_labels.tolist()):
            buckets.setdefault(sl, []).append(i)
        biggest = max(buckets.items(), key=lambda kv: len(kv[1]))[0]
        for sl, members in buckets.items():
            if sl == biggest:
                out[idx[members]] = lb
            else:
                out[idx[members]] = next_lb
                next_lb += 1
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", default="sface_2021dec")
    ap.add_argument("--min-quality", type=float, default=56.0, help="脸宽下限(px)，小脸不参与")
    ap.add_argument("--t-core", type=float, default=0.55)
    ap.add_argument("--t-join", type=float, default=0.50)
    ap.add_argument("--min-faces", type=int, default=MIN_FACES)
    ap.add_argument(
        "--cohesion",
        type=float,
        default=0.0,
        help="簇内一致性门槛：一张脸与同簇其它脸的平均相似度低于它就被踢出",
    )
    ap.add_argument("--dry", action="store_true", help="只报告不写库")
    ap.add_argument("--reset", action="store_true", help="清掉所有人物（包括已命名的）重来")
    args = ap.parse_args()

    if not APP_DB.exists():
        print(f"找不到索引库：{APP_DB}", file=sys.stderr)
        return 2

    con = sqlite3.connect(str(APP_DB), timeout=20)
    try:
        named = con.execute("SELECT COUNT(*) FROM persons WHERE name IS NOT NULL").fetchone()[0]
        total_p = con.execute("SELECT COUNT(*) FROM persons").fetchone()[0]
        if total_p and not args.reset:
            print(
                f"库里已有 {total_p} 个人物（其中 {named} 个已命名）。\n"
                "重聚类会冲掉这些结果 —— 确认要重建请加 --reset。",
                file=sys.stderr,
            )
            return 3

        face_ids, mat = load_faces(con, args.model, args.min_quality)
        if len(face_ids) < args.min_faces:
            print(f"可聚类的人脸太少（{len(face_ids)} 张）")
            return 0

        sim = mat @ mat.T
        labels = two_stage_cluster(sim, args.t_core, args.t_join)
        before = len(set(labels.tolist()) - {-1})
        if args.cohesion > 0:
            labels = filter_cohesion(sim, labels, args.cohesion)
            labels = split_loose(sim, labels)
        dropped = int((labels == -1).sum())
        print(
            f"簇内一致性过滤（门槛 {args.cohesion}）："
            f"{before} 簇 → {len(set(labels.tolist()) - {-1})} 簇，剔除 {dropped} 张脸"
        )
        # ⛔ 过滤后会有 -1（被剔除的脸），np.bincount 不接受负值 —— 直接跑会抛
        #    "list argument must have no negative elements"，而脚本前面已经把
        #    结果 print 出来了，很容易误以为跑成功了（2026-10-25 踩过）。
        counts = np.bincount(labels[labels >= 0])
        groups: dict[int, list[int]] = {}
        for idx, lb in enumerate(labels.tolist()):
            groups.setdefault(lb, []).append(idx)
        keep = {lb: g for lb, g in groups.items() if len(g) >= args.min_faces}

        print(f"人脸 {len(face_ids)} 张 · 簇 {len(groups)} 个 · 其中 ≥{args.min_faces} 张的 {len(keep)} 个")
        print(f"单例簇 {int((counts == 1).sum())} 个（不建人物，仍保留 faces 行）")
        sizes = sorted((len(g) for g in keep.values()), reverse=True)
        print(f"人物规模 top10：{sizes[:10]}")

        if args.dry:
            print("\n--dry：未写库")
            return 0

        # 写库：先清（--reset 时连命名人物一起），再建
        con.execute("UPDATE faces SET person_id = NULL")
        con.execute("DELETE FROM persons")

        # 大簇优先建（人物列表默认按张数排，先建先有序）
        for lb, g in sorted(keep.items(), key=lambda kv: len(kv[1]), reverse=True):
            member_ids = [face_ids[i] for i in g]
            cur = con.execute(
                """INSERT INTO persons (name, cover_face_id, face_count, cluster_version,
                                        created_ts)
                   VALUES (NULL, NULL, 0, ?, strftime('%s','now'))""",
                (CLUSTER_VERSION,),
            )
            pid = cur.lastrowid
            con.executemany(
                "UPDATE faces SET person_id = ? WHERE id = ?",
                [(pid, fid) for fid in member_ids],
            )
            # 封面：**最大的那张脸**（2026-10-25 用户定稿回退到此规则）。
            # 试过 score×quality、Laplacian 清晰度、加权综合，用户实测后仍觉得
            # "最大脸"最稳 —— 脸大通常意味着离镜头近、曝光足，比算法指标更符合直觉。
            cover = con.execute(
                """SELECT id FROM faces WHERE person_id = ?
                    ORDER BY w * h DESC LIMIT 1""",
                (pid,),
            ).fetchone()
            con.execute(
                "UPDATE persons SET face_count = ?, cover_face_id = ? WHERE id = ?",
                (len(member_ids), cover[0] if cover else None, pid),
            )

        # ⭐ 2026-10-25 用户要求：**同一张图上同一个人只算一次**。
        # 合影、浮雕、海报都会让同一张脸被检出多次（甚至把一群人聚成一个"人"）；
        # 这里按 (person_id, image_id) 只保留质量最高的一张，其余解绑。
        con.execute(
            """
            UPDATE faces SET person_id = NULL
             WHERE person_id IS NOT NULL
               AND id NOT IN (
                 SELECT id FROM (
                   SELECT id, ROW_NUMBER() OVER (
                     PARTITION BY person_id, image_id ORDER BY COALESCE(quality,0) DESC, id
                   ) AS rn FROM faces WHERE person_id IS NOT NULL
                 ) WHERE rn = 1
               )
            """
        )

        # 兜底：重算一遍（防止上面某步漏了）
        con.execute(
            """UPDATE persons SET face_count =
                 (SELECT COUNT(*) FROM faces WHERE faces.person_id = persons.id)"""
        )
        con.commit()

        got = con.execute("SELECT COUNT(*), SUM(face_count) FROM persons").fetchone()
        print(f"\n已写入 {got[0]} 个人物，覆盖 {got[1]} 张脸（其余留作未归属）")
        print("→ 建议接着跑 `tools/ai/pick_covers.py --write`：")
        print("   它会对每张候选脸**读图复检 + 算 Laplacian 清晰度**，挑出真正清晰且是真脸的那张当封面")
        print("⚠️ 应用需重扫（F5）或重启才能在界面看到")
        return 0
    finally:
        con.close()


if __name__ == "__main__":
    raise SystemExit(main())
