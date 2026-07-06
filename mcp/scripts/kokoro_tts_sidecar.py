#!/usr/bin/env python3
"""Kokoro TTS sidecar — synthesizes speech from text using the kokoro-onnx package.

Usage:
    python3 kokoro_tts_sidecar.py --text "Hello world" --voice af_heart --speed 1.0 \
        --model mcp/assets/kokoro/onnx/kokoro-v1.0.onnx \
        --voices mcp/assets/kokoro/voices/voices-v1.0.bin \
        --output /tmp/output.wav

Outputs a 24kHz mono WAV file at the specified path.

Model + voices download:
    curl -L -o mcp/assets/kokoro/onnx/kokoro-v1.0.onnx \
        https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/kokoro-v1.0.onnx
    curl -L -o mcp/assets/kokoro/voices/voices-v1.0.bin \
        https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/voices-v1.0.bin
"""

import argparse
import sys
import numpy as np
from kokoro_onnx import Kokoro
import wave


def main():
    parser = argparse.ArgumentParser(description="Kokoro TTS sidecar")
    parser.add_argument("--text", required=True, help="Text to synthesize")
    parser.add_argument("--voice", default="af_heart", help="Voice name (e.g. af_heart)")
    parser.add_argument("--speed", type=float, default=1.0, help="Speech speed multiplier")
    parser.add_argument("--model", required=True, help="Path to ONNX model file")
    parser.add_argument("--voices", required=True, help="Path to voices .bin file")
    parser.add_argument("--output", required=True, help="Output WAV file path")
    args = parser.parse_args()

    try:
        kokoro = Kokoro(args.model, args.voices)
    except Exception as e:
        print(f"ERROR: Failed to load Kokoro model: {e}", file=sys.stderr)
        sys.exit(1)

    try:
        samples, sample_rate = kokoro.create(args.text, voice=args.voice, speed=args.speed)
    except Exception as e:
        print(f"ERROR: synthesis failed: {e}", file=sys.stderr)
        sys.exit(1)

    # Convert to 16-bit PCM WAV
    samples = np.array(samples, dtype=np.float32)
    # Normalize to prevent clipping
    max_val = np.max(np.abs(samples))
    if max_val > 0:
        samples = samples / max_val * 0.95
    samples_int16 = (samples * 32767).astype(np.int16)

    with wave.open(args.output, 'w') as wav_file:
        wav_file.setnchannels(1)
        wav_file.setsampwidth(2)
        wav_file.setframerate(sample_rate)
        wav_file.writeframes(samples_int16.tobytes())

    duration_ms = int(len(samples) / sample_rate * 1000)
    print(f"OK: {args.output} ({duration_ms}ms, {sample_rate}Hz)")


if __name__ == "__main__":
    main()
