/**
 * IPC 桥接层出口（barrel）。
 *
 * 按职责拆分为子模块，外部统一从 `../ipc` 引用：
 * - `types`     纯类型定义
 * - `protocol`  PVI1（静图）/ PVA1（动图）像素协议解析
 * - `image`     probe_image / decode_image / decode_animation（含缓存与渐进加载）
 * - `edit`      save_edited_image（画布调整覆盖写回）
 * - `print`     print_image（系统打印对话框 + GDI，所见即所印）
 * - `app`       app_info / supported_formats / 文件关联 / traceLog
 * - `folder`    list_siblings（同目录浏览）
 * - `errors`    错误归一化与诊断信息
 * - `format`    展示格式化
 *
 * **像素一律走裸字节**（`tauri::ipc::Response`）：前提是 `tauri.conf.json`
 * 的 CSP 放行了 `connect-src ipc: http://ipc.localhost`，详见 `commands/pixel.rs`。
 */

export * from './types'
export * from './protocol'
export * from './image'
export * from './edit'
export * from './print'
export * from './app'
export * from './folder'
export * from './errors'
export * from './format'
