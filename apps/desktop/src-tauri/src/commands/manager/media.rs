//! 缩略图、导出、定位、评分/旗标 —— 「对这批图做什么」这条线。
//!
//! ⭐ 缩略图走 **PVI1 裸字节**（`tauri::ipc::Response`），与单图浏览同一条
//! 零 base64 通道；其余命令走 JSON。

use std::path::PathBuf;

use tauri::ipc::Response;
use tauri::State;

use crate::commands::dto::ErrorDto;
use super::{merr, parse_tier, ExportFailureDto, ExportSummaryDto};
use crate::state::AppState;
use crate::commands::pack_pixels;

/// 取缩略图：**PVI1 裸字节**（与单图浏览同一条零 base64 通道）。
///
/// 三级回退（L1 内存 → L2 磁盘 → 现生成）都在 `pv-store` 内完成。
///
/// ## ⚠️ 为什么这个命令**必须**是 `async`（2026-10-15 实机踩坑）
/// Tauri 的规矩是：**非 `async` 的命令跑在主线程上**（要离开主线程就得写 `async fn`）。
/// 而这里的「现生成」是一次**完整解码**（几十 MB 的 JPEG / 7680×4320 的 RAW 内嵌预览）。
/// 图库首屏会同时挂几百个单元格，每个都 `invoke` 一次 —— 命令在主线程上**串行**跑完，
/// 消息泵被堵住，Windows 直接给窗口打上**「未响应」**（实测 `test-image/` 54 张
/// 生成了 50 个缩略图时窗口一直假死）。
///
/// 改成 `async` 后执行点在 async runtime 的线程池上，主线程只负责 IPC 收发。
/// 前端另外还有 `ipc/thumbQueue.ts` 的并发闸门 + 可视区懒加载，两层都要有：
/// - 后端 `async` 解决「窗口假死」；
/// - 前端限流解决「一次堆几百个请求、进度不可见」。
///
/// ⚠️ 注意 `Store` 仍被 `std::sync::Mutex` 串行保护（`rusqlite::Connection` 不是
/// `Sync`），所以**吞吐仍是串行**的；但窗口不卡了，配合 L2 缓存第二次访问就是瞬时。
/// 真要做到解码并行，得把 L1/L2/解码器从数据库锁里拆出来（留给 S2.2）。
#[tauri::command]
pub async fn manager_thumbnail(
    state: State<'_, AppState>,
    image_id: i64,
    tier: String,
) -> Result<Response, ErrorDto> {
    let tier = parse_tier(&tier).ok_or_else(|| merr(format!("未知缩略图档位：{tier}")))?;
    let bytes = state
        .with_manager(|s| {
            let img = pv_index::repo::images::get(s.conn(), image_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("图像 {image_id} 不在索引里"))?;
            match s.thumbnail(&img, tier).map_err(|e| e.to_string())? {
                Some(th) => {
                    let rgba = pv_decode::RgbaImage {
                        width: th.width,
                        height: th.height,
                        data: th.rgba,
                    };
                    Ok(pack_pixels(&rgba))
                }
                None => Err("图片离线（所在卷未挂载）".to_string()),
            }
        })
        .map_err(merr)?;
    Ok(Response::new(bytes))
}

/// 批量导出：把选中的**原图**复制到 `dest_dir`。
///
/// ⚠️ **只复制、不移动、不删除** —— 索引和原图都不受影响。
/// 重名自动退成 `name (1).jpg`；源目录 == 目标目录的会跳过（见 `pv_store::export`）。
///
/// ⚠️ **必须 `async`**：复制几十上百张原图是纯 IO，非 async 命令会跑在主线程上
/// 把窗口堵成「未响应」（同 `manager_thumbnail` 的教训）。
#[tauri::command]
pub async fn manager_export_images(
    paths: Vec<String>,
    dest_dir: String,
) -> Result<ExportSummaryDto, ErrorDto> {
    if paths.is_empty() {
        return Err(merr("没有选中任何图片"));
    }
    let dest = PathBuf::from(&dest_dir);
    let sum = pv_store::export::copy_images(&paths, &dest).map_err(|e| merr(e.to_string()))?;
    Ok(ExportSummaryDto {
        copied: sum.copied,
        skipped: sum.skipped,
        failed: sum
            .failed
            .into_iter()
            .map(|f| ExportFailureDto {
                name: f.name,
                reason: f.reason,
            })
            .collect(),
        dest_dir: sum.dest_dir.to_string_lossy().into_owned(),
    })
}

/// 在系统资源管理器里定位到某个文件（选中它）。
///
/// 用 `explorer /select,"<path>"`。⚠️ 参数必须用 **`raw_arg`** 传：
/// `Command` 默认会给每个参数加引号，而 explorer 的 `/select,` 要求
/// 「开关 + 路径」是**同一个参数**，被拆开就会当成两个路径打开两个窗口。
#[tauri::command]
pub fn reveal_in_explorer(path: String) -> Result<(), ErrorDto> {
    let p = PathBuf::from(&path);
    if !p.exists() {
        return Err(merr("文件不存在（卷可能未挂载）"));
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // raw_arg 需要「完整的一个参数」，路径带空格时也要保住引号
        let arg = format!("/select,\"{}\"", p.display());
        std::process::Command::new("explorer.exe")
            .raw_arg(arg)
            .spawn()
            .map_err(|e| merr(format!("打开资源管理器失败：{e}")))?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        Err(merr("当前平台不支持在资源管理器中定位"))
    }
}

/// 批量设置星级（0–5）。**只改索引**，不动原图、不动缓存。
///
/// 与「导出/删除」不同，这条不需要 `async` 的 IO 理由 —— 但为一致性仍走
/// `with_manager_mut`，且一次 `UPDATE ... WHERE id IN (...)` 就够快。
#[tauri::command]
pub async fn manager_set_rating(
    state: State<'_, AppState>,
    ids: Vec<i64>,
    rating: i64,
) -> Result<usize, ErrorDto> {
    state
        .with_manager_mut(|s| s.set_rating(&ids, rating).map_err(|e| e.to_string()))
        .map_err(merr)
}

/// 批量设置旗标（0 = 无，1 = 保留）。**只改索引**。
#[tauri::command]
pub async fn manager_set_flag(
    state: State<'_, AppState>,
    ids: Vec<i64>,
    flag: i64,
) -> Result<usize, ErrorDto> {
    state
        .with_manager_mut(|s| s.set_flag(&ids, flag).map_err(|e| e.to_string()))
        .map_err(merr)
}
