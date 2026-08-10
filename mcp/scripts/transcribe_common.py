#!/usr/bin/env python3
"""
Shared transcription utilities for Whisper and Nemotron ONNX sidecars.

This module contains the common functions used by both transcription engines:
- Audio extraction and conversion (extract_audio, ensure_wav_16k)
- SRT generation (fmt_ts, group_words_into_phrases, generate_word_srt, generate_phrase_srt)
- Logging (_log)
- Constants (SAMPLE_RATE, PHRASE_MAX_WORDS, etc.)

Both nemotron_transcriber.py and nemotron_onnx_transcriber.py import from this module
to avoid code duplication.
"""

import json
import os
import subprocess
import sys
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


def _log(msg: str, prefix: str = "transcriber"):
    """Log a message to stderr with a prefix."""
    print(f"[{prefix}] {msg}", file=sys.stderr, flush=True)


# ---------------------------------------------------------------------------
# ONNX device selection (GPU-first)
# ---------------------------------------------------------------------------

_CUDA_OK = None


def cuda_usable():
    """True when the CUDA driver actually works right now (memoized).

    `get_available_providers()` only reports compile-time availability; a
    driver/library mismatch (NVML "Driver/library version mismatch") makes
    every CUDAExecutionProvider session fail with a ~30s EP retry storm per
    session plus an EP Error banner printed to STDOUT — which corrupts the
    JSON stdout protocol of sidecars like parakeet_align. Probe libcuda
    cuInit once and memoize.
    """
    global _CUDA_OK
    if _CUDA_OK is not None:
        return _CUDA_OK
    try:
        import ctypes

        lib = ctypes.CDLL("libcuda.so.1")
        _CUDA_OK = lib.cuInit(0) == 0
    except Exception:
        _CUDA_OK = False
    return _CUDA_OK


def ort_providers():
    """Return ONNX Runtime providers, preferring CUDA when available.

    OPENSCRIPT_DEVICE=auto (default) | cuda | cpu forces the choice.
    Requires onnxruntime-gpu for CUDAExecutionProvider; falls back to CPU.
    Used by parakeet_align.py and any ONNX sidecar that imports this module.
    """
    dev = os.environ.get("OPENSCRIPT_DEVICE", "auto").strip().lower()
    if dev == "cpu":
        return ["CPUExecutionProvider"]
    try:
        import onnxruntime as _ort
        available = _ort.get_available_providers()
    except Exception:
        return ["CPUExecutionProvider"]
    if dev == "cuda":
        if "CUDAExecutionProvider" in available:
            return ["CUDAExecutionProvider", "CPUExecutionProvider"]
        _log("OPENSCRIPT_DEVICE=cuda requested but CUDAExecutionProvider is not "
             "available (onnxruntime-gpu missing?) — falling back to CPU", "device")
        return ["CPUExecutionProvider"]
    if dev == "auto" and "CUDAExecutionProvider" in available and cuda_usable():
        return ["CUDAExecutionProvider", "CPUExecutionProvider"]
    return ["CPUExecutionProvider"]


# ---------------------------------------------------------------------------
# Audio extraction (from video)
# ---------------------------------------------------------------------------

def extract_audio(video_path: str, wav_path: str, prefix: str = "transcriber") -> bool:
    """Extract 16kHz mono WAV from video using ffmpeg."""
    cmd = [
        "ffmpeg", "-y", "-i", video_path,
        "-vn", "-acodec", "pcm_s16le",
        "-ar", str(SAMPLE_RATE), "-ac", "1",
        wav_path,
    ]
    _log("Extracting audio...", prefix)
    result = subprocess.run(cmd, capture_output=True, timeout=300)
    if result.returncode != 0:
        _log(f"Audio extraction failed: {result.stderr.decode()[:300]}", prefix)
        return False
    _log(f"Audio extracted: {wav_path}", prefix)
    return True


def ensure_wav_16k(media_path: str, out_dir: str, prefix: str = "transcriber", suffix: str = "whisper") -> str:
    """Convert any media to 16kHz mono WAV. Returns path to WAV file.

    Args:
        media_path: Path to input media file
        out_dir: Output directory for WAV file
        prefix: Log prefix (e.g., "transcriber", "nemotron-onnx")
        suffix: WAV filename suffix (e.g., "whisper", "nemotron")

    Returns:
        Path to the WAV file
    """
    stem = Path(media_path).stem
    wav_path = str(Path(out_dir) / f"{stem}.{suffix}.wav")

    if media_path.lower().endswith((".wav",)):
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
        if not extract_audio(media_path, wav_path, prefix):
            raise RuntimeError(f"Audio extraction failed for {media_path}")

    return wav_path


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
# Pipeline builder
# ---------------------------------------------------------------------------

def build_srt_files(
    words: list,
    out_dir: str,
    stem: str,
) -> dict:
    """Generate word-level, phrase-level, and output SRT files.

    Args:
        words: List of word dicts with 'word', 'start_s', 'end_s'
        out_dir: Output directory
        stem: Filename stem (without extension)

    Returns:
        dict with word_srt_path, phrase_srt_path, output_srt_path, phrases
    """
    # Generate SRT files
    word_srt_path = str(Path(out_dir) / f"{stem}.nemotron.word.srt")
    phrase_srt_path = str(Path(out_dir) / f"{stem}.nemotron.phrase.srt")
    output_srt_path = str(Path(out_dir) / f"{stem}.nemotron.srt")

    generate_word_srt(words, word_srt_path)

    phrases = group_words_into_phrases(words)
    generate_phrase_srt(phrases, phrase_srt_path)
    generate_phrase_srt(phrases, output_srt_path)

    return {
        "word_srt_path": word_srt_path,
        "phrase_srt_path": phrase_srt_path,
        "output_srt_path": output_srt_path,
        "phrases": phrases,
    }
