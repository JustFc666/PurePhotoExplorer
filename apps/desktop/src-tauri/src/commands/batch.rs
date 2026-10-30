//! 图库模式的**批量写文件**命令：转格式、按 EXIF 摆正、批量打印。
//!
//! ## 与单图命令的关系
//! 单图的 `save_edited_image` / `print_image` 已经包含了完整管线（解码 → 几何变换 →
//! 编码 → 搬运 EXIF）。这里**不重复实现**，只做「循环 + 重名处理 + 逐张失败隔离」，
//! 直接复用 `edit::save_impl` / `print::print_impl` —— 否则批量导出的图和单张另存为的图
//! 会走出两套像素结果，这是最难查的一类不一致。
//!
//! ## 为什么一律 `async` + `spawn_blocking`
//! Tauri 里**非 async 命令跑在主线程**上。批量转格式要把每张原图整幅解码（Export 档，
//! 不是 2048 预览），几十张就能让窗口「未响应」几分钟。`spawn_blocking` 把它挪到
//! 阻塞线程池，UI 线程只管画界面。
//!
//! ## 失败隔离
//! 一张坏了（截断、卷掉线、格式不支持）只进 `failed`，后面的继续做 ——
//! 批量操作里「做了一半就整体回滚」比「尽量多救几张」更让人难受。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pv_decode::DecoderRegistry;
use tauri::State;

use super::dto::ErrorDto;
use crate::state::AppState;

/// 单条失败
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchFailureDto {
    pub name: String,
    pub reason: String,
}

/// 批量操作结果（转格式 / 摆正 / 打印共用）
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchSummaryDto {
    /// 成功处理的张数
    pub done: usize,
    /// 按规则跳过（无需处理 / 目标即源目录）
    pub skipped: usize,
    pub failed: Vec<BatchFailureDto>,
    /// 输出目录（写回原文件时为 `None`）
    pub dest_dir: Option<String>,
}

impl BatchSummaryDto {
    fn new() -> Self {
        Self {
            done: 0,
            skipped: 0,
            failed: Vec::new(),
            dest_dir: None,
        }
    }

    fn fail(&mut self, path: &Path, reason: impl Into<String>) {
        self.failed.push(BatchFailureDto {
            name: file_name_of(path),
            reason: reason.into(),
        });
    }
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// 把 IO 错误压成人话。
fn io_reason(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => "文件不存在（卷可能未挂载）".into(),
        std::io::ErrorKind::PermissionDenied => "没有权限".into(),
        _ => e.to_string(),
    }
}

/// 目标格式白名单（与 `pv_decode::format_from_extension` / 前端 `EXPORT_FORMATS` 一致）。
fn parse_target_format(ext: &str) -> Option<&'static str> {
    match ext.trim_start_matches('.').to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => Some("jpg"),
        "png" => Some("png"),
        "bmp" => Some("bmp"),
        _ => None,
    }
}

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
        match super::edit::save_impl(registry, src, 0, "free", None, Some(target)) {
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

        match super::edit::save_impl(registry, src, steps, "free", None, target) {
            Ok(_) => out.done += 1,
            Err(e) => out.fail(src, e.to_string()),
        }
    }
    out
}

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
            match super::print::print_impl(
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

/// 纯文件重命名（**不碰索引**）。浏览模式（未入库）走这里。
///
/// 校验规则与 `Store::rename_image` 保持一致：非法字符 / 目标已存在都要拒绝。
/// ⛔ 绝不覆盖 —— 覆盖用户文件比让改名失败严重得多。
fn rename_plain(path: Option<&str>, new_name: &str) -> std::result::Result<(), String> {
    let Some(path) = path else {
        return Err("既没有索引记录也没有本地路径".into());
    };
    if new_name.trim().is_empty() {
        return Err("文件名不能为空".into());
    }
    if new_name.chars().any(|c| r#"\/:*?"<>|"#.contains(c)) {
        return Err("文件名里有非法字符".into());
    }
    let old = Path::new(path);
    if !old.is_file() {
        return Err("文件不存在（卷可能未挂载）".into());
    }
    let Some(parent) = old.parent() else {
        return Err("路径没有父目录".into());
    };
    let new = parent.join(new_name);
    if new.exists() {
        return Err("目标文件名已存在".into());
    }
    std::fs::rename(old, &new).map_err(|e| match e.kind() {
        std::io::ErrorKind::PermissionDenied => "没有权限".into(),
        std::io::ErrorKind::NotFound => "文件不存在".into(),
        _ => e.to_string(),
    })
}

fn merr(msg: impl Into<String>) -> ErrorDto {
    ErrorDto {
        code: "managerError".into(),
        message: msg.into(),
        user_visible: true,
    }
}

/// 让 `Arc<DecoderRegistry>` 能进 `spawn_blocking` 闭包。
#[allow(dead_code)]
fn clone_registry(state: &AppState) -> Arc<DecoderRegistry> {
    state.registry_arc()
}

/// 删除请求里的一个目标（id + 绝对路径）。
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteItemDto {
    /// 浏览模式（未入库）为 `None` —— 只删文件，没有索引行要清
    pub id: Option<i64>,
    /// 卷未挂载时为 `None` —— 这种只能删索引，删不了文件
    pub path: Option<String>,
}

/// 批量删除的结果。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteSummaryDto {
    /// 已移入回收站的文件数
    pub deleted_files: usize,
    /// 已从索引移除的行数
    pub removed_index: usize,
    /// 因为「卷未挂载」只能清索引、没删到文件的条目数
    pub offline_index_only: usize,
    pub failed: Vec<BatchFailureDto>,
}

/// 批量删除：**移入回收站**（可撤销），再清索引与缩略图缓存。
///
/// ## 顺序很重要
/// 1. **先删文件**：删不掉（占用中、权限不足）就**不动索引** ——
///    否则列表里照片消失了、磁盘上还留着，用户再也找不到它。
/// 2. 删成功（或文件本来就不存在）的，才 `remove_images` 清索引 + 缓存。
/// 3. 删不掉的把原因逐条带回，前端显示。
///
/// ## 为什么回收站而不是直接删
/// 照片管理里「删除」默认必须是可撤销的。`std::fs::remove_file` 直接抹掉，
/// 没有任何挽回余地 —— 这里走 `SHFileOperationW` + `FOF_ALLOWUNDO`。
#[tauri::command]
pub async fn manager_delete_images(
    state: State<'_, AppState>,
    items: Vec<DeleteItemDto>,
) -> Result<DeleteSummaryDto, ErrorDto> {
    if items.is_empty() {
        return Err(merr("没有选中任何图片"));
    }

    let mut out = DeleteSummaryDto {
        deleted_files: 0,
        removed_index: 0,
        offline_index_only: 0,
        failed: Vec::new(),
    };

    #[cfg(windows)]
    {
        use std::path::PathBuf;

        // 1) 有路径的先送去回收站
        let to_trash: Vec<PathBuf> = items
            .iter()
            .filter_map(|i| i.path.as_ref())
            .map(PathBuf::from)
            .collect();
        if !to_trash.is_empty() {
            if let Err(e) = crate::win_trash::move_to_recycle_bin(&to_trash) {
                // Shell 整批失败：索引一个都不动，让用户处理完再来
                return Err(merr(format!("移入回收站失败：{e}")));
            }
        }

        // 2) 只有「文件确实不在了」的才删索引
        let mut ids_to_remove: Vec<i64> = Vec::new();
        for it in &items {
            match it.path.as_deref() {
                Some(p) => {
                    if std::path::Path::new(p).exists() {
                        // Shell 说成功但文件还在（罕见：被别的进程重建 / 权限问题）
                        out.failed.push(BatchFailureDto {
                            name: file_name_of(std::path::Path::new(p)),
                            reason: "文件仍在磁盘上（可能被占用）".into(),
                        });
                    } else {
                        out.deleted_files += 1;
                        // 浏览模式的项没有索引行，只有入库的才需要清
                        if let Some(id) = it.id {
                            ids_to_remove.push(id);
                        }
                    }
                }
                None => {
                    match it.id {
                        // 卷未挂载：删不了文件，但仍然清掉失效索引
                        Some(id) => {
                            out.offline_index_only += 1;
                            ids_to_remove.push(id);
                        }
                        // 既没路径也没索引：无从下手
                        None => out.failed.push(BatchFailureDto {
                            name: "(无路径)".into(),
                            reason: "既没有本地路径也没有索引记录".into(),
                        }),
                    }
                }
            }
        }

        // 3) 清索引 + 缓存
        if !ids_to_remove.is_empty() {
            let n = state
                .with_manager_mut(|s| s.remove_images(&ids_to_remove).map_err(|e| e.to_string()))
                .map_err(merr)?
                .images;
            out.removed_index = n as usize;
        }
        Ok(out)
    }
    #[cfg(not(windows))]
    {
        let _ = (state, items);
        Err(merr("当前平台不支持删除到回收站"))
    }
}

/// 重命名请求里的一项
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameItemDto {
    /// 入库行的索引 id；浏览模式（未入库）为 `None`
    pub id: Option<i64>,
    /// 当前绝对路径（`id == None` 时用它做纯文件重命名）
    pub path: Option<String>,
    /// 目标文件名（**含扩展名**，由前端按模板算好）
    pub new_name: String,
}

/// 批量重命名：改文件名 + 同步索引 + 迁移缩略图缓存。
///
/// ⚠️ 新名字由**前端**按模板算好（`{name}` / `{n}` 的展开在前端更容易做），
/// 后端只负责执行与校验（非法字符、目标已存在）。
///
/// ⚠️ 逐张执行、逐张报告：一张重名失败不该让整批停住。
/// 但因为**绝不覆盖**，「A→B 且 B→C」这种链式改名会有一部分失败 ——
/// 这是有意的：覆盖用户文件比让几个改名失败严重得多。
#[tauri::command]
pub async fn manager_rename_images(
    state: State<'_, AppState>,
    items: Vec<RenameItemDto>,
) -> Result<BatchSummaryDto, ErrorDto> {
    if items.is_empty() {
        return Err(merr("没有选中任何图片"));
    }
    let mut out = BatchSummaryDto::new();
    for it in &items {
        let res: std::result::Result<(), String> = match it.id {
            // 入库行：走 Store，改名同时同步索引 + 清旧缓存
            Some(id) => state
                .with_manager_mut(|s| s.rename_image(id, &it.new_name).map_err(|e| e.to_string()))
                .map(|_| ()),
            // 浏览模式：没有索引行，纯文件改名。
            // ⚠️ 同样**绝不覆盖**：目标存在就报错，跟入库那条保持一致。
            None => rename_plain(it.path.as_deref(), &it.new_name),
        };
        match res {
            Ok(()) => out.done += 1,
            Err(e) => out.failed.push(BatchFailureDto {
                name: it.new_name.clone(),
                reason: e,
            }),
        }
    }
    Ok(out)
}

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
            super::edit::save_impl(registry, src, 0, "free", None, Some(target.clone()))
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

/// 批量**复制**到文件夹：字节复制（不重新编码、保留原格式与 EXIF）。
///
/// 与「导出」同一条实现（`pv_store::export::copy_images`），只是入口在右键菜单里、
/// 目标目录由用户当场选。重名自动变成 `名字 (1).jpg`，绝不覆盖已有文件。
#[tauri::command]
pub async fn manager_copy_to_folder(
    paths: Vec<String>,
    dest_dir: String,
) -> Result<BatchSummaryDto, ErrorDto> {
    if paths.is_empty() {
        return Err(merr("没有选中任何图片"));
    }
    let dest = PathBuf::from(dest_dir);

    let res = tauri::async_runtime::spawn_blocking(move || {
        let r = pv_store::export::copy_images(&paths, &dest).map_err(|e| e.to_string())?;
        let mut out = BatchSummaryDto::new();
        out.done = r.copied;
        out.skipped = r.skipped;
        out.dest_dir = Some(r.dest_dir.to_string_lossy().into_owned());
        out.failed = r
            .failed
            .into_iter()
            .map(|f| BatchFailureDto {
                name: f.name,
                reason: f.reason,
            })
            .collect();
        Ok::<_, String>(out)
    })
    .await
    .map_err(|e| merr(format!("复制任务异常：{e}")))?;

    res.map_err(merr)
}

/// 把单个文件搬到目标路径：同卷 `rename`（瞬时、不占额外空间），
/// 跨卷（`rename` 报错）退化成「复制 + 删源」。
///
/// ⚠️ 失败时不能留下半个文件：`copy` 成功但 `remove_file` 失败的话，
/// 结果是**源和目标各有一份** —— 至少数据没丢，进 failed 让用户自己判断。
fn move_one(src: &Path, dst: &Path) -> std::io::Result<()> {
    if std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    std::fs::copy(src, dst)?;
    std::fs::remove_file(src)
}

/// 批量**移动**到文件夹：真实搬走文件（不是复制），并清掉源索引与缩略图缓存。
///
/// ## 顺序（与删除同一条铁律）
/// 1. **先搬文件**；2. 只有搬成功的那些，才 `remove_images` 清索引 + 缓存。
/// 搬失败却先清索引 → 列表里照片没了、磁盘上还在，用户再也找不到。
///
/// ## 只清「入库过」的行
/// 浏览模式（未入库）传进来的 `id` 是 `None`，没有索引行要清 ——
/// 但它下面的缩略图缓存也不存在（浏览不写缓存），所以什么都不用做。
#[tauri::command]
pub async fn manager_move_to_folder(
    state: State<'_, AppState>,
    items: Vec<DeleteItemDto>,
    dest_dir: String,
) -> Result<BatchSummaryDto, ErrorDto> {
    if items.is_empty() {
        return Err(merr("没有选中任何图片"));
    }
    let dest = PathBuf::from(dest_dir);

    let payload: Vec<(Option<i64>, Option<String>)> = items
        .into_iter()
        .map(|i| (i.id, i.path))
        .collect();

    let res = tauri::async_runtime::spawn_blocking(move || {
        if !dest.is_dir() {
            std::fs::create_dir_all(&dest).map_err(|e| io_reason(&e))?;
        }
        let mut out = BatchSummaryDto::new();
        out.dest_dir = Some(dest.to_string_lossy().into_owned());
        let mut used: HashSet<String> = HashSet::new();
        let mut moved_ids: Vec<i64> = Vec::new();

        for (id, path) in payload {
            let Some(raw) = path else {
                // 卷未挂载：连源文件都找不到，只能跳过
                out.skipped += 1;
                continue;
            };
            let src = PathBuf::from(&raw);
            if !src.is_file() {
                out.fail(&src, "文件不存在（卷可能未挂载）");
                continue;
            }
            let Some(name) = src.file_name().and_then(|n| n.to_str()) else {
                out.fail(&src, "路径不合法");
                continue;
            };
            let target = pv_store::export::unique_target(&dest, name, &mut used);
            // 目标就是自己（选了源所在目录）→ 跳过，不能把自己搬到自己身上
            if target == src {
                out.skipped += 1;
                continue;
            }
            match move_one(&src, &target) {
                Ok(()) => {
                    out.done += 1;
                    if let Some(i) = id {
                        moved_ids.push(i);
                    }
                }
                Err(e) => out.fail(&src, io_reason(&e)),
            }
        }
        Ok::<_, String>((out, moved_ids))
    })
    .await
    .map_err(|e| merr(format!("移动任务异常：{e}")))?;

    let (out, moved_ids) = res.map_err(merr)?;
    if !moved_ids.is_empty() {
        // 索引清不掉不算移动失败（文件已经在目标目录了），最多让列表多几行幽灵
        let _ = state.with_manager_mut(|s| s.remove_images(&moved_ids).map_err(|e| e.to_string()));
    }
    Ok(out)
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

/// 旋转/翻转并**写回原文件**（批量）。
///
/// ## 参数语义
/// - `rotate`：顺时针 90° 的步数，`1` = 顺 90，`-1`（或 `3`）= 逆 90，`2` = 180
/// - `flip_h` / `flip_v`：水平 / 垂直镜像
///
/// ## 顺序：先旋转，后翻转
/// 与「先把照片摆正、再左右翻一下」的直觉一致。同一次调用里两者可以叠加
/// （例如「顺 90° + 水平翻转」）。
///
/// ## 写回是破坏性的
/// - 只有 JPEG / PNG / BMP 能覆盖（`can_overwrite_original`）；
/// - RAW / HEIC / WebP / TIFF 等一律拒绝并给出原因 —— 尤其是 RAW：
///   画布像素来自内嵌 JPEG 预览，覆盖等于把 14bit 数据换成 8bit，不可逆；
/// - 先编码到内存、再原子替换（`write_rgba_atomic_with_exif`），
///   中途失败不会留下半截文件。
///
/// ## EXIF
/// 只要发生了任何几何变换（旋转或翻转），`Orientation` 一律置 1：
/// 像素已经是最终朝向，留着旧方向标记会让别的软件再转一次。
#[tauri::command]
pub async fn manager_transform_images(
    state: State<'_, AppState>,
    paths: Vec<String>,
    rotate: Option<i32>,
    flip_h: Option<bool>,
    flip_v: Option<bool>,
) -> Result<BatchSummaryDto, ErrorDto> {
    if paths.is_empty() {
        return Err(merr("没有选中任何图片"));
    }
    let steps = rotate.unwrap_or(0).rem_euclid(4);
    let fh = flip_h.unwrap_or(false);
    let fv = flip_v.unwrap_or(false);
    if steps == 0 && !fh && !fv {
        return Err(merr("没有指定任何调整（旋转或翻转）"));
    }
    let registry = state.registry_arc();

    let sum = tauri::async_runtime::spawn_blocking(move || {
        let mut out = BatchSummaryDto::new();
        for raw in &paths {
            let src_path = Path::new(raw);
            match transform_one(&registry, src_path, steps, fh, fv) {
                Ok(()) => out.done += 1,
                Err(e) => out.fail(src_path, e),
            }
        }
        out
    })
    .await
    .map_err(|e| merr(format!("批量旋转任务异常：{e}")))?;

    Ok(sum)
}

fn transform_one(
    registry: &DecoderRegistry,
    path: &Path,
    steps: i32,
    flip_h: bool,
    flip_v: bool,
) -> std::result::Result<(), String> {
    // 1) 格式能不能覆盖写回
    let info = registry.probe(path).map_err(|e| e.to_string())?;
    if !pv_decode::can_overwrite_original(info.format) {
        return Err(pv_decode::not_writable_reason(info.format));
    }

    // 2) 解码全尺寸（Export 档：写回不能拿 2048 预览降级原图）
    let src = registry
        .decode(path, pv_decode::DecodeTarget::new(pv_decode::DecodePurpose::Export))
        .map_err(|e| e.to_string())?;

    // 3) 先旋转、再翻转
    let mut img = pv_decode::rotate90_steps(&src, steps);
    if flip_h {
        img = flip_mirror(&img, true, false)?;
    }
    if flip_v {
        img = flip_mirror(&img, false, true)?;
    }

    // 4) EXIF：任何几何变换都让方向标记失效 → 置 1
    let exif = pv_meta::read_exif_raw(path).map(|mut t| {
        let _ = pv_meta::align_exif_with_pixels(&mut t, 1);
        t
    });

    // 5) 原子写回（编码到内存 → 临时文件 → rename）
    pv_decode::write_rgba_atomic_with_exif(&img, info.format, path, exif.as_deref())
        .map_err(|e| e.to_string())
}

/// 镜像翻转（RGBA 交织数据按行/列搬运，不走逐像素 `get/put_pixel`）。
fn flip_mirror(
    src: &pv_decode::RgbaImage,
    horizontal: bool,
    vertical: bool,
) -> std::result::Result<pv_decode::RgbaImage, String> {
    let (w, h) = (src.width as usize, src.height as usize);
    let row = w * 4;
    let s = &src.data;
    let mut out = pv_decode::RgbaImage::new(w as u32, h as u32).map_err(|e| e.to_string())?;
    let d = &mut out.data;
    for y in 0..h {
        let dy = if vertical { h - 1 - y } else { y };
        if horizontal {
            // 整行倒序：每个像素 4 字节一组反过来
            for x in 0..w {
                let si = y * row + x * 4;
                let di = dy * row + (w - 1 - x) * 4;
                d[di..di + 4].copy_from_slice(&s[si..si + 4]);
            }
        } else {
            d[dy * row..dy * row + row].copy_from_slice(&s[y * row..y * row + row]);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod flip_tests {
    use super::flip_mirror;
    use pv_decode::RgbaImage;

    /// 2×2 图，每像素首字节标记编号：`[1 2 / 3 4]`
    fn make() -> RgbaImage {
        let mut img = RgbaImage::new(2, 2).unwrap();
        for (i, v) in [1u8, 2, 3, 4].iter().enumerate() {
            img.data[i * 4..i * 4 + 4].copy_from_slice(&[*v, *v, *v, 255]);
        }
        img
    }

    fn at(img: &RgbaImage, idx: usize) -> u8 {
        img.data[idx * 4]
    }

    #[test]
    fn 水平翻转交换左右列() {
        // [1 2 / 3 4] → [2 1 / 4 3]
        let out = flip_mirror(&make(), true, false).unwrap();
        assert_eq!(
            [at(&out, 0), at(&out, 1), at(&out, 2), at(&out, 3)],
            [2, 1, 4, 3]
        );
    }

    #[test]
    fn 垂直翻转交换上下行() {
        // [1 2 / 3 4] → [3 4 / 1 2]
        let out = flip_mirror(&make(), false, true).unwrap();
        assert_eq!(
            [at(&out, 0), at(&out, 1), at(&out, 2), at(&out, 3)],
            [3, 4, 1, 2]
        );
    }

    #[test]
    fn 同时水平垂直等于旋转180() {
        // [1 2 / 3 4] → [4 3 / 2 1]
        let out = flip_mirror(&make(), true, true).unwrap();
        assert_eq!(
            [at(&out, 0), at(&out, 1), at(&out, 2), at(&out, 3)],
            [4, 3, 2, 1]
        );
    }

    #[test]
    fn 不翻转时原样返回() {
        let out = flip_mirror(&make(), false, false).unwrap();
        assert_eq!(
            [at(&out, 0), at(&out, 1), at(&out, 2), at(&out, 3)],
            [1, 2, 3, 4]
        );
    }
}

#[cfg(test)]
mod move_tests {
    use super::*;

    /// 「移动到文件夹」的底层：同卷必须是**真的搬走**（源没了、目标有内容）。
    ///
    /// 这条护栏的由来：移动如果用「复制 + 忘了删源」，用户会得到两份文件，
    /// 而且列表里还留着已经不在原处的行 —— 看起来像"移动没生效"。
    #[test]
    fn 移动同卷替换且源文件消失() {
        let base = std::env::temp_dir().join(format!("pv-move-{}", std::process::id()));
        let from = base.join("from");
        let to = base.join("to");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&from).unwrap();
        std::fs::create_dir_all(&to).unwrap();

        let src = from.join("a.txt");
        std::fs::write(&src, b"hello").unwrap();
        let dst = to.join("a.txt");

        move_one(&src, &dst).unwrap();

        assert!(!src.exists(), "源文件必须被搬走（不能留一份）");
        assert!(dst.is_file(), "目标文件必须存在");
        assert_eq!(std::fs::read(&dst).unwrap(), b"hello", "内容要一致");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 目标已存在时 `unique_target` 要给新名字，绝不覆盖（移动也不许覆盖）。
    #[test]
    fn 移动目标重名不覆盖() {
        let base = std::env::temp_dir().join(format!("pv-move-dup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("b.jpg"), b"old").unwrap();

        let mut used = HashSet::new();
        let t = pv_store::export::unique_target(&base, "b.jpg", &mut used);
        assert_eq!(t.file_name().unwrap(), "b (1).jpg", "重名要加序号");
        assert_eq!(std::fs::read(base.join("b.jpg")).unwrap(), b"old", "原文件不能动");

        let _ = std::fs::remove_dir_all(&base);
    }
}
