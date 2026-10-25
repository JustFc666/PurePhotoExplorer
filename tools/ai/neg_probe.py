"""负向提示词分离度探针：动物/宠物组，看「雕塑/模型」类图能否被 neg 压下去。"""
import sys, os, sqlite3
sys.path.insert(0, 'tools/ai')
import numpy as np, torch
import auto_tag as A

NEG = ["一种{}的图案", "一个{}的模型", "一幅{}的画", "{}的雕像", "{}的玩具", "卡通{}"]

vocab = A.load_vocab()
animals = [v["term"] for v in vocab if v["l1"] in ("动物", "宠物")]
print("动物/宠物词条：", animals)

md = A.pick_model_dir(None)
model = A.ChineseCLIPModel.from_pretrained(md, torch_dtype=torch.float32).to("cpu").eval()
proc = A.ChineseCLIPProcessor.from_pretrained(md)


def mat(prompts):
    t = A.text_matrix(model, proc, prompts, "cpu").float()
    return t / t.norm(dim=-1, keepdim=True)


pos_p, pos_owner = [], []
for j, w in enumerate(animals):
    for tpl in A.TEMPLATES:
        pos_p.append(tpl.format(w)); pos_owner.append(j)
pos_owner = np.array(pos_owner)

neg_p, neg_owner = [], []
for j, w in enumerate(animals):
    for tpl in NEG:
        neg_p.append(tpl.format(w)); neg_owner.append(j)
neg_owner = np.array(neg_owner)

P = mat(pos_p); N = mat(neg_p)
items = A.db_images("测试图库")
imf, keep = A.image_features(model, proc, [it["path"] for it in items], "cpu")
items = [items[j] for j in keep]
S = (imf @ P.T).numpy(); T = (imf @ N.T).numpy()
pos = np.array([[S[i][pos_owner == j].max() for j in range(len(animals))] for i in range(len(items))])
neg = np.array([[T[i][neg_owner == j].max() for j in range(len(animals))] for i in range(len(items))])

con = sqlite3.connect(f"file:{A.DB}?mode=ro", uri=True)
tagmap = {}
for iid, tname in con.execute(
    "select it.image_id, t.name from image_tags it join tags t on t.id=it.tag_id where it.source='ai'"
):
    tagmap.setdefault(iid, set()).add(tname)

ART = {"雕塑", "文物", "绘画", "博物馆"}
animal_set = set(animals)
rows = []
for i, it in enumerate(items):
    j = int(np.argmax(pos[i]))
    tags = tagmap.get(it["id"], set())
    kind = "艺术" if tags & ART else ("动物" if tags & animal_set else "其他")
    rows.append((kind, animals[j], pos[i][j], neg[i][j], it["name"]))

for kind in ["艺术", "动物"]:
    sel = [r for r in rows if r[0] == kind]
    if not sel:
        continue
    d = [r[2] - r[3] for r in sel]
    print(f"\n{kind}（{len(sel)} 张）：pos-neg 均值 {np.mean(d):+.3f} "
          f"P10 {np.percentile(d, 10):+.3f} P50 {np.percentile(d, 50):+.3f} P90 {np.percentile(d, 90):+.3f}")
    for r in sel[:8]:
        print(f"   {r[1]:<4} pos={r[2]:.3f} neg={r[3]:.3f} Δ={r[2] - r[3]:+.3f}  {r[4][:40]}")

worst = sorted(rows, key=lambda r: r[2] - r[3])[:10]
print("\n全库最容易被 neg 压掉的 10 张（Δ 最小）：")
for r in worst:
    print(f"   [{r[0]}] {r[1]:<4} pos={r[2]:.3f} neg={r[3]:.3f} Δ={r[2]-r[3]:+.3f}  {r[4][:40]}")
print("\n全库 Δ 分位：", " ".join(f"P{p}={np.percentile([r[2]-r[3] for r in rows], p):+.3f}" for p in (10, 25, 50, 75, 90)))
