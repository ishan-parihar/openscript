# TTS Engine Post-Mortem: Higgs Audio v3 + Gepard, and the IndexTTS-2.5 Evaluation

> Date: 2026-08-14. Decision (user): **audio8 (Qwen3-TTS-0.6B clone) is the canonical
> voice-clone engine.** Higgs Audio v3 and Gepard are removed from the active
> system architecture. This document records *why* (higgs post-mortem), what was
> tried, and the evaluation of IndexTTS-2.5 as the next clone-engine candidate.

---## 1. Decision summary (final, 2026-08-14)

| Engine | Verdict | Reason |
|---|---|---|
| **audio8** (Qwen3-TTS-12Hz-0.6B, ONNX int4 clone) | ✅ **CANONICAL** | Cleanest spectral profile in the bakeoff (86–89% energy below 4 kHz), natural pace, stable operating window, proven in production renders |
| **higgs** (Higgs Audio v3 4B, ONNX int4) | ⏸️ **ON HOLD** (kept, not canonical) | Chronic instability across every configuration, weaker clone fidelity than audio8, Boson **non-commercial** license. Code retained (sidecar, Rust module, config toggle) for later salvage if IndexTTS-2.5 underdelivers; demoted from the canonical clone path |
| **gepard** (Qwen3.5-AR + NeMo codec clone) | ❌ **PURGED** (Phase 174) | audio8 wins on the same reference; removed from code, config, docs, venv, and assets |
| **IndexTTS-2.5** (0.8B, PyTorch) | 🚧 **INTEGRATING** (Phase 175) | Strong CV3-Eval cloning scores; bilibili license is commercial-restricted (verify before business use); PyTorch-only — torch sidecar pattern |

Voice engines that remain in the stack: **audio8** (clone, canonical), **kokoro** (presets),
**voicedesign** (Qwen3 VoiceDesign — multi-character synthesis, no cloning), **higgs**
(on hold, kept), and the optional `sidecar` (faster-qwen3-tts) bridge.

---

## 2. Higgs Audio v3 — implementation post-mortem

### 2.1 What the model is

Higgs Audio v3 is a 4B autoregressive **conversational TTS** (Boson), exported by
`onnx-community` as a self-contained 6-sub-model ONNX pipeline
(`cuda_int4`, ~3.6 GB): text_embed → llm_decoder (Qwen3-4B backbone, int4 QDQ) →
audio_heads (fused 8-codebook heads, 1026 vocab) → audio_tokenizer (Higgs v2
codec, 24 kHz, 25 fps). Prompt format is a special-token sandwich with an
optional reference-code section for zero-shot cloning; generation uses a
**delay pattern** across the 8 codebooks (BOC=1024 pads, EOC=1025 terminates).
It also supports 43 inline control tags (emotion/style/sfx/prosody).

### 2.2 Timeline of failures and fixes (all landed, in order)

1. **Wrong runtime initially → CPU-bound (hours per generation).**
   The int4 `llm_decoder` is a plain ONNX QDQ graph, but the first cut ran the
   whole AR loop with naive ORT inference on CPU. Fixed by reading the graph's
   activation dtype from `inputs_embeds` (fp16 on CUDA), running the LLM on the
   `CUDAExecutionProvider`, and pinning only the conv `audio_encoder` to CPU
   (cuDNN unavailable on this machine's NVML 610.57 driver — `CUDNN_STATUS_INTERNAL_ERROR`).

2. **Incremental-decode bug → BFC OOM on long draws.**
   The first `_llm_step` re-fed the *entire accumulated sequence* every step
   alongside the past KV cache → every past position was re-attended AND the KV
   cache grew quadratically. Fixed to the canonical contract: prefill once
   (empty KV), then feed exactly ONE new code-embed row per step with
   `attn_len = running total`.

3. **Raw reference codes → hissy/metallic clone.**
   Feeding parallel (non-delay-patterned) reference frames is off-distribution.
   Fixed by applying `apply_delay_pattern` to the reference codes before fusing
   them into the prompt (mirrors the sglang-omni reference).

4. **EOC masked by top-k → the "evolve-evolve-..." ramble.**
   top_k=50 truncation was applied to codebook 0 too, masking the EOC
   termination token (probed: EOC rank 325–599, outside top-50, for 88% of AR
   steps). The model physically could not stop → it rambled on the final
   syllable (a 1.92 s "evolve" tail on a 6-word line) until EOC randomly
   re-entered the window. Fixed with **EOC-safe sampling** on codebook 0 (never
   mask the terminator) + a **masked-EOC force-stop** safety net.

5. **Sustained-tone loops on longer one-shot draws.**
   The AR model locks onto a cb0-repeat attractor (a held tone). The first
   retry logic only *cooled* temperature — which makes a tone loop MORE
   deterministic and MORE stuck. Stress tests showed all cooled retries looping.
   Fixed with **cause-aware retry temperatures**: tone → retry HOTTER (+0.15 up
   to 0.95) to break the attractor; ramble/silence/spectral → retry cooler.
   Validation: scene with 3 consecutive cooled failures recovered on the
   hotter ladder; one tone-loop retry at 0.95 drew a clean natural EOC.

6. **Spectral-noise draws (clean termination, hissy content).**
   A draw can terminate cleanly yet decode as broadband noise (zcps > 8000,
   < 40% energy below 4 kHz) — the model locked onto a hiss attractor whose
   vectors vary enough to dodge the identical-vector guard. Added a
   `spectral_noise_check` (zero-crossing rate + low-frequency energy ratio) to
   flag and retry these.

7. **Control-tag delivery.**
   Only the 43 recognized tags are valid; free-form tone text is *read aloud*
   (model degradation), so `instruct`/tone are deliberately NOT injected.
   Emote → tag mapping had gaps (grave/somber missing → no tag emitted on
   documentary scenes); aliases added (Phase 173). Tags are stripped from
   captions/timeline (control_tags.rs).

### 2.3 Why it still lost to audio8 (the bakeoff, 2026-08-14)

Same reference (`ref_ishan.wav`), same 2 lines, one-shot, no emotes:

| Engine/config | below-4k % | zcps | s/word | Read |
|---|---|---|---|---|
| **audio8** | **89.3 / 85.7** | 3550 / 4289 | 0.36 | cleanest, most "recorded" |
| gepard | 81.5 / 84.0 | 3523 / 3078 | 0.25–0.34 | natural |
| higgs t070 | 76.1 / 80.2 | 4967 / 3057 | 0.34 | ok |
| higgs t080 | 77.2 / 79.3 | 2914 / 3748 | 0.36 | ok |
| higgs t090 | **55.1 / 69.6** | **6174** / 3550 | 0.23 | bright/hissy |
| higgs t080+top_p | 70.5 / 81.5 | 3531 / 3210 | 0.33 | mixed |

Even after every sampler fix, the higgs export sits in a **narrow, unstable
operating window**: 0.7 one-shot rambles, 0.9 turns hissy, 0.8 is borderline;
long lines need retry ladders to survive. Clone fidelity (speaker similarity on
this int4 export) and long-line expressiveness consistently trail audio8's
Qwen3-0.6B on the same reference. Combined with the Boson **non-commercial
license**, the cost/benefit is negative for a production content stack.

### 2.4 What was salvageable vs not

- ✅ All *integration* bugs were fixable and fixed (runtime, KV loop, delay
  pattern, EOC masking, retry direction, spectral check, tag maps).
- ❌ The *fidelity ceiling* of this export — clone similarity + stable long-line
  one-shot delivery — is a model/export property, not an integration bug.
- ⚠️ Machine-specific: NVML driver mismatch forced the audio_encoder to CPU;
  on a clean driver the model would be faster, but not better-sounding.

---

## 3. IndexTTS-2.5 evaluation (IndexTeam)

### 3.1 What it is
0.8B-parameter three-stage cascade TTS (from the technical report / HF card):
1. **T2S** — AR semantic-token language model (structured conditioning + text).
2. **S2M** — flow-matching semantic→mel generator on a **Zipformer** backbone
   (replaces IndexTTS-2's U-DiT).
3. **Neural vocoder** — mel → waveform.
vs IndexTTS-2: semantic codec frame rate halved (50 → 25 Hz), Zipformer
backbone, cross-lingual training strategies (boundary-aware alignment,
token-level concatenation, instruction-guided generation), GRPO RL
post-training (ASR-WER + speaker-similarity preference), ~2.28× RTF improvement.

### 3.2 Voice cloning (the relevant axis)
- Zero-shot cloning from a **single short reference clip**; CV3-Eval scores:
  speaker similarity ≈ 68–77% (by language), WER ≈ 3.3–5.6% — at or above
  larger models (VoxCPM2 2B, FireRedTTS-2 1.5B).
- Fine-grained control: emotional reference audio, **8-dim emotion vectors**,
  text-to-emotion auto-conversion (`use_emo_text`), explicit `emo_text`,
  speaking-speed `duration_factor` — richer emotion control than higgs' tags.

### 3.3 Operational facts
- **Languages:** zh, en, ja, es, ar (5).
- **License:** **bilibili Model Use License Agreement** — commercial use requires
  contacting `indexspeech@bilibili.com`. ⚠️ Unlike audio8, this is NOT an open
  commercial license.
- **Hardware:** RTX 4090-class; BF16/FP16; RTF ≈ 0.20 (very fast).
- **Formats:** native PyTorch checkpoints only — **no official ONNX/quantized
  export** (community `audio.cpp` wrapper exists for CUDA/Vulkan/Metal/HIP).
  Integration would follow the gepard pattern: a torch venv + long-lived
  stdin/stdout sidecar (`mcp/scripts/indextts_sidecar.py` + `scripts/setup_indextts.sh`).

### 3.4 Fit for OpenScript
- **Pro:** strongest zero-shot cloning candidate we've researched; explicit
  emotion control channels that map cleanly onto our per-scene `emote`/`tone`
  schema; fast at 0.8B/25 Hz.
- **Con:** commercial license gate; PyTorch-only (needs a ~4 GB venv like
  gepard's); English is one of 5 supported languages (fine for our EN content).
- **Recommendation (SUPERSEDED — integrated, Phase 175):** do NOT integrate
  until (a) the license question is resolved for the target business use, and
  (b) audio8's current performance is demonstrably insufficient. The license
  gate was accepted for research-stage integration (bilibili — research /
  non-commercial; commercial use requires contacting indexspeech@bilibili.com),
  and IndexTTS-2.5 is now registered behind the config-driven backend toggle
  (`tts.backend: "indextts"`) as a sidecar clone engine.

### 3.5 IndexTTS-2.5 integration status (Phase 175, DONE)

Integrated as a long-lived stdin/stdout sidecar (`mcp/scripts/indextts_tts_sidecar.py`
+ `scripts/setup_indextts.sh`, `.venv-indextts` torch 2.8 + CUDA, ~5.7 GB
checkpoints in `mcp/assets/indextts` — gitignored). Verified end-to-end on the
RTX 2060 (8 GB): first synth 99.5 s (cold load), warm synth 37 s, 22.05 kHz
mono, per-emote `emo_text` guidance applied, loudness-normalized.

**8 GB-GPU fixes baked into the vendored copy** (idempotent
`scripts/patch_indextts_vendored.py`, applied by setup):
- `QwenEmotion` (float16 Qwen 0.6B, ~1.5 GB) moved from `device_map="auto"`
  (GPU) to `device_map="cpu"` — it is a tiny per-line text classifier; the
  GPU stays free for the audio pipeline. Without this the process peaked at
  6.36 GiB and OOM'd at inference on the 2060.
- `PYTORCH_CUDA_ALLOC_CONF=expandable_segments:True` (fragmentation guard).
- `use_cuda_kernel=False` (BigVGAN fused kernel targets newer archs than
  Turing sm_75; plain torch fallback is fine).

---

## 4. Removal scope — GEPARD (DONE, Phase 174)

gepard was purged across the whole stack (28 files touched):
- **Rust:** deleted `crates/openscript-tts/src/gepard.rs` + its `lib.rs` registration; removed the `gepard` backend from the tool enums/schemas in `tools.rs`, `tools_script.rs`, `tools_audio.rs`, `tools_system.rs`, `tools_character.rs`, `config.rs` (feature toggle), and `script.rs` (valid backends); tests updated.
- **Python/scripts:** deleted `mcp/scripts/gepard_tts_sidecar.py`, `scripts/setup_gepard.sh`, the 9 GB `.venv-gepard`, and `third_party/gepard-inference`.
- **Assets:** removed the gepard clone refs (`mcp/assets/gepard/voices/ishan{,_gepard}.wav`) and the `ishan_gepard` profile. **Kept** `air_analyst*.wav` + `hero_sidekick.wav` — they are voicedesign character reference audios stored in the legacy dir (path unchanged so existing character profiles keep working).
- **Docs/config:** AGENT_GUIDE, INSTALL, env example, setup.sh provisioning.
- **Higgs: UNTOUCHED** (on hold per decision).

Reversibility: everything is in git history (`git revert` restores), and model
assets stay on disk — the purge is code-level, not knowledge-level.
