#!/usr/bin/env python3
"""Higgs Audio v3 TTS sidecar — expressive 4B zero-shot TTS via ONNX Runtime GenAI.

Drives the self-contained ONNX export at
`onnx-community/higgs-audio-v3-tts-4b` (branch `cuda_int4`, ~3.6 GB —
provisioned by `scripts/setup_higgs.sh`). Higgs Audio v3 is a 4B autoregressive
conversational TTS: **100+ languages, zero-shot voice cloning, and inline
control tokens** for emotion / prosody / style / sound effects (43 tags, see
the model card's PROMPTING.md). 24 kHz, 25 fps, 8-codebook Higgs v2 codec.

The export is a self-contained 6-sub-model pipeline (manifest.json):

    text_embed        input_ids[B,L] int64        -> inputs_embeds[B,L,2560]
    audio_encoder     input_values[B,1,T] f32@24k -> audio_codes[B,8,frames]   (clone ref)
    audio_embed       codes[B,L,8] int64          -> audio_embeds[B,L,2560]    (fused)
    llm_decoder       inputs_embeds + past_kv     -> hidden_states + present   (OGA, int4)
    audio_heads       hidden_states[B,L,2560]     -> audio_logits[B,L,8,1026]  (fused head)
    audio_tokenizer   audio_codes[B,8,T] int64    -> waveform[B,1,L] f32@24k   (codec)

PROMPT FORMAT (CAPABILITIES.md, zero-shot):

    <|tts|> <|text|> tok(text) <|audio|>                    (no reference)
    <|tts|> <|ref_text|> tok(ref) <|ref_audio|> [codes] <|text|> tok(text) <|audio|>
                                                            (voice clone)

GENERATION: the 8 codebooks follow a **delay pattern** (codebook k is shifted
by k positions, BOC=1024 pads the start, EOC=1025 terminates each column).
The prompt's REFERENCE codes are ALSO delay-patterned before fusion
(`apply_delay_pattern` — T+7 rows, BOC prefix + EOC tail per column), exactly
as the sglang-omni reference does; feeding raw parallel ref frames is
off-distribution and yields a hissy/metallic clone. Termination follows the
reference sampler state machine: during the delay window codebooks above
`delay_count` are forced to BOC; when **codebook 0** samples EOC a wind-down
of N-2 = 6 further rows runs (letting each column's EOC land just outside its
reverse-delay window), then generation stops. The int4 `llm_decoder` is a
plain ONNX QDQ graph, so it runs under ordinary onnxruntime with a manual
KV-cache loop (the export takes `inputs_embeds` + `past_key_values.*` and
returns `hidden_states` + `present.*` — probed against ORT 1.28, fp16); audio
logits come from the fused `audio_heads` model; the generated rows are
de-delayed with fixed geometry (`reverse_delay_pattern`) and decoded to
waveform by the `audio_tokenizer`.

LONG-LIVED SERVE MODE (--serve) — mirrors gepard_tts_sidecar.py:

    → {"op":"synth","text":"Hello","voice":"ishan","output_path":"/tmp/a.wav",
       "emote":"excited","temperature":0.8,"top_k":50}
    ← {"status":"ok","duration_ms":1234,"sample_rate":24000}

    → {"op":"register","name":"ishan","audio_path":"/abs/ref.wav","text":"Exact transcript.","overwrite":true}
    ← {"status":"ok","voice":"ishan"}

    → {"op":"list"} / {"op":"health"}

    On error: {"status":"error","error":"..."}

LICENSE NOTICE: the Higgs Audio v3 weights are released under the **Boson
Higgs Audio v3 Research and Non-Commercial License** — research / non-commercial
use only; production, hosted APIs, or revenue-generating use requires a
separate commercial license from Boson.

ENV:
  HIGGS_MODEL_DIR     ONNX export dir (default <root>/mcp/assets/higgs/cuda_int4)
  HIGGS_DEVICE        auto|cuda|cpu (default auto)
  HIGGS_VOICES_DIR    registered reference voices (default <root>/mcp/assets/higgs/voices)
  HIGGS_LOG           diagnostics log (default /tmp/higgs_tts_sidecar.log)
  HIGGS_TEMPERATURE   sampling temperature (default 0.8)
  HIGGS_TOP_K         sampling top-k (default 50)
  HIGGS_MAX_NEW_TOKENS  max audio-code positions per chunk (default 1024)
  OPENSCRIPT_ROOT     repo root (defaults to script location + ../..)
"""

import argparse
import json
import os
import shutil
import sys
from pathlib import Path

# --- Shared TTS post-processing (loudness normalization + crossfade concat) --
_SCRIPT_DIR = Path(__file__).resolve().parent
if str(_SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(_SCRIPT_DIR))
from tts_common import crossfade_concat, normalize_lufs  # noqa: E402

_ROOT = Path(os.environ.get("OPENSCRIPT_ROOT", _SCRIPT_DIR.parent.parent)).resolve()
MODEL_DIR = Path(
    os.environ.get("HIGGS_MODEL_DIR", _ROOT / "mcp/assets/higgs/cuda_int4")
).resolve()
VOICES_DIR = Path(
    os.environ.get("HIGGS_VOICES_DIR", _ROOT / "mcp/assets/higgs/voices")
).resolve()

# Higgs v2 codec constants (manifest.json / CAPABILITIES.md).
SAMPLE_RATE = 24000
FRAME_SAMPLES = 960          # 25 fps @ 24 kHz (samples per audio frame)
NUM_CODEBOOKS = 8
CODEC_VOCAB = 1026           # 0..1023 real codes
BOC = 1024                   # beginning-of-code (delay-pattern padding)
EOC = 1025                   # end-of-code (column terminator)

# Sampling defaults — the sglang-omni reference recommendation for voice
# cloning: temperature 0.8, top_k 50, max_new_tokens 1024.
DEFAULT_TEMPERATURE = float(os.environ.get("HIGGS_TEMPERATURE", "0.8"))
DEFAULT_TOP_K = int(os.environ.get("HIGGS_TOP_K", "50"))
# The sglang-omni reference passes top_p through (default 1.0 = off) alongside
# top_k. None = off (pure top-k, matching the prior behavior).
DEFAULT_TOP_P = float(os.environ.get("HIGGS_TOP_P", "0")) or None
DEFAULT_MAX_NEW_TOKENS = int(os.environ.get("HIGGS_MAX_NEW_TOKENS", "1024"))
# max_new_tokens=1024 ≈ 1024 positions ≈ 1017 audio frames ≈ 40.7 s.

# Chunking strategy. Higgs is a CONVERSATIONAL model (8192-token context)
# trained on multi-sentence responses with inline control tags; a single
# autoregressive draw keeps prosody/tonality continuous across the whole
# line. Splitting into sentence chunks re-anchors the model to the reference
# at every seam and resets the delivery — the "tonality breaks per chunk"
# symptom. So the CANONICAL mode for higgs is ONE-SHOT: the entire scene
# text is one prompt, one draw.
#   "one_shot"  (canonical): whole text as a single prompt (up to a context
#               budget guard; see MAX_CHARS_ONE_SHOT).
#   "sentence"  (legacy): sentence-aligned chunking + crossfade concat.
#   "auto"      (default): one_shot while the text fits comfortably inside
#               the 8192 context (MAX_CHARS_ONE_SHOT), else sentence chunks
#               as a safety net for pathological scene sizes.
MAX_CHARS_ONE_SHOT = 500     # ~80 words ≈ 20-25 s audio — inside the model's
                             # stable one-shot envelope (probed: loops become
                             # frequent past ~500 rows/20 s, retries then carry
                             # the draw; typical scenes are 10-40 words)
MAX_CHARS_PER_CHUNK = 700    # legacy sentence fallback budget
# One-shot ceiling: ~80 s of audio. 20 tokens/word at 25 fps ≈ 0.8 s/word of
# budget; a 250-word scene needs ~6250 positions, so this is generous while
# still bounded (degenerate-loop guards below still apply per draw).
MAX_TOKENS_ONE_SHOT = 2048

# Degenerate-loop guards. The AR model can lock onto a repeated code vector
# (a sustained tone) or keep speaking long past the prompt; with the delay
# pattern a fresh EOC is only sampled when the model "decides" to stop, and
# a 1024-token cap gives it ~20× the headroom a sentence actually needs.
# We therefore (a) cap max_new_tokens proportional to the text length and
# (b) force-terminate when the sampled code vector repeats for many
# consecutive positions (the tone state). 15-20 tokens/word ≈ 0.6-0.8 s of
# audio per word of headroom — generous for slow speech, tight enough to
# never run 40 s on a 5-word line.
MAX_TOKENS_PER_WORD = 20
MIN_MAX_TOKENS = 96                 # ~3.8 s floor (very short lines)
MAX_TOKENS_CAP = 1500               # ~60 s hard cap per chunk (sentence mode)
# Natural speech ~2.5 wps @ 25 fps ≈ 10 audio rows/word; used to detect the
# "finished the text but never sampled EOC" failure — the model completes the
# line and then loops on a final phoneme. When the repetition guard fires at
# >= 85% of the expected row count, the draw is COMPLETE and only the looping
# tail needs cutting (recovers draws that would otherwise be wasted retries).
ROWS_PER_WORD_EST = 10
# Consecutive identical full code vectors => "stuck" tone loop. The default
# is 25 frames = 1.0 s of identical 8-codebook vectors. This is safely above
# any NATURAL sustained phoneme (a held vowel carries residual-codebook
# modulation and rarely holds >8-10 identical vectors) yet still catches a
# genuine loop LONG before the token caps. The old 8-frame threshold fired on
# long one-shot draws mid-sentence (the "9 s audio for a 45 s scene" bug):
# a dramatic pause or held vowel in real speech can exceed 8 identical
# vectors, killing a healthy draw. Tunable via HIGGS_REPEAT_BREAK_AFTER.
REPEAT_BREAK_AFTER = int(os.environ.get("HIGGS_REPEAT_BREAK_AFTER", "32"))

# Retry-on-degenerate. Even with the reference-exact sampler the 4B AR model
# stochastically (~10-40% of clone draws, rising with draw length) enters a bad
# state: a sustained-tone repetition (caught by the guard above), dead-air
# silence (decoded RMS ~0), or rambling far past the text without ever sampling
# cb0 EOC. Each draw is independent (global RNG), so retrying a degenerate
# chunk with a fresh sample is cheap and recovers the vast majority of
# failures. Each retry ALSO cools the temperature (see the synth loop) — a hot
# draw that looped usually terminates cleanly 0.1-0.2 lower.
SILENCE_RMS = 0.008               # decoded float32 RMS below this = dead air
MAX_CHUNK_RETRIES = 2             # attempts beyond the first per chunk
MIN_RETRY_TEMPERATURE = 0.5       # cooling floor for degenerate retries

# Emote -> Higgs control-tag mapping. Keys are the emotes used across the
# content-format playbooks / script scenes; the 21 Higgs emotions + 3 styles
# map 1:1. Unknown emotes are skipped (never degrade the text).
EMOTION_TAGS = {
    # direct Higgs emotions
    "anger": "anger", "angry": "anger", "furious": "anger", "rage": "anger",
    "sadness": "sadness", "sad": "sadness", "sorrow": "sadness", "grief": "sadness",
    "elation": "elation", "joy": "elation", "joyful": "elation", "happy": "elation",
    "thrilled": "elation", "delighted": "elation",
    "enthusiasm": "enthusiasm", "excited": "enthusiasm", "excitement": "enthusiasm",
    "energetic": "enthusiasm", "passionate": "enthusiasm",
    "amusement": "amusement", "playful": "amusement", "funny": "amusement",
    "determination": "determination", "determined": "determination", "firm": "determination",
    "serious": "determination", "resolute": "determination",
    "pride": "pride", "proud": "pride", "confident": "pride",
    "contentment": "contentment", "calm": "contentment", "content": "contentment",
    "peaceful": "contentment", "serene": "contentment",
    "affection": "affection", "warm": "affection", "loving": "affection", "tender": "affection",
    "relief": "relief", "relieved": "relief",
    "contemplation": "contemplation", "thoughtful": "contemplation", "reflective": "contemplation",
    "confusion": "confusion", "confused": "confusion", "puzzled": "confusion",
    "surprise": "surprise", "surprised": "surprise", "shocked": "surprise", "astonished": "surprise",
    "awe": "awe", "wonder": "awe", "amazed": "awe", "wondering": "awe",
    "longing": "longing", "nostalgic": "longing", "yearning": "longing",
    "arousal": "arousal", "flirty": "arousal", "seductive": "arousal",
    "fear": "fear", "scared": "fear", "afraid": "fear", "terrified": "fear", "nervous": "fear",
    "disgust": "disgust", "disgusted": "disgust",
    "bitterness": "bitterness", "bitter": "bitterness",
    "shame": "shame", "ashamed": "shame", "embarrassed": "shame", "guilty": "shame",
    "helplessness": "helplessness", "helpless": "helplessness", "desperate": "helplessness",
    # styles
    "whisper": "style:whispering", "whispering": "style:whispering", "hushed": "style:whispering",
    "shout": "style:shouting", "shouting": "style:shouting", "yelling": "style:shouting",
    "sing": "style:singing", "singing": "style:singing",
    # sfx-driven emotes (paired with an onomatopoeia by the caller)
    "laugh": "sfx:laughter", "laughter": "sfx:laughter",
    "sigh": "sfx:sigh", "sighing": "sfx:sigh",
    "cough": "sfx:cough",
    "cry": "sfx:crying", "crying": "sfx:crying",
}

# prosody tags from default_speed (script.tts.default_speed): ~1.25+ very
# fast, 1.08+ fast, 0.92-1.08 neutral, 0.75+ slow, else very slow. Matches the
# model's ~0.65/0.85/1.2/1.4 ratios.
SPEED_TAGS = [
    (1.25, "prosody:speed_very_fast"),
    (1.08, "prosody:speed_fast"),
    (0.92, None),            # neutral band
    (0.75, "prosody:speed_slow"),
    (0.0, "prosody:speed_very_slow"),
]


def log(msg: str) -> None:
    sys.stderr.write(f"[higgs_tts_sidecar] {msg}\n")
    sys.stderr.flush()


# ---------------------------------------------------------------------------
# Pure helpers (unit-testable without the model)
# ---------------------------------------------------------------------------

def chunk_text(text: str, max_chars: int = MAX_CHARS_PER_CHUNK) -> list:
    """Split text into sentence-aligned chunks under max_chars (safety net).

    Identical strategy to the gepard/audio8 sidecars: whole sentence while it
    fits, else comma/semicolon parts, else hard word cuts. Each chunk is
    synthesized independently and crossfade-concatenated, so a 1024-token cap
    can never truncate a scene.
    """
    import re

    if len(text) <= max_chars:
        return [text] if text.strip() else []

    sentences = re.split(r"(?<=[.!?…;])\s+|\n+", text.strip())
    chunks: list = []
    cur = ""

    def flush() -> None:
        nonlocal cur
        if cur:
            chunks.append(cur.strip())
            cur = ""

    for sent in sentences:
        sent = sent.strip()
        if not sent:
            continue
        if len(cur) + len(sent) + 1 <= max_chars:
            cur = f"{cur} {sent}".strip() if cur else sent
            continue
        flush()
        if len(sent) <= max_chars:
            cur = sent
            continue
        for part in re.split(r"(?<=[,;:])\s+", sent):
            part = part.strip()
            if not part:
                continue
            if len(cur) + len(part) + 1 <= max_chars:
                cur = f"{cur} {part}".strip() if cur else part
                continue
            flush()
            while len(part) > max_chars:
                cut = part[:max_chars]
                if " " in cut:
                    cut = cut.rsplit(" ", 1)[0]
                chunks.append(cut.strip())
                part = part[len(cut):].lstrip()
            if part:
                cur = part
    flush()
    return chunks


def compose_prompt(text: str, emote: str | None = None,
                   default_speed: float | None = None,
                   pitch: float | None = None,
                   pause_ms: int | None = None,
                   control_tags: str | None = None) -> str:
    """Fold emote/speed/pitch/pause/raw control tags into the text.

    Returns the raw prompt text (tags + text). Emission rules mirror
    PROMPTING.md placement semantics exactly:
      - emotion / style / prosody speed / pitch / expressive are
        sentence-level -> emitted at the START (they color the whole line).
      - `pause` / `long_pause` are inline -> emitted at the END of the line
        (between this line and whatever follows).
      - `control_tags` is a RAW passthrough (e.g. "<|prosody:pause|> mid,"
        or an sfx+onomatopoeia pair "<|sfx:cough|>Ahem,") prepended verbatim
        so agents can hand-place inline effects the structured fields don't
        express.

    NOTE: free-form instruct/tone text is deliberately NOT injected. Higgs
    only recognizes its 43 control tags — anything else "degrades output or
    gets read literally" (CAPABILITIES.md), so a parenthetical delivery
    instruction would be SPOKEN ALOUD. Structured emote/speed/pitch mapping
    + raw control_tags are the only channels.
    """
    tags = []
    if emote:
        key = emote.strip().lower()
        if key in EMOTION_TAGS:
            tag = EMOTION_TAGS[key]
            if ":" in tag:
                # Already namespaced (style:whispering, sfx:laughter) — use
                # as-is. Inline sfx needs an onomatopoeia attached, which the
                # speaker line usually already carries.
                tags.append(f"<|{tag}|>")
            else:
                # Plain emotion name -> sentence-level <|emotion:X|> tag.
                tags.append(f"<|emotion:{tag}|>")
    if default_speed is not None:
        for thr, tag in SPEED_TAGS:
            if default_speed >= thr:
                if tag:
                    tags.append(f"<|{tag}|>")
                break
    if pitch is not None:
        # Model ratios: pitch_low ≈ -3 semitones, pitch_high ≈ +2.5. Map the
        # script's pitch multiplier into the nearest tag band (neutral ~1.0
        # ±0.1 emits nothing).
        if pitch <= 0.9:
            tags.append("<|prosody:pitch_low|>")
        elif pitch >= 1.1:
            tags.append("<|prosody:pitch_high|>")
    if pause_ms is not None and pause_ms >= 400:
        # PROMPTING.md: long_pause ≈ 700-1500 ms, pause ≈ 400-700 ms.
        tags.append("<|prosody:long_pause|>" if pause_ms >= 800
                    else "<|prosody:pause|>")
    head = " ".join(tags)
    head = f"{head} " if head else ""
    if control_tags and control_tags.strip():
        raw = control_tags.strip()
        # Raw tags are prepended; ensure a space separates them from the
        # text unless the agent already attached an onomatopoeia (no space
        # after the tag per PROMPTING.md: "<|sfx:cough|>Ahem, ...").
        sep = "" if raw.endswith(("|", ",", "…", ".")) else " "
        return f"{head}{raw}{sep}{text}"
    return f"{head}{text}"


def estimate_max_tokens(text: str, ref_text: str | None = None,
                        one_shot: bool = False) -> int:
    """Length-proportional generation budget (delay-pattern positions).

    Normal speech is ~2.5 words/second ≈ 10 audio frames/word; budget
    20 tokens/word (2× headroom for slow/expressive delivery) plus a floor.
    One-shot draws allow the full MAX_TOKENS_ONE_SHOT ceiling (the model's
    8192 context absorbs whole scenes); chunked draws keep the 1500 cap
    (~60 s per chunk). The reference transcript is fixed conditioning (not
    generated), so it does NOT extend the budget.

    Returns an int in [MIN_MAX_TOKENS, cap]. Pure — unit-testable.
    """
    words = len([w for w in text.split() if w.strip()])
    budget = words * MAX_TOKENS_PER_WORD + 40
    cap = MAX_TOKENS_ONE_SHOT if one_shot else MAX_TOKENS_CAP
    return max(MIN_MAX_TOKENS, min(cap, budget))


def apply_delay_pattern(codes_TN, num_codebooks: int = NUM_CODEBOOKS,
                        boc: int = BOC, eoc: int = EOC):
    """Forward delay pattern — EXACTLY mirrors the sglang-omni reference
    (``sglang_omni/models/higgs_tts/utils.py::apply_delay_pattern``).

    ``codes_TN`` is a [T, N] int array of raw codec codes (0..1023). Returns
    the [T + N - 1, N] delayed matrix: column c gets BOC for rows 0..c-1,
    the real codes for rows c..c+T-1, and EOC for the remaining tail rows
    (the full matrix is initialized to EOC).

    The REFERENCE codes must be delay-patterned before they are fused into
    the prompt — the model was trained on delayed ref rows, and feeding raw
    parallel frames is off-distribution (caused the hissy/metallic clone).
    """
    import numpy as np  # noqa: PLC0415
    import numpy.typing as npt  # noqa: PLC0415

    arr = np.asarray(codes_TN, dtype=np.int64)
    if arr.ndim != 2:
        raise ValueError(f"codes must be 2-D [T, N], got {arr.shape}")
    t, n = arr.shape
    if n != num_codebooks:
        raise ValueError(f"codes have {n} codebooks, expected {num_codebooks}")
    out = np.full((t + n - 1, n), eoc, dtype=np.int64)
    for c in range(n):
        out[:c, c] = boc
        out[c:c + t, c] = arr[:, c]
    return out


def reverse_delay_pattern(delayed, num_codebooks: int = NUM_CODEBOOKS):
    """Reverse delay pattern — EXACTLY mirrors the sglang-omni reference
    (``sglang_omni/utils/codec_delay.py::reverse_delay_pattern``).

    ``[L, N]`` delayed rows -> ``[L - (N - 1), N]`` raw codes by fixed
    geometry (column c contributes rows c .. c+rows-1). The model is trained
    to place each column's EOC just outside its extracted window (cb0 EOC at
    row s, column c at row s+c, generation stops at s+N-2), so NO EOC/BOC ids
    ever reach the codec. Returns the raw [rows, N] int matrix.
    """
    import numpy as np  # noqa: PLC0415

    arr = np.asarray(delayed, dtype=np.int64)
    if arr.ndim != 2:
        raise ValueError(f"delayed must be 2-D [L, N], got {arr.shape}")
    length, n = arr.shape
    rows = length - (n - 1)
    if rows <= 0:
        return arr[:0, :]
    out = np.empty((rows, n), dtype=np.int64)
    for c in range(n):
        out[:, c] = arr[c:c + rows, c]
    return out


def dedelay_codes(position_codes: list, num_codebooks: int = NUM_CODEBOOKS,
                  boc: int = BOC, eoc: int = EOC):
    """Legacy EOC-scanning de-delay — kept for unit-test compatibility and
    as a defensive fallback. Production decode uses :func:`reverse_delay_pattern`
    (fixed geometry, reference-exact); this variant cuts each column at its
    first EOC.

    `position_codes[p]` = the 8 codes sampled at generation position p.
    Returns (codes[8][T] ints, frames T). Pure — unit-testable.
    """
    if not position_codes:
        return [], 0
    columns: list[list] = [[] for _ in range(num_codebooks)]
    done = [False] * num_codebooks
    for p, codes in enumerate(position_codes):
        for k in range(num_codebooks):
            if p < k:
                continue                      # BOC pad — no real code yet
            if done[k]:
                continue                      # column already terminated
            c = codes[k]
            if c == eoc:
                done[k] = True                # column terminated (stays done)
                continue
            columns[k].append(c)
    t = min((len(col) for col in columns), default=0)
    frames = [columns[k][:t] for k in range(num_codebooks)]
    return frames, t


def sample_from_logits(logits, temperature: float, top_k: int,
                       top_p: float | None = None,
                       eoc_safe: bool = False, eoc_id: int = EOC) -> int:
    """Sample one code id from a [1026]-shaped logits vector.

    Mirrors the sglang-omni `_sample_independent`: greedy short-circuit,
    temperature scaling, then top-k and/or top-p truncation, then a
    categorical draw. top_p is applied AFTER top-k (the reference order);
    None disables it.

    ``eoc_safe`` (used for codebook 0 only): never mask the EOC termination
    token. Top-k/top-p are CONTENT quality filters — masking EOC makes
    termination physically impossible while its logit rank sits outside the
    window, forcing the model to ramble on the final syllable until the
    distribution randomly lifts EOC into the window (probed: EOC rank 325-599
    for 88% of steps on a clean draw; the "evolve -> evolve-evolve-..."
    trailing-syllable artifact). Exempting EOC keeps it sampleable at its
    natural probability at every step: mid-sentence its prob is ~0 (no
    premature termination), and once the model's stop signal rises it can
    terminate immediately.
    """
    import numpy as np  # noqa: PLC0415

    logits = np.asarray(logits, dtype=np.float32).reshape(-1)
    # Greedy short-circuit (reference `_GREEDY_TEMP_THRESHOLD`): temperature
    # <= ~0 must argmax, NOT divide — logits/0 → NaN probs → a broken draw.
    if temperature <= 1e-5:
        return int(np.argmax(logits))
    scaled = logits / temperature
    logits = scaled
    if top_k > 0 and top_k < logits.size:
        kth = np.sort(scaled)[-top_k]
        logits = np.where(scaled < kth, -np.inf, scaled)
    if top_p is not None and 0.0 < top_p < 1.0:
        sorted_idx = np.argsort(logits)[::-1]
        sorted_logits = logits[sorted_idx]
        probs = np.exp(sorted_logits - sorted_logits.max())
        probs = probs / probs.sum()
        cum = np.cumsum(probs)
        cutoff = int(np.searchsorted(cum, top_p)) + 1
        keep = sorted_idx[:max(cutoff, 1)]
        mask = np.full(logits.shape, -np.inf)
        mask[keep] = logits[keep]
        logits = mask
    if eoc_safe:
        # Restore the temperature-scaled EOC logit after any truncation so the
        # termination token always remains a candidate.
        logits[eoc_id] = scaled[eoc_id]
    probs = np.exp(logits - logits.max())
    probs = probs / probs.sum()
    return int(np.random.choice(probs.size, p=probs))


def sampler_step(codes, delay_count, eoc_countdown,
                 num_codebooks: int = NUM_CODEBOOKS,
                 boc: int = BOC, eoc: int = EOC):
    """One step of the reference Higgs sampler state machine.

    EXACTLY mirrors ``sglang_omni/models/higgs_tts/sampler.py::step``: during
    the delay window (``delay_count < N``) codebooks above ``delay_count`` are
    forced to BOC and the counter advances; once codebook 0 samples EOC a
    wind-down of ``N - 2`` further rows runs; ``done`` becomes True when the
    wind-down completes. The caller appends the row on which ``done`` first
    fires (the reference returns that row too).

    ``codes`` is a mutable [N] list of sampled ids. Returns
    ``(codes, delay_count, eoc_countdown, done)``. Pure — unit-testable.
    """
    n = num_codebooks
    if delay_count < n:
        next_cb = delay_count + 1
        if next_cb < n:
            for k in range(next_cb, n):
                codes[k] = boc            # forced pad
        return codes, delay_count + 1, eoc_countdown, False
    if eoc_countdown is not None:
        eoc_countdown -= 1
        return codes, delay_count, eoc_countdown, eoc_countdown <= 0
    if codes[0] == eoc:
        return codes, delay_count, n - 2, False
    return codes, delay_count, None, False


# ---------------------------------------------------------------------------
# Runtime (lazy) — model + sessions
# ---------------------------------------------------------------------------

def spectral_noise_check(audio, sample_rate: int = SAMPLE_RATE):
    """Return True when the waveform is broadband noise, not speech.

    Uses zero-crossing rate + low-frequency energy ratio — the same metrics
    the config sweep uses: clean speech zcps < ~3500 and >75% energy below
    4 kHz; hiss/degraded 20-40% below 4 kHz. A draw can
    terminate cleanly (EOC wind-down ran, no guard, no leaks, non-silent
    RMS) yet still be broadband noise — the AR model occasionally locks onto
    a hiss pattern whose vectors vary enough to dodge the identical-vector
    repetition guard.
    """
    import numpy as np  # noqa: PLC0415

    if audio is None or audio.size < 1024:
        return False
    audio = np.asarray(audio, dtype=np.float32)
    seg = audio - audio.mean()
    zcps = float(np.abs(np.diff(np.signbit(seg))).sum() / len(seg) * sample_rate)
    n = 1 << 17
    if len(seg) < n:
        n = 1 << (len(seg).bit_length() - 1)
    mid = seg[len(seg) // 2 - n // 2: len(seg) // 2 + n // 2]
    if len(mid) < 1024:
        return zcps > 8000
    # Silence guard: if the FFT window is dead-air (a pause between
    # sentences), the energy ratio is meaningless (near-zero denominator) and
    # would false-flag a clean draw. Use the highest-energy window instead of
    # a fixed middle window, and skip the FFT entirely when even that is
    # quiet — the zcps gate above still catches true broadband hiss.
    mid_rms = float(np.sqrt((mid ** 2).mean())) if mid.size else 0.0
    seg_rms = float(np.sqrt((seg ** 2).mean())) if seg.size else 0.0
    if mid_rms < 0.25 * max(seg_rms, 1e-6):
        # Middle is a pause — fall back to the zcps gate only.
        return zcps > 8000
    spec = np.abs(np.fft.rfft(mid * np.hanning(len(mid))))
    freqs = np.fft.rfftfreq(len(mid), 1.0 / sample_rate)
    below4k = 100.0 * spec[freqs <= 4000].sum() / (spec.sum() + 1e-9)
    # Hiss band from the config sweep: clean speech 75-95% below 4 kHz;
    # hiss/degraded 20-40%. Flag anything in/below the hiss band so it gets
    # a cooled retry instead of shipping.
    noisy = zcps > 8000 or below4k < 40.0
    if noisy:
        log(f"spectral-noise flag: zcps={zcps:.0f} below4k={below4k:.1f}% "
            f"(clean speech ~<3500 zcps / >75% below4k)")
    return noisy


class HiggsPipeline:
    """All six ONNX sub-models + the generation loop."""

    def __init__(self, model_dir: Path, device: str = "auto"):
        self.model_dir = model_dir
        self.device = _resolve_device(device)
        self.llm_session = None
        self.text_embed = None
        self.audio_embed = None
        self.audio_heads = None
        self.audio_tokenizer = None
        self.audio_encoder = None
        self.tokenizer = None
        self.special_ids = {}
        self.llm_dtype = None  # probed activation dtype for inputs_embeds

    # -- loading ------------------------------------------------------------
    def load(self):
        import numpy as np  # noqa: PLC0415
        import onnxruntime as ort  # noqa: PLC0415
        from tokenizers import Tokenizer  # noqa: PLC0415

        required = [
            "genai_config.json", "llm_decoder.onnx", "llm_decoder.onnx.data",
            "text_embed.onnx", "audio_embed.onnx", "audio_heads.onnx",
            "audio_tokenizer.onnx", "audio_encoder.onnx", "tokenizer.json",
        ]
        missing = [f for f in required if not (self.model_dir / f).exists()]
        if missing:
            raise RuntimeError(
                f"Higgs model dir {self.model_dir} incomplete; missing {missing}. "
                "Run: bash scripts/setup_higgs.sh"
            )

        providers = _ort_providers(self.device)
        log(f"loading Higgs export from {self.model_dir} on {self.device} "
            f"(providers: {providers})")

        sess_opts = ort.SessionOptions()
        sess_opts.log_severity_level = 3

        # The int4 llm_decoder is a standard ONNX QDQ graph — plain ORT runs
        # it directly (probed: inputs_embeds fp16 + required past_key_values).
        # Manual KV-cache loop; NO onnxruntime-genai dependency.
        self.llm_session = ort.InferenceSession(
            str(self.model_dir / "llm_decoder.onnx"), sess_opts, providers=providers)
        self.llm_out_index = {
            o.name: i for i, o in enumerate(self.llm_session.get_outputs())
        }
        self.llm_inputs = {i.name for i in self.llm_session.get_inputs()}
        self.num_layers = 36  # qwen3-4B backbone (probed: 0..35 present.*)
        # Activation dtype is READ FROM THE GRAPH — reference inference.py:
        # "CUDA/fp16 ModelBuilder decoders expect float16 inputs_embeds + KV
        # cache; CPU int4 expects float32. Hardcoding fails on the other
        # build." Never assume fp16.
        self.llm_dtype = np.float32
        for _inp in self.llm_session.get_inputs():
            if _inp.name == "inputs_embeds":
                self.llm_dtype = np.float16 if "float16" in _inp.type else np.float32
        self.tokenizer = Tokenizer.from_file(str(self.model_dir / "tokenizer.json"))

        # Transformer sub-models (cuBLAS matmuls) run on the same provider as
        # the LLM — verified fast + stable on CUDA. The convolutional
        # `audio_encoder` is the ONE sub-model that needs cuDNN, and
        # cudnnCreate fails on this driver (CUDNN_STATUS_INTERNAL_ERROR at
        # session init) while the transformer path works perfectly. It only
        # runs ONCE per clone synth on a short reference clip, so pin it to
        # CPU — negligible cost, avoids the cuDNN failure entirely.
        self.text_embed = ort.InferenceSession(
            str(self.model_dir / "text_embed.onnx"), sess_opts, providers=providers)
        self.audio_embed = ort.InferenceSession(
            str(self.model_dir / "audio_embed.onnx"), sess_opts, providers=providers)
        self.audio_heads = ort.InferenceSession(
            str(self.model_dir / "audio_heads.onnx"), sess_opts, providers=providers)
        self.audio_tokenizer = ort.InferenceSession(
            str(self.model_dir / "audio_tokenizer.onnx"), sess_opts, providers=providers)
        self.audio_encoder = ort.InferenceSession(
            str(self.model_dir / "audio_encoder.onnx"), sess_opts,
            providers=["CPUExecutionProvider"])
        if self.device == "cuda":
            log("audio_encoder pinned to CPU (cuDNN unavailable on this driver); "
                "transformer path stays on CUDA")
        # Special prompt tokens + codec markers.
        for tok in ("<|tts|>", "<|text|>", "<|audio|>", "<|ref_text|>", "<|ref_audio|>"):
            tid = self.tokenizer.token_to_id(tok)
            if tid is None:
                raise RuntimeError(f"tokenizer missing special token {tok!r}")
            self.special_ids[tok] = tid

        # Smoke the llm once (empty KV) so a broken EP fails fast at load.
        probe_ids = np.array([[self.special_ids["<|tts|>"]]], dtype=np.int64)
        emb = self.text_embed.run(None, {"input_ids": probe_ids})[0]
        _ = self._llm_step(emb.astype(self.llm_dtype), emb.shape[1], None)
        log("llm ready (manual KV, plain ORT)")

    # -- llm step -----------------------------------------------------------
    def _llm_step(self, inputs_embeds, attn_len, past):
        """One llm_decoder forward with manual KV cache.

        CANONICAL contract — mirrors the onnx-community reference
        `inference.py::_llm_step` exactly: `inputs_embeds` is the [1, L, 2560]
        tensor for ONLY the new positions this step (prefill = the whole
        prompt; every decode step = the single new code-embed row),
        `attn_len` is the TOTAL sequence length (past + new) so the graph can
        build the causal mask, and `past` is the previous present.* KV list
        (None on prefill — the graph REQUIRES explicit empty KVs).
        Returns (hidden_last [1,1,2560], present_kvs list of 36 pairs).
        """
        import numpy as np  # noqa: PLC0415

        # attention_mask length = TOTAL sequence length (past + new); the
        # graph concatenates past KV internally. Feeding the whole accumulated
        # sequence every step instead (the old bug) re-attended every past
        # position AND made the KV cache grow quadratically (the BFC OOM on
        # long draws) — this is the exact reference contract.
        feeds = {
            "inputs_embeds": np.ascontiguousarray(inputs_embeds),
            "attention_mask": np.ones((1, int(attn_len)), dtype=np.int64),
        }
        for i in range(self.num_layers):
            if past is not None:
                feeds[f"past_key_values.{i}.key"] = past[i][0]
                feeds[f"past_key_values.{i}.value"] = past[i][1]
            else:
                # Graph requires explicit past inputs — zero-length KV.
                feeds[f"past_key_values.{i}.key"] = np.zeros(
                    (1, 8, 0, 128), dtype=self.llm_dtype)
                feeds[f"past_key_values.{i}.value"] = np.zeros(
                    (1, 8, 0, 128), dtype=self.llm_dtype)

        out = self.llm_session.run(None, feeds)
        hidden = out[self.llm_out_index["hidden_states"]]
        h = np.asarray(hidden)[:, -1:, :]
        present = [
            (np.asarray(out[self.llm_out_index[f"present.{i}.key"]]),
             np.asarray(out[self.llm_out_index[f"present.{i}.value"]]))
            for i in range(self.num_layers)
        ]
        return h, present

    # -- synthesis ----------------------------------------------------------
    def synth(self, text: str, output_path: str, voice: str | None = None,
              ref_audio: str | None = None, ref_text: str | None = None,
              emote: str | None = None, instruct: str | None = None,
              default_speed: float | None = None,
              pitch: float | None = None,
              pause_ms: int | None = None,
              control_tags: str | None = None,
              chunking: str | None = None,
              temperature: float | None = None, top_k: int | None = None,
              top_p: float | None = None,
              max_new_tokens: int | None = None) -> tuple:
        import numpy as np  # noqa: PLC0415
        import soundfile as sf  # noqa: PLC0415

        temperature = DEFAULT_TEMPERATURE if temperature is None else temperature
        top_k = DEFAULT_TOP_K if top_k is None else top_k
        if top_p is None:
            top_p = DEFAULT_TOP_P
        max_new_tokens = DEFAULT_MAX_NEW_TOKENS if max_new_tokens is None else max_new_tokens

        # Choose the chunking strategy. ONE-SHOT is canonical for higgs — the
        # conversational model keeps prosody continuous across a whole line;
        # sentence chunking re-anchors per chunk (the tonality-break bug).
        mode = (chunking or "auto").strip().lower()
        if mode not in ("one_shot", "sentence", "auto"):
            log(f"unknown chunking mode {mode!r}; using auto")
            mode = "auto"
        one_shot = mode == "one_shot" or (mode == "auto" and len(text) <= MAX_CHARS_ONE_SHOT)
        chunks = [text] if text.strip() else []
        if not one_shot:
            chunks = chunk_text(text)
        if not chunks:
            raise ValueError("synth text produced no chunks")

        # Length-proportional budget (degenerate-loop guard). The budget is
        # capped differently for one-shot draws (8192 context) vs chunked
        # (~30 s/chunk ceiling). The reference transcript is fixed
        # conditioning, so it does not extend the budget.
        plain_words = " ".join(c if isinstance(c, str) else c for c in chunks).split()
        if max_new_tokens > estimate_max_tokens(" ".join(plain_words), one_shot=one_shot):
            capped = estimate_max_tokens(" ".join(plain_words), one_shot=one_shot)
            log(f"capping max_new_tokens {max_new_tokens} -> {capped} "
                f"({len(plain_words)} words, {len(chunks)} chunk(s), "
                f"one_shot={one_shot}; degenerate-loop guard)")
            max_new_tokens = capped

        # `instruct` is accepted for protocol compatibility but never injected
        # into the prompt — Higgs is control-tag-only and would read it aloud.
        if instruct:
            log(f"ignoring free-form instruct (control-tag-only engine): {instruct[:80]}")

        # Resolve the reference: explicit ref_audio wins, else registered voice.
        if ref_audio:
            ref_path = Path(ref_audio)
            if not ref_path.exists():
                raise ValueError(f"reference audio not found: {ref_audio}")
            ref_text = ref_text or ""
        elif voice:
            ref_path = _voice_path(voice)
            if not ref_path.exists():
                raise ValueError(
                    f"no registered higgs voice '{voice}' (expected {ref_path}). "
                    "Register it via voice.profile.add with provider=higgs.")
            ref_text = ref_text or _voice_ref_text(voice)
        else:
            ref_path = None

        parts = []
        any_degenerate = False
        for i, chunk in enumerate(chunks):
            chunk_txt = compose_prompt(
                chunk, emote=emote, default_speed=default_speed,
                pitch=pitch, pause_ms=pause_ms, control_tags=control_tags)
            audio = None
            meta = None
            for attempt in range(MAX_CHUNK_RETRIES + 1):
                # TEMPERATURE-COOLING RETRIES: a degenerate draw (tone loop)
                # is usually the AR model running hot — retrying at a lower
                # temperature drastically raises the chance of a clean draw
                # (sweep data: 3 hot retries all looped; the cooled attempt
                # terminates cleanly). Cool 0.1 per retry, floor 0.5.
                attempt_temp = max(
                    MIN_RETRY_TEMPERATURE, temperature - 0.1 * attempt)
                try:
                    audio, meta = self._synth_chunk(
                        chunk_txt, ref_path=ref_path, ref_text=ref_text or None,
                        temperature=attempt_temp, top_k=top_k, top_p=top_p,
                        max_new_tokens=max_new_tokens)
                except Exception as exc:  # noqa: BLE001 — any failed draw
                    # (structural, codec, ORT OOM/GPU Fail) is just another
                    # degenerate attempt; a cooled retry is strictly better
                    # than propagating. onnxruntime raises
                    # onnxruntime.capi.onnxruntime_pybind11_state.Fail (not
                    # RuntimeError) on OOM/provider errors — catch broadly.
                    log(f"chunk {i + 1} attempt {attempt + 1} raised "
                        f"{type(exc).__name__}: {exc}")
                    meta = {"degenerate": True, "wind_down": False,
                            "rms": 0.0, "rows": 0, "leaked": 0,
                            "spectral_noise": False}
                if not meta["degenerate"]:
                    break
                log(f"degenerate chunk {i + 1} attempt {attempt + 1}/"
                    f"{MAX_CHUNK_RETRIES + 1} (wind_down={meta['wind_down']} "
                    f"rms={meta['rms']:.4f} rows={meta['rows']}"
                    f" leaked={meta.get('leaked', 0)}, temp={attempt_temp:.2f}); "
                    f"retrying")
            if meta and meta["degenerate"]:
                any_degenerate = True
                log(f"WARNING: chunk {i + 1} still degenerate after "
                    f"{MAX_CHUNK_RETRIES + 1} attempts (wind_down="
                    f"{meta['wind_down']}, rms={meta['rms']:.4f}, "
                    f"spectral={meta.get('spectral_noise', False)}); "
                    f"using last output")
            if len(chunks) > 1:
                log(f"chunk {i + 1}/{len(chunks)} ({len(chunk)} chars) -> "
                    f"{len(audio) / SAMPLE_RATE:.2f}s")
            parts.append(np.asarray(audio, dtype=np.float32))

        if len(parts) > 1:
            audio = crossfade_concat(parts, SAMPLE_RATE)
        else:
            audio = parts[0]
        if audio is None:
            raise ValueError("synthesis produced no audio")
        out = Path(output_path)
        out.parent.mkdir(parents=True, exist_ok=True)
        sf.write(str(out), audio, SAMPLE_RATE)
        # Uniform per-scene loudness — matches the gepard/voicedesign sidecars.
        normalize_lufs(str(out))
        duration_ms = int(round(len(audio) / SAMPLE_RATE * 1000.0))
        return duration_ms, SAMPLE_RATE, len(chunks), any_degenerate

    def _synth_chunk(self, chunk_txt: str, ref_path, ref_text,
                     temperature: float, top_k: int, max_new_tokens: int,
                     top_p: float | None = None):
        """Synthesize one chunk. Returns (audio float32 [N], meta dict).

        meta: {"degenerate": bool, "wind_down": bool, "rows": int, "rms": float}
        A chunk is flagged degenerate when the reference sampler never ran its
        cb0-EOC wind-down (repetition guard fired or the token cap was hit),
        or when the decoded waveform is dead-air silence.
        """
        import numpy as np  # noqa: PLC0415

        # -- build prompt embeds --------------------------------------------
        ids = [self.special_ids["<|tts|>"]]
        if ref_path is not None:
            ids += [self.special_ids["<|ref_text|>"]]
            ids += self.tokenizer.encode(ref_text, add_special_tokens=False).ids
            ids += [self.special_ids["<|ref_audio|>"]]
        ids += [self.special_ids["<|text|>"]]
        ids += self.tokenizer.encode(chunk_txt, add_special_tokens=False).ids
        ids += [self.special_ids["<|audio|>"]]
        ids = np.array([ids], dtype=np.int64)

        text_emb = self.text_embed.run(None, {"input_ids": ids})[0].astype(self.llm_dtype)

        if ref_path is not None:
            # Reference codes MUST be delay-patterned before fusing into the
            # prompt (sglang-omni reference: encode_reference ->
            # apply_delay_pattern -> fused embed at placeholder rows). Feeding
            # raw parallel frames is off-distribution and produces a hissy /
            # metallic clone that doesn't anchor to the speaker.
            codes = self._encode_reference(ref_path)          # [1, 8, F]
            codes_tn = codes.transpose(0, 2, 1)[0]            # [F, 8]
            delayed = apply_delay_pattern(codes_tn)           # [F+7, 8]
            ref_emb = self.audio_embed.run(
                None, {"codes": delayed[None].astype(np.int64)})[0]
            ref_emb = ref_emb.astype(self.llm_dtype)          # [1, F+7, 2560]
            # Delayed ref embeds go right after the <|ref_audio|> text token.
            ref_pos = int(np.where(ids[0] == self.special_ids["<|ref_audio|>"])[0][0]) + 1
            full = np.concatenate(
                [text_emb[:, :ref_pos], ref_emb, text_emb[:, ref_pos:]], axis=1)
        else:
            full = text_emb

        # -- delay-pattern autoregressive loop -------------------------------
        # Sampler state machine mirrors the sglang-omni reference exactly
        # (sampler.py::step): during the delay window codebooks > delay_count
        # are forced to BOC; once codebook 0 samples EOC a wind-down of
        # N-2 = 6 further rows runs (giving each delayed column's EOC time to
        # land just outside its reverse-delay window), then generation stops.
        # CANONICAL incremental decode — mirrors the reference
        # `inference.py::_run_ar` exactly: prefill the whole prompt ONCE
        # (empty KV), then feed ONE new code-embed row per step with
        # attn_len = running total. The OLD code re-fed the full accumulated
        # sequence every step alongside the past KV, which re-attended every
        # past position AND grew the KV cache quadratically (the BFC OOM on
        # long draws) — the exact bug the reference pattern avoids.
        past = None
        delayed_rows = []
        delay_count = 0
        eoc_countdown = None
        repeat_run = 0
        last_cb0 = None
        wind_down = False
        guard_fired = False
        cap_hit = False
        # Masked-EOC ramble guard (see below): the model must sample cb0 EOC
        # to terminate, but top-k can keep EOC masked (rank > top_k -> -inf ->
        # impossible to sample; probed on a clean 6-word draw: EOC rank 325-599
        # for 88% of steps). Natural speech runs ~ROWS_PER_WORD_EST rows/word
        # (+7-row delay prefix); past that, EOC masked for a stretch means the
        # model is rambling on the final syllable with drifting codes that the
        # identical-vector repeat guard cannot see.
        natural_end_rows = int(len(chunk_txt.split()) * ROWS_PER_WORD_EST) + 7
        ramble_budget = int(os.environ.get("HIGGS_RAMBLE_BUDGET", "0") or 0)
        if ramble_budget <= 0:
            ramble_budget = max(16, natural_end_rows // 5)
        ramble_steps = 0
        total = full.shape[1]
        hidden, present = self._llm_step(full, total, None)           # prefill
        past = present
        for step in range(max_new_tokens):
            logits = self.audio_heads.run(
                None, {"hidden_states": hidden.astype(np.float32)})[0]  # [1,1,8,1026]
            l = np.asarray(logits)[0, 0]                              # [8,1026]
            codes = []
            for k in range(NUM_CODEBOOKS):
                # EOC-safe on codebook 0 only: the termination token must
                # always remain sampleable (top-k/top-p are content filters).
                c = sample_from_logits(l[k], temperature, top_k, top_p,
                                       eoc_safe=(k == 0))
                codes.append(int(c))
            # --- reference sampler state machine (pure helper) ---
            codes, delay_count, eoc_countdown, done = sampler_step(
                codes, delay_count, eoc_countdown)
            delayed_rows.append(codes)
            if done:
                wind_down = True
                log(f"wind-down complete after cb0 EOC "
                    f"(total {len(delayed_rows)} rows)")
                break
            # Degenerate-loop guard (safety net only): if CODEBOOK-0 is
            # identical for REPEAT_BREAK_AFTER consecutive positions, the
            # model is stuck in a sustained-tone state and never samples EOC.
            # Tracks cb0 only (the reference convention) — full-8-vector
            # equality over-fires on held vowels mid-speech.
            if delay_count >= NUM_CODEBOOKS and eoc_countdown is None:
                if codes[0] == last_cb0:
                    repeat_run += 1
                    if repeat_run >= REPEAT_BREAK_AFTER:
                        guard_fired = True
                        log(f"degenerate repetition at step {step} "
                            f"(cb0 repeated {repeat_run}x); terminating")
                        break
                else:
                    repeat_run = 0
                # Masked-EOC force-stop: past the text-end estimate, if EOC
                # stays OUTSIDE top-k (masked) for `ramble_budget` consecutive
                # rows, the model cannot terminate and is rambling — cut at the
                # natural text end (de-delay geometry stays exact) and accept
                # the draw. EOC dipping back into top-k resets the counter so
                # slow-but-terminating speech gets room.
                scaled0 = np.asarray(l[0], dtype=np.float32) / temperature
                eoc_rank = int((scaled0 > scaled0[EOC]).sum()) + 1
                if eoc_rank > top_k and len(delayed_rows) >= natural_end_rows:
                    ramble_steps += 1
                    if ramble_steps >= ramble_budget:
                        log(f"masked-EOC ramble: cb0 EOC outside top-{top_k} "
                            f"for {ramble_steps} rows past text end "
                            f"(~{natural_end_rows} rows) — cutting rambling "
                            f"tail at natural end")
                        delayed_rows = delayed_rows[:natural_end_rows]
                        wind_down = True
                        break
                else:
                    ramble_steps = 0
            last_cb0 = int(codes[0])
            new_codes = np.array([[codes]], dtype=np.int64)           # [1,1,8]
            emb = self.audio_embed.run(None, {"codes": new_codes})[0].astype(self.llm_dtype)
            total += 1
            hidden, present = self._llm_step(emb, total, past)        # 1 new row
            past = present
        else:
            cap_hit = True

        # -- cap-hit handling (reference: loud TRUNCATED warning) -------------
        if cap_hit:
            secs = len(delayed_rows) * FRAME_SAMPLES / SAMPLE_RATE
            log(f"WARNING: hit max_new_tokens={max_new_tokens} without cb0 EOC "
                f"({secs:.1f}s, {len(delayed_rows)} rows) — output likely "
                f"TRUNCATED mid-speech; raise the budget for this scene")

        # -- cut-at-end recovery (finish-without-EOC) -------------------------
        # The most common failure on LONG draws: the model reads the WHOLE
        # text correctly but never samples cb0 EOC, then loops on the final
        # phoneme (sweep-verified: the guard fired at rows 223-279 for a
        # 28-word scene whose clean draws end at 248-308). If the guard fired
        # at >= 85% of the expected word-based row count, the speech is
        # COMPLETE — cut the looping tail and accept the draw instead of
        # wasting retries (and falling back to a 600-row rambling draw).
        if guard_fired and len(delayed_rows) >= int(
                len(chunk_txt.split()) * ROWS_PER_WORD_EST * 0.85):
            expected = int(len(chunk_txt.split()) * ROWS_PER_WORD_EST)
            keep = len(delayed_rows) - REPEAT_BREAK_AFTER
            if keep >= NUM_CODEBOOKS + 2:
                log(f"guard fired at natural end ({len(delayed_rows)} rows "
                    f"vs ~{expected} expected) — cutting looping tail, "
                    f"accepting complete draw")
                delayed_rows = delayed_rows[:keep]
                wind_down = True
                guard_fired = False
        # Reference tail-drop on repeat-stop: the trailing repeat rows ARE the
        # degenerate buzz that tripped the guard — drop them so a shipped
        # last-output doesn't end in a tone (inference.py `del delayed[-max_repeat:]`).
        if guard_fired and len(delayed_rows) > REPEAT_BREAK_AFTER + NUM_CODEBOOKS:
            log(f"dropping {REPEAT_BREAK_AFTER} trailing repeat rows "
                f"(reference tail-drop on repeat-stop)")
            delayed_rows = delayed_rows[:-REPEAT_BREAK_AFTER]

        # -- de-delay (fixed geometry, reference-exact) + codec decode -------
        if len(delayed_rows) < NUM_CODEBOOKS:
            raise RuntimeError(
                f"generation produced too few rows ({len(delayed_rows)}); "
                "model likely failed to emit audio (check log for OOM/NaN)")
        delayed_mat = np.array(delayed_rows, dtype=np.int64)           # [L,8]
        raw = reverse_delay_pattern(delayed_mat)                       # [L-7,8]
        if raw.shape[0] < 8:
            raise RuntimeError(
                f"de-delay produced too few audio frames ({raw.shape[0]}); "
                "model likely failed to emit audio (check log for OOM/NaN)")
        # Misalignment guard: with clean geometry each column's EOC lands just
        # outside its extraction window. If any BOC/EOC id reaches this point,
        # the model misaligned its termination — treat the draw as degenerate
        # (and CLIP the ids below so a shipped last-output never feeds special
        # ids to the codec — reference inference.py: `np.clip(codes, 0, 1023)`
        # — which would decode as a real code and produce distorted audio).
        leaked = int(np.isin(raw, [BOC, EOC]).sum())
        if leaked:
            log(f"WARNING: {leaked} BOC/EOC ids leaked into de-delayed codes "
                f"(misaligned termination); treating draw as degenerate")
        raw = np.clip(raw, 0, CODEC_VOCAB - 1)                         # drop BOC/EOC
        codes_mat = raw.T[None]                                        # [1,8,T]
        wave = self.audio_tokenizer.run(None, {"audio_codes": codes_mat})[0]
        audio = np.asarray(wave).reshape(-1)
        rms = float(np.sqrt((audio ** 2).mean())) if audio.size else 0.0
        # SPECTRAL-HEALTH CHECK: a draw can terminate cleanly (wind-down ran,
        # no guard, no leaks, non-silent RMS) yet still be broadband noise —
        # the AR model occasionally locks onto a hiss pattern whose vectors
        # vary enough to dodge the identical-vector repetition guard. Detect
        # it by zero-crossing rate + low-frequency energy ratio.
        spectral_noise = spectral_noise_check(audio, SAMPLE_RATE)
        # Degenerate = the sampler never ran its EOC wind-down (cap hit or
        # guard fired) AND the draw wasn't recovered by the cut-at-end path,
        # the codec would see special ids, the waveform is dead-air silence,
        # OR the draw is spectrally broadband noise.
        degenerate = (not wind_down) or guard_fired or leaked > 0 or rms < SILENCE_RMS or spectral_noise
        meta = {
            "degenerate": degenerate,
            "wind_down": wind_down,
            "rows": len(delayed_rows),
            "rms": rms,
            "leaked": leaked,
            "spectral_noise": spectral_noise,
        }
        return audio, meta

    def _encode_reference(self, wav_path):
        """Reference WAV -> audio codes [1, 8, frames] @24k (padded to 960-multiples)."""
        import numpy as np  # noqa: PLC0415
        import soundfile as sf  # noqa: PLC0415

        data, sr = sf.read(str(wav_path), dtype="float32", always_2d=True)
        if data.shape[1] > 1:
            data = data.mean(axis=1, keepdims=True)
        if sr != SAMPLE_RATE:
            data = _resample_mono(data[:, 0], sr, SAMPLE_RATE)
        else:
            data = data[:, 0]
        data = data.astype(np.float32)
        # Pad to a multiple of FRAME_SAMPLES (codec requirement).
        pad = (-len(data)) % FRAME_SAMPLES
        if pad:
            data = np.pad(data, (0, pad))
        x = data.reshape(1, 1, -1)                                     # [1,1,T]
        codes = self.audio_encoder.run(None, {"input_values": x})[0]   # [1,8,F]
        return np.asarray(codes)


# ---------------------------------------------------------------------------
# Module-level helpers
# ---------------------------------------------------------------------------

def _resolve_device(device: str) -> str:
    dev = (device or "auto").strip().lower()
    if dev in ("cuda", "cpu"):
        return dev
    # torch is not in .venv-higgs; fall back to ORT's compiled-in EP list.
    # NOTE: get_available_providers() can list CUDA even when the driver is
    # broken (compiled-in EPs) — ORT falls back to CPU at session creation
    # with a warning, so this is a preference, not a guarantee.
    try:
        import onnxruntime as ort  # noqa: PLC0415
        return "cuda" if "CUDAExecutionProvider" in ort.get_available_providers() else "cpu"
    except Exception:
        return "cpu"


def _ort_providers(device: str) -> list:
    if device == "cuda":
        return ["CUDAExecutionProvider", "CPUExecutionProvider"]
    return ["CPUExecutionProvider"]




def _resample_mono(x, src_sr: int, dst_sr: int):
    """Linear-interpolation resample for the reference clip (24k conversion)."""
    import numpy as np  # noqa: PLC0415
    if src_sr == dst_sr:
        return x
    n_out = int(round(len(x) * dst_sr / src_sr))
    xp = np.linspace(0.0, 1.0, len(x), dtype=np.float64)
    fp = np.linspace(0.0, 1.0, n_out, dtype=np.float64)
    return np.interp(fp, xp, x).astype(np.float32)


def _voice_path(name: str) -> Path:
    safe = "".join(c for c in name if c.isalnum() or c in "-_.").strip(".")
    if not safe:
        raise ValueError(f"invalid voice name: {name!r}")
    return VOICES_DIR / f"{safe}.wav"


def _voice_ref_text(name: str) -> str:
    meta_path = VOICES_DIR / "meta.json"
    if meta_path.exists():
        try:
            return json.loads(meta_path.read_text()).get(name, {}).get("ref_text", "")
        except Exception:
            return ""
    return ""


# ---------------------------------------------------------------------------
# Ops
# ---------------------------------------------------------------------------

_pipeline: HiggsPipeline | None = None


def get_pipeline() -> HiggsPipeline:
    global _pipeline
    if _pipeline is None:
        log(f"loading Higgs pipeline (first load can take minutes)")
        p = HiggsPipeline(MODEL_DIR, device=os.environ.get("HIGGS_DEVICE", "auto"))
        p.load()
        _pipeline = p
        log("Higgs pipeline ready")
    return _pipeline


def handle_synth(req):
    text = req.get("text", "")
    output_path = req.get("output_path", "")
    if not text or not output_path:
        raise ValueError("synth requires text, output_path")
    pipe = get_pipeline()
    duration_ms, sr, chunks, degenerate_any = pipe.synth(
        text,
        output_path=output_path,
        voice=req.get("voice") or None,
        ref_audio=req.get("ref_audio") or None,
        ref_text=req.get("ref_text") or None,
        emote=req.get("emote") or req.get("emotion") or None,
        instruct=req.get("instruct") or None,
        default_speed=req.get("default_speed"),
        pitch=req.get("pitch"),
        pause_ms=req.get("pause_ms"),
        control_tags=req.get("control_tags") or None,
        chunking=req.get("chunking") or None,
        temperature=req.get("temperature"),
        top_k=req.get("top_k"),
        top_p=req.get("top_p"),
        max_new_tokens=req.get("max_new_tokens"),
    )
    resp = {"status": "ok", "duration_ms": duration_ms, "sample_rate": sr, "chunks": chunks}
    if degenerate_any:
        # The draw(s) never recovered after cooled retries — the last output
        # shipped, but the caller MUST surface this (tools.rs logs a loud
        # warning; agents should re-audit the scene or lower temperature).
        resp["status"] = "warning"
        resp["warning"] = ("one or more chunks still degenerate after all retries; "
                            "last output shipped — re-audit the scene")
        resp["degenerate"] = True
    if req.get("emote"):
        resp["emote"] = req["emote"]
    return resp


def handle_register(req):
    import subprocess  # noqa: PLC0415
    import tempfile  # noqa: PLC0415

    name = req.get("name", "")
    audio_path = req.get("audio_path", "")
    ref_text = req.get("text", "")
    overwrite = bool(req.get("overwrite", True))
    if not name or not audio_path:
        raise ValueError("register requires name, audio_path")

    src = Path(audio_path)
    if not src.exists():
        raise ValueError(f"reference audio not found: {audio_path}")

    dst = _voice_path(name)
    VOICES_DIR.mkdir(parents=True, exist_ok=True)
    if dst.exists() and not overwrite:
        raise ValueError(f"voice '{name}' already registered (overwrite=false)")

    # Normalize the reference to 24 kHz mono WAV (the codec's native format).
    with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as tf:
        tmp = tf.name
    try:
        r = subprocess.run(
            ["ffmpeg", "-y", "-v", "error", "-i", str(src),
             "-ar", str(SAMPLE_RATE), "-ac", "1", "-c:a", "pcm_s16le", tmp],
            capture_output=True, text=True, shell=False)
        if r.returncode != 0:
            raise ValueError(f"ffmpeg ref normalization failed: {r.stderr[-300:]}")
        shutil.copy2(tmp, dst)
    finally:
        Path(tmp).unlink(missing_ok=True)

    meta_path = VOICES_DIR / "meta.json"
    meta = {}
    if meta_path.exists():
        try:
            meta = json.loads(meta_path.read_text())
        except Exception:
            meta = {}
    meta[name] = {"ref_text": ref_text, "ref_audio": str(src), "sample_rate": SAMPLE_RATE}
    meta_path.write_text(json.dumps(meta, ensure_ascii=False, indent=2))
    log(f"registered voice '{name}' -> {dst}")
    return {"status": "ok", "voice": name, "ref_path": str(dst)}


def handle_list(_req):
    VOICES_DIR.mkdir(parents=True, exist_ok=True)
    voices = []
    for f in sorted(VOICES_DIR.glob("*.wav")):
        voices.append({"name": f.stem, "ref_path": str(f)})
    meta_path = VOICES_DIR / "meta.json"
    if meta_path.exists():
        try:
            meta = json.loads(meta_path.read_text())
            for v in voices:
                v["ref_text"] = meta.get(v["name"], {}).get("ref_text", "")
        except Exception:
            pass
    return {"status": "ok", "voices": voices}


def handle_health(_req):
    voices = handle_list(_req).get("voices", [])
    return {
        "status": "ok",
        "checkpoint": "onnx-community/higgs-audio-v3-tts-4b (cuda_int4)",
        "model_dir": str(MODEL_DIR),
        "voices_dir": str(VOICES_DIR),
        "model_loaded": _pipeline is not None,
        "sample_rate": SAMPLE_RATE,
        "device": _pipeline.device if _pipeline else "not-loaded",
        "voices": voices,
    }


def _isolate_streams():
    """Protect the JSON protocol on stdout from ORT/OGA log chatter.

    Same pattern as gepard_tts_sidecar.py: dup fd 1 into a private protocol
    handle, point all Python-level stdout/stderr at a diagnostics log file,
    and redirect fd 2 (C-level writes from onnxruntime/torch) to that file.
    Only the protocol handle writes to the real stdout pipe.
    """
    import os

    log_path = Path(os.environ.get("HIGGS_LOG", "/tmp/higgs_tts_sidecar.log"))
    log_path.parent.mkdir(parents=True, exist_ok=True)
    log_fd = os.open(str(log_path), os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o644)
    os.dup2(log_fd, 2)
    proto_fd = os.dup(1)
    sys.stderr = os.fdopen(os.dup(2), "w", buffering=1)
    sys.stdout = os.fdopen(os.dup(2), "w", buffering=1)
    return os.fdopen(proto_fd, "w", buffering=1)


def _proto_write(proto, obj) -> None:
    proto.write(json.dumps(obj, ensure_ascii=False) + "\n")
    proto.flush()


def _dispatch(req) -> dict:
    op = req.get("op", "synth")
    if op == "synth":
        return handle_synth(req)
    if op == "register":
        return handle_register(req)
    if op == "list":
        return handle_list(req)
    if op == "health":
        return handle_health(req)
    raise ValueError(f"unknown op: {op}")


def serve() -> int:
    proto = _isolate_streams()
    log(f"ready (model_dir={MODEL_DIR}, voices_dir={VOICES_DIR})")
    _proto_write(proto, {"ready": True})
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        op = "synth"
        try:
            req = json.loads(line)
            op = req.get("op", "synth")
            resp = _dispatch(req)
        except Exception as exc:
            log(f"error handling {op!r}: {exc}")
            resp = {"status": "error", "error": str(exc)}
        _proto_write(proto, resp)
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description="Higgs Audio v3 TTS sidecar (long-lived serve mode)")
    parser.add_argument("--serve", action="store_true", help="Run as long-lived stdin/stdout server")
    parser.add_argument("--text", help="Text to synthesize (fresh-process mode)")
    parser.add_argument("--voice", help="Voice profile name (registered clone)")
    parser.add_argument("--output", help="Output WAV path")
    args = parser.parse_args()

    if args.serve:
        return serve()
    if args.text and args.output:
        proto = _isolate_streams()
        resp = handle_synth({"text": args.text, "voice": args.voice or None,
                             "output_path": args.output})
        _proto_write(proto, resp)
        return 0
    print("usage: higgs_tts_sidecar.py --serve   |   --text T --voice V --output OUT",
          file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main())
