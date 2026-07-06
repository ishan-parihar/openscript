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
    """Compute log-mel spectrogram using librosa.

    Parakeet TDT expects 128-bin log-mel features at 16kHz with:
      - n_fft=512 (32ms window)
      - hop_length=160 (10ms hop)
      - win_length=512
      - Pre-normalized log-mel (no standardization — NeMo handles it differently)

    Returns array of shape [n_mels, time_frames] suitable for Parakeet encoder.
    """
    import librosa

    # Load audio at 16kHz (librosa handles resampling automatically)
    audio, sr = librosa.load(wav_path, sr=16000, mono=True)

    # Compute mel spectrogram using librosa (handles window, FFT, mel filterbank)
    mel = librosa.feature.melspectrogram(
        y=audio,
        sr=16000,
        n_fft=n_fft,
        hop_length=hop_length,
        win_length=win_length,
        n_mels=n_mels,
        window='hann',
        center=True,
    )

    # NeMo uses log(power_mel + eps), NOT librosa.power_to_db
    # power_to_db uses 10*log10(mel/ref), NeMo uses log(mel+eps)
    log_mel = np.log(mel + 1e-10)

    # Per-feature normalization (NeMo's default: normalize="per_feature")
    mean = log_mel.mean(axis=1, keepdims=True)
    std = log_mel.std(axis=1, keepdims=True)
    log_mel = (log_mel - mean) / (std + 1e-10)

    return log_mel.astype(np.float32)


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
    """Load vocabulary file. Returns {token_id: token_string}.

    The vocab.txt format has lines like: "token_string index"
    e.g. "<unk> 0", "<|nospeech|> 1", "the 12"
    """
    vocab = {}
    with open(vocab_path, 'r', encoding='utf-8') as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            # Split on the last space — the index is the last token
            parts = line.rsplit(' ', 1)
            if len(parts) == 2:
                token = parts[0]
                try:
                    idx = int(parts[1])
                    vocab[idx] = token
                except ValueError:
                    # Not a valid index line — skip
                    pass
            else:
                # No index — use line number
                pass
    return vocab


def greedy_decode(encoder_out, encoder_len, decoder_session, vocab):
    """Greedy RNN-T decoding for Parakeet TDT model.

    This model uses a stateful decoder with LSTM states. The decoder_joint
    model takes:
      - encoder_outputs: [1, time, 1024]
      - targets: [1, seq_len] (token IDs)
      - target_length: [1]
      - input_states_1: [2, batch, 640] (LSTM state 1)
      - input_states_2: [2, batch, 640] (LSTM state 2)

    And outputs:
      - outputs: [1, seq_len, 8198] (logits — 8193 vocab + 5 TDT tokens)
      - prednet_lengths: [1]
      - output_states_1: [2, batch, 640]
      - output_states_2: [2, batch, 640]
    """
    # Blank token for RNN-T is <|nospeech|> at index 1
    blank_id = 1
    # Start-of-transcript token — the model expects this as the initial
    # decoder input, not blank. Without it, the model predicts only
    # duration tokens (8193-8197) and no content.
    start_token = 4  # <|startoftranscript|>

    # TDT-specific tokens (8193-8197 are duration tokens, not content)
    max_vocab_id = len(vocab)  # 8193

    max_len = int(encoder_len[0])
    # Encoder output is [batch, 1024, time] (features × time, transposed)
    encoder_out_trimmed = encoder_out[0, :, :max_len]  # [1024, time]

    # Initialize decoder states (zeros for LSTM)
    state_1 = np.zeros((2, 1, 640), dtype=np.float32)
    state_2 = np.zeros((2, 1, 640), dtype=np.float32)

    emitted_tokens = []
    # Start with start-of-transcript token (not blank)
    current_token = start_token

    for t in range(max_len):
        # Extract one time frame: [1024, 1] → reshape to [1, 1024, 1]
        enc_frame = encoder_out_trimmed[:, t:t+1].reshape(1, 1024, 1)  # [1, 1024, 1]

        # Build feed dict with correct input names
        targets = np.array([[current_token]], dtype=np.int32)
        target_length = np.array([1], dtype=np.int32)

        feed = {
            'encoder_outputs': enc_frame,
            'targets': targets,
            'target_length': target_length,
            'input_states_1': state_1,
            'input_states_2': state_2,
        }

        try:
            outputs = decoder_session.run(None, feed)
            logits = outputs[0]  # [1, 1, 8198]
            state_1 = outputs[2]  # [2, 1, 640]
            state_2 = outputs[3]  # [2, 1, 640]

            # Get top token
            top_token = int(np.argmax(logits[0, -1, :]))

            # In TDT, tokens >= max_vocab_id are duration tokens (not content)
            # Blank is also a "stay" token — advance to next encoder frame
            if top_token < max_vocab_id and top_token != blank_id:
                # Content token — emit it
                emitted_tokens.append(top_token)
                current_token = top_token
            else:
                # Blank or duration token — advance encoder frame
                current_token = blank_id

        except Exception as e:
            import traceback
            traceback.print_exc(file=sys.stderr)
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
