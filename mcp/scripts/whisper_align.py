#!/usr/bin/env python3
"""Force-align word timestamps using Whisper transcription of TTS audio.

Given a WAV file, transcribes it with Whisper and returns word-level timestamps
as JSON. This gives frame-accurate caption sync — much better than even-spacing
estimation.

Usage:
    python3 whisper_align.py --wav /tmp/voiceover.wav --output /tmp/alignment.json

Output JSON format:
    {
        "words": [
            {"word": "Hello", "start_ms": 0, "end_ms": 320},
            {"word": "world", "start_ms": 320, "end_ms": 680},
            ...
        ],
        "duration_ms": 1750
    }
"""

import argparse
import json
import sys
import whisper


def main():
    parser = argparse.ArgumentParser(description="Whisper force alignment for TTS audio")
    parser.add_argument("--wav", required=True, help="Path to WAV file")
    parser.add_argument("--output", required=True, help="Output JSON path")
    parser.add_argument("--model", default="base", help="Whisper model (tiny/base/small)")
    args = parser.parse_args()

    try:
        model = whisper.load_model(args.model)
    except Exception as e:
        print(f"ERROR: Failed to load whisper model: {e}", file=sys.stderr)
        sys.exit(1)

    try:
        result = model.transcribe(args.wav, word_timestamps=True, language="en")
    except Exception as e:
        print(f"ERROR: Transcription failed: {e}", file=sys.stderr)
        sys.exit(1)

    words = []
    for segment in result.get("segments", []):
        for word_info in segment.get("words", []):
            word = word_info.get("word", "").strip()
            start = word_info.get("start", 0)
            end = word_info.get("end", 0)
            if word:
                words.append({
                    "word": word,
                    "start_ms": int(start * 1000),
                    "end_ms": int(end * 1000),
                })

    duration_ms = int(result.get("segments", [{}])[-1].get("end", 0) * 1000) if result.get("segments") else 0

    output = {
        "words": words,
        "duration_ms": duration_ms,
    }

    with open(args.output, "w") as f:
        json.dump(output, f, indent=2)

    print(f"OK: {len(words)} words aligned, duration={duration_ms}ms")


if __name__ == "__main__":
    main()
