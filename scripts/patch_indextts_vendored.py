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

# --- Patch 3: low_vram chunking control (the "podcast" speaker-pop fix) -----
# The constructor auto-enables low-VRAM chunking on <10GB GPUs, which splits
# ANY text >40 chars into separate generation segments — each re-anchored,
# so a single scene line can drift into a different-sounding speaker per
# chunk. Honor INDEXTTS_LOW_VRAM=0 to disable chunking entirely, and make the
# chunk threshold configurable (INDEXTTS_LOW_VRAM_CHARS, default 200 — scene
# lines are typically 90-160 chars, so they stay one shot; only very long
# paragraphs chunk for VRAM safety).
old_lv = "        # Detect low-VRAM GPUs (< 10 GB) to enable automatic text chunking\n        self.low_vram = False\n        if torch.cuda.is_available() and self.device.startswith(\"cuda\"):\n            dev_idx = int(self.device.split(\":\")[-1]) if \":\" in self.device else 0\n            total_vram_gb = torch.cuda.get_device_properties(dev_idx).total_memory / (1024 ** 3)\n            if total_vram_gb < 10.0:\n                self.low_vram = True\n                print(f\">> Low-VRAM mode enabled ({total_vram_gb:.1f} GB < 10 GB), long text will be split into chunks\")\n"
new_lv = ("        # Detect low-VRAM GPUs (< 10 GB) to enable automatic text chunking.\n"
          "        # OpenScript override: INDEXTTS_LOW_VRAM=0 forces chunking OFF\n"
          "        # (the 40-char split re-anchors conditioning per chunk and can\n"
          "        # drift into a different speaker — set 0 once VRAM allows).\n"
          "        self.low_vram = False\n"
          "        _lv_override = os.environ.get(\"INDEXTTS_LOW_VRAM\", \"1\").strip().lower()\n"
          "        if _lv_override not in (\"0\", \"false\", \"no\", \"off\"):\n"
          "            if torch.cuda.is_available() and self.device.startswith(\"cuda\"):\n"
          "                dev_idx = int(self.device.split(\":\")[-1]) if \":\" in self.device else 0\n"
          "                total_vram_gb = torch.cuda.get_device_properties(dev_idx).total_memory / (1024 ** 3)\n"
          "                if total_vram_gb < 10.0:\n"
          "                    self.low_vram = True\n"
          "                    print(f\">> Low-VRAM mode enabled ({total_vram_gb:.1f} GB < 10 GB), long text will be split into chunks\")\n")
if old_lv in src:
    src = src.replace(old_lv, new_lv)
    changed.append("low_vram env override")
else:
    print("WARN patch3a (low_vram override) pattern not found")

old_40 = "            if verbose:\n                print(f\">> Low-VRAM: split into {len(segments)} segments: {segments}\")\n"
# replace the chunk-size literal: split_text_by_punctuation(text, max_chars=40)
old_split = "split_text_by_punctuation(text, max_chars=40)"
new_split = "split_text_by_punctuation(text, max_chars=int(os.environ.get(\"INDEXTTS_LOW_VRAM_CHARS\", \"200\")))"
if old_split in src:
    src = src.replace(old_split, new_split)
    changed.append("low_vram chunk size -> INDEXTTS_LOW_VRAM_CHARS (200)")
else:
    print("WARN patch3b (chunk size) pattern not found")

# Make sure `os` is imported at module top for the env reads above.
if "import os" not in src.split("\n", 20)[0:20] and "\nimport os\n" not in src[:500]:
    m_os = re.search(r"^import (?:re|sys|time)\b", src, re.M)
    if m_os is not None:
        src = src[: m_os.start()] + "import os\n" + src[m_os.start():]
        changed.append("import os")

if "import os" in src[:600]:
    pass
elif m is not None:
    src = src[: m.start()] + "import os\n" + src[m.start():]
    changed.append("import os (torch anchor)")

target.write_text(src)
print("changed:", changed if changed else "none (already applied)")
print("OK")
