#!/usr/bin/env bash
# setup.sh — One-shot bootstrap for a fresh OpenScript clone.
#
# Idempotent: safe to re-run. Detects what's already done and skips.
#
# FEATURE-GATED PROVISIONING (cold-start): which deps get downloaded and which
# engines get built is driven by the ACTIVE CONFIGURATION — the same toggles
# the runtime gates on. Toggles live in ~/.openscript/config.json under
# `features.<category>.<name>` (all default ON) and can be overridden per-run
# with OPENSCRIPT_FEATURE_<CATEGORY>_<NAME>=0|1 or `--feature cat.name=0`.
#
#   bash setup.sh --list-features   # print the toggle table + what each pulls
#   OPENSCRIPT_FEATURE_TTS_VOICEDESIGN=0 bash setup.sh   # skip the 4.3GB model
#   bash setup.sh --feature tts.gepard=0                 # skip the heavy venv
#
# Only the deps for ENABLED features are downloaded/built:
#   - tts.kokoro        → kokoro-onnx pip deps + Kokoro model (~340MB)
#   - tts.voicedesign   → scripts/setup_voicedesign.sh (int4 model ~4.3GB + venv)
#   - tts.gepard        → scripts/setup_gepard.sh (CUDA torch + NeMo venv)
#   - tts.audio8        → scripts/setup_audio8.sh (int4 model ~1GB + pip deps)
#   - transcription.hinglish_ggml → whisper.cpp build + GGML model
#   - transcription.parakeet_align→ Parakeet ONNX (~320MB) + onnxruntime/librosa
#   - frontend          → npm install (Tauri/React web UI)
#
# Brings a fresh clone from `git clone` to "ready to render" by:
#   1. Verifying required tools (python3, cargo, node, ffmpeg, ffprobe)
#   2. Installing Python ML sidecar deps (feature-gated)
#   3. Downloading TTS models (feature-gated per engine)
#   4. Building the Rust workspace (excludes Tauri — needs GDK dev headers)
#   5. Building the MCP server release binary (for smoke test)
#   6. Installing frontend npm deps (only if the frontend feature is active)
#   7. Running the post-iteration gate (build + test + tsc + lint)
#   8. Running the MCP smoke test (verifies key tools work end-to-end)
#
# Optional steps (skipped if prerequisites are missing):
#   - Build the music library index (~2 minutes, requires yt-dlp)
#   - Download Parakeet TDT transcription model (only if the feature is active)
#
# Usage:
#   bash setup.sh             # full setup (all enabled features)
#   bash setup.sh --skip-models   # skip the model downloads
#   bash setup.sh --skip-build    # skip cargo build (use existing target/)
#   bash setup.sh --list-features # show the feature toggle table
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

# Resolve repo root (script lives in /scripts/ for the setup_*.sh helpers)
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$REPO_ROOT"

# ----------------------------------------------------------------------------
# Feature toggle resolution
# ----------------------------------------------------------------------------
# Reads toggles from ~/.openscript/config.json `features.<category>.<name>`,
# overridable by OPENSCRIPT_FEATURE_<CATEGORY>_<NAME> env vars (0/1/true/false).
# Emits F_<CATEGORY>_<NAME>=0|1 shell vars. Missing python3 or config → all ON.
load_features() {
  if command -v python3 >/dev/null 2>&1; then
    eval "$(python3 - <<'PY' 2>/dev/null
import json, os
from pathlib import Path
cfg = {}
p = Path.home() / ".openscript" / "config.json"
if p.exists():
    try:
        cfg = json.loads(p.read_text()).get("features", {}) or {}
    except Exception:
        cfg = {}
def enabled(cat, name):
    v = os.environ.get("OPENSCRIPT_FEATURE_%s_%s" % (cat.upper(), name.upper()))
    if v is not None:
        return 0 if str(v).strip().lower() in ("0", "false", "no", "off") else 1
    return 1 if cfg.get(cat, {}).get(name, True) else 0
cats = {
    "TTS": ["kokoro", "audio8", "gepard", "voicedesign", "sidecar"],
    "TRANSCRIPTION": ["hinglish_ggml", "whisper_align", "parakeet_align"],
    "MEDIA": ["pexels", "giphy", "pixabay", "youtube"],
    "LLM": ["opencode", "openrouter"],
    "RENDER": ["ffmpeg", "hyperframes", "remotion", "nvenc"],
}
for cat, names in cats.items():
    for n in names:
        print("F_%s_%s=%d" % (cat, n.upper(), enabled(cat.lower(), n)))
print("F_FRONTEND=%d" % enabled("frontend", "frontend"))
PY
)"
  else
    F_TTS_KOKORO=1; F_TTS_AUDIO8=1; F_TTS_GEPARD=1; F_TTS_VOICEDESIGN=1; F_TTS_SIDECAR=1
    F_TRANSCRIPTION_HINGLISH_GGML=1; F_TRANSCRIPTION_WHISPER_ALIGN=1; F_TRANSCRIPTION_PARAKEET_ALIGN=1
    F_MEDIA_PEXELS=1; F_MEDIA_GIPHY=1; F_MEDIA_PIXABAY=1; F_MEDIA_YOUTUBE=1
    F_LLM_OPENCODE=1; F_LLM_OPENROUTER=1
    F_RENDER_FFMPEG=1; F_RENDER_HYPERFRAMES=1; F_RENDER_REMOTION=1; F_RENDER_NVENC=1
    F_FRONTEND=1
  fi
}

print_features() {
  echo ""
  echo "=== Feature toggles (drives what setup.sh provisions) ==="
  echo "Config: ~/.openscript/config.json → features.<category>.<name>  (all default ON)"
  echo "Env override: OPENSCRIPT_FEATURE_<CATEGORY>_<NAME>=0|1"
  echo ""
  printf "  %-42s %s\n" "TOGGLE" "STATE / DEP PROVISIONED"
  printf "  %-42s %s\n" "-----" "-----"
  printf "  %-42s %s\n" "tts.kokoro"       "$F_TTS_KOKORO / kokoro-onnx + Kokoro model (~340MB)"
  printf "  %-42s %s\n" "tts.audio8"       "$F_TTS_AUDIO8 / Audio8 int4 model (~1GB) + pip deps"
  printf "  %-42s %s\n" "tts.gepard"       "$F_TTS_GEPARD / .venv-gepard (CUDA torch + NeMo, heavy)"
  printf "  %-42s %s\n" "tts.voicedesign"  "$F_TTS_VOICEDESIGN / Qwen3 VoiceDesign int4 (~4.3GB) + venv"
  printf "  %-42s %s\n" "tts.sidecar"      "$F_TTS_SIDECAR / remote voicebox server (no local deps)"
  printf "  %-42s %s\n" "transcription.hinglish_ggml" "$F_TRANSCRIPTION_HINGLISH_GGML / whisper.cpp build + GGML model"
  printf "  %-42s %s\n" "transcription.whisper_align" "$F_TRANSCRIPTION_WHISPER_ALIGN / openai-whisper pip pkg"
  printf "  %-42s %s\n" "transcription.parakeet_align" "$F_TRANSCRIPTION_PARAKEET_ALIGN / Parakeet ONNX (~320MB)"
  printf "  %-42s %s\n" "media.pexels|giphy|pixabay|youtube" "$F_MEDIA_PEXELS$F_MEDIA_GIPHY$F_MEDIA_PIXABAY$F_MEDIA_YOUTUBE / keys + yt-dlp (no downloads)"
  printf "  %-42s %s\n" "llm.opencode|openrouter" "$F_LLM_OPENCODE$F_LLM_OPENROUTER / keys only"
  printf "  %-42s %s\n" "render.ffmpeg|hyperframes|remotion|nvenc" "$F_RENDER_FFMPEG$F_RENDER_HYPERFRAMES$F_RENDER_REMOTION$F_RENDER_NVENC / binaries + repo assets"
  printf "  %-42s %s\n" "frontend"         "$F_FRONTEND / npm install (Tauri/React UI)"
  echo ""
}

# ----------------------------------------------------------------------------
# Parse args
# ----------------------------------------------------------------------------
SKIP_MODELS=0
SKIP_BUILD=0
LIST_FEATURES=0
ARGS=("$@")
i=0
while [ $i -lt ${#ARGS[@]} ]; do
  arg="${ARGS[$i]}"
  case "$arg" in
    --skip-models) SKIP_MODELS=1 ;;
    --skip-build)  SKIP_BUILD=1 ;;
    --list-features) LIST_FEATURES=1 ;;
    --feature=*)
      # --feature tts.voicedesign=0  →  export OPENSCRIPT_FEATURE_TTS_VOICEDESIGN=0
      val="${arg#--feature=}"
      name="${val%%=*}"; value="${val#*=}"
      envname="$(printf '%s' "$name" | tr '[:lower:].' '[:upper:]_')"
      export "OPENSCRIPT_FEATURE_$envname"="$value"
      ;;
    --feature)
      # two-arg form: --feature tts.voicedesign 0
      i=$((i+1))
      name="${ARGS[$i]:-}"
      i=$((i+1))
      value="${ARGS[$i]:-}"
      if [ -n "$name" ] && [ -n "$value" ]; then
        envname="$(printf '%s' "$name" | tr '[:lower:].' '[:upper:]_')"
        export "OPENSCRIPT_FEATURE_$envname"="$value"
      else
        warn "ignoring --feature (expected --feature category.name 0|1)"
      fi
      ;;
    --help|-h)
      grep '^#' "${BASH_SOURCE[0]}" | sed 's/^# \?//'
      exit 0
      ;;
    *) fail "Unknown arg: $arg (try --help)" ;;
  esac
  i=$((i+1))
done

load_features
if [ "${LIST_FEATURES:-0}" = "1" ]; then
  print_features
  exit 0
fi

# ----------------------------------------------------------------------------
# Step 1: Verify required tools
# ----------------------------------------------------------------------------
step "1/9 — Verifying required tools"

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
# Step 2: Install Python ML sidecar deps (feature-gated)
# ----------------------------------------------------------------------------
step "2/9 — Installing Python ML sidecar deps"

# whisper.cpp + GGML model — only when the HinglishGgml transcription feature
# is active. HinglishGgml transcription uses whisper.cpp + GGML model (see
# hinglish_ggml_transcriber.py). Build whisper.cpp from source if not already
# built, install libs to ~/.local/lib/.
if [ "${F_TRANSCRIPTION_HINGLISH_GGML:-1}" = "1" ]; then
  WHISPER_SRC="$HOME/.local/src/whisper.cpp"
  WHISPER_LIB_DIR="$HOME/.local/lib"
  mkdir -p "$WHISPER_LIB_DIR"
  if [ ! -f "$HOME/.local/bin/whisper-cli" ] || [ ! -f "$WHISPER_LIB_DIR/libwhisper.so.1" ]; then
      echo "[setup] Building whisper.cpp from source..."
      mkdir -p "$HOME/.local/src"
      rm -rf "$WHISPER_SRC"
      git clone --depth 1 https://github.com/ggerganov/whisper.cpp.git "$WHISPER_SRC" || { echo "[setup] ERROR: Failed to clone whisper.cpp"; exit 1; }
      ( cd "$WHISPER_SRC" && cmake -B build -DGGML_NATIVE=ON && cmake --build build --config Release -j"$(nproc)" ) || { echo "[setup] ERROR: Failed to build whisper.cpp"; exit 1; }
      cp "$WHISPER_SRC/build/bin/whisper-cli" "$HOME/.local/bin/whisper-cli"
      cp "$WHISPER_SRC/build/bin/libwhisper.so"* "$WHISPER_LIB_DIR/"
      cp "$WHISPER_SRC/build/bin/libggml"*.so* "$WHISPER_LIB_DIR/"
      cp "$WHISPER_SRC/build/bin/libparakeet.so"* "$WHISPER_LIB_DIR/"
      echo "[setup] Whisper shared libraries installed to $WHISPER_LIB_DIR"
  else
      echo "[setup] whisper-cli already installed at $HOME/.local/bin/whisper-cli"
  fi
else
  warn "Skipping whisper.cpp build — transcription.hinglish_ggml is disabled."
fi

# Kokoro sidecar deps — only when the tts.kokoro feature is active.
if [ "${F_TTS_KOKORO:-1}" = "1" ]; then
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
else
  warn "Skipping kokoro-onnx pip deps — tts.kokoro is disabled."
fi

# ----------------------------------------------------------------------------
# Step 3: Download TTS model files (feature-gated)
# ----------------------------------------------------------------------------
step "3/9 — Downloading TTS model files"

KOKORO_ONNX_URL="https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/kokoro-v1.0.onnx"
KOKORO_VOICES_URL="https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/voices-v1.0.bin"

KOKORO_ONNX_PATH="mcp/assets/kokoro/onnx/kokoro-v1.0.onnx"
KOKORO_VOICES_PATH="mcp/assets/kokoro/voices/voices-v1.0.bin"

mkdir -p mcp/assets/kokoro/onnx mcp/assets/kokoro/voices

if [ "$SKIP_MODELS" = "1" ]; then
  warn "Skipping model downloads (--skip-models)"
elif [ "${F_TTS_KOKORO:-1}" != "1" ]; then
  warn "Skipping Kokoro model download — tts.kokoro is disabled."
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
# Step 3a: Per-engine TTS provisioning (feature-gated)
#   voicedesign → setup_voicedesign.sh (~4.3GB model + .venv-voicedesign)
#   gepard      → setup_gepard.sh (CUDA torch + NeMo + .venv-gepard)
#   audio8      → setup_audio8.sh (~1GB int4 model + pip deps)
# ----------------------------------------------------------------------------
step "3a/9 — Provisioning active TTS engines"

if [ "${F_TTS_VOICEDESIGN:-1}" = "1" ]; then
  if [ -x "scripts/setup_voicedesign.sh" ]; then
    info "Provisioning VoiceDesign (Qwen3 int4 ~4.3GB + .venv-voicedesign)..."
    bash scripts/setup_voicedesign.sh || warn "setup_voicedesign.sh reported issues (see above)"
  else
    warn "tts.voicedesign is enabled but scripts/setup_voicedesign.sh is missing"
  fi
else
  warn "Skipping VoiceDesign provision — tts.voicedesign is disabled."
fi

if [ "${F_TTS_GEPARD:-1}" = "1" ]; then
  if [ -x "scripts/setup_gepard.sh" ]; then
    info "Provisioning Gepard (.venv-gepard: CUDA torch + NeMo + gepard-inference)..."
    bash scripts/setup_gepard.sh || warn "setup_gepard.sh reported issues (see above)"
  else
    warn "tts.gepard is enabled but scripts/setup_gepard.sh is missing"
  fi
else
  warn "Skipping Gepard provision — tts.gepard is disabled."
fi

if [ "${F_TTS_AUDIO8:-1}" = "1" ]; then
  if [ -x "scripts/setup_audio8.sh" ]; then
    info "Provisioning Audio8 (int4 model ~1GB + pip deps)..."
    bash scripts/setup_audio8.sh || warn "setup_audio8.sh reported issues (see above)"
  else
    warn "tts.audio8 is enabled but scripts/setup_audio8.sh is missing"
  fi
else
  warn "Skipping Audio8 provision — tts.audio8 is disabled."
fi

# ----------------------------------------------------------------------------
# Step 3b: Download Parakeet TDT ONNX models (for caption alignment)
# Parakeet TDT 0.6b v3 — accurate word-level timestamps for captions.
# Without these, script.build_captions falls back to even-spacing estimation.
# Total download: ~320MB (encoder ~310MB int8 + decoder ~5MB + vocab <1KB).
# Skipped if models already present or the feature is disabled.
# Source: huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx
# ----------------------------------------------------------------------------
PARAKEET_DIR="mcp/assets/parakeet"
PARAKEET_BASE_URL="https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main"
PARAKEET_ENCODER="$PARAKEET_DIR/encoder-model.int8.onnx"
PARAKEET_DECODER="$PARAKEET_DIR/decoder_joint-model.int8.onnx"
PARAKEET_VOCAB="$PARAKEET_DIR/vocab.txt"

mkdir -p "$PARAKEET_DIR"

if [ "$SKIP_MODELS" = "1" ]; then
  warn "Skipping Parakeet model downloads (--skip-models)"
elif [ "${F_TRANSCRIPTION_PARAKEET_ALIGN:-1}" != "1" ]; then
  warn "Skipping Parakeet models — transcription.parakeet_align is disabled."
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
if [ "${F_TRANSCRIPTION_PARAKEET_ALIGN:-1}" = "1" ]; then
  if python3 -c "import onnxruntime" 2>/dev/null; then
    ok "onnxruntime already installed"
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
fi

# ----------------------------------------------------------------------------
# Step 4: Build the Rust workspace
# ----------------------------------------------------------------------------
step "4/9 — Building Rust workspace"

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
step "5/9 — Building MCP server release binary"

if [ "$SKIP_BUILD" = "1" ]; then
  warn "Skipping release build (--skip-build)"
else
  info "cargo build -p openscript-mcp --release --bin mcp-server (for smoke test)..."
  cargo build -p openscript-mcp --release --bin mcp-server 2>&1 | tail -5 \
    || fail "release build failed"
  ok "MCP server release binary built"
fi

# ----------------------------------------------------------------------------
# Step 6: Install frontend npm deps (only if the frontend feature is active)
# ----------------------------------------------------------------------------
step "6/9 — Installing frontend npm deps"

FRONTEND_DIR="crates/openscript-tauri/src/frontend"
if [ "${F_FRONTEND:-1}" != "1" ]; then
  warn "Skipping frontend npm install — frontend feature is disabled."
elif [ -d "$FRONTEND_DIR" ]; then
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
step "7/9 — Running post-iteration gate"

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
step "8/9 — Running MCP smoke test"

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
step "9/9 — Media bootstrap doctor"
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
info "  1. Keys:   bash scripts/setup_openscript_config.sh --pexels-key … --giphy-key … --pixabay-key …"
info "  2. Doctor: bash scripts/bootstrap_media.sh   (or MCP system.doctor)"
info "  3. CLI:    ./target/debug/openscript --help"
info "  4. MCP:    ./target/release/mcp-server"
info "  5. Docs:   docs/INSTALL.md · AGENT_GUIDE.md · AGENTS.md"
echo ""
info "Feature toggles: bash setup.sh --list-features  (config: ~/.openscript/config.json → features.*)"
info "If anything above failed, see AGENTS.md §16 (Getting Unstuck)."
