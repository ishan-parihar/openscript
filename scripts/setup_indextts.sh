#!/usr/bin/env bash
# Create the IndexTTS-2.5 TTS inference venv (`.venv-indextts`) for the
# OpenScript IndexTTS voice-cloning engine.
#
#   - Python 3.11 (uv provisions a managed 3.11 — index-tts requires >=3.10,<3.12)
#   - torch 2.8 + the pinned index-tts dependency set (modelscope, librosa, ...)
#   - third_party/index-tts installed as the local `indextts` package
#   - IndexTeam/IndexTTS-2.5 checkpoints (~5.7 GB) -> mcp/assets/indextts
#
# Idempotent-ish: safe to re-run; a broken venv can be rebuilt with
# `rm -rf .venv-indextts && bash scripts/setup_indextts.sh`.
#
# After setup, the Rust side auto-discovers `.venv-indextts/bin/python`
# (override with INDEXTTS_PYTHON). The checkpoint dir is mcp/assets/indextts
# (override with INDEXTTS_MODEL_DIR).
#
# LICENSE: IndexTTS-2.5 is under the bilibili Model Use License — research /
# non-commercial use; commercial use requires contacting
# indexspeech@bilibili.com.

set -euo pipefail
cd "$(dirname "$0")/.."  # repo root

TTS_DIR="third_party/index-tts"
VENV=".venv-indextts"
export PATH="${HOME}/.local/bin:${HOME}/.cargo/bin:${PATH}"

[ -d "${TTS_DIR}" ] || {
  echo "cloning index-tts (shallow)..."
  git clone --depth 1 https://github.com/index-tts/index-tts.git "${TTS_DIR}"
}

# 8GB-GPU compatibility patch (idempotent): QwenEmo classifier -> CPU +
# expandable_segments guard so an 8GB card (e.g. RTX 2060) fits the pipeline.
python3 scripts/patch_indextts_vendored.py

command -v uv >/dev/null 2>&1 || { echo "uv is required (pip install uv)" >&2; exit 1; }

echo "=== [1/3] IndexTTS inference venv (${VENV}) ==="
( cd "${TTS_DIR}" && UV_PROJECT_ENVIRONMENT="../../${VENV}" uv sync --no-dev )

echo "=== [2/3] Checkpoints -> mcp/assets/indextts (${INDEXTTS_MODEL_DIR:-mcp/assets/indextts}) ==="
MODEL_DIR="${INDEXTTS_MODEL_DIR:-mcp/assets/indextts}"
mkdir -p "${MODEL_DIR}"
if [ ! -f "${MODEL_DIR}/config.yaml" ]; then
  hf download IndexTeam/IndexTTS-2.5 --local-dir "${MODEL_DIR}"
else
  echo "checkpoints present — skipping download"
fi

echo "=== [3/3] Smoke import ==="
(cd "${MODEL_DIR}" && ../../${VENV}/bin/python -c "
import indextts.infer_v2_5 as m
print('indextts import OK — IndexTTS2 ready to load on first synth')
")

echo "IndexTTS setup complete. First synth loads the ~5.7 GB checkpoints (cold start, not a hang)."
