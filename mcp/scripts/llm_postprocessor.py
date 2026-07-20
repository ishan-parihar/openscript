#!/usr/bin/env python3
"""
LLM Post-Processor — Devanagari Hindi → Natural Hinglish.

Converts Nemotron's Devanagari output to natural Hinglish (Romanized Hindi
with English code-switching) using an LLM API.

Input:  stdin JSON  {"text": "...", "source_lang": "hi-IN"}
Output: stdout JSON {"text": "...", "script": "hinglish"}

Also supports CLI: llm_postprocessor.py --input <file> --output <file>

Architecture:
  Nemotron outputs Devanagari for Hindi → this script converts to Hinglish
  via API call to mimo-v2.5-free (or configured LLM backend).
"""

import argparse
import json
import os
import re
import sys
import time
from pathlib import Path

try:
    import requests
except ImportError:
    requests = None

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

# LLM API configuration (from env vars or defaults)
LLM_API_KEY = os.environ.get(
    "OPENCODE_API",
    os.environ.get("OPENROUTER_API_KEY", ""),
)
LLM_BASE_URL = os.environ.get(
    "LLM_BASE_URL",
    os.environ.get("OPENCODE_BASE_URL", "https://opencode.ai/zen/v1"),
)
LLM_MODEL = os.environ.get(
    "LLM_MODEL",
    "mimo-v2.5-free",
)

# Timeout for API calls
API_TIMEOUT_S = 30

# Script detection thresholds
DEVANAGARI_RANGE = (0x0900, 0x097F)
LATIN_THRESHOLD = 0.5  # If >50% Latin chars, skip conversion


def _log(msg: str):
    print(f"[llm-post] {msg}", file=sys.stderr, flush=True)


# ---------------------------------------------------------------------------
# Script detection
# ---------------------------------------------------------------------------

def detect_script(text: str) -> str:
    """Detect the dominant script of the text."""
    chars = list(text)
    total_alpha = sum(1 for c in chars if c.isalpha())

    if total_alpha == 0:
        return "empty"

    latin = sum(1 for c in chars if c.isascii() and c.isalpha())
    devanagari = sum(
        1 for c in chars
        if DEVANAGARI_RANGE[0] <= ord(c) <= DEVANAGARI_RANGE[1]
    )

    latin_pct = latin / total_alpha
    deva_pct = devanagari / total_alpha

    if latin_pct > LATIN_THRESHOLD:
        return "latin"
    elif deva_pct > 0.3:
        return "devanagari"
    else:
        return "other"


# ---------------------------------------------------------------------------
# LLM API call
# ---------------------------------------------------------------------------

HINGLISH_PROMPT = """Convert the following Devanagari Hindi transcript to natural Hinglish (Romanized Hindi with English code-switching).

Rules:
1. Transliterate Devanagari to Latin script (e.g., "मैं" → "main", "है" → "hai")
2. Preserve English words as-is (e.g., "मैं engineer हूँ" → "main engineer hoon")
3. Use natural Hinglish conventions:
   - "और" → "aur" (not "and")
   - "लेकिन" → "lekin"
   - "क्योंकि" → "kyunki"
   - "तो" → "toh"
   - "भी" → "bhi"
   - "ही" → "hi"
   - "नहीं" → "nahi" or "nahin"
   - "कर" → "kar"
   - "हो" → "ho"
   - "जा" → "ja"
   - "रहा" → "raha"
   - "रही" → "rahi"
4. Add proper punctuation and capitalization
5. Keep sentence boundaries from the input
6. Output ONLY the converted text, no explanations

Input (Devanagari):
{text}

Output (Hinglish):"""


def call_llm_api(text: str) -> str:
    """Call LLM API to convert Devanagari to Hinglish."""
    if not requests:
        _log("requests library not available, using rule-based fallback")
        return rule_based_transliterate(text)

    if not LLM_API_KEY:
        _log("No LLM API key configured, using rule-based fallback")
        return rule_based_transliterate(text)

    prompt = HINGLISH_PROMPT.format(text=text)

    headers = {
        "Authorization": f"Bearer {LLM_API_KEY}",
        "Content-Type": "application/json",
    }

    payload = {
        "model": LLM_MODEL,
        "messages": [
            {"role": "user", "content": prompt},
        ],
        "max_tokens": 4096,
        "temperature": 0.1,
    }

    try:
        url = f"{LLM_BASE_URL}/chat/completions"
        _log(f"Calling LLM API: {url} (model={LLM_MODEL})")
        start = time.time()

        resp = requests.post(url, json=payload, headers=headers, timeout=API_TIMEOUT_S)
        elapsed = time.time() - start
        _log(f"LLM API response in {elapsed:.1f}s (status={resp.status_code})")

        if resp.status_code != 200:
            _log(f"LLM API error: {resp.status_code} {resp.text[:200]}")
            return rule_based_transliterate(text)

        data = resp.json()
        result_text = data["choices"][0]["message"]["content"].strip()
        _log(f"LLM output: {result_text[:100]}...")
        return result_text

    except Exception as e:
        _log(f"LLM API call failed: {e}")
        return rule_based_transliterate(text)


# ---------------------------------------------------------------------------
# Rule-based fallback transliteration
# ---------------------------------------------------------------------------

# Devanagari → Latin mapping (common Hindi words)
DEVANAGARI_MAP = {
    "अ": "a", "आ": "aa", "इ": "i", "ई": "ee", "उ": "u", "ऊ": "oo",
    "ए": "e", "ऐ": "ai", "ओ": "o", "औ": "au",
    "ं": "n", "ः": "h", "ँ": "n", "ृ": "r",
    "क": "k", "ख": "kh", "ग": "g", "घ": "gh", "ङ": "ng",
    "च": "ch", "छ": "chh", "ज": "j", "झ": "jh", "ञ": "ny",
    "ट": "t", "ठ": "th", "ड": "d", "ढ": "dh", "ण": "n",
    "त": "t", "थ": "th", "द": "d", "ध": "dh", "न": "n",
    "प": "p", "फ": "ph", "ब": "b", "भ": "bh", "म": "m",
    "य": "y", "र": "r", "ल": "l", "व": "v", "श": "sh",
    "ष": "sh", "स": "s", "ह": "h",
    "ा": "aa", "ि": "i", "ी": "ee", "ु": "u", "ू": "oo",
    "े": "e", "ै": "ai", "ो": "o", "ौ": "au",
    "्": "",  # virama (halant) — removes inherent vowel
    "ँ": "n", "ं": "n", "ः": "h",
    "।": ".", "॥": ".",
    # Digits
    "०": "0", "१": "1", "२": "2", "३": "3", "४": "4",
    "५": "5", "६": "6", "७": "7", "८": "8", "९": "9",
}


def rule_based_transliterate(text: str) -> str:
    """Simple rule-based Devanagari → Latin transliteration.

    This is a fallback when the LLM API is unavailable.
    Not as good as LLM conversion but functional.
    """
    result = []
    for char in text:
        if DEVANAGARI_RANGE[0] <= ord(char) <= DEVANAGARI_RANGE[1]:
            result.append(DEVANAGARI_MAP.get(char, char))
        elif char.isascii():
            result.append(char)
        else:
            result.append(char)

    # Clean up double spaces
    output = "".join(result)
    output = re.sub(r"\s+", " ", output).strip()

    # Capitalize first letter of sentences
    output = re.sub(r"(^|[.!?]\s+)(\w)", lambda m: m.group(1) + m.group(2).upper(), output)

    return output


# ---------------------------------------------------------------------------
# Main pipeline
# ---------------------------------------------------------------------------

def postprocess(text: str, source_lang: str = "hi-IN") -> dict:
    """Post-process transcription text.

    Args:
        text: Input text (Devanagari for Hindi, Latin for other langs)
        source_lang: Source language code

    Returns:
        dict with converted text and script info
    """
    script = detect_script(text)

    if script == "empty":
        return {"text": "", "script": "empty", "skipped": True}

    # Skip conversion for non-Devanagari text
    if script != "devanagari":
        _log(f"Text is {script} script, skipping conversion")
        return {"text": text, "script": script, "skipped": True}

    _log(f"Converting Devanagari → Hinglish ({len(text)} chars)")
    start = time.time()

    converted = call_llm_api(text)

    elapsed = time.time() - start
    _log(f"Conversion done in {elapsed:.1f}s")

    return {
        "text": converted,
        "script": "hinglish",
        "source_script": "devanagari",
        "source_lang": source_lang,
        "skipped": False,
    }


# ---------------------------------------------------------------------------
# CLI entry point
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(
        description="LLM Post-Processor: Devanagari → Hinglish"
    )
    sub = parser.add_subparsers(dest="cmd", required=True)

    # One-shot mode
    p_convert = sub.add_parser("convert", help="Convert text")
    p_convert.add_argument("--text", required=True, help="Text to convert")
    p_convert.add_argument("--lang", default="hi-IN", help="Source language")

    # File mode
    p_file = sub.add_parser("file", help="Convert SRT file")
    p_file.add_argument("--input", required=True, help="Input SRT file")
    p_file.add_argument("--output", required=True, help="Output SRT file")

    # Stdin/stdout serve mode
    p_serve = sub.add_parser("serve", help="Long-lived stdin/stdout JSON mode")

    args = parser.parse_args()

    if args.cmd == "convert":
        result = postprocess(args.text, args.lang)
        print(json.dumps(result))

    elif args.cmd == "file":
        # Read SRT, convert Devanagari entries, write output
        with open(args.input, "r", encoding="utf-8") as f:
            content = f.read()

        # Parse SRT entries
        entries = []
        blocks = content.strip().split("\n\n")
        for block in blocks:
            lines = block.strip().split("\n")
            if len(lines) >= 3:
                idx = lines[0].strip()
                timestamp = lines[1].strip()
                text = "\n".join(lines[2:])
                entries.append({"idx": idx, "timestamp": timestamp, "text": text})

        # Convert each entry
        converted_entries = []
        for entry in entries:
            result = postprocess(entry["text"])
            converted_entries.append({
                "idx": entry["idx"],
                "timestamp": entry["timestamp"],
                "text": result["text"],
            })

        # Write output SRT
        with open(args.output, "w", encoding="utf-8") as f:
            for entry in converted_entries:
                f.write(f"{entry['idx']}\n{entry['timestamp']}\n{entry['text']}\n\n")

        print(json.dumps({
            "status": "converted",
            "entries": len(converted_entries),
            "output_path": args.output,
        }))

    elif args.cmd == "serve":
        _log("Starting stdin/stdout serve mode")
        print(json.dumps({"ready": True}), flush=True)

        for line in sys.stdin:
            line = line.strip()
            if not line:
                continue
            try:
                req = json.loads(line)
            except json.JSONDecodeError as e:
                print(json.dumps({"error": f"Invalid JSON: {e}"}), flush=True)
                continue

            text = req.get("text", "")
            source_lang = req.get("source_lang", "hi-IN")

            if not text:
                print(json.dumps({"error": "missing text"}), flush=True)
                continue

            try:
                result = postprocess(text, source_lang)
                print(json.dumps(result), flush=True)
            except Exception as e:
                print(json.dumps({"error": str(e)}), flush=True)


if __name__ == "__main__":
    main()
