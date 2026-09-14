# Five Windows Scripts - Diagnostic Test
# Run this in PowerShell to check your environment

Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "Five Windows Scripts - Diagnostic Test" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

$fiveDir = Split-Path -Parent $MyInvocation.MyCommand.Path
Write-Host "Current directory: $(Get-Location)"
Write-Host "Script directory: $fiveDir"
Write-Host ""

# Test 1: Check if we can find Cargo.toml
if (Test-Path "$fiveDir\Cargo.toml") {
    Write-Host "[OK] Cargo.toml found" -ForegroundColor Green
} else {
    Write-Host "[ERROR] Cargo.toml NOT found in $fiveDir" -ForegroundColor Red
}

# Test 2: Check Rust
try {
    $cargoVersion = cargo --version 2>&1
    Write-Host "[OK] cargo found: $cargoVersion" -ForegroundColor Green
} catch {
    Write-Host "[ERROR] cargo NOT found" -ForegroundColor Red
}

# Test 3: Check cmake
try {
    $cmakeVersion = cmake --version 2>&1 | Select-String "cmake version"
    Write-Host "[OK] cmake found: $cmakeVersion" -ForegroundColor Green
} catch {
    Write-Host "[WARN] cmake NOT found" -ForegroundColor Yellow
}

# Test 4: Check config files
if (Test-Path "$fiveDir\config.windows.yaml") {
    Write-Host "[OK] config.windows.yaml exists" -ForegroundColor Green
} elseif (Test-Path "$fiveDir\config.example.yaml") {
    Write-Host "[INFO] config.example.yaml exists (can create config.windows.yaml)" -ForegroundColor Cyan
} else {
    Write-Host "[ERROR] No config file found" -ForegroundColor Red
}

# Test 5: Check models directory
if (Test-Path "$fiveDir\models") {
    Write-Host "[OK] models directory exists" -ForegroundColor Green
    Get-ChildItem "$fiveDir\models" -Name
} else {
    Write-Host "[WARN] models directory does not exist" -ForegroundColor Yellow
}

# Test 6: Check binary
if (Test-Path "$fiveDir\target\release\five-daemon.exe") {
    Write-Host "[OK] Release binary found" -ForegroundColor Green
} elseif (Test-Path "$fiveDir\target\debug\five-daemon.exe") {
    Write-Host "[INFO] Debug binary found" -ForegroundColor Cyan
} else {
    Write-Host "[WARN] No binary found (build required)" -ForegroundColor Yellow
}

# Test 7: Check Execution Policy
$execPolicy = Get-ExecutionPolicy -Scope CurrentUser
Write-Host "[INFO] PowerShell Execution Policy: $execPolicy" -ForegroundColor White

Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "Test Complete!" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""
Write-Host "If you see ERRORs above, fix them before running the main scripts."
Write-Host ""
Read-Host "Press Enter to exit"
