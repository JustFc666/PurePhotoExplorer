//! 压缩包内图片浏览（zip / cbz / rar / cbr / 7z / cb7）。
//!
//! ## 为什么用「虚拟路径 + 解压缓存」而不是内存解码
//! 解码层（`pv-decode`）整条链都是 `&Path` 驱动的：EXIF 读取、RAW 内嵌预览、
//! 缓存键、打印、人脸检测，全都按路径拿文件。要支持「字节流解码」得给
//! `ImageDecoder` trait 加一整套 `decode_bytes`，每个后端都要实现一遍 ——
//! 而效果上并不比这里更好：**同一张包内图片通常会被看很多次**
//! （缩略图 / 大图 / 打印 / 再打开），解压一次落盘再复用，比每次重新解压快得多。
//!
//! 所以这里的做法是：**按需把包内单个条目解到缓存目录，返回真实路径**，
//! 上层完全不知道有压缩包这回事。虚拟路径只在「前端 ↔ 命令入口」之间流动。
//!
//! ## 虚拟路径
//! `<压缩包绝对路径>!<包内路径>`，例如 `E:\books\a.cbz!pages/001.jpg`。
//!
//! - 分隔符用 `!`：Windows 文件名合法但不常见；真的撞上了也不怕 ——
//!   拆分时**从左往右找第一个使前缀成为存在文件的 `!`**（见 [`split_virtual`]）。
//! - 包内路径**保留原样**（含 `/` 子目录），只在落盘时清洗成安全的文件名。
//!
//! ## 缓存
//! `%LOCALAPPDATA%\PurePhotoExplorer\arcache\<包哈希>\`，包哈希 =
//! `blake3(绝对路径 + 字节数 + mtime)`：
//! 包被替换或重新下载后，哈希变了 → 旧目录自然成了孤儿，不会张冠李戴。
//!
//! ⚠️ **哈希不含包内容**，只含元数据。这对「图片包」够用（替换必然是重下载），
//! 但如果有人原地改包内某张图而不动大小/时间，缓存会陈旧。可接受 ——
//! 反面（每次算内容哈希）要把几百 MB 的包整个读一遍。
//!
//! ## 后端选择（按魔数，不看扩展名）
//! - `PK\x03\x04` → `zip` crate（纯 Rust，最快）
//! - `Rar!` / `7z\xBC\xAF` → 7-Zip 命令行
//!
//! ⛔ RAR **没有**纯 Rust 方案：`unrar` 是 C++ 库（要 MSVC 编译），
//! `unar` 是 GPL。而 7z.exe 本来就要随应用分发（见 [`seven_zip_exe`]），
//! 复用它比自己编一份 RAR 解码器划算得多。

#![cfg_attr(test, allow(non_snake_case))]

use std::path::{Path, PathBuf};

pub mod cache;
pub mod cli;
pub mod zip_backend;

/// 虚拟路径分隔符。
pub const SEP: char = '!';

/// 压缩包扩展名（小写，含点）。
///
/// ⚠️ 与 `pv-decode::format_from_extension`、`pv-scan::IMAGE_EXTS` **不是一回事**：
/// 那些是「能解码的图片」，这些是「装着图片的容器」。容器本身不进图库。
pub const ARCHIVE_EXTS: &[&str] = &[".zip", ".cbz", ".rar", ".cbr", ".7z", ".cb7"];

/// 包内被当作图片的扩展名（小写，含点）。
///
/// 比 `IMAGE_EXTS` 宽松一点没用 —— 这里必须**只认解码层真能解的**，
/// 否则列出来的条目点开会报「格式不支持」。RAW / HEIC / AVIF 一并列上：
/// 它们在包里和普通 JPEG 没有区别。
const IMAGE_EXTS: &[&str] = &[
    ".jpg", ".jpeg", ".jpe", ".jfif", ".png", ".gif", ".webp", ".bmp", ".tif", ".tiff", ".psd",
    ".avif", ".heic", ".heif",
];

/// 包内一个条目。
#[derive(Debug, Clone)]
pub struct Entry {
    /// 包内路径（原样，含 `/`）
    pub name: String,
    /// 解压后字节数（`0` = 后端没给）
    pub size: u64,
}

/// 一个包的最小抽象：列清单 + 读单个条目。
pub trait Pack {
    fn entries(&mut self) -> Result<Vec<Entry>, String>;
    fn read(&mut self, name: &str) -> Result<Vec<u8>, String>;
}

/// 扩展名是否是我们支持的压缩包。
pub fn has_archive_ext(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    ARCHIVE_EXTS.iter().any(|e| lower.ends_with(e))
}

/// 包内文件名是否是图片。
pub fn is_image_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    IMAGE_EXTS.iter().any(|e| lower.ends_with(e))
}

/// 拼虚拟路径。`inner` 里的反斜杠统一成正斜杠（zip 用 `/`，RAR 有时用 `\`）。
pub fn make_virtual(pack: &str, inner: &str) -> String {
    format!("{pack}{SEP}{}", inner.replace('\\', "/"))
}

/// 拆虚拟路径 → `(包路径, 包内路径)`；不是虚拟路径返回 `None`。
///
/// 找法：**从左往右**逐个 `!` 试，第一个「前面那截是存在的、带压缩包扩展名的文件」
/// 就是分隔点。这样包文件名里带 `!`（`我的!漫画.cbz`）也能拆对。
pub fn split_virtual(virtual_path: &str) -> Option<(&str, &str)> {
    if !virtual_path.contains(SEP) {
        return None;
    }
    let bytes = virtual_path.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if *b != SEP as u8 {
            continue;
        }
        let (head, tail) = virtual_path.split_at(i);
        if !has_archive_ext(head) {
            continue;
        }
        // 存在性检查放在这里而不是调用方：拆错的代价是「把目录当包」，
        // 那会让后面的解压报一个风马牛不相及的错。
        if !Path::new(head).is_file() {
            continue;
        }
        let inner = tail[SEP.len_utf8()..].trim_start_matches(['/', '\\']);
        if inner.is_empty() {
            return None;
        }
        return Some((head, inner));
    }
    None
}

/// 是否是虚拟路径（`E:\a.cbz!x.jpg`）。
pub fn is_virtual(path: &str) -> bool {
    split_virtual(path).is_some()
}

/// 读文件头 8 字节，判后端。
fn sniff(path: &Path) -> Result<Kind, String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| format!("打不开 {}：{e}", path.display()))?;
    let mut head = [0u8; 8];
    let n = f
        .read(&mut head)
        .map_err(|e| format!("读 {} 失败：{e}", path.display()))?;
    Ok(Kind::detect(&head[..n], path))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Zip,
    Cli,
}

impl Kind {
    fn detect(head: &[u8], path: &Path) -> Self {
        // `PK\x03\x04` 普通 zip；`PK\x05\x06` 空 zip；`PK\x07\x08` 分卷
        if head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06") || head.starts_with(b"PK\x07\x08") {
            return Kind::Zip;
        }
        // RAR 4/5 都是 `Rar!\x1a\x07`
        if head.starts_with(b"Rar!\x1a\x07") {
            return Kind::Cli;
        }
        // 7z 魔数
        if head.starts_with(b"7z\xbc\xaf\x27\x1c") {
            return Kind::Cli;
        }
        // 认不出来时按扩展名兜底（自解压 zip 的前面是一段 exe 头）
        if has_archive_ext(&path.to_string_lossy()) {
            Kind::Cli
        } else {
            Kind::Zip
        }
    }
}

/// 打开一个包（只为列清单 / 读条目，读完即关）。
pub fn open(pack: &str) -> Result<Box<dyn Pack>, String> {
    let p = Path::new(pack);
    if !p.is_file() {
        return Err(format!("{pack} 不是文件"));
    }
    match sniff(p)? {
        Kind::Zip => Ok(Box::new(zip_backend::ZipPack::open(p)?)),
        Kind::Cli => Ok(Box::new(cli::CliPack::open(p)?)),
    }
}

/// 列出包内的**图片**条目（自然排序：`2.jpg` 排在 `10.jpg` 前面）。
pub fn list_images(pack: &str) -> Result<Vec<Entry>, String> {
    let mut arc = open(pack)?;
    let mut items: Vec<Entry> = arc
        .entries()?
        .into_iter()
        .filter(|e| is_image_name(&e.name))
        .collect();
    items.sort_by(|a, b| natural_cmp(&a.name, &b.name));
    Ok(items)
}

/// 把虚拟路径落成缓存里的真实文件，返回该路径。
///
/// 非虚拟路径**原样返回**（不检查存在性，交给下游报错）—— 这样所有调用点
/// 都可以无条件套一层，不用先自己判断是不是压缩包。
pub fn materialize(virtual_path: &str, cache_root: &Path) -> Result<PathBuf, String> {
    let Some((pack, inner)) = split_virtual(virtual_path) else {
        return Ok(PathBuf::from(virtual_path));
    };
    cache::extract_one(pack, inner, cache_root)
}

/// 自然排序：数字段按数值比。
///
/// 漫画包几乎全是 `1.jpg / 2.jpg / … / 10.jpg`，纯字典序会把 10 排到 1 后面，
/// 翻页顺序直接乱掉（ComicViewer 当年就踩过）。
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    let (ab, bb) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0usize, 0usize);
    while i < ab.len() && j < bb.len() {
        let (ca, cb) = (ab[i], bb[j]);
        if ca.is_ascii_digit() && cb.is_ascii_digit() {
            let (mut na, mut nb) = (0u128, 0u128);
            // 逐位累积，不用字符串中转；`saturating_*` 防止超长数字串溢出
            while i < ab.len() && ab[i].is_ascii_digit() {
                na = na.saturating_mul(10).saturating_add(u128::from(ab[i] - b'0'));
                i += 1;
            }
            while j < bb.len() && bb[j].is_ascii_digit() {
                nb = nb.saturating_mul(10).saturating_add(u128::from(bb[j] - b'0'));
                j += 1;
            }
            match na.cmp(&nb) {
                Ordering::Equal => {}
                ord => return ord,
            }
            continue;
        }
        // 逐字节比（UTF-8 多字节也能保持稳定顺序，只是不按拼音）
        match ca.to_ascii_lowercase().cmp(&cb.to_ascii_lowercase()) {
            Ordering::Equal => {
                i += 1;
                j += 1;
            }
            ord => return ord,
        }
    }
    ab.len().cmp(&bb.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 自然排序把数字段按数值比() {
        let mut v = vec!["10.jpg", "2.jpg", "1.jpg", "20.jpg", "3.jpg"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["1.jpg", "2.jpg", "3.jpg", "10.jpg", "20.jpg"]);
    }

    #[test]
    fn 扩展名判定() {
        assert!(has_archive_ext("E:/a/b.CBZ"));
        assert!(has_archive_ext("c.cbr"));
        assert!(!has_archive_ext("c.jpg"));
        assert!(is_image_name("pages/001.JPG"));
        assert!(!is_image_name("readme.txt"));
    }

    /// 包名里带 `!` 也必须拆对 —— 否则整条虚拟路径解析会张冠李戴。
    #[test]
    fn 包名含感叹号也能拆() {
        let dir = std::env::temp_dir().join("pv_arc_test_split");
        std::fs::create_dir_all(&dir).unwrap();
        let pack = dir.join("我的!漫画.cbz");
        std::fs::write(&pack, b"PK\x03\x04").unwrap();
        let v = format!("{}!pages/001.jpg", pack.display());
        let (p, inner) = split_virtual(&v).expect("应当拆得出来");
        assert_eq!(Path::new(p), pack);
        assert_eq!(inner, "pages/001.jpg");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 普通路径不是虚拟路径() {
        assert!(!is_virtual(r"E:\photo\a.jpg"));
        assert!(materialize(r"E:\photo\a.jpg", Path::new("Z:/nope")).is_ok());
    }
}
