//! Windows 窗口最大化（绕开 `WS_EX_WINDOWEDGE` 导致的错位）。
//!
//! ## 背景：为什么不能直接用系统最大化
//!
//! tao 的 `window_state::apply_diff()` 在窗口任何 flag 变化时都会用
//! `to_window_styles()` **完整重写** `GWL_STYLE` / `GWL_EXSTYLE`，而该函数
//! 无条件写入 `WS_EX_WINDOWEDGE`（见 tao-0.37.1 源码 239 行、413 行）。
//! 因此**无法把它从窗口上摘掉** —— 清掉之后瞬间就会被写回。
//!
//! ## 真正的坑：外框对了，客户区没对
//!
//! `WS_EX_WINDOWEDGE` 会在窗口「外框」内侧留一圈**不可见的非客户区**。
//! 本机 DPI 120 实测四边偏移（外框 → 客户区右上角起点）并不对称：
//!
//! ```text
//! L=9  T=0  R=9  B=9        （物理像素）
//! ```
//!
//! 上边为 0 是因为 `decorations:false` 已经吃掉了整条标题栏，
//! 左右下三边则仍是 `WS_EX_WINDOWEDGE` 残留的 9px。
//!
//! 于是「把窗口外框摆到工作区」的结果是：
//!
//! ```text
//! GetWindowRect = (0,0) 2560×1402   ← 看外框完全正确 ✅
//! 客户区实际   = (9,0) 2542×1393    ← 左边 9px、右 9px、下 9px 空间是空的 ❌
//! ```
//!
//! 肉眼效果就是「最大化了但四周没覆盖全」。
//!
//! ## 本模块的做法
//!
//! 1. 查询系统「工作区矩形」(`SPI_GETWORKAREA`，已自动排除任务栏)；
//! 2. **动态测量**当前窗口的四边 non-client 偏移（不硬编码 9px，
//!    换 DPI / 换主题 / 关掉 decorations 时都能自适应）；
//! 3. 按偏移把目标外框**向外扩**，再用 `SetWindowPos` 摆上去 ——
//!    这样客户区恰好等于工作区。
//!
//! 全程不碰 `ShowWindow(SW_MAXIMIZE)`，也就不会被系统的最大化布局覆盖。
//!
//! 代价：窗口不再带 `IsZoomed` 标记，最大化语义由前端自己记账
//! （另见 `useWindowDrag`）。贴边分屏（Aero Snap）同样失效 ——
//! 这是 `decorations: false` 自绘方案固有的取舍。

#![cfg(windows)]

use tauri::WebviewWindow;
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetWindowRect, IsZoomed, SetWindowPos, ShowWindow, SystemParametersInfoW,
    SPI_GETWORKAREA, SWP_NOACTIVATE, SWP_NOZORDER, SW_RESTORE,
};

/// 窗口四边的 non-client 偏移（物理像素）：外框 → 客户区。
#[derive(Clone, Copy, Default)]
struct NcInsets {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

/// 测量窗口四边的 non-client 偏移。
///
/// 用 `GetWindowRect`（外框）与 `ClientToScreen(0,0)`（客户区屏幕原点）
/// 之差算出左/上，再由 `GetClientRect` 反推右/下。
fn nc_insets(hwnd: HWND) -> NcInsets {
    let mut outer = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    let mut client = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    let mut origin = POINT { x: 0, y: 0 };
    unsafe {
        if GetWindowRect(hwnd, &mut outer) == 0
            || GetClientRect(hwnd, &mut client) == 0
            || ClientToScreen(hwnd, &mut origin) == 0
        {
            return NcInsets::default();
        }
    }
    let outer_w = outer.right - outer.left;
    let outer_h = outer.bottom - outer.top;
    let left = origin.x - outer.left;
    let top = origin.y - outer.top;
    NcInsets {
        left,
        top,
        right: outer_w - left - (client.right - client.left),
        bottom: outer_h - top - (client.bottom - client.top),
    }
}

/// 工作区矩形（物理像素，已排除任务栏）。`None` 表示查询失败。
pub fn work_area() -> Option<(i32, i32, i32, i32)> {
    let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            (&mut rc as *mut RECT).cast(),
            0,
        )
    };
    if ok == 0 {
        return None;
    }
    Some((rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top))
}

/// 当前窗口**客户区**矩形（物理像素，屏幕坐标）。
///
/// 这是 WebView2 真正渲染的那块区域。`fill_work_area` 铺的就是它，
/// 所以「最大化前的还原态」也应当用同样的度量来记 —— 否则
/// 「最大化 → 还原 → 再最大化」会因为外框/客户区两套坐标混用而漂移。
pub fn client_rect(window: &WebviewWindow) -> Option<(i32, i32, i32, i32)> {
    let hwnd = window.hwnd().ok()?.0 as HWND;
    let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    let mut origin = POINT { x: 0, y: 0 };
    unsafe {
        if GetClientRect(hwnd, &mut rc) == 0 || ClientToScreen(hwnd, &mut origin) == 0 {
            return None;
        }
    }
    Some((origin.x, origin.y, rc.right - rc.left, rc.bottom - rc.top))
}

/// 把窗口**客户区**精确铺满工作区。成功返回 `true`。
///
/// 内部会把目标外框按实测的非客户区偏移向外扩，
/// 因此调用方传的就是「希望客户区占满的那块矩形」。
pub fn fill_work_area(window: &WebviewWindow) -> bool {
    let Some((wx, wy, ww, wh)) = work_area() else {
        return false;
    };
    let Ok(hwnd) = window.hwnd() else { return false };
    let hwnd = hwnd.0 as HWND;

    unsafe {
        // 若窗口处于系统最大化态，先还原，否则 SetWindowPos 会被忽略
        if IsZoomed(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
    }

    // 注意：必须在「还原之后」测偏移 —— 最大化态下 GetClientRect 不同。
    let ins = nc_insets(hwnd);
    let (tx, ty) = (wx - ins.left, wy - ins.top);
    let (tw, th) = (ww + ins.left + ins.right, wh + ins.top + ins.bottom);

    unsafe {
        SetWindowPos(hwnd, core::ptr::null_mut(), tx, ty, tw, th, SWP_NOZORDER | SWP_NOACTIVATE)
            != 0
    }
}

/// 把窗口**客户区**恢复到指定矩形（物理像素，屏幕坐标）。
///
/// 与 [`fill_work_area`] 对称：入参是「客户区」的期望位置与尺寸，
/// 内部按实测偏移换算成外框后再 `SetWindowPos`。
pub fn restore_bounds(window: &WebviewWindow, x: i32, y: i32, w: i32, h: i32) -> bool {
    let Ok(hwnd) = window.hwnd() else { return false };
    let hwnd = hwnd.0 as HWND;

    unsafe {
        if IsZoomed(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
    }

    let ins = nc_insets(hwnd);
    let (tx, ty) = (x - ins.left, y - ins.top);
    let (tw, th) = (w + ins.left + ins.right, h + ins.top + ins.bottom);

    unsafe {
        SetWindowPos(hwnd, core::ptr::null_mut(), tx, ty, tw, th, SWP_NOZORDER | SWP_NOACTIVATE)
            != 0
    }
}
