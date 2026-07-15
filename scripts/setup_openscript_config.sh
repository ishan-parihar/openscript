#!/usr/bin/env bash
# Create / update ~/.openscript/config.json for OpenScript.
#
# Usage:
#   bash scripts/setup_openscript_config.sh
#   PEXELS_API_KEY=... GIPHY_API_KEY=... bash scripts/setup_openscript_config.sh
#   bash scripts/setup_openscript_config.sh --pexels-key KEY --giphy-key KEY
#   bash scripts/setup_openscript_config.sh --openrouter-key sk-or-...
#
# Merges into existing config (does not wipe other keys).
# Never commits secrets. File mode is 0600.
set -euo pipefail

CONFIG_DIR="${OPENSCRIPT_CONFIG_DIR:-$HOME/.openscript}"
CONFIG_FILE="$CONFIG_DIR/config.json"
GGUF_PATH="${OPENSCRIPT_GGUF_PATH:-$HOME/Downloads/Qwen3.5-4B-Q4_K_M.gguf}"
LOCAL_MODEL="${OPENSCRIPT_LOCAL_MODEL:-qwen3.5-4b}"
OR_KEY="${OPENROUTER_API_KEY:-${OPENROUTER_KEY:-}}"
PEXELS_KEY="${PEXELS_API_KEY:-}"
GIPHY_KEY="${GIPHY_API_KEY:-}"
PIXABAY_KEY="${PIXABAY_API_KEY:-}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --openrouter-key) OR_KEY="${2:-}"; shift 2 ;;
    --pexels-key) PEXELS_KEY="${2:-}"; shift 2 ;;
    --giphy-key) GIPHY_KEY="${2:-}"; shift 2 ;;
    --pixabay-key) PIXABAY_KEY="${2:-}"; shift 2 ;;
    --gguf) GGUF_PATH="${2:-}"; shift 2 ;;
    --model) LOCAL_MODEL="${2:-}"; shift 2 ;;
    -h|--help)
      cat <<'EOF'
Create/update ~/.openscript/config.json (mode 0600).

  --pexels-key KEY       Pexels API key (primary multi-broll)
  --giphy-key KEY        GIPHY key (stickers / memes)
  --pixabay-key KEY      Pixabay key (optional music/video)
  --openrouter-key KEY   OpenRouter key (vision cascade)
  --gguf PATH            Local GGUF path
  --model NAME           Ollama model name (default qwen3.5-4b)

Env overrides: PEXELS_API_KEY, GIPHY_API_KEY, PIXABAY_API_KEY, OPENROUTER_API_KEY
EOF
      exit 0
      ;;
    *) echo "Unknown arg: $1" >&2; exit 1 ;;
  esac
done

mkdir -p "$CONFIG_DIR"
chmod 700 "$CONFIG_DIR" 2>/dev/null || true

export CONFIG_FILE LOCAL_MODEL GGUF_PATH OR_KEY PEXELS_KEY GIPHY_KEY PIXABAY_KEY
python3 <<'PY'
import json, os
from pathlib import Path

path = Path(os.environ["CONFIG_FILE"])
existing = {}
if path.exists():
    try:
        existing = json.loads(path.read_text())
    except Exception:
        existing = {}

keys = existing.get("api_keys") or {}
if not isinstance(keys, dict):
    keys = {}

def pick(*vals):
    for v in vals:
        if isinstance(v, str) and v.strip():
            return v.strip()
    return ""

api_keys = {
    "pexels": pick(os.environ.get("PEXELS_KEY"), keys.get("pexels"), existing.get("pexels_api_key"), os.environ.get("PEXELS_API_KEY", "")),
    "giphy": pick(os.environ.get("GIPHY_KEY"), keys.get("giphy"), existing.get("giphy_api_key"), os.environ.get("GIPHY_API_KEY", "")),
    "pixabay": pick(os.environ.get("PIXABAY_KEY"), keys.get("pixabay"), existing.get("pixabay_api_key"), os.environ.get("PIXABAY_API_KEY", "")),
    "openrouter": pick(
        os.environ.get("OR_KEY"),
        keys.get("openrouter"),
        existing.get("openrouter_api_key"),
        os.environ.get("OPENROUTER_API_KEY", ""),
        os.environ.get("OPENROUTER_KEY", ""),
    ),
}

llm_existing = existing.get("llm") if isinstance(existing.get("llm"), dict) else {}
gguf_path = os.environ.get("GGUF_PATH", "")
gguf = None
if gguf_path and Path(gguf_path).expanduser().exists():
    gguf = str(Path(gguf_path).expanduser())
elif llm_existing.get("gguf_path"):
    g = str(Path(str(llm_existing["gguf_path"])).expanduser())
    gguf = g if Path(g).exists() else None

local_model = os.environ.get("LOCAL_MODEL") or llm_existing.get("local_model") or "qwen3.5-4b"

cfg = {
    "version": 1,
    "api_keys": api_keys,
    "llm": {
        "local_model": local_model,
        "local_base_url": llm_existing.get("local_base_url")
            or os.environ.get("OPENSCRIPT_LLM_URL", "http://127.0.0.1:11434/v1"),
        "gguf_path": gguf,
        "mmproj_path": llm_existing.get("mmproj_path"),
        "local_vision": bool(llm_existing.get("local_vision", False)),
        "openrouter_base_url": llm_existing.get("openrouter_base_url")
            or "https://openrouter.ai/api/v1",
        "openrouter_models": llm_existing.get("openrouter_models")
            or [
                "google/gemma-4-31b-it:free",
                "nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free",
            ],
        "prefer_openrouter_vision": llm_existing.get("prefer_openrouter_vision", True),
    },
    "paths": existing.get("paths") if isinstance(existing.get("paths"), dict) else {},
    "render": existing.get("render")
    if isinstance(existing.get("render"), dict)
    else {"default_aspect": "9:16", "normalize_lufs": -16.0},
}

path.write_text(json.dumps(cfg, indent=2) + "\n")
os.chmod(path, 0o600)
print(f"Wrote {path} (mode 0600)")
for k, v in api_keys.items():
    print(f"  {k}: {'set' if v else 'NOT set'}")
print(f"  local_model: {cfg['llm']['local_model']}")
print(f"  gguf_path:   {cfg['llm']['gguf_path']}")
PY

echo ""
echo "Next: bash scripts/bootstrap_media.sh  # doctor + optional library.build"
echo "      system.capabilities  # pexels/giphy should be available"
