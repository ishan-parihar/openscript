#!/usr/bin/env python3
"""
Nemotron 3.5 ASR Transcriber — ONNX Runtime direct inference.

Uses nvidia/nemotron-3.5-asr-streaming-0.6b via ONNX Runtime.
This sidecar handles:
  1. Audio preprocessing (16kHz mono WAV → log-mel spectrogram)
  2. Nemotron encoder inference
  3. RNNT greedy decode via decoder_joint
  4. Token decoding to text
  5. SRT generation (word-level + phrase-level)

Input:  stdin JSON  {"wav_path": "...", "language_hint": "hi-IN"|"auto"}
Output: stdout JSON {"text": "...", "word_srt": "...", "phrase_srt": "...",
                     "segments": [...], "duration_s": float}

Also supports one-shot CLI: nemotron_transcriber.py --wav <path> --out-dir <dir>
"""

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path

import numpy as np

try:
    import onnxruntime as ort
except ImportError:
    print(json.dumps({"error": "onnxruntime not installed"}))
    sys.exit(1)

try:
    import sentencepiece as spm
except ImportError:
    print(json.dumps({"error": "sentencepiece not installed"}))
    sys.exit(1)

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parent.parent  # openscript root

# Model paths — resolved relative to repo root
MODEL_DIR = REPO_ROOT / "models" / "nemotron-onnx"
ENCODER_PATH = str(MODEL_DIR / "encoder.onnx")
DECODER_PATH = str(MODEL_DIR / "decoder_joint.onnx")
TOKENIZER_PATH = str(MODEL_DIR / "tokenizer.model")
TOKENS_PATH = str(MODEL_DIR / "tokens.txt")

# Mel spectrogram parameters (matching NeMo AudioToMelSpectrogramPreprocessor)
SAMPLE_RATE = 16000
N_MELS = 128
N_FFT = 400
HOP_LENGTH = 160
WIN_LENGTH = 400
PREEMPH = 0.97
LOG_GUARD = 1e-5

# RNNT decode parameters
MAX_DECODE_STEPS = 500
BOS_TOKEN = 1  # Beginning of sequence
EOS_TOKEN = 2  # End of sequence
SOT_TOKEN = 1  # Start of transcript (same as BOS for this model)
VOCAB_SIZE = 13087  # Model logits have 13088 entries; token 13087 is EOS overflow

# Phrase grouping parameters
PHRASE_MAX_WORDS = 12
PHRASE_MAX_CHARS = 64
PHRASE_MAX_GAP_S = 0.6


def _log(msg: str):
    print(f"[nemotron] {msg}", file=sys.stderr, flush=True)


# ---------------------------------------------------------------------------
# Vocabulary loading
# ---------------------------------------------------------------------------

def load_vocab(tokens_path: str) -> dict:
    """Load vocabulary from tokens.txt (line-number index format)."""
    vocab = {}
    with open(tokens_path, "r", encoding="utf-8") as f:
        for idx, line in enumerate(f):
            vocab[idx] = line.strip()
    return vocab


def load_tokenizer(tokenizer_path: str):
    """Load SentencePiece tokenizer."""
    sp = spm.SentencePieceProcessor()
    sp.Load(tokenizer_path)
    return sp


# ---------------------------------------------------------------------------
# Audio preprocessing
# ---------------------------------------------------------------------------

def compute_mel_spectrogram(wav_path: str) -> np.ndarray:
    """Compute log-mel spectrogram matching NeMo's preprocessor.

    Returns: [n_mels, time] float32 array
    """
    try:
        import librosa
        audio, _ = librosa.load(wav_path, sr=SAMPLE_RATE, mono=True)
    except ImportError:
        # Fallback: use subprocess + ffmpeg to get raw PCM
        _log("librosa not available, using ffmpeg fallback")
        audio = _load_audio_ffmpeg(wav_path)

    # Preemphasis
    audio = np.append(audio[0], audio[1:] - PREEMPH * audio[:-1]).astype(np.float32)

    # Pad audio to at least one frame
    if len(audio) < WIN_LENGTH:
        audio = np.pad(audio, (0, WIN_LENGTH - len(audio)))

    # Compute mel spectrogram
    try:
        import librosa
        mel = librosa.feature.melspectrogram(
            y=audio,
            sr=SAMPLE_RATE,
            n_fft=N_FFT,
            hop_length=HOP_LENGTH,
            win_length=WIN_LENGTH,
            n_mels=N_MELS,
            window="hann",
            center=True,
            power=2.0,
        )
    except ImportError:
        _log("librosa not available and no fallback installed. Install: pip install librosa")
        raise RuntimeError("librosa is required for mel spectrogram computation. Install: pip install librosa")

    # Log with zero guard
    log_mel = np.log(mel + LOG_GUARD)

    # Per-feature normalization
    mean = log_mel.mean(axis=1, keepdims=True)
    std = log_mel.std(axis=1, keepdims=True)
    log_mel = (log_mel - mean) / (std + 1e-10)

    return log_mel.astype(np.float32)


def _load_audio_ffmpeg(wav_path: str) -> np.ndarray:
    """Load audio via ffmpeg as raw float32 PCM."""
    cmd = [
        "ffmpeg", "-i", wav_path, "-f", "s16le", "-acodec", "pcm_s16le",
        "-ar", str(SAMPLE_RATE), "-ac", "1", "-v", "quiet", "-"
    ]
    result = subprocess.run(cmd, capture_output=True, timeout=60)
    if result.returncode != 0:
        raise RuntimeError(f"ffmpeg failed: {result.stderr.decode()[:200]}")
    raw = np.frombuffer(result.stdout, dtype=np.int16)
    return raw.astype(np.float32) / 32768.0





# ---------------------------------------------------------------------------
# ONNX Runtime inference
# ---------------------------------------------------------------------------

class NemotronASR:
    """Nemotron 3.5 ASR model via ONNX Runtime."""

    def __init__(self, num_threads: int = 4):
        opts = ort.SessionOptions()
        opts.inter_op_num_threads = num_threads
        opts.intra_op_num_threads = num_threads

        _log(f"Loading encoder: {ENCODER_PATH}")
        self.encoder = ort.InferenceSession(
            ENCODER_PATH, opts, providers=["CPUExecutionProvider"]
        )
        _log(f"Loading decoder_joint: {DECODER_PATH}")
        self.decoder = ort.InferenceSession(
            DECODER_PATH, opts, providers=["CPUExecutionProvider"]
        )
        self.vocab = load_vocab(TOKENS_PATH)
        self.tokenizer = load_tokenizer(TOKENIZER_PATH)

        _log(f"Models loaded. Vocab size: {len(self.vocab)}")

    def encode(self, mel: np.ndarray) -> tuple:
        """Run encoder on log-mel spectrogram.

        Args:
            mel: [n_mels, time] float32

        Returns:
            (encoder_out, encoder_len) where encoder_out is [1, 1024, time']
        """
        # Add batch dimension: [1, n_mels, time]
        mel_batch = mel[np.newaxis, :, :]
        length = np.array([mel.shape[1]], dtype=np.int64)

        # Initialize caches (full sequence, not streaming)
        max_time = mel.shape[1]
        cache_last_channel = np.zeros((24, 1, 56, 1024), dtype=np.float32)
        cache_last_time = np.zeros((24, 1, 1024, 8), dtype=np.float32)
        cache_last_channel_len = np.array([0], dtype=np.int64)
        prompt_index = np.array([0], dtype=np.int64)

        outputs = self.encoder.run(None, {
            "processed_signal": mel_batch,
            "processed_signal_length": length,
            "cache_last_channel": cache_last_channel,
            "cache_last_time": cache_last_time,
            "cache_last_channel_len": cache_last_channel_len,
            "prompt_index": prompt_index,
        })

        return outputs[0], outputs[1]  # encoded, encoded_len

    def decode(self, encoder_out: np.ndarray, encoder_len: np.ndarray,
               language_hint: str = "auto") -> list:
        """RNNT greedy decode.

        Args:
            encoder_out: [1, 1024, time] from encoder
            encoder_len: [1] encoder output length
            language_hint: "auto", "hi-IN", "en-US", etc.

        Returns:
            List of token IDs
        """
        # Truncate encoder output to actual length
        time_steps = int(encoder_len[0])
        enc = encoder_out[:, :, :time_steps]

        # Initialize decoder states
        state_1 = np.zeros((2, 1, 640), dtype=np.float32)
        state_2 = np.zeros((2, 1, 640), dtype=np.int32) if False else np.zeros((2, 1, 640), dtype=np.float32)

        # Start token
        current_token = BOS_TOKEN
        emitted_tokens = []

        for step in range(MAX_DECODE_STEPS):
            dec_out = self.decoder.run(None, {
                "encoder_outputs": enc,
                "targets": np.array([[current_token]], dtype=np.int32),
                "target_length": np.array([1], dtype=np.int32),
                "input_states_1": state_1,
                "input_states_2": state_2,
            })

            logits = dec_out[0]  # [1, time, 1, vocab_size]
            state_1 = dec_out[2]  # output_states_1
            state_2 = dec_out[3]  # output_states_2

            # Get logits for the last encoder frame
            flat = logits[0, -1, 0, :]
            top_id = int(np.argmax(flat))

            # Check for EOS, padding, or out-of-range tokens
            # Token 13087 is the model's EOS/overflow token (logits have 13088 entries)
            if top_id == EOS_TOKEN or top_id == 0 or top_id >= VOCAB_SIZE:
                break

            # Skip special tokens (language tags, etc.)
            if top_id >= 10:  # Content tokens start after specials
                emitted_tokens.append(top_id)
                current_token = top_id
            else:
                # Special token — feed it back but don't emit
                current_token = top_id
                # If we've been running too long without emitting, stop
                if step > 20 and len(emitted_tokens) == 0:
                    break

            # Safety: if we've been running too long without emitting, stop
            if step > 100 and len(emitted_tokens) == 0:
                break

        return emitted_tokens

    def tokens_to_text(self, token_ids: list) -> str:
        """Convert token IDs to text using SentencePiece."""
        return self.tokenizer.Decode(token_ids)

    def transcribe(self, wav_path: str, language_hint: str = "auto") -> dict:
        """Full transcription pipeline.

        Returns:
            dict with text, tokens, duration_s, segments
        """
        start = time.time()

        # Compute mel spectrogram
        _log(f"Computing mel spectrogram for {wav_path}")
        mel = compute_mel_spectrogram(wav_path)
        duration_s = mel.shape[1] * HOP_LENGTH / SAMPLE_RATE
        _log(f"Mel shape: {mel.shape}, duration: {duration_s:.1f}s")

        # Encode
        _log("Running encoder...")
        enc_start = time.time()
        encoder_out, encoder_len = self.encode(mel)
        enc_time = time.time() - enc_start
        _log(f"Encoder done in {enc_time:.1f}s")

        # Decode
        _log("Running RNNT greedy decode...")
        dec_start = time.time()
        token_ids = self.decode(encoder_out, encoder_len, language_hint)
        dec_time = time.time() - dec_start
        _log(f"Decode done in {dec_time:.1f}s, {len(token_ids)} tokens")

        # Convert to text
        text = self.tokens_to_text(token_ids)
        _log(f"Text: {text[:100]}...")

        total_time = time.time() - start
        _log(f"Total transcription: {total_time:.1f}s ({total_time/duration_s:.1f}x realtime)")

        return {
            "text": text,
            "token_ids": token_ids,
            "duration_s": duration_s,
            "tokens_per_second": len(token_ids) / max(duration_s, 0.001),
            "encoding_time_s": enc_time,
            "decoding_time_s": dec_time,
            "total_time_s": total_time,
        }


# ---------------------------------------------------------------------------
# SRT generation
# ---------------------------------------------------------------------------

def fmt_ts(seconds: float) -> str:
    """Format seconds to SRT timestamp."""
    ms = round((seconds % 1) * 1000) % 1000
    s_int = int(seconds)
    return "%02d:%02d:%02d,%03d" % (
        s_int // 3600, (s_int % 3600) // 60, s_int % 60, ms
    )


def group_words_into_phrases(
    words: list,
    max_words: int = PHRASE_MAX_WORDS,
    max_chars: int = PHRASE_MAX_CHARS,
    max_gap: float = PHRASE_MAX_GAP_S,
) -> list:
    """Group word-level entries into phrase-level SRT entries."""
    groups = []
    cur_words = []
    cur_start = None
    cur_end = None

    for w in words:
        text = w.get("word", "").strip()
        if not text:
            continue
        start = w.get("start_s", 0)
        end = w.get("end_s", 0)

        if cur_start is None:
            cur_start = start
            cur_end = end
            cur_words = [text]
            continue

        gap = start - (cur_end or start)
        combined = " ".join(cur_words)
        next_len = len(combined) + 1 + len(text)

        if gap > max_gap or len(cur_words) >= max_words or next_len > max_chars:
            groups.append({
                "text": " ".join(cur_words),
                "start_s": cur_start,
                "end_s": cur_end,
            })
            cur_start = start
            cur_end = end
            cur_words = [text]
        else:
            cur_words.append(text)
            cur_end = end

    if cur_words:
        groups.append({
            "text": " ".join(cur_words),
            "start_s": cur_start,
            "end_s": cur_end,
        })

    return groups


def generate_word_srt(words: list, output_path: str) -> str:
    """Generate word-level SRT file."""
    with open(output_path, "w", encoding="utf-8") as f:
        for i, w in enumerate(words, 1):
            f.write("%d\n%s --> %s\n%s\n\n" % (
                i, fmt_ts(w["start_s"]), fmt_ts(w["end_s"]), w["word"].strip()
            ))
    return output_path


def generate_phrase_srt(phrases: list, output_path: str) -> str:
    """Generate phrase-level SRT file."""
    with open(output_path, "w", encoding="utf-8") as f:
        for i, p in enumerate(phrases, 1):
            f.write("%d\n%s --> %s\n%s\n\n" % (
                i, fmt_ts(p["start_s"]), fmt_ts(p["end_s"]), p["text"]
            ))
    return output_path


# ---------------------------------------------------------------------------
# Audio extraction (from video)
# ---------------------------------------------------------------------------

def extract_audio(video_path: str, wav_path: str) -> bool:
    """Extract 16kHz mono WAV from video using ffmpeg."""
    cmd = [
        "ffmpeg", "-y", "-i", video_path,
        "-vn", "-acodec", "pcm_s16le",
        "-ar", str(SAMPLE_RATE), "-ac", "1",
        wav_path,
    ]
    _log("Extracting audio...")
    result = subprocess.run(cmd, capture_output=True, timeout=300)
    if result.returncode != 0:
        _log(f"Audio extraction failed: {result.stderr.decode()[:300]}")
        return False
    _log(f"Audio extracted: {wav_path}")
    return True


# ---------------------------------------------------------------------------
# Main pipeline
# ---------------------------------------------------------------------------

def run_transcription(
    media_path: str,
    out_dir: str,
    language_hint: str = "auto",
    num_threads: int = 4,
) -> dict:
    """Full transcription pipeline: audio → Nemotron → SRT."""
    Path(out_dir).mkdir(parents=True, exist_ok=True)
    stem = Path(media_path).stem

    # Step 1: Extract audio if needed
    wav_path = str(Path(out_dir) / f"{stem}.nemotron.wav")
    if media_path.lower().endswith((".wav", ".mp3", ".flac", ".ogg")):
        # Already audio — just convert to 16kHz mono WAV
        cmd = [
            "ffmpeg", "-y", "-i", media_path,
            "-vn", "-acodec", "pcm_s16le",
            "-ar", str(SAMPLE_RATE), "-ac", "1",
            wav_path,
        ]
        subprocess.run(cmd, capture_output=True, timeout=300)
    else:
        if not extract_audio(media_path, wav_path):
            return {"error": "Audio extraction failed", "status": "error"}

    # Step 2: Transcribe with Nemotron
    model = NemotronASR(num_threads=num_threads)
    result = model.transcribe(wav_path, language_hint)

    if "error" in result:
        return result

    # Step 3: For now, we don't have word-level timestamps from Nemotron
    # (RNNT greedy decode doesn't give word timings). We create estimated
    # word-level SRT by splitting text into words and distributing evenly.
    text = result["text"]
    duration_s = result["duration_s"]
    words = text.split()

    if words:
        word_duration = duration_s / len(words)
        word_entries = []
        for i, word in enumerate(words):
            word_entries.append({
                "word": word,
                "start_s": i * word_duration,
                "end_s": (i + 1) * word_duration,
            })

        # Step 4: Generate SRT files
        word_srt_path = str(Path(out_dir) / f"{stem}.nemotron.word.srt")
        phrase_srt_path = str(Path(out_dir) / f"{stem}.nemotron.phrase.srt")

        generate_word_srt(word_entries, word_srt_path)

        phrases = group_words_into_phrases(word_entries)
        generate_phrase_srt(phrases, phrase_srt_path)

        # Copy phrase SRT to output path
        output_srt_path = str(Path(out_dir) / f"{stem}.nemotron.srt")
        generate_phrase_srt(phrases, output_srt_path)

        result["word_srt_path"] = word_srt_path
        result["phrase_srt_path"] = phrase_srt_path
        result["output_srt_path"] = output_srt_path
        result["segments"] = [{"text": p["text"], "start_s": p["start_s"], "end_s": p["end_s"]} for p in phrases]
    else:
        result["error"] = "No text produced from transcription"
        result["status"] = "error"

    result["status"] = "transcribed"
    result["language"] = language_hint
    result["engine"] = "nemotron"

    # Cleanup temp WAV
    try:
        os.remove(wav_path)
    except OSError:
        pass

    return result


# ---------------------------------------------------------------------------
# CLI entry point
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(
        description="Nemotron 3.5 ASR Transcriber (ONNX Runtime)"
    )
    sub = parser.add_subparsers(dest="cmd", required=True)

    # One-shot mode
    p_run = sub.add_parser("run", help="Transcribe a media file")
    p_run.add_argument("--video", required=True, help="Path to video/audio file")
    p_run.add_argument("--out-dir", default=None, help="Output directory")
    p_run.add_argument("--language", default="auto", help="Language hint (auto, hi-IN, en-US)")
    p_run.add_argument("--threads", type=int, default=4, help="Number of threads")

    # Stdin/stdout serve mode (for Rust sidecar)
    p_serve = sub.add_parser("serve", help="Long-lived stdin/stdout JSON mode")

    args = parser.parse_args()

    if args.cmd == "run":
        out_dir = args.out_dir or str(Path(args.video).parent)
        if Path(out_dir).resolve() == Path("/"):
            out_dir = "."
        result = run_transcription(
            args.video, out_dir, args.language, args.threads
        )
        if result.get("error"):
            _log(f"ERROR: {result['error']}")
            print(json.dumps(result))
            sys.exit(1)
        print(json.dumps(result))

    elif args.cmd == "serve":
        _log("Starting stdin/stdout serve mode")
        print(json.dumps({"ready": True}), flush=True)

        for line in sys.stdin:
            line = line.strip()
            if not line:
                continue
            try:
                req = json.loads(line)
            except json.JSONDecodeError as e:
                print(json.dumps({"error": f"Invalid JSON: {e}"}), flush=True)
                continue

            wav_path = req.get("wav_path", "")
            out_dir = req.get("out_dir", str(Path(wav_path).parent) if wav_path else ".")
            language_hint = req.get("language_hint", "auto")
            num_threads = req.get("num_threads", 4)

            if not wav_path:
                print(json.dumps({"error": "missing wav_path"}), flush=True)
                continue

            try:
                result = run_transcription(wav_path, out_dir, language_hint, num_threads)
                print(json.dumps(result), flush=True)
            except Exception as e:
                print(json.dumps({"error": str(e)}), flush=True)


if __name__ == "__main__":
    main()
