#!/usr/bin/env bash
# Create / update ~/.openscript/config.json for OpenScript.
#
# Usage:
#   bash scripts/setup_openscript_config.sh
#   OPENROUTER_API_KEY=sk-or-... bash scripts/setup_openscript_config.sh
#   bash scripts/setup_openscript_config.sh --openrouter-key sk-or-...
#
# Never commits secrets. File mode is 0600.
set -euo pipefail

CONFIG_DIR="${OPENSCRIPT_CONFIG_DIR:-$HOME/.openscript}"
CONFIG_FILE="$CONFIG_DIR/config.json"
GGUF_PATH="${OPENSCRIPT_GGUF_PATH:-$HOME/Downloads/Qwen3.5-4B-Q4_K_M.gguf}"
LOCAL_MODEL="${OPENSCRIPT_LOCAL_MODEL:-qwen3.5-4b}"
OR_KEY="${OPENROUTER_API_KEY:-${OPENROUTER_KEY:-}}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --openrouter-key) OR_KEY="${2:-}"; shift 2 ;;
    --gguf) GGUF_PATH="${2:-}"; shift 2 ;;
    --model) LOCAL_MODEL="${2:-}"; shift 2 ;;
    -h|--help) sed -n '2,12p' "$0"; exit 0 ;;
    *) echo "Unknown arg: $1" >&2; exit 1 ;;
  esac
done

mkdir -p "$CONFIG_DIR"
chmod 700 "$CONFIG_DIR" 2>/dev/null || true

python3 - "$CONFIG_FILE" "$OR_KEY" "$LOCAL_MODEL" "$GGUF_PATH" <<'PY'
import json, os, sys
from pathlib import Path

path = Path(sys.argv[1])
or_key_arg = sys.argv[2]
local_model = sys.argv[3]
gguf_path = sys.argv[4]

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
    "pexels": pick(keys.get("pexels"), existing.get("pexels_api_key"), os.environ.get("PEXELS_API_KEY", "")),
    "giphy": pick(keys.get("giphy"), existing.get("giphy_api_key"), os.environ.get("GIPHY_API_KEY", "")),
    "pixabay": pick(keys.get("pixabay"), existing.get("pixabay_api_key"), os.environ.get("PIXABAY_API_KEY", "")),
    "openrouter": pick(
        or_key_arg,
        keys.get("openrouter"),
        existing.get("openrouter_api_key"),
        os.environ.get("OPENROUTER_API_KEY", ""),
        os.environ.get("OPENROUTER_KEY", ""),
    ),
}

llm_existing = existing.get("llm") or {}
if not isinstance(llm_existing, dict):
    llm_existing = {}

gguf = gguf_path if gguf_path and Path(gguf_path).expanduser().exists() else llm_existing.get("gguf_path")
if isinstance(gguf, str):
    gguf = str(Path(gguf).expanduser()) if gguf else None
    if gguf and not Path(gguf).exists():
        gguf = None

cfg = {
    "version": 1,
    "api_keys": api_keys,
    "llm": {
        "local_model": local_model or llm_existing.get("local_model") or "qwen3.5-4b",
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
    "paths": existing.get("paths") or {},
    "render": existing.get("render")
    or {"default_aspect": "9:16", "normalize_lufs": -16.0},
}

path.write_text(json.dumps(cfg, indent=2) + "\n")
os.chmod(path, 0o600)
print(f"Wrote {path} (mode 0600)")
print(f"  local_model: {cfg['llm']['local_model']}")
print(f"  gguf_path:   {cfg['llm']['gguf_path']}")
print(f"  openrouter:  {'set' if cfg['api_keys']['openrouter'] else 'NOT set'}")
print(f"  models:      {cfg['llm']['openrouter_models']}")
PY

echo ""
echo "Next:"
echo "  1. ollama serve   # if not running"
echo "  2. bash scripts/import_local_gguf.sh"
echo "  3. MCP: system.config.get / system.capabilities"
