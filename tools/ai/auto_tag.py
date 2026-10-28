#!/usr/bin/env python
# -*- coding: utf-8 -*-
"""CN-CLIP 零样本打标 —— 离线验证脚本（2026-10-20）。

## 这一步是「验证」，不是最终形态
最终形态是 **Rust + ONNX Runtime(DirectML)**（见 docs/AI自动打标与人脸识别-可行性.md）。
本脚本用 Python 把整条链路先跑通、把结果写进真实索引库，用来验证：
- 词表选得对不对（423 条 zh_tags.csv）
- 阈值该定多少
- 每张 8 个标签是否合理
- EXIF 标签（品牌 / 机型 / 镜头 / 地点）是否该单独加

## 用法
```
J:/ai/tts/.venv/Scripts/python.exe tools/ai/auto_tag.py --library 测试图库 --dry
J:/ai/tts/.venv/Scripts/python.exe tools/ai/auto_tag.py --library 测试图库 --write
```

## 关键实现
- 文本侧：每个词条跑 **2 条模板**（裸词 + 「一张X的照片」），同义词再各加一条；
  同一词条取 **max**（不是 mean）—— 同义词是「另一种叫法」，max 才不会互相稀释。
- 图像侧：`ImageOps.exif_transpose` **必须先做**（我们的解码器不做方向校正，
  竖拍不转会直接毁掉特征）。
- 打分：余弦相似度；阈值默认 0.20（低于它的宁可不显示，参考 §7.10）。
"""

import argparse
import asyncio
import csv
import json
import os
import re
import sqlite3
import tempfile
import sys
import time
from pathlib import Path

import numpy as np
import torch
from PIL import Image, ImageOps
from transformers import ChineseCLIPModel, ChineseCLIPProcessor

ROOT = Path(__file__).resolve().parents[2]
MODEL_B = ROOT / "assets" / "ai" / "chinese-clip"
MODEL_H = ROOT / "assets" / "ai" / "chinese-clip-h14"


def pick_model_dir(explicit):
    """选模型目录：显式指定优先，**默认仍用 ViT-B/16**。

    ## 为什么 H/14 不是默认（2026-10-22 实测）
    在 110 张测试图库上对比：
    - 泛词占比 B/16 10% vs H/14 9%（几乎没区别）
    - 节日误判 B/16 17 次 vs H/14 **24 次**（更差）
    - 标签种类 216 vs 187（H 更保守）
    - 速度 **0.10 s/张 vs 1.34 s/张**（慢 13 倍）
    → 结论：**换大模型不解决问题**，瓶颈是「用 CLIP 对 385 个中文词做零样本匹配」这个范式
      和打分方式（分布极平：P10=0.20 / P90=0.26，模型其实分不出这些词）。
    H/14 保留在 `assets/ai/chinese-clip-h14`，需要时 `--model` 显式指定。
    ⚠️ 阈值与模型强相关：B/16 用 0.35，H/14 必须用 0.20，不能跨模型复用。
    """
    if explicit:
        return Path(explicit)
    return MODEL_B
VOCAB = ROOT / "crates" / "pv-ai" / "vocab" / "zh_tags.csv"
DB = Path(os.path.expandvars(r"%LOCALAPPDATA%\PurePhotoExplorer\library.db"))

TEMPLATES = ["{}", "一张{}的照片"]


def load_vocab():
    rows = list(csv.DictReader(VOCAB.open(encoding="utf-8-sig")))
    out = []
    for r in rows:
        syn = [s for s in r["同义词"].split(";") if s]
        out.append({
            "group": (r.get("互斥组") or "").strip(),
            "term": r["主题词"],
            "l1": r["一级类目"],
            "l2": r["二级类目"],
            "syn": syn,
            "en": r["英文对照"],
        })
    return out


def db_settings():
    """读 UI 里保存的 AI 配置（阈值 / 每张最多 / 停用的词）。

    UI 与脚本共用 `settings` 表，所以「在界面上关掉的词」这里必须真跳过，
    否则用户改了半天发现脚本照旧 —— 这是最容易失信的地方。
    """
    try:
        con = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
        rows = dict(con.execute("SELECT key, value FROM settings").fetchall())
        con.close()
    except Exception:
        return {}
    return rows


# 一级 / 二级 的生效阈值（用户 2026-10-23 定，见 main() 里的规则注释）
L1_WEAK = 0.36    # 一级"勉强可用"线：必须配合二级过线
L1_STRONG = 0.40  # 一级"单独成立"线
L2_MIN = 0.41     # 二级生效线

# ── 一级阈值的**按类目覆盖**（2026-10-24 用户要求）────────────────────────
# 为什么需要：CLIP 对不同一级词的**基线差异很大**。统一 0.40 时，
# 「活动 / 兵器」这类泛词天然高分（占全库一半），而「美食 / 植物 / 宠物」天然低分，
# 同一把尺子只会把前者放宽、把后者砍死。所以低频类目单独放低。
# ⚠️ 只影响一级，二级仍是 L2_MIN；未列出的类目仍用 `--l1-threshold`（当前 0.40）。
L1_THRESHOLD_OVERRIDE = {
    "美食": 0.39,
}


def l1_threshold(name, base):
    """某个一级类目的准入线：优先用覆盖表，否则用全局值。"""
    return L1_THRESHOLD_OVERRIDE.get(name, base)


# ── 组合规则：两个证据凑一起才成立的标签（2026-10-24 用户要求）──────────────
# 现在只有一条：**博物馆 + 人群 → 旅游**。
# 单看「博物馆」是展陈（文物/雕塑/古迹），再出现一群人，才是"旅游"这件事。
# 这类"人在干什么"的语义 CLIP 给不出来（它只懂画面里有什么），所以用规则拼。
#
# ⚠️ "人群"的实现：原计划用「合照」（≥2 张脸），但实测**一次都不触发** ——
#    测试图库 博物馆 26 张、合照 17 张，**交集 0**：远景人群的脸在 1000px 缩放下
#    太小，YuNet 根本检不到。所以条件放宽成 `people` = **画面里有任意人脸**
#    （这类图实测 4 张，才是真正会命中的量）。要严格"多人"就把条件改回 `crowd`。
COMPOSITE_RULES = [
    # (触发词**或一级类目**, 额外条件, 产出的标签)；条件：people=有任意人脸 / crowd=≥2 张脸
    ("博物馆", "people", "旅游"),
    # 一级类目当触发词：自然（森林/山/湖泊/沙滩…）+ 画面里有人 = 旅游照
    ("自然", "people", "旅游"),
]

# ── 场景内外对立（2026-10-24 用户要求）─────────────────────────────────
# ⚠️ 和「互斥组」语义**不同**，别混：
#   互斥组 = 同一维度只留一个（单人 vs 合照）；
#   对立组 = 两派对峙，**输了的那一派整派删掉**（夜里拍的山被判成"商场"→ 把室内那派全砍）。
# 为什么不能做成一个互斥组：把 14 个室外词塞进同一个组，会把「森林+山+湖泊」压成一个。
# 这份名单是可调的：只收**含义明确**的，模糊的（天空/公园/街景/夜市/机场）先不收。
INDOOR_TERMS = frozenset({"商场", "酒店", "学校", "医院", "工厂", "博物馆"})
OUTDOOR_TERMS = frozenset({
    "森林", "海洋", "山", "峡谷", "沙滩", "草原", "雪", "山洞",
    "湖泊", "河流", "星空", "日出日落", "太阳", "农田/农场",
})

OPPOSITE_PAIRS = [
    ("场景-内外", INDOOR_TERMS, OUTDOOR_TERMS),
]


def build_l1_prompts(vocab):
    """一级类目的文本向量（分层打分的第一级）。

    一级只有 20 个候选，CLIP 在这一层是**准的**；错得多的是「385 个词里挑 8 个」。
    所以先判类目、再在命中的类目里挑词条 —— 每层候选从 385 降到 10~30。
    """
    l1s = sorted({v["l1"] for v in vocab})
    prompts, owner = [], []
    for i, name in enumerate(l1s):
        # 两种说法取平均：裸名 + 「主题」句式。裸名对「风格/材质」这类抽象类目更稳
        for tpl in ("{}", "一张{}主题的照片"):
            prompts.append(tpl.format(name))
            owner.append(i)
    return l1s, prompts, owner


def build_prompts(vocab):
    """返回 (prompts, owner)：owner[i] = 该 prompt 属于第几个词条。"""
    prompts, owner = [], []
    for i, v in enumerate(vocab):
        for t in TEMPLATES:
            prompts.append(t.format(v["term"]))
            owner.append(i)
        # 同义词各加一条（用「一张X的照片」，中文更稳）
        for s in v["syn"][:3]:
            prompts.append(TEMPLATES[1].format(s))
            owner.append(i)
    return prompts, owner


@torch.no_grad()
def text_matrix(model, proc, prompts, device):
    feats = []
    for i in range(0, len(prompts), 64):
        batch = prompts[i:i + 64]
        inp = proc(text=batch, return_tensors="pt", padding=True, truncation=True).to(device)
        f = model.get_text_features(**inp)
        feats.append(f / f.norm(dim=-1, keepdim=True))
    return torch.cat(feats)


@torch.no_grad()
def image_features(model, proc, paths, device, batch=8):
    """返回 (特征矩阵, 有效下标)。读不出来的（RAW/HEIC，PIL 没插件）跳过。"""
    feats, keep = [], []
    for i in range(0, len(paths), batch):
        imgs, idx = [], []
        for j, p in enumerate(paths[i:i + batch], start=i):
            try:
                im = Image.open(p)
                # ⭐ draft：JPEG 在解码阶段就降采样，6000×4000 的图不用整幅进内存
                im.draft("RGB", (448, 448))
                imgs.append(ImageOps.exif_transpose(im).convert("RGB"))
                idx.append(j)
            except Exception as e:
                print(f"  跳过 {Path(p).name}: {type(e).__name__}", flush=True)
        if not imgs:
            continue
        inp = proc(images=imgs, return_tensors="pt").to(device)
        f = model.get_image_features(**inp)
        feats.append((f / f.norm(dim=-1, keepdim=True)).float().cpu())
        keep.extend(idx)
        print(f"  图像塔 {min(i + batch, len(paths))}/{len(paths)}", flush=True)
    return torch.cat(feats) if feats else torch.zeros((0, 1)), keep


# ── 人脸检测（人像类标签的唯一来源）─────────────────────────────────
# ⛔ 为什么不用 CLIP 判人像：实测 257 张里 CLIP 只认出 26 张，而人脸检测找到 104 张，
#    前者召回 12%、还有 14 张误报。CLIP 的「人像」分数在 0.33~0.38 之间挤成一团，
#    阈值怎么定都切不开 —— 这类"实体有无"必须交给专用检测器。
PORTRAIT_TERMS = {"人像", "单人", "合照", "半身", "全身", "面部特写"}

# 人脸置信度门槛：YuNet 输出 0~1，实测 0.5~0.7 那一段基本都是误检（占 29%）。
# ⚠️ 这个值和 CLIP 的余弦（0.3~0.5）**不是一个量纲**，别拿去和二级阈值比较。
FACE_MIN = 0.90

_FACE_CASCADES = None
_YUNET = None


def _yunet():
    """YuNet（ONNX，228KB）—— 比 Haar 准得多。

    ⛔ 别退回 Haar：实测沙丘图会被 Haar 误检出一张"脸"（面积 0.41%），
    而 YuNet 对同一张图是 0 张；合影图 Haar 检 4 张、YuNet 检 7 张（更全）。
    """
    global _YUNET
    if _YUNET is None:
        import cv2

        m = ROOT / "assets" / "ai" / "face" / "yunet.onnx"
        _YUNET = cv2.FaceDetectorYN.create(str(m), "", (320, 320), 0.6, 0.3, 5000) if m.is_file() else False
    return _YUNET or None


def _face_cascades():
    global _FACE_CASCADES
    if _FACE_CASCADES is None:
        import cv2

        base = cv2.data.haarcascades
        cs = [cv2.CascadeClassifier(base + "haarcascade_frontalface_default.xml")]
        prof = cv2.CascadeClassifier(base + "haarcascade_profileface.xml")
        if not prof.empty():
            cs.append(prof)  # 侧脸：合照/抓拍里很多不是正脸
        _FACE_CASCADES = [c for c in cs if not c.empty()]
    return _FACE_CASCADES


def _iou(a, b):
    ax, ay, aw, ah = a
    bx, by, bw, bh = b
    x1, y1 = max(ax, bx), max(ay, by)
    x2, y2 = min(ax + aw, bx + bw), min(ay + ah, by + bh)
    if x2 <= x1 or y2 <= y1:
        return 0.0
    inter = (x2 - x1) * (y2 - y1)
    return inter / (aw * ah + bw * bh - inter)


def face_tags(path):
    """检测人脸 → 人像类标签。

    规则（脸框高度占画面高度的比例决定景别）：
    - 0 张脸         → 不打人像
    - ≥2 张脸        → 人像 + 合照
    - 1 张脸         → 人像 + 单人 + 景别（≥0.30 面部特写 / ≥0.12 半身 / 否则全身）

    正脸+侧脸两个级联都跑，重叠框按 IoU 去重（同一张脸会被两个级联各检一次）。
    """
    try:
        import cv2
        import numpy as np

        # ⛔ 不能用 cv2.imread：Windows 下**读不了中文路径**，直接返回 None，
        #    于是所有中文文件名的图都被静默跳过（IMG_7507_已裁剪.jpg 就是这样漏的）。
        buf = np.fromfile(str(path), dtype=np.uint8)
        img = cv2.imdecode(buf, cv2.IMREAD_COLOR) if buf.size else None
        if img is None:
            return []
        h, w = img.shape[:2]
        if max(h, w) > 1000:
            k = 1000 / max(h, w)
            img = cv2.resize(img, (int(w * k), int(h * k)))
            h, w = img.shape[:2]

        kept = []  # [(x, y, w, h, conf)]
        net = _yunet()
        if net is not None:
            net.setInputSize((w, h))
            _, faces = net.detect(img)
            for f in (faces if faces is not None else []):
                conf = float(f[14])
                if conf < FACE_MIN:  # 低置信基本是误检，直接丢
                    continue
                kept.append((int(f[0]), int(f[1]), int(f[2]), int(f[3]), conf))
        else:
            gray = cv2.cvtColor(img, cv2.COLOR_BGR2GRAY)
            boxes = []
            for c in _face_cascades():
                for b in c.detectMultiScale(gray, 1.1, 6, minSize=(40, 40)):
                    boxes.append(tuple(int(v) for v in b))
            for b in sorted(boxes, key=lambda b: -b[2] * b[3]):
                if all(_iou(b[:4], k2[:4]) < 0.3 for k2 in kept):
                    kept.append((b[0], b[1], b[2], b[3], 0.0))
        n = len(kept)
        if n == 0:
            return []

        # 置信度：人像取最高那张脸；合照取第二高（"≥2 张脸"由它保证）；景别用最大那张
        kept.sort(key=lambda b: -b[4])
        tags = [("人像", kept[0][4])]
        if n >= 2:
            tags.append(("合照", kept[1][4]))
        else:
            x, y, bw, bh, cf = kept[0]
            tags.append(("单人", cf))
            ratio = bh / h
            if ratio >= 0.30:
                tags.append(("面部特写", cf))
            elif ratio >= 0.12:
                tags.append(("半身", cf))
            else:
                tags.append(("全身", cf))
        return tags
    except Exception:
        return []


# ── OCR（Windows.Media.Ocr，系统自带，离线，中文可用）───────────────
_OCR_ENGINE = None


def _ocr_engine():
    """懒加载中文 OCR 引擎。系统没装中文语言包时返回 None（不报错，只是跳过）。"""
    global _OCR_ENGINE
    if _OCR_ENGINE is None:
        try:
            from winrt.windows.globalization import Language
            from winrt.windows.media.ocr import OcrEngine

            _OCR_ENGINE = OcrEngine.try_create_from_language(Language("zh-Hans-CN"))
        except Exception as e:  # noqa: BLE001
            print(f"  OCR 不可用：{e}", flush=True)
            _OCR_ENGINE = False
    return _OCR_ENGINE or None


async def _ocr_async(path):
    from winrt.windows.graphics.imaging import BitmapDecoder
    from winrt.windows.storage import FileAccessMode, StorageFile

    f = await StorageFile.get_file_from_path_async(str(path))
    stream = await f.open_async(FileAccessMode.READ)
    decoder = await BitmapDecoder.create_async(stream)
    bmp = await decoder.get_software_bitmap_async()
    engine = _ocr_engine()
    if engine is None:
        return ""
    res = await engine.recognize_async(bmp)
    return res.text or ""


def ocr_text(path):
    """返回图里的文字（失败返回空串）。

    ⚠️ 大图先缩到 2000px 内再 OCR：Windows OCR 对超大位图会直接失败，
    而且缩一半对文字识别几乎没有损失（截图/文档本来就清晰）。
    """
    try:
        im = Image.open(path)
        im.draft("RGB", (2000, 2000))
        if max(im.size) > 2000:
            im.thumbnail((2000, 2000))
        tmp = Path(tempfile.gettempdir()) / f"pv_ocr_{os.getpid()}.jpg"
        im.convert("RGB").save(tmp, "JPEG", quality=90)
        return asyncio.run(_ocr_async(tmp))
    except Exception:
        return ""


# 文字 → 标签：词表命中 + 版式特征
_TEXT_RULES = [
    ("发票", ("发票", "税额", "价税合计", "开票")),
    ("票据", ("订单号", "运单", "收据", "小票", "流水号")),
    ("表格", ("合计", "序号", "金额", "备注")),
    ("证件", ("身份证", "签发机关", "有效期")),
    ("聊天截图", ("微信", "消息", "撤回", "语音")),
    ("网页截图", ("http", "www.", "登录", "注册")),
]


# 证件 / 票据类文本的特征词：命中就说明这张图是"纸上印着字"，不是"正在做某事"
_DOC_HINTS = (
    "身份证", "签发机关", "公民身份号码", "有效期限", "出生", "住址", "签发日期",
    "价税合计", "纳税人识别号", "开户行", "账号",
)
# ⛔ 文档语境下**不采信**的一级类目：这些是"事件/场景"，不是"纸上写着什么"。
#    实锤案例（2026-10-24）：身份证上印着「出生 1995 年…」，被字面命中了「生日」，
#    于是这张证件照同时挂了 `证件` 和 `活动-生日`。
_DOC_BLOCKED_L1 = {"活动"}

# ⛔ 证件/票据类标签一旦命中，**人脸标签一律丢弃**（2026-10-24 用户确认）：
#    身份证/工牌上的头像是"卡面上的照片"，不是一张人像照 —— 实测那张身份证
#    同时挂了 `证件` + `人像/单人/半身`（人脸置信度 0.94，来自证件照本身）。
#    ⚠️ 没开 `--ocr` 时拿不到这条线索，抑制不会生效。
_DOC_TAGS = frozenset({"证件", "票据", "发票", "银行卡"})


def ocr_tags(text, terms, size=None, term_l1=None):
    """图里的文字 → 标签。只保留**确实出现**的词，不猜。

    ⚠️ Windows 中文 OCR 会在每个汉字之间插空格（`发 票 价 税 合 计`），
    所以匹配前必须**去掉所有空白**，否则「上海」永远匹配不上。

    `term_l1` = {词条: 一级}。给了它就能做**文档语境抑制**：
    证件上的「出生」不等于「生日」，事件类（活动组）的词一概不采信。
    """
    if not text or len(text.strip()) < 6:
        return []
    flat = "".join(text.split())
    tags = []
    for name, keys in _TEXT_RULES:
        if any(k in flat for k in keys):
            tags.append(name)
    # 词表命中（只算 2 字以上的词，避免「花」「塔」这种单字到处误中）
    low = flat.lower()
    is_doc = any(k in flat for k in _DOC_HINTS)
    for t in terms:
        if len(t) < 2 or t not in low or t in tags:
            continue
        if is_doc and term_l1 and term_l1.get(t) in _DOC_BLOCKED_L1:
            continue
        tags.append(t)
    # ⛔ 不要再加「文字多 = 截图」的兜底（2026-10-23 砍掉）：
    #    竖版照片 1256×1777（A4 比例）带一段文字就会中招，实测 286 张里误伤 2 张。
    #    「截图」现在只由 `rule_tags()` 按**文件名**判定。
    return tags[:4]


# ── 文件属性类：**不走 CLIP**，只看文件名 ────────────────────────────────
#
# 为什么（2026-10-23 实测）：CLIP 对「截图」这个词在**普通照片**上也能给 0.38~0.41，
# 297 张里 147 张被判成截图，分数全挤在阈值上下（0.38:48 0.39:72 0.40:21 0.41:6），
# 调阈值只能靠硬砍。真正可靠的判据只有**文件名**（Screenshot_ / 截屏 / 扫描 …），
# 比例和文字密度都试过，误伤比命中多（见 `rule_tags` 的注释）。
RULE_L1 = {"截图", "文档"}

# 文件名里像截图的
_RE_SHOT = re.compile(
    r"(screenshot|screen[ _-]?shot|snipaste|截屏|截图|屏幕截图|微信截图|qq截图)", re.I
)
# 文件名里像文档/票证的
_RE_DOC = re.compile(
    r"(scan|scanned|scan\d|发票|票据|receipt|invoice|证件|银行卡|营业执照)", re.I
)


def rule_tags(path, size, text=""):
    """截图 / 文档 的规则判定，返回 `[(标签名, None)]`。

    ⭐ 置信度一律 `None`：这是**规则**判出来的，不是模型打分，
       界面上不显示小数，免得跟 CLIP 的相似度混为一谈。

    ## 为什么只看文件名（2026-10-23 试过又砍掉的两条）
    - 「A4 纸比例 1.25~1.55 + 文字 ≥60」→ 4:3 / 3:2 的普通照片全中招（1.33 / 1.5 都在区间里），
      一次跑出 6 张假「文档」（猫、雕塑、夜景都算）。
    - 「细长比例 ≥1.9 + 有文字」→ 全景照（3028×1048）被当成手机长截图，
      而且这类图的 OCR 结果本身是乱码（27~38 个噪声字符），文字量根本不可信。
    → 文件名是最可靠的一条：手机/系统截图一定有 `Screenshot_`、`截屏` 这类前缀。
      「发票 / 证件 / 银行卡」这种靠 **OCR 字面命中**（source=ocr）已经能捞到，不必猜。
    """
    name = Path(path).name
    out = []
    if _RE_SHOT.search(name):
        out.append(("截图", None))
    if _RE_DOC.search(name):
        out.append(("文档", None))
    return out


class Scorer:
    """一次加载模型 + 文本塔，之后可以对任意图片反复打分。

    ⭐ 抽出来是为了**只有一条打分链路**：之前 `score_one.py` 自己拼了一套
    （模型→文本→图像→相似度），结果和主流程对不上（同一张图 0.3891 vs 0.4088）。
    两边共用一个类，就不会再出现"工具算的和库里不一样"。
    """

    def __init__(self, model_dir=None, device=None):
        self.device = device or ("cuda" if torch.cuda.is_available() else "cpu")
        self.model_dir = pick_model_dir(model_dir)
        self.model = ChineseCLIPModel.from_pretrained(
            self.model_dir, torch_dtype=torch.float16 if self.device == "cuda" else torch.float32
        ).to(self.device).eval()
        self.proc = ChineseCLIPProcessor.from_pretrained(self.model_dir)

        # 词表：与主流程完全一致（含 UI 里禁用的词）
        cfg = db_settings()
        disabled = set(json.loads(cfg.get("ai.disabled_terms", "[]")) or [])
        self.vocab = [v for v in load_vocab() if v["term"] not in disabled]

        self.prompts, self.owner = build_prompts(self.vocab)
        self.l1s, l1_prompts, l1_owner = build_l1_prompts(self.vocab)

        with torch.no_grad():
            self.tf = text_matrix(self.model, self.proc, self.prompts, self.device).float().cpu()
            l1_raw = text_matrix(self.model, self.proc, l1_prompts, self.device).float().cpu()
            l1_mat = torch.stack(
                [l1_raw[torch.tensor([o == i for o in l1_owner])].mean(0) for i in range(len(self.l1s))]
            )
            self.l1_mat = l1_mat / l1_mat.norm(dim=-1, keepdim=True)

        self.owner_arr = np.array(self.owner)
        self.l1_index = {name: i for i, name in enumerate(self.l1s)}
        self.term_l1 = np.array([self.l1_index[v["l1"]] for v in self.vocab])
        self.term_group = {v["term"]: v.get("group", "") for v in self.vocab if v.get("group")}
        self.term_to_l1 = {v["term"]: v["l1"] for v in self.vocab}

    def score_paths(self, paths):
        """返回 (sims[N,P], l1_sims[N,C], keep_idx)。"""
        imf, keep = image_features(self.model, self.proc, paths, self.device)
        sims = (imf @ self.tf.T).numpy()
        l1_sims = (imf @ self.l1_mat.T).numpy()
        return sims, l1_sims, keep


def db_images(library_name):
    con = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
    con.row_factory = sqlite3.Row
    rows = con.execute("""
        SELECT i.id, i.rel_name, f.rel_path, v.last_mount_path,
               e.make, e.model, e.lens, p.place
          FROM images i
          JOIN folders f ON f.id = i.dir_id
          JOIN volumes v ON v.id = f.volume_id
          JOIN library_folders lf ON lf.folder_id = f.id
          JOIN libraries l ON l.id = lf.library_id
          LEFT JOIN image_exif e ON e.image_id = i.id
          LEFT JOIN image_places p ON p.image_id = i.id
         WHERE l.name = ?
         ORDER BY i.id
    """, (library_name,)).fetchall()
    con.close()
    out = []
    for r in rows:
        # ⚠️ 不能写成 Path("C:") / "Users/..." —— 那会变成「C 盘当前目录」的相对路径
        mount = (r["last_mount_path"] or "").rstrip("\\/") + "\\"
        p = Path(mount + r["rel_path"].replace("/", "\\") + "\\" + r["rel_name"])
        out.append({
            "id": r["id"], "path": p, "name": r["rel_name"],
            "make": r["make"], "model": r["model"], "lens": r["lens"], "place": r["place"],
        })
    return [x for x in out if x["path"].is_file()]


def exif_tags(it):
    """EXIF 标签只保留**相机机型**和**地点**（2026-10-21 用户定）。

    ⛔ 不要再加品牌（`make`）和镜头（`lens`）：
    - 品牌和机型重复（`Apple` + `iPhone 15 Pro Max` 等于说两遍）；
    - 镜头是 `Apple iPhone 15 Pro Max back triple camera 6.86mm f/1.78` 这种长串，
      既难读又把菜单撑爆，对找图没有帮助。
    """
    tags = []
    if it["model"]:
        tags.append(it["model"])
    if it["place"]:
        tags.append(it["place"])
    return tags


def clear_ai_tags(image_ids=None):
    """清掉旧的 AI / EXIF / OCR / 人脸标签。

    ⚠️ 传 image_ids 时**只清这些图**（按图库作用域），否则多图库分批跑会互相清空。
    """
    con = sqlite3.connect(DB)
    ids = None if image_ids is None else list(image_ids)
    if ids is None:
        con.execute("DELETE FROM image_tags WHERE source IN ('ai','exif','ocr','face')")
    else:
        for i in range(0, len(ids), 500):
            chunk = ids[i:i + 500]
            q = ",".join("?" * len(chunk))
            con.execute(
                f"DELETE FROM image_tags WHERE source IN ('ai','exif','ocr','face') "
                f"AND image_id IN ({q})", chunk)
    con.execute("DELETE FROM tags WHERE id NOT IN (SELECT tag_id FROM image_tags)")
    con.commit()
    con.close()
    scope = "全部" if ids is None else f"{len(ids)} 张图"
    print(f"已清空旧的 AI / EXIF 标签（{scope}）")


def write_db(results, source_visual="ai", source_exif="exif"):
    con = sqlite3.connect(DB)
    cur = con.cursor()
    n_tag = n_link = 0
    for r in results:
        rows = (
            [(t, None, source_exif) for t in r["exif"]]
            + [(t, None, "ocr") for t in (r.get("ocr") or [])]
            # 截图 / 文档：规则判定，置信度为空（不是模型分，界面上不显示小数）
            + [(t, c, "ai") for t, c in (r.get("rule") or [])]
            + [(t, c, "face") for t, c in (r.get("face") or [])]
            + [(t, c, source_visual) for t, c in r["visual"]]
            # 一级标签：名字就是一级名，分数是一级相似度
            + [(t, c, source_visual) for t, c in (r.get("l1_scored") or [])]
        )
        for name, conf, src in rows:
            cur.execute("INSERT OR IGNORE INTO tags(name, kind) VALUES (?, ?)", (name, src))
            tag_id = cur.execute("SELECT id FROM tags WHERE name = ?", (name,)).fetchone()[0]
            n_tag += 1
            cur.execute(
                "INSERT OR REPLACE INTO image_tags(image_id, tag_id, source, confidence) VALUES (?,?,?,?)",
                (r["id"], tag_id, src, None if conf is None else round(float(conf), 2)),
            )
            n_link += 1
    con.commit()
    con.close()
    print(f"写库：{n_tag} 条挂载（{n_link} 次）")
    return n_link


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--library", default="测试图库")
    ap.add_argument("--model", default=None, help="模型目录（默认自动挑 L/14）")
    # ⭐ 2026-10-22：`--max-tags` 是**上限**不是目标 —— 不再"凑满 8 个"，
    # 够格（过阈值）几个就几个，宁少勿错（手机厂商也是这么做的）。
    ap.add_argument("--max-tags", type=int, default=None)
    ap.add_argument("--ocr", action="store_true", help="用 Windows 系统 OCR 给含文字的图打标签")
    ap.add_argument("--no-faces", action="store_true", help="关闭人脸检测（默认开启）")
    ap.add_argument("--threshold", type=float, default=None)
    ap.add_argument("--window", type=float, default=0.08, help="相对最佳分的最大差距（尾部砍掉）")
    ap.add_argument("--l1-bonus", type=float, default=0.02, help="类目命中给词条的加成")
    ap.add_argument("--l1-threshold", type=float, default=None, help="一级类目的准入门槛")
    ap.add_argument(
        "--rules",
        choices=["simple", "strict"],
        default="simple",
        help="simple=一级≥l1-threshold 全放行（上一版）；strict=四级规则（一级≥0.40 或 >0.36 且有二级）",
    )
    ap.add_argument("--write", action="store_true", help="写进索引库")
    ap.add_argument("--dry", action="store_true", help="只看结果，不写库")
    ap.add_argument("--limit", type=int, default=0, help="只跑前 N 张（调试用）")
    ap.add_argument("--reset", action="store_true", help="先清掉旧的 ai/exif 标签再写")
    args = ap.parse_args()

    device = "cuda" if torch.cuda.is_available() else "cpu"
    model_dir = pick_model_dir(args.model)
    t0 = time.time()
    print(f"模型：{model_dir.name}")
    model = ChineseCLIPModel.from_pretrained(
        model_dir, torch_dtype=torch.float16 if device == "cuda" else torch.float32
    )
    model = model.to(device).eval()
    proc = ChineseCLIPProcessor.from_pretrained(model_dir)
    print(f"加载完成 {time.time() - t0:.1f}s｜device={device}")

    cfg = db_settings()
    disabled = set(json.loads(cfg.get("ai.disabled_terms", "[]")) or [])
    vocab_all = [v for v in load_vocab() if v["term"] not in disabled]
    # ⛔ 截图 / 文档 这两类**不交给 CLIP**（见 `RULE_L1` 的注释），
    #    但它们仍参与 OCR 字面命中 —— 「发票」「银行卡」出现在发票图上才是最强证据。
    vocab = [v for v in vocab_all if v["l1"] not in RULE_L1]
    if disabled:
        print(f"按 UI 设置停用 {len(disabled)} 个词条")

    # 命令行没显式指定时，跟随 UI 里保存的阈值 / 每张最多
    if cfg.get("ai.threshold") and args.threshold is None:
        args.threshold = float(cfg["ai.threshold"])
    # 一级类目门槛：UI 里单独可调（比二级宽松，粗分类更容易命中）
    if cfg.get("ai.l1Threshold") and args.l1_threshold is None:
        args.l1_threshold = float(cfg["ai.l1Threshold"])
    if cfg.get("ai.max_tags") and args.max_tags is None:
        args.max_tags = int(cfg["ai.max_tags"])
    # ⚠️ UI 里没存过配置 / 命令行也没给 → 必须有兜底，否则 None 会一路传到比较运算里
    if args.threshold is None:
        # 取消"凑满"之后，阈值就是**唯一**的准入门槛，必须更严
        args.threshold = 0.40
    if args.max_tags is None:
        args.max_tags = 6
    if args.l1_threshold is None:
        args.l1_threshold = 0.36 if args.rules == "simple" else args.threshold
    over = " ".join(f"{k}={v}" for k, v in L1_THRESHOLD_OVERRIDE.items())
    print(
        f"参数：二级 {args.threshold} · 一级 {args.l1_threshold}"
        + (f"（另：{over}）" if over else "")
        + f" · 每张最多 {args.max_tags}"
    )
    prompts, owner = build_prompts(vocab)
    print(f"词表 {len(vocab)} 条 → {len(prompts)} 条 prompt")

    t = time.time()
    tf = text_matrix(model, proc, prompts, device).float().cpu()
    l1s, l1_prompts, l1_owner = build_l1_prompts(vocab)
    l1_raw = text_matrix(model, proc, l1_prompts, device).float().cpu()
    # 每个一级类目取它所有 prompt 的均值，再归一化
    l1_mat = torch.stack([l1_raw[torch.tensor([o == i for o in l1_owner])].mean(0) for i in range(len(l1s))])
    l1_mat = l1_mat / l1_mat.norm(dim=-1, keepdim=True)
    print(f"文本塔 {time.time() - t:.1f}s（词条 {len(prompts)} + 类目 {len(l1_prompts)} 条 prompt）")

    items = db_images(args.library)
    print(f"图库「{args.library}」：{len(items)} 张（存在且可读）")
    if args.limit:
        items = items[: args.limit]
    if not items:
        sys.exit(1)

    t = time.time()
    imf, keep = image_features(model, proc, [it["path"] for it in items], device)
    items = [items[j] for j in keep]  # 跳过读不出的
    dt = time.time() - t
    print(f"图像塔 {dt:.1f}s（{dt / max(len(items), 1) * 1000:.0f} ms/张，{len(items)} 张）")

    sims = (imf @ tf.T).numpy()  # [N, P]
    owner_arr = np.array(owner)
    l1_sims = (imf @ l1_mat.T).numpy()  # [N, C]
    l1_index = {name: i for i, name in enumerate(l1s)}
    term_l1 = np.array([l1_index[v["l1"]] for v in vocab])

    # 调试：PV_DEBUG_IMG=文件名 PV_DEBUG_TERM=词  时打印该图该词的原始分（临时排查用）
    _dbg_img = os.environ.get("PV_DEBUG_IMG")
    _dbg_term = os.environ.get("PV_DEBUG_TERM")
    if _dbg_img and _dbg_term:
        for _i, _it in enumerate(items):
            if _it["name"] == _dbg_img:
                for _j, _v in enumerate(vocab):
                    if _v["term"] == _dbg_term:
                        _val = float(sims[_i][owner_arr == _j].max())
                        print(f"[DEBUG] {_dbg_img} / {_dbg_term} = {_val:.4f}（第 {_i} 行）")

    terms_for_ocr = [v["term"] for v in vocab_all]
    # OCR 用全量词表（含 截图/文档 组），所以映射也要用全量
    term_to_l1_all = {v["term"]: v["l1"] for v in vocab_all}
    # 词条 → 一级（组合规则要判断"某个一级下有没有命中"）
    term_to_l1 = {v["term"]: v["l1"] for v in vocab}
    l1_names = {v["l1"] for v in vocab}
    results = []
    t_ocr = time.time()
    for i, it in enumerate(items):
        excl_dropped = []
        # 二级原始分：同一词条的多条 prompt 取 max（同义词不该互相稀释）
        # ⛔ 这个 raw 是**唯一可信的分数**：写库、显示、比阈值都用它。
        raw = np.array([sims[i][owner_arr == j].max() for j in range(len(vocab))])
        score = raw.copy()

        # ── 规则①：先匹配一级，再在命中的一级里挑二级 ──
        # 一级只有 15 个候选，CLIP 在这一层准得多；命中类目下的二级给一点加成，
        # 让"同类目内的词"排在"跨类目的偶发高分"前面。
        l1_row = l1_sims[i]
        # ⚠️ 按类目取阈值：`L1_THRESHOLD_OVERRIDE` 里列出的类目用自己的线
        l1_idx = [
            k for k, name in enumerate(l1s)
            if l1_row[k] >= l1_threshold(name, args.l1_threshold)
        ]
        l1_hit = {l1s[k] for k in l1_idx}
        # ⭐ 一级也当标签写库（带自己的置信度）——用户 2026-10-22 要求
        # 一级的置信度 = max(一级自身相似度, 该组里最高的二级分)
        # 理由：一级名（"自然"/"活动"）比二级词更抽象，CLIP 给的分普遍偏低，
        # 只按自身分数会导致"二级命中了、一级却没分"。取子项最高分能反映"这一类里有东西"。
        # ⚠️ 必须是**全部** 15 个一级的原始分（不只是过线的），
        # 否则没自己过线的一级会拿合并值冒充"原始分"，前台就看不到真实情况了
        l1_own = {name: float(l1_row[k]) for k, name in enumerate(l1s)}
        if l1_hit:
            score = score + np.array(
                [args.l1_bonus if l1s[term_l1[j]] in l1_hit else 0.0 for j in range(len(vocab))]
            )

        # ⚠️ 阈值/展示一律用 raw；score 里带 +0.02 类目加成，**只用于排序**
        idx = np.argsort(-score)
        best = float(raw[idx[0]]) if len(idx) else 0.0
        floor = max(args.threshold, best - args.window)
        picked = []
        for j in idx:
            if len(picked) >= args.max_tags:
                break
            s_j = float(raw[j])
            if s_j < floor:
                break
            picked.append((vocab[j]["term"], s_j))

        # ── 人像类改由人脸检测产出，CLIP 的结果一律丢掉 ──
        picked = [(t, c) for t, c in picked if t not in PORTRAIT_TERMS]
        # ⚠️ 人脸要**提前**算：下面的「博物馆 + 人群 → 旅游」要用人脸判定有没有人群
        face_out = [] if args.no_faces else face_tags(it["path"])

        # ── 互斥组：同一组只留分数最高的一个 ──
        # 例：「单人 0.4057 / 合照 0.4103」只差 0.0046，模型分不出画面里有几个人，
        # 但这两个概念互斥，同时出现必然有一个是错的 —— 用规则砍掉低分那个。
        if any(v["group"] for v in vocab):
            group_of = {v["term"]: v["group"] for v in vocab if v["group"]}
            seen: dict[str, str] = {}
            kept = []
            dropped = []
            for name, sc in picked:  # picked 已按分数降序
                g = group_of.get(name)
                if g and g in seen:
                    dropped.append((name, sc, seen[g]))
                    continue
                if g:
                    seen[g] = name
                kept.append((name, sc))
            picked = kept
            excl_dropped = [(n, round(c, 3), w) for n, c, w in dropped]

        # ── 场景内外对立：两派都有，砍掉分数低的那一派 ──
        for gname, side_a, side_b in OPPOSITE_PAIRS:
            hit_a = [(n, c) for n, c in picked if n in side_a]
            hit_b = [(n, c) for n, c in picked if n in side_b]
            if not (hit_a and hit_b):
                continue
            # 平手时留室外（自然景物比"室内"这种推断更可信）
            if max(c for _, c in hit_a) > max(c for _, c in hit_b):
                lose, win_at = hit_b, "室内"
            else:
                lose, win_at = hit_a, "室外"
            lose_names = {n for n, _ in lose}
            picked = [(n, c) for n, c in picked if n not in lose_names]
            excl_dropped += [(n, round(c, 3), f"{gname}·{win_at}") for n, c in lose]

        # ── 组合规则：博物馆 / 自然 + 人 → 旅游 ──
        # `need` 可以是一级类目名（自然），也可以是具体词条（博物馆）
        has_crowd = any(t == "合照" for t, _ in face_out)
        has_people = bool(face_out)
        derived = []
        for need, cond, out_tag in COMPOSITE_RULES:
            if out_tag in {n for n, _ in picked}:
                continue
            if cond == "crowd" and not has_crowd:
                continue
            if cond == "people" and not has_people:
                continue
            if need in l1_names:
                hit = [(n, c) for n, c in picked if term_to_l1.get(n) == need]
            else:
                hit = [(n, c) for n, c in picked if n == need]
            if not hit:
                continue
            # 置信度沿用触发侧的**最高分**（同量纲），不是人脸置信度
            score = max(c for _, c in hit)
            picked.append((out_tag, score))
            derived.append((out_tag, round(score, 3), need))

        # ── 规则②：二级命中，就把它的一级也算进来 ──
        # 即使一级本身没过阈值（比如「沙滩」命中而「自然」没到线），
        # 也把「自然」归到这张图的一级里 —— 一级只是分组，不该丢。
        l1_out = list(l1_hit) + [term_to_l1[n] for n, _ in picked]
        kept_names = list(dict.fromkeys(l1_out))

        # 一级有两套分数：
        # - `l1_merged`：max(自身, 组内最高二级) —— **只用于排序/入选判断**
        # - `l1_scored`：一级**自身**的相似度 —— **写库、前台显示的就是这个原始值**
        #   用户 2026-10-22 明确要求前台看原始分数，别用合并值糊弄。
        # ── 一级 / 二级 生效规则（用户 2026-10-23 定）────────────────
        # ① L1 > 0.36 且 L2 ≥ 0.41 → 一级和二级都生效（"都显示"由本段 + picked 共同决定）
        # ② L1 ≥ 0.40 且 L2 < 0.41 → 只一级
        # ③ L2 ≥ 0.41 且 L1 ≤ 0.36 → 只二级
        # ④ 其余 → 都不生效
        # 合并成两条判据：
        #   二级：L2 ≥ L2_MIN（0.41）就走 picked（见上方阈值）
        #   一级：L1 ≥ L1_STRONG(0.40)，或者 L1 > L1_WEAK(0.36) 且该一级下有二级过线
        has_child = {term_to_l1[n] for n, _ in picked} if picked else set()
        if args.rules == "strict":
            keep_l1 = [
                (name, own)
                for name, own in l1_own.items()
                if own >= L1_STRONG or (own > L1_WEAK and name in has_child)
            ]
        else:
            # 上一版：一级只要 ≥ 该类目的阈值就放行（不看有没有二级）
            keep_l1 = [
                (name, own)
                for name, own in l1_own.items()
                if own >= l1_threshold(name, args.l1_threshold) or name in has_child
            ]
        # 一级「人像」也由人脸检测决定，CLIP 的不算
        l1_scored = sorted([x for x in keep_l1 if x[0] != "人像"], key=lambda x: -x[1])
        # ── 规则③：一个都没匹配上 → 明确标记，UI 显示「无匹配」 ──
        no_match = len(picked) == 0
        # OCR：只在有文字时产出标签，且与视觉标签去重
        ocr_out = []
        # 尺寸先取：截图/文档规则要看宽高比（拿不到就当归零，规则自然不触发）
        try:
            _w, _h = Image.open(it["path"]).size
        except Exception:
            _w = _h = 0
        text = ""
        if args.ocr:
            text = ocr_text(it["path"])
            ocr_out = [
                t
                for t in ocr_tags(text, terms_for_ocr, (_w, _h), term_l1=term_to_l1_all)
                if t not in [n for n, _ in picked]
            ]
            if text:
                picked = picked[: max(0, args.max_tags - len(ocr_out))]

        # 证件/票据上的头像是卡面照片，不算人像 —— 人脸标签整组丢掉
        if any(t in _DOC_TAGS for t in ocr_out):
            face_out = []

        # 截图 / 文档：不看 CLIP，只看文件名、屏幕比例、OCR 文字
        rule_out = [
            (n, c) for n, c in rule_tags(it["path"], (_w, _h), text)
            if n not in [x for x, _ in picked] and n not in ocr_out
        ]

        results.append({
            "id": it["id"], "name": it["name"], "place": it["place"],
            "make": it["make"], "model": it["model"], "lens": it["lens"],
            "visual": picked, "ocr": ocr_out, "face": face_out, "exif": exif_tags(it),
            "rule": rule_out,
            "derived": derived,
            "l1": kept_names, "l1_scored": l1_scored, "no_match": no_match,
            "excl_dropped": excl_dropped,
        })

    if args.ocr:
        print(f"OCR {time.time() - t_ocr:.1f}s（{len(items)} 张）")

    # 打印结果
    for r in results:
        l1_str = " ".join(f"{t}({c:.2f})" for t, c in (r.get("l1_scored") or []))
        vis = " ".join(f"{t}({c:.2f})" for t, c in r["visual"])
        if l1_str:
            vis = f"[{l1_str}] " + vis
        rule_str = " ".join(t for t, _ in (r.get("rule") or []))
        if rule_str:
            vis = (vis + " " if vis else "") + f"<规则: {rule_str}>"
        d_str = " ".join(f"{t}({c:.2f})←{need}" for t, c, need in (r.get("derived") or []))
        if d_str:
            vis = (vis + " " if vis else "") + f"<组合: {d_str}>"
        print(f"\n{r['name']}\n  EXIF: {' / '.join(r['exif']) or '—'}\n  视觉: {vis or '—'}")

    lens = [len(r["visual"]) for r in results]
    print(f"\n统计：平均 {np.mean(lens):.1f} 个视觉标签，最少 {min(lens)}，最多 {max(lens)}")
    hist = {}
    for j in np.argsort(-sims.max(axis=1))[:8]:
        pass
    top_scores = np.sort(sims, axis=1)[:, ::-1][:, : args.max_tags].ravel()
    print(f"Top-K 分数分位：P10={np.percentile(top_scores, 10):.3f} "
          f"P50={np.percentile(top_scores, 50):.3f} P90={np.percentile(top_scores, 90):.3f}")

    (ROOT / "assets" / "ai" / "tag_result.json").write_text(
        json.dumps(results, ensure_ascii=False, indent=1), encoding="utf-8")
    print("明细：assets/ai/tag_result.json")

    if args.write:
        if args.reset:
            clear_ai_tags([r["id"] for r in results])
        write_db(results)
    else:
        print("（未写库，加 --write 才写入）")


if __name__ == "__main__":
    main()
