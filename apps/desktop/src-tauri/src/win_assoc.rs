//! 文件关联（仅 Windows）：把 PurePhotoExplorer 加进右键「**打开方式**」列表。
//!
//! ## 为什么不做「设为默认程序」
//! Windows 8 起引入了 UserChoice 哈希保护：`HKCU\Software\Microsoft\Windows\
//! CurrentVersion\Explorer\FileExts\.jpg\UserChoice` 里的 ProgId 被一个与
//! 用户 SID + 时间戳绑定的哈希保护着，**任何程序都无法静默改写它** ——
//! 改完会被系统还原，或者弹「默认应用已重置」。官方唯一途径是弹系统对话框
//! 让用户手动点确认。
//!
//! 所以这里只做**合法且稳定**的那一半：把本程序写进
//! `HKCU\Software\Classes\.<ext>\OpenWithProgids`。之后用户
//! 右键图片 →「打开方式」就能看到 PurePhotoExplorer，并可自行勾选「始终使用」。
//! 这样虽然多一步用户操作，但不会被系统回滚，也不会污染用户的选择。
//!
//! ## 写了什么（全部在 HKCU，不需要管理员权限）
//! 1. `HKCU\Software\Classes\PurePhotoExplorer.<ext>` —— ProgID：显示名、
//!    默认图标、以及 open 命令（`"<exe>" "%1"`）；
//! 2. `HKCU\Software\Classes\.<ext>\OpenWithProgids` 增加一个
//!    `PurePhotoExplorer.<ext>` 值（类型 REG_NONE、数据为空 —— 这是 Windows 的约定写法）；
//! 3. `SHChangeNotify(SHCNE_ASSOCCHANGED)` 通知 shell 刷新缓存，
//!    否则「打开方式」菜单要等重启才更新。

/// ProgID 前缀：`PurePhotoExplorer.jpg` / `PurePhotoExplorer.png` …
pub const PROG_ID_PREFIX: &str = "PurePhotoExplorer";

/// 规范化扩展名：去点、转小写。
///
/// ⚠️ 这里同时是**安全边界**：扩展名会被拼进注册表路径，
/// 所以只接受「1–8 个 ASCII 字母数字」。带 `\`、`.`、空格的输入一律拒绝，
/// 否则可以构造出 `..\..\Software\...` 这类路径穿越。
pub fn normalize_ext(ext: &str) -> Option<String> {
    let e = ext.trim().trim_start_matches('.').to_ascii_lowercase();
    if e.is_empty() || e.len() > 8 || !e.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    Some(e)
}

/// 给前端的一句话说明（放 Rust 侧写一处，避免前后端两套文案打架）
pub const NOTE: &str =
    "Windows 不允许程序自行设为「默认」，这里只把 PurePhotoExplorer 加入右键「打开方式」列表。\
     之后右键图片 →「打开方式」→ 选中 PurePhotoExplorer 并勾选「始终使用此应用」即可。";

// ─────────────────────────── Windows 实现 ───────────────────────────

#[cfg(windows)]
mod imp {
    use super::{normalize_ext, PROG_ID_PREFIX};
    use winreg::enums::{HKEY_CURRENT_USER, KEY_ALL_ACCESS, REG_NONE};
    use winreg::{RegKey, RegValue};

    /// 打开（必要时创建）`HKCU\Software\Classes`
    fn classes() -> Result<RegKey, String> {
        RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey("Software\\Classes")
            .map(|(k, _)| k)
            .map_err(|e| format!("打开 HKCU\\Software\\Classes 失败：{e}"))
    }

    /// 本程序可执行文件路径（写入 open 命令用）
    fn exe_path() -> Result<String, String> {
        std::env::current_exe()
            .map(|p| p.to_string_lossy().into_owned())
            .map_err(|e| format!("获取程序路径失败：{e}"))
    }

    /// 通知 shell「关联变了」。
    ///
    /// 不做这一步的话，「打开方式」菜单要等资源管理器重启才刷新 ——
    /// 用户会以为「点了没反应」。
    ///
    /// `pub`：批量操作（「全选」）时由调用方在循环结束后统一调一次，
    /// 而不是每个扩展名都广播一次。
    pub fn notify_shell() {
        use windows_sys::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};
        unsafe {
            SHChangeNotify(
                SHCNE_ASSOCCHANGED as i32,
                SHCNF_IDLIST,
                std::ptr::null(),
                std::ptr::null(),
            );
        }
    }

    /// 把 `ext` 注册进「打开方式」列表。`display_name` 是菜单里显示的类型名。
    ///
    /// 单扩展名入口 = [`register_silent`] + 一次 shell 通知。
    pub fn register(ext: &str, display_name: &str) -> Result<(), String> {
        register_silent(ext, display_name)?;
        notify_shell();
        Ok(())
    }

    /// 同 [`register`]，但**不通知 shell**。
    ///
    /// 批量操作（「全选」一次改 7 个扩展名）时，每个都广播一次
    /// `SHCNE_ASSOCCHANGED` 是纯浪费 —— 调用方在循环结束后统一调一次
    /// [`notify_shell`] 即可。
    pub fn register_silent(ext: &str, display_name: &str) -> Result<(), String> {
        let ext = normalize_ext(ext).ok_or_else(|| format!("非法扩展名：{ext}"))?;
        let exe = exe_path()?;
        let prog_id = format!("{PROG_ID_PREFIX}.{ext}");
        let classes = classes()?;

        // ① ProgID 本体
        let (prog, _) = classes
            .create_subkey(&prog_id)
            .map_err(|e| format!("创建 ProgID {prog_id} 失败：{e}"))?;
        prog.set_value("", &display_name)
            .map_err(|e| format!("写入 ProgID 显示名失败：{e}"))?;

        // 默认图标取 exe 内置图标（索引 0）
        prog.create_subkey("DefaultIcon")
            .and_then(|(k, _)| k.set_value("", &format!("{exe},0")))
            .map_err(|e| format!("写入默认图标失败：{e}"))?;

        // open 命令：路径两侧都要引号，否则带空格的安装目录会解析错
        prog.create_subkey("shell\\open\\command")
            .and_then(|(k, _)| k.set_value("", &format!("\"{exe}\" \"%1\"")))
            .map_err(|e| format!("写入 open 命令失败：{e}"))?;

        // ② 挂到「打开方式」列表。
        //    值是 REG_NONE（类型 None、数据为空）—— 这是 Windows 的约定，
        //    写别的类型虽然也能被识别，但与系统自身写法不一致。
        let (owp, _) = classes
            .create_subkey(format!(".{ext}\\OpenWithProgids"))
            .map_err(|e| format!("创建 OpenWithProgids 失败：{e}"))?;
        owp.set_raw_value(
            &prog_id,
            &RegValue {
                bytes: Vec::new(),
                vtype: REG_NONE,
            },
        )
        .map_err(|e| format!("写入 OpenWithProgids 失败：{e}"))?;

        Ok(())
    }

    /// 取消注册（删掉本程序加的两处痕迹，不动系统的其他关联）。
    ///
    /// 单扩展名入口 = [`unregister_silent`] + 一次 shell 通知。
    pub fn unregister(ext: &str) -> Result<(), String> {
        unregister_silent(ext)?;
        notify_shell();
        Ok(())
    }

    /// 同 [`unregister`]，但**不通知 shell**（批量时统一通知，理由见 [`register_silent`]）。
    pub fn unregister_silent(ext: &str) -> Result<(), String> {
        let ext = normalize_ext(ext).ok_or_else(|| format!("非法扩展名：{ext}"))?;
        let prog_id = format!("{PROG_ID_PREFIX}.{ext}");
        let classes = classes()?;

        if let Ok(owp) =
            classes.open_subkey_with_flags(format!(".{ext}\\OpenWithProgids"), KEY_ALL_ACCESS)
        {
            // 值可能本来就不在，删除失败不算错
            let _ = owp.delete_value(&prog_id);
        }
        // 整棵 ProgID 子树删掉
        let _ = classes.delete_subkey_all(&prog_id);

        Ok(())
    }

    /// 是否已注册进「打开方式」列表
    pub fn is_registered(ext: &str) -> bool {
        let Some(ext) = normalize_ext(ext) else {
            return false;
        };
        let prog_id = format!("{PROG_ID_PREFIX}.{ext}");
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(format!("Software\\Classes\\.{ext}\\OpenWithProgids"))
            .and_then(|k| k.get_raw_value(&prog_id))
            .is_ok()
    }
}

// ─────────────────────────── 非 Windows 空实现 ───────────────────────────

#[cfg(not(windows))]
mod imp {
    pub fn register(_ext: &str, _display_name: &str) -> Result<(), String> {
        Err("文件关联仅支持 Windows".to_string())
    }
    pub fn register_silent(_ext: &str, _display_name: &str) -> Result<(), String> {
        Err("文件关联仅支持 Windows".to_string())
    }
    pub fn unregister(_ext: &str) -> Result<(), String> {
        Err("文件关联仅支持 Windows".to_string())
    }
    pub fn unregister_silent(_ext: &str) -> Result<(), String> {
        Err("文件关联仅支持 Windows".to_string())
    }
    pub fn is_registered(_ext: &str) -> bool {
        false
    }
    pub fn notify_shell() {}
}

pub use imp::{
    is_registered, notify_shell, register, register_silent, unregister, unregister_silent,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 扩展名规范化去点并转小写() {
        assert_eq!(normalize_ext("JPG").as_deref(), Some("jpg"));
        assert_eq!(normalize_ext(".PNG").as_deref(), Some("png"));
        assert_eq!(normalize_ext("  Gif ").as_deref(), Some("gif"));
    }

    /// ⚠️ 扩展名会被拼进注册表路径，必须挡住路径穿越与空值。
    #[test]
    fn 扩展名拒绝路径穿越与非法字符() {
        for bad in [
            "",
            "  ",
            ".",
            "..",
            "....",
            "a\\b",
            "a/b",
            "a.b",
            "a b",
            "verylongext",
            "图片",
            "jpg\u{0}",
        ] {
            assert!(
                normalize_ext(bad).is_none(),
                "应拒绝非法扩展名：{bad:?}"
            );
        }
    }

    #[test]
    fn 边界长度刚好可接受() {
        assert!(normalize_ext("abcdefgh").is_some(), "8 位应接受");
        assert!(normalize_ext("abcdefghi").is_none(), "9 位应拒绝");
    }

    /// `PROG_ID_PREFIX` 决定写进注册表的值名。改它会让**已经关联过的用户**
    /// 出现「旧 ProgID 残留 + 勾选状态丢失」，所以钉住它。
    #[test]
    fn ProgID前缀保持不变() {
        assert_eq!(PROG_ID_PREFIX, "PurePhotoExplorer");
    }

    /// 「全选」走的正是 `register` / `unregister` 这条路径（批量命令只是把它们
    /// 循环调用、合并成一次 shell 通知），所以这里直接验证
    /// 写入 → 查得到 → 撤销 → 查不到。
    ///
    /// ⚠️ 会真的写 `HKCU\Software\Classes`。用**自定义扩展名 `pvtst`**
    /// （不与任何真实格式冲突，也不会动用户的 .jpg 关联），且结束时清理干净。
    #[cfg(windows)]
    #[test]
    fn 注册与注销往返一致() {
        let ext = "pvtst";
        // 起点先清一次：上次测试异常中断可能留下痕迹
        let _ = unregister(ext);
        assert!(!is_registered(ext), "起点应是未注册");

        register(ext, "PurePhotoExplorer 测试").expect("注册应成功");
        assert!(is_registered(ext), "注册后 is_registered 应为 true");

        unregister(ext).expect("注销应成功");
        assert!(!is_registered(ext), "注销后 is_registered 应为 false");
    }

    /// 注销一个**从未注册过**的扩展名不该报错 ——
    /// 「全不选」在某些格式本来就没关联时会走到这条路。
    #[cfg(windows)]
    #[test]
    fn 注销未注册的扩展名不报错() {
        let ext = "pvnon";
        let _ = unregister(ext);
        assert!(unregister(ext).is_ok(), "注销不存在的东西应视为成功");
    }

    /// 非法扩展名必须被挡在注册表操作之前（防止路径穿越写进别的键）。
    #[cfg(windows)]
    #[test]
    fn 非法扩展名注册被拒绝() {
        assert!(register("../evil", "x").is_err());
        assert!(unregister("../evil").is_err());
        assert!(!is_registered("../evil"));
    }
}
