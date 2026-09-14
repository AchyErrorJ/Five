#!/usr/bin/env bash
# build-five.sh — Build the Five daemon
# Usage: ./scripts/build-five.sh [--release]
#
# Simple build script that ensures all dependencies are installed and builds
# the project.

set -euo pipefail

echo ""
echo "   ╔══════════════════════════════════════════════════════════════════════╗"
echo "   ║                                                                      ║"
echo "   ║   🔨 Building Five Daemon                                            ║"
echo "   ║                                                                      ║"
echo "   ╚══════════════════════════════════════════════════════════════════════╝"
echo ""

RELEASE=""
BUILD_TYPE="debug"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --release)
      RELEASE="--release"
      BUILD_TYPE="release"
      shift
      ;;
    *)
      echo "Unknown option: $1"
      echo "Usage: $0 [--release]"
      exit 1
      ;;
  esac
done

echo "[..] Checking for required build tools..."

# Check for cargo/rustc
if ! command -v cargo &> /dev/null; then
  echo "[!!] cargo not found. Install Rust from https://rustup.rs/"
  exit 1
fi
echo "[OK] cargo: $(cargo --version)"

# Check for cmake (required by whisper-rs)
if ! command -v cmake &> /dev/null; then
  echo "[!!] cmake not found. Install with: sudo apt install cmake"
  exit 1
fi
echo "[OK] cmake: $(cmake --version | head -1)"

# Check for g++ (required by whisper-rs)
if ! command -v g++ &> /dev/null; then
  echo "[!!] g++ not found. Install with: sudo apt install build-essential"
  exit 1
fi
echo "[OK] g++: $(g++ --version | head -1)"

# Check for libclang-dev (required by bindgen)
if ! dpkg -l | grep -q libclang-dev; then
  echo "[WARN] libclang-dev may not be installed. If build fails, install with:"
  echo "       sudo apt install libclang-dev"
else
  echo "[OK] libclang-dev is installed"
fi

# Check for libasound2-dev (ALSA headers)
if ! dpkg -l | grep -q libasound2-dev; then
  echo "[WARN] libasound2-dev may not be installed. If build fails, install with:"
  echo "       sudo apt install libasound2-dev"
else
  echo "[OK] libasound2-dev is installed"
fi

echo ""
echo "[..] Building five-daemon ($BUILD_TYPE)..."
echo ""

cargo build ${RELEASE}

echo ""
echo "[OK] Build complete!"
echo ""

if [[ "$BUILD_TYPE" == "release" ]]; then
  echo "Binary: ./target/release/five-daemon"
  echo ""
  echo "To start Five, run:"
  echo "  ./scripts/start-five.sh --release"
else
  echo "Binary: ./target/debug/five-daemon"
  echo ""
  echo "To start Five, run:"
  echo "  ./scripts/start-five.sh"
fi
echo ""
