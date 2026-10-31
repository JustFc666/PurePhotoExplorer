//! 批量「另存为」：目录 + 目标格式 + 文件名模板，三件事一次配齐。
//!
//! 同格式走**字节复制**（重编码会让 JPEG 二次压缩掉画质），换格式才解码重编。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use pv_decode::DecoderRegistry;
use tauri::State;

use super::{io_reason, merr, parse_target_format, BatchFailureDto, BatchSummaryDto};
use crate::commands::dto::ErrorDto;
use crate::state::AppState;

/// 批量「另存为」：选目标目录 +（可选）目标格式 +（可选）文件名模板。
///
/// ## 与「导出」「转换格式」的分工
/// - **导出**：字节级复制，无选项，最快，格式不变；
/// - **转换格式**：只换编码，名字跟着扩展名走；
/// - **另存为**：这三件事一次配齐 —— 目录 + 格式 + 命名。
///
/// ## 同格式时走字节复制
/// 目标格式与源格式一致（`jpg`/`jpeg` 视为同一类）时**不重新编码**：
/// 重编码会让 JPEG 二次压缩掉画质，纯属白损。只有真的换格式才解码重编。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn manager_save_as(
    state: State<'_, AppState>,
    paths: Vec<String>,
    dest_dir: String,
    // `None` = 保持每张原格式；`Some("jpg"|"png"|"bmp")` = 统一转成该格式
    // （参数上不能用 `///` doc 注释 —— 不在合法属性白名单内）
    format: Option<String>,
    // 文件名模板：`{name}` = 原主名，`{n}` = 序号。`None`/空 = 保持原名
    name_template: Option<String>,
    // 精确文件名（含扩展名），只有**单张**时由系统「另存为」对话框给出；
    // 多张时一律走 `name_template`，否则 N 张会挤到同一个名字上
    file_name: Option<String>,
) -> Result<BatchSummaryDto, ErrorDto> {
    if paths.is_empty() {
        return Err(merr("没有选中任何图片"));
    }
    let target_format = match format.as_deref() {
        None | Some("") => None,
        Some(f) => match parse_target_format(f) {
            Some(v) => Some(v.to_string()),
            None => return Err(merr(format!("不支持的目标格式：{f}（只能是 jpg / png / bmp）"))),
        },
    };
    let template = name_template.filter(|t| !t.trim().is_empty());
    let file_name = file_name.filter(|t| !t.trim().is_empty());
    let registry = state.registry_arc();
    let dest = PathBuf::from(dest_dir);

    let sum = tauri::async_runtime::spawn_blocking(move || {
        save_as_impl(
            &registry,
            &paths,
            &dest,
            target_format.as_deref(),
            template.as_deref(),
            file_name.as_deref(),
        )
    })
    .await
    .map_err(|e| merr(format!("批量另存任务异常：{e}")))?;

    Ok(sum)
}

/// 展开文件名模板。
///
/// - `{name}` → 原主名（不含扩展名）
/// - `{n}` → 序号，按总数自动补零（9 张 → `1`，100 张 → `001`）
/// - **只有「既没有 `{name}` 也没有 `{n}`」的纯固定文本**才自动补 `_{n}`——
///   否则 N 张会全叫同一个名字。写了 `{name}` 就说明要保留原名，
///   原主名本来就各不相同（不同目录撞名由 `unique_target` 加 `(1)` 兜底）。
fn expand_name_template(tpl: &str, stem: &str, seq: usize, width: usize) -> String {
    let has_name = tpl.contains("{name}");
    let has_index = tpl.contains("{n}");
    let with_index = if !has_name && !has_index {
        format!("{tpl}_{{n}}")
    } else {
        tpl.to_string()
    };
    let n = format!("{seq:0width$}");
    with_index
        .replace("{name}", stem)
        .replace("{n}", &n)
        // Windows 文件名非法字符 → 下划线（否则 rename/copy 直接失败）
        .chars()
        .map(|c| if r#"\/:*?"<>|"#.contains(c) { '_' } else { c })
        .collect::<String>()
        .trim()
        .to_string()
}

fn save_as_impl(
    registry: &DecoderRegistry,
    paths: &[String],
    dest: &Path,
    format: Option<&str>,
    template: Option<&str>,
    file_name: Option<&str>,
) -> BatchSummaryDto {
    // 精确文件名只对单张有意义；多张时忽略，避免互相覆盖
    let exact: Option<&str> = if paths.len() == 1 { file_name } else { None };
    let mut out = BatchSummaryDto::new();
    out.dest_dir = Some(dest.to_string_lossy().into_owned());
    if let Err(e) = std::fs::create_dir_all(dest) {
        out.failed.push(BatchFailureDto {
            name: dest.display().to_string(),
            reason: io_reason(&e),
        });
        return out;
    }

    let width = std::cmp::max(3, paths.len().to_string().len());
    let mut used: HashSet<String> = HashSet::new();

    for (i, raw) in paths.iter().enumerate() {
        let src = Path::new(raw);
        let old_ext = src
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_string();
        let old_stem = src
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("image_{}", i + 1));

        // 目标扩展名：换格式就用新格式，保持就沿用原扩展名
        let target_ext = match format {
            Some(f) => f.to_string(),
            None => old_ext.clone(),
        };
        if target_ext.is_empty() {
            out.fail(src, "无法确定输出格式（源文件没有扩展名）");
            continue;
        }

        // 编码层面是否等价（jpg/jpeg 视为同一类，避免无意义重编码）
        let same_format = format.is_none() || {
            let a = target_ext.to_ascii_lowercase();
            let b = old_ext.to_ascii_lowercase();
            a == b || (a == "jpg" && b == "jpeg") || (a == "jpeg" && b == "jpg")
        };

        // 命名优先级：精确文件名（系统对话框给的） > 模板 > 原名
        let exact_stem = exact.map(|f| {
            // 去掉用户可能带进来的扩展名，后面统一按 target_ext 拼
            let p = Path::new(f);
            p.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| f.to_string())
        });
        let base = match (exact_stem, template) {
            (Some(e), _) => e,
            (None, Some(t)) => expand_name_template(t, &old_stem, i + 1, width),
            (None, None) => old_stem.clone(),
        };
        // 模板被清空成空串时退回原名，别产出 `.jpg` 这种隐藏文件
        let base = if base.is_empty() { old_stem } else { base };
        let file = format!("{base}.{target_ext}");
        let target = pv_store::export::unique_target(dest, &file, &mut used);

        // ⛔ 目标就是源（用户在「另存为」里直接回车、没改目录和名字）：
        // 绝不能 fs::copy 到自身 —— 那是数据损坏的高危操作。跳过并如实报告。
        if target == src {
            out.skipped += 1;
            continue;
        }

        let res = if same_format {
            // 字节复制：无损、保留 EXIF 与原始压缩数据
            std::fs::copy(src, &target).map(|_| ())
        } else {
            // 换格式：整幅解码再编码（复用单图「另存为」的同一条管线）
            crate::commands::edit::save_impl(registry, src, 0, "free", None, Some(target.clone()))
                .map(|_| ())
                .map_err(|e| std::io::Error::other(e.to_string()))
        };

        match res {
            Ok(()) => out.done += 1,
            Err(e) => out.fail(src, io_reason(&e)),
        }
    }
    out
}

#[cfg(test)]
mod saveas_tests {
    use super::expand_name_template;

    #[test]
    fn 模板_带序号与原名() {
        assert_eq!(expand_name_template("{name}_{n}", "IMG_1", 7, 3), "IMG_1_007");
    }

    #[test]
    fn 模板_没写序号会自动补() {
        // 不补的话 N 张全叫 "旅行.jpg"，除第一张外会被 unique_target 加序号
        assert_eq!(expand_name_template("旅行", "x", 2, 3), "旅行_002");
    }

    #[test]
    fn 模板_非法字符换成下划线() {
        assert_eq!(expand_name_template("a/b:c*{n}", "x", 1, 3), "a_b_c_001");
    }

    #[test]
    fn 模板_只有原名() {
        assert_eq!(expand_name_template("{name}", "DSC0001", 5, 3), "DSC0001");
    }
}
