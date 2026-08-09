#!/usr/bin/env python3
"""Regression tests for tts_common.py.

Run:  python3 mcp/scripts/test_tts_common.py
Requires: numpy, ffmpeg on PATH (a hard dependency of the render pipeline).

Locks the two invariants that regressed repeatedly during development:
  1. normalize_lufs two-pass loudnorm: JSON parse (stdout vs stderr, `-v info`
     requirement, trailing muxing text on stderr), near-silent guard, and the
     actual loudness boost.
  2. crossfade_concat equal-power seam blending and length accounting.
"""

import math
import os
import sys
import tempfile
import wave

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import tts_common  # noqa: E402

SR = 16000


def _write_wav(path, samples, sr=SR):
    data = (np.clip(samples, -1.0, 1.0) * 32767).astype(np.int16)
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(sr)
        w.writeframes(data.tobytes())


def _read_wav(path):
    with wave.open(path, "rb") as w:
        assert w.getframerate() == SR
        return np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).astype(
            np.float64
        ) / 32767.0


def _rms_db(samples):
    r = np.sqrt(np.mean(np.asarray(samples, dtype=np.float64) ** 2))
    return 20 * math.log10(r + 1e-12)


def test_normalize_boosts_quiet_audio():
    """A quiet-but-audible clip must be lifted toward the -16 LUFS target."""
    t = np.arange(SR * 3) / SR
    quiet = (0.02 * np.sin(2 * np.pi * 220 * t)).astype(np.float32)
    path = os.path.join(tempfile.mkdtemp(), "quiet.wav")
    _write_wav(path, quiet)
    assert tts_common.normalize_lufs(path) is True
    out = _read_wav(path)
    assert _rms_db(out) > _rms_db(quiet) + 8, "did not boost quiet audio enough"
    assert np.max(np.abs(out)) <= 1.0, "clipped"


def test_normalize_skips_near_silence():
    """A >25 dB-below-target clip is near-silence and must NOT be boosted."""
    t = np.arange(SR * 3) / SR
    sil = (0.0001 * np.sin(2 * np.pi * 220 * t)).astype(np.float32)
    path = os.path.join(tempfile.mkdtemp(), "sil.wav")
    _write_wav(path, sil)
    before = _read_wav(path)
    assert tts_common.normalize_lufs(path) is False
    assert np.array_equal(_read_wav(path), before), "near-silence was modified"


def test_crossfade_seam_blends_step():
    """A 0.7->0 step must be blended through the seam, not hard-cut."""
    n = SR // 4
    a = np.full(n, 0.7, dtype=np.float32)
    b = np.zeros(n, dtype=np.float32)
    fade_n = int(SR * 25 / 1000)
    out = tts_common.crossfade_concat([a, b], SR, fade_ms=25)
    assert len(out) == 2 * n - fade_n, f"len {len(out)} != {2 * n - fade_n}"
    v_start = out[n - fade_n - 1]
    v_mid = out[n - fade_n // 2]
    v_after = out[n + fade_n // 2]
    assert v_start > 0.68, "fade-out not at full level before seam"
    assert 0.3 < v_mid < 0.6, f"midpoint {v_mid} not in equal-power band"
    assert v_after < 0.1, "fade-in did not complete"


def test_crossfade_single_and_empty():
    """Single part returns unchanged; empty list returns None."""
    single = np.zeros(10, dtype=np.float32)
    assert tts_common.crossfade_concat([single], SR) is single
    assert tts_common.crossfade_concat([], SR) is None


def test_crossfade_hard_join_for_tiny_parts():
    """Parts too short to fade must still concatenate (no crash)."""
    a = np.ones(4, dtype=np.float32) * 0.5
    b = np.ones(4, dtype=np.float32) * 0.5
    out = tts_common.crossfade_concat([a, b], SR, fade_ms=25)
    assert len(out) == 8
    assert np.max(np.abs(out)) <= 1.0


def main():
    tests = [v for k, v in sorted(globals().items()) if k.startswith("test_")]
    for fn in tests:
        fn()
        print(f"PASS {fn.__name__}")
    print(f"ALL {len(tests)} tts_common TESTS PASSED")
    return 0


if __name__ == "__main__":
    sys.exit(main())
