# Gepard Model & Inference Guide

This document is the technical reference for the **Gepard** autoregressive speech
model as it is used in *this* inference repository: what the model is, how it is
put together, and how it turns text into speech. It is condensed from the full
project guide — the training pipeline, dataset construction, and the training
project's configuration system have been removed. Only what you need to
understand the model and run inference remains.

## Contents

- [1. Introduction and Serving Architecture](#1-introduction-and-serving-architecture)
  - [1.1. Overarching Design Principle](#11-overarching-design-principle)
  - [1.2. Concurrency and Latency Benchmarks](#12-concurrency-and-latency-benchmarks)
- [2. Acoustic Backbone and Codec Interface](#2-acoustic-backbone-and-codec-interface)
  - [2.1. Stock Transformer Backbone](#21-stock-transformer-backbone)
  - [2.2. Codec: GroupFSQ Orthogonality](#22-codec-groupfsq-orthogonality)
  - [2.3. Mixed-Radix Unfolding to 32 Heads](#23-mixed-radix-unfolding-to-32-heads)
- [3. The Model: Architecture](#3-the-model-architecture)
  - [3.1. Anatomy: Stock Core, Thin Overlay](#31-anatomy-stock-core-thin-overlay)
  - [3.2. The Backbone](#32-the-backbone)
  - [3.3. Audio Interface — Input Side](#33-audio-interface--input-side)
  - [3.4. Output Side](#34-output-side)
  - [3.5. Forward Contract](#35-forward-contract)
- [4. Voice Cloning](#4-voice-cloning)
  - [4.1. Input Representation: Codec Codes, Not Waveforms](#41-input-representation-codec-codes-not-waveforms)
  - [4.2. `RefCompressor` Architecture](#42-refcompressor-architecture)
  - [4.3. The K-Query Bottleneck Is the Central Design Bet](#43-the-k-query-bottleneck-is-the-central-design-bet)
  - [4.4. The Unconditional Path: `null_prefix`](#44-the-unconditional-path-null_prefix)
  - [4.5. Generation-Time Usage](#45-generation-time-usage)
- [5. Short Register Failure Mode and Text Repetition](#5-short-register-failure-mode-and-text-repetition)
  - [5.1. Onset Entropy Probe](#51-onset-entropy-probe)
  - [5.2. Text Repetition Layout](#52-text-repetition-layout)
  - [5.3. Token Budget Calibration](#53-token-budget-calibration)
- [6. Inference](#6-inference)
  - [6.1. The Classes](#61-the-classes)
  - [6.2. Loading](#62-loading)
  - [6.3. Generation Anatomy](#63-generation-anatomy)
  - [6.4. CFG Is Optional — and Off by Default](#64-cfg-is-optional--and-off-by-default)
  - [6.5. The Notebook Flow](#65-the-notebook-flow)
- [7. Self-Describing Checkpoints](#7-self-describing-checkpoints)

---

## 1. Introduction and Serving Architecture

Gepard is an autoregressive decoder-only model designed for real-time spoken dialogue, generating speech tokens directly from text prompts. The primary design goal is **standard engine serving compatibility** (such as with vLLM): the model runs on stock inference engines without custom CUDA kernels or custom layers in the autoregressive loop.

### 1.1. Overarching Design Principle
To ensure compatibility with continuous batching and PagedAttention mechanisms, Gepard enforces a strict separation:
*   **Prefill Phase (Offline/Static):** All custom sequence preparations—such as speaker-profile extraction via a Q-Former compressor and adaptive text repetition—are computed once during prompt prefill and are excluded from the step-by-step autoregressive decode loop.
*   **Generation Phase (Online/Autoregressive):** The model operates as a flat, single-pass decoder generating speech frames sequentially, preserving a clean KV-caching loop. Complex two-pass generation techniques like Classifier-Free Guidance (CFG) are distilled into the weights via Direct Preference Optimization (DPO) — a training-time step. The result is that at inference the shipped model runs **single-pass** and needs no two-pass guidance (§6.4).

### 1.2. Concurrency and Latency Benchmarks
Under serving workloads on server-class hardware running in 16-bit precision, Gepard streams audio chunk-by-chunk using the SSE protocol:
*   **Single Stream:** Achieves a Real-Time Factor (RTF) of approximately `0.067` (about 15 times faster than real-time) with a Time-to-First-Audio (TTFA) of ~0.046 seconds.
*   **Concurrent Scaling:** Aggregate throughput scales linearly, reaching system-level speeds of over `200x` real-time throughput at 256 concurrent streams on a single server-class GPU.
*   **Interactive Limit:** The optimal operating range is 64 to 128 simultaneous streams per GPU, beyond which compute saturation causes individual streams to fall behind real-time.

---

## 2. Acoustic Backbone and Codec Interface

### 2.1. Stock Transformer Backbone
The core transformer is a stock decoder-only architecture (based on Qwen3.5 with 14 layers, a hidden dimension of 1024, and 8 attention heads). All custom linear block overrides are removed to maintain standard FlashAttention-2 compatibility. The backbone's parameters are defined and instantiated in `gepard_inference/modeling.py`.

### 2.2. Codec: GroupFSQ Orthogonality
Gepard tokenizes speech using a neural codec operating at a frame rate of 21.5 Hz with a low bitrate (~1.89 kbps).

Instead of Residual Vector Quantization (RVQ), which introduces hierarchical dependencies (where each codebook layer quantizes the residual of the previous layer), Gepard is built around **GroupFSQ (Finite Scalar Quantization)**. The latent space is divided into 8 independent groups, each quantized by its own FSQ grid of levels (yielding 2,016 code capacities per group).

Because the channels are orthogonal and independent by design, the mutual information between channels given the hidden state is negligible. This allows **factorized parallel sampling** across all channels in a single autoregressive step, rendering vertical depth-transformers or codebook-by-codebook sampling loops unnecessary.

### 2.3. Mixed-Radix Unfolding to 32 Heads
The 8 packed codebook tokens (values 0 to 2015) are unfolded into 32 independent channels per frame with repeating alphabet capacities of `[8, 7, 6, 6]`. This is done using a little-endian mixed-radix decomposition implemented in `gepard_inference/codec_ops.py`.

Predicting these 32 channels directly via 32 tiny linear heads (totaling 216 logits) instead of 8 heads of size 2016 significantly reduces classification layer overhead, bypasses redundant decompression steps during inference, and aligns the backbone outputs directly with the codec's internal dequantization stage.

---

## 3. The Model: Architecture

This chapter is the engineering reference for `GepardModel`
(`gepard_inference/modeling.py`): exactly which parts are the stock backbone and
which are the TTS overlay, and how the pieces wire together at inference. The
voice-cloning compressor gets its own chapter (§4); here it appears only as a
module in the parameter map.

### 3.1. Anatomy: Stock Core, Thin Overlay

`GepardModel` is a plain `nn.Module` (deliberately **not** a
`PreTrainedModel` — see §7 for the checkpointing consequences) wrapping one
stock module and a thin overlay. The parameter-name prefixes below are a
public contract: LoRA targeting and the checkpoint key layout key off them.

| Prefix | Module | Origin | Shape drivers |
|---|---|---|---|
| `model.*` | `Qwen3_5TextModel` | **stock transformers** — zero overrides | nested backbone config |
| `audio_embeddings.{0..31}.*` | 32 × `nn.Embedding(L_i, 32)` | overlay | `audio_heads`, `audio_embed_dim` |
| `audio_embed_proj.*` | `Linear(1024→d) → GELU → Linear(d→d) → LayerNorm(no affine)` | overlay | `audio_embed_dim × 32`, backbone `hidden_size` |
| `audio_embed_scale` | scalar **buffer** (not a parameter) | overlay | — (set from text-embedding std) |
| `codebook_heads.{0..31}.*` | 32 × `nn.Linear(d, L_i)` | overlay | `audio_heads` |
| `stop_head.*` | `nn.Linear(d, 1)` | overlay | — |
| `ref_compressor.*` | Q-Former (§4) | overlay, **only when VC enabled** | codec geometry, compressor dims |
| `null_prefix` | `nn.Parameter[K, d]` | overlay, only when VC enabled | `compressor.num_queries` |

What is deliberately **absent**: the backbone's `lm_head` (text generation is
not a task — it is discarded at load), and any custom module *inside*
the decoder stack. Everything TTS-specific lives strictly before
`model.embed-level` inputs or after `model` outputs, which is what keeps the
autoregressive loop stock-engine-servable (§1.1).

Text tokens are embedded by the backbone's own `model.embed_tokens` — the
overlay adds no text path of its own, so pretrained text representations are
reused as-is.

### 3.2. The Backbone

*   **Class and variant.** `Qwen3_5TextModel` from `transformers`, built from
    the repo `nineninesix/qwen3_5-full-attn-only-14`: a Qwen3.5 export whose
    `layer_types` are all `"full_attention"` (14 layers, hidden 1024, 8 heads,
    GQA with 2 KV heads). Full-attention-only matters twice: FlashAttention-2
    applies to every layer, and every layer carries the `q/k/v/o_proj` set
    that LoRA targets (mixed linear-attention layers would not).
*   **`partial_rotary_factor` — read this before touching RoPE.** Since
    transformers 5.x the value lives in TWO places: the flat top-level config
    attribute and `config.rope_parameters["partial_rotary_factor"]`. The HF
    model computes RoPE **only from the nested copy**; vLLM reads **only the
    flat copy**; the constructor does not keep them in sync, and the stock
    backbone repo itself ships them diverged (flat 0.25 vs effective nested
    1.0). Gepard therefore treats `model.partial_rotary_factor` (default 1.0
    = full rotary coverage) as authoritative and forces it into **both**
    copies at every seam: model load
    (`GepardModel.from_pretrained(partial_rotary_factor=…)`, applied *before*
    the backbone is built so `inv_freq` is computed with it), checkpoint
    reconstruction (`reconcile_backbone_config` inside `build_model`), and
    every written `config.json` (§7). RoPE is parameter-free, so the
    override never conflicts with checkpoint weights. Helpers:
    `set_partial_rotary_factor` / `effective_partial_rotary_factor` in
    `gepard_inference/configuration.py`.
*   **Attention implementation** is a runtime choice, not a weight property:
    the runner loads with `eager` for inference
    (`from_checkpoint(attn_implementation="eager")` strips any serialized attn
    setting and re-applies the override); FlashAttention-2 is a training-time
    optimization.

### 3.3. Audio Interface — Input Side

One audio frame is 32 discrete codes (§2.3). The frame embedding is:

```
level_audio_0..31  ──►  32 × Embedding(L_i, 32)   # per-channel lookup
                   ──►  concat → [B, T, 1024]
                   ──►  Linear(1024→1024) → GELU → Linear(1024→1024)
                   ──►  LayerNorm(elementwise_affine=False)   # unit-norm frame
                   ──►  × audio_embed_scale                    # buffer ≈ text-emb std
```

Design facts an engineer must not "fix" without understanding:

*   **MLP, not sum.** A sum of per-codebook lookups is an additive (linear)
    function of the channels; the 2-layer GELU MLP models cross-codebook
    interactions within a frame.
*   **The LayerNorm is affine-free on purpose.** The backbone's input RMSNorm
    makes the audio-embedding *magnitude* a free direction — it would drift
    unbounded and push the GELU toward degenerating into a linear map. The
    affine-free LN pins the scale; do not add affine parameters back.
*   **`audio_embed_scale` is a buffer, not a parameter.** Set once to the
    pretrained text-embedding std (`embed_tokens.weight.std()`), it rescales
    the unit-norm frame so audio embeddings are in-distribution next to text
    (the backbone itself discards scale via RMSNorm; the match matters for the
    ref-compressor and diagnostics). Being a persisted buffer, it survives
    checkpoint/resume — a loaded checkpoint carries its trained value.

### 3.4. Output Side

Applied to the backbone hidden states **after slicing off the K prefix
positions** (`hidden[:, K:, :]`): the audio labels span `[text | audio]` and
know nothing about the prefix, so forgetting this slice breaks the causal-shift
alignment silently — it is the single most fragile index in the model.

*   `codebook_heads` — 32 independent `Linear(1024, L_i)`, 216 logits total
    per position (§2.3: two orders of magnitude cheaper than 8×2016 heads).
*   `stop_head` — `Linear(1024, 1)`, a per-position Bernoulli "this frame is
    the last" predictor. At inference it is thresholded (`stop_threshold`,
    default 0.5).

### 3.5. Forward Contract

`forward(text_ids, attention_mask, labels_stop=None, **kwargs)`:

*   Audio inputs arrive **by channel name** in `kwargs` (`level_audio_i`) — the
    collator outputs and `audio_heads` keys are the same namespace by
    construction, and head order is positional (hence the ordered-dict
    discipline in §7).
*   Sequence assembly: `[prefix? | text_embeds | audio_embeds]`, attention
    mask extended with ones over the prefix.
*   Returns `MultiheadTTSOutput(loss, logits_audio: List[32], logits_stop)`.
    KV-cache generation lives in the runner (§6), not in `forward` itself.

---

## 4. Voice Cloning

Voice cloning extracts speaker characteristics from a reference clip once during
prompt prefill and prepends them as sequence-prefix tokens. Worth stating plainly
what makes this subsystem interesting as engineering: speaker identity is carried
**entirely in activation space** — K prefix vectors computed on the fly from the
same discrete codec codes the decoder already speaks. There is no speaker
embedding table, no enrollment step, no per-voice fine-tune, no separate audio
encoder (mel/SSL) at serving time, and the whole path is prefill-only, so it
costs nothing in the autoregressive loop and survives stock-engine serving
(§1.1).

### 4.1. Input Representation: Codec Codes, Not Waveforms

The compressor consumes a reference clip as `[B, T_ref, C]` **discrete FSQ
codes** — the exact currency of the rest of the model — not waveforms or
spectrograms. Codes are dequantized to their FSQ lattice values in `[-1, 1]`
(`dequantize_codes`), so the input is a compact float matrix
(`C_total = 32` channels at 21.5 fps) that is already speech-specific.
Consequences:

*   Any audio the codec can encode is a valid reference; at serving, encoding
    the user's clip through the codec is the only preprocessing.
*   No second audio frontend to ship, version, or keep in dtype/device sync.
*   The compressor tolerates both on-disk layouts: packed per-layer codes are
    unfolded in-forward; unfolded codes are consumed as-is
    (`do_unfold_in_forward = not codec.do_unfold`).

### 4.2. `RefCompressor` Architecture

`gepard_inference/ref_compressor.py` — a Q-Former-style bottleneck:

```
ref_codes [B, T_ref, C] ─ dequantize ─► Linear(C → d) ─ + sinusoidal PE ─► ref_feats
queries   nn.Parameter[K=8, d=1024]  ─ batch-expand ──────────────────────► q

× L=2 blocks (pre-norm RMSNorm everywhere):
    q = q + SelfAttn(q)                        # bidirectional, queries only
    q = q + CrossAttn(q ← ref_feats, key_padding_mask=ref_mask)
    q = q + SwiGLU_FFN(q)

q_normed = RMSNorm(q)                          # RMS = 1 per token
prefix   = output_scale · q_normed             # what the decoder consumes
```

Facts with reasons:

*   **Queries are position-less**; order carries no meaning. The reference
    gets sinusoidal PE so the cross-attention can exploit temporal structure
    of the clip, but the *output* is a set, not a sequence.
*   **`output_scale` starts at `1/√d_model`.** RMSNorm alone gives per-token
    RMS = 1, i.e. L2 ≈ √d ≈ 32 — which would dwarf the text/audio embeddings
    (scale ~1) it sits next to in the decoder input. The learnable scalar
    starts the prefix at L2 ≈ 1 and lets training rescale if useful. This is
    the prefix-side mirror of `audio_embed_scale` (§3.3): every input stream
    into the backbone gets its scale pinned explicitly.
*   Attention is plain SDPA with a key-padding mask over reference padding;
    self-attention over the 8 queries needs no mask.
*   `d_model = 1024` equals the backbone hidden size — the prefix is
    injected directly into the decoder's embedding stream with no adapter.

### 4.3. The K-Query Bottleneck Is the Central Design Bet

`num_queries: 8` is not a capacity knob to casually raise. The reconstruction
objective actively incentivizes the compressor to smuggle *content* — copy the
reference's spectral sequence so the decoder can cheat — rather than abstract
timbre. K is the structural mechanism that holds that leakage down:

**K=8 tokens for a 64–322-frame reference** is an 8–40× temporal compression:
there is simply no room to encode the frame sequence. (Two training-time
regularizers — a diversity/hinge-variance term and a supervised-contrastive
term — further shape the surviving capacity toward speaker-discriminative,
content-invariant features, but the bottleneck itself is the architectural bet.)

The bottleneck is also what makes the *same-audio* reference path safe:
feeding the target's own audio as the reference (the single-speaker fallback)
without the model degenerating into copy-through relies on K being small.

### 4.4. The Unconditional Path: `null_prefix`

A learnable `nn.Parameter[K, d]` that **replaces** the compressor output,
defining the model's *unconditional* branch (generation with no speaker
conditioning). It serves two inference-relevant consumers:

*   **Classifier-free guidance** needs a *trained* unconditional branch to
    guide against (§5, §6.4).
*   **No-reference generation** at serving falls back to it implicitly.

During training the model is exposed to this branch by swapping the real
compressor prefix for `null_prefix` on a fraction of samples ("CFG-dropout")
and by routing low-frequency / speaker-less rows to it. That exposure is what
makes the unconditional path well-trained enough for CFG to work at inference.

### 4.5. Generation-Time Usage

*   **Runner path.** `runner.generate(text, ref_codes=…)` →
    `_compute_ref_prefix`: one compressor call during prefill, prefix
    prepended to the text embeddings, and the autoregressive loop never sees
    the compressor again (§1.1). `ref_codes` at inference are `[1, T_ref, 32]`
    unfolded codes — produce them with `UnfoldedCodecModel.encode` +
    `unfold_tokens` (see `inference_demo.ipynb`).
*   **CFG shares the prefix.** In `GepardRunner` both the conditioned and the
    unconditioned branch carry the **same** speaker prefix; only the text is
    removed from the uncond branch. The speaker prior is common mode and
    cancels in `logit_uncond + w·(logit_cond − logit_uncond)` — guidance
    amplifies the *text* direction specifically, which is why text-CFG fixes
    prefix dominance instead of fighting the voice (§5).
*   **No reference** → no prefix at all (legacy path), which works because the
    null/CFG training exposed the model to text-only conditioning; passing
    nothing is not the same as passing `null_prefix`, but both are
    in-distribution.

---

## 5. Short Register Failure Mode and Text Repetition

On short text inputs (1 to 2 words), the speaker prefix (8 tokens) dominates the causal self-attention states of the transformer, weakening the text conditioning. The model fails to lock onto text alignment, resulting in high-entropy frame predictions and infinite generation loops (runaway). This is the failure mode that both text repetition and CFG exist to fix.

### 5.1. Onset Entropy Probe
Frame-by-frame token negative log-likelihood (NLL) and belief entropy over the 32 heads can be tracked during generation to diagnose derailment:
*   **Clean Generation:** Belief entropy drops rapidly within the first 50 frames as the model locks onto speech generation.
*   **Derailed Loop:** Belief entropy remains flat and elevated throughout the generation.
*   The minimum word error rate (WER) across multiple temperature-scaled rollouts remains zero, confirming that the model knows the vocabulary but fails due to generation instability.

### 5.2. Text Repetition Layout
To strengthen text conditioning on short prompts, the input text is repeated multiple times before the canonical copy (implemented in `gepard_inference/text_repetition.py`):

```
[ (SOT text EOT) x (R-1) | SOT text EOT SOS | audio ... ]
```

*   **SOS Gating:** The Start of Speech (SOS) token is attached only to the final copy, which triggers audio generation; the context copies are read by self-attention but are never voiced.

The runner replays the **same deterministic repetition policy** the checkpoint was trained under — the effective layout values are stamped into `gepard_config.json` (§7), so inference reproduces the training-time layout exactly without any manual configuration.

### 5.3. Token Budget Calibration
Sweeping the failure rate against the text-token budget reveals three regimes:
1.  **Cliff Zone (≤ 6 tokens):** High failure rates (60–96%).
2.  **Transition Zone (7–12 tokens):** Failure rate decreases.
3.  **Plateau Zone (≥ 13 tokens):** Failure rate stabilizes below 5% for familiar voices.

This sweep justifies a target text token budget of `16` and a threshold of `13` tokens for applying repetition — the values the runner uses.

---

## 6. Inference

The in-repo inference stack is the **reference implementation**: it is what the
demo notebook uses and the semantic ground truth that production serving must
match. Production itself is a vLLM deployment of the single-pass model (§1);
nothing in this chapter is required at serving time except the checkpoint files
(§7).

### 6.1. The Classes

Everything lives in `gepard_inference/`:

| Class | File | Role |
|---|---|---|
| `GepardRunner` | `runner.py` | **The canonical runner.** `TTSRunner` + optional text-CFG; with the default `cfg_scale=1.0` it is exactly the single-pass generator. Use this one. |
| `TTSRunner` | `runner.py` | The plain single-pass base class, driving the forward helpers directly. |
| `FullAttnCache` | `runner.py` | KV-cache shim: the stock `Qwen3_5DynamicCache` constructor crashes on a model whose `layer_types` contain no `linear_attention` entries — which is precisely our full-attention-only backbone. The shim re-implements init for the all-full-attention case. |
| `UnfoldedCodecModel` | `codec_wrapper.py` | NeMo `AudioCodecModel` subclass that (a) strips the training-only SLM discriminator from the config before init (skips a ~360 MB WavLM download), and (b) adds `decode_from_codes` — direct waveform decode from the 32 unfolded per-dimension codes, bypassing mixed-radix recomposition. Needs the NeMo stack. |

### 6.2. Loading

`GepardRunner.from_checkpoint(path_or_repo)` — one call, self-describing
(§7): `gepard_config.json` → `build_model` → safetensors → tokenizer, with
`attn_implementation="eager"` as the inference default (FlashAttention-2 is a
training optimization; eager keeps the runner CPU-capable and
dependency-light) and device auto-detect. Legacy checkpoints need
`fallback=<composed config>`; everything exported by the current trainer
loads standalone.

### 6.3. Generation Anatomy

`generate(text, ref_codes=None, …)` is a hand-rolled AR loop over the
**backbone only** (the overlay is applied manually each step):

1.  **Text layout** — tokenizer + the same deterministic `TextRepeater`
    policy the checkpoint was trained under (§5.2), from the stamped
    `text_repetition` config.
2.  **Prefix** — one `RefCompressor` call when `ref_codes` are given (§4.5);
    omitted entirely otherwise.
3.  **Prefill** — `[prefix? | text]` through the backbone with
    `use_cache=True` into a `FullAttnCache`; the hidden state at the SOS
    position seeds the first frame.
4.  **Frame loop** — embed the previous frame through the audio-embedding
    stack (§3.3) → one cached decode step → stop decision
    (`sigmoid(stop_head) > stop_threshold`) → sample all 32 heads
    independently in fp32 (temperature → top-k → multinomial, with optional
    repetition penalty over a sliding window of recent frames).
5.  Output: `(num_heads, T)` long tensor — `unsqueeze(0)` and feed straight
    to `UnfoldedCodecModel.decode_from_codes`.

Knobs (defaults): `temperature=1.0` (0.4 is the production operating point),
`top_k=0` (off), `stop_threshold=0.5`, `max_frames=2000` (hard ceiling),
`repetition_penalty=1.0` + `repetition_window=32`, and `force_stop_frames` — a
deterministic guardrail that truncates regardless of the stop head.

### 6.4. CFG Is Optional — and Off by Default

`GepardRunner.generate` adds three arguments; **with `cfg_scale=1.0` (the
default) none of the CFG machinery runs** — no second prefill, no extra
forward per frame, bit-identical to the base single-pass path. Turning it on:

*   `cfg_scale > 1.0` — a second, text-free branch is prefilled (same speaker
    prefix, §4.5) and per-head logits are guided:
    `logit = logit_uncond + cfg_scale · (logit_cond − logit_uncond)` before
    temperature/top-k. Typical 2.0–3.0.
*   `cfg_frames = N` — onset-only guidance: after frame N the uncond branch
    is dropped entirely (the derailment CFG fixes is born in the first frames,
    §5); `None` guides every frame.
*   `cfg_uncond_mode` — how the text-free branch is built: `"empty_text"`
    (`[SOT EOT SOS]`, cleanest contrast, default) or `"audio_only"` (`[SOS]`,
    more aggressive, more OOD).

One trap: `cfg_frames=0` with `cfg_scale>1` disables guidance but still pays
the uncond prefill — the off switch is `cfg_scale=1.0`.

When to reach for CFG: short prompts on a checkpoint that has NOT been
through DPO (CFG at scale 3 is the crutch DPO later distills away). A post-DPO
checkpoint is designed to run single-pass — CFG remains available as a quality
lever for pathological inputs, but the production configuration is
`cfg_scale=1.0`, which is also the only vLLM-compatible mode (two-pass
guidance does not fit continuous batching, §1.2).

### 6.5. The Notebook Flow

`inference_demo.ipynb` is the end-to-end template:

```python
player = UnfoldedCodecModel.from_pretrained("nvidia/nemo-nano-codec-22khz-1.89kbps-21.5fps").to(device).eval()
model  = GepardRunner.from_checkpoint("nineninesix/gepard-1.0")

# reference voice → unfolded codes [1, T_ref, 32]
wave, _ = librosa.load("ref_audio/audio_en.wav", sr=22050)
tokens, _ = player.encode(audio=wave_tensor, audio_len=wave_len)
ref_codes = unfold_tokens(tokens.cpu(), num_levels=[8, 7, 6, 6]).permute(0, 2, 1).to(device)

out = model.generate(text, ref_codes=ref_codes, temperature=0.4)   # single-pass
audio, _ = player.decode_from_codes(out.unsqueeze(0), enc_len)     # 22.05 kHz waveform
```

The `ref_codes` shape contract is `[1, T_ref, 32]` unfolded int64 codes.
`unfold_tokens` + `dequantize_codes` live in `gepard_inference/codec_ops.py`
and are shared by the compressor and this path — one mixed-radix implementation
everywhere.

---

## 7. Self-Describing Checkpoints

A Gepard checkpoint carries everything the runner needs to rebuild the model
with **zero Hub access for the architecture and zero training configs**. Two
files coexist by design and never collide (different filenames, different
readers):

*   **`gepard_config.json`** — the complete self-description (backbone config
    nested inside it, audio-head layout, codec identity, special-token map,
    and the `text_repetition` layout values). This is what
    `GepardRunner.from_checkpoint` / `build_model` read to reconstruct the
    exact model.
*   **`config.json`** — the *backbone-only* HF-standard config for the external
    ecosystem (vLLM serving, `AutoConfig`, Hub tooling); it knows nothing of
    the audio heads. `partial_rotary_factor` is guaranteed present in both flat
    and nested form (§3.2).
*   **`model.safetensors`** — the weights. **`tokenizer.json` /
    `tokenizer_config.json` / `chat_template.jinja`** — loaded via
    `AutoTokenizer`.

Deleting `config.json` breaks serving; deleting `gepard_config.json` degrades
the checkpoint to the legacy fallback path.

**Reading flow.** `from_checkpoint` resolves files through
`gepard_inference/checkpoint_io.py::resolve_checkpoint_file` (local file /
local dir / HF Hub, uniformly), then:

```
gepard_config.json found ──► build_model(cfg, attn_implementation="eager")
                             └ backbone from nested config, rotary reconciled
     state_dict ◄── model.safetensors, load_state_dict(strict=False)
     tokenizer  ◄── AutoTokenizer(checkpoint)
     runner     ◄── special_tokens + text_repetition from the SAME config
no gepard_config.json      ──► fallback= composed config tree (legacy), or
                               FileNotFoundError with re-export guidance
```

Two load-bearing serialization facts:

*   **`audio_heads` order IS the head wiring.** The config is serialized
    **without key sorting** — lexicographic sorting would put `level_audio_10`
    before `level_audio_2` and silently permute every head on reload.
*   **`strict=False` semantics.** *Unexpected* keys (modules removed since the
    checkpoint was written) are dropped silently by design; *missing* keys stay
    at random init — harmless only for modules a given lineage never trained.
    Both lists are printed at load; an unexpected key you cannot name is a red
    flag, not noise.
