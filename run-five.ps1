# run-five.ps1 — non-interactive watchdog for five-daemon.
# Registered as a logon Scheduled Task ("FiveDaemon"). Restarts the daemon
# if it exits (crash, panic, killed by accident), logs to logs\five-<date>.log.
# Stop it with:  schtasks /End /TN FiveDaemon ; taskkill /IM five-daemon.exe /F

$ErrorActionPreference = 'Continue'
$fiveDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$exe = Join-Path $fiveDir 'target\release\five-daemon.exe'
$config = Join-Path $fiveDir 'config.windows.yaml'
$logDir = Join-Path $fiveDir 'logs'

New-Item -ItemType Directory -Force $logDir | Out-Null

$backoff = 5
while ($true) {
    $stamp = Get-Date -Format 'yyyy-MM-dd HH:mm:ss'
    $log = Join-Path $logDir ("five-" + (Get-Date -Format 'yyyyMMdd') + ".log")
    Add-Content -Encoding utf8 $log "`n=== run-five: starting daemon at $stamp ==="

    $started = Get-Date
    # Redirect via cmd so the daemon's output lands as plain UTF-8 —
    # PowerShell native redirection writes UTF-16, which breaks grep/tail.
    cmd /c "`"$exe`" --config `"$config`" listen >> `"$log`" 2>&1"
    $code = $LASTEXITCODE

    $stamp = Get-Date -Format 'yyyy-MM-dd HH:mm:ss'
    Add-Content -Encoding utf8 $log "=== run-five: daemon exited code $code at $stamp; restarting in ${backoff}s ==="
    Start-Sleep -Seconds $backoff
    # Crash-loop guard: back off up to 5 minutes, but a run longer than
    # 5 minutes was healthy — reset the backoff.
    if ((Get-Date) - $started -gt [TimeSpan]::FromMinutes(5)) {
        $backoff = 5
    } else {
        $backoff = [Math]::Min($backoff * 2, 300)
    }
}
