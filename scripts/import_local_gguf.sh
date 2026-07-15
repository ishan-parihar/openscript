#!/usr/bin/env bash
# Import Unsloth Qwen3.5-4B GGUF into Ollama for OpenScript's local LLM cascade.
#
# Source: https://huggingface.co/unsloth/Qwen3.5-4B-GGUF
# Default path: ~/Downloads/Qwen3.5-4B-Q4_K_M.gguf
#
# After import, OpenScript tools use:
#   llm.complete / vision.* → Ollama OpenAI API at OPENSCRIPT_LLM_URL
#   (default http://127.0.0.1:11434/v1) model OPENSCRIPT_LOCAL_MODEL
#   (default qwen3.5-4b)
#
# OpenRouter free multimodal fallbacks (set OPENROUTER_API_KEY):
#   google/gemma-4-31b-it:free
#   nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free
set -euo pipefail

MODEL_NAME="${OPENSCRIPT_LOCAL_MODEL:-qwen3.5-4b}"
GGUF_PATH="${OPENSCRIPT_GGUF_PATH:-$HOME/Downloads/Qwen3.5-4B-Q4_K_M.gguf}"

if ! command -v ollama >/dev/null 2>&1; then
  echo "error: ollama not on PATH. Install from https://ollama.com" >&2
  exit 1
fi

if [ ! -f "$GGUF_PATH" ]; then
  echo "error: GGUF not found at: $GGUF_PATH" >&2
  echo "Download from: https://huggingface.co/unsloth/Qwen3.5-4B-GGUF" >&2
  echo "  e.g. huggingface-cli download unsloth/Qwen3.5-4B-GGUF Qwen3.5-4B-Q4_K_M.gguf --local-dir ~/Downloads" >&2
  exit 1
fi

# If the model already exists, keep it (user may have created it earlier).
if ollama show "$MODEL_NAME" >/dev/null 2>&1; then
  echo "✓ Ollama model already present: $MODEL_NAME"
  ollama show "$MODEL_NAME" 2>/dev/null | head -20 || true
  echo ""
  echo "To force re-import from GGUF:"
  echo "  ollama rm $MODEL_NAME && $0"
  exit 0
fi

TMPDIR_MODEL="$(mktemp -d /tmp/openscript-gguf-XXXXXX)"
cleanup() { rm -rf "$TMPDIR_MODEL"; }
trap cleanup EXIT

# Ollama create requires the GGUF under the Modelfile directory (or absolute FROM).
cp -n "$GGUF_PATH" "$TMPDIR_MODEL/model.gguf" 2>/dev/null || ln -sf "$GGUF_PATH" "$TMPDIR_MODEL/model.gguf"

cat > "$TMPDIR_MODEL/Modelfile" <<EOF
FROM ./model.gguf
PARAMETER temperature 0.2
PARAMETER num_ctx 8192
SYSTEM You are OpenScript's local video director model (Qwen3.5-4B). Be concise and factual. Prefer compact JSON when asked for structured output.
EOF

echo "→ Creating Ollama model '$MODEL_NAME' from $GGUF_PATH ..."
(
  cd "$TMPDIR_MODEL"
  ollama create "$MODEL_NAME" -f Modelfile
)

echo "✓ Imported. Smoke test:"
echo "  ollama run $MODEL_NAME 'Reply with one word: pong'"
echo ""
echo "OpenScript env (optional):"
echo "  export OPENSCRIPT_LLM_URL=http://127.0.0.1:11434/v1"
echo "  export OPENSCRIPT_LOCAL_MODEL=$MODEL_NAME"
echo "  export OPENSCRIPT_GGUF_PATH=$GGUF_PATH"
echo "  export OPENROUTER_API_KEY=sk-or-...   # free multimodal vision fallbacks"
echo ""
echo "Tools: llm.complete | vision.analyze_clip | vision.score_clip | system.capabilities → llm"
