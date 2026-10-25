//! 「我的电脑」式**文件浏览器**的枚举逻辑（侧栏「文件夹」用）。
//!
//! 与 `pv-scan` 的分工：这里只做**展示用的目录枚举**，不建索引 ——
//! 用户点了某个目录之后，才由 `manager_scan` 把它扫进图库。
//!
//! ## 为什么只有「我的电脑」一层，没有「图片/桌面」这类快捷方式
//! 产品决策（2026-10-15）：侧栏就是**文件浏览器**，从「我的电脑」往下
//! 树状展开自己选。不预置任何登录用户的标准文件夹 ——
//! 预置了就得替用户决定「哪个目录才该看」，而这是他的事。
//!
//! 📌 曾经实现过「读 `HKCU\...\User Shell Folders` 拿图片/桌面/文档/下载」
//! 并已删除。若日后要做「快速访问」，那段逻辑的关键结论是：
//! **必须读注册表，不能拼 `%USERPROFILE%`** —— 本机实测 shell 文件夹
//! 全部重定向到 `E:\Users\Fc\...`，而 `C:\Users\Fc\Desktop` 根本不存在。

use std::path::Path;

/// 文件浏览器的一个节点。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FsNode {
    /// 显示名（盘符显示卷标，取不到就用类型名）
    pub name: String,
    /// `None` = 虚拟节点（只有「我的电脑」本身没有对应路径）
    pub path: Option<String>,
    /// `computer` / `drive` / `dir`
    pub kind: String,
    /// 附加说明（盘符 `(C:)`）
    pub note: Option<String>,
}

/// `list_dirs` 单层最多返回的子目录数 —— 防止把
/// `C:\Windows\WinSxS` 这种上万子目录的目录一次性灌进侧栏。
const MAX_CHILDREN: usize = 400;

/// 「我的电脑」虚拟节点的路径占位（前端拿它当 key 用）。
pub const COMPUTER_KEY: &str = "__computer__";

// ── 盘符 ─────────────────────────────────────────────────────

#[cfg(windows)]
fn drives() -> Vec<FsNode> {
    use windows_sys::Win32::Storage::FileSystem as fs;

    // windows-sys 0.59 没有导出这些常量，用字面量并在此处命名
    const DRIVE_NO_ROOT_DIR: u32 = 1;
    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;
    const DRIVE_REMOTE: u32 = 4;
    const DRIVE_CDROM: u32 = 5;
    const DRIVE_RAMDISK: u32 = 6;

    fn from_wide(buf: &[u16]) -> String {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..len])
    }

    let mask = unsafe { fs::GetLogicalDrives() };
    let mut out = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root = format!("{letter}:\\");
        let mut wide: Vec<u16> = root.encode_utf16().collect();
        wide.push(0);

        let ty = unsafe { fs::GetDriveTypeW(wide.as_ptr()) };
        // 无根目录（空光驱 / 空读卡器）：列出来只会让用户点进去看报错
        if ty == DRIVE_NO_ROOT_DIR || ty == 0 {
            continue;
        }

        // 卷标签取不到不算错（未格式化的盘、无权限的加密盘都会失败）
        let mut label = vec![0u16; 128];
        let ok = unsafe {
            fs::GetVolumeInformationW(
                wide.as_ptr(),
                label.as_mut_ptr(),
                label.len() as u32,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
            )
        };
        let label = if ok != 0 { from_wide(&label) } else { String::new() };

        let type_name = match ty {
            DRIVE_REMOVABLE => "可移动磁盘",
            DRIVE_FIXED => "本地磁盘",
            DRIVE_REMOTE => "网络驱动器",
            DRIVE_CDROM => "光盘",
            DRIVE_RAMDISK => "内存盘",
            _ => "驱动器",
        };

        out.push(FsNode {
            name: if label.is_empty() {
                type_name.to_string()
            } else {
                label
            },
            path: Some(root),
            kind: "drive".to_string(),
            note: Some(format!("({letter}:)")),
        });
    }
    out
}

#[cfg(not(windows))]
fn drives() -> Vec<FsNode> {
    // 非 Windows：只把根目录当作一个「盘」
    vec![FsNode {
        name: "根目录".to_string(),
        path: Some("/".to_string()),
        kind: "drive".to_string(),
        note: None,
    }]
}

/// 统一分隔符、去掉尾部多余斜杠；**盘根（`C:\`）的尾斜杠要保留**。
fn normalize_dir(dir: &Path) -> String {
    let mut s = dir.to_string_lossy().replace('/', "\\");
    let is_drive_root = s.len() >= 2 && s.as_bytes()[1] == b':' && s.ends_with('\\');
    while s.ends_with('\\') && !(is_drive_root && s.len() == 3) {
        s.pop();
    }
    s
}

/// 从路径取一个可读的显示名。
///
/// 盘根（`E:\`）没有「最后一段」，退回该盘的**卷标**（复用 [`drives`]）——
/// 侧栏里显示的正是卷标，这样从地址栏跳过去和点树看到的标题一致。
fn display_name(normalized: &str, vol_label: Option<String>) -> String {
    let p = Path::new(normalized);
    if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
        if !name.is_empty() {
            return name.to_string();
        }
    }
    vol_label.unwrap_or_else(|| normalized.to_string())
}

/// 把用户在地址栏里敲的路径**解析成要浏览的目录**，返回 `(绝对路径, 显示名)`。
///
/// - 目录 → 它自己
/// - 文件 → **它所在的目录**（把一张图片的路径粘进来，落到它的文件夹，符合直觉）
/// - 只有盘符的 `E:` → 补成 `E:\`
/// - 前后空格、成对引号（从别处复制常常带引号）都会被剥掉
/// - 不存在 / 空串 → `None`，由前端提示，**不要**默默跳到一个不存在的地方
///
/// ⛔ **不要用 `std::fs::canonicalize()`**：Windows 上它会返回 `\\?\C:\...` 前缀，
/// 而文件浏览器里所有路径都是 `read_dir` 拼出来的 `C:\...` 形式，
/// 两者对不上 → 侧栏高亮与树展开都会失败。
pub fn resolve_dir(input: &str) -> Option<(String, String)> {
    let s = input.trim().trim_matches('"').trim();
    if s.is_empty() {
        return None;
    }
    // `E:` → `E:\`（`Path::new("E:").is_dir()` 是 false）
    let owned;
    let s: &str = if s.len() == 2 && s.ends_with(':') {
        owned = format!("{s}\\");
        &owned
    } else {
        s
    };

    let p = Path::new(s);
    let dir = if p.is_dir() {
        p.to_path_buf()
    } else if p.is_file() {
        p.parent()?.to_path_buf()
    } else {
        return None;
    };

    let normalized = normalize_dir(&dir);
    // 盘根时用卷标当显示名（侧栏点的就是这个）
    let is_drive_root = normalized.len() == 3 && normalized.as_bytes()[1] == b':';
    let label = if is_drive_root {
        drives()
            .into_iter()
            .find(|d| d.path.as_deref().map(|x| x.eq_ignore_ascii_case(&normalized)) == Some(true))
            .map(|d| d.name)
    } else {
        None
    };
    Some((normalized.clone(), display_name(&normalized, label)))
}

// ── 当前用户的常用文件夹 ─────────────────────────────────────

/// 取一个「已知文件夹」的真实路径（**尊重用户的自定义位置**）。
///
/// 很多人把图片库移到了 D 盘 / 移动盘，硬编码 `%USERPROFILE%\Pictures` 会指向空目录，
/// 于是侧栏多几个点进去什么都没有的入口。`SHGetKnownFolderPath` 返回的是
/// 用户（或组策略）重定向后的实际位置。
#[cfg(windows)]
fn known_folder(rfid: *const windows_sys::core::GUID, fallback: &str) -> Option<String> {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::SHGetKnownFolderPath;

    let mut raw: windows_sys::core::PWSTR = std::ptr::null_mut();
    let hr = unsafe { SHGetKnownFolderPath(rfid, 0, std::ptr::null_mut(), &mut raw) };
    if hr >= 0 && !raw.is_null() {
        let len = unsafe {
            let mut n = 0usize;
            while *raw.add(n) != 0 {
                n += 1;
            }
            n
        };
        let s = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(raw, len) });
        unsafe { CoTaskMemFree(raw as *const _ as *mut _) };
        if !s.is_empty() && std::path::Path::new(&s).is_dir() {
            return Some(s);
        }
    }
    // 回退：注册表/API 拿不到时按默认位置试（不存在就不加这个入口）
    let home = std::env::var("USERPROFILE").ok()?;
    let p = std::path::Path::new(&home).join(fallback);
    p.is_dir().then(|| p.to_string_lossy().into_owned())
}

// ── 对外接口 ─────────────────────────────────────────────────

/// 顶层节点：**当前用户的常用文件夹 + 「我的电脑」**。
///
/// 常用文件夹与「我的电脑」**平级**（2026-10-17 用户要求）—— 点照片比逐个盘符翻快。
/// 取不到 / 不存在的入口直接不列，不留点进去看报错的行。
pub fn roots() -> Vec<FsNode> {
    let mut out: Vec<FsNode> = Vec::new();

    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::Shell::{FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Pictures};
        let known: [(&str, *const windows_sys::core::GUID, &str); 3] = [
            ("图片", &FOLDERID_Pictures, "Pictures"),
            ("文档", &FOLDERID_Documents, "Documents"),
            ("下载", &FOLDERID_Downloads, "Downloads"),
        ];
        for (name, rfid, fallback) in known {
            if let Some(path) = known_folder(rfid, fallback) {
                out.push(FsNode {
                    name: name.to_string(),
                    path: Some(path),
                    kind: "known".to_string(),
                    note: None,
                });
            }
        }
    }

    out.push(FsNode {
        name: "我的电脑".to_string(),
        path: None,
        kind: "computer".to_string(),
        note: None,
    });
    out
}

/// 展开一个节点。
///
/// - `None` / [`COMPUTER_KEY`] → 盘符
/// - 其余 → 该目录的**直接子目录**（不递归；`read_dir` 失败的条目直接跳过）
pub fn list_dirs(path: Option<&str>) -> Vec<FsNode> {
    let Some(raw) = path else {
        return drives();
    };
    if raw == COMPUTER_KEY {
        return drives();
    }

    let dir = Path::new(raw);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut out: Vec<FsNode> = Vec::new();
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        // 不跟随符号链接：可能指向自身或成环，展开时会无限套娃
        if ft.is_symlink() || !ft.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        // 隐藏/系统目录（回收站、卷影信息）对浏览照片没有意义，且常有权限问题
        if name.starts_with('.') || name.starts_with('$') || name == "System Volume Information" {
            continue;
        }
        out.push(FsNode {
            name,
            path: Some(entry.path().to_string_lossy().into_owned()),
            kind: "dir".to_string(),
            note: None,
        });
    }

    // `sort_by_cached_key` 而不是 `sort_by_key`：大小写转换要分配 String，
    // 前者每个元素只算一次，后者每次比较都要算。
    out.sort_by_cached_key(|n| n.name.to_lowercase());
    if out.len() > MAX_CHILDREN {
        out.truncate(MAX_CHILDREN);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 顶层 = 常用文件夹（图片/文档/下载，可能缺）+ 末位的虚拟节点「我的电脑」。
    ///
    /// ⚠️ 这条测试曾经断言「只有我的电脑、不预置用户目录」—— 那是加常用文件夹
    /// **之前**的约定（2026-10-17 改成要显示了）。现在钉的是新约定：
    /// 常用文件夹必须在前面且真实存在，**「我的电脑」永远在最后**（它是虚拟节点，
    /// 前端按位置判断"这是我的电脑"，跑到中间会点错）。
    #[test]
    fn 顶层是常用文件夹加我的电脑() {
        let r = roots();
        assert!(!r.is_empty());
        let last = r.last().unwrap();
        assert_eq!(last.name, "我的电脑");
        assert_eq!(last.kind, "computer");
        assert!(last.path.is_none(), "我的电脑是虚拟节点，不该有路径");

        for n in &r[..r.len() - 1] {
            assert_eq!(n.kind, "known", "常用文件夹的 kind 必须是 known：{}", n.name);
            let p = n.path.as_ref().expect("常用文件夹必须有路径");
            assert!(
                std::path::Path::new(p).is_dir(),
                "常用文件夹必须真实存在：{p}"
            );
        }
    }

    #[test]
    fn 我的电脑能展开出盘符() {
        let d = list_dirs(None);
        assert!(!d.is_empty(), "至少应有一个盘");
        for n in &d {
            let p = n.path.as_ref().expect("盘符必须有路径");
            assert!(p.ends_with('\\'), "盘符路径应以反斜杠结尾：{p}");
            assert_eq!(n.kind, "drive");
            assert!(n.note.is_some(), "盘符应带 (X:) 说明");
        }
        // 占位 key 与「不传路径」等价（前端展开「我的电脑」时传的就是它）
        assert_eq!(list_dirs(Some(COMPUTER_KEY)).len(), d.len());
    }

    #[test]
    fn 列出子目录不跟随符号链接且跳系统目录() {
        let sys = std::env::temp_dir();
        let d = list_dirs(Some(&sys.to_string_lossy()));
        for n in &d {
            assert_eq!(n.kind, "dir");
            assert!(!n.name.starts_with('$'), "不该列出 $ 开头的系统目录");
            assert!(!n.name.starts_with('.'), "不该列出点开头的隐藏目录");
        }
    }

    /// 列出的每个子目录都必须真的存在（否则用户点进去会报错）。
    #[test]
    fn 列出的子目录都真实存在() {
        let root = std::env::temp_dir();
        for n in list_dirs(Some(&root.to_string_lossy())) {
            let p = Path::new(n.path.as_ref().unwrap());
            assert!(p.is_dir(), "列出的路径必须存在：{}", p.display());
        }
    }

    #[test]
    fn 不存在的路径返回空而不是报错() {
        let d = list_dirs(Some(r"Z:\definitely-not-here-pv"));
        assert!(d.is_empty());
    }

    /// 地址栏的路径解析：目录 / 文件 / 盘符 / 垃圾输入。
    #[test]
    fn 地址栏解析成要浏览的目录() {
        let root = std::env::temp_dir().join(format!("pv_resolve_{}", std::process::id()));
        let sub = root.join("sub");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&sub).unwrap();
        let file = sub.join("a.jpg");
        std::fs::write(&file, b"not really a jpeg").unwrap();

        let root_s = root.to_string_lossy().into_owned();
        let sub_s = sub.to_string_lossy().into_owned();
        let file_s = file.to_string_lossy().into_owned();

        // 目录 → 它自己
        let (p, name) = resolve_dir(&root_s).expect("目录应能解析");
        assert_eq!(p, root_s);
        assert_eq!(
            name,
            root.file_name().unwrap().to_string_lossy(),
            "显示名应是最后一段"
        );
        let (_, n2) = resolve_dir(&sub_s).unwrap();
        assert_eq!(n2, "sub", "显示名应是最后一段");

        // 文件 → 它所在的目录
        let (p3, n3) = resolve_dir(&file_s).expect("文件应解析到其父目录");
        assert_eq!(p3, sub_s);
        assert_eq!(n3, "sub");

        // 前后空格 / 成对引号要剥掉（从别处复制常常带引号）
        assert!(resolve_dir(&format!("  \"{sub_s}\"  ")).is_some());
        // 正斜杠要归一成反斜杠
        let (p4, _) = resolve_dir(&sub_s.replace('\\', "/")).unwrap();
        assert!(!p4.contains('/'), "应已归一成反斜杠：{p4}");
        // 尾部斜杠要去掉（盘根除外）
        let (p5, _) = resolve_dir(&format!("{sub_s}\\\\")).unwrap();
        assert!(!p5.ends_with('\\'));

        // 垃圾输入 → None
        assert!(resolve_dir("").is_none());
        assert!(resolve_dir("    ").is_none());
        assert!(resolve_dir(r"Z:\definitely-not-here-pv").is_none());
        assert!(resolve_dir("这不是路径").is_none());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 盘根：`E:` 要补成 `E:\`，而且尾斜杠**不能**被 normalize 削掉。
    #[test]
    fn 地址栏解析盘根() {
        let d = drives();
        let Some(first) = d.first() else { return };
        let p = first.path.as_ref().unwrap(); // 形如 `C:\`
        assert_eq!(normalize_dir(Path::new(p)), *p, "盘根的尾斜杠必须保留");

        let letter = &p[..2]; // `C:`
        let (got, name) = resolve_dir(letter).expect("只写盘符也应能解析");
        assert_eq!(&got, p, "`{letter}` 应补成 `{p}`");
        assert_eq!(name, first.name, "盘根的显示名应是卷标");
    }
}
