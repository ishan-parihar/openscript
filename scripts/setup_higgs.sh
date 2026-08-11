#!/usr/bin/env bash
# Create the Higgs Audio v3 TTS inference venv (`.venv-higgs`) for the
# OpenScript Higgs expressive-TTS engine, and download the ONNX export.
#
# Higgs Audio v3 (bosonai/higgs-audio-v3-tts-4b) is a 4B conversational TTS:
# 100+ languages, zero-shot voice cloning, inline emotion/prosody/style/sfx
# control tags, 24 kHz. The self-contained ONNX export we use lives at
# `onnx-community/higgs-audio-v3-tts-4b` (branch `cuda_int4`, ~3.6 GB):
#
#   llm_decoder.onnx(.data)  int4 Qwen3-4B backbone (ONNX Runtime GenAI)
#   text_embed / audio_embed / audio_heads   fused multimodal parts (ORTCUDA)
#   audio_encoder / audio_tokenizer          Higgs v2 codec (waveform<->codes)
#   tokenizer.json + genai_config.json + manifest.json
#
# What gets installed:
#   - Python 3.12 (uv provisions a managed 3.12 if the host lacks one)
#   - onnxruntime-genai(-cuda)  — the int4 llm_decoder runtime
#   - onnxruntime-gpu|onnxruntime — the audio/text sub-model runtime
#   - onnx, numpy, soundfile, tokenizers, huggingface_hub
#
# Idempotent-ish: re-runnable; a broken venv can be rebuilt with
# `rm -rf .venv-higgs && bash scripts/setup_higgs.sh`. The model download is
# resumed/skipped when the files already exist.
#
# After setup the Rust side auto-discovers `.venv-higgs/bin/python`
# (override with HIGGS_PYTHON). Model dir: `mcp/assets/higgs/cuda_int4`
# (override with HIGGS_MODEL_DIR).
#
# NOTE ON LICENSE: the Higgs Audio v3 weights are research / non-commercial
# (Boson Higgs Audio v3 Research and Non-Commercial License). Monetized use
# requires a separate commercial license from Boson.

set -euo pipefail
cd "$(dirname "$0")/.."  # repo root

VENV=".venv-higgs"
MODEL_REPO="onnx-community/higgs-audio-v3-tts-4b"
MODEL_DIR="${HIGGS_MODEL_DIR:-$(pwd)/mcp/assets/higgs/cuda_int4}"
export MODEL_REPO MODEL_DIR
export PATH="${HOME}/.local/bin:${HOME}/.cargo/bin:${PATH}"

# --- GPU detection (override with --no-gpu / HIGGS_GPU=0) --------------------
USE_GPU=1
[[ -n "${HIGGS_GPU:-}" && "${HIGGS_GPU}" == "0" ]] && USE_GPU=0
for arg in "$@"; do
  [[ "$arg" == "--no-gpu" ]] && USE_GPU=0
done
if [[ "${USE_GPU}" == "1" ]] && ! command -v nvidia-smi >/dev/null 2>&1; then
  echo "nvidia-smi not found — installing CPU-only onnxruntime." >&2
  USE_GPU=0
fi

echo "=== [0] Higgs TTS venv setup (${VENV}) — GPU=${USE_GPU} ==="

# --- venv --------------------------------------------------------------------
command -v uv >/dev/null 2>&1 || {
  echo "ERROR: uv not found — install it first (curl -LsSf https://astral.sh/uv/install.sh | sh)" >&2
  exit 1
}
rm -rf "${VENV}"
uv venv "${VENV}" --python 3.12
# shellcheck disable=SC1091
source "${VENV}/bin/activate"

echo "=== [1/4] onnxruntime (llm_decoder + audio sub-models) ==="
# The int4 llm_decoder is a standard ONNX QDQ graph — plain onnxruntime runs
# it directly with a manual KV-cache loop (no onnxruntime-genai needed).
if [[ "${USE_GPU}" == "1" ]]; then
  uv pip install onnxruntime-gpu || uv pip install onnxruntime
else
  uv pip install onnxruntime
fi
python -c "import onnxruntime as ort; print('ort', ort.__version__, ort.get_available_providers())"

echo "=== [2/4] audio/text sub-model + tokenizer deps ==="
uv pip install numpy soundfile tokenizers huggingface_hub

echo "=== [3/4] download cuda_int4 export (~3.6 GB) -> ${MODEL_DIR} ==="
mkdir -p "$(dirname "${MODEL_DIR}")"
python - <<PY
import os, shutil, sys
from pathlib import Path
from huggingface_hub import snapshot_download

target = Path(os.environ["MODEL_DIR"])
required = ["genai_config.json", "llm_decoder.onnx", "llm_decoder.onnx.data",
            "text_embed.onnx", "audio_embed.onnx", "audio_heads.onnx",
            "audio_tokenizer.onnx", "audio_encoder.onnx", "tokenizer.json"]
if all((target / f).exists() and (target / f).stat().st_size > 0 for f in required):
    print(f"model already present at {target} — skipping download")
    sys.exit(0)

snapshot_download(
    repo_id=os.environ["MODEL_REPO"],
    allow_patterns=["cuda_int4/*"],
    local_dir=str(target.parent),
    local_dir_use_symlinks=False,
)
missing = [f for f in required if not (target / f).exists()]
if missing:
    print(f"ERROR: download incomplete; missing {missing}", file=sys.stderr)
    sys.exit(1)
print(f"model download complete: {target}")
PY

echo "=== [4/4] verify ==="
"${VENV}/bin/python" - <<'PY'
import os
from pathlib import Path
import numpy, soundfile, tokenizers, onnxruntime  # noqa: F401

print("ort          :", onnxruntime.__version__)
print("providers    :", onnxruntime.get_available_providers())
d = Path(os.environ["MODEL_DIR"])
print("model files  :", sorted(p.name for p in d.iterdir())[:6], "...")
print("model size   : %.1f GB" % (sum(p.stat().st_size for p in d.glob("*")) / 1e9))
PY
echo "🎉 Higgs TTS env ready at ${VENV}/ — Rust auto-discovers it via HIGGS_PYTHON"
echo "   Model at ${MODEL_DIR}. First synth loads the pipeline (slow); then it caches."
