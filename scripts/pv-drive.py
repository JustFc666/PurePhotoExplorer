"""驱动 PurePhotoExplorer 做一次「置顶→发键鼠→抓窗口」的原子操作序列。

用法：python pv-drive.py <actions.json>
actions 是 [{op:'key',name:'a'},{op:'click',x:..,y:..},{op:'drag',x1,..,x2,..},{op:'wait',ms:..},{op:'shot',file:'..'}]
PhotosVault 会被临时置顶以便接收 SendInput，结束自动取消置顶。
"""
import ctypes
import json
import os
import sys
import time

os.chdir(r'C:\Users\Fc\WorkBuddy\2026-10-02-22-48-04')
import importlib.util

spec = importlib.util.spec_from_file_location('pvi', 'scripts/pv-input.py')
pvi = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pvi)
spec2 = importlib.util.spec_from_file_location('pvsw', 'scripts/pv-shot-win.py')
pvsw = importlib.util.module_from_spec(spec2)

u = ctypes.windll.user32
u.SetProcessDPIAware()
HWND_TOPMOST, HWND_NOTOPMOST = -1, -2
SWP_NOMOVE, SWP_NOSIZE = 0x0002, 0x0001


def find_hwnd():
    found = []
    @ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)
    def _cb(h, _lp):
        buf = ctypes.create_unicode_buffer(512)
        u.GetWindowTextW(h, buf, 512)
        if 'purephotoexplorer' in buf.value.lower() and u.IsWindowVisible(h):
            found.append(h)
        return True
    u.EnumWindows(_cb, 0)
    return found[0]


hwnd = find_hwnd()
actions = json.load(open(sys.argv[1], encoding='utf-8'))

fg = u.GetForegroundWindow()
tf = u.GetWindowThreadProcessId(fg, None)
tc = ctypes.windll.kernel32.GetCurrentThreadId()
u.AttachThreadInput(tc, tf, True)
u.ShowWindow(hwnd, 9)
u.SetForegroundWindow(hwnd)
u.BringWindowToTop(hwnd)
u.AttachThreadInput(tc, tf, False)
u.SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE)
time.sleep(0.5)

try:
    for a in actions:
        op = a['op']
        if op == 'key':
            pvi.key(a['name'])
        elif op == 'click':
            pvi.click(a['x'], a['y'])
        elif op == 'drag':
            pvi.drag(a['x1'], a['y1'], a['x2'], a['y2'], a.get('steps', 24))
        elif op == 'wheel':
            pvi.wheel(a['delta'], a['x'], a['y'])
        elif op == 'wait':
            time.sleep(a['ms'] / 1000)
        elif op == 'shot':
            sys.argv_tmp = sys.argv
            u.SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE)
            time.sleep(0.35)
            os.system(f'"{sys.executable}" scripts/pv-shot-win.py "{a["file"]}" {hwnd}')
            u.SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE)
            time.sleep(0.25)
        time.sleep(a.get('after', 0.35))
finally:
    u.SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE)
    print('done; foreground=', u.GetForegroundWindow() == hwnd)
