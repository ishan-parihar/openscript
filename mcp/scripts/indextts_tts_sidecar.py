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

    # Emotion channel: a take ref clip wins (emo_audio_prompt + emo_alpha);
    # else emo_text guidance (needs QwenEmo); else neutral.
    emo_alpha = float(req.get("emo_alpha", 1.0)) if req.get("emo_alpha") is not None else 1.0
    take_ref = req.get("emo_audio_prompt") or None  # explicit emotion-take clip
    emo_text = req.get("emo_text") or None
    if emote and not take_ref and not emo_text:
        emo_text = EMOTION_TEXT.get(emote.strip().lower())

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
        f"emo_text={emo_text!r} take={take_ref!r} dur_factor={duration_factor} "
        f"temp={gen_kwargs['temperature']}")
    try:
        session.infer(
            spk_audio_prompt=str(ref),
            text=text,
            output_path=tmp_out,
            lang="EN",
            emo_audio_prompt=take_ref,
            emo_alpha=emo_alpha,
            use_emo_text=emo_text is not None,
            emo_text=emo_text,
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
    # Uniform per-scene loudness (matches the audio8/voicedesign sidecars).
    normalize_lufs(str(out))
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
