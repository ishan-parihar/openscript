#!/usr/bin/env python3
"""IndexTTS-2.5 TTS sidecar — emotionally expressive zero-shot voice cloning.

Drives the official `IndexTTS-2.5` stack (index-tts, bilibili) from the
`third_party/index-tts` checkout + the `IndexTeam/IndexTTS-2.5` checkpoints
(~5.7 GB, provisioned by `scripts/setup_indextts.sh`). IndexTTS-2.5 is a
~0.6-0.8B semantic GPT + Zipformer-flow S2M + BigVGAN vocoder pipeline:
22.05 kHz output, 5 languages (zh/en/ja/es/ar), and THREE emotion channels:

    - `emo_audio_prompt` + `emo_alpha` — condition on a separate emotional
      reference clip (maps to our profile emotion takes 1:1)
    - `emo_text` (needs use_qwen_emo) — natural-language emotion guidance
    - `emo_vector` — the 8-dim emotion vector

LONG-LIVED SERVE MODE (--serve):

    → {"op":"synth","text":"Hello","voice":"ishan","output_path":"/tmp/a.wav",
       "emote":"grave","temperature":0.9}
    ← {"status":"ok","duration_ms":2340,"sample_rate":22050}

    → {"op":"register","name":"ishan","audio_path":"/abs/ref.wav","overwrite":true}
    ← {"status":"ok","voice":"ishan"}

    → {"op":"list"} / {"op":"health"}

ENV:
  INDEXTTS_MODEL_DIR   checkpoints dir (default <root>/mcp/assets/indextts)
  INDEXTTS_VOICES_DIR  registered reference voices (default <root>/mcp/assets/indextts/voices)
  INDEXTTS_DEVICE      auto|cuda|cpu (default auto)
  INDEXTTS_QWEN_EMO    load the QwenEmo text-to-emotion model (default 1)
  INDEXTTS_LOG         diagnostics log (default /tmp/indextts_tts_sidecar.log)
  OPENSCRIPT_ROOT      repo root

LICENSE: IndexTTS-2.5 is released under the bilibili Model Use License —
research/non-commercial use; commercial use requires contacting
indexspeech@bilibili.com. Verify before any revenue-generating deployment.
"""

import argparse
import json
import os
import shutil
import sys
from pathlib import Path

# --- 8GB-GPU compatibility (RTX 2060) -------------------------------------
# Enable PyTorch's expandable segments so fragmented blocks are freed instead
# of OOM'ing during GPT decoding + vocoder. Must be set before torch import.
os.environ.setdefault("PYTORCH_CUDA_ALLOC_CONF", "expandable_segments:True")
# Quality-first: force one-shot generation (chunking re-anchors conditioning
# per segment and causes speaker drift/podcast effect). The 8GB OOM that low-
# vram mode existed for is already solved by QwenEmo CPU-offload +
# expandable_segments. Operators can re-enable via INDEXTTS_LOW_VRAM=1.
os.environ.setdefault("INDEXTTS_LOW_VRAM", "0")

# --- Shared TTS post-processing (loudness normalization + crossfade concat) --
_SCRIPT_DIR = Path(__file__).resolve().parent
if str(_SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(_SCRIPT_DIR))
from tts_common import normalize_lufs  # noqa: E402

_ROOT = Path(os.environ.get("OPENSCRIPT_ROOT", _SCRIPT_DIR.parent.parent)).resolve()
MODEL_DIR = Path(
    os.environ.get("INDEXTTS_MODEL_DIR", _ROOT / "mcp/assets/indextts")
).resolve()
VOICES_DIR = Path(
    os.environ.get("INDEXTTS_VOICES_DIR", _ROOT / "mcp/assets/indextts/voices")
).resolve()
SAMPLE_RATE = 22050

# Sampling defaults MATCH the engine's own canonical values
# (infer_v2_5.py generation_kwargs pops: temperature 0.8, top_k 30,
# top_p 0.8, repetition_penalty 10.0, num_beams 3). Passing top_k=0 would
# DISABLE top-k filtering entirely (engine default is 30) and a weak
# repetition_penalty lets the mel-code sampler wander — both cause the
# voice drift / second-speaker artifact.
DEFAULT_TEMPERATURE = float(os.environ.get("INDEXTTS_TEMPERATURE", "0.8"))
DEFAULT_TOP_K = int(os.environ.get("INDEXTTS_TOP_K", "30"))
DEFAULT_TOP_P = float(os.environ.get("INDEXTTS_TOP_P", "0.8"))
DEFAULT_REPETITION_PENALTY = float(os.environ.get("INDEXTTS_REPETITION_PENALTY", "10.0"))
# num_beams matches the engine default (3) — do not override.

# Emote -> natural-language emotion guidance (emo_text, QwenEmo). Unknown
# emotes fall back to the emote string itself; empty when no emote.
EMOTION_TEXT = {
    "neutral": None,
    "grave": "sad, somber, subdued, serious",
    "somber": "sad, somber, subdued, serious",
    "sad": "sad, sorrowful",
    "sadness": "sad, sorrowful",
    "sorrow": "sad, sorrowful",
    "grief": "grieving, sorrowful, heavy",
    "firm": "determined, resolute, assertive, serious",
    "determination": "determined, resolute, assertive",
    "determined": "determined, resolute, assertive",
    "serious": "serious, focused",
    "happy": "happy, cheerful, bright",
    "joy": "joyful, bright, cheerful",
    "elation": "elated, joyful, bright",
    "excited": "excited, enthusiastic, energetic",
    "enthusiasm": "excited, enthusiastic, energetic",
    "energetic": "energetic, lively",
    "calm": "calm, peaceful, gentle",
    "contentment": "calm, content, peaceful",
    "peaceful": "calm, peaceful, gentle",
    "warm": "warm, affectionate, gentle",
    "affection": "warm, affectionate, tender",
    "tender": "warm, tender, gentle",
    "loving": "loving, warm, tender",
    "whisper": "soft, whispered, hushed, intimate",
    "whispering": "soft, whispered, hushed, intimate",
    "hushed": "soft, whispered, hushed, intimate",
    "shout": "loud, shouting, emphatic",
    "angry": "angry, frustrated, tense",
    "anger": "angry, frustrated, tense",
    "furious": "furious, enraged, intense",
    "fear": "fearful, anxious, tense",
    "scared": "fearful, anxious, tense",
    "nervous": "nervous, anxious, hesitant",
    "surprise": "surprised, astonished",
    "surprised": "surprised, astonished",
    "awe": "awed, wonder-struck, amazed",
    "confusion": "confused, puzzled, uncertain",
    "confused": "confused, puzzled, uncertain",
    "playful": "playful, light, amused",
    "amusement": "amused, playful, light",
    "funny": "amused, playful",
    "proud": "proud, confident",
    "pride": "proud, confident",
    "confident": "confident, assured",
    "relief": "relieved, relaxed, easy",
    "relieved": "relieved, relaxed, easy",
    "sigh": "resigned, weary, soft",
    "sighing": "resigned, weary, soft",
    "cry": "crying, tearful, upset",
    "crying": "crying, tearful, upset",
    "laugh": "laughing, amused, bright",
    "laughter": "laughing, amused, bright",
}

# Emote -> DIRECT 8-dim emotion vector (order: happy, angry, sad, afraid,
# disgusted, melancholic, surprised, calm) passed via the engine's native
# `emo_vector` channel. This BYPASSES QwenEmo for known emotes: QwenEmo is a
# Chinese-trained text classifier whose `self.prompt = "文本情感分类"` flattens
# English delivery adjectives — probed output for "determined, resolute,
# assertive, serious" (firm) and "soft, whispered, hushed, intimate" (whisper)
# is `calm = 1.00`, which is the EXACT same conditioning as a neutral line
# (mixing is `emovec = emovec_mat + (1 - sum(w)) * neutral`, so a calm=1.0
# vector carries zero emotion). Curated vectors target sum ~0.85-0.95 to stay
# in the engine's expected distribution (QwenEmo itself emits sums ~1.0).
# Unknown emotes still fall back to EMOTION_TEXT -> QwenEmo.
EMOTE_VECTORS = {
    # [happy, angry, sad, afraid, disgusted, melancholic, surprised, calm]
    "neutral": None,
    "firm": [0.0, 0.35, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5],
    "determination": [0.0, 0.35, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5],
    "determined": [0.0, 0.35, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5],
    "assertive": [0.0, 0.35, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5],
    "resolute": [0.0, 0.35, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5],
    "serious": [0.0, 0.2, 0.05, 0.0, 0.0, 0.0, 0.0, 0.65],
    "grave": [0.0, 0.0, 0.55, 0.0, 0.0, 0.15, 0.0, 0.15],
    "somber": [0.0, 0.0, 0.55, 0.0, 0.0, 0.15, 0.0, 0.15],
    "sad": [0.0, 0.0, 0.8, 0.0, 0.0, 0.05, 0.0, 0.05],
    "sadness": [0.0, 0.0, 0.8, 0.0, 0.0, 0.05, 0.0, 0.05],
    "sorrow": [0.0, 0.0, 0.8, 0.0, 0.0, 0.05, 0.0, 0.05],
    "grief": [0.0, 0.0, 0.7, 0.0, 0.0, 0.15, 0.0, 0.05],
    "grieving": [0.0, 0.0, 0.7, 0.0, 0.0, 0.15, 0.0, 0.05],
    "happy": [0.85, 0.0, 0.0, 0.0, 0.0, 0.0, 0.05, 0.05],
    "joy": [0.85, 0.0, 0.0, 0.0, 0.0, 0.0, 0.05, 0.05],
    "joyful": [0.85, 0.0, 0.0, 0.0, 0.0, 0.0, 0.05, 0.05],
    "elation": [0.9, 0.0, 0.0, 0.0, 0.0, 0.0, 0.05, 0.0],
    "elated": [0.9, 0.0, 0.0, 0.0, 0.0, 0.0, 0.05, 0.0],
    "excited": [0.7, 0.0, 0.0, 0.0, 0.0, 0.0, 0.15, 0.1],
    "enthusiasm": [0.7, 0.0, 0.0, 0.0, 0.0, 0.0, 0.15, 0.1],
    "enthusiastic": [0.7, 0.0, 0.0, 0.0, 0.0, 0.0, 0.15, 0.1],
    "energetic": [0.75, 0.0, 0.0, 0.0, 0.0, 0.0, 0.1, 0.1],
    "lively": [0.75, 0.0, 0.0, 0.0, 0.0, 0.0, 0.1, 0.1],
    "calm": [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.7],
    "contentment": [0.1, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.6],
    "content": [0.1, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.6],
    "peaceful": [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.7],
    "gentle": [0.0, 0.0, 0.0, 0.0, 0.0, 0.1, 0.0, 0.6],
    "warm": [0.15, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.6],
    "affection": [0.2, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.55],
    "affectionate": [0.2, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.55],
    "tender": [0.1, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.6],
    "loving": [0.2, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.55],
    "whisper": [0.0, 0.0, 0.0, 0.0, 0.0, 0.25, 0.0, 0.55],
    "whispering": [0.0, 0.0, 0.0, 0.0, 0.0, 0.25, 0.0, 0.55],
    "hushed": [0.0, 0.0, 0.0, 0.0, 0.0, 0.25, 0.0, 0.55],
    "intimate": [0.1, 0.0, 0.0, 0.0, 0.0, 0.2, 0.0, 0.55],
    "shout": [0.0, 0.8, 0.0, 0.0, 0.0, 0.0, 0.1, 0.0],
    "loud": [0.0, 0.8, 0.0, 0.0, 0.0, 0.0, 0.1, 0.0],
    "angry": [0.0, 0.85, 0.0, 0.0, 0.0, 0.0, 0.0, 0.05],
    "anger": [0.0, 0.85, 0.0, 0.0, 0.0, 0.0, 0.0, 0.05],
    "frustrated": [0.0, 0.6, 0.05, 0.0, 0.0, 0.0, 0.0, 0.25],
    "furious": [0.0, 0.9, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    "fear": [0.0, 0.0, 0.0, 0.8, 0.0, 0.0, 0.05, 0.05],
    "fearful": [0.0, 0.0, 0.0, 0.8, 0.0, 0.0, 0.05, 0.05],
    "scared": [0.0, 0.0, 0.0, 0.8, 0.0, 0.0, 0.05, 0.05],
    "terrified": [0.0, 0.0, 0.0, 0.85, 0.0, 0.0, 0.05, 0.0],
    "anxious": [0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.05, 0.35],
    "nervous": [0.0, 0.0, 0.0, 0.45, 0.0, 0.0, 0.05, 0.35],
    "hesitant": [0.0, 0.0, 0.0, 0.3, 0.0, 0.0, 0.1, 0.45],
    "surprise": [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.8, 0.1],
    "surprised": [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.8, 0.1],
    "astonished": [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.8, 0.1],
    "awe": [0.1, 0.0, 0.0, 0.0, 0.0, 0.0, 0.65, 0.15],
    "awed": [0.1, 0.0, 0.0, 0.0, 0.0, 0.0, 0.65, 0.15],
    "confusion": [0.0, 0.0, 0.0, 0.0, 0.0, 0.1, 0.4, 0.35],
    "confused": [0.0, 0.0, 0.0, 0.0, 0.0, 0.1, 0.4, 0.35],
    "puzzled": [0.0, 0.0, 0.0, 0.0, 0.0, 0.1, 0.4, 0.35],
    "uncertain": [0.0, 0.0, 0.0, 0.0, 0.0, 0.1, 0.25, 0.5],
    "playful": [0.6, 0.0, 0.0, 0.0, 0.0, 0.0, 0.15, 0.15],
    "amusement": [0.6, 0.0, 0.0, 0.0, 0.0, 0.0, 0.15, 0.15],
    "amused": [0.6, 0.0, 0.0, 0.0, 0.0, 0.0, 0.15, 0.15],
    "funny": [0.65, 0.0, 0.0, 0.0, 0.0, 0.0, 0.1, 0.15],
    "proud": [0.2, 0.1, 0.0, 0.0, 0.0, 0.0, 0.0, 0.55],
    "pride": [0.2, 0.1, 0.0, 0.0, 0.0, 0.0, 0.0, 0.55],
    "confident": [0.1, 0.15, 0.0, 0.0, 0.0, 0.0, 0.0, 0.6],
    "assured": [0.1, 0.15, 0.0, 0.0, 0.0, 0.0, 0.0, 0.6],
    "relief": [0.15, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.6],
    "relieved": [0.15, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.6],
    "relaxed": [0.05, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.65],
    "easy": [0.05, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.65],
    "sigh": [0.0, 0.0, 0.05, 0.0, 0.0, 0.3, 0.0, 0.5],
    "sighing": [0.0, 0.0, 0.05, 0.0, 0.0, 0.3, 0.0, 0.5],
    "resigned": [0.0, 0.0, 0.2, 0.0, 0.0, 0.35, 0.0, 0.35],
    "weary": [0.0, 0.0, 0.2, 0.0, 0.0, 0.4, 0.0, 0.3],
    "cry": [0.0, 0.0, 0.8, 0.0, 0.0, 0.1, 0.0, 0.0],
    "crying": [0.0, 0.0, 0.8, 0.0, 0.0, 0.1, 0.0, 0.0],
    "tearful": [0.0, 0.0, 0.75, 0.0, 0.0, 0.15, 0.0, 0.0],
    "upset": [0.0, 0.0, 0.55, 0.0, 0.0, 0.1, 0.0, 0.25],
    "laugh": [0.8, 0.0, 0.0, 0.0, 0.0, 0.0, 0.1, 0.0],
    "laughter": [0.8, 0.0, 0.0, 0.0, 0.0, 0.0, 0.1, 0.0],
    "laughing": [0.8, 0.0, 0.0, 0.0, 0.0, 0.0, 0.1, 0.0],
    "melancholic": [0.0, 0.0, 0.0, 0.0, 0.0, 0.8, 0.0, 0.1],
    "melancholy": [0.0, 0.0, 0.0, 0.0, 0.0, 0.8, 0.0, 0.1],
    "depressed": [0.0, 0.0, 0.3, 0.0, 0.0, 0.5, 0.0, 0.1],
    "gloomy": [0.0, 0.0, 0.25, 0.0, 0.0, 0.5, 0.0, 0.15],
    "disgusted": [0.0, 0.1, 0.0, 0.0, 0.7, 0.0, 0.0, 0.1],
    "disgust": [0.0, 0.1, 0.0, 0.0, 0.7, 0.0, 0.0, 0.1],
    "afraid": [0.0, 0.0, 0.0, 0.85, 0.0, 0.0, 0.0, 0.05],
    "thoughtful": [0.0, 0.0, 0.0, 0.0, 0.0, 0.15, 0.1, 0.6],
    "thinking": [0.0, 0.0, 0.0, 0.0, 0.0, 0.1, 0.1, 0.65],
}

# Emotes that additionally get a quieting gain (dB) applied AFTER loudness
# normalization — a whisper-class delivery must be audibly hushed, and the
# uniform -16 LUFS normalization would otherwise erase the quietness entirely.
WHISPER_GAIN_DB = {"whisper": -8.0, "whispering": -8.0, "hushed": -8.0}


def log(msg: str) -> None:
    sys.stderr.write(f"[indextts_tts_sidecar] {msg}\n")
    sys.stderr.flush()


def _voice_path(name: str) -> Path:
    safe = "".join(c for c in name if c.isalnum() or c in "-_.").strip(".")
    if not safe:
        raise ValueError(f"invalid voice name: {name!r}")
    return VOICES_DIR / f"{safe}.wav"


# --- Protocol stream isolation ----------------------------------------------
# The index-tts inference code prints progress to stdout (">> starting
# inference...", timing lines, wav shapes). With stdin/stdout JSON-RPC that
# corrupts the protocol, so we dup fd 1 into a private handle, reroute fd 1 to
# stderr, and write protocol JSON ONLY via the private handle (the gepard
# sidecar uses the identical pattern).
def _isolate_streams():
    import io  # noqa: PLC0415

    proto = os.fdopen(os.dup(1), "w", buffering=1)
    os.dup2(2, 1)  # fd 1 -> stderr (model prints land in the log, not the pipe)
    sys.stdout = sys.stderr
    return proto


def _proto_write(proto, obj) -> None:
    proto.write(json.dumps(obj) + "\n")
    proto.flush()


# --- Runtime (lazy) ----------------------------------------------------------
_session = None  # IndexTTS2 — created on first synth, kept alive


def _resolve_device() -> str:
    dev = os.environ.get("INDEXTTS_DEVICE", "auto").strip().lower()
    if dev in ("cuda", "cpu"):
        return dev
    try:
        import torch  # noqa: PLC0415

        return "cuda" if torch.cuda.is_available() else "cpu"
    except Exception:
        return "cpu"


def get_session():
    """Lazily build the IndexTTS2 pipeline (~5.7 GB checkpoints on first load)."""
    global _session
    if _session is None:
        import torch  # noqa: PLC0415

        cfg_path = MODEL_DIR / "config.yaml"
        if not cfg_path.exists():
            raise RuntimeError(
                f"IndexTTS model dir {MODEL_DIR} incomplete (missing config.yaml). "
                "Run: bash scripts/setup_indextts.sh"
            )
        device = _resolve_device()
        use_qwen_emo = os.environ.get("INDEXTTS_QWEN_EMO", "1").strip().lower() not in (
            "0", "false", "no")
        # infer_v2_5.py hardcodes HF_HUB_CACHE relative to CWD at import — run
        # from the model dir so the Qwen cache lands inside the asset dir, then
        # RESTORE the original CWD so relative output_paths (e.g.
        # "output/samples/x.wav") resolve from the caller's working directory,
        # not from inside the model dir.
        orig_cwd = os.getcwd()
        os.chdir(MODEL_DIR)
        log(f"loading IndexTTS-2.5 from {MODEL_DIR} on {device} "
            f"(bf16={device != 'cpu'}, qwen_emo={use_qwen_emo}); first load takes minutes)")
        try:
            from indextts.infer_v2_5 import IndexTTS2  # noqa: PLC0415
        finally:
            os.chdir(orig_cwd)

        _session = IndexTTS2(
            cfg_path=str(cfg_path),
            model_dir=str(MODEL_DIR),
            use_bf16=(device != "cpu"),
            device=device,
            use_qwen_emo=use_qwen_emo,
            # Turing (sm_75) compatibility: the BigVGAN fused CUDA kernel is
            # built for newer archs; the plain torch fallback is fine.
            use_cuda_kernel=False,
        )
        log("IndexTTS2 pipeline ready")
    return _session


def handle_synth(req):
    import numpy as np  # noqa: PLC0415
    import soundfile as sf  # noqa: PLC0415

    text = req.get("text", "")
    voice = req.get("voice", "")
    output_path = req.get("output_path", "")
    emote = req.get("emote") or req.get("emotion") or None
    if not text or not output_path:
        raise ValueError("synth requires text, output_path")

    # Reference: explicit ref_audio override (emotion take) wins, else the
    # registered voice's neutral reference WAV.
    ref_audio = req.get("ref_audio") or None
    if ref_audio:
        ref = Path(ref_audio)
        if not ref.exists():
            raise ValueError(f"reference audio not found: {ref_audio}")
    elif voice:
        ref = _voice_path(voice)
        if not ref.exists():
            raise ValueError(
                f"no registered indextts voice '{voice}' (expected {ref}). "
                "Register it via voice.profile.add with provider=indextts.")
    else:
        raise ValueError("synth requires 'voice' or 'ref_audio' (IndexTTS is a clone engine)")

    session = get_session()

    # Emotion channel resolution (priority):
    #   1. explicit `emo_vector`   (direct 8-dim, deterministic, strongest)
    #   2. `emo_audio_prompt`      (emotional reference clip, model-native)
    #   3. explicit `emo_text`     (QwenEmo natural-language guidance)
    #   4. emote -> curated EMOTE_VECTORS (bypasses QwenEmo, which flattens
    #      EN delivery adjectives like firm/whisper to calm=1.0 == neutral)
    #   5. emote -> EMOTION_TEXT   (QwenEmo fallback for unknown emotes)
    emo_alpha = float(req.get("emo_alpha", 1.0)) if req.get("emo_alpha") is not None else 1.0
    take_ref = req.get("emo_audio_prompt") or None  # explicit emotion-take clip
    emo_text = req.get("emo_text") or None
    emo_vector = req.get("emo_vector") or None
    if emote and not take_ref and not emo_text and emo_vector is None:
        key = emote.strip().lower()
        if key in EMOTE_VECTORS:
            emo_vector = EMOTE_VECTORS[key]
        else:
            emo_text = EMOTION_TEXT.get(key)

    duration_factor = float(req.get("duration_factor", 1.0))
    if req.get("speed") is not None:
        duration_factor = float(req["speed"])
    if duration_factor <= 0:
        duration_factor = 1.0

    gen_kwargs = {}
    if req.get("temperature") is not None:
        gen_kwargs["temperature"] = float(req["temperature"])
    else:
        gen_kwargs["temperature"] = DEFAULT_TEMPERATURE
    if req.get("top_k") is not None:
        gen_kwargs["top_k"] = int(req["top_k"])
    else:
        gen_kwargs["top_k"] = DEFAULT_TOP_K
    if req.get("top_p") is not None:
        gen_kwargs["top_p"] = float(req["top_p"])
    else:
        gen_kwargs["top_p"] = DEFAULT_TOP_P
    gen_kwargs["repetition_penalty"] = (
        float(req["repetition_penalty"])
        if req.get("repetition_penalty") is not None
        else DEFAULT_REPETITION_PENALTY
    )

    tmp_out = output_path + ".tmp.wav"
    log(f"synth {len(text)} chars voice={voice} emote={emote} "
        f"emo_vector={emo_vector} emo_text={emo_text!r} take={take_ref!r} "
        f"dur_factor={duration_factor} temp={gen_kwargs['temperature']}")
    try:
        session.infer(
            spk_audio_prompt=str(ref),
            text=text,
            output_path=tmp_out,
            lang="EN",
            emo_audio_prompt=take_ref,
            emo_alpha=emo_alpha,
            use_emo_text=emo_text is not None and emo_vector is None,
            emo_text=emo_text if emo_vector is None else None,
            emo_vector=emo_vector,
            duration_factor=duration_factor,
            verbose=False,
            **gen_kwargs,
        )
    except Exception as exc:  # noqa: BLE001
        raise RuntimeError(f"IndexTTS infer failed: {exc}") from exc

    if not Path(tmp_out).exists():
        raise RuntimeError("IndexTTS infer returned without writing output WAV")

    data, sr = sf.read(tmp_out, dtype="float32")
    if data.ndim > 1:
        data = data.mean(axis=1)
    out = Path(output_path)
    out.parent.mkdir(parents=True, exist_ok=True)
    sf.write(str(out), data.astype(np.float32), SAMPLE_RATE)
    os.remove(tmp_out)
    # Uniform per-scene loudness (matches the audio8/voicedesign sidecars),
    # then re-apply any emote-specific delivery gain (whisper-class quieting)
    # so the intentional hushed character survives normalization.
    normalize_lufs(str(out))
    if emote and not take_ref:
        gain_db = WHISPER_GAIN_DB.get(emote.strip().lower())
        if gain_db:
            import numpy as np  # noqa: PLC0415

            data2, sr2 = sf.read(str(out), dtype="float32")
            if data2.ndim > 1:
                data2 = data2.mean(axis=1)
            gain = 10.0 ** (gain_db / 20.0)
            sf.write(str(out), (data2 * gain).astype(np.float32), sr2)
            data = data2 * gain
    duration_ms = int(round(len(data) / SAMPLE_RATE * 1000.0))
    resp = {"status": "ok", "duration_ms": duration_ms, "sample_rate": SAMPLE_RATE, "chunks": 1}
    if emote:
        resp["emote"] = emote
    return resp


def handle_register(req):
    name = req.get("name", "")
    audio_path = req.get("audio_path", "")
    overwrite = bool(req.get("overwrite", True))
    if not name or not audio_path:
        raise ValueError("register requires name, audio_path")
    src = Path(audio_path)
    if not src.exists():
        raise ValueError(f"reference audio not found: {audio_path}")
    VOICES_DIR.mkdir(parents=True, exist_ok=True)
    dst = _voice_path(name)
    if dst.exists() and not overwrite:
        return {"status": "ok", "voice": name, "skipped": True}
    shutil.copy2(str(src), str(dst))
    log(f"registered voice '{name}' -> {dst}")
    return {"status": "ok", "voice": name}


def handle_list():
    if not VOICES_DIR.exists():
        return {"status": "ok", "voices": []}
    names = sorted(p.stem for p in VOICES_DIR.glob("*.wav"))
    return {"status": "ok", "voices": names}


def handle_health():
    return {
        "status": "ok",
        "model_dir": str(MODEL_DIR),
        "voices_dir": str(VOICES_DIR),
        "model_loaded": _session is not None,
        "voices": sorted(p.stem for p in VOICES_DIR.glob("*.wav")) if VOICES_DIR.exists() else [],
    }


def _dispatch(req) -> dict:
    op = req.get("op", "synth")
    if op == "synth":
        return handle_synth(req)
    if op == "register":
        return handle_register(req)
    if op == "list":
        return handle_list()
    if op == "health":
        return handle_health()
    raise ValueError(f"unknown op: {op}")


def serve() -> int:
    proto = _isolate_streams()
    VOICES_DIR.mkdir(parents=True, exist_ok=True)
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
        except Exception as exc:  # protocol-level error -> structured response
            log(f"error handling {op!r}: {exc}")
            resp = {"status": "error", "error": str(exc)}
        _proto_write(proto, resp)
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description="IndexTTS-2.5 TTS sidecar")
    parser.add_argument("--serve", action="store_true",
                        help="Run as long-lived stdin/stdout server")
    parser.add_argument("--text", help="Text to synthesize")
    parser.add_argument("--voice", help="Registered voice id")
    parser.add_argument("--output", help="Output WAV path")
    args = parser.parse_args()
    if args.serve:
        return serve()
    # One-shot convenience mode (for manual smoke tests).
    if args.text and args.output:
        req = {"op": "synth", "text": args.text, "voice": args.voice or "",
               "output_path": args.output}
        print(json.dumps(handle_synth(req)))
        return 0
    parser.print_help(file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main())
