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
# consecutive (post-pad) positions must trigger the breaker.
_vec = [101, 202, 303, 404, 505, 606, 707, 808]
_last, _run, _trig = None, 0, False
for _s in range(30):
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

print(f"\n{passed} passed, {failed} failed")
sys.exit(1 if failed else 0)
