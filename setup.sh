#!/usr/bin/env bash
# setup.sh — One-shot bootstrap for a fresh OpenScript clone.
#
# Idempotent: safe to re-run. Detects what's already done and skips.
#
# Brings a fresh clone from `git clone` to "ready to render" by:
#   1. Verifying required tools (python3, cargo, node, ffmpeg, ffprobe)
#   2. Installing Python ML sidecar deps (kokoro-onnx, numpy)
#   3. Downloading Kokoro TTS model files (310MB + 27MB)
#   4. Building the Rust workspace (excludes Tauri — needs GDK dev headers)
#   5. Building the MCP server release binary (for smoke test)
#   6. Installing frontend npm deps (for tsc checks)
#   7. Running the post-iteration gate (build + test + tsc + lint)
#   8. Running the MCP smoke test (verifies 10 key tools work end-to-end)
#
# Optional steps (skipped if prerequisites are missing):
#   - Build the music library index (~2 minutes, requires yt-dlp)
#   - Download Parakeet TDT transcription model (only if you plan to use it)
#
# Usage:
#   bash setup.sh             # full setup
#   bash setup.sh --skip-models   # skip the 420MB Kokoro download
#   bash setup.sh --skip-build    # skip cargo build (use existing target/)
#   bash setup.sh --help
#
# Exit codes:
#   0 — setup complete, all gates pass
#   1 — prerequisite missing or step failed (see message)
#
# This script is safe to re-run. It is the single source of truth for
# "how do I get a fresh clone working?" — AGENTS.md §19 (Environment
# Recovery Protocol) points here.

set -euo pipefail

# ----------------------------------------------------------------------------
# Colors + helpers
# ----------------------------------------------------------------------------
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

ok()    { echo -e "${GREEN}✓${NC} $1"; }
warn()  { echo -e "${YELLOW}⚠${NC} $1"; }
info()  { echo -e "${BLUE}ℹ${NC} $1"; }
fail()  { echo -e "${RED}✗${NC} $1"; exit 1; }
step()  { echo -e "\n${BLUE}=== $1 ===${NC}"; }

# Resolve repo root (script lives in /scripts/)
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$REPO_ROOT"

# Parse args
SKIP_MODELS=0
SKIP_BUILD=0
for arg in "$@"; do
  case "$arg" in
    --skip-models) SKIP_MODELS=1 ;;
    --skip-build)  SKIP_BUILD=1 ;;
    --help|-h)
      grep '^#' "${BASH_SOURCE[0]}" | sed 's/^# \?//'
      exit 0
      ;;
    *) fail "Unknown arg: $arg (try --help)" ;;
  esac
done

# ----------------------------------------------------------------------------
# Step 1: Verify required tools
# ----------------------------------------------------------------------------
step "1/8 — Verifying required tools"

# Cargo is needed to build the workspace. Look in ~/.cargo/bin (survives
# container resets) and on PATH.
if [ -d "$HOME/.cargo/bin" ]; then
  export PATH="$HOME/.cargo/bin:$PATH"
fi

check_tool() {
  local tool="$1"
  local hint="$2"
  if command -v "$tool" >/dev/null 2>&1; then
    ok "$tool: $(command -v "$tool")"
  else
    fail "$tool not found. $hint"
  fi
}

check_tool python3   "Install Python 3.11+: https://www.python.org/downloads/"
check_tool cargo     "Install Rust: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
check_tool node      "Install Node.js 18+: https://nodejs.org/"
check_tool npm       "Comes with Node.js"
check_tool ffmpeg    "Install ffmpeg: sudo apt install ffmpeg  (or brew install ffmpeg)"
check_tool ffprobe   "Comes with ffmpeg"

# Optional tools (warn if missing, don't fail)
OPT_MISSING=0
for opt in yt-dlp pip3; do
  if ! command -v "$opt" >/dev/null 2>&1; then
    warn "Optional: $opt not found (some features will be unavailable)"
    OPT_MISSING=1
  else
    ok "$opt: $(command -v "$opt")"
  fi
done
if [ "$OPT_MISSING" = "1" ]; then
  info "Optional tools missing — library.build and pip installs will be skipped."
fi

# ----------------------------------------------------------------------------
# Step 2: Install Python ML sidecar deps
# ----------------------------------------------------------------------------
step "2/8 — Installing Python ML sidecar deps"

# Kokoro sidecar needs: kokoro-onnx + numpy.
# Pin a range that supports Python 3.11–3.13. Old pin kokoro-onnx==0.4.0
# fails on Python 3.13 (requires <3.13). Prefer >=0.4.4 / latest 0.5.x.
# Whisper sidecar (apex_transcriber.py) needs whisper_timestamped in the
# whisper-hindi conda env (see AGENTS.md §16).

if command -v pip3 >/dev/null 2>&1; then
  info "Installing kokoro-onnx + numpy (user-level, no virtualenv)..."
  # --user so we don't need sudo; --break-system-packages for PEP 668 systems
  # Try modern pin first; fall back to whatever pip can resolve.
  pip3 install --user --break-system-packages --quiet \
    "kokoro-onnx>=0.4.4" \
    "numpy" \
    2>&1 | tail -5 \
    || pip3 install --user --break-system-packages --quiet "kokoro-onnx" "numpy" 2>&1 | tail -5 \
    || warn "pip install failed (some Python sidecars may not work)"
  if python3 -c "import kokoro_onnx" 2>/dev/null; then
    ok "kokoro-onnx importable ($(python3 -c 'import kokoro_onnx,inspect; print(getattr(kokoro_onnx,\"__file__\",\"ok\"))' 2>/dev/null || echo ok))"
  else
    warn "kokoro-onnx still not importable — script.to_video TTS will fail until fixed"
    # Check if the user has a conda env with kokoro_onnx installed
    if [ -f "$HOME/miniconda3/envs/kokoro-tts/bin/python" ]; then
      info "Detected conda env at ~/miniconda3/envs/kokoro-tts — it may have kokoro_onnx."
      info "Set KOKORO_PYTHON=$HOME/miniconda3/envs/kokoro-tts/bin/python in your shell profile."
    fi
  fi
  ok "Python deps step finished"
else
  warn "pip3 not found; skipping Python dep install. The Kokoro sidecar will fail to start."
  warn "Install pip3 + run: pip3 install --user 'kokoro-onnx>=0.4.4' numpy"
fi

# ----------------------------------------------------------------------------
# Step 3: Download Kokoro TTS model files
# ----------------------------------------------------------------------------
step "3/8 — Downloading Kokoro TTS model files"

KOKORO_ONNX_URL="https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/kokoro-v1.0.onnx"
KOKORO_VOICES_URL="https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/voices-v1.0.bin"

KOKORO_ONNX_PATH="mcp/assets/kokoro/onnx/kokoro-v1.0.onnx"
KOKORO_VOICES_PATH="mcp/assets/kokoro/voices/voices-v1.0.bin"

mkdir -p mcp/assets/kokoro/onnx mcp/assets/kokoro/voices

if [ "$SKIP_MODELS" = "1" ]; then
  warn "Skipping model downloads (--skip-models)"
else
  # Kokoro ONNX model (~310MB)
  if [ -f "$KOKORO_ONNX_PATH" ] && [ "$(stat -c%s "$KOKORO_ONNX_PATH" 2>/dev/null || stat -f%z "$KOKORO_ONNX_PATH")" -gt 100000000 ]; then
    ok "Kokoro ONNX model already present ($(du -h "$KOKORO_ONNX_PATH" | cut -f1))"
  else
    info "Downloading Kokoro ONNX model (~310MB)..."
    curl -L --fail --progress-bar -o "$KOKORO_ONNX_PATH" "$KOKORO_ONNX_URL" \
      || fail "Download failed: $KOKORO_ONNX_URL"
    ok "Kokoro ONNX model downloaded"
  fi

  # Kokoro voices file (~27MB)
  if [ -f "$KOKORO_VOICES_PATH" ] && [ "$(stat -c%s "$KOKORO_VOICES_PATH" 2>/dev/null || stat -f%z "$KOKORO_VOICES_PATH")" -gt 10000000 ]; then
    ok "Kokoro voices file already present ($(du -h "$KOKORO_VOICES_PATH" | cut -f1))"
  else
    info "Downloading Kokoro voices file (~27MB)..."
    curl -L --fail --progress-bar -o "$KOKORO_VOICES_PATH" "$KOKORO_VOICES_URL" \
      || fail "Download failed: $KOKORO_VOICES_URL"
    ok "Kokoro voices file downloaded"
  fi
fi

# ----------------------------------------------------------------------------
# Step 3b: Download Parakeet TDT ONNX models (for caption alignment)
# Parakeet TDT 0.6b v3 — accurate word-level timestamps for captions.
# Without these, script.build_captions falls back to even-spacing estimation.
# Total download: ~320MB (encoder ~310MB int8 + decoder ~5MB + vocab <1KB).
# Skipped if models already present. Source: huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx
# ----------------------------------------------------------------------------
PARAKEET_DIR="mcp/assets/parakeet"
PARAKEET_BASE_URL="https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main"
PARAKEET_ENCODER="$PARAKEET_DIR/encoder-model.int8.onnx"
PARAKEET_DECODER="$PARAKEET_DIR/decoder_joint-model.int8.onnx"
PARAKEET_VOCAB="$PARAKEET_DIR/vocab.txt"

mkdir -p "$PARAKEET_DIR"

if [ "$SKIP_MODELS" = "1" ]; then
  warn "Skipping Parakeet model downloads (--skip-models)"
else
  if [ -f "$PARAKEET_ENCODER" ] && [ -f "$PARAKEET_DECODER" ] && [ -f "$PARAKEET_VOCAB" ]; then
    ok "Parakeet TDT models already present ($(du -sh "$PARAKEET_DIR" | cut -f1))"
  else
    info "Downloading Parakeet TDT ONNX models (~320MB)..."
    curl -L --fail --progress-bar -o "$PARAKEET_ENCODER" "$PARAKEET_BASE_URL/encoder-model.int8.onnx" \
      || fail "Download failed: encoder-model.int8.onnx"
    curl -L --fail --progress-bar -o "$PARAKEET_DECODER" "$PARAKEET_BASE_URL/decoder_joint-model.int8.onnx" \
      || fail "Download failed: decoder_joint-model.int8.onnx"
    curl -L --fail --progress-bar -o "$PARAKEET_VOCAB" "$PARAKEET_BASE_URL/vocab.txt" \
      || fail "Download failed: vocab.txt"
    ok "Parakeet TDT models downloaded"
  fi
fi

# Also ensure onnxruntime is installed (needed by parakeet_align.py)
if python3 -c "import onnxruntime" 2>/dev/null; then
  ok "onnxruntime already installed"
elif python3 -c "import onnxruntime" 2>/dev/null; then
  ok "onnxruntime already installed (conda)"
else
  info "Installing onnxruntime for Parakeet alignment..."
  pip3 install --user onnxruntime 2>&1 | tail -3 \
    || warn "onnxruntime install failed — Parakeet alignment will be unavailable"
fi

# Also ensure librosa is installed (needed by parakeet_align.py)
if python3 -c "import librosa" 2>/dev/null; then
  ok "librosa already installed"
else
  info "Installing librosa for Parakeet alignment..."
  pip3 install --user librosa 2>&1 | tail -3 \
    || warn "librosa install failed — Parakeet alignment will be unavailable"
fi

# ----------------------------------------------------------------------------
# Step 4: Build the Rust workspace
# ----------------------------------------------------------------------------
step "4/8 — Building Rust workspace"

if [ "$SKIP_BUILD" = "1" ]; then
  warn "Skipping cargo build (--skip-build)"
else
  info "cargo build --workspace --exclude openscript-tauri (this takes a few minutes)..."
  cargo build --workspace --exclude openscript-tauri 2>&1 | tail -5 \
    || fail "cargo build failed"
  ok "Rust workspace built"
fi

# ----------------------------------------------------------------------------
# Step 5: Build MCP server release binary (for smoke test)
# ----------------------------------------------------------------------------
step "5/8 — Building MCP server release binary"

if [ "$SKIP_BUILD" = "1" ]; then
  warn "Skipping release build (--skip-build)"
else
  info "cargo build -p openscript-mcp --release --bin mcp-server (for smoke test)..."
  cargo build -p openscript-mcp --release --bin mcp-server 2>&1 | tail -5 \
    || fail "release build failed"
  ok "MCP server release binary built"
fi

# ----------------------------------------------------------------------------
# Step 6: Install frontend npm deps (for tsc checks)
# ----------------------------------------------------------------------------
step "6/8 — Installing frontend npm deps"

FRONTEND_DIR="crates/openscript-tauri/src/frontend"
if [ -d "$FRONTEND_DIR" ]; then
  if [ -d "$FRONTEND_DIR/node_modules" ]; then
    ok "node_modules already present"
  else
    info "npm install (frontend)..."
    (cd "$FRONTEND_DIR" && npm install --no-audit --no-fund 2>&1 | tail -3) \
      || fail "npm install failed"
    ok "Frontend deps installed"
  fi
else
  warn "Frontend dir not found at $FRONTEND_DIR — skipping"
fi

# ----------------------------------------------------------------------------
# Step 7: Run post-iteration gate (build + test + tsc + lint)
# ----------------------------------------------------------------------------
step "7/8 — Running post-iteration gate"

if [ -x "scripts/post-iteration.sh" ]; then
  # The gate ends with `git push origin main` which we don't want here
  # (the user may not have push access yet, and there's nothing to push
  # since setup.sh doesn't make code changes). So we run the individual
  # checks instead of the full gate.
  info "cargo build (zero warnings)..."
  cargo build --workspace --exclude openscript-tauri 2>&1 | tail -3 \
    || fail "cargo build failed"

  info "cargo test..."
  TEST_OUTPUT=$(cargo test --workspace --exclude openscript-tauri --lib --bins --tests 2>&1) \
    || { echo "$TEST_OUTPUT" | tail -30; fail "cargo test failed"; }
  TEST_COUNT=$(echo "$TEST_OUTPUT" | grep -E "^test result:" | awk '{n+=$4} END {print n+0}')
  ok "$TEST_COUNT tests pass"

  if [ -d "$FRONTEND_DIR/node_modules" ]; then
    info "npx tsc --noEmit..."
    (cd "$FRONTEND_DIR" && npx tsc --noEmit 2>&1 | tail -3) \
      || fail "tsc failed"
    ok "tsc clean"
  fi

  info "workspace-lint..."
  python3 scripts/workspace-lint/workspace_lint.py 2>&1 | tail -3 \
    || fail "workspace-lint failed"
  ok "workspace-lint clean"
else
  warn "scripts/post-iteration.sh not found — skipping gate"
fi

# ----------------------------------------------------------------------------
# Step 8: Run MCP smoke test
# ----------------------------------------------------------------------------
step "8/8 — Running MCP smoke test"

if [ -x "scripts/smoke_test_mcp.sh" ]; then
  bash scripts/smoke_test_mcp.sh 2>&1 | tail -10 \
    || fail "MCP smoke test failed"
  ok "MCP smoke test passed"
else
  warn "scripts/smoke_test_mcp.sh not found — skipping smoke test"
fi

# ----------------------------------------------------------------------------
# Media doctor (keys, portable packs, production readiness)
# ----------------------------------------------------------------------------
step "Media bootstrap doctor"
if [ -x "scripts/bootstrap_media.sh" ]; then
  bash scripts/bootstrap_media.sh --probe-only 2>&1 | tail -25 \
    || warn "bootstrap_media probe reported issues (see docs/INSTALL.md)"
  if [ -f "mcp/assets/music_production/index.json" ]; then
    ok "music_production pack present (cold-start beds)"
  fi
  if [ -d "mcp/assets/sfx_pack" ]; then
    ok "portable sfx_pack present"
  fi
else
  warn "scripts/bootstrap_media.sh missing"
fi

# ----------------------------------------------------------------------------
# Optional: Build the music library index (large; portable pack is enough for cold-start)
# ----------------------------------------------------------------------------
if [ -f "mcp/assets/music_library_index.json" ]; then
  ok "Music library index already present ($(du -h mcp/assets/music_library_index.json | cut -f1))"
elif command -v yt-dlp >/dev/null 2>&1; then
  echo ""
  info "Optional: tagged YT music library — run when you want library.search:"
  info "  bash scripts/bootstrap_media.sh --with-library"
else
  warn "yt-dlp not found — YouTube stock/music fallback unavailable."
fi

# ----------------------------------------------------------------------------
# Done
# ----------------------------------------------------------------------------
echo ""
ok "Setup complete!"
echo ""
info "Next steps:"
info "  1. Keys:   bash scripts/setup_openscript_config.sh --pexels-key … --giphy-key …"
info "  2. Doctor: bash scripts/bootstrap_media.sh   (or MCP system.doctor)"
info "  3. CLI:    ./target/debug/openscript --help"
info "  4. MCP:    ./target/release/mcp-server"
info "  5. Docs:   docs/INSTALL.md · AGENT_GUIDE.md · AGENTS.md"
echo ""
info "If anything above failed, see AGENTS.md §16 (Getting Unstuck)."
