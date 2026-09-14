# Quick Start Guide

## First Time Setup

Run the setup script to get started:

```bash
./scripts/setup-five.sh
```

This will:
1. Check for required system dependencies (Rust, cmake, g++, etc.)
2. Create a `config.yaml` from the example template
3. Create the models directory
4. Build the project

## Manual Setup

If you prefer to set up manually:

### 1. Install Dependencies

```bash
sudo apt update
sudo apt install -y cmake build-essential libclang-dev libasound2-dev
```

Install Rust from [rustup.rs](https://rustup.rs/) if you haven't already.

### 2. Configure

```bash
cp config.example.yaml config.yaml
# Edit config.yaml with your settings
```

### 3. Download Models

You'll need to download these model files:

- **Whisper STT**: `models/ggml-tiny.en.bin` (or `ggml-base.en.bin` for better accuracy)
- **Kokoro TTS**: `models/kokoro/model.onnx` and `models/kokoro/voices.bin`
- **Wake word** (optional): `models/wakeword.rpw` (train your own with rustpotter)

See `HANDOFF.md` for model download links and instructions.

### 4. Build

```bash
# Debug build (faster compilation, larger binary)
./scripts/build-five.sh

# Release build (slower compilation, optimized binary)
./scripts/build-five.sh --release
```

## Running Five

### Start in Foreground

```bash
# Debug mode
./scripts/start-five.sh

# Release mode
./scripts/start-five.sh --release

# Custom config
./scripts/start-five.sh --config path/to/config.yaml
```

Press `Ctrl+C` to stop.

### Rebuild and Restart (Development)

```bash
# Stop, rebuild, and restart in one command
./scripts/rebuild.sh [--release] [--config path]
```

### Background Operation

For production use, see the systemd service configuration in the repository.

## Available Scripts

| Script | Purpose |
|--------|---------|
| `setup-five.sh` | Initial setup (dependencies, config, build) |
| `build-five.sh` | Build the daemon (checks dependencies first) |
| `start-five.sh` | Start the daemon in foreground |
| `rebuild.sh` | Stop, rebuild, and restart (development workflow) |
| `record-wakeword-samples.sh` | Record training samples for wake word |

## Troubleshooting

### Build fails with missing headers
Install the required development packages:
```bash
sudo apt install libclang-dev libasound2-dev
```

### "Config file not found"
Copy the example config:
```bash
cp config.example.yaml config.yaml
```

### "Model file not found"
Download the required model files (see above). The daemon won't start without them.

### Audio issues
Check that your microphone is connected and set as the default input device. Edit `config.yaml` to specify the correct ALSA device if needed.

## Documentation

- `HANDOFF.md` — Architecture, development guide, and technical details
- `config.example.yaml` — Configuration reference with all options
- `WINDOWS-PORT.md` — Windows-specific setup instructions
