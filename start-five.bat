@echo off
setlocal EnableDelayedExpansion

title Five — Voice Assistant
mode con: cols=80 lines=30
color 0B

:: ---------------------------------------------------------------------------
:: Five Startup Script for Windows (Legion Go)
:: ---------------------------------------------------------------------------
:: This starts Five in the foreground so you can see logs and stop it with
:: Ctrl+C. If you want it to run silently in the background as a service,
:: look at NSSM (Non-Sucking Service Manager) or WinSW.
:: ---------------------------------------------------------------------------

set "FIVE_DIR=%~dp0"
set "CONFIG=%FIVE_DIR%config.windows.yaml"
set "FIVE_EXE=%FIVE_DIR%target\release\five-daemon.exe"
set "FIVE_DEBUG=%FIVE_DIR%target\debug\five-daemon.exe"
set "KIMI_KEY=%FIVE_DIR%kimi-key.txt"

echo.
echo    ╔══════════════════════════════════════════════════════════════════════╗
echo    ║                                                                      ║
echo    ║   🔥 FIVE — Voice Assistant Daemon                                   ║
echo    ║      Legion Go Edition                                               ║
echo    ║                                                                      ║
echo    ╚══════════════════════════════════════════════════════════════════════╝
echo.

:: ---------------------------------------------------------------------------
:: CHECK: Is Five already running?
:: ---------------------------------------------------------------------------
tasklist /FI "IMAGENAME eq five-daemon.exe" 2>nul | find /I "five-daemon.exe" >nul
if %ERRORLEVEL% == 0 (
    echo [WARN] five-daemon.exe is ALREADY RUNNING.
    echo        Check Task Manager or run: taskkill /IM five-daemon.exe /F
    echo.
    echo        Press any key to exit.
    pause >nul
    exit /b 1
)

:: ---------------------------------------------------------------------------
:: 1. Find the five-daemon binary
:: ---------------------------------------------------------------------------
set "FIVE_BIN="
if exist "%FIVE_EXE%" (
    set "FIVE_BIN=%FIVE_EXE%"
    echo [OK] Found release binary: target\release\five-daemon.exe
) else if exist "%FIVE_DEBUG%" (
    set "FIVE_BIN=%FIVE_DEBUG%"
    echo [OK] Found debug binary: target\debug\five-daemon.exe
) else (
    echo [!!] five-daemon.exe not found.
    echo      Tried: target\release\five-daemon.exe
    echo            target\debug\five-daemon.exe
    echo.
    echo      Build it first:
    echo        cargo build --release
    echo.
    pause
    exit /b 1
)

:: ---------------------------------------------------------------------------
:: 2. Check config file
:: ---------------------------------------------------------------------------
if exist "%CONFIG%" (
    echo [OK] Config found: config.windows.yaml
) else (
    echo [!!] config.windows.yaml not found in %FIVE_DIR%
    pause
    exit /b 1
)

:: ---------------------------------------------------------------------------
:: 3. Check model files
:: ---------------------------------------------------------------------------
set "MODEL_OK=1"

if not exist "%FIVE_DIR%models\ggml-tiny.en.bin" (
    echo [!!] Missing: models\ggml-tiny.en.bin  (Whisper STT model)
    set "MODEL_OK=0"
) else (
    echo [OK] Whisper model found
)

if not exist "%FIVE_DIR%models\kokoro\model.onnx" (
    echo [!!] Missing: models\kokoro\model.onnx  (Kokoro TTS model)
    set "MODEL_OK=0"
) else (
    echo [OK] Kokoro TTS model found
)

if not exist "%FIVE_DIR%models\kokoro\voices.bin" (
    echo [!!] Missing: models\kokoro\voices.bin  (Kokoro voices)
    set "MODEL_OK=0"
) else (
    echo [OK] Kokoro voices found
)

if "%MODEL_OK%"=="0" (
    echo.
    echo      Download models or check your models\ directory.
    echo      See README for setup instructions.
    echo.
    pause
    exit /b 1
)

:: ---------------------------------------------------------------------------
:: 4. Check Kimi API key (for lesson plans + escalations)
:: ---------------------------------------------------------------------------
if exist "%KIMI_KEY%" (
    echo [OK] Kimi key file found
) else (
    echo [WARN] kimi-key.txt not found — lesson plans and Kimi escalations won't work.
    echo        Create it with your API key if you want those features.
    echo.
)

:: ---------------------------------------------------------------------------
:: 5. Quick OpenClaw gateway check
:: ---------------------------------------------------------------------------
echo.
echo [..] Checking OpenClaw gateway on 127.0.0.1:10000 ...
powershell -NoProfile -Command "try { $r = Invoke-RestMethod -Uri 'http://127.0.0.1:10000/status' -TimeoutSec 3 -ErrorAction Stop; Write-Host '     [OK] Gateway is up.' } catch { Write-Host '     [WARN] Gateway not responding. Five will still start, but' ; Write-Host '            commands that need OpenClaw will fail until it is.' }"

:: ---------------------------------------------------------------------------
:: 6. Optional: quick audio test
:: ---------------------------------------------------------------------------
echo.
echo [..] Ready to start. What mode?
echo.
echo      1 — Normal mode  (tutor + voice commands, default)
echo      2 — Coding mode  (routes to Claude Code bridge)
echo      3 — Test audio   (record 3s, then speak it back)
echo      4 — Test STT     (record 5s, transcribe to text)
echo      5 — Just run     (skip menu, start immediately)
echo      D — Diagnose     (full audio pipeline check)
echo      Q — Quit
echo.
set /p MODE="Pick: "

if /i "%MODE%"=="q" exit /b 0
if /i "%MODE%"=="d" goto :DIAGNOSE
if "%MODE%"=="5" goto :RUN
if "%MODE%"=="1" goto :RUN
if "%MODE%"=="2" goto :CODING
if "%MODE%"=="3" goto :TEST_AUDIO
if "%MODE%"=="4" goto :TEST_STT

goto :RUN

:: ---------------------------------------------------------------------------
:: NORMAL / TUTOR MODE
:: ---------------------------------------------------------------------------
:RUN
echo.
echo    ╔══════════════════════════════════════════════════════════════════════╗
echo    ║  Starting Five...                                                    ║
echo    ║  Say "Five" or just speak — it listens to everything.                ║
echo    ║  Press Ctrl+C to stop.                                               ║
echo    ╚══════════════════════════════════════════════════════════════════════╝
echo.
echo [TIP] If no commands come through, press Ctrl+C and run this script
echo       again, then pick D for Diagnose mode.
echo.
"%FIVE_BIN%" --config "%CONFIG%" listen
goto :END

:: ---------------------------------------------------------------------------
:: CODING MODE
:: ---------------------------------------------------------------------------
:CODING
echo.
echo    ╔══════════════════════════════════════════════════════════════════════╗
echo    ║  Starting Five in CODING MODE...                                     ║
echo    ║  Voice commands route to: claude-bridge.txt                          ║
echo    ║  Run `claude` in another terminal and tail that file.                ║
echo    ╚══════════════════════════════════════════════════════════════════════╝
echo.
"%FIVE_BIN%" --config "%CONFIG%" listen --bridge claude-bridge.txt
goto :END

:: ---------------------------------------------------------------------------
:: AUDIO TEST
:: ---------------------------------------------------------------------------
:TEST_AUDIO
echo.
echo [..] Recording 3 seconds of audio...
"%FIVE_BIN%" --config "%CONFIG%" record --output test-audio.wav --duration 3
echo [OK] Saved to test-audio.wav
echo [..] Playing it back via TTS speak command...
"%FIVE_BIN%" --config "%CONFIG%" speak "Audio test complete. If you heard this, Five is working."
pause
goto :END

:: ---------------------------------------------------------------------------
:: STT TEST
:: ---------------------------------------------------------------------------
:TEST_STT
echo.
echo [..] Recording 5 seconds — say something clearly...
"%FIVE_BIN%" --config "%CONFIG%" record --output test-stt.wav --duration 5
echo [..] Transcribing...
"%FIVE_BIN%" --config "%CONFIG%" transcribe test-stt.wav
del test-stt.wav 2>nul
pause
goto :END

:: ---------------------------------------------------------------------------
:: DIAGNOSE — Full pipeline check
:: ---------------------------------------------------------------------------
:DIAGNOSE
echo.
echo    ╔══════════════════════════════════════════════════════════════════════╗
echo    ║  DIAGNOSTIC MODE — Full audio pipeline check                         ║
echo    ╚══════════════════════════════════════════════════════════════════════╝
echo.

:: Test 1: Record
echo [TEST 1/3] Recording 5 seconds — SAY SOMETHING CLEARLY into your mic...
"%FIVE_BIN%" --config "%CONFIG%" record --output diagnose.wav --duration 5
if not exist "diagnose.wav" (
    echo [FAIL] Recording failed. Check your mic is enabled and set as default.
    goto :DIAG_END
)
echo [OK] Recording saved.
echo.

:: Test 2: Transcribe
echo [TEST 2/3] Transcribing what you said...
"%FIVE_BIN%" --config "%CONFIG%" transcribe diagnose.wav
echo.

:: Test 3: TTS
echo [TEST 3/3] Testing text-to-speech...
"%FIVE_BIN%" --config "%CONFIG%" speak "If you can hear this, Five's audio output is working."
echo.

echo [..] Did you hear the voice? Did the transcription show your words?
echo.
echo      If recording failed     =^> mic issue. Check Windows Sound Settings.
echo      If transcribe is blank  =^> audio too quiet, or model issue.
echo      If no voice heard       =^> output_device in config.windows.yaml
echo                               may point to a disconnected device.
echo      If all 3 passed         =^> audio works. Issue may be OpenClaw gateway.
echo.

:DIAG_END
pause
del diagnose.wav 2>nul
goto :END

:: ---------------------------------------------------------------------------
:: END
:: ---------------------------------------------------------------------------
:END
echo.
echo Five stopped. Press any key to close.
pause >nul
endlocal
