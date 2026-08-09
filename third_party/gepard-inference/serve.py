"""GEPARD TTS server — load the model once, synthesize over HTTP.

Run it in one terminal; fire requests from another (cli.py, curl, or the
interactive docs at ``/docs``). The model + codec stay in memory, so every
request is just autoregressive decoding.

    python serve.py                       # 127.0.0.1:8000, defaults from config.yaml
    python serve.py --host 0.0.0.0 --port 8080 --reference ref_audio/audio_en.wav
"""

from __future__ import annotations

import argparse
import io
from typing import Optional

import soundfile as sf
from fastapi import FastAPI, HTTPException, Response
from pydantic import BaseModel

from gepard_inference import GepardSession, SessionConfig

app = FastAPI(
    title="GEPARD TTS",
    description="Open-source autoregressive text-to-speech. POST /synthesize → WAV.",
)

# Populated by main() before the server starts serving.
_session: Optional[GepardSession] = None


class SynthesizeRequest(BaseModel):
    """A synthesis request. Only ``text`` is required; everything else falls
    back to the server's config defaults when omitted."""

    text: str
    reference: Optional[str] = None  # audio path to clone; omit → default voice
    temperature: Optional[float] = None
    top_k: Optional[int] = None
    cfg_scale: Optional[float] = None
    cfg_frames: Optional[int] = None
    stop_threshold: Optional[float] = None
    max_frames: Optional[int] = None
    repetition_penalty: Optional[float] = None
    repetition_window: Optional[int] = None


@app.get("/")
def info():
    """Server status and the active defaults."""
    if _session is None:
        raise HTTPException(503, "session not loaded")
    cfg = _session.config
    return {
        "status": "ready",
        "checkpoint": cfg.checkpoint,
        "device": _session.device,
        "default_voice": cfg.reference_audio,
        "defaults": cfg.defaults,
    }


@app.post(
    "/synthesize",
    responses={200: {"content": {"audio/wav": {}}, "description": "WAV audio"}},
)
def synthesize(req: SynthesizeRequest):
    """Synthesize speech and return a WAV file."""
    if _session is None:
        raise HTTPException(503, "session not loaded")
    overrides = req.model_dump(exclude={"text", "reference"}, exclude_none=True)
    try:
        sr, wave = _session.synthesize(req.text, reference=req.reference, **overrides)
    except (ValueError, FileNotFoundError) as e:
        raise HTTPException(400, str(e))

    buf = io.BytesIO()
    sf.write(buf, wave, sr, format="WAV")
    return Response(content=buf.getvalue(), media_type="audio/wav")


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="gepard-serve", description="GEPARD TTS server.")
    p.add_argument("--config", default="config.yaml", help="Defaults file (default: config.yaml).")
    p.add_argument("--host", default="127.0.0.1", help="Bind host (default: 127.0.0.1).")
    p.add_argument("--port", type=int, default=8000, help="Bind port (default: 8000).")
    p.add_argument("--device", default=None, help="'cuda', 'cpu', or omit for auto-detect.")
    p.add_argument("--checkpoint", default=None, help="Override the config's checkpoint.")
    p.add_argument("--reference", default=None, help="Override the config's default voice clip.")
    return p


def main(argv=None) -> int:
    import uvicorn

    args = build_parser().parse_args(argv)

    cfg = SessionConfig.from_yaml(args.config)
    if args.checkpoint:
        cfg.checkpoint = args.checkpoint
    if args.reference:
        cfg.reference_audio = args.reference

    global _session
    _session = GepardSession(cfg, device=args.device).load()

    print(f"[serve] http://{args.host}:{args.port}  (docs at /docs)")
    uvicorn.run(app, host=args.host, port=args.port)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
