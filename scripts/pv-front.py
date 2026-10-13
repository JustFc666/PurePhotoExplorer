"""把 PurePhotoExplorer 窗口置前并报告客户区几何，可选设为固定尺寸。

用法：python pv-front.py [outerW outerH]
"""
import ctypes
import ctypes.wintypes as W
import sys
import time

u = ctypes.windll.user32
u.SetProcessDPIAware()


def find_window(title_part="PurePhotoExplorer"):
    found = []

    @ctypes.WINFUNCTYPE(ctypes.c_bool, W.HWND, W.LPARAM)
    def _cb(hwnd, _lp):
        buf = ctypes.create_unicode_buffer(512)
        u.GetWindowTextW(hwnd, buf, 512)
        if title_part.lower() in buf.value.lower() and u.IsWindowVisible(hwnd):
            found.append(hwnd)
        return True

    u.EnumWindows(_cb, 0)
    return found[0] if found else 0


hwnd = find_window()
if not hwnd:
    raise SystemExit("未找到 PurePhotoExplorer 窗口")

# 前置（绕过前台锁定策略）
fg = u.GetForegroundWindow()
tf = u.GetWindowThreadProcessId(fg, None)
tc = ctypes.windll.kernel32.GetCurrentThreadId()
u.AttachThreadInput(tc, tf, True)
if u.IsZoomed(hwnd):
    u.ShowWindow(hwnd, 9)  # SW_RESTORE
u.ShowWindow(hwnd, 9)
u.SetForegroundWindow(hwnd)
u.BringWindowToTop(hwnd)
u.AttachThreadInput(tc, tf, False)
time.sleep(0.5)

if len(sys.argv) >= 3:
    ow, oh = int(sys.argv[1]), int(sys.argv[2])
    u.SetWindowPos(hwnd, 0, 489 - 9, 201, ow, oh, 0x0004 | 0x0010)
    time.sleep(0.5)

cr = W.RECT()
u.GetClientRect(hwnd, ctypes.byref(cr))
pt = W.POINT(0, 0)
u.ClientToScreen(hwnd, ctypes.byref(pt))
wr = W.RECT()
u.GetWindowRect(hwnd, ctypes.byref(wr))
print(f"hwnd={hwnd}")
print(f"client {cr.right}x{cr.bottom} @ screen ({pt.x},{pt.y})")
print(f"outer  ({wr.left},{wr.top}) {wr.right - wr.left}x{wr.bottom - wr.top}")
print(f"foreground={u.GetForegroundWindow() == hwnd}")
