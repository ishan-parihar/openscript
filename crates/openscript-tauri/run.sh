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
  [ -n "$VITE_PID" ] && kill "$VITE_PID" 2>/dev/null && wait "$VITE_PID" 2>/dev/null
  [ -n "$TAURI_PID" ] && kill "$TAURI_PID" 2>/dev/null && wait "$TAURI_PID" 2>/dev/null
  exit 0
}
trap cleanup SIGINT SIGTERM

# ─── Linux: Check/install GStreamer media plugins ────────────────────────
ensure_gstreamer_linux() {
  # Check if required GStreamer elements are available
  local missing=()
  for element in autoaudiosink playbin decodebin; do
    if ! gst-inspect-1.0 "$element" >/dev/null 2>&1; then
      missing+=("$element")
    fi
  done

  [ ${#missing[@]} -eq 0 ] && return 0

  echo "⚠ Missing GStreamer elements: ${missing[*]}"
  echo "  Video playback requires GStreamer plugins."

  local pkg_mgr="" install_cmd=""
  local -a pkgs=()

  if command -v pacman >/dev/null 2>&1; then
    pkg_mgr="pacman"
    pkgs=(gst-plugins-base gst-plugins-good gst-libav gst-plugins-bad)
    install_cmd="sudo pacman -S --noconfirm ${pkgs[*]}"
  elif command -v apt-get >/dev/null 2>&1; then
    pkg_mgr="apt"
    pkgs=(gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-libav gstreamer1.0-plugins-bad)
    install_cmd="sudo apt-get install -y ${pkgs[*]}"
  elif command -v dnf >/dev/null 2>&1; then
    pkg_mgr="dnf"
    pkgs=(gstreamer1-plugins-base gstreamer1-plugins-good gstreamer1-libav gstreamer1-plugins-bad-free)
    install_cmd="sudo dnf install -y ${pkgs[*]}"
  elif command -v zypper >/dev/null 2>&1; then
    pkg_mgr="zypper"
    pkgs=(gstreamer-plugins-base gstreamer-plugins-good gstreamer-plugins-libav gstreamer-plugins-bad)
    install_cmd="sudo zypper install -y ${pkgs[*]}"
  else
    echo "  ⚠ Unknown package manager. Install GStreamer plugins manually:"
    echo "    pacman: sudo pacman -S gst-plugins-base gst-plugins-good gst-libav"
    echo "    apt:    sudo apt install gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-libav"
    echo "    dnf:    sudo dnf install gstreamer1-plugins-base gstreamer1-plugins-good gstreamer1-libav"
    return 1
  fi

  echo "  Package manager: $pkg_mgr"
  echo "  Installing: ${pkgs[*]}"
  if eval "$install_cmd"; then
    echo "  ✅ GStreamer plugins installed successfully"
    return 0
  else
    echo "  ❌ Failed to install GStreamer plugins. Run manually: $install_cmd"
    return 1
  fi
}

# ─── Mode ────────────────────────────────────────────────────────────────
MODE="${1:---dev}"

case "$MODE" in
  --dev)
    # Check GStreamer on Linux before anything else
    if [ "$(uname)" = "Linux" ]; then
      ensure_gstreamer_linux || true
    fi

    # Build frontend if dist is missing, start Vite dev server, run Tauri
    echo "=== OpenScript Tauri — Dev Mode ==="
    if [ ! -d "$FRONTEND_DIR/node_modules" ]; then
      echo "Installing frontend dependencies..."
      npm --prefix "$FRONTEND_DIR" install
    fi
    echo "Building Tauri binary..."
    cargo build -p openscript-tauri --manifest-path "$SCRIPT_DIR/Cargo.toml" 2>&1 | tail -1

    # Kill any stale Vite process on the target port before starting
    if command -v lsof >/dev/null 2>&1; then
      STALE_PID=$(lsof -ti :$VITE_PORT 2>/dev/null)
      if [ -n "$STALE_PID" ]; then
        echo "  Killing stale process on port $VITE_PORT (PID $STALE_PID)..."
        kill "$STALE_PID" 2>/dev/null || true
        sleep 1
      fi
    elif command -v fuser >/dev/null 2>&1; then
      STALE_PID=$(fuser $VITE_PORT/tcp 2>/dev/null)
      if [ -n "$STALE_PID" ]; then
        echo "  Killing stale process on port $VITE_PORT (PID $STALE_PID)..."
        kill "$STALE_PID" 2>/dev/null || true
        sleep 1
      fi
    fi

    echo "Starting Vite dev server on :$VITE_PORT..."
    cd "$FRONTEND_DIR" && npx vite --port "$VITE_PORT" --strictPort >> /tmp/openscript-vite.log 2>&1 &
    VITE_PID=$!
    cd "$SCRIPT_DIR"

    # Wait for Vite to be ready (up to 15s)
    echo "Waiting for Vite..."
    for i in $(seq 1 30); do
      if curl -s -o /dev/null -w '' "http://localhost:$VITE_PORT" 2>/dev/null; then
        ELAPSED=$(echo "$i * 0.5" | bc 2>/dev/null || echo "$((i / 2)).$(( (i % 2) * 5 ))")
        echo "  Vite ready (${ELAPSED}s, attempt ${i}/30)"
        break
      fi
      [ "$i" -eq 30 ] && { echo "Vite did not start in time (15s). Check /tmp/openscript-vite.log"; exit 1; }
      sleep 0.5
    done
    ;;

  --release)
    # Check GStreamer on Linux before building
    if [ "$(uname)" = "Linux" ]; then
      ensure_gstreamer_linux || true
    fi

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

"$TAURI_BIN" >> /tmp/openscript-tauri.log 2>&1 &
TAURI_PID=$!

sleep 3
if kill -0 "$TAURI_PID" 2>/dev/null; then
  echo "  OpenScript running (PID $TAURI_PID)"
  echo "  Logs: /tmp/openscript-tauri.log"
  echo "  Press Ctrl+C to exit (stops Vite + Tauri)"
else
  echo "  Error: Tauri process exited immediately."
  cat /tmp/openscript-tauri.log
  exit 1
fi
