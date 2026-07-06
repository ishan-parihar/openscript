#!/usr/bin/env python3
"""Kokoro TTS sidecar — synthesizes speech from text using the kokoro-onnx package.

TWO MODES:

  1. Fresh-process (legacy, default):

     python3 kokoro_tts_sidecar.py --text "Hello world" --voice af_heart \
         --speed 1.0 \
         --model mcp/assets/kokoro/onnx/kokoro-v1.0.onnx \
         --voices mcp/assets/kokoro/voices/voices-v1.0.bin \
         --output /tmp/output.wav

     One process per call. Pays ~360ms cold-start per invocation (Python
     startup + kokoro_onnx import + ONNX model load + voices load).

  2. Long-lived serve mode (new, --serve):

     python3 kokoro_tts_sidecar.py --serve \
         --model mcp/assets/kokoro/onnx/kokoro-v1.0.onnx \
         --voices mcp/assets/kokoro/voices/voices-v1.0.bin

     Loads the ONNX model ONCE, then reads JSON requests from stdin (one
     per line) and writes JSON responses to stdout. Subsequent synth calls
     pay only the inference cost (~150ms for a short chunk), eliminating
     the cold-start penalty for multi-scene scripts (40 chunks × 360ms =
     14.4s saved on a typical 20-scene run).

PROTOCOL (serve mode only):

  → {"text":"Hello world","voice":"af_heart","speed":1.0,"output_path":"/tmp/foo.wav"}
  ← {"status":"ok","duration_ms":1234,"sample_rate":24000}

  On error:
  ← {"status":"error","error":"synthesis failed: ..."}

  On startup, the sidecar prints `{"ready":true}` once the model is loaded
  and before reading the first request. The Rust side waits for this
  signal before sending the first request.

Model + voices download:
    curl -L -o mcp/assets/kokoro/onnx/kokoro-v1.0.onnx \
        https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/kokoro-v1.0.onnx
    curl -L -o mcp/assets/kokoro/voices/voices-v1.0.bin \
        https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/voices-v1.0.bin
"""

import argparse
import json
import sys
import numpy as np
from kokoro_onnx import Kokoro
import wave


def synth_to_wav(kokoro, text, voice, speed, output_path):
    """Synthesise one chunk and write a 16-bit PCM WAV to output_path.

    Returns (duration_ms, sample_rate) on success, raises on error. Shared
    by both the fresh-process and serve modes so the WAV encoding logic is
    not duplicated.
    """
    samples, sample_rate = kokoro.create(text, voice=voice, speed=speed)

    # Convert to 16-bit PCM WAV
    samples = np.array(samples, dtype=np.float32)
    # Normalize to prevent clipping
    max_val = np.max(np.abs(samples))
    if max_val > 0:
        samples = samples / max_val * 0.95
    samples_int16 = (samples * 32767).astype(np.int16)

    with wave.open(output_path, 'w') as wav_file:
        wav_file.setnchannels(1)
        wav_file.setsampwidth(2)
        wav_file.setframerate(sample_rate)
        wav_file.writeframes(samples_int16.tobytes())

    duration_ms = int(len(samples) / sample_rate * 1000)
    return duration_ms, sample_rate


def run_fresh(args):
    """Legacy one-shot mode: load model, synth one chunk, exit."""
    try:
        kokoro = Kokoro(args.model, args.voices)
    except Exception as e:
        print(f"ERROR: Failed to load Kokoro model: {e}", file=sys.stderr)
        sys.exit(1)

    try:
        duration_ms, sample_rate = synth_to_wav(
            kokoro, args.text, args.voice, args.speed, args.output
        )
    except Exception as e:
        print(f"ERROR: synthesis failed: {e}", file=sys.stderr)
        sys.exit(1)

    print(f"OK: {args.output} ({duration_ms}ms, {sample_rate}Hz)")


def run_serve(args):
    """Long-lived mode: load model once, loop on stdin/stdout JSON."""
    try:
        kokoro = Kokoro(args.model, args.voices)
    except Exception as e:
        # Print an error JSON so the Rust side can read it from stdout
        # (which it scans for the ready signal). Also exit non-zero so
        # stderr-based diagnostics work too.
        print(json.dumps({"ready": False, "error": f"Failed to load Kokoro model: {e}"}))
        sys.stderr.write(f"ERROR: Failed to load Kokoro model: {e}\n")
        sys.exit(1)

    # Signal readiness. The Rust side blocks on reading this line before
    # sending the first synth request.
    print(json.dumps({"ready": True}), flush=True)

    # Loop: read one JSON request per line, write one JSON response per line.
    # On EOF or fatal error, exit. On per-request error, respond with
    # status=error and keep looping (a single bad chunk should not kill
    # the sidecar and force a re-load).
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue

        try:
            req = json.loads(line)
        except json.JSONDecodeError as e:
            print(json.dumps({"status": "error", "error": f"invalid JSON: {e}"}), flush=True)
            continue

        text = req.get("text", "")
        voice = req.get("voice", "af_heart")
        speed = float(req.get("speed", 1.0))
        output_path = req.get("output_path", "")

        if not text or not output_path:
            print(json.dumps({
                "status": "error",
                "error": "missing required field: text or output_path",
            }), flush=True)
            continue

        try:
            duration_ms, sample_rate = synth_to_wav(
                kokoro, text, voice, speed, output_path
            )
            print(json.dumps({
                "status": "ok",
                "duration_ms": duration_ms,
                "sample_rate": sample_rate,
            }), flush=True)
        except Exception as e:
            print(json.dumps({
                "status": "error",
                "error": f"synthesis failed: {e}",
            }), flush=True)


def main():
    parser = argparse.ArgumentParser(description="Kokoro TTS sidecar")
    parser.add_argument("--serve", action="store_true",
                        help="Long-lived serve mode (stdin/stdout JSON protocol)")
    parser.add_argument("--text", default=None, help="Text to synthesize (fresh mode only)")
    parser.add_argument("--voice", default="af_heart", help="Voice name (e.g. af_heart)")
    parser.add_argument("--speed", type=float, default=1.0, help="Speech speed multiplier")
    parser.add_argument("--model", required=True, help="Path to ONNX model file")
    parser.add_argument("--voices", required=True, help="Path to voices .bin file")
    parser.add_argument("--output", default=None, help="Output WAV file path (fresh mode only)")
    args = parser.parse_args()

    if args.serve:
        run_serve(args)
    else:
        # Fresh mode requires --text and --output.
        if args.text is None or args.output is None:
            parser.error("--text and --output are required in fresh mode (or use --serve)")
        run_fresh(args)


if __name__ == "__main__":
    main()
