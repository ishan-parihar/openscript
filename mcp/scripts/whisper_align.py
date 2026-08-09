#!/usr/bin/env python3
"""
Word Alignment Sidecar — Uses openai-whisper for word-level timestamps.

Takes a transcript (from Nemotron or any ASR) + audio file and produces
word-level timestamps via Whisper's forced alignment.

Input:  stdin JSON  {"wav_path": "...", "text": "...", "language": "hi"}
Output: stdout JSON {"words": [{"word": "...", "start_s": ..., "end_s": ...}]}

Also supports CLI: whisper_align.py --wav <path> --text "..." --output <path.json>
"""

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path

try:
    import whisper
    WHISPER_AVAILABLE = True
except ImportError:
    WHISPER_AVAILABLE = False

try:
    import numpy as np
    NUMPY_AVAILABLE = True
except ImportError:
    NUMPY_AVAILABLE = False


def _log(msg: str):
    print(f"[whisper-align] {msg}", file=sys.stderr, flush=True)


def _resolve_device() -> str:
    """GPU-first device for openai-whisper (OPENSCRIPT_DEVICE=auto|cuda|cpu)."""
    dev = os.environ.get("OPENSCRIPT_DEVICE", "auto").strip().lower()
    if dev == "cpu":
        return "cpu"
    if dev == "cuda":
        return "cuda"
    try:
        import torch

        if torch.cuda.is_available():
            return "cuda"
    except Exception:
        pass
    return "cpu"


# ---------------------------------------------------------------------------
# Audio extraction
# ---------------------------------------------------------------------------

SAMPLE_RATE = 16000


def ensure_wav_16k(media_path: str, out_dir: str) -> str:
    """Convert media to 16kHz mono WAV. Returns path to WAV file."""
    stem = Path(media_path).stem
    wav_path = str(Path(out_dir) / f"{stem}.align.wav")

    cmd = [
        "ffmpeg", "-y", "-i", media_path,
        "-vn", "-acodec", "pcm_s16le",
        "-ar", str(SAMPLE_RATE), "-ac", "1",
        wav_path,
    ]
    result = subprocess.run(cmd, capture_output=True, timeout=300)
    if result.returncode != 0:
        raise RuntimeError(f"ffmpeg failed: {result.stderr.decode()[:200]}")
    return wav_path


# ---------------------------------------------------------------------------
# Whisper alignment
# ---------------------------------------------------------------------------

def align_with_whisper(
    wav_path: str,
    text: str,
    language: str = "hi",
    model_name: str = "base",
) -> dict:
    """Align transcript with audio using Whisper word timestamps.

    Args:
        wav_path: Path to 16kHz mono WAV
        text: Reference transcript for alignment
        language: Language code (hi, en, etc.)
        model_name: Whisper model size (tiny, base, small, medium, large)

    Returns:
        dict with words list and timing info
    """
    if not WHISPER_AVAILABLE:
        return {"error": "openai-whisper not installed", "status": "error"}

    _log(f"Loading Whisper model '{model_name}' for alignment...")
    start = time.time()

    try:
        model = whisper.load_model(model_name, device=_resolve_device())
    except Exception as e:
        return {"error": f"Failed to load Whisper model: {e}", "status": "error"}

    load_time = time.time() - start
    _log(f"Model loaded in {load_time:.1f}s")

    # Transcribe with word timestamps. When a reference transcript is given,
    # seed the decoder with it as initial_prompt — this conditions Whisper
    # toward the KNOWN words so the word-level timing windows stay aligned to
    # them (and word counts match, which remap_words_to_script needs to keep
    # real timings instead of falling back to even spacing).
    _log(f"Running Whisper alignment (language={language})...")
    align_start = time.time()

    transcribe_kwargs = dict(
        language=language if language != "auto" else None,
        word_timestamps=True,
        condition_on_previous_text=False,
        fp16=False,  # CPU mode
    )
    if text and text.strip():
        # Strip ASR markup that Whisper could echo back (timestamps, brackets).
        import re as _re

        prompt = _re.sub(r"\{\{?[^}]*\}?\}", "", text.strip())
        prompt = _re.sub(r"[\[\]()]", " ", prompt)
        transcribe_kwargs["initial_prompt"] = prompt[:500]

    try:
        result = model.transcribe(wav_path, **transcribe_kwargs)
    except Exception as e:
        return {"error": f"Whisper alignment failed: {e}", "status": "error"}

    align_time = time.time() - align_start
    _log(f"Alignment done in {align_time:.1f}s")

    # Extract word-level timestamps
    all_words = []
    for segment in result.get("segments", []):
        for word_info in segment.get("words", []):
            word = word_info.get("word", "").strip()
            if word:
                all_words.append({
                    "word": word,
                    "start_s": word_info.get("start", 0.0),
                    "end_s": word_info.get("end", 0.0),
                    "score": word_info.get("probability", 0.0),
                })

    # Extract segments
    segments = []
    for seg in result.get("segments", []):
        segments.append({
            "text": seg.get("text", "").strip(),
            "start_s": seg.get("start", 0.0),
            "end_s": seg.get("end", 0.0),
        })

    # If Whisper produced no word timestamps, fall back to estimated timings
    if not all_words and text:
        _log("No word timestamps from Whisper, generating estimated timings")
        all_words = _estimate_word_timings(text, segments)

    total_duration = all_words[-1]["end_s"] if all_words else 0.0

    # Cleanup
    del model

    return {
        "words": all_words,
        "segments": segments,
        "word_count": len(all_words),
        "segment_count": len(segments),
        "duration_s": total_duration,
        "align_time_s": align_time,
        "model": model_name,
        "language": result.get("language", language),
    }


def _estimate_word_timings(text: str, segments: list) -> list:
    """Estimate word timings from segment-level timestamps."""
    words = text.split()
    if not words:
        return []

    if segments:
        # Distribute words across segments
        all_words = []
        words_per_seg = max(1, len(words) // max(len(segments), 1))

        word_idx = 0
        for seg in segments:
            seg_words = words[word_idx:word_idx + words_per_seg]
            if not seg_words:
                continue

            seg_duration = seg["end_s"] - seg["start_s"]
            word_duration = seg_duration / max(len(seg_words), 1)

            for i, w in enumerate(seg_words):
                all_words.append({
                    "word": w,
                    "start_s": seg["start_s"] + i * word_duration,
                    "end_s": seg["start_s"] + (i + 1) * word_duration,
                    "score": 0.0,  # Estimated
                })
            word_idx += words_per_seg

        # Handle remaining words
        if word_idx < len(words):
            last_end = segments[-1]["end_s"] if segments else 0.0
            remaining = words[word_idx:]
            word_duration = 0.3  # Default 300ms per word
            for i, w in enumerate(remaining):
                all_words.append({
                    "word": w,
                    "start_s": last_end + i * word_duration,
                    "end_s": last_end + (i + 1) * word_duration,
                    "score": 0.0,
                })

        return all_words
    else:
        # No segments — distribute evenly across estimated duration
        word_duration = 0.3
        return [
            {
                "word": w,
                "start_s": i * word_duration,
                "end_s": (i + 1) * word_duration,
                "score": 0.0,
            }
            for i, w in enumerate(words)
        ]


# ---------------------------------------------------------------------------
# SRT generation
# ---------------------------------------------------------------------------

def fmt_ts(seconds: float) -> str:
    """Format seconds to SRT timestamp."""
    ms = round((seconds % 1) * 1000) % 1000
    s_int = int(seconds)
    return "%02d:%02d:%02d,%03d" % (
        s_int // 3600, (s_int % 3600) // 60, s_int % 60, ms
    )


def generate_word_srt(words: list, output_path: str) -> str:
    """Generate word-level SRT file."""
    with open(output_path, "w", encoding="utf-8") as f:
        for i, w in enumerate(words, 1):
            f.write("%d\n%s --> %s\n%s\n\n" % (
                i, fmt_ts(w["start_s"]), fmt_ts(w["end_s"]), w["word"]
            ))
    return output_path


def generate_phrase_srt(words: list, output_path: str,
                        max_words: int = 12, max_chars: int = 64,
                        max_gap: float = 0.6) -> str:
    """Generate phrase-level SRT by grouping words."""
    groups = []
    cur_words = []
    cur_start = None
    cur_end = None

    for w in words:
        text = w.get("word", "").strip()
        if not text:
            continue
        start = w.get("start_s", 0)
        end = w.get("end_s", 0)

        if cur_start is None:
            cur_start = start
            cur_end = end
            cur_words = [text]
            continue

        gap = start - (cur_end or start)
        combined = " ".join(cur_words)
        next_len = len(combined) + 1 + len(text)

        if gap > max_gap or len(cur_words) >= max_words or next_len > max_chars:
            groups.append((cur_words, cur_start, cur_end))
            cur_start = start
            cur_end = end
            cur_words = [text]
        else:
            cur_words.append(text)
            cur_end = end

    if cur_words:
        groups.append((cur_words, cur_start, cur_end))

    with open(output_path, "w", encoding="utf-8") as f:
        for i, (words_list, start, end) in enumerate(groups, 1):
            f.write("%d\n%s --> %s\n%s\n\n" % (
                i, fmt_ts(start), fmt_ts(end), " ".join(words_list)
            ))
    return output_path


# ---------------------------------------------------------------------------
# CLI entry point
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(
        description="Word Alignment Sidecar (openai-whisper)"
    )
    parser.add_argument("--wav", help="Path to WAV/audio file")
    parser.add_argument("--text", help="Reference transcript text")
    parser.add_argument("--language", default="hi", help="Language code (hi, en, etc.)")
    parser.add_argument("--model", default="base", help="Whisper model size")
    parser.add_argument("--output", help="Output JSON path")
    parser.add_argument("--out-dir", help="Output directory for SRT files")
    parser.add_argument("--serve", action="store_true", help="Stdin/stdout serve mode")
    args = parser.parse_args()

    if args.serve:
        _log("Starting stdin/stdout serve mode")
        print(json.dumps({"ready": True, "whisper_available": WHISPER_AVAILABLE}), flush=True)

        for line in sys.stdin:
            line = line.strip()
            if not line:
                continue
            try:
                req = json.loads(line)
            except json.JSONDecodeError as e:
                print(json.dumps({"error": f"Invalid JSON: {e}"}), flush=True)
                continue

            wav_path = req.get("wav_path", "")
            text = req.get("text", "")
            language = req.get("language", "hi")
            model_name = req.get("model", "base")

            if not wav_path:
                print(json.dumps({"error": "missing wav_path"}), flush=True)
                continue

            try:
                result = align_with_whisper(wav_path, text, language, model_name)
                print(json.dumps(result), flush=True)
            except Exception as e:
                print(json.dumps({"error": str(e)}), flush=True)

    elif args.wav and args.text:
        result = align_with_whisper(args.wav, args.text, args.language, args.model)

        if result.get("error"):
            _log(f"ERROR: {result['error']}")
            sys.exit(1)

        # Generate SRT files if output dir specified
        if args.out_dir:
            out_dir = Path(args.out_dir)
            out_dir.mkdir(parents=True, exist_ok=True)
            stem = Path(args.wav).stem

            word_srt = str(out_dir / f"{stem}.word.srt")
            phrase_srt = str(out_dir / f"{stem}.phrase.srt")

            generate_word_srt(result["words"], word_srt)
            generate_phrase_srt(result["words"], phrase_srt)

            result["word_srt_path"] = word_srt
            result["phrase_srt_path"] = phrase_srt

        # Write JSON output
        if args.output:
            with open(args.output, "w", encoding="utf-8") as f:
                json.dump(result, f, indent=2, ensure_ascii=False)
            _log(f"Output written to {args.output}")
        else:
            print(json.dumps(result, indent=2, ensure_ascii=False))

    else:
        parser.error("--wav and --text are required (or use --serve)")


if __name__ == "__main__":
    main()
