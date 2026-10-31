//! 批量打印：逐张按预览浮层选定的版面直接送印。
//!
//! ⛔ 模块名不能叫 `print` —— 那样子模块里的 `super::print` 会指向自己，
//! 转调 `commands::print::print_impl` 就找不到了。

use std::path::Path;

use tauri::State;

use super::{merr, BatchSummaryDto};
use crate::commands::dto::ErrorDto;
use crate::state::AppState;

/// 批量打印：逐张按「适应页面 / 纵向 / 正常质量 / A4 / 10mm 边距」直接送印。
///
/// ⚠️ 用 `quick = true` **跳过系统打印对话框** —— 批量场景下一张弹一次没人受得了。
/// 代价是没有「取消」机会，所以前端必须先弹确认框说清楚要印几张。
///
/// 每张单独一个打印作业（一张一页），不做「多页合并」：不同尺寸的照片合并排版
/// 需要真正的排版引擎，现在这套 GDI 绘制是「一张纸一张图」。
#[tauri::command]
#[allow(clippy::too_many_arguments)] // 参数镜像预览浮层的版面选项，拆分收益低
pub async fn manager_print_images(
    state: State<'_, AppState>,
    paths: Vec<String>,
    fit: Option<bool>,
    orientation: Option<String>,
    quality: Option<String>,
    paper_w_mm: Option<f64>,
    paper_h_mm: Option<f64>,
    margin_mm: Option<f64>,
    printer: Option<String>,
) -> Result<BatchSummaryDto, ErrorDto> {
    if paths.is_empty() {
        return Err(merr("没有选中任何图片"));
    }
    // 版面参数与单图打印预览**同一套**（用户在预览里选什么就印什么）；
    // 缺省值保持 A4 / 纵向 / 普通质量 / 10mm，与预览的默认一致。
    let fit = fit.unwrap_or(true);
    let orientation = orientation.unwrap_or_else(|| "portrait".into());
    let quality = quality.unwrap_or_else(|| "normal".into());
    let paper_w = paper_w_mm.unwrap_or(210.0);
    let paper_h = paper_h_mm.unwrap_or(297.0);
    let margin = margin_mm.unwrap_or(10.0);
    let registry = state.registry_arc();

    let sum = tauri::async_runtime::spawn_blocking(move || {
        let mut out = BatchSummaryDto::new();
        for raw in &paths {
            let src = Path::new(raw);
            match crate::commands::print::print_impl(
                &registry,
                src,
                0, // 批量不做画布旋转/裁剪：按文件原始像素印
                "free",
                None,
                fit,
                &orientation,
                &quality,
                paper_w,
                paper_h,
                margin,
                printer.as_deref(),
                true, // quick：批量必须跳过系统对话框
            ) {
                Ok(_) => out.done += 1,
                Err(e) => out.fail(src, e.to_string()),
            }
        }
        out
    })
    .await
    .map_err(|e| merr(format!("批量打印任务异常：{e}")))?;

    Ok(sum)
}
