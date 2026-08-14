#!/usr/bin/env python3
"""Patch vendored index-tts infer_v2_5.py for 8GB-GPU compatibility (idempotent).

Applied by scripts/setup_indextts.sh on fresh clones AND locally now.

1. QwenEmotion (emo_text classifier, float16 Qwen 0.6B ~1.5GB) -> CPU.
   device_map="auto" places it on the GPU, which OOMs an 8GB card once the
   main pipeline (~2.5GB bf16) is loaded. It is a tiny per-line text
   classification, so CPU is fine (46GB RAM machine).
2. Guard PYTORCH_CUDA_ALLOC_CONF=expandable_segments:True so torch frees
   fragmented blocks instead of OOM'ing on the 2060.
"""
import re
from pathlib import Path

# Allow running from repo root or scripts/
base = Path(__file__).resolve().parent.parent
target = base / "third_party/index-tts/indextts/infer_v2_5.py"
assert target.exists(), f"not found: {target}"
src = target.read_text()
changed = []

# --- Patch 1: QwenEmotion -> CPU ------------------------------------------
old1 = '            torch_dtype="float16",  # "auto"\n            device_map="auto"\n        )'
new1 = '            torch_dtype="float16",  # "auto"\n            device_map="cpu",  # OpenScript: keep the 8GB GPU for the audio pipeline\n        )'
if "device_map=\"cpu\"" in src and "OpenScript: keep the 8GB GPU" in src:
    print("patch1 already applied (QwenEmotion CPU)")
elif old1 in src:
    src = src.replace(old1, new1)
    changed.append("QwenEmotion -> CPU")
else:
    print(f"WARN patch1 pattern not found; device_map lines:\n" +
          "\n".join(l for l in src.splitlines() if "device_map" in l))

# --- Patch 2: expandable_segments guard at torch import --------------------
m = re.search(r"^import torch\b", src, re.M)
if m is not None and "expandable_segments" not in src:
    guard = (
        "# OpenScript: enable expandable segments so the 8GB RTX 2060 avoids\n"
        "# fragmentation OOM during GPT decoding + vocoder (see sidecar).\n"
        "import os as _os\n"
        "_os.environ.setdefault(\"PYTORCH_CUDA_ALLOC_CONF\", \"expandable_segments:True\")\n"
    )
    src = src[: m.start()] + guard + src[m.start():]
    changed.append("expandable_segments guard")
elif "expandable_segments" in src:
    print("patch2 already applied (expandable_segments)")

target.write_text(src)
print("changed:", changed if changed else "none (already applied)")
print("OK")
