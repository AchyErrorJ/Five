# Five — Windows Quick Start Guide

## Prerequisites

1. **Rust** — Install from https://rustup.rs/
2. **Visual Studio Build Tools** with C++ support
   - Download: https://visualstudio.microsoft.com/downloads/
   - Select: "Desktop development with C++"
3. **cmake** — Install via `choco install cmake` or download from https://cmake.org/download/

## Quick Setup (First Time)

Double-click **`setup-five.bat`** to run the interactive setup wizard that will:
- Check all dependencies
- Create your config file
- Download required models (guides you through it)
- Build the project
- Start Five

Or manually:

```batch
# 1. Build the release version
build-five.bat --release

# 2. Copy and edit config
copy config.example.yaml config.windows.yaml
notepad config.windows.yaml

# 3. Download models to models\ directory
# See "Models" section below

# 4. Start Five
start-five.bat
```

## Scripts Overview

| Script | Purpose |
|--------|---------|
| `setup-five.bat` | First-time setup wizard (recommended for new users) |
| `build-five.bat` | Build the daemon (--release, --clean options) |
| `start-five.bat` | Interactive startup with menu (diagnostics, tests, normal mode) |
| `run-five.ps1` | Background watchdog that auto-restarts on crash |

## Using start-five.bat

When you run `start-five.bat`, you'll get an interactive menu:

```
1 — Normal mode  (tutor + voice commands, default)
2 — Coding mode  (routes to Claude Code bridge)
3 — Test audio   (record 3s, then speak it back)
4 — Test STT     (record 5s, transcribe to text)
5 — Just run     (skip menu, start immediately)
D — Diagnose     (full audio pipeline check)
Q — Quit
```

**Diagnose mode** is especially useful for troubleshooting audio issues — it tests recording, transcription, and playback in sequence.

## Configuration

Edit `config.windows.yaml` to configure:

```yaml
audio:
  output_device: "Esinkin"    # Your speaker/headphone name (substring match)
  # input_device: "Microphone Array"  # Optional: specific mic name

transcription:
  model_path: "models/ggml-tiny.en.bin"

voice:
  model_path: "models/kokoro/model.onnx"
  voices_dir: "voices"
  provider: "cpu"             # CPU is more stable than DirectML on AMD iGPU

brain:
  enabled: true
  local_url: "http://169.254.83.107:1234/v1"  # Your local LLM
  kimi_key_file: "kimi-key.txt"               # Create this with your API key
```

### Kimi API Key (Optional)

For lesson plans and complex reasoning:

```batch
echo YOUR_KIMI_API_KEY > kimi-key.txt
```

## Models

Create a `models\` directory and download:

### Whisper STT Model
```batch
mkdir models
cd models
# Download ggml-tiny.en.bin from:
# https://huggingface.co/ggerganov/whisper.cpp/blob/main/ggml-tiny.en.bin
```

### Kokoro TTS Models
```batch
mkdir models\kokoro
# Download from: https://github.com/thewh1teagle/kokoro-onnx
# - model.onnx
# - voices.bin
```

## Troubleshooting

### Build Fails

**Error: cargo not found**
- Install Rust from https://rustup.rs/
- Open a NEW terminal after installation

**Error: cmake not found**
- Install: `choco install cmake`
- Or download from https://cmake.org/download/

**Error: MSVC linker errors**
- Install Visual Studio Build Tools
- Make sure "Desktop development with C++" workload is selected

**Error: path too long**
- Move project to shorter path like `C:\five\`

### Runtime Issues

**Five crashes immediately**
- Use release build: `cargo build --release`
- Debug builds crash due to MSVC CRT asserts in whisper.cpp

**No audio input/output**
- Run `start-five.bat` → Diagnose mode (D)
- Check Windows Sound Settings
- Edit `output_device` in config.windows.yaml to match your speaker name
- Check microphone privacy settings

**Already running error**
```batch
taskkill /IM five-daemon.exe /F
```

**Transcription is slow**
- Use `ggml-tiny.en.bin` instead of base.en (configured by default)
- tiny.en is ~4x faster on CPU

**TTS produces silence**
- Set `provider: "cpu"` in config (DirectML fails on AMD iGPU)

### Auto-Restart (Background Mode)

To run Five as a background service that restarts on crash:

```powershell
# One-time setup: create scheduled task
schtasks /Create /TN FiveDaemon /TR "powershell -ExecutionPolicy Bypass -File %CD%\run-five.ps1" /SC ONLOGON /RU SYSTEM

# Start it
schtasks /Run /TN FiveDaemon

# Stop it
schtasks /End /TN FiveDaemon
taskkill /IM five-daemon.exe /F
```

## Field Notes (Legion Go Specific)

From WINDOWS-PORT.md:

- **Build release-only**: Debug builds crash with MSVC CRT debug asserts
- **Use tiny.en model**: ~4s per utterance vs ~9s for base.en
- **CPU for TTS**: DirectML intermittently fails Kokoro's ConvTranspose node
- **Pin output_device**: Windows default may be a disconnected BT adapter
- **Playback tail-clip**: The WASAPI backend pads 1s silence to prevent cutoff

## Getting Help

1. Run `start-five.bat` → Diagnose mode for audio issues
2. Check `WINDOWS-PORT.md` for detailed porting notes
3. Review logs in `logs\` directory
4. Run from cmd.exe to see full error output if window closes instantly
