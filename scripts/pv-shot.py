"""抓取 PurePhotoExplorer 窗口客户区截图（用于回归验证存档）。

用法：python pv-shot.py <输出相对路径>

设计说明：
- 抓的是**屏幕坐标系下的客户区矩形**（GetClientRect + ClientToScreen），
  而不是窗口自身 DC —— 后者在窗口被遮挡或最大化超出屏幕时会给出黑块。
- 走 BitBlt(SRCCOPY) 从屏幕 DC 拷，反映的是**真实观感**。
"""
import ctypes
import ctypes.wintypes as W
import sys

from PIL import Image

user32 = ctypes.windll.user32
gdi32 = ctypes.windll.gdi32
user32.SetProcessDPIAware()


class BITMAPINFOHEADER(ctypes.Structure):
    _fields_ = [
        ("biSize", W.DWORD), ("biWidth", W.LONG), ("biHeight", W.LONG),
        ("biPlanes", W.WORD), ("biBitCount", W.WORD), ("biCompression", W.DWORD),
        ("biSizeImage", W.DWORD), ("biXPelsPerMeter", W.LONG),
        ("biYPelsPerMeter", W.LONG), ("biClrUsed", W.DWORD), ("biClrImportant", W.DWORD),
    ]


class BITMAPINFO(ctypes.Structure):
    _fields_ = [("bmiHeader", BITMAPINFOHEADER), ("bmiColors", W.DWORD * 3)]


def find_window(title_part: str = "PurePhotoExplorer") -> int:
    found = []

    @ctypes.WINFUNCTYPE(ctypes.c_bool, W.HWND, W.LPARAM)
    def _cb(hwnd, _lparam):
        buf = ctypes.create_unicode_buffer(512)
        user32.GetWindowTextW(hwnd, buf, 512)
        if title_part.lower() in buf.value.lower() and user32.IsWindowVisible(hwnd):
            found.append(hwnd)
        return True

    user32.EnumWindows(_cb, 0)
    return found[0] if found else 0


def client_rect_on_screen(hwnd: int):
    cr = W.RECT()
    user32.GetClientRect(hwnd, ctypes.byref(cr))
    pt = W.POINT(0, 0)
    user32.ClientToScreen(hwnd, ctypes.byref(pt))
    return pt.x, pt.y, cr.right, cr.bottom


def grab(x: int, y: int, w: int, h: int) -> Image.Image:
    hdc = user32.GetDC(0)
    mem = gdi32.CreateCompatibleDC(hdc)
    bmp = gdi32.CreateCompatibleBitmap(hdc, w, h)
    gdi32.SelectObject(mem, bmp)
    # SRCCOPY
    gdi32.BitBlt(mem, 0, 0, w, h, hdc, x, y, 0x00CC0020)

    bi = BITMAPINFO()
    bi.bmiHeader.biSize = ctypes.sizeof(BITMAPINFOHEADER)
    bi.bmiHeader.biWidth = w
    bi.bmiHeader.biHeight = -h  # 负数 = 自上而下
    bi.bmiHeader.biPlanes = 1
    bi.bmiHeader.biBitCount = 32
    bi.bmiHeader.biCompression = 0

    buf = ctypes.create_string_buffer(w * h * 4)
    gdi32.GetDIBits(mem, bmp, 0, h, buf, ctypes.byref(bi), 0)
    img = Image.frombuffer("RGBA", (w, h), buf, "raw", "BGRA", 0, 1).convert("RGB")

    gdi32.DeleteObject(bmp)
    gdi32.DeleteDC(mem)
    user32.ReleaseDC(0, hdc)
    return img


def main() -> None:
    out = sys.argv[1] if len(sys.argv) > 1 else "docs/verification/pv.png"
    hwnd = find_window()
    if not hwnd:
        raise SystemExit("未找到 PurePhotoExplorer 窗口")
    x, y, w, h = client_rect_on_screen(hwnd)
    print(f"hwnd={hwnd} client@({x},{y}) {w}x{h}")
    grab(x, y, w, h).save(out)
    print(f"saved {out}")


if __name__ == "__main__":
    main()
