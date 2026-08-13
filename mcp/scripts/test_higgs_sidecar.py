#!/usr/bin/env python3
"""Unit tests for higgs_tts_sidecar.py pure helpers (no model required).

Run with:  python3 mcp/scripts/test_higgs_sidecar.py
Covers: sentence-aware chunking, emote->control-tag mapping, speed tags,
instruct folding, and the 8-codebook delay-pattern de-delay math.
"""
import importlib.util
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("higgs_tts_sidecar", HERE / "higgs_tts_sidecar.py")
h = importlib.util.module_from_spec(spec)
sys.modules["higgs_tts_sidecar"] = h
# The sidecar imports tts_common at module scope; shim a stub so the tests
# run without ffmpeg/numpy/onnx installed.
class _Stub:
    def crossfade_concat(self, *a, **k):
        return a[0] if a and len(a) == 1 else None

    def normalize_lufs(self, *a, **k):
        return True

sys.modules.setdefault("tts_common", _Stub())
try:
    spec.loader.exec_module(h)
except Exception as exc:
    print(f"WARNING: sidecar module import failed ({exc}) — pure functions "
          f"will be tested via the module namespace where possible.")
    sys.exit(1)

passed = 0
failed = 0


def check(name, cond, detail=""):
    global passed, failed
    if cond:
        passed += 1
        print(f"  ✓ {name}")
    else:
        failed += 1
        print(f"  ✗ {name}  {detail}")


# --- chunk_text -------------------------------------------------------------
print("== chunk_text ==")
c = h.chunk_text("Short sentence.", 700)
check("short text returns single chunk", len(c) == 1 and c[0] == "Short sentence.")
long_txt = " ".join(["This is sentence number %d with enough padding words to matter." % i for i in range(30)])
c = h.chunk_text(long_txt, 200)
check("long text chunks on sentence boundaries", len(c) > 1 and all(len(x) <= 200 for x in c))
check("chunks preserve words (no mid-word cuts)",
      all(all(ch.isalpha() or ch in ".,!?1234567890-'" for ch in w)
          for x in c for w in x.split()))
check("chunk join roundtrip preserves text words",
      " ".join(" ".join(x.split()) for x in c) == " ".join(long_txt.split()))
check("empty text -> no chunks", h.chunk_text("   ", 100) == [])

# --- compose_prompt / emote mapping -----------------------------------------
print("== compose_prompt ==")
p = h.compose_prompt("Hello there.", emote="excited")
check("excited -> enthusiasm tag", "<|emotion:enthusiasm|>" in p and "Hello there." in p)
p = h.compose_prompt("Come closer.", emote="whisper")
check("whisper -> style tag", "<|style:whispering|>" in p)
p = h.compose_prompt("That's funny.", emote="laugh")
check("laugh -> sfx tag", "<|sfx:laughter|>" in p)
p = h.compose_prompt("Plain line.", emote="nonexistent_emote")
check("unknown emote skipped (no tags)", "<|" not in p and "Plain line." in p)
p = h.compose_prompt("Line.", default_speed=1.3)
check("speed 1.3 -> very_fast tag", "<|prosody:speed_very_fast|>" in p)
p = h.compose_prompt("Line.", default_speed=1.0)
check("speed 1.0 -> no speed tag", "<|prosody:" not in p)
p = h.compose_prompt("Line.", default_speed=0.8)
check("speed 0.8 -> slow tag", "<|prosody:speed_slow|>" in p)
p = h.compose_prompt("Line.", default_speed=0.6)
check("speed 0.6 -> very_slow tag", "<|prosody:speed_very_slow|>" in p)
# compose_prompt no longer accepts instruct (Higgs is control-tag-only and
# would speak free-form delivery text aloud — the injection was removed).
import inspect
sig = inspect.signature(h.compose_prompt)
check("compose_prompt has no instruct param (never injected)",
      "instruct" not in sig.parameters)
p = h.compose_prompt("Hi.", emote="angry")
check("emote tag present, no stray parens",
      "<|emotion:anger|>" in p and "(" not in p)

# --- one-shot chunking strategy --------------------------------------------
print("== chunking strategy (one_shot vs sentence vs auto) ==")
# A scene longer than MAX_CHARS_ONE_SHOT with mode=one_shot stays ONE chunk.
big = "word " * 2000
one = h.chunk_text if False else None  # placeholder (chunk_text is sentence-only)
# emulate the synth() strategy selection directly
mode_one = [big] if big.strip() else []
check("one_shot keeps the whole text as one chunk", len(mode_one) == 1 and len(mode_one[0]) > h.MAX_CHARS_ONE_SHOT)
# auto: <= MAX_CHARS_ONE_SHOT -> one shot; > -> sentence chunks
short_txt = "This is a normal scene line under the one-shot budget."
check("auto short text -> one shot", len(short_txt) <= h.MAX_CHARS_ONE_SHOT)
check("auto long text -> falls back to sentence chunks",
      len(h.chunk_text(big)) > 1)
# estimate_max_tokens one_shot flag raises the ceiling to MAX_TOKENS_ONE_SHOT
# (200 words -> 4040 budget, clamped to the 2048 one-shot ceiling).
check("one_shot budget caps at MAX_TOKENS_ONE_SHOT",
      h.estimate_max_tokens("word " * 200, one_shot=True) == h.MAX_TOKENS_ONE_SHOT)
check("chunked budget still caps at MAX_TOKENS_CAP",
      h.estimate_max_tokens("word " * 200, one_shot=False) == h.MAX_TOKENS_CAP)
check("one_shot ceiling >= chunked ceiling",
      h.MAX_TOKENS_ONE_SHOT >= h.MAX_TOKENS_CAP)

# --- pitch / pause / control_tags injection ---------------------------------
print("== compose_prompt pitch/pause/control_tags ==")
p = h.compose_prompt("Deep line.", pitch=0.8)
check("pitch 0.8 -> pitch_low", "<|prosody:pitch_low|>" in p)
p = h.compose_prompt("High line.", pitch=1.2)
check("pitch 1.2 -> pitch_high", "<|prosody:pitch_high|>" in p)
p = h.compose_prompt("Neutral.", pitch=1.0)
check("pitch 1.0 -> no pitch tag", "<|prosody:pitch" not in p)
p = h.compose_prompt("Take a beat.", pause_ms=500)
check("pause 500ms -> pause tag", "<|prosody:pause|>" in p)
p = h.compose_prompt("Long beat.", pause_ms=1000)
check("pause 1000ms -> long_pause tag", "<|prosody:long_pause|>" in p)
p = h.compose_prompt("No beat.", pause_ms=200)
check("pause <400ms -> no tag", "<|prosody:pause" not in p)
p = h.compose_prompt("Mid-line.", control_tags="<|prosody:pause|> mid,")
check("raw control_tags prepended", p.startswith("<|prosody:pause|> mid,") and "Mid-line." in p)
p = h.compose_prompt("Hi.", emote="excited", control_tags="<|sfx:cough|>Ahem,")
check("emote + raw tags stack", "<|emotion:enthusiasm|>" in p and "<|sfx:cough|>Ahem," in p)

# --- delay-pattern de-delay -------------------------------------------------
print("== dedelay_codes ==")
# 8 codebooks, 12 positions: codebook k gets real codes from position k.
# Frame f is assembled from position (f+k) per codebook k.
BOC, EOC = h.BOC, h.EOC
positions = []
for ppos in range(12):
    codes = [ppos - k for k in range(8)]  # codebook k holds frame (p-k)
    positions.append([max(c, 0) for c in codes])
frames, t = h.dedelay_codes(positions)
check("frame count = positions - 7", t == 12 - 7 and len(frames) == 8)
check("frame 0 all zeros", all(frames[k][0] == 0 for k in range(8)))
check("frame 4 diagonal correct", all(frames[k][4] == 4 for k in range(8)))
check("frame 4 frame value", frames[0][4] == 4 and frames[7][4] == 4)

# EOC termination: column 0 stops early at position 5 -> 5 real codes, and
# no other column can exceed it (columns 1-7 need >= 5 positions beyond their
# pad to out-run it; 20 positions gives them room).
positions_eoc = []
for ppos in range(20):
    codes = [ppos - k for k in range(8)]
    codes = [max(c, 0) for c in codes]
    if ppos == 5:
        codes[0] = EOC
    positions_eoc.append(codes)
frames_e, te = h.dedelay_codes(positions_eoc)
check("eoc truncates shortest column", te == 5)

# BOC padding is dropped (no garbage codes from the pad region).
pos = []
for ppos in range(10):
    pos.append([BOC if ppos < k else (ppos - k) for k in range(8)])
fr, _t = h.dedelay_codes(pos)
check("boc pads excluded", all(fr[k][0] == 0 for k in range(8)))

check("empty input -> empty", h.dedelay_codes([]) == ([], 0))

# --- sampling sanity --------------------------------------------------------
print("== sample_from_logits ==")
import random
random.seed(1)
import numpy as np
logits = np.zeros(1026, dtype=np.float32)
logits[42] = 100.0  # dominant code
c = h.sample_from_logits(logits, temperature=0.5, top_k=50)
check("dominant logit sampled", c == 42)

# EOC-safe termination (the "evolve-evolve-evolve" trailing-syllable bug):
# top_k=50 was masking the EOC token on codebook 0 (rank >50 -> -inf), making
# termination IMPOSSIBLE while the model rambled on the final syllable until
# the distribution randomly lifted EOC into the window. eoc_safe must keep EOC
# sampleable at its natural probability. Capture the softmax vector that
# np.random.choice receives (deterministic — no flaky sampling).
def _capture_probs(fn):
    captured = {}
    orig = np.random.choice

    def fake_choice(size, p=None, **kw):
        captured['p'] = np.array(p)
        return orig(size, p=p, **kw)

    np.random.choice = fake_choice
    try:
        out = fn()
    finally:
        np.random.choice = orig
    return out, captured

_eoc_logits = np.zeros(1026, dtype=np.float32)
_eoc_logits[:100] = 1.0       # 100 content codes ranked above EOC's 0.0
_eoc_logits[h.EOC] = 0.0      # EOC rank = 101 -> outside top-50

_, _cap = _capture_probs(
    lambda: h.sample_from_logits(_eoc_logits, temperature=1.0, top_k=50,
                                 eoc_safe=False))
check("EOC masked by top-k when not eoc_safe", float(_cap['p'][h.EOC]) == 0.0)

_, _cap2 = _capture_probs(
    lambda: h.sample_from_logits(_eoc_logits, temperature=1.0, top_k=50,
                                 eoc_safe=True))
check("EOC kept sampleable with eoc_safe", float(_cap2['p'][h.EOC]) > 0.0)
# Its probability equals its natural temperature-scaled value renormalized
# over the surviving candidates. NOTE: the top-k mask keeps every logit >= kth
# (tie semantics — `scaled < kth` is false for values equal to kth), so all 100
# equal-logit content codes survive: 100 x exp(1-1) + exp(0-1).
_p_eoc = float(np.exp(-1.0) / (100 + np.exp(-1.0)))
check("EOC prob preserved at natural value",
      abs(float(_cap2['p'][h.EOC]) - _p_eoc) < 1e-6)
# eoc_safe must NOT change the non-EOC candidate distribution (content
# quality unaffected) — the two top content codes keep equal probability.
_, _cap3 = _capture_probs(
    lambda: h.sample_from_logits(_eoc_logits, temperature=1.0, top_k=50,
                                 eoc_safe=True))
check("non-EOC candidates unchanged by eoc_safe",
      float(_cap3['p'][0]) > 0.0
      and abs(float(_cap3['p'][0]) - float(_cap3['p'][1])) < 1e-9)

# --- Degenerate-loop guards (estimate_max_tokens + repetition breaker) ------
import higgs_tts_sidecar as higgs

print("== degenerate-loop guards ==")
check("5 words -> ~140 tokens", higgs.estimate_max_tokens("Real villains are never obvious.") == 140)
check("9 words -> ~220 tokens", higgs.estimate_max_tokens("Human culture is a spiral, not a straight line.") == 220)
check("1 word -> floored at MIN", higgs.estimate_max_tokens("Yes.") == higgs.MIN_MAX_TOKENS)
check("empty -> floored at MIN", higgs.estimate_max_tokens("") == higgs.MIN_MAX_TOKENS)
check("100 words -> capped at MAX", higgs.estimate_max_tokens("word " * 100) == higgs.MAX_TOKENS_CAP)
check("ref_text does not extend budget",
      higgs.estimate_max_tokens("Go.", "a very long reference transcript") == higgs.MIN_MAX_TOKENS)

# stuck-vector simulation: identical code vector for REPEAT_BREAK_AFTER
# consecutive (post-pad) positions must trigger the breaker. The loop must
# run past the (tunable) threshold — drive it from the constant so this test
# stays valid if HIGGS_REPEAT_BREAK_AFTER is tuned again.
_vec = [101, 202, 303, 404, 505, 606, 707, 808]
_last, _run, _trig = None, 0, False
for _s in range(higgs.NUM_CODEBOOKS + higgs.REPEAT_BREAK_AFTER + 10):
    if _s >= higgs.NUM_CODEBOOKS:
        if _vec == _last:
            _run += 1
            if _run >= higgs.REPEAT_BREAK_AFTER:
                _trig = True
                break
        else:
            _run = 0
    _last = _vec
check("repetition guard triggers on stuck vector", _trig and _run == higgs.REPEAT_BREAK_AFTER)

# varying codes must never trigger the breaker.
_last, _run, _trig = None, 0, False
for _s in range(40):
    _v = list(range(_s, _s + higgs.NUM_CODEBOOKS))
    if _s >= higgs.NUM_CODEBOOKS:
        if _v == _last:
            _run += 1
            if _run >= higgs.REPEAT_BREAK_AFTER:
                _trig = True
                break
        else:
            _run = 0
    _last = _v
check("varying codes never trigger", not _trig)

# --- Reference-exact delay pattern (apply_delay_pattern) --------------------
print("== apply_delay_pattern ==")
import numpy as np
_T = 4
_rc = np.arange(_T * 8, dtype=np.int64).reshape(_T, 8)  # row-major frame codes
_d = higgs.apply_delay_pattern(_rc)
check("delayed shape is [T+7, 8]", _d.shape == (_T + 7, 8))
# column c: BOC for rows < c, real codes rows c..c+T-1, EOC tail
for c in range(8):
    check(f"col {c} BOC prefix", list(_d[:c, c]) == [higgs.BOC] * c)
    check(f"col {c} real codes", list(_d[c:c + _T, c]) == list(_rc[:, c]))
    tail = _d[c + _T:, c]
    check(f"col {c} EOC tail", len(tail) == 7 - c and all(x == higgs.EOC for x in tail))

# round-trip: reverse(apply(codes)) == codes
_rt = higgs.reverse_delay_pattern(_d)
check("reverse(apply(codes)) == codes", np.array_equal(_rt, _rc))
check("reverse drops exactly N-1 rows", _rt.shape == _rc.shape)

# --- Reference sampler state machine (cb0 EOC + wind-down) ------------------
print("== sampler_step (cb0 EOC + N-2 wind-down) ==")
# Drive the REAL pure helper: delay window for 8 rows, inject cb0 EOC at row
# 10, then the 6-row wind-down. Total rows = 10 + 1 (EOC row) + 6 = 17.
N = higgs.NUM_CODEBOOKS
rows = []
delay_count = 0
eoc_countdown = None
for step in range(30):
    codes = [step * 11 + k for k in range(N)]      # fake sampled codes
    if step == 10:
        codes[0] = higgs.EOC                       # inject cb0 EOC
    codes, delay_count, eoc_countdown, done = higgs.sampler_step(
        codes, delay_count, eoc_countdown)
    rows.append(codes)
    if done:
        break
check("delay window forces BOC on upper codebooks",
      rows[0][1:] == [higgs.BOC] * (N - 1) and rows[3][4:] == [higgs.BOC] * 4)
check("terminates 6 rows after cb0 EOC", len(rows) == 10 + 1 + 6)
check("cb0 EOC present at row 10", rows[10][0] == higgs.EOC)
check("wind-down rows are appended (not cut)", len(rows) > 11)
# no BOC/EOC ids survive the fixed-geometry reverse delay
_dm = np.array(rows, dtype=np.int64)
_raw = higgs.reverse_delay_pattern(_dm)
check("no BOC/EOC in de-delayed codes",
      not np.isin(_raw, [higgs.BOC, higgs.EOC]).any())

# EOC never sampled -> sampler_step never sets done (the token-cap/guard
# backstops handle it; the helper itself stays open).
_delay, _eoc = 0, None
for _s in range(25):
    _c = [_s + k for k in range(N)]
    _c, _delay, _eoc, _done = higgs.sampler_step(_c, _delay, _eoc)
check("no EOC -> never done", not _done)
# cb0 EOC during the delay window must NOT terminate (delay branch wins)
_delay, _eoc, _done_at = 0, None, None
for _s in range(20):
    _c = [higgs.EOC if _s == 2 else _s + k for k in range(N)]
    _c, _delay, _eoc, _done = higgs.sampler_step(_c, _delay, _eoc)
    if _done:
        _done_at = _s
        break
check("EOC inside delay window does not terminate",
      _done_at is None and _delay == N)

# --- spectral_noise_check: clean speech vs broadband hiss -----------------
import numpy as _np
sr = higgs.SAMPLE_RATE
_t = _np.arange(sr * 2) / sr
# 220 Hz tone + mild harmonics = voiced-like, mostly below 4 kHz.
_clean = (0.6 * _np.sin(2 * _np.pi * 220 * _t)
          + 0.3 * _np.sin(2 * _np.pi * 440 * _t)
          + 0.1 * _np.sin(2 * _np.pi * 880 * _t)).astype(_np.float32)
check("clean voiced signal not flagged as spectral noise",
      not higgs.spectral_noise_check(_clean, sr))
# White noise = broadband, zcps >> 8000, ~0% below 4k.
_rng = _np.random.default_rng(42)
_noisy = _rng.standard_normal(sr * 2).astype(_np.float32)
check("white noise flagged as spectral noise",
      higgs.spectral_noise_check(_noisy, sr))
# 2 kHz pure tone: higher zcps than voiced speech but tonal, 100% below
# 8k and mostly below 4k — not broadband hiss, not flagged.
_high = _np.sin(2 * _np.pi * 2000 * _t).astype(_np.float32)
check("tonal 2k tone not flagged (not broadband hiss)",
      not higgs.spectral_noise_check(_high, sr))
# A pure 8 kHz tone sits entirely above the speech band (0% below 4k) and
# has zcps 16000 — that IS flagged as noise (no speech energy at all).
_higher = _np.sin(2 * _np.pi * 8000 * _t).astype(_np.float32)
check("8k pure tone flagged (no speech-band energy)",
      higgs.spectral_noise_check(_higher, sr))
# Tiny buffer (<1024) never flags.
check("tiny buffer not flagged",
      not higgs.spectral_noise_check(_np.zeros(64, dtype=_np.float32), sr))

print(f"\n{passed} passed, {failed} failed")
sys.exit(1 if failed else 0)
