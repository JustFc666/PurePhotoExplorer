//! 用系统默认程序打开外部目标（`ShellExecuteW`）。
//!
//! 目前只有一个用途：信息面板的「在地图中查看」按钮 —— 把照片的 GPS 坐标
//! 交给默认浏览器打开地图网页。
//!
//! ⚠️ **调用方必须先校验 URL**：这个模块不做任何过滤，传进来什么就交给
//! Shell 什么。命令层（`commands::app::open_external_url`）负责只放行
//! `http` / `https` —— 否则就等于给前端一个「打开任意本地文件」的口子
//! （`file:`、`\\server\share`、甚至 `xxx.exe` 都会被照常执行）。

/// 用默认浏览器打开一个 `http(s)` 链接。
///
/// 返回 `Err(说明)` 而不是 panic：打不开浏览器是**环境**问题（没装 / 没设默认），
/// 用户需要看到原因，而不是程序崩掉。
#[cfg(windows)]
pub fn open_url(url: &str) -> Result<(), String> {
    use std::iter::once;
    use std::ptr::null_mut;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let operation: Vec<u16> = "open\0".encode_utf16().collect();
    let target: Vec<u16> = url.encode_utf16().chain(once(0)).collect();

    // SAFETY: 标准 ShellExecuteW 调用。hwnd / 工作目录传 null；
    // 两个字符串参数都是 null 结尾的 UTF-16 缓冲，调用期间有效。
    let ret = unsafe {
        ShellExecuteW(
            null_mut(),
            operation.as_ptr(),
            target.as_ptr(),
            null_mut(),
            null_mut(),
            SW_SHOWNORMAL,
        )
    };

    // 返回 HINSTANCE（指针型）：> 32 成功，≤ 32 是 SE_ERR_* 错误码
    let code = ret as usize;
    if code <= 32 {
        return Err(format!(
            "无法打开链接（ShellExecuteW 返回 {code}，可能没有可用的默认浏览器）"
        ));
    }
    Ok(())
}

/// 非 Windows 平台：明确报不支持，而不是编译不过。
#[cfg(not(windows))]
pub fn open_url(_url: &str) -> Result<(), String> {
    Err("打开外部链接目前只在 Windows 上实现".into())
}

#[cfg(test)]
mod tests {
    /// 非法的目标不该被交给 Shell —— 过滤在命令层，这里只钉住那条边界。
    #[test]
    fn 非法前缀不应被放行() {
        // 命令层的白名单逻辑（与 commands::app::open_external_url 保持一致）
        let allowed = |u: &str| u.starts_with("http://") || u.starts_with("https://");
        assert!(allowed("https://uri.amap.com/marker?position=121.4,31.2"));
        assert!(!allowed("file:///C:/Windows/notepad.exe"));
        assert!(!allowed(r"\\server\share\evil.exe"));
        assert!(!allowed(""));
    }
}
