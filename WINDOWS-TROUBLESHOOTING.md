# WINDOWS-TROUBLESHOOTING.md

## Scripts Keep Crashing Immediately?

If the batch files (`.bat`) or PowerShell scripts (`.ps1`) close before you can see any error message, try these steps:

### Step 1: Run the Diagnostic Script

**In PowerShell:**
```powershell
.\test-scripts.ps1
```

**Or in Command Prompt:**
```cmd
test-scripts.bat
```

This will show you exactly what's missing or broken.

### Step 2: Common Issues & Fixes

#### Issue: "cargo not found"
**Fix:** Install Rust from https://rustup.rs/
- After installation, **close and reopen your terminal**
- Verify with: `cargo --version`

#### Issue: "cmake not found"
**Fix:** Install cmake
```cmd
choco install cmake
```
Or download from: https://cmake.org/download/

#### Issue: "config.windows.yaml not found"
**Fix:** Create it from the example:
```cmd
copy config.example.yaml config.windows.yaml
```
Then edit `config.windows.yaml` to set your audio device and API keys.

#### Issue: "models directory missing"
**Fix:** Create the directory and download models:
```cmd
mkdir models
mkdir models\kokoro
```
Download:
- Whisper model: https://huggingface.co/ggerganov/whisper.cpp/blob/main/ggml-tiny.en.bin → place in `models\ggml-tiny.en.bin`
- Kokoro TTS: https://github.com/thewh1teagle/kokoro-onnx → extract to `models\kokoro\`

#### Issue: "No binary found"
**Fix:** Build the project:
```cmd
build-five.bat --release
```
This takes 5-15 minutes on first build.

#### Issue: PowerShell script won't run
**Fix:** Check execution policy:
```powershell
Get-ExecutionPolicy
```
If it says `Restricted`, run:
```powershell
Set-ExecutionPolicy -Scope CurrentUser RemoteSigned
```

### Step 3: Run Scripts with Debug Mode

**For batch files:**
```cmd
cmd /k ".\setup-five.bat"
```
The `/k` flag keeps the window open after the script finishes.

**For PowerShell scripts:**
```powershell
powershell -NoExit -File .\test-scripts.ps1
```

### Step 4: Check Logs

If a script crashes, check for log files:
```cmd
dir logs\*.log /O-D
type logs\five-*.log | more
```

### Step 5: Manual Test

Try running the daemon directly to see errors:
```cmd
target\release\five-daemon.exe --config config.windows.yaml listen
```

If the binary doesn't exist yet, build it first:
```cmd
cargo build --release
```

---

## Quick Reference

| Script | Purpose | How to Run |
|--------|---------|------------|
| `test-scripts.bat` or `test-scripts.ps1` | Diagnose environment issues | Double-click or `.\test-scripts.ps1` |
| `setup-five.bat` | First-time setup wizard | `.\setup-five.bat` |
| `build-five.bat` | Build the daemon | `build-five.bat --release` |
| `start-five.bat` | Start with interactive menu | `.\start-five.bat` |
| `run-five.ps1` | Auto-restart watchdog service | `.\run-five.ps1` |

---

## Still Having Issues?

1. **Take a screenshot** of the diagnostic output from `test-scripts.ps1`
2. **Check Windows Event Viewer** for application crashes
3. **Run as Administrator** if you get permission errors
4. **Disable antivirus temporarily** if builds fail (some AVs block Rust binaries)
