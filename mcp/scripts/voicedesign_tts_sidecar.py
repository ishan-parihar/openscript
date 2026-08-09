#!/usr/bin/env python3
"""VoiceDesign TTS sidecar — Qwen3-TTS-12Hz-1.7B-VoiceDesign via ONNX Runtime.

Designs novel character voices from natural-language instructions only (no
reference audio). This is the "full voice design" engine: give it a persona
description (`instruct`) and a sample line (`text`) and it synthesizes speech
in a brand-new voice that matches the description.

Model: wavekat/Qwen3-TTS-1.7B-VoiceDesign-ONNX (Apache-2.0), int4 quantization,
~4.3 GB total. Zero PyTorch at inference — the pipeline is NumPy + ONNX Runtime
(four sessions: talker_prefill, talker_decode, code_predictor, vocoder),
ported from the upstream reference `generate_onnx.py` into a long-lived
sidecar that reuses sessions across requests. Output: 24 kHz mono WAV, 10
languages (en/zh/ja/ko/de/fr/es/it/pt/ru).

LONG-LIVED SERVE MODE (--serve):
    Loads the four ONNX sessions ONCE (lazily on the first design request,
    so MCP server startup stays fast), then reads JSON requests from stdin
    (one per line) and writes JSON responses to stdout.
    Prints `{"ready":true}` immediately.

PROTOCOL (mirrors audio8/gepard sidecars):
  → {"op":"design","instruct":"Speak in a warm and friendly female voice",
     "text":"Give every small business the voice of a big one.",
     "output_path":"/tmp/persona.wav","language":"english",
     "seed":42,"temperature":0.9,"top_k":50,"repetition_penalty":1.05}
  ← {"status":"ok","output_path":"/tmp/persona.wav","duration_ms":1234,"sample_rate":24000,
     "frames":94,"gen_seconds":3.1,"device":"cuda"}

  → {"op":"health"}
  ← {"status":"ok","model_loaded":true,"model_dir":"...","variant":"int4","device":"cuda",
     "config":{"sample_rate":24000,"languages":10}}

  On error:
  ← {"status":"error","error":"..."}

ENV:
  VOICEDESIGN_MODEL_DIR  model root (default <repo>/mcp/assets/voicedesign)
  VOICEDESIGN_VARIANT    fp32 | int4 (default int4)
  VOICEDESIGN_DEVICE     auto|cuda|cpu (default auto — CUDA first)
  VOICEDESIGN_LOG        diagnostics log path (default /tmp/voicedesign_tts_sidecar.log)
  OPENSCRIPT_ROOT        repo root (defaults to script location + ../../)
"""

import argparse
import json
import os
import sys
import time
from pathlib import Path

_SCRIPT_DIR = Path(__file__).resolve().parent
_ROOT = Path(os.environ.get("OPENSCRIPT_ROOT", _SCRIPT_DIR.parent.parent)).resolve()

MODEL_DIR = Path(
    os.environ.get("VOICEDESIGN_MODEL_DIR", _ROOT / "mcp/assets/voicedesign")
).resolve()
VARIANT = os.environ.get("VOICEDESIGN_VARIANT", "int4")
SAMPLE_RATE = 24000


def log(msg: str) -> None:
    sys.stderr.write(f"[voicedesign_tts_sidecar] {msg}\n")
    sys.stderr.flush()


# ---------------------------------------------------------------------------
# ONNX Runtime providers (CUDA-first; mirrors transcribe_common/audio8)
# ---------------------------------------------------------------------------
def _resolve_providers() -> list[str]:
    dev = os.environ.get("VOICEDESIGN_DEVICE", "auto").strip().lower()
    if dev == "cpu":
        return ["CPUExecutionProvider"]
    import onnxruntime as ort

    available = ort.get_available_providers()
    if dev == "cuda" and "CUDAExecutionProvider" in available:
        return ["CUDAExecutionProvider", "CPUExecutionProvider"]
    if "CUDAExecutionProvider" in available:
        return ["CUDAExecutionProvider", "CPUExecutionProvider"]
    return ["CPUExecutionProvider"]


def _resolve_device_name(providers: list[str]) -> str:
    return "cuda" if providers and providers[0].startswith("CUDA") else "cpu"


# ---------------------------------------------------------------------------
# Engine (lazy-loaded, sessions reused across requests)
# ---------------------------------------------------------------------------
class VoiceDesignEngine:
    """Qwen3-TTS-1.7B-VoiceDesign ONNX pipeline (NumPy-only inference)."""

    def __init__(self, model_dir: Path, variant: str):
        self.model_dir = Path(model_dir)
        self.variant = variant
        self.providers = _resolve_providers()
        self.device = _resolve_device_name(self.providers)
        self.config = None
        self.emb = None
        self.tokenizer = None
        self.sessions = None

    # --- load -----------------------------------------------------------------
    def load(self):
        import numpy as np  # noqa: PLC0415
        import onnxruntime as ort  # noqa: PLC0415
        from transformers import AutoTokenizer  # noqa: PLC0415

        if self.config is not None:
            return self
        t0 = time.time()
        with open(self.model_dir / "config.json") as f:
            self.config = json.load(f)
        self.emb = self._load_embeddings(np)
        self.tokenizer = AutoTokenizer.from_pretrained(str(self.model_dir / "tokenizer"))

        onnx_dir = self.model_dir / self.variant
        self.sessions = {
            "prefill": ort.InferenceSession(
                str(onnx_dir / "talker_prefill.onnx"), providers=self.providers
            ),
            "decode": ort.InferenceSession(
                str(onnx_dir / "talker_decode.onnx"), providers=self.providers
            ),
            "cp": ort.InferenceSession(
                str(onnx_dir / "code_predictor.onnx"), providers=self.providers
            ),
            "vocoder": ort.InferenceSession(
                str(onnx_dir / "vocoder.onnx"), providers=self.providers
            ),
        }
        log(
            f"loaded {self.variant} sessions in {time.time() - t0:.1f}s "
            f"on {self.device} (prefill={self.config.get('talker_num_layers')}L "
            f"h={self.config.get('talker_hidden_size')})"
        )
        return self

    def _load_embeddings(self, np):
        edir = self.model_dir / "embeddings"
        d = {}
        for name in [
            "text_embedding",
            "text_projection_fc1_weight",
            "text_projection_fc1_bias",
            "text_projection_fc2_weight",
            "text_projection_fc2_bias",
            "talker_codec_embedding",
        ]:
            d[name] = np.load(str(edir / f"{name}.npy"))
        d["cp_codec_embeddings"] = []
        i = 0
        while True:
            path = edir / f"cp_codec_embedding_{i}.npy"
            if not path.exists():
                break
            d["cp_codec_embeddings"].append(np.load(str(path)))
            i += 1
        return d

    # --- helpers --------------------------------------------------------------
    def _text_proj(self, token_ids):
        np = self.emb["text_embedding"].__class__  # keep numpy ref local
        import numpy  # noqa: PLC0415

        emb = self.emb["text_embedding"][token_ids]
        hidden = emb @ self.emb["text_projection_fc1_weight"].T + self.emb["text_projection_fc1_bias"]
        activated = hidden * (1.0 / (1.0 + numpy.exp(-hidden)))
        return activated @ self.emb["text_projection_fc2_weight"].T + self.emb["text_projection_fc2_bias"]

    @staticmethod
    def _sample_top_k(logits, top_k, temperature, rng):
        np = logits.__class__  # unused
        import numpy  # noqa: PLC0415

        if temperature != 1.0:
            logits = logits / temperature
        if top_k > 0 and top_k < len(logits):
            top_k_idx = numpy.argpartition(logits, -top_k)[-top_k:]
            mask = numpy.full_like(logits, -numpy.inf)
            mask[top_k_idx] = logits[top_k_idx]
            logits = mask
        logits = logits - numpy.max(logits)
        probs = numpy.exp(logits)
        probs = probs / probs.sum()
        return int(rng.choice(len(probs), p=probs))

    # --- generation -----------------------------------------------------------
    def design(
        self,
        text: str,
        instruct: str,
        language: str,
        output_path: str,
        max_new_tokens: int = 2048,
        temperature: float = 0.9,
        top_k: int = 50,
        repetition_penalty: float = 1.05,
        seed: int | None = None,
    ) -> dict:
        import numpy  # noqa: PLC0415
        import soundfile as sf  # noqa: PLC0415

        self.load()
        if seed is not None:
            numpy.random.seed(seed)
            rng = numpy.random.default_rng(seed)
        else:
            rng = numpy.random.default_rng()
        cfg = self.config
        emb = self.emb
        c = cfg
        num_layers = c["talker_num_layers"]
        cp_num_layers = c["cp_num_layers"]
        cp_num_kv_heads = c["cp_num_kv_heads"]
        cp_head_dim = c["cp_head_dim"]
        num_code_groups = c["talker_num_code_groups"]
        vocab_size = c["talker_vocab_size"]
        codec_eos = c["codec_eos_token_id"]

        # --- tokenize -----------------------------------------------------------
        chat_text = f"<|im_start|>assistant\n{text}<|im_end|>\n<|im_start|>assistant\n"
        input_ids = self.tokenizer.encode(chat_text, add_special_tokens=False)
        instruct_tokens = None
        if instruct:
            instruct_text = f"<|im_start|>user\n{instruct}<|im_end|>\n"
            instruct_tokens = self.tokenizer.encode(instruct_text, add_special_tokens=False)

        # --- prefill embeddings ---------------------------------------------------
        language_id = c["codec_language_id"].get(language.lower())
        if language_id is not None:
            codec_prefix_ids = [
                c["codec_think_id"],
                c["codec_think_bos_id"],
                language_id,
                c["codec_think_eos_id"],
            ]
        else:
            codec_prefix_ids = [
                c["codec_nothink_id"],
                c["codec_think_bos_id"],
                c["codec_think_eos_id"],
            ]
        codec_emb = emb["talker_codec_embedding"]
        tts_pad = self._text_proj([c["tts_pad_token_id"]])[0]
        tts_bos = self._text_proj([c["tts_bos_token_id"]])[0]
        tts_eos = self._text_proj([c["tts_eos_token_id"]])[0]
        codec_pad = codec_emb[c["codec_pad_id"]]
        codec_bos = codec_emb[c["codec_bos_id"]]

        embeds_list = []
        if instruct_tokens is not None:
            embeds_list.append(self._text_proj(instruct_tokens))
        embeds_list.append(self._text_proj(input_ids[:3]))
        for cid in codec_prefix_ids:
            embeds_list.append((tts_pad + codec_emb[cid]).reshape(1, -1))
        embeds_list.append((tts_bos + codec_pad).reshape(1, -1))
        text_tokens = input_ids[3:-5]
        for tid in text_tokens:
            embeds_list.append((self._text_proj([tid])[0] + codec_pad).reshape(1, -1))
        embeds_list.append((tts_eos + codec_pad).reshape(1, -1))
        embeds_list.append((tts_pad + codec_bos).reshape(1, -1))
        prefill_embeds = numpy.concatenate(embeds_list, axis=0)[numpy.newaxis, :, :].astype(numpy.float32)
        T = prefill_embeds.shape[1]
        attention_mask = numpy.ones((1, T), dtype=numpy.int64)
        position_ids = numpy.arange(T).reshape(1, 1, T).repeat(3, axis=0)

        t0 = time.time()
        prefill_out = self.sessions["prefill"].run(
            None,
            {
                "inputs_embeds": prefill_embeds,
                "attention_mask": attention_mask,
                "position_ids": position_ids,
            },
        )
        logits = prefill_out[0]
        hidden_states = prefill_out[1]
        kv_outputs = prefill_out[2:]
        past_keys = numpy.stack([kv_outputs[i * 2] for i in range(num_layers)])
        past_values = numpy.stack([kv_outputs[i * 2 + 1] for i in range(num_layers)])
        trailing_hidden = tts_pad.reshape(1, -1)

        # --- decode loop -----------------------------------------------------------
        suppress_mask = numpy.zeros(vocab_size, dtype=bool)
        suppress_mask[vocab_size - 1024 : vocab_size] = True
        suppress_mask[codec_eos] = False
        all_codes = []
        current_pos = T
        generated_tokens = []
        for step in range(max_new_tokens):
            last_logits = logits[0, -1, :].copy()
            last_logits[suppress_mask] = -numpy.inf
            if step < 2:
                last_logits[codec_eos] = -numpy.inf
            if repetition_penalty != 1.0 and generated_tokens:
                seen = numpy.array(generated_tokens)
                scores = last_logits[seen]
                scores = numpy.where(scores > 0, scores / repetition_penalty, scores * repetition_penalty)
                last_logits[seen] = scores
            group0_token = self._sample_top_k(last_logits, top_k, temperature, rng)
            if group0_token == codec_eos:
                break
            generated_tokens.append(group0_token)
            frame_codes = [group0_token]
            talker_hidden = hidden_states[0, -1:, :]
            group0_embed = codec_emb[group0_token].reshape(1, -1)
            cp_input = numpy.concatenate([talker_hidden, group0_embed], axis=0)
            cp_input = cp_input[numpy.newaxis, :, :].astype(numpy.float32)
            cp_past_keys = numpy.zeros((cp_num_layers, 1, cp_num_kv_heads, 0, cp_head_dim), dtype=numpy.float32)
            cp_past_values = numpy.zeros((cp_num_layers, 1, cp_num_kv_heads, 0, cp_head_dim), dtype=numpy.float32)
            for g in range(num_code_groups - 1):
                cp_out = self.sessions["cp"].run(
                    None,
                    {
                        "inputs_embeds": cp_input,
                        "generation_steps": numpy.array([g], dtype=numpy.int64),
                        "past_keys": cp_past_keys,
                        "past_values": cp_past_values,
                    },
                )
                cp_logits = cp_out[0]
                cp_past_keys = cp_out[1]
                cp_past_values = cp_out[2]
                token = self._sample_top_k(cp_logits[0, -1, :], top_k, temperature, rng)
                frame_codes.append(token)
                cp_embed = emb["cp_codec_embeddings"][g][token].reshape(1, 1, -1).astype(numpy.float32)
                cp_input = cp_embed
            all_codes.append(frame_codes)

            next_embed = codec_emb[group0_token].copy()
            for g in range(num_code_groups - 1):
                next_embed = next_embed + emb["cp_codec_embeddings"][g][frame_codes[g + 1]]
            next_embed = next_embed + trailing_hidden[0]
            next_embed = next_embed.reshape(1, 1, -1).astype(numpy.float32)
            decode_mask = numpy.ones((1, current_pos + 1), dtype=numpy.int64)
            decode_pos = numpy.array([[[current_pos]]]).repeat(3, axis=0)
            decode_out = self.sessions["decode"].run(
                None,
                {
                    "inputs_embeds": next_embed,
                    "attention_mask": decode_mask,
                    "position_ids": decode_pos,
                    "past_keys": past_keys,
                    "past_values": past_values,
                },
            )
            logits = decode_out[0]
            hidden_states = decode_out[1]
            past_keys = decode_out[2]
            past_values = decode_out[3]
            current_pos += 1

        gen_time = time.time() - t0
        num_frames = len(all_codes)
        if num_frames == 0:
            raise RuntimeError("voice design produced no codec frames")

        codes_arr = numpy.array(all_codes, dtype=numpy.int64)  # (T, 16)
        codes_input = codes_arr.T[numpy.newaxis, :, :]  # (1, 16, T)
        wav = self.sessions["vocoder"].run(None, {"codes": codes_input})[0].flatten()

        out = Path(output_path)
        out.parent.mkdir(parents=True, exist_ok=True)
        sf.write(str(out), wav, SAMPLE_RATE)
        duration_ms = int(round(len(wav) / SAMPLE_RATE * 1000.0))
        return {
            "status": "ok",
            "output_path": str(out),
            "duration_ms": duration_ms,
            "sample_rate": SAMPLE_RATE,
            "frames": num_frames,
            "gen_seconds": round(gen_time, 2),
            "device": self.device,
        }


_engine = None


def get_engine() -> VoiceDesignEngine:
    global _engine
    if _engine is None:
        if not (MODEL_DIR / "config.json").exists():
            raise RuntimeError(
                f"voice design model not found at {MODEL_DIR}. "
                f"Run scripts/setup_voicedesign.sh to download wavekat/Qwen3-TTS-1.7B-VoiceDesign-ONNX."
            )
        _engine = VoiceDesignEngine(MODEL_DIR, VARIANT)
    return _engine


def handle_design(req):
    text = req.get("text", "")
    instruct = req.get("instruct", "")
    output_path = req.get("output_path", "")
    if not text or not output_path:
        raise ValueError("design requires text, output_path (instruct may be empty)")
    if not instruct.strip():
        raise ValueError("design requires a non-empty 'instruct' voice description")
    language = req.get("language", "english")
    max_tokens = int(req.get("max_tokens", 2048))
    temperature = float(req.get("temperature", 0.9))
    top_k = int(req.get("top_k", 50))
    repetition_penalty = float(req.get("repetition_penalty", 1.05))
    seed = req.get("seed")
    if seed is not None:
        seed = int(seed)
    return get_engine().design(
        text=text,
        instruct=instruct,
        language=language,
        output_path=output_path,
        max_new_tokens=max_tokens,
        temperature=temperature,
        top_k=top_k,
        repetition_penalty=repetition_penalty,
        seed=seed,
    )


def handle_health(_req):
    loaded = _engine is not None
    resp = {
        "status": "ok",
        "model_loaded": loaded,
        "model_dir": str(MODEL_DIR),
        "model_present": (MODEL_DIR / "config.json").exists(),
        "variant": VARIANT,
        "sample_rate": SAMPLE_RATE,
    }
    if loaded:
        resp["device"] = _engine.device
        resp["config"] = {
            "sample_rate": SAMPLE_RATE,
            "languages": len(_engine.config.get("codec_language_id", {})),
            "talker_layers": _engine.config.get("talker_num_layers"),
        }
    return resp


def _isolate_streams():
    """Protect the JSON protocol on stdout from library chatter.

    onnxruntime/transformers print progress to STDOUT/STDERR; reroute all
    Python-level + C-level writes to a log file and keep a private handle to
    the real stdout pipe for protocol JSON only (same pattern as the gepard
    sidecar — prevents JSON corruption and pipe-fill deadlocks).
    """
    import os  # noqa: PLC0415

    log_path = Path(os.environ.get("VOICEDESIGN_LOG", "/tmp/voicedesign_tts_sidecar.log"))
    log_path.parent.mkdir(parents=True, exist_ok=True)
    log_fd = os.open(str(log_path), os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o644)
    os.dup2(log_fd, 2)  # fd 2 -> log file (C-level stderr writes)
    proto_fd = os.dup(1)  # private handle to the real stdout pipe
    sys.stderr = os.fdopen(os.dup(2), "w", buffering=1)
    sys.stdout = os.fdopen(os.dup(2), "w", buffering=1)
    return os.fdopen(proto_fd, "w", buffering=1)


def _proto_write(proto, obj) -> None:
    proto.write(json.dumps(obj, ensure_ascii=False) + "\n")
    proto.flush()


def _dispatch(req) -> dict:
    op = req.get("op", "design")
    if op == "design":
        return handle_design(req)
    if op == "health":
        return handle_health(req)
    raise ValueError(f"unknown op: {op}")


def serve() -> int:
    proto = _isolate_streams()
    log(f"ready (model_dir={MODEL_DIR}, variant={VARIANT})")
    _proto_write(proto, {"ready": True})
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        op = "design"
        try:
            req = json.loads(line)
            op = req.get("op", "design")
            resp = _dispatch(req)
        except Exception as exc:  # protocol-level error -> structured response
            log(f"error handling {op!r}: {exc}")
            resp = {"status": "error", "error": str(exc)}
        _proto_write(proto, resp)
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description="VoiceDesign TTS sidecar (long-lived serve mode)")
    parser.add_argument("--serve", action="store_true", help="Run as long-lived stdin/stdout server")
    parser.add_argument("--instruct", help="Voice description (fresh-process mode)")
    parser.add_argument("--text", help="Text to synthesize (fresh-process mode)")
    parser.add_argument("--output", help="Output WAV path (fresh-process mode)")
    parser.add_argument("--language", default="english", help="Language code (default: english)")
    parser.add_argument("--seed", type=int, default=None, help="Sampling seed")
    args = parser.parse_args()

    if args.serve:
        return serve()
    if args.instruct and args.text and args.output:
        proto = _isolate_streams()
        resp = handle_design(
            {
                "instruct": args.instruct,
                "text": args.text,
                "output_path": args.output,
                "language": args.language,
                "seed": args.seed,
            }
        )
        _proto_write(proto, resp)
        return 0
    print(
        "usage: voicedesign_tts_sidecar.py --serve | --instruct I --text T --output OUT",
        file=sys.stderr,
    )
    return 2


if __name__ == "__main__":
    sys.exit(main())
