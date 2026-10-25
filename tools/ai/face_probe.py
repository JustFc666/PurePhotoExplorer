#!/usr/bin/env python
"""人脸探针：YuNet 检测 + SFace 识别 + Chinese Whispers 聚类。

**用途**：在真实图库上量出"该用什么阈值"，而不是拍脑袋定。
产出三张表：① 人脸分布 ② 同图负样本相似度分布 ③ 阈值扫描下的聚类质量。

模型（都放 `assets/ai/face/`，许可干净）：
- 检测 `yunet.onnx`（MIT，228 KB，5 关键点）
- 识别 `sface.onnx`（Apache-2.0，MobileFaceNet + SFace loss，128 维）

用法：
    python tools/ai/face_probe.py                       # 默认扫 test-image
    python tools/ai/face_probe.py --dir <目录> --limit 200
    python tools/ai/face_probe.py --reuse                # 复用上次的 embedding 缓存，只重跑聚类

⛔ 必须自己做 EXIF 转正：cv2 不解 EXIF，我们的解码器也不做 orientation，
   竖拍照片直接送进去特征全是错的（这是最容易漏的一步）。
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import cv2
import numpy as np
from PIL import Image, ImageOps

ROOT = Path(__file__).resolve().parent.parent.parent
FACE_DIR = ROOT / "assets" / "ai" / "face"
DET_MODEL = FACE_DIR / "yunet.onnx"
REC_MODEL = FACE_DIR / "sface.onnx"
EMB_CACHE = ROOT / "assets" / "ai" / "face_emb.json"
# 应用索引库（图库视图就是查它）
APP_DB = Path.home() / "AppData" / "Local" / "PurePhotoExplorer" / "library.db"

# cv2 能直接读的格式；HEIC / RAW / PSD 读不了，跳过并计数
IMG_EXTS = {".jpg", ".jpeg", ".png", ".bmp", ".webp", ".gif"}

# SFace 官方余弦阈值（OpenCV FaceRecognizerSF 的默认判定线）
SFACE_THRESHOLD = 0.363


def load_bgr(path: Path, long_side: int) -> np.ndarray:
    """读图 → EXIF 转正 → 长边降采样 → BGR。"""
    with Image.open(path) as im:
        im = ImageOps.exif_transpose(im)
        im = im.convert("RGB")
        w, h = im.size
        scale = long_side / max(w, h)
        if scale < 1:
            im = im.resize((max(1, int(w * scale)), max(1, int(h * scale))), Image.LANCZOS)
        return cv2.cvtColor(np.asarray(im), cv2.COLOR_RGB2BGR)


def paths_from_library(name: str) -> list[Path]:
    """从应用索引库里取某个图库的照片**绝对路径**（图库 = 若干目录的并集）。

    ⭐ 这才是"图库里有哪些图"的唯一真源：用户看到的是图库视图，
    不是某个文件夹。目录树外的图（比如 rednote）只有走这里才拿得到。
    """
    import sqlite3

    if not APP_DB.exists():
        raise SystemExit(f"找不到索引库：{APP_DB}")
    con = sqlite3.connect(f"file:{APP_DB}?mode=ro", uri=True)
    rows = con.execute(
        """
        SELECT v.last_mount_path, f.rel_path, i.rel_name
        FROM libraries l
        JOIN library_folders lf ON lf.library_id = l.id
        JOIN folders f ON f.id = lf.folder_id
        JOIN volumes v ON v.id = f.volume_id
        JOIN images i ON i.dir_id = f.id
        WHERE l.name = ? AND i.rel_name IS NOT NULL
        """,
        (name,),
    ).fetchall()
    con.close()
    out, missing = [], 0
    for base, rel, fname in rows:
        p = Path(base) / rel.replace("/", "\\") / fname if rel else Path(base) / fname
        if p.is_file():
            out.append(p)
        else:
            missing += 1
    if missing:
        print(f"⚠️ 索引里有 {missing} 张文件已不在磁盘（卷未挂载或已删除），跳过")
    return sorted(set(out))


def detect_and_embed(files: list[Path], args) -> tuple[list[dict], dict]:
    """逐张检测 + 取特征。返回 (faces, stats)。"""
    det = cv2.FaceDetectorYN.create(
        str(DET_MODEL), "", (320, 320), args.score, 0.3, 5000
    )
    rec = cv2.FaceRecognizerSF.create(str(REC_MODEL), "")

    faces: list[dict] = []
    stats = {"images": 0, "with_face": 0, "skipped": [], "multi": 0, "tiny": 0}
    for i, p in enumerate(files, 1):
        try:
            img = load_bgr(p, args.long_side)
        except Exception:
            stats["skipped"].append(p.name)
            continue
        stats["images"] += 1
        h, w = img.shape[:2]
        det.setInputSize((w, h))
        _, rows = det.detect(img)
        if rows is None or len(rows) == 0:
            continue
        stats["with_face"] += 1
        if len(rows) > 1:
            stats["multi"] += 1
        for f in rows:
            x, y, fw, fh = (float(v) for v in f[:4])
            score = float(f[-1])
            # 太小的脸特征不可信（文档 §4：小脸/模糊脸必须过滤）
            if min(fw, fh) < args.min_face:
                stats["tiny"] += 1
                continue
            try:
                aligned = rec.alignCrop(img, f)
                emb = rec.feature(aligned).ravel().astype(np.float32)
            except cv2.error:
                continue
            norm = float(np.linalg.norm(emb))
            if norm < 1e-6:
                continue
            emb = emb / norm
            faces.append(
                {
                    # 绝对路径：不同目录可能有同名文件，只用名字会串；
                    # ⛔ 必须 resolve —— 传相对目录（`--dir rednote`）时否则匹配不上索引库
                    "image": str(p.resolve()),
                    "name": p.name,
                    "x": round(x / w, 5),
                    "y": round(y / h, 5),
                    "w": round(fw / w, 5),
                    "h": round(fh / h, 5),
                    "px": round(fw, 1),  # 像素宽（判断质量用）
                    "score": round(score, 4),
                    "emb": [round(float(v), 5) for v in emb],
                }
            )
        if i % 50 == 0:
            print(f"  …{i}/{len(files)} 张，已收集 {len(faces)} 张脸", flush=True)
    return faces, stats


def cos_matrix(faces: list[dict]) -> np.ndarray:
    e = np.asarray([f["emb"] for f in faces], dtype=np.float32)
    return e @ e.T


def negative_pairs(faces: list[dict], sim: np.ndarray) -> np.ndarray:
    """同一张图里不同人脸的相似度 = 天然负样本（同图极少出现同一个人两次）。"""
    vals = []
    by_img: dict[str, list[int]] = {}
    for idx, f in enumerate(faces):
        by_img.setdefault(f["image"], []).append(idx)
    for idxs in by_img.values():
        for a in range(len(idxs)):
            for b in range(a + 1, len(idxs)):
                vals.append(sim[idxs[a], idxs[b]])
    return np.asarray(vals, dtype=np.float32)


def chinese_whispers(sim: np.ndarray, threshold: float, iters: int = 30, seed: int = 42):
    """Chinese Whispers 图聚类（PhotoPrism 用的就是这套）。

    比 DBSCAN 抗"链式误合并"：每次迭代让每个点投奔"邻居权重总和最大"的标签，
    只有**持续**的强连接才能把两个大簇拉到一起。
    """
    n = sim.shape[0]
    if n == 0:
        return np.zeros(0, dtype=np.int64)
    neigh = [np.where((sim[i] > threshold) & (np.arange(n) != i))[0] for i in range(n)]
    labels = np.arange(n)
    rng = np.random.default_rng(seed)
    order = np.arange(n)
    for _ in range(iters):
        changed = 0
        rng.shuffle(order)
        for i in order:
            nb = neigh[i]
            if nb.size == 0:
                continue
            w = sim[i, nb]
            # 按邻居标签累加权重，取最大者
            best_label, best_w = labels[i], -1.0
            acc: dict[int, float] = {}
            for j, ww in zip(nb.tolist(), w.tolist()):
                acc[labels[j]] = acc.get(labels[j], 0.0) + ww
            for lb, ww in acc.items():
                if ww > best_w + 1e-9:
                    best_label, best_w = lb, ww
            if labels[i] != best_label:
                labels[i] = best_label
                changed += 1
        if changed == 0:
            break
    _, inv = np.unique(labels, return_inverse=True)
    return inv.astype(np.int64)


def two_stage_cluster(
    sim: np.ndarray, t_core: float, t_join: float, min_core: int = 3
) -> np.ndarray:
    """两阶段聚类：先用**严阈值**聚出高纯度核心簇，再把散脸按"离簇心多远"并入。

    为什么需要它：社媒美颜图（磨皮/大眼/瘦脸）会让不同人的向量一起塌向"平均脸"，
    单一阈值陷两难 —— 松了张冠李戴，紧了全成单例。两阶段只用**簇心**做二次判定，
    比两两比较更能抵抗噪声。
    """
    labels = chinese_whispers(sim, t_core)
    counts = np.bincount(labels)
    core = [lb for lb, c in enumerate(counts) if c >= min_core]
    if not core:
        return labels
    medoid = {}
    for lb in core:
        idx = np.where(labels == lb)[0]
        sub = sim[np.ix_(idx, idx)]
        medoid[lb] = int(idx[int(sub.mean(axis=1).argmax())])
    out = labels.copy()
    for i in range(len(labels)):
        if counts[labels[i]] >= min_core:
            continue
        best, best_w = None, t_join
        for lb, m in medoid.items():
            w = float(sim[i, m])
            if w > best_w:
                best, best_w = lb, w
        if best is not None:
            out[i] = best
    _, inv = np.unique(out, return_inverse=True)
    return inv.astype(np.int64)


def cluster_report(faces: list[dict], sim: np.ndarray, thresholds: list[float]) -> list[dict]:
    out = []
    for t in thresholds:
        labels = chinese_whispers(sim, t)
        counts = np.bincount(labels) if labels.size else np.asarray([0])
        sizes = np.sort(counts)[::-1]
        out.append(
            {
                "t": t,
                "clusters": int(sizes.size) if labels.size else 0,
                "singletons": int((counts == 1).sum()) if labels.size else 0,
                "top5": [int(v) for v in sizes[:5]],
                "median": int(np.median(counts)) if labels.size else 0,
            }
        )
    return out


def dump_cluster_sheets(faces: list[dict], labels: np.ndarray, out_dir: Path, per: int = 20) -> int:
    """把每个簇的人脸拼成一张 contact sheet，供人眼核对「这堆是不是同一个人」。

    ⭐ 这是整个探针最有说服力的产出：阈值扫描只能给数字，
    真正决定阈值的是"用户能不能一眼认出这些人是谁"。
    """
    out_dir.mkdir(parents=True, exist_ok=True)
    groups: dict[int, list[int]] = {}
    for idx, lb in enumerate(labels.tolist()):
        groups.setdefault(int(lb), []).append(idx)
    order = sorted(groups.items(), key=lambda kv: len(kv[1]), reverse=True)

    cache: dict[str, np.ndarray] = {}
    written = 0
    print("\n" + "=" * 66)
    print("④ 每簇抽样（拼图写到 assets/ai/clusters/，自己看是不是同一个人）")
    print("=" * 66)
    for rank, (lb, idxs) in enumerate(order[:20], 1):
        # 挑脸大的优先（更清楚），最多 per 张
        idxs = sorted(idxs, key=lambda i: faces[i]["px"], reverse=True)[:per]
        cells = []
        for i in idxs:
            f = faces[i]
            img = cache.get(f["image"])
            if img is None:
                try:
                    img = load_bgr(Path(f["image"]), 2048)
                except Exception:
                    continue
                cache[f["image"]] = img
            h, w = img.shape[:2]
            x0, y0 = int(f["x"] * w), int(f["y"] * h)
            x1, y1 = int((f["x"] + f["w"]) * w), int((f["y"] + f["h"]) * h)
            crop = img[max(0, y0) : max(1, y1), max(0, x0) : max(1, x1)]
            if crop.size == 0:
                continue
            cells.append(cv2.resize(crop, (112, 112), interpolation=cv2.INTER_AREA))
        if not cells:
            continue
        cols = 5
        rows = (len(cells) + cols - 1) // cols
        sheet = np.full((rows * 112, cols * 112, 3), 24, dtype=np.uint8)
        for k, c in enumerate(cells):
            r, cc = divmod(k, cols)
            sheet[r * 112 : (r + 1) * 112, cc * 112 : (cc + 1) * 112] = c
        name = f"cluster_{rank:02d}_size{len(groups[lb])}.jpg"
        cv2.imwrite(str(out_dir / name), sheet, [cv2.IMWRITE_JPEG_QUALITY, 88])
        written += 1
        imgs = sorted({faces[i]["name"] for i in groups[lb]})
        print(f"  {name:34s} 簇内 {len(groups[lb]):>3} 张脸 / {len(imgs):>3} 张图  例: {', '.join(imgs[:3])}")
    if len(order) > 20:
        print(f"  …另有 {len(order) - 20} 个更小的簇未导出")
    return written


def write_db(faces: list[dict], model: str, tier: int) -> tuple[int, int, int]:
    """把检测结果写进应用索引库的 `faces` 表。

    ## 三条必须与 Rust 侧对齐的约定
    1. **int8 量化**：`round(clamp(v,-1,1) * 127)` 存成 1 字节（有符号 → 取补码低 8 位），
       与 `pv_index::repo::faces::quantize` 完全一致，Rust 读出来 `as i8 / 127.0` 即可；
    2. **按图幂等**：先 `DELETE FROM faces WHERE image_id=?` 再插（重跑不会攒重复行）；
    3. **`model` 必填**：换模型要整库重建，没版本号的数据无法判断能不能混。

    返回 `(写入张数, 索引里找不到图的张数, 命中的图片数)`。
    """
    import sqlite3

    if not APP_DB.exists():
        raise SystemExit(f"找不到索引库：{APP_DB}")

    def quant(v: float) -> int:
        q = max(-127, min(127, int(round(max(-1.0, min(1.0, v)) * 127.0))))
        return q & 0xFF

    con = sqlite3.connect(str(APP_DB), timeout=20)
    try:
        # 绝对路径 → image_id（卷 + 相对路径拼回来，与 Rust 侧同一套规则）
        path2id: dict[str, int] = {}
        for base, rel, name, iid in con.execute(
            """
            SELECT v.last_mount_path, f.rel_path, i.rel_name, i.id
            FROM images i
            JOIN folders f ON f.id = i.dir_id
            JOIN volumes v ON v.id = f.volume_id
            WHERE i.rel_name IS NOT NULL
            """
        ):
            p = Path(base) / rel.replace("/", "\\") / name if rel else Path(base) / name
            path2id[str(p).lower()] = iid

        by_img: dict[str, list[dict]] = {}
        for f in faces:
            by_img.setdefault(f["image"], []).append(f)

        written = missing = 0
        touched = 0
        for path, fs in by_img.items():
            iid = path2id.get(path.lower())
            if iid is None:
                missing += len(fs)
                continue
            con.execute("DELETE FROM faces WHERE image_id = ?", (iid,))
            con.executemany(
                """INSERT INTO faces
                     (image_id, person_id, x, y, w, h, tier, model, score, quality, embedding)
                   VALUES (?, NULL, ?, ?, ?, ?, ?, ?, ?, ?, ?)""",
                [
                    (
                        iid,
                        f["x"],
                        f["y"],
                        f["w"],
                        f["h"],
                        tier,
                        model,
                        f["score"],
                        f["px"],
                        bytes(quant(v) for v in f["emb"]),
                    )
                    for f in fs
                ],
            )
            written += len(fs)
            touched += 1
        con.commit()
        return written, missing, touched
    finally:
        con.close()


# 人像类目的标签名（一级 + 二级）+ 证件：只有这些图才值得跑人脸
PERSON_TAGS = (
    "人像", "单人", "合照", "半身", "全身", "面部特写", "儿童", "婴儿", "证件",
)


def paths_with_person_tags(library: str) -> set[str]:
    """打了人像类目 / 证件 标签的图片（绝对路径小写集合）。

    ⭐ 用户口径：**只对已检测出人像 tag 或证件的图做人脸识别**，
    风景、静物、截图这些跑人脸纯属浪费（还容易把雕像/海报认成人）。
    """
    import sqlite3

    con = sqlite3.connect(str(APP_DB), timeout=20)
    try:
        ph = ",".join("?" * len(PERSON_TAGS))
        rows = con.execute(
            f"""
            SELECT v.last_mount_path, f.rel_path, i.rel_name
            FROM images i
            JOIN folders f ON f.id = i.dir_id
            JOIN volumes v ON v.id = f.volume_id
            WHERE i.rel_name IS NOT NULL
              AND i.id IN (
                SELECT it.image_id FROM image_tags it
                JOIN tags t ON t.id = it.tag_id
                WHERE t.name IN ({ph})
              )
            """,
            PERSON_TAGS,
        ).fetchall()
        out: set[str] = set()
        for base, rel, name in rows:
            p = Path(base) / rel.replace("/", "\\") / name if rel else Path(base) / name
            out.add(str(p).lower())
        return out
    finally:
        con.close()


def pct(a: np.ndarray, q: float) -> float:
    return float(np.percentile(a, q)) if a.size else float("nan")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--dir", default=str(ROOT / "test-image"))
    ap.add_argument(
        "--library",
        default="",
        help="按**图库名**取照片（读应用索引库，图库=若干目录的并集）；给了它就不用 --dir",
    )
    ap.add_argument("--long-side", type=int, default=1024, help="送检测的长边（降采样，别用原图）")
    ap.add_argument("--score", type=float, default=0.6, help="YuNet 置信度阈值")
    ap.add_argument("--min-face", type=float, default=80, help="脸最小边长(px)，小于它不进聚类（80 = 头像 80px 显示时源充足，不再放大发糊）")
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--reuse", action="store_true", help="复用 embedding 缓存，只重跑聚类")
    ap.add_argument("--dump", action="store_true", help="导出每簇的拼图（人眼核对用）")
    ap.add_argument("--dump-t", type=float, default=0.40, help="导出拼图用的阈值")
    ap.add_argument("--two-stage", action="store_true", help="导出拼图时用两阶段聚类")
    ap.add_argument(
        "--only-person-tags",
        action="store_true",
        help="只处理打了人像类目或「证件」标签的图（2026-10-25 用户要求：其余图不做人脸）",
    )
    ap.add_argument("--write-db", action="store_true", help="把结果写进索引库 faces 表")
    ap.add_argument("--model-tag", default="sface_2021dec", help="写库时记录的模型版本")
    ap.add_argument("--out", default=str(ROOT / "assets" / "ai" / "face_probe.json"))
    args = ap.parse_args()

    if not DET_MODEL.exists():
        print(f"缺检测模型：{DET_MODEL}", file=sys.stderr)
        return 2
    if not REC_MODEL.exists():
        print(f"缺识别模型：{REC_MODEL}（OpenCV Zoo 的 face_recognition_sface_2021dec.onnx）", file=sys.stderr)
        return 2

    if args.reuse and EMB_CACHE.exists():
        data = json.loads(EMB_CACHE.read_text(encoding="utf-8"))
        faces, stats = data["faces"], data["stats"]
        print(f"复用缓存：{len(faces)} 张脸（{stats.get('images', 0)} 张图）")
    else:
        if args.library:
            files = paths_from_library(args.library)
            src = f"图库「{args.library}」"
        else:
            files = sorted(p for p in Path(args.dir).iterdir() if p.suffix.lower() in IMG_EXTS)
            src = args.dir
        # ⛔ 不要把它塞进 `if args.limit` 里：不传 limit 时整段会被跳过
        if args.only_person_tags:
            keep = paths_with_person_tags(args.library)
            before = len(files)
            files = [p for p in files if str(p.resolve()).lower() in keep]
            print(f"只做人像/证件标签图：{before} → {len(files)} 张")
        if args.limit:
            files = files[: args.limit]
        print(f"扫描 {len(files)} 张（{src}，长边 {args.long_side}, 阈值 {args.score}）")
        faces, stats = detect_and_embed(files, args)
        EMB_CACHE.parent.mkdir(parents=True, exist_ok=True)
        EMB_CACHE.write_text(
            json.dumps({"faces": faces, "stats": stats}, ensure_ascii=False), encoding="utf-8"
        )

    print("\n" + "=" * 66)
    print("① 人脸分布")
    print("=" * 66)
    print(f"可解码图片      : {stats['images']}（跳过 {len(stats['skipped'])}：HEIC/RAW/损坏）")
    print(f"含人脸的图片    : {stats['with_face']}（{stats['with_face'] / max(1, stats['images']):.1%}）")
    print(f"多人合影        : {stats['multi']} 张")
    print(f"入库人脸        : {len(faces)}（丢弃小脸 {stats.get('tiny', 0)} 张，<{args.min_face:g}px）")
    if faces:
        pxs = np.asarray([f["px"] for f in faces])
        print(f"脸宽像素         : p10={pct(pxs, 10):.0f} p50={pct(pxs, 50):.0f} p90={pct(pxs, 90):.0f}")

    if len(faces) < 2:
        print("\n人脸太少，聚类无意义。")
        return 0

    sim = cos_matrix(faces)
    neg = negative_pairs(faces, sim)

    print("\n" + "=" * 66)
    print("② 同图负样本相似度（天然「不同人」分布，用来定阈值上界）")
    print("=" * 66)
    if neg.size:
        print(f"负样本对数      : {neg.size}")
        print(
            f"相似度           : p50={pct(neg, 50):.3f} p90={pct(neg, 90):.3f} "
            f"p99={pct(neg, 99):.3f} max={neg.max():.3f}"
        )
        over = int((neg > SFACE_THRESHOLD).sum())
        print(
            f"超过 {SFACE_THRESHOLD:.3f} 的 : {over} 对（{over / neg.size:.2%}）"
            " ← 这些会被错合并，越高说明阈值要往上调"
        )
    else:
        print("没有合影（每张图只有一张脸），无法统计负样本 —— 多放几张合影再跑。")

    thresholds = [0.25, 0.30, 0.35, 0.363, 0.40, 0.45, 0.50]
    rows = cluster_report(faces, sim, thresholds)
    print("\n" + "=" * 66)
    print("③ 阈值扫描（Chinese Whispers）")
    print("=" * 66)
    print(f"{'阈值':>6} {'簇数':>6} {'单例簇':>8} {'中位簇':>8}  top5 簇大小")
    for r in rows:
        print(
            f"{r['t']:>6.3f} {r['clusters']:>6} {r['singletons']:>8} {r['median']:>8}  {r['top5']}"
        )
    print("\n判读：单例簇占比过高 → 阈值太严（同一人被拆开）；")
    print("      最大簇异常大    → 阈值太松（不同人被合并）。取向：宁可多簇，不要错合并。")

    if args.dump:
        if args.two_stage:
            labels = two_stage_cluster(sim, args.dump_t, args.dump_t - 0.05)
            print(f"\n[两阶段] t_core={args.dump_t:.2f} / t_join={args.dump_t - 0.05:.2f}")
        else:
            labels = chinese_whispers(sim, args.dump_t)
        n = dump_cluster_sheets(faces, labels, ROOT / "assets" / "ai" / "clusters")
        print(f"共导出 {n} 张拼图（阈值 {args.dump_t:.3f}）")

    print("\n" + "=" * 66)
    print("⑤ 两阶段聚类对比（严阈值出核心簇 → 散脸按簇心并入）")
    print("=" * 66)
    print(f"{'t_core':>7} {'t_join':>7} {'簇数':>6} {'单例簇':>8} {'中位簇':>8}  top5 簇大小")
    for t_core, t_join in [(0.50, 0.45), (0.50, 0.50), (0.55, 0.50), (0.45, 0.42)]:
        lb = two_stage_cluster(sim, t_core, t_join)
        counts = np.bincount(lb)
        sizes = np.sort(counts)[::-1]
        print(
            f"{t_core:>7.2f} {t_join:>7.2f} {sizes.size:>6} "
            f"{int((counts == 1).sum()):>8} {int(np.median(counts)):>8}  {[int(v) for v in sizes[:5]]}"
        )

    if args.write_db:
        w, miss, imgs = write_db(faces, args.model_tag, args.long_side)
        print("\n" + "=" * 66)
        print("⑥ 写库")
        print("=" * 66)
        print(f"写入 {w} 张脸 → {imgs} 张图；索引里找不到的 {miss} 张（未入库 / 卷离线）")
        print("⚠️ 应用若正在运行，界面要重扫（F5）或重启才会看到新数据")

    Path(args.out).write_text(
        json.dumps(
            {"stats": stats, "faces": faces, "cluster_sweep": rows, "negative": {
                "pairs": int(neg.size),
                "p50": round(pct(neg, 50), 4) if neg.size else None,
                "p99": round(pct(neg, 99), 4) if neg.size else None,
                "max": round(float(neg.max()), 4) if neg.size else None,
            }},
            ensure_ascii=False,
        ),
        encoding="utf-8",
    )
    print(f"\n明细已写 {args.out}（faces 带归一化坐标与 128 维归一化向量）")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
