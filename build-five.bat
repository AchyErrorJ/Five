@echo off
setlocal EnableDelayedExpansion

title Five — Build Script
color 0A
:: Only try to resize if we're in a real console
mode con: cols=100 lines=30 2>nul || echo [INFO] Running in non-console mode

:: ---------------------------------------------------------------------------
:: build-five.bat — Build Five daemon for Windows
:: Usage: build-five.bat [--release] [--clean]
:: ---------------------------------------------------------------------------

set "FIVE_DIR=%~dp0"
set "BUILD_TYPE=debug"
set "DO_CLEAN=0"

:: Parse arguments
:PARSE_ARGS
if "%~1"=="" goto :END_PARSE
if /i "%~1"=="--release" (
    set "BUILD_TYPE=release"
    shift
    goto :PARSE_ARGS
)
if /i "%~1"=="--clean" (
    set "DO_CLEAN=1"
    shift
    goto :PARSE_ARGS
)
if /i "%~1"=="-h" (
    goto :SHOW_HELP
)
if /i "%~1"=="--help" (
    goto :SHOW_HELP
)
echo [WARN] Unknown argument: %~1
shift
goto :PARSE_ARGS

:END_PARSE

:: Check for cargo
where cargo >nul 2>&1
if %ERRORLEVEL% NEQ 0 (
    echo [ERROR] cargo not found.
    echo.
    echo Please install Rust from https://rustup.rs/
    echo After installation, restart your terminal and run this script again.
    pause
    exit /b 1
)

:: Check for cmake (required for whisper.cpp)
where cmake >nul 2>&1
if %ERRORLEVEL% NEQ 0 (
    echo [WARN] cmake not found in PATH.
    echo        The build may fail without it.
    echo.
    echo Install with: choco install cmake
    echo Or download from: https://cmake.org/download/
    echo.
    set /p CONTINUE="Continue anyway? (Y/N): "
    if /i not "%CONTINUE%"=="Y" (
        exit /b 1
    )
)

echo.
echo    ╔══════════════════════════════════════════════════════════════════════╗
echo    ║                                                                      ║
echo    ║   🔨 FIVE — Build Script                                             ║
echo    ║                                                                      ║
echo    ╚══════════════════════════════════════════════════════════════════════╝
echo.

if "%DO_CLEAN%"=="1" (
    echo [..] Cleaning previous build...
    cargo clean
    echo [OK] Clean complete.
    echo.
)

echo Build configuration:
echo   Type: %BUILD_TYPE%
echo   Directory: %FIVE_DIR%
echo.

if "%BUILD_TYPE%"=="release" (
    echo [..] Running: cargo build --release
    echo.
    echo This may take 5-15 minutes on first build.
    echo Subsequent builds will be faster.
    echo.
    cargo build --release
) else (
    echo [..] Running: cargo build
    echo.
    cargo build
)

if %ERRORLEVEL% NEQ 0 (
    echo.
    echo    ╔══════════════════════════════════════════════════════════════════════╗
    echo    ║  BUILD FAILED                                                        ║
    echo    ╚══════════════════════════════════════════════════════════════════════╝
    echo.
    echo Common issues on Windows:
    echo.
    echo 1. Missing Visual Studio Build Tools with C++ support
    echo    Install from: https://visualstudio.microsoft.com/downloads/
    echo    Select: Desktop development with C++
    echo.
    echo 2. Missing cmake
    echo    Install with: choco install cmake
    echo.
    echo 3. Path too long (Windows MAX_PATH limit)
    echo    Try moving the project to a shorter path like C:\five\
    echo.
    echo 4. Whisper.cpp native build failed
    echo    Check that you have Visual Studio Build Tools installed
    echo.
    pause
    exit /b 1
)

echo.
echo    ╔══════════════════════════════════════════════════════════════════════╗
echo    ║  BUILD SUCCESSFUL!                                                   ║
echo    ╚══════════════════════════════════════════════════════════════════════╝
echo.

if "%BUILD_TYPE%"=="release" (
    echo Executable: %FIVE_DIR%target\release\five-daemon.exe
    echo.
    echo To start Five, run:
    echo   .\start-five.bat
    echo.
    echo Or directly:
    echo   .\target\release\five-daemon.exe --config config.windows.yaml listen
) else (
    echo Executable: %FIVE_DIR%target\debug\five-daemon.exe
    echo.
    echo [NOTE] Debug builds may crash due to MSVC CRT debug asserts in whisper.
    echo       For stable operation, build with --release flag.
    echo.
    echo To start Five, run:
    echo   .\start-five.bat
)

echo.
echo Next steps:
echo   1. Make sure you have config.windows.yaml configured
echo   2. Download models to the models\ directory
echo   3. Run: .\start-five.bat
echo.
pause

goto :EOF

:: ---------------------------------------------------------------------------
:: SHOW_HELP
:: ---------------------------------------------------------------------------
:SHOW_HELP
echo.
echo Build Five daemon for Windows
echo.
echo Usage: build-five.bat [OPTIONS]
echo.
echo Options:
echo   --release    Build optimized release version (recommended)
echo   --clean      Clean previous build before building
echo   -h, --help   Show this help message
echo.
echo Examples:
echo   build-five.bat                 Build debug version
echo   build-five.bat --release       Build release version
echo   build-five.bat --clean --release   Clean and build release
echo.
goto :EOF
