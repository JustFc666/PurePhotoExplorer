"""向 PurePhotoExplorer 窗口发送模拟输入（SendInput）。

## 为什么必须是 SendInput（2026-10-07 踩坑，勿走弯路）
`PostMessageW` / `mouse_event` / `keybd_event` 对 WebView2 **全部无效** ——
WebView2 有自己的输入线程模型，只有 SendInput 走系统输入队列才进得去页面。

鼠标坐标用 0–65535 归一化 + `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_MOVE`。

用法：
    python pv-input.py click 800 500
    python pv-input.py move  800 500
    python pv-input.py drag  700 400 900 600
    python pv-input.py key   a
    python pv-input.py wheel -480 800 500
    python pv-input.py text  16:9        # 逐字符按键（暂仅支持可打印 ASCII）
"""
import ctypes
import ctypes.wintypes as W
import sys
import time

user32 = ctypes.windll.user32
user32.SetProcessDPIAware()

INPUT_MOUSE = 0
INPUT_KEYBOARD = 1
KEYEVENTF_KEYUP = 0x0002
KEYEVENTF_UNICODE = 0x0004
MOUSEEVENTF_MOVE = 0x0001
MOUSEEVENTF_ABSOLUTE = 0x8000
MOUSEEVENTF_LEFTDOWN = 0x0002
MOUSEEVENTF_LEFTUP = 0x0004
MOUSEEVENTF_WHEEL = 0x0800


class MOUSEINPUT(ctypes.Structure):
    _fields_ = [("dx", W.LONG), ("dy", W.LONG), ("mouseData", W.DWORD),
                ("dwFlags", W.DWORD), ("time", W.DWORD),
                ("dwExtraInfo", ctypes.POINTER(ctypes.c_ulong))]


class KEYBDINPUT(ctypes.Structure):
    _fields_ = [("wVk", W.WORD), ("wScan", W.WORD), ("dwFlags", W.DWORD),
                ("time", W.DWORD), ("dwExtraInfo", ctypes.POINTER(ctypes.c_ulong))]


class HARDWAREINPUT(ctypes.Structure):
    _fields_ = [("uMsg", W.DWORD), ("wParamL", W.WORD), ("wParamH", W.WORD)]


class _INPUTunion(ctypes.Union):
    _fields_ = [("mi", MOUSEINPUT), ("ki", KEYBDINPUT), ("hi", HARDWAREINPUT)]


class INPUT(ctypes.Structure):
    _fields_ = [("type", W.DWORD), ("union", _INPUTunion)]


def _send(*inputs):
    n = len(inputs)
    arr = (INPUT * n)(*inputs)
    user32.SendInput(n, arr, ctypes.sizeof(INPUT))


def _screen_size():
    return user32.GetSystemMetrics(0), user32.GetSystemMetrics(1)


def _abs(x: int, y: int):
    w, h = _screen_size()
    return int(x * 65535 / (w - 1)), int(y * 65535 / (h - 1))


def _mouse(flags, x=0, y=0, data=0):
    ax, ay = _abs(x, y)
    mi = MOUSEINPUT(ax, ay, data, flags, 0, None)
    _send(INPUT(INPUT_MOUSE, _INPUTunion(mi=mi)))


def move(x, y):
    _mouse(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE, x, y)
    time.sleep(0.08)


def click(x, y):
    move(x, y)
    _mouse(MOUSEEVENTF_LEFTDOWN, x, y)
    time.sleep(0.05)
    _mouse(MOUSEEVENTF_LEFTUP, x, y)
    time.sleep(0.25)


def drag(x1, y1, x2, y2, steps=24):
    move(x1, y1)
    _mouse(MOUSEEVENTF_LEFTDOWN, x1, y1)
    time.sleep(0.08)
    for i in range(1, steps + 1):
        t = i / steps
        move(int(x1 + (x2 - x1) * t), int(y1 + (y2 - y1) * t))
    time.sleep(0.06)
    _mouse(MOUSEEVENTF_LEFTUP, x2, y2)
    time.sleep(0.25)


def wheel(delta, x, y):
    move(x, y)
    _mouse(MOUSEEVENTF_WHEEL, x, y, delta)
    time.sleep(0.15)


VK = {'esc': 0x1B, 'enter': 0x0D, 'space': 0x20, 'tab': 0x09}


def key(name: str):
    name = name.lower()
    vk = VK.get(name)
    if vk is None:
        if len(name) == 1:
            vk = user32.VkKeyScanW(ord(name)) & 0xFF
        else:
            raise SystemExit(f"未知按键 {name}")
    down = KEYBDINPUT(vk, 0, 0, 0, None)
    up = KEYBDINPUT(vk, 0, KEYEVENTF_KEYUP, 0, None)
    _send(INPUT(INPUT_KEYBOARD, _INPUTunion(ki=down)))
    time.sleep(0.04)
    _send(INPUT(INPUT_KEYBOARD, _INPUTunion(ki=up)))
    time.sleep(0.25)


def text(s: str):
    for ch in s:
        code = ord(ch)
        down = KEYBDINPUT(0, code, KEYEVENTF_UNICODE, 0, None)
        up = KEYBDINPUT(0, code, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP, 0, None)
        _send(INPUT(INPUT_KEYBOARD, _INPUTunion(ki=down)))
        _send(INPUT(INPUT_KEYBOARD, _INPUTunion(ki=up)))
        time.sleep(0.06)
    time.sleep(0.2)


def main():
    cmd = sys.argv[1]
    a = [int(v) for v in sys.argv[2:]] if cmd != 'key' and cmd != 'text' else sys.argv[2:]
    if cmd == 'click':
        click(a[0], a[1])
    elif cmd == 'move':
        move(a[0], a[1])
    elif cmd == 'drag':
        drag(*a)
    elif cmd == 'key':
        key(a[0])
    elif cmd == 'text':
        text(a[0])
    elif cmd == 'wheel':
        wheel(int(a[0]), int(a[1]), int(a[2]))
    else:
        raise SystemExit(f"未知命令 {cmd}")


if __name__ == '__main__':
    main()
