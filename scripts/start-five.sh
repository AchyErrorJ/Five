#!/usr/bin/env bash
# start-five.sh — Start the Five daemon
# Usage: ./scripts/start-five.sh [--release] [--config path]
#
# This starts Five in the foreground so you can see logs and stop it with
# Ctrl+C. For background operation, use rebuild.sh or the systemd service.

set -euo pipefail

RELEASE=""
CONFIG="config.yaml"
BIN="./target/debug/five-daemon"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --release)
      RELEASE="--release"
      BIN="./target/release/five-daemon"
      shift
      ;;
    --config)
      CONFIG="$2"
      shift 2
      ;;
    *)
      echo "Unknown option: $1"
      echo "Usage: $0 [--release] [--config path]"
      exit 1
      ;;
  esac
done

# Check if binary exists
if [[ ! -x "$BIN" ]]; then
  echo "[!!] five-daemon not found at $BIN"
  echo "     Build it first with: cargo build ${RELEASE}"
  exit 1
fi

# Check config file
if [[ ! -f "$CONFIG" ]]; then
  echo "[!!] Config file not found: $CONFIG"
  echo "     Copy config.example.yaml to $CONFIG and edit it."
  exit 1
fi

echo ""
echo "   ╔══════════════════════════════════════════════════════════════════════╗"
echo "   ║                                                                      ║"
echo "   ║   🔥 FIVE — Voice Assistant Daemon                                   ║"
echo "   ║                                                                      ║"
echo "   ╚══════════════════════════════════════════════════════════════════════╝"
echo ""
echo "[OK] Binary: $BIN"
echo "[OK] Config: $CONFIG"
echo ""
echo "Starting Five... Press Ctrl+C to stop."
echo ""

exec "$BIN" --config "$CONFIG" listen
