# PurePhotoExplorer

> 多格式图像浏览与编辑软件 · Windows 桌面版 · Rust + Tauri 2 + React

PurePhotoExplorer 是一个轻量、快速的本地图片浏览器与编辑器。**解码链全部用纯 Rust 实现，无 C 依赖**，绿色免安装。

## 特性

- **多格式支持**：JPEG / PNG / GIF（动图）/ WebP（动图）/ HEIC / HEIF / BMP / TIFF / AVIF / PSD
- **相机 RAW**：CR3 / DNG / NRW / RAF / RW2 / ORF / SR2 / SRF（读取内嵌预览，秒开）
- **流畅浏览**：大图预览档解码优化（22 MB / 5847×3898 约 218 ms）、滚轮翻页、全屏
- **非破坏性编辑**：旋转 / 裁剪 / 固定比例（1:1、3:4、4:3、16:9、9:16、2.35:1、2:3），可另存
- **动图渐进式播放**：GIF / WebP 动画首帧秒开，后台补齐剩余帧
- **EXIF 信息**、打印、文件关联（右键「打开方式」）

## 支持格式

| 类别 | 扩展名 |
| --- | --- |
| 位图 | `jpg` `jpeg` `png` `bmp` `gif` `webp` `tif` `tiff` `avif` |
| 手机 / 苹果 | `heic` `heif` |
| 设计稿 | `psd`（合成图） |
| 相机 RAW | `cr3` `dng` `nrw` `raf` `rw2` `orf` `sr2` `srf` |

> 格式判定只看魔数，不看扩展名。RAW 仅读取内嵌 JPEG 预览，**不支持写回原文件**。

## 下载

前往 [Releases](https://github.com/JustFc666/PurePhotoExplorer/releases) 下载绿色版 `PurePhotoExplorer_x.y.z_win64_portable.zip`。

运行要求：

- Windows 10 1809+ / Windows 11
- WebView2 Runtime（Win10/11 一般已内置）

## 构建

前置：Rust 1.77+、Node 18+、`nasm`（AVIF 解码需要）。

```bash
cd apps/desktop && npm install && npm run build
cd ../.. && cargo build --release
# 产物：target/release/photovault.exe
```

## 技术栈

- **后端**：Rust workspace（`pv-core` / `pv-decode` / `pv-meta` / `pv-cache` / `pv-index` / `pv-vault` / `pv-edit` / `pv-ai` / `pv-geo`）
- **前端**：React 18 + TypeScript + Vite
- **外壳**：Tauri 2

## 许可

本项目采用 MIT 许可。
