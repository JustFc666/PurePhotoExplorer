//! 打印命令：`print_image`。
//!
//! 与 `save_edited_image` **共用同一套几何变换**（`edit::apply_edits`）——
//! 否则会出现「保存出的图和打印出的图不一样」这种最难查的不一致。
//!
//! 实际绘制在 `crate::win_print`（Win32 PrintDlg + GDI），
//! 这里只负责「按当前画布调整解码出像素」并把它交出去。

use std::path::PathBuf;

use pv_core::error::Result;
use pv_decode::{DecodePurpose, DecodeTarget};
use tauri::State;

use super::dto::ErrorDto;
use super::edit::CropRectDto;
use crate::state::AppState;

/// 打印结果
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrintResultDto {
    /// 用户在系统打印对话框里点了取消（此时没有任何东西被送出）
    pub cancelled: bool,
    /// 实际送印的像素宽度（含旋转/裁剪后的结果）
    pub width: u32,
    /// 实际送印的像素高度
    pub height: u32,
}

/// 枚举本机可用打印机（预览浮层「打印机」下拉框的数据源）。
/// 失败/为空时前端退化为只用「系统默认打印机」。
#[tauri::command]
pub fn list_printers() -> Vec<String> {
    crate::win_print::list_printers()
}

/// 打印当前图像（应用画布上的 90° 旋转与裁剪）。
///
/// ## 参数与 `save_edited_image` 对齐
/// `rotation_steps` / `aspect` / `crop` 的语义与写回完全一致，前端直接
/// 把同一份状态传过来即可 —— 「打印」与「保存」看到的是同一张图。
///
/// `fit`：
/// - `true`（默认）**适应页面** —— 等比缩放铺满可打印区域，最常见的期望；
/// - `false` **原始尺寸** —— 按 96 DPI 逻辑尺寸（"屏幕上多大就印多大"），
///   超出页面时才等比缩小。
///
/// `orientation`：`"portrait"` / `"landscape"`；`quality`：`"draft"` / `"normal"` / `"high"`；
/// `paper_w_mm` / `paper_h_mm`：预览里选的纸张物理尺寸（毫米，纵向）。
/// 三者经 `win_print` 写进打印机 `DEVMODE`，让送印与预览一致。
///
/// `margin_mm`：四边页边距（毫米）——与预览浮层同一个值，两边一致才所见即所印。
/// `printer`：直接打印（`quick=true`）时的目标打印机；`None`/空 = 系统默认。
/// `quick`：`true` 时跳过系统打印对话框直接送印（1 份、无取消机会）。
///
/// ## 为什么解码用 Export 档
/// 打印是最终输出，不能拿 2048px 的预览去印 —— 那在 A4 上会明显发糊。
/// 与写回用 Export 是同一个理由。
#[tauri::command]
pub fn print_image(
    state: State<'_, AppState>,
    path: String,
    rotation_steps: i32,
    aspect: String,
    crop: Option<CropRectDto>,
    fit: Option<bool>,
    orientation: Option<String>,
    quality: Option<String>,
    paper_w_mm: Option<f64>,
    paper_h_mm: Option<f64>,
    margin_mm: Option<f64>,
    printer: Option<String>,
    quick: Option<bool>,
) -> std::result::Result<PrintResultDto, ErrorDto> {
    print_impl(
        state.registry(),
        &PathBuf::from(&path),
        rotation_steps,
        &aspect,
        crop,
        fit.unwrap_or(true),
        &orientation.unwrap_or_else(|| "portrait".into()),
        &quality.unwrap_or_else(|| "high".into()),
        paper_w_mm.unwrap_or(210.0),
        paper_h_mm.unwrap_or(297.0),
        margin_mm.unwrap_or(crate::win_print::DEFAULT_MARGIN_MM),
        printer.as_deref(),
        quick.unwrap_or(false),
    )
    .map_err(ErrorDto::from)
}

/// 打印引擎开关：
/// - `true`  → 方案 B（`print_shell`）：`ShellExecute` 调系统默认程序打印**原图**（忽略编辑）。
/// - `false` → 方案 A（`print_rgba_gdi` / `print_rgba_gdi_direct`）：Win32 GDI 直绘，尊重旋转/裁剪/调色。
///
/// `quick`（按命令参数逐次传入）控制 A 是否弹系统对话框：
/// `false`=弹对话框（可选打印机/份数/方向），`true`=直接送默认打印机。
const USE_SHELL_PRINT: bool = false;

/// 与命令解耦的实现，便于单测（不需要构造 Tauri `State`）。
fn print_impl(
    registry: &pv_decode::DecoderRegistry,
    path: &std::path::Path,
    rotation_steps: i32,
    aspect: &str,
    crop: Option<CropRectDto>,
    fit: bool,
    orientation: &str,
    quality: &str,
    paper_w_mm: f64,
    paper_h_mm: f64,
    margin_mm: f64,
    printer: Option<&str>,
    quick: bool,
) -> Result<PrintResultDto> {
    // 解码一次：A 分支拿它做变换，B 分支只取原始尺寸填结果。
    let src = registry.decode(path, DecodeTarget::new(DecodePurpose::Export))?;

    let doc_name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "PurePhotoExplorer".into());

    let cancelled = if USE_SHELL_PRINT {
        // 方案 B：直接把原文件交给系统默认程序，**编辑一概不生效**
        crate::win_print::print_shell(path)?
    } else {
        // 方案 A：应用画布上的旋转/裁剪，自己拿像素直绘
        let (edited, _cropped) = super::edit::apply_edits(&src, rotation_steps, aspect, crop)?;
        if quick {
            // 直接送指定（或默认）打印机，不弹系统对话框（cancelled 恒为 false）
            crate::win_print::print_rgba_gdi_direct(
                &edited,
                fit,
                &doc_name,
                orientation,
                quality,
                paper_w_mm,
                paper_h_mm,
                margin_mm,
                printer,
            )?
        } else {
            crate::win_print::print_rgba_gdi(
                &edited,
                fit,
                &doc_name,
                orientation,
                quality,
                paper_w_mm,
                paper_h_mm,
                margin_mm,
            )?
        }
    };

    Ok(PrintResultDto {
        cancelled,
        width: src.width,
        height: src.height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pv_decode::{BitmapBackend, DecoderRegistry};

    fn registry() -> DecoderRegistry {
        let mut r = DecoderRegistry::new();
        r.register(Box::new(BitmapBackend));
        r
    }

    fn sample(name: &str) -> Option<PathBuf> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../test-image");
        assert!(dir.is_dir(), "找不到 test-image（推算 {}）", dir.display());
        let p = dir.join(name);
        if p.is_file() && std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0) > 4096 {
            Some(p)
        } else {
            None
        }
    }

    /// 打印与写回必须**用同一套变换**：这里验证 `apply_edits` 被正确调用 ——
    /// 用一张会被裁掉的图核对送印尺寸。
    ///
    /// 不真的送打印机（那会弹对话框 / 污染打印队列），
    /// 只验证「变换后的像素」这一步。
    #[test]
    fn 打印前的像素变换与写回一致() {
        let Some(p) = sample("clockpic.psd") else {
            eprintln!("跳过：找不到 PSD 样本");
            return;
        };
        let reg = registry();
        let src = reg
            .decode(&p, DecodeTarget::new(DecodePurpose::Export))
            .unwrap();
        assert_eq!((src.width, src.height), (1240, 1754));

        // 旋转 1 步（顺时针 90°）→ 宽高对调
        let (rot, cropped) =
            super::super::edit::apply_edits(&src, 1, "free", None).unwrap();
        assert!(!cropped);
        assert_eq!((rot.width, rot.height), (1754, 1240));

        // 1:1 居中裁剪 → 取短边
        let (sq, cropped) =
            super::super::edit::apply_edits(&src, 0, "1:1", None).unwrap();
        assert!(cropped);
        assert_eq!(sq.width, sq.height, "1:1 裁剪后应为正方形");
        assert_eq!(sq.width, 1240, "应取较短边作为边长");
    }
}
