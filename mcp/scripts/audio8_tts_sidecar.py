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

  → {"op":"synth","text":"Hello","voice":"ishan","max_new_tokens":256,
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


# --- Runtime (lazy) ----------------------------------------------------------
_runtime = None  # ArkTtsRuntime — created on first synth, kept alive


def get_runtime():
    global _runtime
    if _runtime is None:
        from arktts_runtime.runtime import ArkTtsRuntime  # noqa: PLC0415

        _runtime = ArkTtsRuntime(MODEL_DIR, VOICES_DIR, precision="int4", codec_precision="fp16", threads=THREADS)
    return _runtime


def handle_synth(req):
    import soundfile as sf  # noqa: PLC0415

    text = req.get("text", "")
    voice = req.get("voice", "")
    output_path = req.get("output_path", "")
    if not text or not voice or not output_path:
        raise ValueError("synth requires text, voice, output_path")
    try:
        runtime = get_runtime()
    except Exception as exc:  # model load failure
        raise RuntimeError(f"failed to load Audio8 runtime: {exc}") from exc

    audio, _codes = runtime.synthesize(
        text=text,
        voice=voice,
        max_new_tokens=int(req.get("max_new_tokens", 256)),
        temperature=float(req.get("temperature", 0.7)),
        top_p=float(req.get("top_p", 0.9)),
        top_k=int(req.get("top_k", 50)),
        seed=int(req.get("seed", 42)),
    )
    out = Path(output_path)
    out.parent.mkdir(parents=True, exist_ok=True)
    sf.write(str(out), audio, SAMPLE_RATE)
    duration_ms = int(round(len(audio) / SAMPLE_RATE * 1000.0))
    return {"status": "ok", "duration_ms": duration_ms, "sample_rate": SAMPLE_RATE}


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
    parser.add_argument("--max-new-tokens", type=int, default=256)
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
