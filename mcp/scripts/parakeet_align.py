#!/usr/bin/env python3
"""Parakeet TDT STT sidecar — replaces Whisper for word-level timestamp alignment.

Uses nvidia/parakeet-tdt-0.6b-v3 via onnx-asr (pure Python, no PyTorch).
Parakeet's TDT architecture natively emits per-token durations, giving
word-level timestamps without external forced alignment.

Usage:
    python3 parakeet_align.py --wav /tmp/voiceover.wav --output /tmp/alignment.json

Output JSON:
    {
        "words": [
            {"word": "Hello", "start_ms": 0, "end_ms": 320},
            ...
        ],
        "duration_ms": 1750,
        "engine": "parakeet-tdt-0.6b-v3"
    }
"""

import argparse
import json
import sys
import onnx_asr


def main():
    parser = argparse.ArgumentParser(description="Parakeet TDT STT alignment")
    parser.add_argument("--wav", required=True, help="Path to WAV file")
    parser.add_argument("--output", required=True, help="Output JSON path")
    args = parser.parse_args()

    try:
        model = onnx_asr.load_model("nemo-parakeet-tdt-0.6b-v3").with_timestamps()
    except Exception as e:
        print(f"ERROR: Failed to load Parakeet model: {e}", file=sys.stderr)
        sys.exit(1)

    try:
        result = model.recognize(args.wav)
    except Exception as e:
        print(f"ERROR: Recognition failed: {e}", file=sys.stderr)
        sys.exit(1)

    # Aggregate tokens into words with timestamps
    tokens = result.tokens
    timestamps = result.timestamps

    words = []
    current_word = ""
    word_start = None

    for i, token in enumerate(tokens):
        token_start = timestamps[i] if i < len(timestamps) else 0
        token_end = timestamps[i + 1] if i + 1 < len(timestamps) else token_start + 0.1

        # Parakeet uses subword tokens — accumulate until we have a full word
        # A new word starts when the token begins with a space or is a word boundary
        if token.startswith("▁") or token.startswith(" "):
            # Save previous word
            if current_word and word_start is not None:
                words.append({
                    "word": current_word.strip(),
                    "start_ms": int(word_start * 1000),
                    "end_ms": int(token_start * 1000),
                })
            current_word = token.lstrip("▁ ").lstrip()
            word_start = token_start
        else:
            current_word += token

    # Don't forget the last word
    if current_word and word_start is not None:
        last_end = timestamps[-1] + 0.1 if timestamps else 0
        words.append({
            "word": current_word.strip(),
            "start_ms": int(word_start * 1000),
            "end_ms": int(last_end * 1000),
        })

    # Get duration from last timestamp
    duration_ms = int(timestamps[-1] * 1000) if timestamps else 0

    output = {
        "words": words,
        "duration_ms": duration_ms,
        "engine": "parakeet-tdt-0.6b-v3",
    }

    with open(args.output, "w") as f:
        json.dump(output, f, indent=2)

    print(f"OK: {len(words)} words aligned, duration={duration_ms}ms")


if __name__ == "__main__":
    main()
