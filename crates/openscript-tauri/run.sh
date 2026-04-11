#!/usr/bin/env bash
# run.sh — Start OpenScript Tauri desktop app
# Handles Vite dev server + Tauri binary lifecycle.
# Usage: ./run.sh [--dev|--release|--prod]

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
FRONTEND_DIR="$SCRIPT_DIR/src/frontend"
VITE_PORT=1420
TAURI_BIN="$PROJECT_ROOT/target/debug/openscript-tauri"
VITE_PID=""
TAURI_PID=""

cleanup() {
  echo ""
  echo "Shutting down..."
  [ -n "$VITE_PID" ] && kill "$VITE_PID" 2>/dev/null
  [ -n "$TAURI_PID" ] && kill "$TAURI_PID" 2>/dev/null
  exit 0
}
trap cleanup SIGINT SIGTERM

# ─── Mode ────────────────────────────────────────────────────────────────
MODE="${1:---dev}"

case "$MODE" in
  --dev)
    # Build frontend if dist is missing, start Vite dev server, run Tauri
    echo "=== OpenScript Tauri — Dev Mode ==="
    if [ ! -d "$FRONTEND_DIR/node_modules" ]; then
      echo "Installing frontend dependencies..."
      npm --prefix "$FRONTEND_DIR" install
    fi
    echo "Building Tauri binary..."
    cargo build -p openscript-tauri --manifest-path "$SCRIPT_DIR/Cargo.toml" 2>&1 | tail -1

    echo "Starting Vite dev server on :$VITE_PORT..."
    nohup npx --prefix "$FRONTEND_DIR" vite --port "$VITE_PORT" --strictPort >> /tmp/openscript-vite.log 2>&1 &
    VITE_PID=$!
    disown "$VITE_PID" 2>/dev/null || true

    # Wait for Vite to be ready (up to 15s)
    echo "Waiting for Vite..."
    for i in $(seq 1 30); do
      if curl -s -o /dev/null -w '' "http://localhost:$VITE_PORT" 2>/dev/null; then
        echo "  Vite ready (took ${i}s)"
        break
      fi
      [ "$i" -eq 30 ] && { echo "Vite did not start in time"; exit 1; }
      sleep 0.5
    done
    ;;

  --release)
    # Rebuild frontend, rebuild Tauri in release mode, run with pre-built dist
    echo "=== OpenScript Tauri — Release Build ==="
    echo "Building frontend..."
    npm --prefix "$FRONTEND_DIR" run build
    echo "Building Tauri release binary..."
    cargo build -p openscript-tauri --manifest-path "$SCRIPT_DIR/Cargo.toml" --release 2>&1 | tail -1
    TAURI_BIN="$PROJECT_ROOT/target/release/openscript-tauri"
    echo "Starting (no dev server, using pre-built dist)..."
    ;;

  --prod)
    # Run pre-built binary with pre-built dist (no rebuilds)
    echo "=== OpenScript Tauri — Production Run ==="
    if [ ! -f "$TAURI_BIN" ]; then
      echo "Error: Release binary not found. Run './run.sh --release' first."
      exit 1
    fi
    echo "Starting (pre-built)..."
    ;;

  *)
    echo "Usage: $0 [--dev|--release|--prod]"
    exit 1
    ;;
esac

# ─── Launch Tauri ────────────────────────────────────────────────────────
echo "Launching OpenScript..."

# Workaround for Wayland/WebKitGTK issues on Linux
export WEBKIT_DISABLE_DMABUF_RENDERER=1
export GDK_BACKEND=x11

nohup "$TAURI_BIN" >> /tmp/openscript-tauri.log 2>&1 &
TAURI_PID=$!
disown "$TAURI_PID" 2>/dev/null || true

sleep 3
if kill -0 "$TAURI_PID" 2>/dev/null; then
  echo "  OpenScript running (PID $TAURI_PID)"
  echo "  Logs: /tmp/openscript-tauri.log"
  echo "  Press Ctrl+C to exit (app keeps running)"
else
  echo "  Error: Tauri process exited immediately."
  cat /tmp/openscript-tauri.log
  exit 1
fi
