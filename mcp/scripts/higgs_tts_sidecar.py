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
DEFAULT_MAX_NEW_TOKENS = int(os.environ.get("HIGGS_MAX_NEW_TOKENS", "1024"))
# max_new_tokens=1024 ≈ 1024 positions ≈ 1017 audio frames ≈ 40.7 s — far
# beyond any scene chunk; longer scenes are split on sentence boundaries.
MAX_CHARS_PER_CHUNK = 700

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
MAX_TOKENS_CAP = 768                # ~30 s hard cap per chunk
REPEAT_BREAK_AFTER = 8              # consecutive identical code vectors => stuck

# Retry-on-degenerate. Even with the reference-exact sampler the 4B AR model
# stochastically (~10-20% of clone draws) enters a bad state: a sustained-tone
# repetition (caught by the guard above), dead-air silence (decoded RMS ~0), or
# rambling far past the text without ever sampling cb0 EOC. Each draw is
# independent (global RNG), so retrying a degenerate chunk with a fresh sample
# is cheap and recovers the vast majority of failures.
SILENCE_RMS = 0.015               # decoded float32 RMS below this = dead air
MAX_CHUNK_RETRIES = 2             # attempts beyond the first per chunk

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
                   default_speed: float | None = None) -> str:
    """Fold emote/speed into the text using Higgs control tags.

    Returns the raw prompt text (tags + text). The emote maps to a sentence-
    level tag (or sfx/style); `default_speed` selects a prosody tag.

    NOTE: free-form instruct/tone text is deliberately NOT injected. Higgs
    only recognizes its 43 control tags — anything else "degrades output or
    gets read literally" (CAPABILITIES.md), so a parenthetical delivery
    instruction would be SPOKEN ALOUD. Emote-tag mapping is the only channel.
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
    head = " ".join(tags)
    head = f"{head} " if head else ""
    return f"{head}{text}"


def estimate_max_tokens(text: str, ref_text: str | None = None) -> int:
    """Length-proportional generation budget (delay-pattern positions).

    Normal speech is ~2.5 words/second ≈ 10 audio frames/word; budget
    20 tokens/word (2× headroom for slow/expressive delivery) plus a floor,
    capped at MAX_TOKENS_CAP. The reference transcript is fixed conditioning
    (not generated), so it does NOT extend the budget.

    Returns an int in [MIN_MAX_TOKENS, MAX_TOKENS_CAP]. Pure — unit-testable.
    """
    words = len([w for w in text.split() if w.strip()])
    budget = words * MAX_TOKENS_PER_WORD + 40
    return max(MIN_MAX_TOKENS, min(MAX_TOKENS_CAP, budget))


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


def sample_from_logits(logits, temperature: float, top_k: int) -> int:
    """Sample one code id from a [1026]-shaped logits vector."""
    import numpy as np  # noqa: PLC0415

    logits = np.asarray(logits, dtype=np.float32).reshape(-1)
    # Mask EOC/BOC so they are only chosen when genuinely most likely.
    if temperature > 0.0:
        logits = logits / temperature
    if top_k > 0 and top_k < logits.size:
        kth = np.sort(logits)[-top_k]
        logits = np.where(logits < kth, -np.inf, logits)
    probs = np.exp(logits - logits.max())
    probs = probs / probs.sum()
    return int(np.random.choice(probs.size, p=probs))


# ---------------------------------------------------------------------------
# Runtime (lazy) — model + sessions
# ---------------------------------------------------------------------------

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
        self.llm_dtype = np.float16  # probed: inputs_embeds is fp16 in the graph
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
        _ = self._llm_step(emb.astype(self.llm_dtype), past=None)
        log("llm ready (manual KV, plain ORT)")

    # -- llm step -----------------------------------------------------------
    def _llm_step(self, inputs_embeds, past):
        """One llm_decoder forward with manual KV cache.

        `inputs_embeds` is the fp16 [1, L, 2560] tensor for the CURRENT step's
        new positions (prefill = the whole prompt; later steps = one new
        position). `past` is a list of 36 (key, value) fp16 arrays or None for
        prefill (the graph REQUIRES past inputs — pass explicit empty KVs).
        Returns (hidden_last [1,1,2560], present_kvs list of 36 pairs).
        """
        import numpy as np  # noqa: PLC0415

        seq_len = inputs_embeds.shape[1]
        # attention_mask is per-input-segment (the graph concatenates past KV
        # internally); the probe declares 'total_sequence_length' but coherent
        # multi-token audio was produced passing ones(1, cur_len) — verified
        # empirically against ORT 1.28 (plain Session.run, incremental decode).
        feeds = {
            "inputs_embeds": np.ascontiguousarray(inputs_embeds),
            "attention_mask": np.ones((1, seq_len), dtype=np.int64),
        }
        for i in range(self.num_layers):
            if past is not None:
                feeds[f"past_key_values.{i}.key"] = past[i][0]
                feeds[f"past_key_values.{i}.value"] = past[i][1]
            else:
                # Graph requires explicit past inputs — zero-length KV.
                feeds[f"past_key_values.{i}.key"] = np.zeros(
                    (1, 8, 0, 128), dtype=np.float16)
                feeds[f"past_key_values.{i}.value"] = np.zeros(
                    (1, 8, 0, 128), dtype=np.float16)

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
              temperature: float | None = None, top_k: int | None = None,
              max_new_tokens: int | None = None) -> tuple:
        import numpy as np  # noqa: PLC0415
        import soundfile as sf  # noqa: PLC0415

        temperature = DEFAULT_TEMPERATURE if temperature is None else temperature
        top_k = DEFAULT_TOP_K if top_k is None else top_k
        max_new_tokens = DEFAULT_MAX_NEW_TOKENS if max_new_tokens is None else max_new_tokens
        # Never let a scene run 20× past its text — scale the budget to the
        # chunk length (ignoring control tags in the word count).
        plain_words = " ".join(chunk_text(text)).split()
        if max_new_tokens > estimate_max_tokens(" ".join(plain_words)):
            capped = estimate_max_tokens(" ".join(plain_words))
            log(f"capping max_new_tokens {max_new_tokens} -> {capped} "
                f"({len(plain_words)} words; degenerate-loop guard)")
            max_new_tokens = capped

        chunks = chunk_text(text)
        if not chunks:
            raise ValueError("synth text produced no chunks")
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
        for i, chunk in enumerate(chunks):
            chunk_txt = compose_prompt(chunk, emote=emote, default_speed=default_speed)
            audio = None
            last_meta = None
            for attempt in range(MAX_CHUNK_RETRIES + 1):
                audio, meta = self._synth_chunk(
                    chunk_txt, ref_path=ref_path, ref_text=ref_text or None,
                    temperature=temperature, top_k=top_k,
                    max_new_tokens=max_new_tokens)
                last_meta = meta
                if not meta["degenerate"]:
                    break
                log(f"degenerate chunk {i + 1} attempt {attempt + 1}/"
                    f"{MAX_CHUNK_RETRIES + 1} (wind_down={meta['wind_down']} "
                    f"rms={meta['rms']:.4f} rows={meta['rows']}); retrying")
            if meta["degenerate"]:
                log(f"WARNING: chunk {i + 1} still degenerate after "
                    f"{MAX_CHUNK_RETRIES + 1} attempts (wind_down="
                    f"{last_meta['wind_down']}, rms={last_meta['rms']:.4f}); "
                    f"using last output")
            if len(chunks) > 1:
                log(f"chunk {i + 1}/{len(chunks)} ({len(chunk)} chars) -> "
                    f"{len(audio) / SAMPLE_RATE:.2f}s")
            parts.append(np.asarray(audio, dtype=np.float32))

        audio = crossfade_concat(parts, SAMPLE_RATE)
        if audio is None:
            raise ValueError("synthesis produced no audio")
        out = Path(output_path)
        out.parent.mkdir(parents=True, exist_ok=True)
        sf.write(str(out), audio, SAMPLE_RATE)
        # Uniform per-scene loudness — matches the gepard/voicedesign sidecars.
        normalize_lufs(str(out))
        duration_ms = int(round(len(audio) / SAMPLE_RATE * 1000.0))
        return duration_ms, SAMPLE_RATE, len(chunks)

    def _synth_chunk(self, chunk_txt: str, ref_path, ref_text,
                     temperature: float, top_k: int, max_new_tokens: int):
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
        past = None
        delayed_rows = []
        cur = full
        delay_count = 0
        eoc_countdown = None
        repeat_run = 0
        last_codes = None
        wind_down = False
        guard_fired = False
        for step in range(max_new_tokens):
            hidden, present = self._llm_step(cur, past)
            past = present
            logits = self.audio_heads.run(
                None, {"hidden_states": hidden.astype(np.float32)})[0]  # [1,1,8,1026]
            l = np.asarray(logits)[0, 0]                              # [8,1026]
            codes = []
            for k in range(NUM_CODEBOOKS):
                c = sample_from_logits(l[k], temperature, top_k)
                codes.append(int(c))
            # --- reference sampler state machine ---
            if delay_count < NUM_CODEBOOKS:
                next_cb = delay_count + 1
                if next_cb < NUM_CODEBOOKS:
                    for k in range(next_cb, NUM_CODEBOOKS):
                        codes[k] = BOC                # forced pad
                delay_count += 1
            elif eoc_countdown is not None:
                eoc_countdown -= 1
                if eoc_countdown <= 0:
                    delayed_rows.append(codes)
                    wind_down = True
                    log(f"wind-down complete after cb0 EOC "
                        f"(total {len(delayed_rows)} rows)")
                    break
            elif codes[0] == EOC:
                eoc_countdown = NUM_CODEBOOKS - 2
            delayed_rows.append(codes)
            # Degenerate-loop guard (safety net only): if the full code vector
            # is identical for REPEAT_BREAK_AFTER consecutive positions, the
            # model is stuck in a sustained-tone state and never samples EOC.
            if delay_count >= NUM_CODEBOOKS and eoc_countdown is None:
                if codes == last_codes:
                    repeat_run += 1
                    if repeat_run >= REPEAT_BREAK_AFTER:
                        guard_fired = True
                        log(f"degenerate repetition at step {step} "
                            f"(code vector repeated {repeat_run}x); terminating")
                        break
                else:
                    repeat_run = 0
            last_codes = codes
            new_codes = np.array([[codes]], dtype=np.int64)           # [1,1,8]
            emb = self.audio_embed.run(None, {"codes": new_codes})[0].astype(self.llm_dtype)
            cur = np.concatenate([cur, emb], axis=1)

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
        codes_mat = raw.T[None]                                        # [1,8,T]
        wave = self.audio_tokenizer.run(None, {"audio_codes": codes_mat})[0]
        audio = np.asarray(wave).reshape(-1)
        rms = float(np.sqrt((audio ** 2).mean())) if audio.size else 0.0
        # Degenerate = the sampler never ran its EOC wind-down (cap hit or
        # guard fired) OR the decoded waveform is dead-air silence.
        degenerate = (not wind_down) or guard_fired or rms < SILENCE_RMS
        meta = {
            "degenerate": degenerate,
            "wind_down": wind_down,
            "rows": len(delayed_rows),
            "rms": rms,
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
    duration_ms, sr, chunks = pipe.synth(
        text,
        output_path=output_path,
        voice=req.get("voice") or None,
        ref_audio=req.get("ref_audio") or None,
        ref_text=req.get("ref_text") or None,
        emote=req.get("emote") or req.get("emotion") or None,
        instruct=req.get("instruct") or None,
        default_speed=req.get("default_speed"),
        temperature=req.get("temperature"),
        top_k=req.get("top_k"),
        max_new_tokens=req.get("max_new_tokens"),
    )
    resp = {"status": "ok", "duration_ms": duration_ms, "sample_rate": sr, "chunks": chunks}
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
