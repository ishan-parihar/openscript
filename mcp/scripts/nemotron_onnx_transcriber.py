#!/usr/bin/env python3
"""
Nemotron ONNX Transcription Sidecar — onnxruntime-genai with cache-aware streaming.

Uses the StreamingProcessor API from onnxruntime-genai for proper cache-aware
inference. The model processes audio in 560ms chunks (8960 samples at 16kHz)
with automatic cache management between chunks.

Input:  stdin JSON  {"wav_path": "...", "language_hint": "hi-IN"|"auto"}
Output: stdout JSON {"text": "...", "word_srt_path": "...", "phrase_srt_path": "...",
                     "segments": [...], "duration_s": float, "engine": "nemotron-onnx"}

Also supports one-shot CLI: nemotron_onnx_transcriber.py run --video <path> --out-dir <dir>
"""

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path

import numpy as np

# Import shared utilities from transcribe_common
from transcribe_common import (
    SAMPLE_RATE, PHRASE_MAX_WORDS, PHRASE_MAX_CHARS, PHRASE_MAX_GAP_S,
    SCRIPT_DIR, REPO_ROOT,
    _log, extract_audio, ensure_wav_16k,
    fmt_ts, group_words_into_phrases, generate_word_srt, generate_phrase_srt,
    build_srt_files,
)

# ---------------------------------------------------------------------------
# Language ID mapping (from official onnxruntime-genai example)
# ---------------------------------------------------------------------------

LANG_TO_ID = {
    "en":     (0,   "English (default / US)"),
    "en-US":  (0,   "English (United States)"),
    "en-GB":  (1,   "English (United Kingdom)"),
    "es-ES":  (2,   "Spanish (Spain)"),
    "es":     (3,   "Spanish (default / Latin America)"),
    "es-US":  (3,   "Spanish (US Latin American)"),
    "zh-CN":  (4,   "Chinese (Mandarin, Simplified)"),
    "hi":     (6,   "Hindi"),
    "hi-IN":  (6,   "Hindi (India)"),
    "ar":     (7,   "Arabic"),
    "ar-AR":  (7,   "Arabic"),
    "fr":     (8,   "French (default / France)"),
    "fr-FR":  (8,   "French (France)"),
    "de":     (9,   "German"),
    "de-DE":  (9,   "German (Germany)"),
    "ja":     (10,  "Japanese"),
    "ja-JP":  (10,  "Japanese"),
    "ru":     (11,  "Russian"),
    "ru-RU":  (11,  "Russian"),
    "pt-BR":  (12,  "Portuguese (Brazil)"),
    "pt":     (13,  "Portuguese (default / Portugal)"),
    "pt-PT":  (13,  "Portuguese (Portugal)"),
    "ko":     (14,  "Korean"),
    "ko-KR":  (14,  "Korean (South Korea)"),
    "it":     (15,  "Italian"),
    "it-IT":  (15,  "Italian"),
    "nl":     (16,  "Dutch"),
    "nl-NL":  (16,  "Dutch (Netherlands)"),
    "pl":     (17,  "Polish"),
    "pl-PL":  (17,  "Polish"),
    "tr":     (18,  "Turkish"),
    "tr-TR":  (18,  "Turkish"),
    "uk":     (19,  "Ukrainian"),
    "uk-UA":  (19,  "Ukrainian"),
    "ro":     (20,  "Romanian"),
    "ro-RO":  (20,  "Romanian"),
    "el":     (21,  "Greek"),
    "el-GR":  (21,  "Greek"),
    "cs":     (22,  "Czech"),
    "cs-CZ":  (22,  "Czech"),
    "hu":     (23,  "Hungarian"),
    "hu-HU":  (23,  "Hungarian"),
    "sv":     (24,  "Swedish"),
    "sv-SE":  (24,  "Swedish"),
    "da":     (25,  "Danish"),
    "da-DK":  (25,  "Danish"),
    "fi":     (26,  "Finnish"),
    "fi-FI":  (26,  "Finnish"),
    "sk":     (28,  "Slovak"),
    "sk-SK":  (28,  "Slovak"),
    "hr":     (29,  "Croatian"),
    "hr-HR":  (29,  "Croatian"),
    "bg":     (30,  "Bulgarian"),
    "bg-BG":  (30,  "Bulgarian"),
    "lt":     (31,  "Lithuanian"),
    "lt-LT":  (31,  "Lithuanian"),
    "th":     (32,  "Thai"),
    "th-TH":  (32,  "Thai"),
    "vi":     (33,  "Vietnamese"),
    "vi-VN":  (33,  "Vietnamese"),
    "et":     (60,  "Estonian"),
    "et-EE":  (60,  "Estonian"),
    "lv":     (61,  "Latvian"),
    "lv-LV":  (61,  "Latvian"),
    "sl":     (62,  "Slovenian"),
    "sl-SI":  (62,  "Slovenian"),
    "he":     (64,  "Hebrew"),
    "he-IL":  (64,  "Hebrew (Israel)"),
    "fr-CA":  (100, "French (Canada)"),
    "auto":   (101, "Auto-detect"),
    "mt":     (102, "Maltese"),
    "mt-MT":  (102, "Maltese"),
    "nb":     (103, "Norwegian Bokmål"),
    "nb-NO":  (103, "Norwegian Bokmål"),
    "nn":     (104, "Norwegian Nynorsk"),
    "nn-NO":  (104, "Norwegian Nynorsk"),
}


def _log_onnx(msg: str):
    _log(msg, prefix="nemotron-onnx")


def resolve_language_id(language_hint: str) -> int:
    """Resolve language hint to language ID for the model."""
    hint = language_hint.strip().lower()
    # Exact match first
    if hint in LANG_TO_ID:
        return LANG_TO_ID[hint][0]
    # Try prefix match (e.g., "hi" matches "hi-IN")
    for key, (lid, _) in LANG_TO_ID.items():
        if hint.startswith(key) or key.startswith(hint):
            return lid
    # Default to English
    return 0


# ---------------------------------------------------------------------------
# Audio extraction (from video)
# ---------------------------------------------------------------------------

def load_audio_wav(wav_path: str) -> np.ndarray:
    """Load a 16kHz mono WAV file as float32 numpy array."""
    import wave
    with wave.open(wav_path, "r") as wf:
        frames = wf.readframes(wf.getnframes())
        audio = np.frombuffer(frames, dtype=np.int16).astype(np.float32) / 32768.0
    return audio



def transcribe_nemotron_onnx(
    wav_path: str,
    out_dir: str,
    language_hint: str = "auto",
    model_dir: str = None,
) -> dict:
    """Transcribe audio using Nemotron ONNX via onnxruntime-genai.

    Uses the StreamingProcessor API for proper cache-aware inference.
    Audio is processed in 560ms chunks (8960 samples at 16kHz).

    Args:
        wav_path: Path to 16kHz mono WAV
        language_hint: Language code (e.g., "hi-IN", "en-US", "auto")
        model_dir: Path to model directory (default: models/nemotron-onnx)

    Returns:
        dict with text, words, segments, timing info
    """
    try:
        import onnxruntime_genai as og
    except ImportError:
        return {"error": "onnxruntime-genai not installed. Run: pip install onnxruntime-genai"}

    if model_dir is None:
        model_dir = str(REPO_ROOT / "models" / "nemotron-onnx")

    if not Path(model_dir).exists():
        return {"error": f"Model directory not found: {model_dir}"}

    # Load audio
    _log_onnx(f"Loading audio: {wav_path}")
    audio = load_audio_wav(wav_path)
    duration_s = len(audio) / SAMPLE_RATE
    _log_onnx(f"Audio duration: {duration_s:.1f}s, samples: {len(audio)}")

    # Resolve language
    lang_id = resolve_language_id(language_hint)
    lang_name = LANG_TO_ID.get(language_hint, LANG_TO_ID.get("auto", (101, "Auto-detect")))[1]
    _log_onnx(f"Language: {language_hint} -> {lang_name} (lang_id={lang_id})")

    # Load model
    _log_onnx(f"Loading Nemotron ONNX model from {model_dir}...")
    start = time.time()

    try:
        config = og.Config(model_dir)
        model = og.Model(config)
    except Exception as e:
        return {"error": f"Failed to load ONNX model: {e}"}

    load_time = time.time() - start
    _log_onnx(f"Model loaded in {load_time:.1f}s")

    # Create processor and generator
    processor = og.StreamingProcessor(model)
    tokenizer = og.Tokenizer(model)
    tokenizer_stream = tokenizer.create_stream()

    params = og.GeneratorParams(model)
    params.set_search_options(
        max_length=4096,
        batch_size=1,
    )

    # Set language ID via prompt if supported
    if lang_id != 101:  # Not auto-detect
        try:
            # The model uses language-ID prompt conditioning
            # We set it as a generation parameter if the API supports it
            pass  # Language is handled via the model's internal prompt dictionary
        except Exception:
            pass

    generator = og.Generator(model, params)

    # chunk_samples is already cached at module level
    import json
    with open(os.path.join(model_dir, "genai_config.json"), "r") as f:
        genai_config = json.load(f)
    chunk_samples = genai_config["model"]["chunk_samples"]  # 8960
    _log_onnx(f"Chunk size: {chunk_samples} samples ({chunk_samples / SAMPLE_RATE:.1f}s)")

    # Process audio in chunks
    _log_onnx("Running streaming inference...")
    transcribe_start = time.time()

    full_text = ""
    chunks_total = 0
    chunks_processed = 0

    for i in range(0, len(audio), chunk_samples):
        chunk = audio[i:i + chunk_samples].astype(np.float32)

        # Pad if last chunk is smaller
        if len(chunk) < chunk_samples:
            chunk = np.pad(chunk, (0, chunk_samples - len(chunk)))

        chunks_total += 1

        # Process chunk through StreamingProcessor
        inputs = processor.process(chunk)

        # If the processor returns inputs (not silenced by VAD)
        if inputs is not None:
            generator.set_inputs(inputs)

            # Decode generated tokens
            while not generator.is_done():
                generator.generate_next_token()
                tokens = generator.get_next_tokens()
                if len(tokens) > 0:
                    token_text = tokenizer_stream.decode(tokens[0])
                    if token_text:
                        full_text += token_text

            chunks_processed += 1

    # Flush remaining context
    _log_onnx("Flushing remaining context...")
    inputs = processor.flush()
    if inputs is not None:
        generator.set_inputs(inputs)
        while not generator.is_done():
            generator.generate_next_token()
            tokens = generator.get_next_tokens()
            if len(tokens) > 0:
                token_text = tokenizer_stream.decode(tokens[0])
                if token_text:
                    full_text += token_text

    transcribe_time = time.time() - transcribe_start

    _log_onnx(f"Transcription done in {transcribe_time:.1f}s")
    _log_onnx(f"Chunks: {chunks_processed}/{chunks_total} processed")
    _log_onnx(f"Text: {full_text[:200]}...")

    # Clean up
    del generator

    if not full_text.strip():
        return {"error": "No text produced from transcription", "status": "error"}

    # Step 3: Get real word-level timestamps via whisper_align.py
    words = []
    word_list = full_text.strip().split()
    if word_list:
        # Try whisper_align.py for real timestamps
        whisper_align_script = None
        for candidate in [
            SCRIPT_DIR / "whisper_align.py",
            REPO_ROOT / "mcp" / "scripts" / "whisper_align.py",
        ]:
            if candidate.exists():
                whisper_align_script = candidate
                break

        if whisper_align_script is not None:
            _log_onnx("Running whisper_align.py for word-level alignment")
            import json as json_mod
            align_result = subprocess.run(
                [
                    sys.executable, str(whisper_align_script),
                    "--wav", wav_path,
                    "--text", full_text.strip(),
                    "--language", language_hint[:2] if language_hint != "auto" else "en",
                    "--model", "base",
                    "--out-dir", out_dir,
                ],
                capture_output=True, text=True, timeout=300,
            )
            if align_result.returncode == 0:
                try:
                    # whisper_align.py outputs JSON to stdout
                    align_data = json_mod.loads(align_result.stdout.strip())
                    if "words" in align_data and align_data["words"]:
                        words = [
                            {
                                "word": w.get("word", "").strip(),
                                "start_s": w.get("start_s", 0),
                                "end_s": w.get("end_s", 0),
                                "score": w.get("score", 0.0),
                            }
                            for w in align_data["words"]
                            if w.get("word", "").strip()
                        ]
                        _log_onnx(f"Whisper alignment complete: {len(words)} words with real timestamps")
                except Exception as e:
                    _log_onnx(f"Failed to parse whisper_align output: {e}")
            else:
                _log_onnx(f"whisper_align.py failed: {align_result.stderr[:200] if align_result.stderr else 'unknown error'}")

        # Fallback: estimated timestamps if alignment failed
        if not words and word_list:
            _log_onnx("Using estimated word timestamps (alignment unavailable)")
            word_duration = duration_s / len(word_list)
            words = [
                {"word": w, "start_s": i * word_duration, "end_s": (i + 1) * word_duration, "score": 0.0}
                for i, w in enumerate(word_list)
            ]

    return {
        "text": full_text.strip(),
        "words": words,
        "segments": [{"text": full_text.strip(), "start_s": 0, "end_s": duration_s}],
        "word_count": len(words),
        "segment_count": 1,
        "duration_s": duration_s,
        "load_time_s": load_time,
        "transcribe_time_s": transcribe_time,
        "language": language_hint,
        "language_id": lang_id,
        "model": "nemotron-3.5-asr-streaming-0.6b-onnx-int4",
        "engine": "nemotron-onnx",
    }


# ---------------------------------------------------------------------------
# Main pipeline
# ---------------------------------------------------------------------------

def run_transcription(
    media_path: str,
    out_dir: str,
    language_hint: str = "auto",
    model_dir: str = None,
) -> dict:
    """Full transcription pipeline: media -> text -> SRT.

    Args:
        media_path: Path to video or audio file
        out_dir: Output directory for SRT files
        language_hint: "auto", "hi-IN", "en-US", etc.
        model_dir: Path to Nemotron ONNX model directory

    Returns:
        dict with status, text, SRT paths, segments, etc.
    """
    Path(out_dir).mkdir(parents=True, exist_ok=True)
    stem = Path(media_path).stem

    # Step 1: Convert to 16kHz mono WAV
    _log_onnx(f"Preparing audio from {media_path}...")
    wav_path = ensure_wav_16k(media_path, out_dir)
    _log_onnx(f"Audio ready: {wav_path}")

    # Step 2: Transcribe with Nemotron ONNX
    result = transcribe_nemotron_onnx(wav_path, out_dir, language_hint, model_dir)

    if result.get("error"):
        try:
            os.remove(wav_path)
        except OSError:
            pass
        return result

    text = result["text"]
    words = result.get("words", [])
    duration_s = result.get("duration_s", 0.0)

    if not text or not words:
        try:
            os.remove(wav_path)
        except OSError:
            pass
        return {
            "error": "No text produced from transcription",
            "status": "error",
            "engine": "nemotron-onnx",
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
    result["engine"] = "nemotron-onnx"
    result["language"] = result.get("language", language_hint)
    result["word_srt_path"] = word_srt_path
    result["phrase_srt_path"] = phrase_srt_path
    result["output_srt_path"] = output_srt_path
    result["segments"] = [
        {"text": p["text"], "start_s": p["start_s"], "end_s": p["end_s"]}
        for p in phrases
    ]

    _log_onnx(f"Pipeline complete: {len(words)} words, {len(phrases)} phrases")
    _log_onnx(f"Output: {output_srt_path}")

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
        description="Nemotron ONNX Transcription Sidecar — cache-aware streaming via onnxruntime-genai"
    )
    sub = parser.add_subparsers(dest="cmd", required=True)

    # One-shot mode
    p_run = sub.add_parser("run", help="Transcribe a media file")
    p_run.add_argument("--video", required=True, help="Path to video/audio file")
    p_run.add_argument("--out-dir", default=None, help="Output directory")
    p_run.add_argument("--language", default="auto", help="Language hint (auto, hi-IN, en-US)")
    p_run.add_argument("--model-dir", default=None, help="Path to Nemotron ONNX model directory")
    p_run.add_argument("--threads", type=int, default=4, help="Number of threads")

    # Stdin/stdout serve mode (for Rust sidecar)
    p_serve = sub.add_parser("serve", help="Long-lived stdin/stdout JSON mode")

    args = parser.parse_args()

    if args.cmd == "run":
        out_dir = args.out_dir or str(Path(args.video).parent)
        if Path(out_dir).resolve() == Path("/"):
            out_dir = "."
        result = run_transcription(
            args.video, out_dir, args.language, args.model_dir
        )
        if result.get("error"):
            _log_onnx(f"ERROR: {result['error']}")
            print(json.dumps(result))
            sys.exit(1)
        print(json.dumps(result))

    elif args.cmd == "serve":
        _log_onnx("Starting stdin/stdout serve mode")
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
            model_dir = req.get("model_dir", None)

            if not wav_path:
                print(json.dumps({"error": "missing wav_path"}), flush=True)
                continue

            try:
                result = run_transcription(wav_path, out_dir, language_hint, model_dir)
                print(json.dumps(result), flush=True)
            except Exception as e:
                print(json.dumps({"error": str(e)}), flush=True)


if __name__ == "__main__":
    main()
