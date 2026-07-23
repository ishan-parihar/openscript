#!/usr/bin/env python3
"""
Transcription Sidecar — Whisper (primary).

Primary engine: openai-whisper (reliable, word-level timestamps, 99 languages)

Input:  stdin JSON  {"wav_path": "...", "language_hint": "hi-IN"|"auto"}
Output: stdout JSON {"text": "...", "word_srt_path": "...", "phrase_srt_path": "...",
                     "segments": [...], "duration_s": float, "engine": "whisper"}

Also supports one-shot CLI: nemotron_transcriber.py run --video <path> --out-dir <dir>
"""

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path

# Import shared utilities from transcribe_common
from transcribe_common import (
    SAMPLE_RATE, PHRASE_MAX_WORDS, PHRASE_MAX_CHARS, PHRASE_MAX_GAP_S,
    SCRIPT_DIR, REPO_ROOT,
    _log, extract_audio, ensure_wav_16k,
    build_srt_files,
)



# Whisper model sizes: tiny < base < small < medium < large-v3
# tiny: fastest, ~39x realtime on CPU, lowest accuracy
# base: good balance, ~10x realtime on CPU
WHISPER_DEFAULT_MODEL = "base"


def _log_whisper(msg: str):
    _log(msg, prefix="transcriber")


# ---------------------------------------------------------------------------
# Audio extraction (from video)
# ---------------------------------------------------------------------------

def transcribe_whisper(
    wav_path: str,
    language_hint: str = "auto",
    model_name: str = WHISPER_DEFAULT_MODEL,
) -> dict:
    """Transcribe audio using openai-whisper with word-level timestamps.

    Args:
        wav_path: Path to 16kHz mono WAV
        language_hint: "auto", "hi", "en", etc.
        model_name: Whisper model size (tiny, base, small, medium, large-v3)

    Returns:
        dict with text, words, segments, timing info
    """
    try:
        import whisper
    except ImportError:
        return {"error": "openai-whisper not installed. Run: pip install openai-whisper"}

    _log(f"Loading Whisper model '{model_name}'...")
    start = time.time()

    try:
        model = whisper.load_model(model_name)
    except Exception as e:
        return {"error": f"Failed to load Whisper model: {e}"}

    load_time = time.time() - start
    _log(f"Model loaded in {load_time:.1f}s")

    # Determine language
    lang = None if language_hint == "auto" else language_hint[:2]
    _log(f"Running Whisper transcription (language={language_hint})...")

    transcribe_start = time.time()
    try:
        result = model.transcribe(
            wav_path,
            language=lang,
            word_timestamps=True,
            condition_on_previous_text=False,
            fp16=False,  # CPU mode
        )
    except Exception as e:
        return {"error": f"Whisper transcription failed: {e}"}

    transcribe_time = time.time() - transcribe_start
    _log(f"Transcription done in {transcribe_time:.1f}s")

    # Get audio duration
    duration_s = 0.0
    for seg in result.get("segments", []):
        if seg.get("end", 0) > duration_s:
            duration_s = seg["end"]

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

    detected_language = result.get("language", language_hint)

    _log(f"Text: {result['text'][:100]}...")
    _log(f"Words: {len(all_words)}, Segments: {len(segments)}")
    _log(f"Detected language: {detected_language}")

    # Cleanup
    del model

    return {
        "text": result["text"].strip(),
        "words": all_words,
        "segments": segments,
        "word_count": len(all_words),
        "segment_count": len(segments),
        "duration_s": duration_s,
        "load_time_s": load_time,
        "transcribe_time_s": transcribe_time,
        "language": detected_language,
        "model": model_name,
    }





# ---------------------------------------------------------------------------
# Main pipeline
# ---------------------------------------------------------------------------

def run_transcription(
    media_path: str,
    out_dir: str,
    language_hint: str = "auto",
    engine: str = "whisper",
    model_name: str = WHISPER_DEFAULT_MODEL,
) -> dict:
    """Full transcription pipeline: media → text → SRT.

    Args:
        media_path: Path to video or audio file
        out_dir: Output directory for SRT files
        language_hint: "auto", "hi-IN", "en-US", etc.
        engine: "whisper" (primary) or "nemotron-onnx" (experimental)
        model_name: Whisper model size (tiny, base, small, medium, large-v3)

    Returns:
        dict with status, text, SRT paths, segments, etc.
    """
    Path(out_dir).mkdir(parents=True, exist_ok=True)
    stem = Path(media_path).stem

    # Step 1: Convert to 16kHz mono WAV
    _log(f"Preparing audio from {media_path}...")
    wav_path = ensure_wav_16k(media_path, out_dir)
    _log(f"Audio ready: {wav_path}")

    # Step 2: Transcribe with Whisper
    _log(f"Using Whisper engine (model={model_name})")
    result = transcribe_whisper(wav_path, language_hint, model_name)

    if result.get("error"):
        # Cleanup
        try:
            os.remove(wav_path)
        except OSError:
            pass
        return result

    text = result["text"]
    words = result.get("words", [])
    duration_s = result.get("duration_s", 0.0)

    if not text or not words:
        # Cleanup
        try:
            os.remove(wav_path)
        except OSError:
            pass
        return {
            "error": "No text produced from transcription",
            "status": "error",
            "engine": engine,
        }

    # Step 3: Generate SRT files via shared utility
    srt = build_srt_files(words, out_dir, stem)
    word_srt_path = srt["word_srt_path"]
    phrase_srt_path = srt["phrase_srt_path"]
    output_srt_path = srt["output_srt_path"]
    phrases = srt["phrases"]

    # Step 4: Build result
    result["status"] = "transcribed"
    result["engine"] = engine
    result["language"] = result.get("language", language_hint)
    result["word_srt_path"] = word_srt_path
    result["phrase_srt_path"] = phrase_srt_path
    result["output_srt_path"] = output_srt_path
    result["segments"] = [
        {"text": p["text"], "start_s": p["start_s"], "end_s": p["end_s"]}
        for p in phrases
    ]

    _log(f"Pipeline complete: {len(words)} words, {len(phrases)} phrases")
    _log(f"Output: {output_srt_path}")

    # Cleanup temp WAV
    try:
        os.remove(wav_path)
    except OSError:
        pass

    return result


# ---------------------------------------------------------------------------
# CLI entry point
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(
        description="Transcription Sidecar — Whisper (primary) + Nemotron ONNX (experimental)"
    )
    sub = parser.add_subparsers(dest="cmd", required=True)

    # One-shot mode
    p_run = sub.add_parser("run", help="Transcribe a media file")
    p_run.add_argument("--video", required=True, help="Path to video/audio file")
    p_run.add_argument("--out-dir", default=None, help="Output directory")
    p_run.add_argument("--language", default="auto", help="Language hint (auto, hi-IN, en-US)")
    p_run.add_argument("--engine", default="whisper", choices=["whisper"],
                        help="Transcription engine (whisper only)")
    p_run.add_argument("--model", default=WHISPER_DEFAULT_MODEL,
                        help=f"Whisper model size (default: {WHISPER_DEFAULT_MODEL})")
    p_run.add_argument("--threads", type=int, default=4, help="Number of threads")

    # Stdin/stdout serve mode (for Rust sidecar)
    p_serve = sub.add_parser("serve", help="Long-lived stdin/stdout JSON mode")

    args = parser.parse_args()

    if args.cmd == "run":
        out_dir = args.out_dir or str(Path(args.video).parent)
        if Path(out_dir).resolve() == Path("/"):
            out_dir = "."
        result = run_transcription(
            args.video, out_dir, args.language, args.engine, args.model
        )
        if result.get("error"):
            _log(f"ERROR: {result['error']}")
            print(json.dumps(result))
            sys.exit(1)
        print(json.dumps(result))

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

            wav_path = req.get("wav_path", "")
            out_dir = req.get("out_dir", str(Path(wav_path).parent) if wav_path else ".")
            language_hint = req.get("language_hint", "auto")
            engine = req.get("engine", "whisper")
            model_name = req.get("model", WHISPER_DEFAULT_MODEL)

            if not wav_path:
                print(json.dumps({"error": "missing wav_path"}), flush=True)
                continue

            try:
                result = run_transcription(wav_path, out_dir, language_hint, engine, model_name)
                print(json.dumps(result), flush=True)
            except Exception as e:
                print(json.dumps({"error": str(e)}), flush=True)


if __name__ == "__main__":
    main()
