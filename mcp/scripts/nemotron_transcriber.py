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

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parent.parent  # openscript root
SAMPLE_RATE = 16000

# Phrase grouping parameters
PHRASE_MAX_WORDS = 12
PHRASE_MAX_CHARS = 64
PHRASE_MAX_GAP_S = 0.6

# Whisper model sizes: tiny < base < small < medium < large-v3
# tiny: fastest, ~39x realtime on CPU, lowest accuracy
# base: good balance, ~10x realtime on CPU
WHISPER_DEFAULT_MODEL = "base"


def _log(msg: str):
    print(f"[transcriber] {msg}", file=sys.stderr, flush=True)


# ---------------------------------------------------------------------------
# Audio extraction (from video)
# ---------------------------------------------------------------------------

def extract_audio(video_path: str, wav_path: str) -> bool:
    """Extract 16kHz mono WAV from video using ffmpeg."""
    cmd = [
        "ffmpeg", "-y", "-i", video_path,
        "-vn", "-acodec", "pcm_s16le",
        "-ar", str(SAMPLE_RATE), "-ac", "1",
        wav_path,
    ]
    _log("Extracting audio...")
    result = subprocess.run(cmd, capture_output=True, timeout=300)
    if result.returncode != 0:
        _log(f"Audio extraction failed: {result.stderr.decode()[:300]}")
        return False
    _log(f"Audio extracted: {wav_path}")
    return True


def ensure_wav_16k(media_path: str, out_dir: str) -> str:
    """Convert any media to 16kHz mono WAV. Returns path to WAV file."""
    stem = Path(media_path).stem
    wav_path = str(Path(out_dir) / f"{stem}.whisper.wav")

    if media_path.lower().endswith((".wav",)):
        # Already WAV — just convert to 16kHz mono
        cmd = [
            "ffmpeg", "-y", "-i", media_path,
            "-vn", "-acodec", "pcm_s16le",
            "-ar", str(SAMPLE_RATE), "-ac", "1",
            wav_path,
        ]
        subprocess.run(cmd, capture_output=True, timeout=300)
    elif media_path.lower().endswith((".mp3", ".flac", ".ogg", ".m4a", ".aac")):
        cmd = [
            "ffmpeg", "-y", "-i", media_path,
            "-acodec", "pcm_s16le",
            "-ar", str(SAMPLE_RATE), "-ac", "1",
            wav_path,
        ]
        subprocess.run(cmd, capture_output=True, timeout=300)
    else:
        # Video file — extract audio
        if not extract_audio(media_path, wav_path):
            raise RuntimeError(f"Audio extraction failed for {media_path}")

    return wav_path


# ---------------------------------------------------------------------------
# Whisper transcription (PRIMARY engine)
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
# SRT generation
# ---------------------------------------------------------------------------

def fmt_ts(seconds: float) -> str:
    """Format seconds to SRT timestamp."""
    ms = round((seconds % 1) * 1000) % 1000
    s_int = int(seconds)
    return "%02d:%02d:%02d,%03d" % (
        s_int // 3600, (s_int % 3600) // 60, s_int % 60, ms
    )


def group_words_into_phrases(
    words: list,
    max_words: int = PHRASE_MAX_WORDS,
    max_chars: int = PHRASE_MAX_CHARS,
    max_gap: float = PHRASE_MAX_GAP_S,
) -> list:
    """Group word-level entries into phrase-level SRT entries."""
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
            groups.append({
                "text": " ".join(cur_words),
                "start_s": cur_start,
                "end_s": cur_end,
            })
            cur_start = start
            cur_end = end
            cur_words = [text]
        else:
            cur_words.append(text)
            cur_end = end

    if cur_words:
        groups.append({
            "text": " ".join(cur_words),
            "start_s": cur_start,
            "end_s": cur_end,
        })

    return groups


def generate_word_srt(words: list, output_path: str) -> str:
    """Generate word-level SRT file."""
    with open(output_path, "w", encoding="utf-8") as f:
        for i, w in enumerate(words, 1):
            f.write("%d\n%s --> %s\n%s\n\n" % (
                i, fmt_ts(w["start_s"]), fmt_ts(w["end_s"]), w["word"].strip()
            ))
    return output_path


def generate_phrase_srt(phrases: list, output_path: str) -> str:
    """Generate phrase-level SRT file."""
    with open(output_path, "w", encoding="utf-8") as f:
        for i, p in enumerate(phrases, 1):
            f.write("%d\n%s --> %s\n%s\n\n" % (
                i, fmt_ts(p["start_s"]), fmt_ts(p["end_s"]), p["text"]
            ))
    return output_path


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

    # Step 3: Generate SRT files
    word_srt_path = str(Path(out_dir) / f"{stem}.nemotron.word.srt")
    phrase_srt_path = str(Path(out_dir) / f"{stem}.nemotron.phrase.srt")
    output_srt_path = str(Path(out_dir) / f"{stem}.nemotron.srt")

    generate_word_srt(words, word_srt_path)

    phrases = group_words_into_phrases(words)
    generate_phrase_srt(phrases, phrase_srt_path)
    generate_phrase_srt(phrases, output_srt_path)

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
