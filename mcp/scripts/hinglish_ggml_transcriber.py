#!/usr/bin/env python3
"""
Hinglish GGML Transcription Sidecar — whisper.cpp with Whisper-Hindi2Hinglish-Apex-GGML.

Uses the quantized GGML model that directly outputs Hinglish (Latin script)
from Hindi audio. No LLM post-processing needed.

Model: Marquestra/Whisper-Hindi2Hinglish-Apex-GGML (q8_0, 0.87GB)
Based on: whisper-large-v3-turbo (32-layer encoder, 4-layer decoder)

Input:  stdin JSON  {"wav_path": "...", "language_hint": "hi-IN"|"auto"}
Output: stdout JSON {"text": "...", "word_srt_path": "...", "phrase_srt_path": "...",
                     "segments": [...], "duration_s": float, "engine": "hinglish-ggml"}

Also supports CLI: hinglish_ggml_transcriber.py run --video <path> --out-dir <dir>
"""

import argparse
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path

# Import shared utilities
from transcribe_common import (
    SAMPLE_RATE, PHRASE_MAX_WORDS, PHRASE_MAX_CHARS, PHRASE_MAX_GAP_S,
    SCRIPT_DIR, REPO_ROOT,
    _log, extract_audio, ensure_wav_16k,
    build_srt_files,
)

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

def _find_whisper_cli() -> str:
    """Find the whisper-cli binary."""
    env = os.environ.get("WHISPER_CLI", "")
    if env and Path(env).exists():
        return env
    candidates = [
        Path.home() / ".local/bin/whisper-cli",
        Path("/tmp/whisper.cpp/build/bin/whisper-cli"),
        REPO_ROOT / "tools/whisper-cli",
    ]
    for c in candidates:
        if c.exists() and os.access(str(c), os.X_OK):
            return str(c)
    raise FileNotFoundError(
        "whisper-cli not found. Build whisper.cpp or set WHISPER_CLI env var."
    )


def _find_ggml_model() -> str:
    """Find the GGML model file."""
    env = os.environ.get("HINGLISH_GGML_MODEL", "")
    if env and Path(env).exists():
        return env
    candidates = [
        Path.home() / "models/hinglish-whisper/ggml-apex-hinglish-q8_0.bin",
        REPO_ROOT / "models/hinglish-whisper/ggml-apex-hinglish-q8_0.bin",
    ]
    for c in candidates:
        if c.exists():
            return str(c)
    raise FileNotFoundError(
        "GGML model not found. Download from HuggingFace or set HINGLISH_GGML_MODEL env var."
    )


def _log_hinglish(msg: str):
    _log(msg, prefix="hinglish-ggml")


# ---------------------------------------------------------------------------
# SRT parsing
# ---------------------------------------------------------------------------

def _parse_srt_timestamp(ts: str) -> float:
    """Parse SRT timestamp (HH:MM:SS,mmm) to seconds."""
    ts = ts.strip().replace(',', '.')
    parts = ts.split(':')
    if len(parts) == 3:
        return float(parts[0]) * 3600 + float(parts[1]) * 60 + float(parts[2])
    return 0.0


def _parse_srt_file(srt_path: str) -> list:
    """Parse an SRT file into list of {start_s, end_s, text}."""
    with open(srt_path, 'r', encoding='utf-8') as f:
        content = f.read()
    entries = []
    blocks = content.strip().split('\n\n')
    for block in blocks:
        lines = block.strip().split('\n')
        if len(lines) >= 3:
            timestamp = lines[1].strip()
            text = ' '.join(lines[2:]).strip()
            match = re.match(r'(\d{2}:\d{2}:\d{2}[,\.]\d{3})\s*-->\s*(\d{2}:\d{2}:\d{2}[,\.]\d{3})', timestamp)
            if match and text:
                entries.append({
                    'start_s': _parse_srt_timestamp(match.group(1)),
                    'end_s': _parse_srt_timestamp(match.group(2)),
                    'text': text,
                })
    return entries


# ---------------------------------------------------------------------------
# Word-level SRT parsing (from -owts output)
# ---------------------------------------------------------------------------

def _parse_word_srt_file(srt_path: str) -> list:
    """Parse word-level SRT file into list of {word, start_s, end_s}."""
    with open(srt_path, 'r', encoding='utf-8') as f:
        content = f.read()
    words = []
    blocks = content.strip().split('\n\n')
    for block in blocks:
        lines = block.strip().split('\n')
        if len(lines) >= 3:
            timestamp = lines[1].strip()
            word = ' '.join(lines[2:]).strip()
            match = re.match(r'(\d{2}:\d{2}:\d{2}[,\.]\d{3})\s*-->\s*(\d{2}:\d{2}:\d{2}[,\.]\d{3})', timestamp)
            if match and word:
                words.append({
                    'word': word,
                    'start_s': _parse_srt_timestamp(match.group(1)),
                    'end_s': _parse_srt_timestamp(match.group(2)),
                })
    return words


# ---------------------------------------------------------------------------
# Transcription via whisper.cpp CLI
# ---------------------------------------------------------------------------

def transcribe_ggml(
    wav_path: str,
    out_dir: str,
    language_hint: str = "auto",
) -> dict:
    """Transcribe audio using whisper.cpp with the Hinglish GGML model.
    
    Uses -osrt for segment-level SRT and -owts for word-level SRT.
    Uses --vad for hallucination prevention.
    """
    whisper_cli = _find_whisper_cli()
    model_path = _find_ggml_model()
    
    _log_hinglish(f"whisper-cli: {whisper_cli}")
    _log_hinglish(f"Model: {model_path}")
    
    # Output paths
    segment_srt = Path(out_dir) / "whisper_segments.srt"
    
    # Build whisper-cli command
    # -osrt: segment-level SRT
    # -owts: word-level SRT (actual word timestamps)
    # --vad: Voice Activity Detection (prevents hallucination loops)
    # --vad-model: Silero VAD model path
    cmd = [
        whisper_cli,
        "-m", model_path,
        "-f", wav_path,
        "-l", language_hint[:2] if language_hint != "auto" else "auto",
        "-t", "4",
        "-osrt",
        "-owts",
        "--no-prints",
        "-of", str(segment_srt.with_suffix("")),
    ]
    
    # Add VAD if silero model exists
    # Check persistent locations first (OPENSCRIPT_ROOT, ~/.local/share, ~/models)
    # then fall back to /tmp/whisper.cpp/models (ephemeral)
    openscript_root = Path(os.environ.get("OPENSCRIPT_ROOT", REPO_ROOT))
    vad_candidates = [
        openscript_root / "models" / "silero" / "ggml-silero-v5.1.2.bin",
        Path.home() / ".local" / "share" / "openscript" / "models" / "silero" / "ggml-silero-v5.1.2.bin",
        Path.home() / "models" / "silero" / "ggml-silero-v5.1.2.bin",
        Path("/tmp/whisper.cpp/models/ggml-silero-v5.1.2.bin"),
    ]
    vad_model = None
    for candidate in vad_candidates:
        if candidate.exists():
            vad_model = candidate
            break
    
    if vad_model.exists():
        cmd.extend(["--vad", "--vad-model", str(vad_model)])
        _log_hinglish(f"Using VAD model: {vad_model}")
    else:
        _log_hinglish("WARNING: Silero VAD model not found, running without VAD (may hallucinate)")
    
    _log_hinglish(f"Running: {' '.join(cmd[:6])}...")
    start = time.time()
    
    result = subprocess.run(
        cmd,
        capture_output=True,
        text=True,
        timeout=600,
    )
    
    elapsed = time.time() - start
    _log_hinglish(f"whisper-cli completed in {elapsed:.1f}s (exit={result.returncode})")
    
    # Parse stderr for detected language
    detected_language = "hi"
    for line in result.stderr.split('\n'):
        if 'language:' in line.lower() or 'detected' in line.lower():
            lang_match = re.search(r'language[:\s]+(\w+)', line, re.IGNORECASE)
            if lang_match:
                detected_language = lang_match.group(1)
                break
    
    _log_hinglish(f"Detected language: {detected_language}")
    
    # Parse segment-level SRT
    if not segment_srt.exists():
        # Try finding it with different extension patterns
        for ext in ['.srt', '']:
            candidate = Path(str(segment_srt) + ext)
            if candidate.exists():
                segment_srt = candidate
                break
    
    if not segment_srt.exists():
        return {"error": f"whisper-cli did not produce segment SRT at {segment_srt}"}
    
    _log_hinglish(f"Reading segments from {segment_srt}")
    entries = _parse_srt_file(str(segment_srt))
    
    if not entries:
        return {"error": "Segment SRT was empty or unparseable"}
    
    _log_hinglish(f"Parsed {len(entries)} segments")
    
    # Parse word-level SRT (actual timestamps from -owts)
    all_words = []
    word_srt_path_out = Path(out_dir) / "whisper_words.srt"
    
    if word_srt_path_out.exists():
        _log_hinglish(f"Reading word-level timestamps from {word_srt_path_out}")
        all_words = _parse_word_srt_file(str(word_srt_path_out))
        _log_hinglish(f"Parsed {len(all_words)} words with actual timestamps")
    else:
        _log_hinglish("WARNING: Word-level SRT not found — using segment-level data only")
        # Do NOT estimate word timestamps — they produce garbage timing
        # that breaks word-highlight captions. Use segment-level text only.
        for entry in entries:
            all_words.append({
                "word": entry["text"],
                "start_s": entry["start_s"],
                "end_s": entry["end_s"],
            })
    
    full_text = " ".join(s["text"] for s in entries)
    duration_s = entries[-1]["end_s"] if entries else 0.0
    
    _log_hinglish(f"Words: {len(all_words)}, Segments: {len(entries)}, Duration: {duration_s:.1f}s")
    
    # Cleanup temp SRT files
    for f in [segment_srt, word_srt_path_out]:
        try:
            os.remove(f)
        except OSError:
            pass
    
    return {
        "text": full_text.strip(),
        "words": all_words,
        "segments": entries,
        "word_count": len(all_words),
        "segment_count": len(entries),
        "duration_s": duration_s,
        "load_time_s": 0,
        "transcribe_time_s": elapsed,
        "language": detected_language,
        "model": "hinglish-apex-ggml-q8",
    }


# ---------------------------------------------------------------------------
# Main pipeline
# ---------------------------------------------------------------------------

def run_transcription(
    media_path: str,
    out_dir: str,
    language_hint: str = "auto",
) -> dict:
    """Full transcription pipeline: media -> text -> SRT."""
    Path(out_dir).mkdir(parents=True, exist_ok=True)
    stem = Path(media_path).stem
    
    _log_hinglish(f"Preparing audio from {media_path}...")
    wav_path = ensure_wav_16k(media_path, out_dir, suffix="hinglish")
    _log_hinglish(f"Audio ready: {wav_path}")
    
    result = transcribe_ggml(wav_path, out_dir, language_hint)
    
    if result.get("error"):
        try:
            os.remove(wav_path)
        except OSError:
            pass
        return result
    
    text = result["text"]
    words = result.get("words", [])
    
    if not text or not words:
        try:
            os.remove(wav_path)
        except OSError:
            pass
        return {
            "error": "No text produced from transcription",
            "status": "error",
            "engine": "hinglish-ggml",
        }
    
    # Generate SRT files via shared utility
    srt = build_srt_files(words, out_dir, stem)
    result["status"] = "transcribed"
    result["engine"] = "hinglish-ggml"
    result["word_srt_path"] = srt["word_srt_path"]
    result["phrase_srt_path"] = srt["phrase_srt_path"]
    result["output_srt_path"] = srt["output_srt_path"]
    result["segments"] = [
        {"text": p["text"], "start_s": p["start_s"], "end_s": p["end_s"]}
        for p in srt["phrases"]
    ]
    
    _log_hinglish(f"Pipeline complete: {len(words)} words, {len(srt['phrases'])} phrases")
    _log_hinglish(f"Output: {srt['output_srt_path']}")
    
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
        description="Hinglish GGML Transcription Sidecar — whisper.cpp + Hindi2Hinglish"
    )
    sub = parser.add_subparsers(dest="cmd", required=True)
    
    p_run = sub.add_parser("run", help="Transcribe a media file")
    p_run.add_argument("--video", required=True, help="Path to video/audio file")
    p_run.add_argument("--out-dir", default=None, help="Output directory")
    p_run.add_argument("--language", default="auto", help="Language hint (auto, hi-IN, en-US)")
    
    p_serve = sub.add_parser("serve", help="Long-lived stdin/stdout JSON mode")
    
    args = parser.parse_args()
    
    if args.cmd == "run":
        out_dir = args.out_dir or str(Path(args.video).parent)
        if Path(out_dir).resolve() == Path("/"):
            out_dir = "."
        result = run_transcription(args.video, out_dir, args.language)
        if result.get("error"):
            _log_hinglish(f"ERROR: {result['error']}")
            print(json.dumps(result))
            sys.exit(1)
        print(json.dumps(result))
    
    elif args.cmd == "serve":
        _log_hinglish("Starting stdin/stdout serve mode")
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
            if not wav_path:
                print(json.dumps({"error": "missing wav_path"}), flush=True)
                continue
            try:
                result = run_transcription(wav_path, out_dir, language_hint)
                print(json.dumps(result), flush=True)
            except Exception as e:
                print(json.dumps({"error": str(e)}), flush=True)


if __name__ == "__main__":
    main()
