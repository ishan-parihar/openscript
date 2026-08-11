#!/usr/bin/env bash
# setup_audio8.sh — provision the Audio8 zero-shot voice-cloning engine
# (Audio8 TTS Preview 0.6B, ONNX INT4, ~1GB).
#
# Downloads the model into mcp/assets/audio8/model (the exact layout the
# audio8_tts_sidecar.py runtime expects) and installs the light pip deps
# (onnxruntime + numpy + soundfile — NO torch, NO NeMo).
#
# Audio8 runs on the base Python (AUDIO8_PYTHON env var override) — matching
# how the sidecar resolves its interpreter — so deps are installed user-level.
#
# Idempotent: re-running skips what's already present. Safe to re-run to
# repair a partial download or upgrade deps.
#
# Usage:
#   bash scripts/setup_audio8.sh          # full setup (deps + model)
#   bash scripts/setup_audio8.sh --model-only   # download model only

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

MODEL_DIR="mcp/assets/audio8/model"
REPO="Audio8/Audio8-TTS-Preview-0.6B-ONNX-INT4"

MODEL_ONLY=0
for arg in "$@"; do
  case "$arg" in
    --model-only) MODEL_ONLY=1 ;;
  esac
done

echo "=== Audio8 setup (repo: $ROOT) ==="

# --- 1. Python deps (user-level; onnxruntime + numpy + soundfile) -----------
if [ "$MODEL_ONLY" = "0" ]; then
  PY="${AUDIO8_PYTHON:-python3}"
  echo "using python: $PY"
  if "$PY" -c "import onnxruntime, numpy, soundfile" 2>/dev/null; then
    echo "audio8 python deps already present (skip)"
  else
    echo "installing onnxruntime + numpy + soundfile (user-level)..."
    "$PY" -m pip install --user --break-system-packages --quiet \
      "onnxruntime>=1.20" "numpy>=1.24" "soundfile>=0.13" 2>&1 | tail -3 \
      || {
        echo "pip install failed — retrying without --break-system-packages"
        "$PY" -m pip install --user --quiet "onnxruntime>=1.20" "numpy>=1.24" "soundfile>=0.13" 2>&1 | tail -3 \
          || { echo "ERROR: could not install audio8 python deps" >&2; exit 1; }
      }
    if "$PY" -c "import onnxruntime, numpy, soundfile" 2>/dev/null; then
      echo "audio8 python deps ok: $( "$PY" -c 'import onnxruntime; print(onnxruntime.__version__)' )"
    else
      echo "WARN: deps installed but not importable — check your Python env" >&2
    fi
  fi
fi

# --- 2. Model download (~1GB int4) ------------------------------------------
if [ -f "$MODEL_DIR/runtime_manifest.json" ] && [ -f "$MODEL_DIR/slow_ar_int4.onnx" ]; then
  echo "model present: $MODEL_DIR (skip)"
else
  echo "downloading $REPO -> $MODEL_DIR (~1GB int4)..."
  mkdir -p "$MODEL_DIR"
  if command -v hf >/dev/null 2>&1; then
    hf download "$REPO" --local-dir "$MODEL_DIR"
  elif command -v huggingface-cli >/dev/null 2>&1; then
    huggingface-cli download "$REPO" --local-dir "$MODEL_DIR"
  else
    echo "ERROR: neither 'hf' nor 'huggingface-cli' found. Install huggingface_hub:" >&2
    echo "  python3 -m pip install -U 'huggingface_hub[cli]'" >&2
    exit 1
  fi
fi

echo "=== Audio8 ready ==="
if [ -f "$MODEL_DIR/runtime_manifest.json" ]; then
  du -sh "$MODEL_DIR"
fi
