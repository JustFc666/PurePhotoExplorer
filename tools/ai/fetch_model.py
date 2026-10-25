#!/usr/bin/env python
# -*- coding: utf-8 -*-
"""分块并行下载大文件（hf-mirror 专用兜底）。

## 为什么不用 aria2
hf-mirror 的大文件用 aria2 下到最后会**卡在收尾**：文件 size 已经到了，
但 `.aria2` 控制文件不消失，而且内部有空洞（`torch.load` 报
`failed reading file data/4: invalid header or archive is corrupted`）。
单流 curl 只有 ~400KB/s，3.8GB 要两小时。

## 做法
按字节区间切成 N 块，每块一个线程、**独立重试 + 独立断点续传**，
写完再拼接。任何一块失败只重下那一块，不会污染别的块。

用法：
    python tools/ai/fetch_model.py <url> <out_path> [--threads 8] [--verify-torch]
"""

import argparse
import os
import ssl
import sys
import threading
import time
import urllib.request

# hf-mirror 的证书在这台机器上校验不过（certificate has expired）；
# 完整性靠下载后的 --verify-torch / sha256 兜底。
SSL_CTX = ssl._create_unverified_context()

CHUNK = 8 * 1024 * 1024  # 单次读取 8MB


def head_size(url):
    req = urllib.request.Request(url, method="HEAD")
    with urllib.request.urlopen(req, timeout=30, context=SSL_CTX) as r:
        return int(r.headers["Content-Length"])


def fetch_range(url, start, end, path, label, retries=20):
    """下载 [start, end] 闭区间到 path，自带断点续传与重试。"""
    pos = start
    if os.path.exists(path):
        pos = start + os.path.getsize(path)
    attempt = 0
    while pos <= end:
        try:
            req = urllib.request.Request(url, headers={"Range": f"bytes={pos}-{end}"})
            with urllib.request.urlopen(req, timeout=30, context=SSL_CTX) as r, open(path, "ab") as f:
                while True:
                    buf = r.read(CHUNK)
                    if not buf:
                        break
                    f.write(buf)
                    pos += len(buf)
            attempt = 0
        except Exception as e:  # noqa: BLE001
            attempt += 1
            if attempt > retries:
                raise RuntimeError(f"{label} 重试 {retries} 次仍失败：{e}") from e
            time.sleep(min(2 * attempt, 10))
            # 断点：以实际落盘大小为准（可能比 pos 少，也可能多）
            if os.path.exists(path):
                pos = start + os.path.getsize(path)
    want = end - start + 1
    got = os.path.getsize(path)
    if got != want:
        raise RuntimeError(f"{label} 大小不符：{got} != {want}")
    print(f"  {label} 完成 {got / 1e6:.0f} MB", flush=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("url")
    ap.add_argument("out")
    ap.add_argument("--threads", type=int, default=8)
    ap.add_argument("--verify-torch", action="store_true", help="下完用 torch.load 校验")
    args = ap.parse_args()

    total = head_size(args.url)
    print(f"总大小 {total / 1e9:.2f} GB，{args.threads} 线程分块", flush=True)

    span = total // args.threads
    parts = []
    threads = []
    for i in range(args.threads):
        start = i * span
        end = total - 1 if i == args.threads - 1 else (i + 1) * span - 1
        p = f"{args.out}.part{i}"
        parts.append(p)
        t = threading.Thread(target=fetch_range, args=(args.url, start, end, p, f"块{i}"))
        t.start()
        threads.append(t)
    for t in threads:
        t.join()

    print("拼接…", flush=True)
    with open(args.out, "wb") as out:
        for p in parts:
            with open(p, "rb") as f:
                while True:
                    buf = f.read(CHUNK)
                    if not buf:
                        break
                    out.write(buf)
    for p in parts:
        os.remove(p)

    size = os.path.getsize(args.out)
    print(f"完成：{args.out}（{size / 1e9:.2f} GB）")
    if size != total:
        sys.exit(f"大小不符：{size} != {total}")

    if args.verify_torch:
        import torch

        try:
            sd = torch.load(args.out, map_location="cpu", weights_only=True)
            print(f"torch 校验通过：{len(sd)} 个张量")
        except Exception as e:  # noqa: BLE001
            sys.exit(f"torch 校验失败：{e}")


if __name__ == "__main__":
    main()
