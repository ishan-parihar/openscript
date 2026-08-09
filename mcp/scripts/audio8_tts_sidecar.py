#!/usr/bin/env python3
"""Audio8 TTS sidecar — zero-shot voice cloning via the vendored Audio8 ONNX runtime.

Drives the official ONNX Runtime deployment vendored at
`third_party/Audio8_TTS/onnx_runtime` (Apache-2.0) with the INT4 model at
`mcp/assets/audio8/model`. Replaces Kokoro as the default TTS engine for
cloned (reference-audio) voices.

LONG-LIVED SERVE MODE (--serve):

    Loads the Slow AR / Fast AR / codec-decoder ONNX sessions ONCE (~1 GiB),
    then reads JSON requests from stdin (one per line) and writes JSON
    responses to stdout. Subsequent synth calls pay only inference cost —
    this is what script.generate_voices relies on for multi-scene runs.

PROTOCOL:

  → {"op":"synth","text":"Hello","voice":"ishan","max_new_tokens":1024,
     "temperature":0.7,"top_p":0.9,"top_k":50,"seed":42,"output_path":"/tmp/a.wav"}
  ← {"status":"ok","duration_ms":1234,"sample_rate":44100}

  → {"op":"register","name":"ishan","audio_path":"/abs/ref.wav","text":"Exact transcript.","overwrite":true}
  ← {"status":"ok","voice":"ishan","codes_shape":[10,241]}

  → {"op":"list"}
  ← {"status":"ok","voices":[{"name":"ishan","reference_text":"...","shape":[...]}]}

  → {"op":"health"}
  ← {"status":"ok","model_dir":"...","voices":[...]}

  On error:
  ← {"status":"error","error":"..."}

  On startup the sidecar prints `{"ready":true}` immediately and loads the
  model lazily on the first synth request (keeps MCP server startup fast).
  Errors during lazy load are reported in the synth response.

ENV:
  OPENSCRIPT_ROOT          repo root (defaults to script location + ../../)
  AUDIO8_MODEL_DIR         ONNX model dir (default <root>/mcp/assets/audio8/model)
  AUDIO8_VOICES_DIR        registered voice profiles (default <root>/mcp/assets/audio8/voices)
  AUDIO8_THREADS           ONNX CPU threads (default 5)
"""

import argparse
import gc
import json
import os
import sys
from pathlib import Path

# --- Resolve the vendored runtime -------------------------------------------
_SCRIPT_DIR = Path(__file__).resolve().parent
_ROOT = Path(os.environ.get("OPENSCRIPT_ROOT", _SCRIPT_DIR.parent.parent)).resolve()
_ONNX_RUNTIME = _ROOT / "third_party" / "Audio8_TTS" / "onnx_runtime"
if _ONNX_RUNTIME.exists():
    sys.path.insert(0, str(_ONNX_RUNTIME))
else:
    sys.path.insert(0, str(_ONNX_RUNTIME))  # will fail loudly on import — that's fine

MODEL_DIR = Path(os.environ.get("AUDIO8_MODEL_DIR", _ROOT / "mcp/assets/audio8/model")).resolve()
VOICES_DIR = Path(os.environ.get("AUDIO8_VOICES_DIR", _ROOT / "mcp/assets/audio8/voices")).resolve()
REGISTRATION_DIR = MODEL_DIR / "registration"
THREADS = int(os.environ.get("AUDIO8_THREADS", "5"))
SAMPLE_RATE = 44100


def log(msg: str) -> None:
    sys.stderr.write(f"[audio8_tts_sidecar] {msg}\n")
    sys.stderr.flush()


# --- Chunking ----------------------------------------------------------------
# The AR model stops generating at `max_new_tokens` (default 1024; the runtime
# manifest caps the sequence at max_seq_len=2048 and the generator stops early
# on the EOS token, so a high budget costs nothing for short text). The old 256
# budget allowed only ~11.9s of audio per call — a 207-char scene at natural
# speech rate (~17 c/s, ~1.25 AR tokens/char) needs the full 256, so any longer
# scene silently truncated the END of the sentence (words dropped) — the
# "TTS truncation" bug. Split on sentence boundaries into chunks safely under
# the token budget, synthesize each chunk with the same voice, and concatenate
# the audio. The model produces natural inter-sentence pauses, so no extra
# silence is inserted.

MAX_CHARS_PER_CHUNK = 600  # ~620-780 AR tokens at natural speech rates; well under the 1024 budget


def chunk_text(text: str, max_chars: int = MAX_CHARS_PER_CHUNK) -> list:
    """Split text into sentence-aligned chunks under max_chars.

    Splits on sentence terminators (. ! ? … ; and newlines), keeping each
    chunk under the budget. A single over-long sentence is split at comma/
    semicolon boundaries, then hard-cut at word boundaries (last resort).
    Returns a list of non-empty chunks.
    """
    import re

    if len(text) <= max_chars:
        return [text] if text.strip() else []

    sentences = re.split(r"(?<=[.!?…;])\s+|\n+", text.strip())
    chunks: list = []
    cur = ""

    def push(piece: str) -> None:
        nonlocal cur
        piece = piece.strip()
        if not piece:
            return
        if not cur:
            cur = piece
        else:
            cur = f"{cur} {piece}"

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
            push(sent)
            continue
        flush()
        if len(sent) <= max_chars:
            push(sent)
            continue
        # Over-long sentence: split at comma/semicolon boundaries first.
        parts = re.split(r"(?<=[,;:])\s+", sent)
        for part in parts:
            part = part.strip()
            if not part:
                continue
            if len(cur) + len(part) + 1 <= max_chars:
                push(part)
                continue
            flush()
            # Hard cut at word boundary, then raw char cut as last resort.
            while len(part) > max_chars:
                cut = part[:max_chars]
                if " " in cut:
                    cut = cut.rsplit(" ", 1)[0]
                chunks.append(cut.strip())
                part = part[len(cut):].lstrip()
            if part:
                push(part)
    flush()
    return chunks


# --- Runtime (lazy) ----------------------------------------------------------
_runtime = None  # ArkTtsRuntime — created on first synth, kept alive


def get_runtime():
    global _runtime
    if _runtime is None:
        from arktts_runtime.runtime import ArkTtsRuntime  # noqa: PLC0415

        _runtime = ArkTtsRuntime(MODEL_DIR, VOICES_DIR, precision="int4", codec_precision="fp16", threads=THREADS)
    return _runtime


def handle_synth(req):
    import numpy as np  # noqa: PLC0415
    import soundfile as sf  # noqa: PLC0415

    text = req.get("text", "")
    voice = req.get("voice", "")
    output_path = req.get("output_path", "")
    emotion = req.get("emotion") or None
    if not text or not voice or not output_path:
        raise ValueError("synth requires text, voice, output_path")
    # Audio8 emotion takes are pre-registered compound voices `{base}@{emotion}`
    # (the codec conditions on the reference at registration). A raw ref_audio
    # override is not supported here — if the caller passed one, the router
    # already resolved it to the compound voice id. Accept + ignore with a log.
    if req.get("ref_audio"):
        log(f"synth ref_audio override ignored for audio8 (voice='{voice}' used; "
            f"register emotion takes via voice.profile.add emotions)")
    try:
        runtime = get_runtime()
    except Exception as exc:  # model load failure
        raise RuntimeError(f"failed to load Audio8 runtime: {exc}") from exc

    # Chunk long text on sentence boundaries — the AR model truncates the
    # tail of any input that exceeds max_new_tokens (the "words dropped from
    # the end of the sentence" bug). Chunks keep timing/voice identical and
    # the audio is concatenated into a single WAV.
    chunks = chunk_text(text)
    if not chunks:
        raise ValueError("synth text produced no chunks")

    kwargs = dict(
        voice=voice,
        max_new_tokens=int(req.get("max_new_tokens", 1024)),
        temperature=float(req.get("temperature", 0.7)),
        top_p=float(req.get("top_p", 0.9)),
        top_k=int(req.get("top_k", 50)),
        seed=int(req.get("seed", 42)),
    )
    if len(chunks) == 1:
        audio, _codes = runtime.synthesize(text=chunks[0], **kwargs)
    else:
        log(f"synthesizing {len(chunks)} chunk(s) for {len(text)} chars "
            f"(max_new_tokens={kwargs['max_new_tokens']})")
        parts = []
        for chunk in chunks:
            part, _codes = runtime.synthesize(text=chunk, **kwargs)
            parts.append(np.asarray(part, dtype=np.float32))
        audio = np.concatenate(parts) if len(parts) > 1 else parts[0]
    out = Path(output_path)
    out.parent.mkdir(parents=True, exist_ok=True)
    sf.write(str(out), audio, SAMPLE_RATE)
    duration_ms = int(round(len(audio) / SAMPLE_RATE * 1000.0))
    resp = {"status": "ok", "duration_ms": duration_ms, "sample_rate": SAMPLE_RATE, "chunks": len(chunks)}
    if emotion:
        resp["emotion"] = emotion
    return resp


def handle_register(req):
    from arktts_runtime.registration import VoiceRegistration  # noqa: PLC0415

    name = req.get("name", "")
    audio_path = req.get("audio_path", "")
    text = req.get("text", "")
    overwrite = bool(req.get("overwrite", True))
    if not name or not audio_path or not text:
        raise ValueError("register requires name, audio_path, text")
    data = Path(audio_path).read_bytes()
    fingerprint = json.loads((MODEL_DIR / "runtime_manifest.json").read_text())["model_fingerprint"]
    reg = VoiceRegistration(REGISTRATION_DIR, VOICES_DIR, fingerprint)
    meta = reg.register(data, Path(audio_path).name, text, name, overwrite)
    gc.collect()
    return {"status": "ok", "voice": name, "codes_shape": list(meta["shape"])}


def handle_list(_req):
    try:
        from arktts_runtime.voices import VoiceStore  # noqa: PLC0415

        voices = VoiceStore(VOICES_DIR, 10).list()
    except Exception:
        voices = []
    return {"status": "ok", "voices": voices}


def handle_health(_req):
    manifest = MODEL_DIR / "runtime_manifest.json"
    return {
        "status": "ok",
        "model_dir": str(MODEL_DIR),
        "model_present": manifest.exists(),
        "voices_dir": str(VOICES_DIR),
        "runtime_loaded": _runtime is not None,
        "sample_rate": SAMPLE_RATE,
    }


def serve() -> int:
    log(f"ready (model_dir={MODEL_DIR}, voices_dir={VOICES_DIR}, threads={THREADS})")
    print(json.dumps({"ready": True}), flush=True)
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
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
    parser = argparse.ArgumentParser(description="Audio8 TTS sidecar (long-lived serve mode)")
    parser.add_argument("--serve", action="store_true", help="Run as long-lived stdin/stdout server")
    parser.add_argument("--text", help="Text to synthesize (fresh-process mode)")
    parser.add_argument("--voice", help="Voice profile name")
    parser.add_argument("--output", help="Output WAV path")
    parser.add_argument("--max-new-tokens", type=int, default=1024)
    args = parser.parse_args()

    if args.serve:
        return serve()
    if args.text and args.voice and args.output:
        resp = handle_synth(
            {
                "text": args.text,
                "voice": args.voice,
                "output_path": args.output,
                "max_new_tokens": args.max_new_tokens,
            }
        )
        print(json.dumps(resp, ensure_ascii=False))
        return 0
    print("usage: audio8_tts_sidecar.py --serve   |   --text T --voice V --output OUT", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main())
