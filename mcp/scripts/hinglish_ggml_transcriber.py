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


def _probe_wav_duration(wav_path: str) -> float:
    """Duration in seconds of the 16kHz mono PCM WAV (stdlib wave module).

    whisper.cpp can emit trailing hallucinated segments past the real audio
    end; probing the WAV gives the authoritative duration to clamp against.
    Returns 0.0 on any failure (caller skips clamping).
    """
    try:
        import wave as _wave

        with _wave.open(wav_path, "rb") as w:
            frames = w.getnframes()
            rate = w.getframerate()
        return frames / float(rate) if rate else 0.0
    except Exception:
        return 0.0


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
    # -osrt: segment-level SRT (phrase windows — accurate VAD boundaries).
    # NOTE: -owts (word timestamps) is NOT requested — whisper.cpp writes a
    # karaoke bash script (not SRT), requires a font (-fp) or writes a 0-byte
    # file, and emits subword BPE fragments. Word-level timing is derived
    # downstream by char-proportional split inside the accurate phrase windows.
    # --vad: Voice Activity Detection (prevents hallucination loops)
    # --vad-model: Silero VAD model path
    cmd = [
        whisper_cli,
        "-m", model_path,
        "-f", wav_path,
        "-l", language_hint[:2] if language_hint != "auto" else "auto",
        "-t", "4",
        "-osrt",
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
    
    if vad_model is not None and vad_model.exists():
        cmd.extend(["--vad", "--vad-model", str(vad_model)])
        _log_hinglish(f"Using VAD model: {vad_model}")
    else:
        _log_hinglish("WARNING: Silero VAD model not found, running without VAD (may hallucinate)")
    
    _log_hinglish(f"Running: {' '.join(cmd[:6])}...")
    start = time.time()
    
    # Ensure libwhisper.so can be found at runtime.
    # The shared library lives in ~/.local/lib but whisper-cli may not
    # have RPATH set, so we prepend it to LD_LIBRARY_PATH.
    env = os.environ.copy()
    lib_dir = os.path.join(os.path.expanduser("~"), ".local", "lib")
    if os.path.isdir(lib_dir):
        existing = env.get("LD_LIBRARY_PATH", "")
        env["LD_LIBRARY_PATH"] = (
            lib_dir + (":" + existing if existing else "")
        )

    # Use Popen to stream stderr for progress reporting.
    # whisper-cli emits progress to stderr as percentage lines.
    import sys as _sys
    proc = subprocess.Popen(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=env,
    )

    stderr_lines = []
    # Read stderr line-by-line to extract progress from whisper-cli
    for line in proc.stderr:
        line = line.strip()
        stderr_lines.append(line)
        # whisper-cli emits lines like "progress: 50%" or "[00:05.000 --> 00:10.000]"
        if '%' in line:
            try:
                pct = float(line.split('%')[0].split()[-1])
                print(f'[progress:{pct:.0f}]', flush=True)
            except (ValueError, IndexError):
                pass

    proc.wait(timeout=600)
    stdout_text = proc.stdout.read() if proc.stdout else ''
    stderr_text = '\n'.join(stderr_lines)

    # Return values directly — no need for a wrapper object.
    # The Rust caller reads stdout/stderr from the process pipes.
    return_code = proc.returncode
    captured_stdout = stdout_text
    captured_stderr = stderr_text
    
    elapsed = time.time() - start
    _log_hinglish(f"whisper-cli completed in {elapsed:.1f}s (exit={return_code})")
    
    # Parse stderr for detected language
    detected_language = "hi"
    for line in captured_stderr.split('\n'):
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
    
    # Clamp hallucinated segments past the real audio end. whisper.cpp (with
    # VAD on, as here) can emit trailing segments beyond the source duration
    # — the caption-sync overrun bug: cues at 141-150s on 135.4s audio burn
    # text over silence at the tail. Probe the 16kHz mono WAV duration and
    # drop/clamp anything past it.
    wav_dur = _probe_wav_duration(wav_path)
    if wav_dur > 0:
        before = len(entries)
        entries = [e for e in entries if e["start_s"] < wav_dur - 0.05]
        for e in entries:
            if e["end_s"] > wav_dur:
                e["end_s"] = wav_dur
        if len(entries) != before:
            _log_hinglish(
                f"Clamped {before - len(entries)} hallucinated segment(s) past audio end ({wav_dur:.1f}s)"
            )
    
    # Parse word-level SRT (actual timestamps from -owts)
    all_words = []
    word_srt_path_out = Path(out_dir) / "whisper_words.srt"
    
    if word_srt_path_out.exists():
        _log_hinglish(f"Reading word-level timestamps from {word_srt_path_out}")
        all_words = _parse_word_srt_file(str(word_srt_path_out))
        _log_hinglish(f"Parsed {len(all_words)} words with actual timestamps")
        # Sanity: a REAL word SRT has ~1 entry per word. If the backend wrote a
        # phrase-sized "word" SRT (one entry whose text spans the whole phrase),
        # treat it as missing — downstream zip truncation would give the first
        # word the whole window and zero-duration blips for the rest (the
        # caption-sync bug). Rebuild from phrase windows below.
        phrase_sized = all_words and any(
            len(w["word"].strip().split()) > 1 for w in all_words
        )
        if phrase_sized:
            _log_hinglish("WARNING: word SRT is phrase-sized (multi-word entries) — rebuilding from phrase windows")
            all_words = []
    if not all_words:
        _log_hinglish("WARNING: no usable word timestamps — splitting phrase windows into char-proportional words")
        # Char-proportional split WITHIN each phrase window. Each word gets a
        # slice proportional to its char count, tiling the window exactly so
        # word-highlight captions stay audio-synced even without real ASR
        # word timestamps. (Estimation inside an accurate phrase window is
        # correct; the previous "one fake word per phrase" output collapsed
        # all highlights into the last milliseconds.)
        for entry in entries:
            text = entry["text"].strip()
            words = text.split()
            if not words:
                continue
            total_chars = sum(len(w) for w in words)
            span = entry["end_s"] - entry["start_s"]
            if span <= 0:
                # Zero-duration phrase (VAD anomaly) — skip; downstream tiling
                # would otherwise produce overlapping 0.01s cues.
                continue
            cursor = entry["start_s"]
            for i, w in enumerate(words):
                frac = len(w) / total_chars if total_chars else 1.0 / len(words)
                end_s = entry["end_s"] if i == len(words) - 1 else cursor + span * frac
                all_words.append({"word": w, "start_s": cursor, "end_s": max(end_s, cursor + 0.01)})
                cursor = end_s
    
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

    # Rename SRT files from *.nemotron.* to *.hinglish-ggml.*
    # (build_srt_files uses 'nemotron' prefix; Rust build_result looks for 'hinglish-ggml')
    for key, suffix in [('word_srt_path', '.hinglish-ggml.word.srt'),
                         ('phrase_srt_path', '.hinglish-ggml.phrase.srt'),
                         ('output_srt_path', '.hinglish-ggml.srt')]:
        src = Path(srt[key])
        dst = src.parent / (stem + suffix)
        if src.exists() and str(src) != str(dst):
            src.rename(dst)
            srt[key] = str(dst)

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
