@echo off
setlocal EnableDelayedExpansion

title Five — Setup Wizard
color 0E
:: Only try to resize if we're in a real console
mode con: cols=100 lines=40 2>nul || echo [INFO] Running in non-console mode

:: ---------------------------------------------------------------------------
:: setup-five.bat — First-time setup wizard for Five on Windows
:: This script checks dependencies, creates config, and guides you through setup
:: ---------------------------------------------------------------------------

set "FIVE_DIR=%~dp0"

echo.
echo    ╔══════════════════════════════════════════════════════════════════════╗
echo    ║                                                                      ║
echo    ║   🚀 FIVE — First-Time Setup Wizard                                  ║
echo    ║      Legion Go Edition                                               ║
echo    ║                                                                      ║
echo    ╚══════════════════════════════════════════════════════════════════════╝
echo.
echo This wizard will help you set up Five for the first time.
echo.
pause

:: ---------------------------------------------------------------------------
:: Step 1: Check Rust installation
:: ---------------------------------------------------------------------------
echo.
echo [STEP 1/6] Checking Rust installation...
where cargo >nul 2>&1
if %ERRORLEVEL% NEQ 0 (
    echo.
    echo    ╔══════════════════════════════════════════════════════════════════════╗
    echo    ║  Rust is NOT installed                                               ║
    echo    ╚══════════════════════════════════════════════════════════════════════╝
    echo.
    echo Five requires Rust to build.
    echo.
    echo Install Rust from: https://rustup.rs/
    echo.
    echo After installation:
    echo   1. Close this window
    echo   2. Open a NEW terminal (to get updated PATH)
    echo   3. Run setup-five.bat again
    echo.
    set /p OPEN="Open Rust download page now? (Y/N): "
    if /i "%OPEN%"=="Y" (
        start https://rustup.rs/
    )
    pause
    exit /b 1
) else (
    cargo --version
    echo [OK] Rust is installed.
)

:: ---------------------------------------------------------------------------
:: Step 2: Check cmake
:: ---------------------------------------------------------------------------
echo.
echo [STEP 2/6] Checking cmake...
where cmake >nul 2>&1
if %ERRORLEVEL% NEQ 0 (
    echo [WARN] cmake not found in PATH.
    echo.
    echo cmake is required to build whisper.cpp (speech-to-text).
    echo.
    echo Install with one of these options:
    echo   - Chocolatey: choco install cmake
    echo   - Download: https://cmake.org/download/
    echo.
    set /p INSTALL_CMAKE="Install cmake now via Chocolatey? (Y/N): "
    if /i "%INSTALL_CMAKE%"=="Y" (
        where choco >nul 2>&1
        if %ERRORLEVEL% NEQ 0 (
            echo [ERROR] Chocolatey not found. Please install it first:
            echo          https://chocolatey.org/install
            echo.
            echo Or install cmake manually from the website.
            pause
        ) else (
            echo [..] Installing cmake...
            choco install cmake -y
            hashpath
            echo [OK] cmake installed.
        )
    ) else (
        echo You can install cmake later, but the build will fail without it.
    )
) else (
    cmake --version | findstr /C:"cmake version"
    echo [OK] cmake is installed.
)

:: ---------------------------------------------------------------------------
:: Step 3: Check/create config file
:: ---------------------------------------------------------------------------
echo.
echo [STEP 3/6] Checking configuration file...
if exist "%FIVE_DIR%config.windows.yaml" (
    echo [OK] config.windows.yaml already exists.
    set /p EDIT="Edit it now? (Y/N): "
    if /i "%EDIT%"=="Y" (
        notepad "%FIVE_DIR%config.windows.yaml"
    )
) else (
    echo [!!] config.windows.yaml not found.
    if exist "%FIVE_DIR%config.example.yaml" (
        echo.
        echo Creating config.windows.yaml from config.example.yaml...
        copy "%FIVE_DIR%config.example.yaml" "%FIVE_DIR%config.windows.yaml" >nul
        echo [OK] Config file created.
        echo.
        echo IMPORTANT: Edit config.windows.yaml to configure:
        echo   - output_device: Your speaker/headphone name
        echo   - local_url: Your local LLM server URL (or disable brain)
        echo   - kimi_key: Your Kimi API key (for lesson plans)
        echo.
        set /p EDIT="Edit config.windows.yaml now? (Y/N): "
        if /i "%EDIT%"=="Y" (
            notepad "%FIVE_DIR%config.windows.yaml"
        )
    ) else (
        echo [ERROR] config.example.yaml also not found!
        echo         Cannot create configuration file.
        pause
        exit /b 1
    )
)

:: ---------------------------------------------------------------------------
:: Step 4: Check models directory
:: ---------------------------------------------------------------------------
echo.
echo [STEP 4/6] Checking model files...
set "MODELS_OK=1"

if not exist "%FIVE_DIR%models" (
    echo [!!] models\ directory not found. Creating it...
    mkdir "%FIVE_DIR%models"
    mkdir "%FIVE_DIR%models\kokoro"
)

if not exist "%FIVE_DIR%models\ggml-tiny.en.bin" (
    echo [!!] Missing Whisper model: ggml-tiny.en.bin
    echo      Download from: https://huggingface.co/ggerganov/whisper.cpp
    set "MODELS_OK=0"
) else (
    echo [OK] Whisper model found.
)

if not exist "%FIVE_DIR%models\kokoro\model.onnx" (
    echo [!!] Missing Kokoro TTS model: models\kokoro\model.onnx
    echo      Download from: https://github.com/thewh1teagle/kokoro-onnx
    set "MODELS_OK=0"
) else (
    echo [OK] Kokoro TTS model found.
)

if not exist "%FIVE_DIR%models\kokoro\voices.bin" (
    echo [!!] Missing Kokoro voices: models\kokoro\voices.bin
    set "MODELS_OK=0"
) else (
    echo [OK] Kokoro voices found.
)

if "%MODELS_OK%"=="0" (
    echo.
    echo Model files are missing. Five won't work properly without them.
    echo.
    echo Download links:
    echo   Whisper STT: https://huggingface.co/ggerganov/whisper.cpp/blob/main/ggml-tiny.en.bin
    echo   Kokoro TTS:  https://github.com/thewh1teagle/kokoro-onnx
    echo.
    set /p SKIP_MODELS="Skip for now and download later? (Y/N): "
    if /i not "%SKIP_MODELS%"=="Y" (
        start https://huggingface.co/ggerganov/whisper.cpp
        start https://github.com/thewh1teagle/kokoro-onnx
        echo.
        echo I've opened the download pages in your browser.
        echo Place downloaded files in the models\ directory.
        pause
    )
)

:: ---------------------------------------------------------------------------
:: Step 5: Build Five
:: ---------------------------------------------------------------------------
echo.
echo [STEP 5/6] Building Five...
echo.
if exist "%FIVE_DIR%target\release\five-daemon.exe" (
    echo [OK] Release binary already exists.
    set /p REBUILD="Rebuild anyway? (Y/N): "
    if /i "%REBUILD%"=="Y" (
        goto :DO_BUILD
    ) else (
        echo Skipping build.
        goto :AFTER_BUILD
    )
) else if exist "%FIVE_DIR%target\debug\five-daemon.exe" (
    echo [INFO] Debug binary exists. Release build recommended for stability.
    set /p BUILD_RELEASE="Build release version now? (Y/N): "
    if /i "%BUILD_RELEASE%"=="Y" (
        goto :DO_BUILD
    ) else (
        echo Skipping build.
        goto :AFTER_BUILD
    )
) else (
    echo No binary found. Building is required.
    :DO_BUILD
    echo.
    echo [..] Running: cargo build --release
    echo      This may take 5-15 minutes on first build...
    echo.
    cargo build --release
    if %ERRORLEVEL% NEQ 0 (
        echo.
        echo [ERROR] Build failed! See errors above.
        echo.
        echo Common issues:
        echo   - Missing Visual Studio Build Tools with C++
        echo   - Missing cmake
        echo   - Path too long
        echo.
        echo Try running build-five.bat for detailed troubleshooting.
        pause
        exit /b 1
    )
    echo.
    echo [OK] Build completed successfully!
)

:AFTER_BUILD
:: ---------------------------------------------------------------------------
:: Step 6: Final summary
:: ---------------------------------------------------------------------------
echo.
echo [STEP 6/6] Setup Summary
echo.
echo    ╔══════════════════════════════════════════════════════════════════════╗
echo    ║  Setup Complete!                                                     ║
echo    ╚══════════════════════════════════════════════════════════════════════╝
echo.
echo Next steps:
echo.
echo   1. If you haven't already, download the model files
echo   2. Edit config.windows.yaml with your settings:
echo      - output_device: Your speaker name (e.g., "Speakers", "Esinkin")
echo      - local_url: Your Ollama/local LLM URL
echo      - kimi-key.txt: Create this file with your Kimi API key
echo.
echo   3. Start Five with:
echo      .\start-five.bat
echo.
echo   4. For auto-restart on crash, use:
echo      PowerShell: .\run-five.ps1
echo.
echo Documentation:
echo   - WINDOWS-PORT.md: Windows-specific notes
echo   - QUICKSTART.md: General quick start guide
echo.
echo Troubleshooting:
echo   - Audio not working? Run start-five.bat and use Diagnose mode
echo   - Build errors? Run build-five.bat for detailed help
echo   - Already running? Task Manager or: taskkill /IM five-daemon.exe /F
echo.
pause

:: Ask to start now
set /p START="Start Five now? (Y/N): "
if /i "%START%"=="Y" (
    echo.
    echo Starting Five...
    call "%FIVE_DIR%start-five.bat"
) else (
    echo.
    echo You can start Five anytime by running:
    echo   .\start-five.bat
    echo.
)

endlocal
