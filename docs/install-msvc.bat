@echo off
REM ============================================================
REM  PurePhotoExplorer - 安装 MSVC C++ 构建工具
REM  用途：编译 LibRaw（RAW 解码）需要 C++ 编译器 + Windows SDK
REM
REM  用法：右键本文件 -> 以管理员身份运行
REM  下载量约 6-8 GB，装在 VS Build Tools 目录，不污染系统其他部分
REM ============================================================

echo.
echo 正在检查管理员权限...
net session >nul 2>&1
if errorlevel 1 (
    echo [错误] 请右键本文件，选择 "以管理员身份运行"
    echo.
    pause
    exit /b 1
)
echo [OK] 管理员权限已确认
echo.

set VS_PATH=C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools
set SETUP=C:\Program Files (x86)\Microsoft Visual Studio\Installer\setup.exe

if not exist "%SETUP%" (
    echo [错误] 找不到 Visual Studio Installer
    echo 请先安装 Visual Studio Build Tools 引导程序：
    echo https://visualstudio.microsoft.com/visual-cpp-build-tools/
    echo.
    pause
    exit /b 1
)

echo 正在添加 C++ 桌面开发工作负载...
echo 包含：MSVC 编译器 + Windows SDK + CMake
echo 这一步会下载约 6-8 GB，请耐心等待
echo.

"%SETUP%" modify ^
    --installPath "%VS_PATH%" ^
    --add Microsoft.VisualStudio.Workload.VCTools ^
    --add Microsoft.VisualStudio.Component.VC.Tools.x86.x64 ^
    --add Microsoft.VisualStudio.Component.Windows11SDK.22621 ^
    --includeRecommended ^
    --passive ^
    --norestart

echo.
echo ============================================================
echo  安装程序已退出。正在验证...
echo ============================================================
echo.

REM 加载 vcvars 环境并检查
call "%VS_PATH%\VC\Auxiliary\Build\vcvars64.bat" >nul 2>&1

where cl >nul 2>&1
if errorlevel 1 (
    echo [警告] 未检测到 cl.exe，安装可能未完成
    echo 请查看安装日志：%TEMP%\dd_setup*.log
) else (
    echo [OK] MSVC 编译器就绪
    cl 2>&1 | findstr /C:"Version"
)

where cmake >nul 2>&1
if errorlevel 1 (
    echo [警告] 未检测到 cmake，安装可能未完成
) else (
    echo [OK] CMake 就绪
    cmake --version | findstr /C:"version"
)

echo.
echo 完成。按任意键退出。
pause >nul
