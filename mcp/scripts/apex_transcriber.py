#!/usr/bin/env python3
"""
Apex Transcriber — Oriserve/Whisper-Hindi2Hinglish-Apex (ONLY transcription model).

APEX IS THE ONLY TRANSCRIPTION MODEL. NO FALLBACKS. NO ALTERNATIVES.
This wrapper is the sole entry point for all transcription in OpenScript.
"""

import argparse
import os
import shlex
import subprocess
import sys
from pathlib import Path


def _log(msg: str):
    print("[apex] " + msg, file=sys.stderr)
    sys.stderr.flush()


def _find_whisper_python() -> str:
    """Find the whisper-hindi conda env python."""
    candidates = [
        os.environ.get("WHISPER_HINDI_PYTHON"),
        str(Path.home() / "miniconda3/envs/whisper-hindi/bin/python3.11"),
        str(Path.home() / "miniconda3/envs/whisper-hindi/bin/python3"),
        str(Path.home() / "anaconda3/envs/whisper-hindi/bin/python3.11"),
    ]
    for c in candidates:
        if c and Path(c).exists():
            return c
    _log("WARNING: whisper-hindi python not found, using system python")
    return sys.executable


def extract_audio(video: str, wav_path: str) -> bool:
    """Extract 16kHz mono WAV from video."""
    cmd = (
        "ffmpeg -y -i "
        + shlex.quote(os.path.abspath(video))
        + " -vn -acodec pcm_s16le -ar 16000 -ac 1 "
        + shlex.quote(os.path.abspath(wav_path))
    )
    _log("Extracting audio...")
    result = subprocess.run(cmd, shell=True, capture_output=True, text=True)
    if result.returncode != 0:
        _log("Audio extraction failed: " + result.stderr[:300])
        return False
    _log("Audio extracted: " + wav_path)
    return True


def run_apex_transcription(
    whisper_python: str,
    wav_path: str,
    out_dir: str,
    stem: str,
) -> dict:
    """
    Run Apex transcription in a subprocess inside the whisper-hindi conda env.
    Runs on CPU to avoid VRAM conflicts.
    Returns dict with text, word_count, srt paths.
    """
    text_out = os.path.join(out_dir, stem + ".apex.txt")
    word_srt_out = os.path.join(out_dir, stem + ".apex.word.srt")
    phrase_srt_out = os.path.join(out_dir, stem + ".apex.phrase.srt")

    script = (
        """
import sys
import time
import torch
import whisper_timestamped as whisper

audio_path = """
        + repr(wav_path)
        + """
text_out = """
        + repr(text_out)
        + """
word_srt_out = """
        + repr(word_srt_out)
        + """
phrase_srt_out = """
        + repr(phrase_srt_out)
        + """

_log = lambda m: print("[apex:worker] " + str(m), file=sys.stderr, flush=True)

def fmt_ts(s):
    ms = int((s % 1) * 1000)
    s_int = int(s)
    return "%02d:%02d:%02d,%03d" % (s_int // 3600, (s_int % 3600) // 60, s_int % 60, ms)

torch.set_num_threads(8)
_log("Loading Whisper-Hindi2Hinglish-Apex model (CPU)...")
model = whisper.load_model("Oriserve/Whisper-Hindi2Hinglish-Apex", device="cpu")
_log("Model loaded. Transcribing...")

audio = whisper.load_audio(audio_path)
duration = len(audio) / 16000.0
_log("Audio loaded: %.1fs" % duration)

start = time.time()
result = whisper.transcribe(
    model, audio,
    language="hi",
    condition_on_previous_text=False,
    remove_empty_words=True,
)
elapsed = time.time() - start
_log("Transcription done in %.0fs (%.1f min, %.1fx real-time)" % (elapsed, elapsed/60, elapsed/duration if duration > 0 else 0))

# Extract full text
text = " ".join(seg["text"].strip() for seg in result["segments"])
with open(text_out, "w", encoding="utf-8") as f:
    f.write(text + "\\n")

print(text[:200])
print(str(len(text.split())))

# Extract word-level timestamps
all_words = []
for seg in result["segments"]:
    if "words" in seg and seg["words"]:
        all_words.extend(seg["words"])

_log("Word-level timestamps: %d" % len(all_words))

with open(word_srt_out, "w", encoding="utf-8") as f:
    for i, w in enumerate(all_words, 1):
        f.write("%d\\n%s --> %s\\n%s\\n\\n" % (
            i, fmt_ts(w["start"]), fmt_ts(w["end"]), w["text"].strip()
        ))

# Generate phrase-level SRT (group words into ~3-5s phrases)
def group_words(words, max_words=12, max_chars=64, max_gap=0.6):
    groups = []
    cur_words = []
    cur_start = None
    cur_end = None
    for w in words:
        t = w["text"].strip()
        if not t:
            continue
        if cur_start is None:
            cur_start = w["start"]
            cur_end = w["end"]
            cur_words = [t]
            continue
        gap = w["start"] - (cur_end or w["start"])
        combined = " ".join(cur_words)
        next_len = len(combined) + 1 + len(t)
        if gap > max_gap or len(cur_words) >= max_words or next_len > max_chars:
            groups.append((" ".join(cur_words), cur_start, cur_end))
            cur_start = w["start"]
            cur_end = w["end"]
            cur_words = [t]
        else:
            cur_words.append(t)
            cur_end = w["end"]
    if cur_words:
        groups.append((" ".join(cur_words), cur_start, cur_end))
    return groups

groups = group_words(all_words)
with open(phrase_srt_out, "w", encoding="utf-8") as f:
    for i, (text, start, end) in enumerate(groups, 1):
        f.write("%d\\n%s --> %s\\n%s\\n\\n" % (i, fmt_ts(start), fmt_ts(end), text))

_log("Phrase SRT: %d phrases, avg %.1fs" % (
    len(groups),
    sum(e - s for _, s, e in groups) / max(1, len(groups))
))

del model
import gc; gc.collect()
_log("Done.")
"""
    )

    _log("Running Apex transcription (CPU)...")
    result = subprocess.run(
        [whisper_python, "-c", script],
        capture_output=True,
        text=True,
        timeout=1800,  # 30min timeout for long videos
    )

    if result.returncode != 0:
        _log("Apex transcription failed: " + result.stderr[-500:])
        return {"error": result.stderr[-500:]}

    lines = result.stdout.strip().split("\n")
    text_preview = lines[0] if lines else ""
    word_count = int(lines[1]) if len(lines) > 1 and lines[1].isdigit() else 0

    _log("Apex transcription complete: %d words" % word_count)
    return {
        "text_preview": text_preview,
        "word_count": word_count,
        "word_srt_path": word_srt_out,
        "phrase_srt_path": phrase_srt_out,
        "text_path": text_out,
    }


def run(
    video: str,
    out_dir: str = None,
) -> dict:
    """Main Apex transcription pipeline."""
    out_dir = out_dir or str(Path(video).parent)
    Path(out_dir).mkdir(parents=True, exist_ok=True)

    stem = Path(video).stem
    wav_path = str(Path(out_dir) / (stem + ".apex.wav"))

    # Step 1: Extract audio
    if not extract_audio(video, wav_path):
        return {"error": "Audio extraction failed", "status": "error"}

    # Step 2: Transcribe with Apex
    whisper_python = _find_whisper_python()
    result = run_apex_transcription(whisper_python, wav_path, out_dir, stem)

    if "error" in result:
        return {"error": result["error"], "status": "error"}

    result["status"] = "transcribed"
    result["language"] = "Hinglish"
    result["output_srt_path"] = result["phrase_srt_path"]
    return result


def main():
    parser = argparse.ArgumentParser(
        description="Transcribe video using Whisper-Hindi2Hinglish-Apex"
    )
    sub = parser.add_subparsers(dest="cmd", required=True)

    p_run = sub.add_parser("run", help="Transcribe video/audio")
    p_run.add_argument("--video", required=True, help="Path to video/audio file")
    p_run.add_argument("--out-dir", default=None, help="Output directory")
    # Compat flags (ignored)
    p_run.add_argument("--language", default=None, help="Ignored")
    p_run.add_argument("--model", default="apex", help="Ignored")
    p_run.add_argument("--device", default="cpu", help="Ignored (always CPU)")
    p_run.add_argument("--use-whisper", action="store_true", help="Ignored")
    p_run.add_argument("--whisper-mode", default="cli", help="Ignored")
    p_run.add_argument("--cmd-template", default=None, help="Ignored")

    args = parser.parse_args()

    if args.cmd == "run":
        result = run(
            video=args.video,
            out_dir=args.out_dir,
        )

        if result.get("error"):
            _log("ERROR: " + str(result["error"]))
            sys.exit(1)

        print(result.get("output_srt_path", ""))
        sys.stdout.flush()


if __name__ == "__main__":
    main()
