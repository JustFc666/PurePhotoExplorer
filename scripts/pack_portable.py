r"""打一个「绿色版」zip：解压即用，数据跟着文件夹走。

## 和安装版的区别
- 不写注册表、不建开始菜单项；
- exe 同级多一个空的 `portable` 文件 → `default_data_dir()` 切到 `.\data`，
  索引库、缩略图缓存、人脸向量全在这里，整个文件夹拷走就是搬家；
- AI 模型、DirectML.dll、7z 都放在旁边，不用另外装。

## 用法
    python scripts/pack_portable.py

先 `cargo build --release -p photovault`（或 tauri build）拿到新 exe，再跑这个。
产物在 `release/PurePhotoExplorer_<版本>_win64_portable.zip`。
"""

from __future__ import annotations

import shutil
import sys
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
EXE = ROOT / "target" / "release" / "photovault.exe"
DLL = ROOT / "target" / "release" / "DirectML.dll"
MODELS = ROOT / "assets" / "ai"
SIDE_7Z = ROOT / "release" / "7z.exe"
SIDE_7Z_DLL = ROOT / "release" / "7z.dll"
TAURI_CONF = ROOT / "apps" / "desktop" / "src-tauri" / "tauri.conf.json"
OUT_DIR = ROOT / "release"

# 只带**真被代码读到**的模型文件。`res10.caffemodel` / `deploy.prototxt` 是旧
# OpenCV DNN 方案的遗留，全仓库已无引用，塞进来只会让包大 10MB。
MODEL_FILES = [
    ("tag/clip_vision.onnx", "assets/ai/tag/clip_vision.onnx"),
    ("tag/tag_meta.json", "assets/ai/tag/tag_meta.json"),
    ("tag/text_embeds.bin", "assets/ai/tag/text_embeds.bin"),
    ("face/yunet.onnx", "assets/ai/face/yunet.onnx"),
    ("face/sface.onnx", "assets/ai/face/sface.onnx"),
]

README = """PurePhotoExplorer {ver} · 绿色版（免安装）
========================================

## 怎么用
双击 PurePhotoExplorer.exe 即可，不需要安装。

## 数据在哪
就在这个文件夹的 data\\ 目录里：
  data\\library.db     索引（图库、标签、人脸都在里面）
  data\\thumbs\\       缩略图缓存（删掉会自动重建，只是慢一点）

想换电脑？把**整个文件夹**拷到 U 盘或另一台机器，打开就是原样。
（之所以能做到这点，是因为 exe 旁边这个叫 portable 的空文件 ——
 删掉它，程序就回到「数据存在 %LOCALAPPDATA%\\PurePhotoExplorer」的安装版行为。）

## 第一次用
1. 左边展开「我的电脑」，找到放照片的文件夹；
2. 右键它 →「加入图库」，等底部进度走完；
3. 点左边的图库名，照片就都在了。
顶栏「设置」右边的「帮助」里有完整说明。

## AI 识别
模型就在 assets\\ai\\ 里（约 200MB），不需要联网、不需要另外下载。
第一次跑会加载十几秒，之后每张约 0.4 秒。

## 压缩包里的图
支持 zip / cbz / 7z / rar / cbr / cb7。7z.exe 已经在文件夹里，
所以即使这台电脑没装 7-Zip 也能打开 RAR。

## 已知限制
- 不注册「打开方式」，想双击图片直接打开，请用安装版；
- 便携模式下不要把这个文件夹放在只读位置（U 盘写保护），否则建不了库。
"""


def version() -> str:
    import json

    return json.loads(TAURI_CONF.read_text(encoding="utf-8"))["version"]


def need(p: Path, what: str) -> Path:
    if not p.exists():
        sys.exit(f"缺少 {what}：{p}\n先跑 cargo build --release -p photovault，或确认 assets/ai 里的模型还在。")
    return p


def main() -> None:
    ver = version()
    need(EXE, "release 版 exe")
    need(DLL, "DirectML.dll")
    for src, _ in MODEL_FILES:
        need(MODELS / src, f"模型 {src}")
    need(SIDE_7Z, "7z.exe（在 release/ 下）")

    stage = ROOT / "Temp" / "portable-stage" / "PurePhotoExplorer"
    if stage.parent.exists():
        shutil.rmtree(stage.parent)
    stage.mkdir(parents=True)

    # exe 改名：用户看到的是产品名，不是内部的 photovault
    shutil.copy2(EXE, stage / "PurePhotoExplorer.exe")
    shutil.copy2(DLL, stage / "DirectML.dll")
    shutil.copy2(SIDE_7Z, stage / "7z.exe")
    if SIDE_7Z_DLL.exists():
        shutil.copy2(SIDE_7Z_DLL, stage / "7z.dll")

    for src, dst in MODEL_FILES:
        target = stage / dst
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(MODELS / src, target)

    # 便携标记：空文件即可（`portable_data_dir_from` 判的是 is_file）
    (stage / "portable").write_bytes(b"")
    (stage / "使用说明.txt").write_text(README.format(ver=ver), encoding="utf-8-sig")

    out = OUT_DIR / (sys.argv[1] if len(sys.argv) > 1 else f"PurePhotoExplorer_{ver}_win64_portable.zip")
    if out.exists():
        # 旧包常被 7-Zip / 资源管理器占用，删不掉就换个名字，别整个打包流程失败
        try:
            out.unlink()
        except OSError as e:
            sys.exit(f"{out.name} 正被其他程序占用（{e}），先关掉 7-Zip / 资源管理器，或换个名字：\n    python scripts/pack_portable.py 另一个名字.zip")

    total = 0
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED, compresslevel=1) as z:
        for f in sorted(stage.rglob("*")):
            if f.is_file():
                z.write(f, f.relative_to(stage.parent))
                total += f.stat().st_size

    shutil.rmtree(stage.parent)
    print(f"写出 release/{out.name}")
    print(f"  原始 {total / 1024 / 1024:.0f} MB → 压缩后 {out.stat().st_size / 1024 / 1024:.0f} MB")


if __name__ == "__main__":
    main()
