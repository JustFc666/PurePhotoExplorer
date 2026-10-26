# PurePhotoExplorer

> 多格式图像浏览与本地照片管理器 · Windows 桌面版 · Rust + Tauri 2 + React

PurePhotoExplorer 是一个轻量、快速的本地图片浏览器、编辑器与照片管理器。
**解码链全部用纯 Rust 实现，无 C 依赖**；**所有数据留在本机，不联网、不上传**。绿色免安装。

![PurePhotoExplorer 界面截图](docs/screenshot.png)

## 特性

### 单图浏览
- **多格式支持**：JPEG / PNG / GIF（动图）/ WebP（动图）/ HEIC / HEIF / BMP / TIFF / AVIF / PSD
- **相机 RAW**：CR3 / DNG / NRW / RAF / RW2 / ORF / SR2 / SRF（读取内嵌预览，秒开）
- **流畅浏览**：大图预览档解码优化（22 MB / 5847×3898 约 218 ms）、滚轮翻页、全屏
- **非破坏性编辑**：旋转 / 裁剪 / 8 种固定比例，可另存（覆盖保存会搬运原拍摄参数）
- **EXIF 信息**、拍摄地点（离线地名库，**不联网**）、打印、文件关联

### 图库模式（S2）
- **索引与缩略图**：SQLite 索引 + 两级缓存，大目录首屏 **89 ms**
- **浏览 ≠ 入库**：左键点目录只做临时预览，右键「加入图库」才建立索引
- **多图库 / 智能图库**：按规则自动生成的虚拟图库（今年 / 本月 / 四星以上 / 相机 RAW）
- **标签 · 地点 · 搜索**：文件名 / 标签 / 地点 / 相机机型镜头，多词 AND
- **批量操作**：评分 / 标记 / 旋转翻转 / 重命名 / 另存为 / 转格式 / EXIF 摆正 / 打印 / 删除
- **幻灯片放映**、五种排序、四联筛选

### AI 能力（本地推理，不联网）
- **自动打标**：CN-CLIP 零样本，111 条两级中文词表 + 互斥规则
- **人脸归类**：YuNet 检测 + SFace 128 维向量 + 两阶段聚类，≥3 张脸才建「人物」
- 模型许可：YuNet (MIT) + SFace (Apache-2.0)，可随产品分发

## 支持格式

| 类别 | 扩展名 |
| --- | --- |
| 位图 | `jpg` `jpeg` `png` `bmp` `gif` `webp` `tif` `tiff` `avif` |
| 手机 / 苹果 | `heic` `heif` |
| 设计稿 | `psd`（合成图） |
| 相机 RAW | `cr3` `dng` `nrw` `raf` `rw2` `orf` `sr2` `srf` |

> 格式判定只看魔数，不看扩展名。RAW 仅读取内嵌 JPEG 预览，**不支持写回原文件**。
> 可写格式仅 `jpg/jpeg/png/bmp`。

## 下载

前往 [Releases](https://github.com/JustFc666/PurePhotoExplorer/releases) 下载绿色版
`PurePhotoExplorer_x.y.z_win64_portable.zip`。

运行要求：

- Windows 10 1809+ / Windows 11
- WebView2 Runtime（Win10/11 一般已内置）

## 构建

前置：Rust 1.77+、Node 18+、`nasm`（AVIF 解码需要，需加进 PATH）。

```bash
cd apps/desktop && npm install && npm run build
cd ../.. && cargo build --release
# 产物：target/release/photovault.exe
# 发布时再改名为 PurePhotoExplorer.exe（crate 名沿用 photovault，未改）
```

> ⚠️ 改过 `apps/desktop/src-tauri/tauri.conf.json` 之后，必须先
> `touch apps/desktop/src-tauri/{build.rs,tauri.conf.json,src/main.rs}` 再构建。
> ⚠️ 挪动过工程目录的话，先 `rm -rf target/release/build/tauri-*`，否则会读到旧的绝对路径。

## 技术栈

- **后端**：Rust workspace，12 个 crate
  `pv-core` / `pv-decode` / `pv-meta` / `pv-cache` / `pv-index` / `pv-vault` /
  `pv-edit` / `pv-ai` / `pv-geo` / `pv-scan` / `pv-store` / `pv-lib`
- **前端**：React 18 + TypeScript + Vite
- **外壳**：Tauri 2（无边框全自绘窗口）

## 文档

| 文档 | 内容 |
| --- | --- |
| [docs/项目历程与踩坑总结.md](docs/项目历程与踩坑总结.md) | **接手先读**：25 天做了什么、哪些方案已被推翻、踩过哪些坑 |
| [docs/S1使用说明.md](docs/S1使用说明.md) | 单图浏览怎么用 |
| [docs/S2使用说明.md](docs/S2使用说明.md) | 图库模式怎么用 |
| [docs/S2图像资源管理器开发手册.md](docs/S2图像资源管理器开发手册.md) | 图库模式开发手册（架构 / 数据模型 / UI 规范） |
| [docs/开发顺序规划.md](docs/开发顺序规划.md) | 阶段划分与当前进度 |
| [docs/README.md](docs/README.md) | 全部文档索引 |

## 许可

本项目采用 [MIT](LICENSE) 许可。

第三方署名：离线地名数据来自 [GeoNames](https://www.geonames.org/)，**CC BY 4.0**。
