# PurePhotoExplorer 文档索引

> 更新：2026-10-24 · 产品名 **PurePhotoExplorer**（内部代号 PhotoVault，S2 = 图库模式）

## 怎么看这些文档

| 文档 | 类型 | 什么时候看 |
| --- | --- | --- |
| [S2使用说明.md](S2使用说明.md) | 📖 使用说明 | **日常用这个** —— 图库模式怎么加图库、筛选、批量操作 |
| [S1使用说明.md](S1使用说明.md) | 📖 使用说明 | 单图浏览模式（看图、旋转裁剪、打印） |
| [S2图像资源管理器开发手册.md](S2图像资源管理器开发手册.md) | 🛠 开发手册 | 改图库模式前必读：架构、UI 规范、数据模型、索引管线、加密图库方案 |
| [图像浏览软件开发文档.md](图像浏览软件开发文档.md) | 🛠 开发文档 | 产品需求 / 总体架构 / 威胁模型（v1.1，S1 时期定稿） |
| [AI自动打标与人脸识别-可行性.md](AI自动打标与人脸识别-可行性.md) | 🧪 可行性 | 做 AI 打标 / 人脸识别前读；人脸模型**非商业许可** |
| [开发顺序规划.md](开发顺序规划.md) | 📋 规划 | 想知道「为什么是这个顺序、现在做到哪」 |
| [M0技术预研.md](M0技术预研.md) | 📋 存档 | 环境勘察记录（已完成，只读） |

## 目录约定

| 路径 | 内容 |
| --- | --- |
| `docs/design/` | UI 设计稿（PNG） |
| `docs/preview/` | 配色预览（HTML，双击打开） |
| `assets/` | 图标等素材；`assets/ai/` 是 AI 模型权重（不入仓库） |
| `tools/ai/` | AI 打标脚本（`auto_tag.py` 主入口、词表生成、探针） |
| `scripts/` | 构建 / 自动化辅助脚本（MSVC 环境、UI 自动化、截图） |
| `crates/` | 12 个 Rust crate（索引 / 缓存 / 解码 / 编辑 / 地理 / 扫描 / 存储 / 加密 …） |
| `apps/desktop/` | Tauri + React 前端 |
| `release/` | 打包产物（`PurePhotoExplorer.exe`，不入仓库） |

## 两条硬规矩

1. **改 `tauri.conf.json` 后必须 `touch build.rs tauri.conf.json main.rs` 再 `cargo build`**
   —— tauri-build 的 `rerun-if-changed` 不可靠，不 touch 会沿用旧配置（实测被钳在旧尺寸）。
2. **格式清单只有一个真源**：`apps/desktop/src/constants/formats.ts`；
   Rust 侧 `app.rs::supported_formats()` 与 `BitmapBackend::supports()` 必须同步，
   护栏测试 `tests/format_sync.rs`。
