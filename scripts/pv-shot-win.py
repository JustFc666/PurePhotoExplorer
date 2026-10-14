"""抓取 PurePhotoExplorer **窗口自身**客户区（不依赖前台/不被遮挡）。

⚠️ 用 PrintWindow(PW_CLIENTONLY|PW_RENDERFULLCONTENT)：
  - WebView2 是 DirectComposition 渲染，普通 BitBlt 抓窗口会得到黑屏，
    必须带 PW_RENDERFULLCONTENT (0x2) 才会让系统走「让窗口自己重绘」的路径。
  - 屏幕外的部分也能拿到（普通 BitBlt 不行）。

用法：python pv-shot-win.py <out.png>
"""
import ctypes
import ctypes.wintypes as W
import sys
from PIL import Image

u = ctypes.windll.user32
g = ctypes.windll.gdi32
u.SetProcessDPIAware()

hwnd = int(sys.argv[2]) if len(sys.argv) > 2 else 0
if not hwnd:
    found = []
    @ctypes.WINFUNCTYPE(ctypes.c_bool, W.HWND, W.LPARAM)
    def _cb(h, _lp):
        buf = ctypes.create_unicode_buffer(512)
        u.GetWindowTextW(h, buf, 512)
        if 'purephotoexplorer' in buf.value.lower() and u.IsWindowVisible(h):
            found.append(h)
        return True
    u.EnumWindows(_cb, 0)
    hwnd = found[0]

cr = W.RECT()
u.GetClientRect(hwnd, ctypes.byref(cr))
w, h = cr.right, cr.bottom

hdc = u.GetDC(hwnd)
mdc = g.CreateCompatibleDC(hdc)
bmp = g.CreateCompatibleBitmap(hdc, w, h)
g.SelectObject(mdc, bmp)
# PW_CLIENTONLY=1 | PW_RENDERFULLCONTENT=2
ok = u.PrintWindow(hwnd, mdc, 3)

class BITMAPINFOHEADER(ctypes.Structure):
    _fields_ = [('biSize', W.DWORD), ('biWidth', ctypes.c_long), ('biHeight', ctypes.c_long),
                ('biPlanes', W.WORD), ('biBitCount', W.WORD), ('biCompression', W.DWORD),
                ('biSizeImage', W.DWORD), ('biXPelsPerMeter', ctypes.c_long),
                ('biYPelsPerMeter', ctypes.c_long), ('biClrUsed', W.DWORD), ('biClrImportant', W.DWORD)]

bi = BITMAPINFOHEADER()
bi.biSize = ctypes.sizeof(BITMAPINFOHEADER)
bi.biWidth = w
bi.biHeight = -h  # top-down
bi.biPlanes = 1
bi.biBitCount = 32
bi.biCompression = 0

buf = ctypes.create_string_buffer(w * h * 4)
g.GetDIBits(mdc, bmp, 0, h, buf, ctypes.byref(bi), 0)
img = Image.frombuffer('RGBA', (w, h), buf, 'raw', 'BGRA', 0, 1).convert('RGB')
img.save(sys.argv[1])
g.DeleteObject(bmp)
g.DeleteDC(mdc)
u.ReleaseDC(hwnd, hdc)
print(f'PrintWindow ok={ok} {w}x{h} -> {sys.argv[1]}')
