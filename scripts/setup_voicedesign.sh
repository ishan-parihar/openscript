#!/usr/bin/env bash
# setup_voicedesign.sh — provision the Qwen3-TTS-1.7B-VoiceDesign ONNX engine.
#
# Creates .venv-voicedesign (light deps only: onnxruntime-gpu + numpy +
# soundfile + transformers — NO torch, NO NeMo) and downloads the int4 model
# (~4.3 GB) from wavekat/Qwen3-TTS-1.7B-VoiceDesign-ONNX into
# mcp/assets/voicedesign.
#
# Idempotent: re-running skips what's already present. Safe to re-run to
# repair a partial download or upgrade deps.
#
# Usage:
#   bash scripts/setup_voicedesign.sh          # full setup (venv + model)
#   bash scripts/setup_voicedesign.sh --model-only   # download model only
#   bash scripts/setup_voicedesign.sh --venv-only    # venv only

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

VENV=".venv-voicedesign"
MODEL_DIR="mcp/assets/voicedesign"
REPO="wavekat/Qwen3-TTS-1.7B-VoiceDesign-ONNX"

MODEL_ONLY=0
VENV_ONLY=0
for arg in "$@"; do
  case "$arg" in
    --model-only) MODEL_ONLY=1 ;;
    --venv-only) VENV_ONLY=1 ;;
  esac
done

echo "=== VoiceDesign setup (repo: $ROOT) ==="

# --- 1. Python venv (uv-managed, light deps) ---------------------------------
if [ "$MODEL_ONLY" = "0" ]; then
  if [ -x "$VENV/bin/python" ]; then
    echo "venv present: $VENV (skip)"
  else
    echo "creating venv: $VENV"
    if command -v uv >/dev/null 2>&1; then
      uv venv "$VENV" --python 3.12
    else
      python3 -m venv "$VENV"
    fi
    # NOTE: pinned to onnxruntime-gpu for CUDA. If your driver/ORT combo
    # lacks CUDAExecutionProvider, drop -gpu and it falls back to CPU.
    if command -v uv >/dev/null 2>&1; then
      UV_PYTHON="$VENV/bin/python" uv pip install --python "$VENV/bin/python" \
        "onnxruntime-gpu>=1.24" "numpy>=2.0" "soundfile>=0.13" "transformers>=4.57" 2>&1 | tail -3 || {
        echo "uv install failed — retrying with plain pip"
        "$VENV/bin/pip" install -U pip wheel
        "$VENV/bin/pip" install "onnxruntime-gpu>=1.24" "numpy>=2.0" "soundfile>=0.13" "transformers>=4.57"
      }
    else
      "$VENV/bin/pip" install -U pip wheel
      "$VENV/bin/pip" install "onnxruntime-gpu>=1.24" "numpy>=2.0" "soundfile>=0.13" "transformers>=4.57"
    fi
  fi
fi

# --- 2. Model download (int4 + tokenizer + embeddings) ------------------------
if [ "$VENV_ONLY" = "0" ]; then
  if [ -f "$MODEL_DIR/config.json" ] && [ -f "$MODEL_DIR/int4/talker_prefill.onnx" ]; then
    echo "model present: $MODEL_DIR (skip)"
  else
    echo "downloading $REPO -> $MODEL_DIR (~4.3 GB int4)..."
    mkdir -p "$MODEL_DIR"
    if command -v hf >/dev/null 2>&1; then
      hf download "$REPO" --local-dir "$MODEL_DIR" \
        --include "int4/*" "embeddings/*" "tokenizer/*" "config.json" "requirements.txt"
    elif command -v huggingface-cli >/dev/null 2>&1; then
      huggingface-cli download "$REPO" --local-dir "$MODEL_DIR" \
        --include "int4/*" "embeddings/*" "tokenizer/*" "config.json" "requirements.txt"
    else
      echo "ERROR: neither 'hf' nor 'huggingface-cli' found. Install huggingface_hub."
      exit 1
    fi
  fi
fi

echo "=== VoiceDesign ready ==="
if [ -x "$VENV/bin/python" ]; then
  "$VENV/bin/python" -c "import onnxruntime, numpy, soundfile, transformers; print('deps ok: ort', onnxruntime.__version__)" 2>&1 | tail -1
fi
if [ -f "$MODEL_DIR/config.json" ]; then
  du -sh "$MODEL_DIR"
fi
