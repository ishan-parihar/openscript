#!/usr/bin/env python3
"""Gepard TTS sidecar — zero-shot voice cloning via Gepard 1.0 (Qwen3.5 AR + NeMo NanoCodec).

Drives the reference inference stack vendored at
`third_party/gepard-inference` (Apache-2.0; the NeMo NanoCodec it loads at
runtime is covered by the NVIDIA Open Model License Agreement). Gepard 1.0 is
the high-quality native-English cloned-voice engine: a decoder-only Qwen3.5
backbone predicts FSQ audio codes and the NanoCodec decodes them to 22.05 kHz
speech. Voice cloning extracts a speaker prefix ONCE at prefill via a Q-Former
compressor — cloning adds zero per-word inference cost.

LONG-LIVED SERVE MODE (--serve):

    Loads the GepardSession (model + codec, ~2.5 GB) ONCE, then reads JSON
    requests from stdin (one per line) and writes JSON responses to stdout.
    Prints `{"ready":true}` immediately and loads the model lazily on the
    first synth request (keeps MCP server startup fast).

PROTOCOL (mirrors audio8_tts_sidecar.py):

  → {"op":"synth","text":"Hello","voice":"ishan","output_path":"/tmp/a.wav",
     "temperature":0.3,"cfg_scale":1.0}
  ← {"status":"ok","duration_ms":1234,"sample_rate":22050}

  → {"op":"register","name":"ishan","audio_path":"/abs/ref.wav","text":"Exact transcript.","overwrite":true}
  ← {"status":"ok","voice":"ishan"}

  → {"op":"list"}
  ← {"status":"ok","voices":[{"name":"ishan","ref_text":"..."}]}

  → {"op":"health"}
  ← {"status":"ok","checkpoint":"...","model_loaded":true,"voices":[...]}

  On error:
  ← {"status":"error","error":"..."}

ENV:
  GEPARD_PYTHON        venv python (the Rust side resolves it; run this script
                       with the .venv-gepard interpreter so torch/transformers
                       resolve — the venv has `gepard` installed -e from
                       third_party/gepard-inference).
  GEPARD_CHECKPOINT    HF repo id or local dir (default nineninesix/gepard-1.0,
                       not gated — no HF token required).
  GEPARD_VOICES_DIR    registered reference voices (default <root>/mcp/assets/gepard/voices)
  GEPARD_DEVICE        auto|cuda|cpu (default auto)
  GEPARD_MAX_REF_SECONDS  reference clip truncation (default 30)
  OPENSCRIPT_ROOT      repo root (defaults to script location + ../../)
"""

import argparse
import json
import os
import shutil
import sys
from pathlib import Path

# --- Resolve the vendored inference package ----------------------------------
_SCRIPT_DIR = Path(__file__).resolve().parent
_ROOT = Path(os.environ.get("OPENSCRIPT_ROOT", _SCRIPT_DIR.parent.parent)).resolve()
_VENDORED = _ROOT / "third_party" / "gepard-inference"
if _VENDORED.exists():
    sys.path.insert(0, str(_VENDORED))

CHECKPOINT = os.environ.get("GEPARD_CHECKPOINT", "nineninesix/gepard-1.0")
VOICES_DIR = Path(
    os.environ.get("GEPARD_VOICES_DIR", _ROOT / "mcp/assets/gepard/voices")
).resolve()
MAX_REF_SECONDS = float(os.environ.get("GEPARD_MAX_REF_SECONDS", "30"))
SAMPLE_RATE = 22050

# Generation defaults — match the reference config.yaml (cfg_scale=1.0 =
# single-pass DPO-distilled generation, the production default).
GEN_DEFAULTS = {
    "temperature": 0.3,
    "top_k": 0,
    "cfg_scale": 1.0,
    "cfg_frames": None,
    "stop_threshold": 0.5,
    "max_frames": 2000,
    "repetition_penalty": 1.0,
    "repetition_window": 32,
}

# Guardrail: Gepard's stop head (updated 2026-08-06) carries multi-sentence
# inputs to the end, and max_frames=2000 ≈ 93 s of audio — far beyond any
# scene. Only texts that could plausibly exceed that are chunked on sentence
# boundaries (mirrors the Audio8 truncation fix; chunks concatenate cleanly).
MAX_CHARS_PER_CHUNK = 1500


def log(msg: str) -> None:
    sys.stderr.write(f"[gepard_tts_sidecar] {msg}\n")
    sys.stderr.flush()


def chunk_text(text: str, max_chars: int = MAX_CHARS_PER_CHUNK) -> list:
    """Split text into sentence-aligned chunks under max_chars (safety net)."""
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
        # Over-long sentence: split at comma/semicolon, then hard word cut.
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


# --- Runtime (lazy) ----------------------------------------------------------
_session = None  # GepardSession — created on first synth, kept alive


def _resolve_device() -> str:
    dev = os.environ.get("GEPARD_DEVICE", "auto").strip().lower()
    if dev in ("cuda", "cpu"):
        return dev
    try:
        import torch  # noqa: PLC0415
        return "cuda" if torch.cuda.is_available() else "cpu"
    except Exception:
        return "cpu"


def get_session():
    """Lazily build the GepardSession (model + codec). ~2.5 GB on first load."""
    global _session
    if _session is None:
        from gepard_inference.session import SessionConfig, GepardSession  # noqa: PLC0415

        device = _resolve_device()
        log(f"loading checkpoint {CHECKPOINT} on {device} (first load can take minutes)")
        cfg = SessionConfig(
            checkpoint=CHECKPOINT,
            attn_implementation="eager",
            defaults={
                k: v for k, v in GEN_DEFAULTS.items()
                if k != "cfg_frames"  # cfg_frames handled below
            },
            reference_audio=None,  # per-voice references only; no global default
            max_ref_seconds=MAX_REF_SECONDS,
            root=_ROOT,
        )
        _session = GepardSession(cfg, device=device).load()
        log("GepardSession ready")
    return _session


def _voice_path(name: str) -> Path:
    # Names are profile ids; sanitize to avoid path traversal.
    safe = "".join(c for c in name if c.isalnum() or c in "-_.").strip(".")
    if not safe:
        raise ValueError(f"invalid voice name: {name!r}")
    return VOICES_DIR / f"{safe}.wav"


def handle_synth(req):
    import numpy as np  # noqa: PLC0415
    import soundfile as sf  # noqa: PLC0415

    text = req.get("text", "")
    voice = req.get("voice", "")
    output_path = req.get("output_path", "")
    if not text or not voice or not output_path:
        raise ValueError("synth requires text, voice, output_path")

    ref = _voice_path(voice)
    if not ref.exists():
        raise ValueError(
            f"no registered gepard voice '{voice}' (expected {ref}). "
            f"Register it first via voice.profile.add with provider=gepard."
        )

    session = get_session()

    # Per-request sampling overrides (fall back to GEN_DEFAULTS).
    overrides = {}
    for key, cast in (
        ("temperature", float), ("cfg_scale", float),
        ("stop_threshold", float), ("repetition_penalty", float),
        ("top_k", int), ("max_frames", int), ("repetition_window", int),
    ):
        if req.get(key) is not None:
            overrides[key] = cast(req[key])
    if req.get("cfg_frames") is not None:
        cf = int(req["cfg_frames"])
        overrides["cfg_frames"] = cf if cf > 0 else None

    chunks = chunk_text(text)
    if not chunks:
        raise ValueError("synth text produced no chunks")

    parts = []
    for chunk in chunks:
        sr, wave = session.synthesize(chunk, reference=str(ref), **overrides)
        if len(chunks) > 1:
            log(f"chunk ({len(chunk)} chars) -> {len(wave) / sr:.2f}s")
        parts.append(np.asarray(wave, dtype=np.float32))
    audio = np.concatenate(parts) if len(parts) > 1 else parts[0]
    sr = sr or SAMPLE_RATE

    out = Path(output_path)
    out.parent.mkdir(parents=True, exist_ok=True)
    sf.write(str(out), audio, sr)
    duration_ms = int(round(len(audio) / sr * 1000.0))
    return {"status": "ok", "duration_ms": duration_ms, "sample_rate": sr, "chunks": len(chunks)}


def handle_register(req):
    name = req.get("name", "")
    audio_path = req.get("audio_path", "")
    ref_text = req.get("text", "")
    overwrite = bool(req.get("overwrite", True))
    if not name or not audio_path:
        raise ValueError("register requires name, audio_path")

    src = Path(audio_path)
    if not src.exists():
        raise ValueError(f"reference audio not found: {audio_path}")
    if src.stat().st_size == 0:
        raise ValueError(f"reference audio is empty: {audio_path}")

    dst = _voice_path(name)
    VOICES_DIR.mkdir(parents=True, exist_ok=True)
    if dst.exists() and not overwrite:
        raise ValueError(f"voice '{name}' already registered (overwrite=false)")
    shutil.copy2(src, dst)

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
        name = f.stem
        voices.append({"name": name, "ref_path": str(f)})
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
        "checkpoint": CHECKPOINT,
        "voices_dir": str(VOICES_DIR),
        "model_loaded": _session is not None,
        "sample_rate": SAMPLE_RATE,
        "voices": voices,
    }


def serve() -> int:
    log(f"ready (checkpoint={CHECKPOINT}, voices_dir={VOICES_DIR})")
    print(json.dumps({"ready": True}), flush=True)
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        op = "synth"
        try:
            req = json.loads(line)
            op = req.get("op", "synth")
            if op == "synth":
                resp = handle_synth(req)
            elif op == "register":
                resp = handle_register(req)
            elif op == "list":
                resp = handle_list(req)
            elif op == "health":
                resp = handle_health(req)
            else:
                raise ValueError(f"unknown op: {op}")
        except Exception as exc:  # protocol-level error → structured response
            log(f"error handling {op!r}: {exc}")
            resp = {"status": "error", "error": str(exc)}
        print(json.dumps(resp, ensure_ascii=False), flush=True)
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description="Gepard TTS sidecar (long-lived serve mode)")
    parser.add_argument("--serve", action="store_true", help="Run as long-lived stdin/stdout server")
    parser.add_argument("--text", help="Text to synthesize (fresh-process mode)")
    parser.add_argument("--voice", help="Voice profile name")
    parser.add_argument("--output", help="Output WAV path")
    args = parser.parse_args()

    if args.serve:
        return serve()
    if args.text and args.voice and args.output:
        resp = handle_synth({"text": args.text, "voice": args.voice, "output_path": args.output})
        print(json.dumps(resp, ensure_ascii=False))
        return 0
    print("usage: gepard_tts_sidecar.py --serve   |   --text T --voice V --output OUT", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main())
