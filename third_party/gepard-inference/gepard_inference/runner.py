"""Gepard autoregressive TTS runner with text classifier-free guidance.

``GepardRunner`` is the canonical inference entry point: with the default
``cfg_scale=1.0`` it runs plain single-pass generation; raising ``cfg_scale``
turns on text classifier-free guidance.

Usage:
    runner = GepardRunner.from_checkpoint("nineninesix/kani3-dpo-cfg-round-2")
    tokens = runner.generate("Hello world", ref_codes=ref_codes,
                             temperature=0.3, cfg_scale=3.0)
    # tokens: (num_heads, T) — ready for UnfoldedCodecModel.decode_from_codes()

Why text-CFG
------------
On short inputs the K speaker-prefix tokens dominate the hidden state and the
1–2 text tokens drown → the model never "latches" and runs away. CFG runs two
forward passes per frame:

    cond   = [ prefix | SOT text EOT SOS | audio... ]   (full conditioning)
    uncond = [ prefix | SOT      EOT SOS | audio... ]   (SAME prefix, NO text)

and guides the per-head logits toward what the text adds:

    logit_guided = logit_uncond + cfg_scale * (logit_cond - logit_uncond)

The speaker prior (common to both branches) cancels in the difference, so the
text-specific direction is amplified. Cost: 2x forward per guided frame;
onset-only guidance (``cfg_frames``) keeps it to the first N frames.
"""

from typing import List, Optional

import torch
import torch.nn.functional as F
from transformers import AutoTokenizer
from transformers.models.qwen3_5.modeling_qwen3_5 import Qwen3_5DynamicCache
from safetensors.torch import load_file as load_safetensors

from .checkpoint_io import normalize_scalar_shapes, resolve_safetensors
from .configuration import load_gepard_config
from .modeling import GepardModel, build_model, resolve_dtype
from .text_repetition import TextRepetitionConfig, TextRepeater


class FullAttnCache(Qwen3_5DynamicCache):
    """Qwen3_5DynamicCache that works for full-attention-only models
    (no linear_attention layers)."""

    def __init__(self, config):
        # Skip parent __init__ — it crashes when layer_types has no "linear_attention"
        self.layer_types = config.layer_types
        self.transformer_layers = [
            i for i in range(config.num_hidden_layers)
            if self.layer_types[i] == "full_attention"
        ]
        self.last_linear_layer = -1  # no linear layers in this model
        self.conv_states = [None for _ in range(config.num_hidden_layers)]
        self.recurrent_states = [None for _ in range(config.num_hidden_layers)]
        self.key_cache = [None for _ in range(config.num_hidden_layers)]
        self.value_cache = [None for _ in range(config.num_hidden_layers)]


class GepardRunner:
    """Autoregressive inference for GepardModel with KV cache and text-CFG.

    Generates per-channel FSQ token sequences from text. The returned tensor
    shape (num_heads, T) is directly compatible with
    ``UnfoldedCodecModel.decode_from_codes()`` after unsqueezing the batch dim.

    Token format follows the training layout:
        text_ids = [BOS_text, ...text tokens..., EOT, BOS_audio]
        then audio frames are generated autoregressively.
    """

    def __init__(
        self,
        model: GepardModel,
        tokenizer,
        device: torch.device,
        repetition: Optional[TextRepetitionConfig] = None,
        special_tokens: Optional[dict] = None,
    ):
        self.model = model.eval()
        self.tokenizer = tokenizer
        self.device = torch.device(device)
        self.channel_names: List[str] = model.channel_names
        self.num_heads: int = model.num_codebook_heads
        # Key names follow the checkpoint's GepardConfig.special_tokens convention.
        st = special_tokens or {}
        self.BOS_TEXT = int(st["start_of_text"])
        self.EOT = int(st["end_of_text"])
        self.BOS_AUDIO = int(st["start_of_speech"])
        # Adaptive text repetition — must mirror the training-time layout
        # exactly (same special ids, same target/threshold).
        self.repetition_cfg = repetition or TextRepetitionConfig()
        self.repeater = TextRepeater(
            self.repetition_cfg, self.BOS_TEXT, self.EOT, self.BOS_AUDIO,
        )

    # ------------------------------------------------------------------
    # Factory
    # ------------------------------------------------------------------

    @classmethod
    def from_checkpoint(
        cls,
        checkpoint_path: str,
        device: Optional[str] = None,
        attn_implementation: str = "eager",
    ) -> "GepardRunner":
        """Load model from a self-describing checkpoint (local dir or HF repo id).

        The checkpoint must carry a ``gepard_config.json``; the runner rebuilds
        the exact architecture (backbone, audio heads, voice-cloning
        compressor) from the checkpoint alone — no training configs needed.

        Args:
            checkpoint_path: HF-style checkpoint (contains model.safetensors +
                gepard_config.json + tokenizer files).
            device: 'cuda', 'cpu', or None (auto-detect).
            attn_implementation: 'eager' or 'flash_attention_2'.
        """
        if device is None:
            device = "cuda" if torch.cuda.is_available() else "cpu"
        device = torch.device(device)

        gepard_cfg = load_gepard_config(checkpoint_path)
        if gepard_cfg is None:
            raise FileNotFoundError(
                f"{checkpoint_path!r} has no gepard_config.json — only "
                "self-describing checkpoints are supported by this package."
            )
        model = build_model(gepard_cfg, attn_implementation=attn_implementation)
        model_dtype = resolve_dtype(gepard_cfg.model_dtype)
        print(f"[{cls.__name__}] model config: {checkpoint_path}/gepard_config.json")

        # Load full state dict — covers backbone AND custom heads in one shot.
        sf_path = resolve_safetensors(checkpoint_path)
        state_dict = load_safetensors(sf_path, device="cpu")
        reshaped = normalize_scalar_shapes(state_dict, model)
        if reshaped:
            print(f"[{cls.__name__}] legacy 0-dim params reshaped to match model: {reshaped}")
        missing, unexpected = model.load_state_dict(state_dict, strict=False)
        if missing:
            print(f"[{cls.__name__}] missing keys ({len(missing)}): {missing[:3]} ...")
        if unexpected:
            print(f"[{cls.__name__}] unexpected keys ({len(unexpected)}): {unexpected[:3]} ...")

        model = model.to(device=device, dtype=model_dtype)

        tokenizer = AutoTokenizer.from_pretrained(checkpoint_path)
        repetition = TextRepetitionConfig.from_config(dict(gepard_cfg.text_repetition))

        return cls(
            model=model,
            tokenizer=tokenizer,
            device=device,
            repetition=repetition,
            special_tokens=dict(gepard_cfg.special_tokens),
        )

    # ------------------------------------------------------------------
    # Core generate
    # ------------------------------------------------------------------

    @torch.no_grad()
    def generate(
        self,
        text: str,
        ref_codes: Optional[torch.Tensor] = None,   # [1, T_ref, C_total] long — unfolded codec codes
        ref_mask: Optional[torch.Tensor] = None,    # [1, T_ref] bool, True = real frame
        temperature: float = 1.0,
        top_k: int = 0,
        stop_threshold: float = 0.5,
        max_frames: int = 2000,
        repetition_penalty: float = 1.0,
        repetition_window: int = 32,
        force_stop_frames: Optional[int] = None,
        cfg_scale: float = 1.0,
        cfg_frames: Optional[int] = None,
        cfg_uncond_mode: str = "empty_text",
    ) -> torch.LongTensor:
        """Generate audio tokens for ``text``, optionally with a reference voice
        and text classifier-free guidance.

        Args:
            text: Input text to synthesize.
            ref_codes: Unfolded codec codes for the reference voice, shape
                [1, T_ref, C_total]. Ignored if the model has no ref_compressor.
            ref_mask: Boolean mask for ref_codes, True = real frame. If None
                and ref_codes is given, all frames are assumed real.
            temperature: Sampling temperature (1.0 = no change).
            top_k: If > 0, keep only top-k logits before sampling.
            stop_threshold: Sigmoid threshold on the stop head to terminate.
            max_frames: Hard cap on the number of generated audio frames.
            repetition_penalty: >1.0 penalises tokens seen in the recent window
                per head. 1.0 = disabled. Typical: 1.1–1.3.
            repetition_window: Number of recent frames tracked for
                repetition_penalty. 0 = all history.
            force_stop_frames: Deterministic guardrail — hard-stop at this many
                frames regardless of the stop head. None = disabled.
            cfg_scale: Guidance weight w. 1.0 = disabled (plain single-pass).
                2.0–3.0 typical; higher = stronger text emphasis but risks
                artefacts / premature stop.
            cfg_frames: Onset-only guidance. If set, CFG is applied only to the
                first N frames; later frames use the cond branch alone (the
                uncond branch is then skipped to save compute). None = guide
                every frame.
            cfg_uncond_mode: How to build the text-free uncond branch:
                "empty_text" → [BOS_text, EOT, BOS_audio] (cleanest contrast);
                "audio_only" → [BOS_audio] (most aggressive; more OOD).

        Returns:
            LongTensor of shape (num_heads, T) — per-channel FSQ codes,
            compatible with ``UnfoldedCodecModel.decode_from_codes()`` after
            adding a batch dim: ``tokens.unsqueeze(0)``.
        """
        cfg_on = cfg_scale != 1.0

        # 1. Build cond text_ids (with adaptive repetition).
        text_token_ids = self.tokenizer.encode(text, add_special_tokens=False)
        cond_ids = self.repeater.expand(text_token_ids)

        # 1b. Build uncond text_ids (SAME prefix later, text removed).
        if cfg_uncond_mode == "empty_text":
            uncond_ids = [self.BOS_TEXT, self.EOT, self.BOS_AUDIO]
        elif cfg_uncond_mode == "audio_only":
            uncond_ids = [self.BOS_AUDIO]
        else:
            raise ValueError(f"unknown cfg_uncond_mode={cfg_uncond_mode!r}")

        # 2. Voice-cloning prefix (shared by both branches).
        prefix_embeds = self._compute_ref_prefix(ref_codes, ref_mask)

        # 3. Prefill cond (and uncond if CFG is on).
        cond_hidden, cond_cache, K, T_text_cond = self._prefill(prefix_embeds, cond_ids)
        if cfg_on:
            unc_hidden, unc_cache, _, T_text_unc = self._prefill(prefix_embeds, uncond_ids)
        else:
            unc_hidden = unc_cache = None
            T_text_unc = 0

        # 4. First audio frame from the BOS_audio position of each branch.
        guide_now = cfg_on and (cfg_frames is None or 0 < cfg_frames)
        first_frame = self._sample_frame(
            cond_hidden[:, -1:, :],
            unc_hidden[:, -1:, :] if guide_now else None,
            cfg_scale if guide_now else 1.0,
            temperature, top_k, repetition_penalty, None,
        )
        generated: List[torch.LongTensor] = [first_frame]

        # 5. Autoregressive loop.
        for step in range(1, max_frames):
            prev_frame = generated[-1]
            frame_embed = self._embed_frame(prev_frame.unsqueeze(0))  # (1, 1, d)

            # Attention mask length == total KV after this frame is appended:
            # prefix + text + (step-1) cached audio frames + current frame.
            cond_hidden, cond_cache = self._decode_step(
                frame_embed, cond_cache, K + T_text_cond + step,
            )
            guide_now = cfg_on and (cfg_frames is None or step < cfg_frames)
            if cfg_on and guide_now:
                unc_hidden, unc_cache = self._decode_step(
                    frame_embed, unc_cache, K + T_text_unc + step,
                )
            # Past the onset window guidance is off for good, so the uncond
            # forward is skipped entirely to save compute.

            # Deterministic guardrail: hard-stop runaway the stop head misses.
            if force_stop_frames is not None and len(generated) >= force_stop_frames:
                break

            # Stop decision from the COND branch (the real conditioned model).
            stop_logit = self.model.stop_head(cond_hidden[:, -1, :])  # (1, 1)
            stop_prob = torch.sigmoid(stop_logit.squeeze()).item()
            if stop_prob > stop_threshold:
                break

            if repetition_penalty != 1.0:
                window = generated if repetition_window == 0 else generated[-repetition_window:]
            else:
                window = None

            next_frame = self._sample_frame(
                cond_hidden,
                unc_hidden if (cfg_on and guide_now) else None,
                cfg_scale if (cfg_on and guide_now) else 1.0,
                temperature, top_k, repetition_penalty, window,
            )
            generated.append(next_frame)

        # 6. Stack frames: list of (num_heads,) → (num_heads, T)
        tokens = torch.stack(generated, dim=0).T.contiguous()
        return tokens

    # ------------------------------------------------------------------
    # Forward helpers
    # ------------------------------------------------------------------

    def _prefill(self, prefix_embeds, input_ids):
        """Run the prefill pass for one branch.

        Returns (hidden, past_key_values, K, T_text) where hidden is the full
        prefilled hidden state and the cache covers K + T_text positions.
        """
        text_ids = torch.tensor([input_ids], dtype=torch.long, device=self.device)  # (1, T_text)
        T_text = text_ids.shape[1]
        text_embeds = self.model.model.embed_tokens(text_ids)  # (1, T_text, d)

        if prefix_embeds is not None:
            inputs_embeds = torch.cat([prefix_embeds, text_embeds], dim=1)  # (1, K+T_text, d)
            K = prefix_embeds.size(1)
        else:
            inputs_embeds = text_embeds
            K = 0

        attn_mask = torch.ones(1, K + T_text, dtype=torch.long, device=self.device)
        past_key_values = FullAttnCache(self.model.config)
        out = self.model.model(
            inputs_embeds=inputs_embeds,
            attention_mask=attn_mask,
            use_cache=True,
            past_key_values=past_key_values,
        )
        return out.last_hidden_state, out.past_key_values, K, T_text

    def _decode_step(self, frame_embed, past_key_values, kv_len):
        """One AR decode step for one branch. Returns (hidden, past_key_values)."""
        attn_mask = torch.ones(1, kv_len, dtype=torch.long, device=self.device)
        out = self.model.model(
            inputs_embeds=frame_embed,
            attention_mask=attn_mask,
            use_cache=True,
            past_key_values=past_key_values,
        )
        return out.last_hidden_state, out.past_key_values

    def _compute_ref_prefix(self, ref_codes, ref_mask):
        """Voice-cloning prefix (K speaker tokens) if ref_compressor is present."""
        if self.model.ref_compressor is None or ref_codes is None:
            return None
        ref_codes = ref_codes.to(self.device)
        if ref_mask is None:
            ref_mask = torch.ones(
                ref_codes.shape[0], ref_codes.shape[1],
                dtype=torch.bool, device=ref_codes.device,
            )
        prefix_embeds, _ = self.model.ref_compressor(ref_codes, ref_mask)  # [1, K, d]
        return prefix_embeds

    # ------------------------------------------------------------------
    # Sampling helpers
    # ------------------------------------------------------------------

    def _embed_frame(self, frame_tokens: torch.LongTensor) -> torch.FloatTensor:
        """Embed a single audio frame via the model's audio-embedding stack.

        Args:
            frame_tokens: (B, num_heads) — one token per channel
        Returns:
            (B, 1, d)
        """
        channel_tokens = [frame_tokens[:, i] for i in range(self.num_heads)]
        emb = self.model._embed_audio(channel_tokens)  # (B, d)
        return emb.unsqueeze(1)  # (B, 1, d)

    def _sample_head_logits(
        self,
        logits: torch.FloatTensor,
        head_index: int,
        vocab_size: int,
        temperature: float,
        top_k: int,
        repetition_penalty: float,
        recent_frames: Optional[List[torch.LongTensor]],
    ) -> torch.LongTensor:
        """Apply repetition penalty / temperature / top-k to one head's fp32
        logits and sample."""
        if repetition_penalty != 1.0 and recent_frames:
            seen = {t[head_index].item() for t in recent_frames}
            for tok in seen:
                if logits[0, tok] > 0:
                    logits[0, tok] /= repetition_penalty
                else:
                    logits[0, tok] *= repetition_penalty

        if temperature != 1.0:
            logits = logits / temperature

        if top_k > 0:
            k = min(top_k, vocab_size)
            topk_vals, _ = torch.topk(logits, k)
            threshold = topk_vals[:, -1:]
            logits = logits.masked_fill(logits < threshold, float("-inf"))

        probs = F.softmax(logits, dim=-1)
        return torch.multinomial(probs, num_samples=1).squeeze()  # (1,1) → scalar

    def _sample_frame(
        self,
        cond_hidden: torch.FloatTensor,
        uncond_hidden: Optional[torch.FloatTensor],
        cfg_scale: float,
        temperature: float,
        top_k: int,
        repetition_penalty: float = 1.0,
        recent_frames: Optional[List[torch.LongTensor]] = None,
    ) -> torch.LongTensor:
        """Sample one audio frame from all codebook heads independently.

        If ``uncond_hidden`` is given and ``cfg_scale != 1.0``, per-head logits
        are guided BEFORE temperature/top-k/sampling:
            logit = logit_uncond + cfg_scale * (logit_cond - logit_uncond)
        Guidance is applied in fp32 logit space, on raw head outputs.
        """
        h_c = cond_hidden[:, -1, :]  # (1, d)
        h_u = uncond_hidden[:, -1, :] if uncond_hidden is not None else None
        do_cfg = h_u is not None and cfg_scale != 1.0

        frame_tokens = []
        for i, (head, vocab_size) in enumerate(zip(self.model.codebook_heads, self.model.vocab_sizes)):
            logits = head(h_c).float()  # (1, vocab_size) — fp32 for sampling stability
            if do_cfg:
                logits_u = head(h_u).float()
                logits = logits_u + cfg_scale * (logits - logits_u)

            token = self._sample_head_logits(
                logits, i, vocab_size, temperature, top_k, repetition_penalty, recent_frames,
            )
            frame_tokens.append(token)

        return torch.stack(frame_tokens)  # (num_heads,)
