//! 转格式 / 按 EXIF 摆正：**重新编码**类批量操作。
//!
//! 两条命令共用同一套骨架：入口校验 → `spawn_blocking` → 逐张「解码 → 编码 → 落盘」
//! → 汇总。像素结果与单图另存为**必须一致**，所以内部都转调 `commands::edit` 的实现。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use pv_decode::DecoderRegistry;
use tauri::State;

use super::{file_name_of, io_reason, merr, parse_target_format, BatchFailureDto, BatchSummaryDto};
use crate::commands::dto::ErrorDto;
use crate::state::AppState;

/// 批量转格式：把每张解码成 Export 档、按目标扩展名重新编码，输出到 `dest_dir`。
///
/// ⚠️ 与「导出」的区别：导出是**字节级复制**（最快、无损、保留原格式）；
/// 转格式要**重新编码**（慢、有损、可跨格式）。用户点「转格式」时才知道自己在要后者。
#[tauri::command]
pub async fn manager_convert_images(
    state: State<'_, AppState>,
    paths: Vec<String>,
    dest_dir: String,
    format: String,
) -> Result<BatchSummaryDto, ErrorDto> {
    if paths.is_empty() {
        return Err(merr("没有选中任何图片"));
    }
    let Some(ext) = parse_target_format(&format) else {
        return Err(merr(format!("不支持的目标格式：{format}（只能是 jpg / png / bmp）")));
    };
    let registry = state.registry_arc();
    let dest = PathBuf::from(dest_dir);

    let sum = tauri::async_runtime::spawn_blocking(move || {
        convert_impl(&registry, &paths, &dest, ext)
    })
    .await
    .map_err(|e| merr(format!("批量转格式任务异常：{e}")))?;

    Ok(sum)
}

fn convert_impl(
    registry: &DecoderRegistry,
    paths: &[String],
    dest: &Path,
    ext: &str,
) -> BatchSummaryDto {
    let mut out = BatchSummaryDto::new();
    out.dest_dir = Some(dest.to_string_lossy().into_owned());
    if let Err(e) = std::fs::create_dir_all(dest) {
        out.failed.push(BatchFailureDto {
            name: dest.display().to_string(),
            reason: io_reason(&e),
        });
        return out;
    }

    let mut used: HashSet<String> = HashSet::new();
    for raw in paths {
        let src = Path::new(raw);
        let stem = src
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "image".into());
        let file = format!("{stem}.{ext}");
        // 目标文件名要重新算（扩展名换了），所以不能直接用原来的 `unique_target(name)`
        let target = pv_store::export::unique_target(dest, &file, &mut used);

        // rotation 0 / 自由比例 / 不裁剪 = 只做「解码 → 编码」
        match crate::commands::edit::save_impl(registry, src, 0, "free", None, Some(target)) {
            Ok(_) => out.done += 1,
            Err(e) => out.fail(src, e.to_string()),
        }
    }
    out
}

/// 按 EXIF `Orientation` 批量摆正。
///
/// - `overwrite = true`：写回原文件（`save_impl(target=None)`，原子替换，仅 jpeg/png/bmp）
/// - `overwrite = false`：输出到 `dest_dir`（不改原图）
///
/// ## 只处理能靠**旋转**还原的方向
/// `3` → 180°、`6` → 顺时针 90°、`8` → 逆时针 90°。
/// `2 / 4 / 5 / 7` 需要**镜像**，画布只有旋转/裁剪 —— 这几个进 `failed` 并如实说明，
/// 而不是转 90° 交差（那会把照片转歪还告诉用户「已摆正」）。
#[tauri::command]
pub async fn manager_auto_orient(
    state: State<'_, AppState>,
    paths: Vec<String>,
    dest_dir: Option<String>,
    overwrite: bool,
) -> Result<BatchSummaryDto, ErrorDto> {
    if paths.is_empty() {
        return Err(merr("没有选中任何图片"));
    }
    if !overwrite && dest_dir.is_none() {
        return Err(merr("另存模式必须指定输出目录"));
    }
    let registry = state.registry_arc();
    let dest = dest_dir.map(PathBuf::from);

    let sum = tauri::async_runtime::spawn_blocking(move || {
        auto_orient_impl(&registry, &paths, dest.as_deref(), overwrite)
    })
    .await
    .map_err(|e| merr(format!("批量摆正任务异常：{e}")))?;

    Ok(sum)
}

fn auto_orient_impl(
    registry: &DecoderRegistry,
    paths: &[String],
    dest: Option<&Path>,
    overwrite: bool,
) -> BatchSummaryDto {
    let mut out = BatchSummaryDto::new();
    if let Some(d) = dest {
        out.dest_dir = Some(d.to_string_lossy().into_owned());
        if let Err(e) = std::fs::create_dir_all(d) {
            out.failed.push(BatchFailureDto {
                name: d.display().to_string(),
                reason: io_reason(&e),
            });
            return out;
        }
    }

    let mut used: HashSet<String> = HashSet::new();
    for raw in paths {
        let src = Path::new(raw);
        // 画布不应用 Orientation，所以这里读的是「文件里声明的朝向」
        let Some(orientation) = pv_meta::read_orientation(src) else {
            out.skipped += 1; // 没有方向标签 = 已经摆正，无事可做
            continue;
        };
        // 1 = 正常；其余都要「重写一遍，把方向烤进像素」
        //
        // ⭐ 2026-10-30：解码层已经按 Orientation 摆正了（`registry.decode`），
        //    所以这里的几何步数就是 **0** —— 再传 1/2/3 会**转两次**。
        //    写入时方向标记统一重置为 1（`save_impl` 里保证）。
        //    顺带：镜像类（2/4/5/7）现在也能处理了，以前是直接报不支持。
        if orientation == 1 {
            out.skipped += 1;
            continue;
        }
        if !(2..=8).contains(&orientation) {
            out.fail(src, format!("无法识别的方向值 {orientation}"));
            continue;
        }
        let steps = 0;

        let target = if overwrite {
            None
        } else {
            let d = dest.expect("另存模式已在入口校验");
            let name = file_name_of(src);
            Some(pv_store::export::unique_target(d, &name, &mut used))
        };

        match crate::commands::edit::save_impl(registry, src, steps, "free", None, target) {
            Ok(_) => out.done += 1,
            Err(e) => out.fail(src, e.to_string()),
        }
    }
    out
}
