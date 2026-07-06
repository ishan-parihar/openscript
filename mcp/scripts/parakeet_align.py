#!/usr/bin/env python3
"""Parakeet TDT force-alignment sidecar — replaces whisper_align.py.

Uses the Parakeet TDT 0.6b v3 ONNX model for RNN-T force alignment.
The model uses Token-and-Duration-Transducer (TDT) decoding where:
  - Tokens 0-8191: BPE content tokens
  - Token 8192: blank (no emit, no advance)
  - Tokens 8193-8197: duration tokens (advance 2-6 frames)

Model: istupakov/parakeet-tdt-0.6b-v3-onnx
"""

import argparse
import json
import sys
import os
import numpy as np

try:
    import onnxruntime as ort
except ImportError:
    print(json.dumps({"ready": False, "error": "onnxruntime not installed"}))
    sys.exit(1)

try:
    import librosa
except ImportError:
    print(json.dumps({"ready": False, "error": "librosa not installed"}))
    sys.exit(1)


def compute_mel_spectrogram(wav_path: str) -> np.ndarray:
    """Compute log-mel spectrogram matching NeMo's AudioToMelSpectrogramPreprocessor.

    NeMo Parakeet TDT config:
      - sample_rate=16000
      - n_window_size=400 (25ms at 16kHz)
      - n_window_stride=160 (10ms hop)
      - features=128
      - window='hann'
      - preemph=0.97
      - mag_power=2.0
      - log=true
      - log_zero_guard_type='add', log_zero_guard_value=1e-5
      - normalize='per_feature'
    """
    audio, sr = librosa.load(wav_path, sr=16000, mono=True)

    # Apply preemphasis (NeMo default: 0.97)
    preemph = 0.97
    audio = np.append(audio[0], audio[1:] - preemph * audio[:-1])

    # Compute mel spectrogram (power=2.0 → mag_power)
    mel = librosa.feature.melspectrogram(
        y=audio,
        sr=16000,
        n_fft=400,
        hop_length=160,
        win_length=400,
        n_mels=128,
        window='hann',
        center=True,
    )

    # Log with zero guard (NeMo: log(mel + 1e-5))
    log_mel = np.log(mel + 1e-5)

    # Per-feature normalization (NeMo: normalize='per_feature')
    mean = log_mel.mean(axis=1, keepdims=True)
    std = log_mel.std(axis=1, keepdims=True)
    log_mel = (log_mel - mean) / (std + 1e-10)

    return log_mel.astype(np.float32)


def load_vocab(vocab_path: str) -> dict:
    """Load vocabulary file. Format: 'token_string index' per line."""
    vocab = {}
    with open(vocab_path, 'r', encoding='utf-8') as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            parts = line.rsplit(' ', 1)
            if len(parts) == 2:
                try:
                    idx = int(parts[1])
                    vocab[idx] = parts[0]
                except ValueError:
                    pass
    return vocab


def decode_tdt(encoder_out, encoder_len, decoder_session, vocab):
    """TDT greedy decoding.

    The decoder is stateful (LSTM). We feed the full encoder output + one
    target token at a time, carrying the LSTM state forward.

    The decoder outputs logits [1, time, 1, 8198] for each call.
    We look at the LAST frame's logits to decide the next token.

    TDT tokens:
      0-8191: BPE content tokens
      8192: blank (stay, don't emit)
      8193-8197: duration tokens (advance encoder by N frames)
    """
    max_vocab = 8192  # Content tokens
    blank_id = 8192   # TDT blank

    max_len = int(encoder_len[0])
    enc_full = encoder_out[:, :, :max_len]  # [1, 1024, max_len]

    # Initialize LSTM states
    state_1 = np.zeros((2, 1, 640), dtype=np.float32)
    state_2 = np.zeros((2, 1, 640), dtype=np.float32)

    # Start with <|startoftranscript|> (token 4)
    current_token = 4

    emitted_tokens = []
    token_frames = []  # (token, frame_index) for timestamp estimation

    # The decoder produces logits for ALL encoder frames in one call.
    # We iterate: feed current_token → get logits → pick next token → repeat.
    # The LSTM state carries context across iterations.

    for step in range(500):  # max 500 decode steps
        feed = {
            'encoder_outputs': enc_full,
            'targets': np.array([[current_token]], dtype=np.int32),
            'target_length': np.array([1], dtype=np.int32),
            'input_states_1': state_1,
            'input_states_2': state_2,
        }

        outputs = decoder_session.run(None, feed)
        logits = outputs[0]  # [1, max_len, 1, 8198]
        state_1 = outputs[2]  # [2, 1, 640]
        state_2 = outputs[3]  # [2, 1, 640]

        # Get the logit for the last encoder frame
        flat = logits[0, -1, 0, :]  # [8198]
        top = int(np.argmax(flat))

        if top < max_vocab:
            # Content token — emit it
            # Skip special tokens (0-9: <unk>, <|nospeech|>, <pad>, etc.)
            if top >= 10:
                emitted_tokens.append(top)
                token_frames.append(step)
            current_token = top
        elif top == blank_id:
            # Blank — in TDT this means "done with current position"
            # Feed blank back, let the model decide to continue or stop
            current_token = 1  # <|nospeech|> as blank feedback
            # If we've emitted tokens and get blank, we might be done
            if len(emitted_tokens) > 0 and step > len(emitted_tokens) * 3:
                break
        else:
            # Duration token (8193-8197) — advance frames
            # In TDT, duration tokens tell how many encoder frames to skip.
            # Since we feed the full encoder each time, the decoder internally
            # handles this via attention. We just feed blank back.
            current_token = 1  # <|nospeech|>

        # Safety: if we've been running too long without emitting, stop
        if step > 100 and len(emitted_tokens) == 0:
            break

    return emitted_tokens, token_frames


def tokens_to_words(emitted_tokens, vocab, token_frames, total_duration_ms):
    """Convert BPE tokens to word-level timestamps."""
    if not emitted_tokens:
        return []

    # Group BPE tokens into words (▁ = word boundary in SentencePiece)
    words = []
    current_word = ""
    word_start_frame = 0

    for i, (tid, frame) in enumerate(zip(emitted_tokens, token_frames)):
        token_str = vocab.get(tid, "")
        if token_str.startswith('▁'):
            # Word boundary
            if current_word:
                words.append((current_word, word_start_frame, frame))
            current_word = token_str[1:]  # strip ▁
            word_start_frame = frame
        else:
            current_word += token_str

    if current_word:
        words.append((current_word, word_start_frame, token_frames[-1] if token_frames else 0))

    # Convert frame indices to timestamps
    # Each encoder frame = 10ms (160 hop at 16kHz, 8x subsampling = 80ms per encoder frame)
    # Actually: 160 hop → 10ms audio per mel frame. 8x subsampling → 80ms per encoder frame.
    ms_per_frame = 80  # 8 * 10ms

    word_timings = []
    for word, start_frame, end_frame in words:
        word_timings.append({
            'word': word.strip(),
            'start_ms': int(start_frame * ms_per_frame),
            'end_ms': int((end_frame + 1) * ms_per_frame),
        })

    return word_timings


def align_wav(wav_path, encoder_session, decoder_session, vocab):
    """Align a WAV file to word-level timestamps."""
    mel = compute_mel_spectrogram(wav_path)
    mel_batch = mel[np.newaxis, :, :]
    length = np.array([mel.shape[1]], dtype=np.int64)

    # Run encoder
    enc_feed = {
        encoder_session.get_inputs()[0].name: mel_batch,
        encoder_session.get_inputs()[1].name: length,
    }
    enc_outputs = encoder_session.run(None, enc_feed)
    encoder_out = enc_outputs[0]  # [1, 1024, time]
    encoder_len = enc_outputs[1]  # [1]

    total_duration_ms = int(mel.shape[1] * 10)  # 10ms per mel frame

    # Decode
    emitted_tokens, token_frames = decode_tdt(encoder_out, encoder_len, decoder_session, vocab)

    # Convert to word timings
    words = tokens_to_words(emitted_tokens, vocab, token_frames, total_duration_ms)

    return {
        'status': 'ok',
        'words': words,
        'duration_ms': total_duration_ms,
        'token_count': len(emitted_tokens),
    }


def run_serve(encoder_path, decoder_path, vocab_path):
    """Long-lived serve mode."""
    try:
        opts = ort.SessionOptions()
        opts.inter_op_num_threads = 1
        opts.intra_op_num_threads = 1
        encoder_session = ort.InferenceSession(encoder_path, opts, providers=['CPUExecutionProvider'])
        decoder_session = ort.InferenceSession(decoder_path, opts, providers=['CPUExecutionProvider'])
        vocab = load_vocab(vocab_path)
    except Exception as e:
        print(json.dumps({"ready": False, "error": f"Failed to load Parakeet model: {e}"}))
        sys.stderr.write(f"ERROR: {e}\n")
        sys.exit(1)

    print(json.dumps({"ready": True}), flush=True)

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
            print(json.dumps({"status": "error", "error": "missing wav_path"}), flush=True)
            continue

        try:
            result = align_wav(wav_path, encoder_session, decoder_session, vocab)
            for w in result.get("words", []):
                w["start_ms"] += offset_ms
                w["end_ms"] += offset_ms
            print(json.dumps(result), flush=True)
        except Exception as e:
            print(json.dumps({"status": "error", "error": str(e)}), flush=True)


def main():
    parser = argparse.ArgumentParser(description="Parakeet TDT force-alignment sidecar")
    parser.add_argument("--serve", action="store_true")
    parser.add_argument("--wav", default=None)
    parser.add_argument("--output", default=None)
    parser.add_argument("--encoder", required=True)
    parser.add_argument("--decoder", required=True)
    parser.add_argument("--vocab", required=True)
    args = parser.parse_args()

    if args.serve:
        run_serve(args.encoder, args.decoder, args.vocab)
    elif args.wav and args.output:
        opts = ort.SessionOptions()
        opts.inter_op_num_threads = 1
        opts.intra_op_num_threads = 1
        enc = ort.InferenceSession(args.encoder, opts, providers=['CPUExecutionProvider'])
        dec = ort.InferenceSession(args.decoder, opts, providers=['CPUExecutionProvider'])
        vocab = load_vocab(args.vocab)
        result = align_wav(args.wav, enc, dec, vocab)
        with open(args.output, 'w') as f:
            json.dump(result, f)
        print(f"OK: {len(result.get('words', []))} words aligned")
    else:
        parser.error("--serve or (--wav + --output) is required")


if __name__ == "__main__":
    main()
