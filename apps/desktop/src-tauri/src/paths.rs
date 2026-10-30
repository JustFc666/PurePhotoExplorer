//! 「可能是虚拟路径」的统一解析入口。
//!
//! ## 虚拟路径是什么
//! 压缩包内的图片用 `<包路径>!<包内路径>` 表示（`E:\books\a.cbz!pages/1.jpg`）。
//! 前端拿到、显示、翻页用的都是这个字符串，**它就等于一个文件的身份**；
//! 只有真正要去解码 / 读 EXIF 的那一刻，才需要把它换成磁盘上的真实文件。
//!
//! ## 为什么解析放在命令层而不是解码层
//! 解码层（`pv-decode`）整条链都是 `&Path` 驱动，EXIF、RAW 预览、缓存键、
//! 打印、人脸全都按路径拿文件。往下改要动 `ImageDecoder` trait 和每个后端；
//! 而在这里**每个入口套一层 [`resolve`]** 就够了，上层完全不知道有压缩包这回事。
//!
//! ⭐ 约定：**凡是「吃图片路径」的命令，第一件事就是 `let real = resolve(&path)?`**。
//! 漏掉一个的后果是「那张图在压缩包里显示不出来」，而普通目录一切正常 ——
//! 很容易被当成偶发问题。
//!
//! ## 不支持的操作
//! 压缩包内的图**只读**：写回原图（旋转/翻转保存）、编辑覆盖都不该落进包里
//! （那等于每次旋转都重写整个压缩包）。这类命令自己判断
//! [`is_virtual`] 并给出明确错误，不要指望这里兜底。

use std::path::PathBuf;

use crate::commands::ErrorDto;

/// 把路径落成磁盘上的真实文件。
///
/// - 普通路径：**原样返回**，不做存在性检查（下游的报错信息更专业）。
/// - 虚拟路径：按需解压到缓存目录，已解过直接命中（0ms）。
pub fn resolve(path: &str) -> Result<PathBuf, ErrorDto> {
    if !pv_archive::is_virtual(path) {
        return Ok(PathBuf::from(path));
    }
    pv_archive::materialize(path, &pv_archive::cache::default_cache_root()).map_err(|e| ErrorDto {
        code: "archiveError".into(),
        message: e,
        user_visible: true,
    })
}

/// 是否是压缩包内的条目。
pub fn is_virtual(path: &str) -> bool {
    pv_archive::is_virtual(path)
}

/// 这个路径是不是「装着图片的压缩包」。
///
/// 判据是**扩展名 + 确实是个文件**：只看扩展名的话，叫 `a.zip` 的目录
/// （解压时手滑多建了一层）会被当包打开，报一个莫名其妙的解压错误。
pub fn is_archive_file(path: &str) -> bool {
    pv_archive::has_archive_ext(path) && std::path::Path::new(path).is_file()
}

/// 压缩包被识别出来时给用户看的一句话（浏览模式卡片上的说明）。
pub fn archive_note(path: &str) -> Option<&'static str> {
    if is_archive_file(path) {
        Some("压缩包 · 双击进入")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 普通路径原样返回() {
        let p = resolve(r"E:\photo\a.jpg").unwrap();
        assert_eq!(p, PathBuf::from(r"E:\photo\a.jpg"));
        assert!(!is_virtual(r"E:\photo\a.jpg"));
    }

    #[test]
    fn 目录不算压缩包() {
        let dir = std::env::temp_dir().join("pv_archive_dir_test.zip");
        let _ = std::fs::create_dir_all(&dir);
        assert!(
            !is_archive_file(&dir.to_string_lossy()),
            "叫 .zip 的目录不能被当成包"
        );
        assert!(is_archive_file("E:/a/b.cbz") == false, "不存在的文件也不算");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
