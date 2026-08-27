#!/usr/bin/env bash
# rebuild.sh — Stop, rebuild, and restart the Five daemon
# Usage: ./scripts/rebuild.sh [--release] [--config path]
#
# This is a development convenience script. For production, use the systemd
# service (scripts/five-daemon.service).

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

# ---------------------------------------------------------------------------
# 1. Stop
# ---------------------------------------------------------------------------
PID=$(pgrep -f "five-daemon" || true)
if [[ -n "$PID" ]]; then
  echo "[rebuild] Stopping five-daemon (PID $PID)..."
  kill "$PID" || true
  # Wait up to 5s for clean shutdown
  for i in {1..50}; do
    if ! kill -0 "$PID" 2>/dev/null; then
      break
    fi
    sleep 0.1
  done
  if kill -0 "$PID" 2>/dev/null; then
    echo "[rebuild] Force-killing five-daemon..."
    kill -9 "$PID" || true
  fi
  echo "[rebuild] Stopped."
else
  echo "[rebuild] five-daemon not running."
fi

# ---------------------------------------------------------------------------
# 2. Build
# ---------------------------------------------------------------------------
echo "[rebuild] Building five-daemon ${RELEASE:-(debug)}..."
cargo build ${RELEASE}

# ---------------------------------------------------------------------------
# 3. Start
# ---------------------------------------------------------------------------
if [[ ! -x "$BIN" ]]; then
  echo "[rebuild] ERROR: Binary not found: $BIN"
  exit 1
fi

echo "[rebuild] Starting five-daemon..."
nohup "$BIN" --config "$CONFIG" > five-daemon.log 2>&1 &
NEW_PID=$!
echo "[rebuild] Started (PID $NEW_PID). Log: five-daemon.log"

# Tail the log briefly so the user sees startup status
sleep 0.5
tail -n 20 five-daemon.log 2>/dev/null || true
