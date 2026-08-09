#!/usr/bin/env bash
# Create the Gepard TTS inference venv (`.venv-gepard`) for the OpenScript
# Gepard voice-cloning engine.
#
#   - Python 3.12 (uv provisions a managed 3.12 if the host lacks one)
#   - CUDA-matched PyTorch (cu128 wheels — runs on the RTX 2060 SUPER / sm_75)
#   - NeMo TTS codec stack (nemo-toolkit[tts]==2.4.0)
#   - gepard[inference] installed -e from third_party/gepard-inference
#     (re-pins transformers==5.3.0 after NeMo)
#   - torchcodec ABI-matched to the installed torch
#
# Reuses the vendored upstream helpers (`env_common.sh`) for the gnarly,
# machine-specific parts (CUDA wheel-tag detection, NeMo install ordering,
# torchcodec trial-match, CUDA self-heal) — no reimplementation.
#
# Idempotent-ish: safe to re-run; a broken venv can be rebuilt with
# `rm -rf .venv-gepard && bash scripts/setup_gepard.sh`.
#
# After setup, the Rust side auto-discovers `.venv-gepard/bin/python`
# (override with GEPARD_PYTHON). The Gepard checkpoint downloads from
# HuggingFace on first synth (nineninesix/gepard-1.0, not gated — no HF token
# needed; override with GEPARD_CHECKPOINT).

set -euo pipefail
cd "$(dirname "$0")/.."  # repo root

GEPARD_DIR="third_party/gepard-inference"
VENV=".venv-gepard"
export PATH="${HOME}/.local/bin:${HOME}/.cargo/bin:${PATH}"

[ -d "${GEPARD_DIR}" ] || {
  echo "ERROR: ${GEPARD_DIR} missing — clone the reference inference stack:" >&2
  echo "  git clone --depth 1 https://github.com/nineninesix-ai/gepard-inference.git ${GEPARD_DIR}" >&2
  exit 1
}

echo "=== [0] Gepard inference venv setup (${VENV}) ==="
# shellcheck disable=SC1091
. "${GEPARD_DIR}/scripts/lib/env_common.sh"

# The upstream helpers operate relative to the gepard repo root.
cd "${GEPARD_DIR}"

# A re-run rebuilds from scratch (uv venv refuses to overwrite an existing
# venv, and a half-installed one is not a usable cache).
rm -rf "../../${VENV}"
make_venv "../../${VENV}"          # uv-provisioned Python 3.12
source "../../${VENV}/bin/activate"

# Driver 610.x reports "CUDA UMD Version: 13.3" instead of the legacy
# "CUDA Version:" line the upstream detector greps for — shadow it so any
# present driver resolves to cu128 (the upstream-documented portable ceiling:
# runs on any driver >= 12.8, incl. CUDA 13.x). No driver -> CPU torch.
cuda_wheel_tag() {
  if ! command -v nvidia-smi >/dev/null 2>&1; then echo ""; return; fi
  echo "cu128"
}

echo "=== [1/5] CUDA-matched PyTorch ==="
TAG="$(cuda_wheel_tag)"
[[ -n "${TAG}" ]] && INDEX="https://download.pytorch.org/whl/${TAG}" || INDEX=""
uv_install_torch "${INDEX}" torch torchaudio
python -c "import torch; print('torch', torch.__version__, '| cuda:', torch.cuda.is_available())"

echo "=== [2/5] NeMo codec stack ==="
install_codec_stack

echo "=== [3/5] gepard[inference] (re-pins transformers 5.3.0) ==="
uv_install_package inference

echo "=== [4/5] torchcodec ABI-match ==="
fix_torchcodec

echo "=== [5/5] Self-heal CUDA ==="
verify_cuda_selfheal "${INDEX}" torch torchaudio

cd ../..
echo "=== Verify (fresh venv python) ==="
"${VENV}/bin/python" - <<'PY'
import torch, transformers, soundfile  # noqa: F401
print("torch       :", torch.__version__, "| cuda:", torch.cuda.is_available())
print("transformers:", transformers.__version__)
import gepard_inference  # noqa: F401
print("gepard_inference import ✓")
from nemo.collections.tts.models import AudioCodecModel  # noqa: F401
print("nemo codec import ✓")
PY
echo "🎉 Gepard TTS env ready at ${VENV}/  — Rust auto-discovers it via GEPARD_PYTHON"
echo "   The model downloads on first synth (GEPARD_CHECKPOINT=${GEPARD_CHECKPOINT:-nineninesix/gepard-1.0})."
