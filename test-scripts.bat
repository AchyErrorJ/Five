@echo off
echo.
echo ========================================
echo Five Windows Scripts - Diagnostic Test
echo ========================================
echo.
echo Current directory: %CD%
echo Script directory: %~dp0
echo.

:: Test 1: Check if we can find Cargo.toml
if exist "%~dp0Cargo.toml" (
    echo [OK] Cargo.toml found
) else (
    echo [ERROR] Cargo.toml NOT found in %~dp0
)

:: Test 2: Check Rust
where cargo >nul 2>&1
if %ERRORLEVEL% EQU 0 (
    echo [OK] cargo found
    cargo --version
) else (
    echo [ERROR] cargo NOT found
)

:: Test 3: Check cmake
where cmake >nul 2>&1
if %ERRORLEVEL% EQU 0 (
    echo [OK] cmake found
    cmake --version | findstr /C:"cmake version"
) else (
    echo [WARN] cmake NOT found
)

:: Test 4: Check config files
if exist "%~dp0config.windows.yaml" (
    echo [OK] config.windows.yaml exists
) else if exist "%~dp0config.example.yaml" (
    echo [INFO] config.example.yaml exists (can create config.windows.yaml)
) else (
    echo [ERROR] No config file found
)

:: Test 5: Check models directory
if exist "%~dp0models" (
    echo [OK] models directory exists
    dir /b "%~dp0models" 2>nul
) else (
    echo [WARN] models directory does not exist
)

:: Test 6: Check binary
if exist "%~dp0target\release\five-daemon.exe" (
    echo [OK] Release binary found
) else if exist "%~dp0target\debug\five-daemon.exe" (
    echo [INFO] Debug binary found
) else (
    echo [WARN] No binary found (build required)
)

echo.
echo ========================================
echo Test Complete!
echo ========================================
echo.
pause
