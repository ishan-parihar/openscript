"""GEPARD TTS client — send text to a running server, save the WAV.

Start the server first (``python serve.py`` / ``make serve``), then from any
terminal:

    python cli.py "Hi there! Great to finally meet you." -o out.wav
    python cli.py "Hello" -o out.wav --reference ref_audio/audio_en.wav --cfg-scale 3

Only ``text`` is required; omitted generation flags fall back to the server's
config defaults. Uses the stdlib only (no extra client dependencies).
"""

from __future__ import annotations

import argparse
import json
import sys
import urllib.error
import urllib.request

DEFAULT_SERVER = "http://127.0.0.1:8000"


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="gepard-tts", description="GEPARD text-to-speech client.")
    p.add_argument("text", help="Text to synthesize.")
    p.add_argument("-o", "--output", default="output.wav", help="Output WAV path (default: output.wav).")
    p.add_argument("-r", "--reference", default=None,
                   help="Reference voice clip to clone (path on the server). "
                        "Omit to use the server's default voice.")
    p.add_argument("-s", "--server", default=DEFAULT_SERVER,
                   help=f"Server base URL (default: {DEFAULT_SERVER}).")

    g = p.add_argument_group("generation (override server defaults)")
    g.add_argument("--temperature", type=float)
    g.add_argument("--top-k", type=int)
    g.add_argument("--cfg-scale", type=float, help="Text CFG weight; 1.0 = off.")
    g.add_argument("--cfg-frames", type=int, help="Guide only the first N frames.")
    g.add_argument("--stop-threshold", type=float)
    g.add_argument("--max-frames", type=int)
    g.add_argument("--repetition-penalty", type=float)
    g.add_argument("--repetition-window", type=int)
    return p


def main(argv=None) -> int:
    args = build_parser().parse_args(argv)

    payload = {"text": args.text}
    if args.reference:
        payload["reference"] = args.reference
    for key in ("temperature", "top_k", "cfg_scale", "cfg_frames", "stop_threshold",
                "max_frames", "repetition_penalty", "repetition_window"):
        val = getattr(args, key)
        if val is not None:
            payload[key] = val

    req = urllib.request.Request(
        args.server.rstrip("/") + "/synthesize",
        data=json.dumps(payload).encode(),
        headers={"Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(req) as resp:
            wav = resp.read()
    except urllib.error.HTTPError as e:
        detail = e.read().decode(errors="replace")
        print(f"error: server returned {e.code}: {detail}", file=sys.stderr)
        return 1
    except urllib.error.URLError as e:
        print(f"error: cannot reach server at {args.server} — is it running "
              f"(`make serve`)?  ({e.reason})", file=sys.stderr)
        return 1

    with open(args.output, "wb") as f:
        f.write(wav)
    print(f"✓ wrote {args.output}  ({len(wav) / 1024:.0f} KB)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
