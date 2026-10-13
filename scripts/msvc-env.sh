# PurePhotoExplorer 构建环境 —— MSVC / CMake 环境变量
#
# 用法（Git Bash）：
#   source scripts/msvc-env.sh
#
# 用法（PowerShell）：
#   . scripts\msvc-env.ps1
#
# 背景：Visual Studio 的 vcvars64.bat 需要 cmd.exe 加载，
# 而本项目的开发流程要求不走 cmd.exe。��把路径直接写进环境变量。

# ── 版本探测（避免硬编码，升级 VS 后自动跟随）──
# 用 find 定位 bin 目录，若无匹配则回退到当前已知版本

MSVC_ROOT_DEFAULT="C:/Program Files (x86)/Microsoft Visual Studio/18/BuildTools/VC/Tools/MSVC"
SDK_ROOT_DEFAULT="C:/Program Files (x86)/Windows Kits/10"

# 取版本号最高的目录
if [ -d "$MSVC_ROOT_DEFAULT" ]; then
  MSVC_VER=$(ls -1 "$MSVC_ROOT_DEFAULT" 2>/dev/null | sort -V | tail -1)
  MSVC_ROOT="$MSVC_ROOT_DEFAULT/$MSVC_VER"
fi

if [ -d "$SDK_ROOT_DEFAULT/Include" ]; then
  SDK_VER=$(ls -1 "$SDK_ROOT_DEFAULT/Include" 2>/dev/null | sort -V | tail -1)
  SDK_ROOT="$SDK_ROOT_DEFAULT"
fi

MSVC_BIN="$MSVC_ROOT/bin/Hostx64/x64"
MSVC_CMAKE="C:/Program Files (x86)/Microsoft Visual Studio/18/BuildTools/Common7/IDE/CommonExtensions/Microsoft/CMake/CMake/bin"

# ── PATH ──
export PATH="$MSVC_BIN:$MSVC_CMAKE:$SDK_ROOT/bin/$SDK_VER/x64:$PATH"

# ── INCLUDE / LIB ──
# 关键：C++ 标准库头文件（cstdio/vector/string）在 MSVC 自带目录，
# SDK 只提供 C 头与系统头，两者都要加。
export INCLUDE="$MSVC_ROOT\\include;$SDK_ROOT\\Include\\$SDK_VER\\ucrt;$SDK_ROOT\\Include\\$SDK_VER\\shared;$SDK_ROOT\\Include\\$SDK_VER\\um;$SDK_ROOT\\Include\\$SDK_VER\\winrt"
export LIB="$MSVC_ROOT\\lib\\x64;$SDK_ROOT\\Lib\\$SDK_VER\\ucrt\\x64;$SDK_ROOT\\Lib\\$SDK_VER\\um\\x64"

export WindowsSDKVersion="$SDK_VER\\"
export WindowsSdkVerBinPath="$SDK_ROOT\\bin\\$SDK_VER\\"
export WindowsSdkBinPath="$SDK_ROOT\\bin\\$SDK_VER\\"

echo "[MSVC 环境已加载]"
echo "  MSVC: $MSVC_VER"
echo "  SDK:  $SDK_VER"
echo "  cmake: $(command -v cmake 2>/dev/null || echo '未找到')"
echo "  cl:    $(command -v cl 2>/dev/null || echo '未在 PATH（用完整路径调用）')"
