*多格式图像浏览与编辑（原代号 PhotoVault，此版更名）。*

这是 0.2.0 时期的历史版本。**日常使用请下 1.0.0** —— 图库管理、标签、本地 AI、
批量操作、绿色版都是之后才有的。留在这里是为了保留早期形态。

## 本版内容

- 多格式图像浏览与编辑（原 PhotoVault），更名为 **PurePhotoExplorer**
- 新增 **TIFF**（`.tif` / `.tiff`）与 **AVIF** 解码，纯 Rust，无 C 依赖
- JPEG 预览档解码提速：约 **301 ms → 218 ms**
- 帮助与设置拆成两个面板（设置里含「默认打开方式」与「关于」）
- 右键菜单增加「全屏 / 退出全屏」
- 修复：同目录新增或另存为的图片，未即时出现在兄弟列表里

## 当时能做什么

- 单图浏览：JPEG / PNG / GIF / WebP / HEIC / BMP / TIFF / AVIF / PSD，
  以及 CR3 / DNG / NRW / RAF / RW2 / ORF / SR2 / SRF 八种相机 RAW（读内嵌预览）
- 旋转 / 裁剪（8 种固定比例）/ 另存为，覆盖保存会搬运原拍摄参数
- EXIF 信息、拍摄地点（离线地名库，不联网）、打印、文件关联
- 全屏、滚轮翻页、缩放

## 运行要求

- Windows 10 1809+ / Windows 11
- WebView2 Runtime（Win10/11 一般已内置）

## 之后的变化

[v1.0.0](https://github.com/JustFc666/PurePhotoExplorer/releases/tag/v1.0.0) 在这一版基础上
**从零加了整个图库层**（索引、缩略图缓存、标签、地点、搜索、筛选、批量操作），
并接入本地 AI（CN-CLIP 打标 + YuNet/SFace 人脸归类），另有绿色版。
