//! Tauri IPC 命令层。
//!
//! **设计要点**：
//!
//! 解码后的像素用紧凑格式打包后，以 **`tauri::ipc::Response`（裸字节）** 返回。
//! 前端 `invoke` 侧按响应体的 `content-type` 分派，裸字节是
//! `application/octet-stream` → `response.arrayBuffer()`，
//! **一路真二进制、零 base64、零 JSON**。
//!
//! ## ⚠️ 2026-10-09 重要更正（之前多绕了一大圈）
//!
//! 此前这里写着「不能用二进制 `Response`，要 base64」，理由是
//! 「大 raw 响应会被序列化成 JSON 数字数组导致前端爆栈」。
//! **那个现象是真的，但归因错了** —— 真正的原因是
//! `tauri.conf.json` 的 CSP **漏了 `connect-src ipc: http://ipc.localhost`**：
//! Tauri 的 invoke 走 `fetch('http://ipc.localhost/...')`，被 CSP 拦下后
//! 会**永久回退**到 `window.ipc.postMessage` 通道，而那条通道对大响应走
//! `webview.eval()` —— 把数据拼进 JS 源码交给 WebView2 执行
//! （Rust → COM 还要转 UTF-16）。于是：
//! - 小 raw（< 1024B）被塞成 `new Uint8Array([...]).buffer`（即"数字数组"）；
//! - 大响应则要绕 `fetch_channel_data` 间接命令，链条又长又慢。
//!
//! 修好 CSP 之后，`Response` 恢复走自定义协议的 `fetch`，于是裸字节直通。
//! 实测 299 帧 GIF：Rust 侧 142ms（其中 base64 占 51ms）→ **105ms**，
//! 传输体积 95MB → **71MB**，前端还省掉了一次 74.7M 次的 `atob` 展开循环。
//!
//! **回归保护**：`tests/anim_protocol.rs` 里有一条断言直接读
//! `tauri.conf.json` 检查 `connect-src` 含 `ipc:`，改回去会立刻测试失败。
//!
//! 小数据（探测结果、错误信息、文件关联状态）继续走 JSON —— 量小、结构复杂。
//!
//! ## 子模块
//! - [`dto`]    前后端交换的数据结构（DTO）与错误映射
//! - [`pixel`]  PVI1（静图）/ PVA1（动图）像素协议
//! - [`image`]  `probe_image` / `decode_image` / `decode_animation`
//! - [`edit`]   `save_edited_image`（画布调整覆盖写回原文件）
//! - [`print`]  `print_image`（Win32 PrintDlg + GDI，所见即所印）
//! - [`app`]    `app_info` / `supported_formats` / 文件关联
//! - [`folder`] `list_siblings`（同目录浏览）

mod app;
mod dto;
mod edit;
mod folder;
mod image;
mod pixel;
mod print;

pub use app::*;
pub use dto::*;
pub use edit::*;
pub use folder::*;
pub use image::*;
pub use pixel::*;
pub use print::*;
