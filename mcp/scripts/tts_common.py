#!/usr/bin/env python3
"""
Shared TTS post-processing utilities for the audio8 / gepard / voicedesign sidecars.

Production-grade audio requires two invariants that raw model output does not
guarantee:

1. **Uniform per-scene loudness.** VoiceDesign output amplitude varies wildly
   with the instruct (emotion takes came out 4-14 dB quieter than the base
   voice; cloned scenes inherited the quiet, leaving some lines effectively
   muted under the music bed). The final render only loudness-normalizes the
   WHOLE mix, which cannot fix relative variance between scenes. Each sidecar
   therefore normalizes its output to a target integrated LUFS so every scene
   sits at the same loudness.

2. **Seam-free chunk concatenation.** Long texts are synthesized in sentence
   chunks; a hard `np.concatenate` leaves a perceptible dip/click where one
   chunk ends (model tail-off) and the next begins. A short equal-power
   crossfade at each seam removes the mute dips.

Loudness normalization uses a two-pass ffmpeg `loudnorm` (measure, then apply
with `linear=true`) — ffmpeg is a hard dependency of the render pipeline, so
it is always present. This mirrors the Rust `normalize_lufs` behaviour (-16
LUFS default) so the per-scene normalization and the final-mix loudnorm agree.
"""

import json
import os
import subprocess
import sys
from pathlib import Path

# Target integrated loudness for every synthesized scene (matches the render
# pipeline's `loudnorm=I=-16` final-mix target; per-scene normalization ensures
# scenes are uniform BEFORE they reach the mix).
TARGET_LUFS = float(os.environ.get("OPENSCRIPT_TTS_TARGET_LUFS", "-16.0"))
TARGET_TP = float(os.environ.get("OPENSCRIPT_TTS_TARGET_TP", "-2.5"))
TARGET_LRA = float(os.environ.get("OPENSCRIPT_TTS_TARGET_LRA", "11.0"))

# Equal-power crossfade applied at each chunk seam (ms).
CHUNK_FADE_MS = int(os.environ.get("OPENSCRIPT_TTS_CHUNK_FADE_MS", "40"))


def _log(msg: str, prefix: str = "tts_common"):
    print(f"[{prefix}] {msg}", file=sys.stderr, flush=True)


def _input_sample_rate(path: str) -> int | None:
    """Read the input WAV's sample rate (loudnorm resamples to 192 kHz
    internally, and the apply pass must force `-ar` back to the original or
    every normalized scene would silently become 192 kHz — 4-12x the size and
    a sample-rate mismatch against the mix's other tracks)."""
    r = subprocess.run(
        [
            "ffprobe", "-v", "error", "-select_streams", "a:0",
            "-show_entries", "stream=sample_rate", "-of", "csv=p=0",
            str(path),
        ],
        capture_output=True,
        text=True,
        shell=False,
    )
    if r.returncode == 0 and r.stdout.strip().isdigit():
        return int(r.stdout.strip())
    return None


def _measure_loudness(path: str) -> dict | None:
    """Run the loudnorm measurement pass; return the measured parameters."""
    cmd = [
        "ffmpeg", "-hide_banner", "-y", "-v", "info", "-nostats", "-i", str(path),
        "-af", "loudnorm=print_format=json", "-f", "null", "-",
    ]
    r = subprocess.run(cmd, capture_output=True, text=True, shell=False)
    if r.returncode != 0:
        _log(f"loudnorm measurement failed for {path}: {r.stderr[-400:]}")
        return None
    # loudnorm's print_format=json block lands on stdout on modern ffmpeg
    # (>=4.4) and on stderr on older builds — check both. NOTE: it is printed
    # at info level, so `-v error` would suppress it entirely. On stderr the
    # block is followed by the muxing summary, so raw_decode the single JSON
    # object and ignore trailing text. `-hide_banner` keeps earlier info lines
    # brace-free so `find("{")` cannot mis-hit a log line before the block.
    decoder = json.JSONDecoder()
    for stream in (r.stdout, r.stderr):
        try:
            # Anchor on the loudnorm JSON's unique "input_i" key rather than
            # the first brace — robust to any info line containing "{".
            anchor = stream.find('"input_i"')
            if anchor < 0:
                continue
            start = stream.rfind("{", 0, anchor)
            if start < 0:
                continue
            data, _ = decoder.raw_decode(stream[start:])
            needed = ["input_i", "input_tp", "input_lra", "input_thresh"]
            if all(k in data for k in needed):
                return {k: float(data[k]) for k in needed}
        except (ValueError, json.JSONDecodeError) as exc:
            _log(f"could not parse loudnorm measurement for {path}: {exc}")
    return None


def normalize_lufs(
    path: str,
    target_lufs: float | None = None,
    target_tp: float | None = None,
    target_lra: float | None = None,
) -> bool:
    """Normalize a WAV in place to the target integrated loudness (two-pass).

    Returns True when normalized, False when skipped (near-silent input,
    missing file, or ffmpeg failure — never raises, so synthesis output is
    still written even if normalization is unavailable).
    """
    target_lufs = TARGET_LUFS if target_lufs is None else target_lufs
    target_tp = TARGET_TP if target_tp is None else target_tp
    target_lra = TARGET_LRA if target_lra is None else target_lra

    p = Path(path)
    if not p.exists() or p.stat().st_size < 128:
        return False

    measured = _measure_loudness(str(p))
    if measured is None:
        # Near-silent or unreadable: leave as-is rather than amplifying noise.
        return False

    # Guard: skip only TRUE digital silence, never quiet-but-real speech.
    # A relative guard (e.g. >25 dB below target, i.e. < -41 LUFS) would skip
    # whisper-quiet emotion-take clones at -42 LUFS — exactly the scenes that
    # NEED lifting (they were the "second speaker inaudible" bug). Absolute
    # -60 LUFS catches digital near-silence/dither without touching speech.
    if measured["input_i"] < -60.0:
        _log(f"{Path(path).name} input {measured['input_i']:.1f} LUFS is "
             f"true digital silence — skipping")
        return False

    tmp = p.with_suffix(p.suffix + ".loudnorm.tmp.wav")
    # Force the ORIGINAL sample rate back onto the output — loudnorm resamples
    # to 192 kHz internally (measured above), and without `-ar` every scene
    # would silently become 192 kHz (12x size, sample-rate mismatch in mix).
    in_sr = _input_sample_rate(str(p))
    cmd = [
        "ffmpeg", "-y", "-v", "error", "-i", str(p),
        "-af",
        (
            f"loudnorm=I={target_lufs}:TP={target_tp}:LRA={target_lra}"
            f":measured_I={measured['input_i']}"
            f":measured_TP={measured['input_tp']}"
            f":measured_LRA={measured['input_lra']}"
            f":measured_thresh={measured['input_thresh']}"
            ":linear=true"
        ),
    ]
    if in_sr:
        cmd += ["-ar", str(in_sr)]
    cmd += ["-c:a", "pcm_s16le", str(tmp)]
    r = subprocess.run(cmd, capture_output=True, text=True, shell=False)
    if r.returncode == 0 and tmp.exists():
        os.replace(tmp, p)
        return True
    if tmp.exists():
        tmp.unlink(missing_ok=True)
    _log(f"loudnorm apply failed for {path}: {r.stderr[-300:]}")
    return False


def crossfade_concat(parts: list, sample_rate: int, fade_ms: int | None = None) -> object:
    """Concatenate numpy arrays with an equal-power crossfade at each seam.

    Hard concatenation leaves audible dips where one synthesized chunk ends
    (model tail-off) and the next begins; the equal-power crossfade (cos/sin
    ramps, constant perceived power) blends the boundary smoothly.

    Accepts an empty list (returns None) and a single part (returns it
    unchanged). `fade_ms=0` disables the fade (plain concatenate).
    """
    if not parts:
        return None
    if len(parts) == 1:
        return parts[0]

    import numpy as np  # noqa: PLC0415

    fade_ms = CHUNK_FADE_MS if fade_ms is None else fade_ms
    fade_n = max(0, int(sample_rate * fade_ms / 1000.0))
    out = np.asarray(parts[0], dtype=np.float32)
    for part in parts[1:]:
        part = np.asarray(part, dtype=np.float32)
        n = min(fade_n, len(out) // 2, len(part) // 2)
        if n < 8:  # too short for a meaningful fade — hard join
            out = np.concatenate([out, part])
            continue
        t = np.linspace(0.0, 1.0, n, dtype=np.float32)
        fade_out = np.cos(0.5 * np.pi * t)   # 1 -> 0
        fade_in = np.sin(0.5 * np.pi * t)    # 0 -> 1
        seam = out[-n:] * fade_out + part[:n] * fade_in
        out = np.concatenate([out[:-n], seam, part[n:]])
    return out
