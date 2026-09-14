#!/usr/bin/env bash
# setup-five.sh — Initial setup script for Five daemon
# Usage: ./scripts/setup-five.sh
#
# This script helps you set up Five for the first time:
# 1. Checks system dependencies
# 2. Creates config file from example
# 3. Builds the project
# 4. Provides next steps

set -euo pipefail

echo ""
echo "   ╔══════════════════════════════════════════════════════════════════════╗"
echo "   ║                                                                      ║"
echo "   ║   🚀 Five Daemon — Initial Setup                                     ║"
echo "   ║                                                                      ║"
echo "   ╚══════════════════════════════════════════════════════════════════════╝"
echo ""

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
cd "$ROOT_DIR"

# ---------------------------------------------------------------------------
# Step 1: Check system dependencies
# ---------------------------------------------------------------------------
echo "=== Step 1: Checking system dependencies ==="
echo ""

MISSING_DEPS=()

# Rust
if ! command -v cargo &> /dev/null; then
  MISSING_DEPS+=("Rust (install from https://rustup.rs/)")
else
  echo "[OK] Rust: $(cargo --version)"
fi

# cmake
if ! command -v cmake &> /dev/null; then
  MISSING_DEPS+=("cmake (sudo apt install cmake)")
else
  echo "[OK] cmake: $(cmake --version | head -1)"
fi

# g++
if ! command -v g++ &> /dev/null; then
  MISSING_DEPS+=("g++ (sudo apt install build-essential)")
else
  echo "[OK] g++: $(g++ --version | head -1)"
fi

# libclang-dev
if ! dpkg -l | grep -q libclang-dev; then
  MISSING_DEPS+=("libclang-dev (sudo apt install libclang-dev)")
else
  echo "[OK] libclang-dev"
fi

# libasound2-dev
if ! dpkg -l | grep -q libasound2-dev; then
  MISSING_DEPS+=("libasound2-dev (sudo apt install libasound2-dev)")
else
  echo "[OK] libasound2-dev"
fi

if [[ ${#MISSING_DEPS[@]} -gt 0 ]]; then
  echo ""
  echo "[!!] Missing dependencies:"
  for dep in "${MISSING_DEPS[@]}"; do
    echo "     - $dep"
  done
  echo ""
  echo "Install them with:"
  echo "  sudo apt update && sudo apt install -y cmake build-essential libclang-dev libasound2-dev"
  echo ""
  read -p "Continue anyway? [y/N] " -n 1 -r
  echo ""
  if [[ ! $REPLY =~ ^[Yy]$ ]]; then
    exit 1
  fi
fi

echo ""

# ---------------------------------------------------------------------------
# Step 2: Create config file
# ---------------------------------------------------------------------------
echo "=== Step 2: Setting up configuration ==="
echo ""

if [[ -f "config.yaml" ]]; then
  echo "[OK] config.yaml already exists"
else
  if [[ -f "config.example.yaml" ]]; then
    cp config.example.yaml config.yaml
    echo "[OK] Created config.yaml from config.example.yaml"
    echo "     Edit config.yaml to customize settings for your setup."
  else
    echo "[WARN] config.example.yaml not found"
  fi
fi

echo ""

# ---------------------------------------------------------------------------
# Step 3: Create models directory
# ---------------------------------------------------------------------------
echo "=== Step 3: Setting up models directory ==="
echo ""

mkdir -p models
echo "[OK] models/ directory created"
echo ""
echo "You will need to download model files:"
echo "  - Whisper STT model: models/ggml-tiny.en.bin (or ggml-base.en.bin)"
echo "  - Kokoro TTS: models/kokoro/model.onnx and models/kokoro/voices.bin"
echo "  - Wake word: models/wakeword.rpw (optional, train your own)"
echo ""
echo "See HANDOFF.md for model download links."
echo ""

# ---------------------------------------------------------------------------
# Step 4: Build
# ---------------------------------------------------------------------------
echo "=== Step 4: Building five-daemon ==="
echo ""

read -p "Build now? [Y/n] " -n 1 -r
echo ""
if [[ $REPLY =~ ^[Nn]$ ]]; then
  echo "[..] Skipping build. Run './scripts/build-five.sh' later."
else
  "$SCRIPT_DIR/build-five.sh"
fi

echo ""

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo "=== Setup Complete! ==="
echo ""
echo "Next steps:"
echo "  1. Download required model files (see above)"
echo "  2. Edit config.yaml for your audio device and API keys"
echo "  3. Start Five: ./scripts/start-five.sh [--release]"
echo ""
echo "For development:"
echo "  - Rebuild and restart: ./scripts/rebuild.sh [--release]"
echo "  - Just rebuild: ./scripts/build-five.sh [--release]"
echo ""
echo "Documentation:"
echo "  - HANDOFF.md — Architecture and development guide"
echo "  - config.example.yaml — Configuration reference"
echo ""
