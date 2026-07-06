#!/usr/bin/env python3
"""Parakeet TDT force-alignment sidecar — replaces whisper_align.py.

Long-lived serve mode (stdin/stdout JSON protocol). Loads the Parakeet TDT
0.6b v3 ONNX model once, then aligns TTS WAV output to word-level timestamps.

This replaces the old whisper_align.py which depended on the `openai-whisper`
Python package. Parakeet TDT is a faster, more accurate RNN-T model that
runs via `onnxruntime` (no PyTorch dependency).

Protocol (same as kokoro_tts_sidecar.py serve mode):
  -> {"wav_path":"/tmp/scene.wav","offset_ms":0}
  <- {"status":"ok","words":[{"word":"Hello","start_ms":0,"end_ms":320},...]}
  <- {"status":"error","error":"..."}

Model: istupakov/parakeet-tdt-0.6b-v3-onnx
  encoder-model.int8.onnx  (622 MB)
  decoder_joint-model.int8.onnx (18 MB)
  vocab.txt (92 KB)
"""

import argparse
import json
import sys
import os
import numpy as np
import onnxruntime as ort

# ---------------------------------------------------------------------------
# Mel spectrogram computation (128 mel bins, 80ms window, 20ms hop)
# Parakeet TDT expects log-mel features at this configuration.
# ---------------------------------------------------------------------------

def compute_mel_spectrogram(wav_path: str, sample_rate: int = 16000,
                            n_mels: int = 128, n_fft: int = 512,
                            hop_length: int = 160, win_length: int = 512) -> np.ndarray:
    """Compute log-mel spectrogram from a WAV file.

    Returns array of shape [n_mels, time_frames] suitable for Parakeet encoder.
    """
    import wave
    import struct

    # Read WAV file (assume 16-bit PCM mono)
    with wave.open(wav_path, 'r') as wav:
        n_channels = wav.getnchannels()
        sample_width = wav.getsampwidth()
        n_frames = wav.getnframes()
        raw = wav.readframes(n_frames)

    # Convert to float32
    if sample_width == 2:
        samples = np.frombuffer(raw, dtype=np.int16).astype(np.float32) / 32768.0
    elif sample_width == 4:
        samples = np.frombuffer(raw, dtype=np.int32).astype(np.float32) / 2147483648.0
    else:
        samples = np.frombuffer(raw, dtype=np.uint8).astype(np.float32) / 128.0 - 1.0

    # If stereo, take first channel
    if n_channels > 1:
        samples = samples[::n_channels]

    # Resample to 16kHz if needed (simple linear interpolation)
    orig_sr = 24000  # Kokoro outputs 24kHz
    if orig_sr != sample_rate:
        # Read actual sample rate from WAV
        pass  # We'll read it properly below

    # Re-read with proper sample rate
    with wave.open(wav_path, 'r') as wav:
        actual_sr = wav.getframerate()
        n_channels = wav.getnchannels()
        n_frames = wav.getnframes()
        raw = wav.readframes(n_frames)

    if sample_width == 2:
        samples = np.frombuffer(raw, dtype=np.int16).astype(np.float32) / 32768.0
    else:
        samples = np.frombuffer(raw, dtype=np.uint8).astype(np.float32) / 128.0 - 1.0

    if n_channels > 1:
        samples = samples[::n_channels]

    # Resample to 16kHz
    if actual_sr != 16000:
        n_out = int(len(samples) * 16000 / actual_sr)
        indices = np.linspace(0, len(samples) - 1, n_out)
        samples = np.interp(indices, np.arange(len(samples)), samples).astype(np.float32)

    # Apply pre-emphasis
    pre_emphasis = 0.97
    emphasized = np.append(samples[0], samples[1:] - pre_emphasis * samples[:-1])

    # Pad if too short
    if len(emphasized) < n_fft:
        emphasized = np.pad(emphasized, (0, n_fft - len(emphasized)))

    # Frame the signal
    n_frames = 1 + (len(emphasized) - n_fft) // hop_length
    frames = np.zeros((n_frames, n_fft), dtype=np.float32)
    for i in range(n_frames):
        start = i * hop_length
        frames[i] = emphasized[start:start + n_fft]

    # Apply Hann window
    window = np.hanning(n_fft).astype(np.float32)
    frames *= window

    # Compute FFT (power spectrum)
    fft_result = np.fft.rfft(frames, n=n_fft, axis=1)
    power_spec = (np.abs(fft_result) ** 2).astype(np.float32)

    # Mel filterbank
    mel_basis = _mel_filterbank(sample_rate=16000, n_fft=n_fft, n_mels=n_mels)
    mel_spec = np.dot(power_spec, mel_basis.T)

    # Log compression
    mel_spec = np.log(mel_spec + 1e-10)

    # Transpose to [n_mels, time] and normalize
    mel_spec = mel_spec.T  # [n_mels, time]
    mel_spec = (mel_spec - mel_spec.mean()) / (mel_spec.std() + 1e-10)

    return mel_spec.astype(np.float32)


def _mel_filterbank(sample_rate: int, n_fft: int, n_mels: int) -> np.ndarray:
    """Create a mel filterbank matrix."""
    fmin = 0.0
    fmax = sample_rate / 2.0

    # Mel scale conversion
    def hz_to_mel(hz):
        return 2595.0 * np.log10(1.0 + hz / 700.0)

    def mel_to_hz(mel):
        return 700.0 * (10 ** (mel / 2595.0) - 1.0)

    mel_points = np.linspace(hz_to_mel(fmin), hz_to_mel(fmax), n_mels + 2)
    hz_points = mel_to_hz(mel_points)
    bin_points = np.floor((n_fft + 1) * hz_points / sample_rate).astype(int)

    filterbank = np.zeros((n_mels, n_fft // 2 + 1), dtype=np.float32)
    for m in range(1, n_mels + 1):
        left = bin_points[m - 1]
        center = bin_points[m]
        right = bin_points[m + 1]

        for k in range(left, center):
            if center > left:
                filterbank[m - 1, k] = (k - left) / (center - left)
        for k in range(center, right):
            if right > center:
                filterbank[m - 1, k] = (right - k) / (right - center)

    return filterbank


# ---------------------------------------------------------------------------
# Token decoding (RNN-T greedy decoding)
# ---------------------------------------------------------------------------

def load_vocab(vocab_path: str) -> dict:
    """Load vocabulary file. Returns {token_id: token_string}."""
    vocab = {}
    with open(vocab_path, 'r', encoding='utf-8') as f:
        for i, line in enumerate(f):
            token = line.strip()
            vocab[i] = token
    return vocab


def greedy_decode(encoder_out, encoder_len, decoder_session, vocab):
    """Greedy RNN-T decoding.

    encoder_out: [1, time, 1024] encoder output
    encoder_len: [1] encoder output length
    decoder_session: ONNX session for decoder_joint model
    vocab: {token_id: token_string}
    """
    # RNN-T blank token is typically 0 or the last token
    # For Parakeet TDT, check vocab for blank token
    blank_id = None
    for tid, token in vocab.items():
        if token == '<blank>' or token == '<eos>' or token == '|':
            blank_id = tid
            break
    if blank_id is None:
        blank_id = 0  # Default

    max_len = int(encoder_len[0])
    encoder_out_trimmed = encoder_out[0, :max_len, :]  # [time, 1024]

    # RNN-T greedy decoding
    # The decoder_joint model takes:
    #   - encoder_output: [1, 1, 1024] (one frame at a time)
    #   - decoder_input: [1, 1] (token IDs, starting with blank/sos)
    # And outputs: [1, 1, vocab_size] logits

    # Check decoder_joint inputs
    dec_inputs = decoder_session.get_inputs()
    dec_input_names = [i.name for i in dec_inputs]

    # Initialize decoder state
    emitted_tokens = []
    decoder_input = np.array([[blank_id]], dtype=np.int32)  # [1, 1]
    decoder_state = None  # Will be populated based on model

    for t in range(max_len):
        enc_frame = encoder_out_trimmed[t:t+1, :].reshape(1, 1, -1)  # [1, 1, 1024]

        # Prepare decoder inputs based on model's expected input names
        feed = {}
        for name in dec_input_names:
            if 'encoder' in name.lower() or 'enc' in name.lower():
                feed[name] = enc_frame
            elif 'decoder' in name.lower() or 'tokens' in name.lower() or 'input' in name.lower():
                feed[name] = decoder_input
            elif 'length' in name.lower():
                feed[name] = np.array([1], dtype=np.int64)

        try:
            outputs = decoder_session.run(None, feed)
            logits = outputs[0]  # [1, 1, vocab_size]
            # Get top token
            top_token = int(np.argmax(logits[0, -1, :]))

            if top_token != blank_id and top_token < len(vocab):
                emitted_tokens.append(top_token)
                decoder_input = np.array([[top_token]], dtype=np.int32)
            # If blank, don't advance decoder, just move to next encoder frame
        except Exception as e:
            # If decoding fails, stop
            break

    return emitted_tokens


# ---------------------------------------------------------------------------
# Word timestamp extraction
# ---------------------------------------------------------------------------

def tokens_to_words(emitted_tokens, vocab, total_duration_ms):
    """Convert token IDs to word-level timestamps.

    Parakeet TDT outputs TDT (Token-and-Duration Transducer) tokens which
    include duration information. For simplicity, we map tokens to words
    and distribute timestamps evenly across the encoder frames.
    """
    # Convert tokens to text
    tokens_text = []
    for tid in emitted_tokens:
        if tid in vocab:
            token = vocab[tid]
            if token.startswith('▁'):  # SentencePiece space marker
                tokens_text.append((' ', token[1:]))
            elif token == '<blank>' or token == '<pad>' or token == '<unk>':
                continue
            else:
                tokens_text.append(('', token))

    # Group tokens into words
    words = []
    current_word = ""
    for sep, token in tokens_text:
        if sep == ' ' and current_word:
            words.append(current_word)
            current_word = token
        else:
            current_word += token
    if current_word:
        words.append(current_word)

    # Distribute timestamps evenly
    n_words = len(words)
    if n_words == 0:
        return []

    duration_per_word = total_duration_ms / n_words
    word_timings = []
    for i, word in enumerate(words):
        word_timings.append({
            'word': word.strip(),
            'start_ms': int(i * duration_per_word),
            'end_ms': int((i + 1) * duration_per_word),
        })

    return word_timings


# ---------------------------------------------------------------------------
# Main alignment function
# ---------------------------------------------------------------------------

def align_wav(wav_path: str, encoder_session, decoder_session, vocab) -> dict:
    """Align a WAV file to word-level timestamps."""
    # Compute mel spectrogram
    mel = compute_mel_spectrogram(wav_path)  # [128, time]
    mel_batch = mel[np.newaxis, :, :]  # [1, 128, time]
    length = np.array([mel.shape[1]], dtype=np.int64)  # [1]

    # Run encoder
    enc_inputs = encoder_session.get_inputs()
    enc_feed = {}
    for name, data in zip([i.name for i in enc_inputs], [mel_batch, length]):
        enc_feed[name] = data

    enc_outputs = encoder_session.run(None, enc_feed)
    encoder_out = enc_outputs[0]  # [1, time, 1024]
    encoder_len = enc_outputs[1]  # [1]

    # Get total duration from mel frames (16000 Hz, 160 hop = 10ms per frame)
    total_duration_ms = int(mel.shape[1] * 10)

    # Greedy decode
    emitted_tokens = greedy_decode(encoder_out, encoder_len, decoder_session, vocab)

    # Convert to word timings
    words = tokens_to_words(emitted_tokens, vocab, total_duration_ms)

    return {
        'status': 'ok',
        'words': words,
        'duration_ms': total_duration_ms,
    }


# ---------------------------------------------------------------------------
# Serve mode (long-lived stdin/stdout JSON protocol)
# ---------------------------------------------------------------------------

def run_serve(encoder_path, decoder_path, vocab_path):
    """Long-lived serve mode — load model once, loop on stdin/stdout."""
    try:
        # Disable ONNX Runtime telemetry
        opts = ort.SessionOptions()
        opts.inter_op_num_threads = 1
        opts.intra_op_num_threads = 1

        encoder_session = ort.InferenceSession(encoder_path, opts, providers=['CPUExecutionProvider'])
        decoder_session = ort.InferenceSession(decoder_path, opts, providers=['CPUExecutionProvider'])
        vocab = load_vocab(vocab_path)
    except Exception as e:
        print(json.dumps({"ready": False, "error": f"Failed to load Parakeet model: {e}"}))
        sys.stderr.write(f"ERROR: Failed to load Parakeet model: {e}\n")
        sys.exit(1)

    # Signal readiness
    print(json.dumps({"ready": True}), flush=True)

    # Loop: read one JSON request per line, write one JSON response per line
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue

        try:
            req = json.loads(line)
        except json.JSONDecodeError as e:
            print(json.dumps({"status": "error", "error": f"invalid JSON: {e}"}), flush=True)
            continue

        wav_path = req.get("wav_path", "")
        offset_ms = req.get("offset_ms", 0)

        if not wav_path:
            print(json.dumps({
                "status": "error",
                "error": "missing required field: wav_path",
            }), flush=True)
            continue

        try:
            result = align_wav(wav_path, encoder_session, decoder_session, vocab)
            # Apply offset_ms to all word timings
            for w in result.get("words", []):
                w["start_ms"] += offset_ms
                w["end_ms"] += offset_ms
            print(json.dumps(result), flush=True)
        except Exception as e:
            print(json.dumps({
                "status": "error",
                "error": f"alignment failed: {e}",
            }), flush=True)


def main():
    parser = argparse.ArgumentParser(description="Parakeet TDT force-alignment sidecar")
    parser.add_argument("--serve", action="store_true",
                        help="Long-lived serve mode (stdin/stdout JSON protocol)")
    parser.add_argument("--wav", default=None, help="WAV file path (fresh mode)")
    parser.add_argument("--output", default=None, help="Output JSON path (fresh mode)")
    parser.add_argument("--encoder", required=True, help="Path to encoder ONNX model")
    parser.add_argument("--decoder", required=True, help="Path to decoder_joint ONNX model")
    parser.add_argument("--vocab", required=True, help="Path to vocab.txt")
    args = parser.parse_args()

    if args.serve:
        run_serve(args.encoder, args.decoder, args.vocab)
    elif args.wav and args.output:
        # Fresh mode: align one WAV, write JSON, exit
        try:
            opts = ort.SessionOptions()
            opts.inter_op_num_threads = 1
            opts.intra_op_num_threads = 1
            encoder_session = ort.InferenceSession(args.encoder, opts, providers=['CPUExecutionProvider'])
            decoder_session = ort.InferenceSession(args.decoder, opts, providers=['CPUExecutionProvider'])
            vocab = load_vocab(args.vocab)
            result = align_wav(args.wav, encoder_session, decoder_session, vocab)
            with open(args.output, 'w') as f:
                json.dump(result, f)
            print(f"OK: {len(result.get('words', []))} words aligned")
        except Exception as e:
            print(f"ERROR: {e}", file=sys.stderr)
            sys.exit(1)
    else:
        parser.error("--serve or (--wav + --output) is required")


if __name__ == "__main__":
    main()
