//! 文件搬家：**复制**到文件夹（源不动）与**移动**到文件夹（源消失）。
//!
//! 移动的顺序与删除同一条铁律：先动文件 → 确认落位 → 才清索引与缓存。
//! 索引清不掉不算移动失败（文件已经在目标目录了），最多让列表多几行幽灵。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use tauri::State;

use super::{io_reason, merr, BatchFailureDto, BatchSummaryDto, DeleteItemDto};
use crate::commands::dto::ErrorDto;
use crate::state::AppState;

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
