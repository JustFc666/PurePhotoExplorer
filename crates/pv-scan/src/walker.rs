//! 目录遍历：把一个文件夹里的图像读成候选列表。

use crate::plan::FoundFile;
use crate::Result;
use std::path::Path;

/// 文件的纳秒级修改时间（拿不到则为 0）。
pub fn mtime_ns(md: &std::fs::Metadata) -> i64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

/// 是否跳过：隐藏文件 / 常见临时文件（编辑器/下载中间态）。
pub(crate) fn is_skippable(name: &str) -> bool {
    name.starts_with('.')
        || name.ends_with('~')
        || name.ends_with(".tmp")
        || name.ends_with(".crdownload")
        || name.ends_with(".part")
}

/// 遍历单个目录中的图像（**非递归**）。
///
/// 子目录是索引里独立的 folder，由上层决定是否递归。
pub fn walk_dir(dir: &Path) -> Result<Vec<FoundFile>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)?.flatten() {
        // 单个条目访问失败（权限/竞态）直接跳过，不让整目录扫描失败
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        let Ok(md) = entry.metadata() else { continue };
        if !md.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_skippable(&name) {
            continue;
        }
        if let Some((format, is_raw)) = crate::format::probe_file(&entry.path())? {
            out.push(FoundFile {
                rel_name: name,
                size: md.len() as i64,
                mtime_ns: mtime_ns(&md),
                format,
                is_raw,
            });
        }
    }
    out.sort_by(|a, b| a.rel_name.cmp(&b.rel_name));
    Ok(out)
}

/// **浏览模式**的快速遍历：扩展名能判的就不 `open`。
///
/// 与 [`walk_dir`] 的**唯一**差别是格式判定顺序 —— 跳过规则、排序、返回结构完全一致：
/// 1. 扩展名在白名单 → 直接采信（不碰文件）
/// 2. 扩展名不认识 → 回落到 [`crate::format::probe_file`] 读魔数
///    （这样「无扩展名的图」「`foo.HEICS` 这种奇怪写法」不会漏）
///
/// ## 为什么值得单开一条
/// 每个文件一次 `open` 在机械盘上是 8ms 级随机读。实测 1224 个文件的目录，
/// [`walk_dir`] 冷盘 **9.47 秒**、热缓存 60ms；而 `walk_dir_fast` 只剩下
/// `read_dir` + `metadata`（Windows 上这两者由一次目录枚举完成）。
///
/// ⛔ **入库路径（`open_dir` → [`walk_dir`]）不许换成它**：索引要准，
/// 扩展名可以撒谎（把 PNG 改名成 .jpg 是很常见的操作）。
/// 浏览只是「看一眼」，判错的代价只是卡片上的格式标签。
pub fn walk_dir_fast(dir: &Path) -> Result<Vec<FoundFile>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)?.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        let Ok(md) = entry.metadata() else { continue };
        if !md.is_file() {
            continue;
        }
        // 0 字节不可能是图片。魔数路径本来也会把它筛掉（读不到头 → Unknown），
        // 这里提前挡一下，两条路的结果才对得上（`test-image/021.jpg` 就是这么个空文件）。
        if md.len() == 0 {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_skippable(&name) {
            continue;
        }
        let picked = match crate::format::guess_by_extension(&name) {
            Some((tag, is_raw)) => Some((tag.to_string(), is_raw)),
            None => crate::format::probe_file(&entry.path())?,
        };
        if let Some((format, is_raw)) = picked {
            out.push(FoundFile {
                rel_name: name,
                size: md.len() as i64,
                mtime_ns: mtime_ns(&md),
                format,
                is_raw,
            });
        }
    }
    out.sort_by(|a, b| a.rel_name.cmp(&b.rel_name));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⭐ 核心护栏：**快速遍历与魔数遍历的结果必须一致**。
    ///
    /// 「扩展名优先」是一种性能取舍，它唯一可接受的偏差是「格式标签标错」，
    /// 但**绝不能把文件列漏或列多**。这条测试拿 `test-image` 里的真实样本
    /// （56 个文件、覆盖 jpg/jpeg/png/avif/heic/dng/cr3/raf/nrw/orf/rw2/sr2/srf/webp…）
    /// 逐个对比两条路的结果：文件名、格式标签、RAW 标记必须完全相同。
    ///
    /// 如果哪天有人往 `guess_by_extension` 的白名单里加错一项（比如把 .heic 写成 "heic"
    /// 而不是 "heif"），这里会立刻红。
    #[test]
    fn 快速遍历与魔数遍历结果一致() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-image");
        if !dir.is_dir() {
            eprintln!("跳过：找不到 test-image");
            return;
        }
        let slow = walk_dir(&dir).expect("魔数遍历失败");
        let fast = walk_dir_fast(&dir).expect("快速遍历失败");

        assert_eq!(
            slow.len(),
            fast.len(),
            "两个遍历列出的文件数不同：慢 {} / 快 {}",
            slow.len(),
            fast.len()
        );
        for (a, b) in slow.iter().zip(fast.iter()) {
            assert_eq!(a.rel_name, b.rel_name, "顺序或文件集合不一致");
            assert_eq!(
                (a.format.as_str(), a.is_raw),
                (b.format.as_str(), b.is_raw),
                "{}：扩展名判定 {} ≠ 魔数判定 {}",
                a.rel_name,
                b.format,
                a.format
            );
        }
        eprintln!("已对比 {} 个样本：扩展名判定 == 魔数判定", slow.len());
    }

    /// 扩展名不认识的**必须回落读魔数**（否则「无扩展名的图」会被漏掉）
    #[test]
    fn 扩展名不认识时回落魔数() {
        let dir = std::env::temp_dir().join("pv-walk-fast-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // 把一张真图改成无扩展名（内容仍是 JPEG）
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-image/Bardenas.jpg");
        if src.is_file() {
            let dst = dir.join("无扩展名的图");
            std::fs::copy(&src, &dst).unwrap();
            let out = walk_dir_fast(&dir).unwrap();
            assert_eq!(out.len(), 1, "无扩展名的图不该被漏掉");
            assert_eq!(out[0].format, "jpeg");
        }

        // 扩展名撒谎（内容是文本，名字叫 .jpg）→ 会列出来，但那是浏览模式的
        // 已知取舍：缩略图生成时会失败并显示「无法预览」，不会崩溃也不会写索引。
        std::fs::write(dir.join("其实是文本.jpg"), b"not an image at all").unwrap();
        let out = walk_dir_fast(&dir).unwrap();
        assert!(out.iter().any(|f| f.rel_name == "其实是文本.jpg"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
