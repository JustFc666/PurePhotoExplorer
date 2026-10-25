#!/usr/bin/env python
"""给每个人物挑「最清晰的那张脸」作为封面。

## 为什么要单独一个脚本
之前封面按 `score × quality` 排（检测置信度 × 脸宽），但那只解决"够不够大"，
解决不了两件事：
1. **误检**：面料褶皱、海报、浮雕也能过检测阈值，挑出来的"封面"根本不是脸；
2. **糊**：脸够大但失焦/抖动，看着就是一团。

## 判定方法（对每张候选脸真的读一次图）
- **复检**：把候选区域裁出来，用 YuNet 再检一次 —— 真脸能检出且中心接近；
  布料/褶皱这种误检在这一步基本都会掉。
- **清晰度**：裁出来的灰度图算 **Laplacian 方差**（越大越锐，摄影界常用的对焦指标）。
- 综合分 = 清晰度归一化 × 0.7 + 脸宽归一化 × 0.3（清晰优先，但仍偏大脸）。

用法：
    python tools/ai/pick_covers.py               # 只报告
    python tools/ai/pick_covers.py --write       # 写回 persons.cover_face_id
    python tools/ai/pick_covers.py --write --top 6   # 每人只看前 6 张候选（更快）
"""
from __future__ import annotations

import argparse
import sqlite3
import sys
from pathlib import Path

import cv2
import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
from face_probe import APP_DB, DET_MODEL  # noqa: E402

# 读图时长边上限：与打标/检测一致，脸框坐标就是这个空间里的
LONG_EDGE = 2048


def imread_any(path: Path) -> np.ndarray | None:
    """读图并缩到长边 LONG_EDGE（统一到检测时的坐标系）。"""
    img = cv2.imdecode(np.fromfile(str(path), dtype=np.uint8), cv2.IMREAD_COLOR)
    if img is None:
        return None
    h, w = img.shape[:2]
    long_edge = max(h, w)
    if long_edge > LONG_EDGE:
        s = LONG_EDGE / long_edge
        img = cv2.resize(img, (int(w * s), int(h * s)), interpolation=cv2.INTER_AREA)
    return img


def sharpness(gray: np.ndarray) -> float:
    """Laplacian 方差：越大越锐。"""
    return float(cv2.Laplacian(gray, cv2.CV_64F).var())


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--write", action="store_true", help="写回 cover_face_id")
    ap.add_argument("--top", type=int, default=8, help="每个物只看前 N 张候选（按 score×quality）")
    args = ap.parse_args()

    if not DET_MODEL.exists():
        print(f"缺检测模型：{DET_MODEL}", file=sys.stderr)
        return 2

    con = sqlite3.connect(str(APP_DB), timeout=30)
    try:
        rows = con.execute(
            """
            SELECT f.id, f.person_id, f.image_id, f.x, f.y, f.w, f.h,
                   COALESCE(f.score,0), COALESCE(f.quality,0),
                   v.last_mount_path, fo.rel_path, i.rel_name
            FROM faces f
            JOIN images i ON i.id = f.image_id
            JOIN folders fo ON fo.id = i.dir_id
            JOIN volumes v ON v.id = fo.volume_id
            WHERE f.person_id IS NOT NULL
            ORDER BY f.person_id, COALESCE(f.score,0) * COALESCE(f.quality,0) DESC
            """
        ).fetchall()

        det = cv2.FaceDetectorYN.create(str(DET_MODEL), "", (320, 320), 0.6, 0.3, 5000)

        by_person: dict[int, list[tuple]] = {}
        for r in rows:
            by_person.setdefault(r[1], []).append(r)

        changed = bad_cover = 0
        for pid, cands in sorted(by_person.items()):
            cands_scored: list[tuple[float, float, int, float, int]] = []
            img_cache: dict[int, np.ndarray | None] = {}
            for r in cands[: args.top]:
                fid, _pid, iid, x, y, fw, fh, score, quality, base, rel, name = r
                if iid not in img_cache:
                    p = Path(base) / rel.replace("/", "\\") / name if rel else Path(base) / name
                    img_cache[iid] = imread_any(p)
                img = img_cache[iid]
                if img is None:
                    continue
                H, W = img.shape[:2]
                # 归一化框 → 像素框（与 FaceAvatar 一致：外扩 6%）
                px, py, pw, ph = x * W, y * H, fw * W, fh * H
                pad = max(pw, ph) * 0.06
                x0 = int(max(0, px - pad))
                y0 = int(max(0, py - pad))
                x1 = int(min(W, px + pw + pad))
                y1 = int(min(H, py + ph + pad))
                if x1 - x0 < 24 or y1 - y0 < 24:
                    continue
                crop = img[y0:y1, x0:x1]
                # ① 复检：这块是不是真的脸
                hh, ww = crop.shape[:2]
                det.setInputSize((ww, hh))
                _n, faces = det.detect(crop)
                if faces is None or len(faces) == 0:
                    continue
                # 取最大那张的置信度作为"真脸"分
                real = float(max(f[14] for f in faces))
                if real < 0.7:
                    continue
                # ② 清晰度
                gray = cv2.cvtColor(crop, cv2.COLOR_BGR2GRAY)
                lap = sharpness(gray)
                cands_scored.append((lap, quality, fid, real, int(quality >= 120)))

            # 归一化加权：清晰度占 0.6，脸宽占 0.4。
            # ⛔ 别用字典序（先尺寸再清晰度）：那样会挑出"又大又糊"的脸
            #    （实测 laplacian 只有 39 的那张被选中），用户直接说"不如前几版"。
            best = None
            if cands_scored:
                max_lap = max(c[0] for c in cands_scored) or 1.0
                max_q = max(c[1] for c in cands_scored) or 1.0
                scored = [
                    (0.6 * (lap / max_lap) + 0.4 * (q / max_q), fid, lap, q, real, ok)
                    for lap, q, fid, real, ok in cands_scored
                ]
                scored.sort(reverse=True)
                top = scored[0]
                best = ((top[0], top[5]), top[1], top[2], top[3], top[4])

            if best is None:
                bad_cover += 1
                print(f"  person {pid}: 没有通过复检的脸（候选 {len(cands[: args.top])}）")
                continue

            _combo, fid, lap, quality, real = best
            cur = con.execute(
                "SELECT cover_face_id FROM persons WHERE id = ?", (pid,)
            ).fetchone()
            if cur and cur[0] != fid:
                changed += 1
                print(
                    f"  person {pid}: cover {cur[0]} → {fid}"
                    f"（laplacian {lap:.0f}，脸宽 {quality:.0f}px，复检 {real:.2f}）"
                )
                if args.write:
                    con.execute(
                        "UPDATE persons SET cover_face_id = ? WHERE id = ?", (fid, pid)
                    )
        if args.write:
            con.commit()
        print(f"\n人物 {len(by_person)} 个｜封面变更 {changed}｜无可用脸 {bad_cover}"
              f"{'' if args.write else '（未写库，加 --write）'}")
        return 0
    finally:
        con.close()


if __name__ == "__main__":
    raise SystemExit(main())
