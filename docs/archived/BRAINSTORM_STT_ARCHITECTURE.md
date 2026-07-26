# OpenScript — Ideal STT Pipeline Architecture (Refined)

**Date:** 2026-07-20  
**Context:** Use Nemotron 3.5 ASR as single unified model + LLM post-processing for Hinglish. Deprecate Trelis.

---

## Critical Findings

| Finding | Implication |
|---------|-------------|
| **Nemotron 3.5 ASR is RNNT (not CTC)** | NeMo Forced Aligner (NFA) requires CTC models — **does not work directly** with Nemotron |
| **Nemotron tokenizer: ~13k BPE tokens, 40 locales** | Hindi output is **Devanagari script** (native) |
| **Hindi WER: 6.81% (LangID) / 8.23% (auto)** | **Better than Trelis's 12.57% Hinglish WER** |
| **Sherpa-onnx has ONNX export** | CPU/GPU inference without NeMo dependency |
| **WhisperX still best for word alignment** | Works with any ASR output via forced alignment |

---

## Refined Architecture: Single Model + LLM Post-Processing

### Core Philosophy

```
Nemotron 3.5 ASR (single model, all languages)
    │
    ├── English, Spanish, French... → Direct output (Latin script, punctuated)
    │
    └── Hindi → Devanagari output
                │
                ▼
         LLM Post-Processor
                │
                ├── Devanagari → Latin transliteration
                ├── Hindi → Hinglish code-switch conversion
                └── Punctuation/capitalization normalization
```

### Why This Wins

| Approach | Models | Hindi WER | Hinglish Quality | Complexity |
|----------|--------|-----------|------------------|------------|
| Trelis + Nemotron | 2 | N/A | 12.57% | High (2 models, 2 pipelines) |
| **Nemotron + LLM** | **1** | **6.81%** | **LLM-controlled** | **Low (1 model + prompt)** |

**Trelis is redundant** — Nemotron's Hindi quality is superior, and an LLM can convert Devanagari → Hinglish better than a model trained on limited Hinglish data.

---

## Pipeline Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│ transcribe() — Unified Entry Point                              │
│ Input: media_path, output_srt_path, language_hint (optional)   │
└────────────────────────────┬────────────────────────────────────┘
                             │
                             ▼
┌─────────────────────────────────────────────────────────────────┐
│ STEP 1: Audio Preprocessing                                     │
│ ffmpeg → 16kHz mono WAV (existing apex_transcriber.py logic)   │
└────────────────────────────┬────────────────────────────────────┘
                             │
                             ▼
┌─────────────────────────────────────────────────────────────────┐
│ STEP 2: Nemotron 3.5 ASR Inference                              │
│                                                                  │
│ IF language_hint provided:                                      │
│   target_lang = language_hint (e.g., "hi-IN", "en-US")         │
│ ELSE:                                                           │
│   target_lang = "auto" (auto-detect + emit <xx-XX> tag)        │
│                                                                  │
│ Backend options (pick one):                                     │
│   A. sherpa-onnx (ONNX, no NeMo dep, CPU/GPU)                  │
│   B. NeMo Python (full features, streaming)                    │
│   C. Transformers pipeline (easiest integration)               │
└────────────────────────────┬────────────────────────────────────┘
                             │
              ┌──────────────┴──────────────┐
              │                             │
              ▼                             ▼
    ┌─────────────────────┐       ┌─────────────────────┐
    │ Latin-script langs  │       │ Devanagari-script   │
    │ (en, es, fr, de...) │       │ (hi-IN, etc.)       │
    └──────────┬──────────┘       └──────────┬──────────┘
               │                             │
               ▼                             ▼
    ┌─────────────────────┐       ┌─────────────────────┐
    │ Direct output       │       │ LLM Post-Processor  │
    │ (punctuated,        │       │                     │
    │  capitalized)       │       │ 1. Devanagari→Latin │
    └──────────┬──────────┘       │ 2. Hindi→Hinglish   │
               │                  │ 3. Normalize        │
               │                  └──────────┬──────────┘
               │                             │
               └──────────────┬──────────────┘
                              │
                              ▼
    ┌─────────────────────────────────────────────────────────────┐
    │ STEP 3: Word-Level Alignment (WhisperX)                     │
    │                                                              │
    │ Input: audio.wav + transcript (Latin script)               │
    │ Output: word-level timestamps (JSON)                        │
    │                                                              │
    │ Why WhisperX: works with ANY transcript, robust,            │
    │              supports Hindi (via whisper-large-v3)          │
    └────────────────────────────┬────────────────────────────────┘
                                 │
                                 ▼
    ┌─────────────────────────────────────────────────────────────┐
    │ STEP 4: SRT Generation                                      │
    │ word-level.srt + phrase-level.srt                           │
    │ + Hinglish validation (existing)                            │
    └─────────────────────────────────────────────────────────────┘
```

---

## LLM Post-Processor Design

### Prompt Template

```python
HINGLISH_PROMPT = """Convert the following Devanagari Hindi transcript to natural Hinglish (Romanized Hindi with English code-switching).

Rules:
1. Transliterate Devanagari to Latin script (e.g., "मैं" → "main", "है" → "hai")
2. Preserve English words as-is (e.g., "मैं engineer हूँ" → "main engineer hoon")
3. Use natural Hinglish conventions:
   - "और" → "aur" (not "and")
   - "लेकिन" → "lekin" 
   - "क्योंकि" → "kyunki"
   - "तो" → "toh"
   - "भी" → "bhi"
4. Add proper punctuation and capitalization
5. Keep sentence boundaries from the input
6. Output ONLY the converted text, no explanations

Input (Devanagari):
{devanagari_text}

Output (Hinglish):"""
```

### Implementation Options

| Option | Pros | Cons |
|--------|------|------|
| **Local LLM (llama.cpp)** | Private, no API cost, fast on GPU | Needs model download (~4GB) |
| **OpenAI/Gemini API** | Best quality, zero setup | Latency, cost, privacy |
| **Small fine-tuned model** | Fast, specialized | Training effort |
| **Rule-based + dictionary** | Fast, deterministic | Misses context, brittle |

**Recommendation:** Start with **llama.cpp + small instruct model** (e.g., `gemma-2-2b-it` or `llama-3.2-1b-instruct`) for local, fast, private processing. Fallback to API for quality-critical runs.

---

## Word Alignment: WhisperX (Not NFA)

### Why Not NFA?

- NFA requires **CTC model** (e.g., Conformer-CTC, Canary encoder)
- Nemotron 3.5 ASR is **RNNT (Transducer)** — no CTC head
- NFA's auxiliary CTC model doesn't match Nemotron's tokenizer

### WhisperX Approach

```python
# whisperx_alignment.py
import whisperx

# 1. Load alignment model (multilingual)
model_a, metadata = whisperx.load_align_model(
    language_code="hi",  # or detected language
    device="cuda"
)

# 2. Align
result = whisperx.align(
    transcript_segments,  # from Nemotron + LLM
    model_a, metadata,
    audio_path,
    device="cuda"
)

# 3. Output: word-level timestamps
# result["word_segments"] = [{"word": "main", "start": 0.5, "end": 0.8, "score": 0.99}, ...]
```

**Advantages:**
- Works with ANY transcript (Nemotron, Trelis, Whisper, human)
- Robust to slight transcript mismatches
- GPU accelerated
- Outputs word + segment + phrase level

---

## Backend Selection: sherpa-onnx (Recommended)

### Why sherpa-onnx?

| Factor | sherpa-onnx | NeMo Python | Transformers |
|--------|-------------|-------------|--------------|
| Dependencies | Minimal (ONNX Runtime) | Heavy (NeMo, PyTorch) | Medium (PyTorch) |
| Streaming | ✅ Native | ✅ Native | ⚠️ Limited |
| ONNX Export | ✅ Pre-built | Manual | Manual |
| CPU Inference | ✅ Fast (parakeet.cpp) | Slow | Slow |
| Language Auto-Detect | ✅ `prompt_index=101` | ✅ | ✅ |
| License | Apache 2.0 | Apache 2.0 | Apache 2.0 |

### Integration

```bash
# Pre-built ONNX model available:
# https://huggingface.co/pantinor/nemotron-3.5-asr-streaming-0.6b-onnx
```

```python
# sherpa_onnx_nemotron.py
import sherpa_onnx

recognizer = sherpa_onnx.OfflineRecognizer.from_nemotron(
    model="nemotron-3.5-asr-streaming-0.6b.onnx",
    tokens="tokens.txt",
    num_threads=4,
    sample_rate=16000,
    feature_dim=80,
    decoding_method="greedy_search",
)

# Auto-detect language
stream = recognizer.create_stream()
stream.accept_waveform(16000, audio_samples)
recognizer.decode_stream(stream)

# Result includes language tag: "main engineer hoon <hi-IN>"
result = stream.result
```

---

## Deprecation Plan

| Component | Status | Replacement |
|-----------|--------|-------------|
| `mcp/assets/parakeet/` ONNX models | **Deprecated** | sherpa-onnx Nemotron ONNX |
| `mcp/scripts/apex_transcriber.py` | **Deprecated** | `nemotron_transcriber.py` + `llm_postprocessor.py` |
| `TranscriptionEngine::Apex` | **Deprecated** | `TranscriptionEngine::Nemotron` |
| `find_conda_python()` (whisper-hindi) | **Deprecated** | System Python + pip packages |
| Trelis integration | **Cancelled** | LLM post-processor |
| `whisper_timestamped` dependency | **Deprecated** | WhisperX for alignment only |

---

## Implementation Phases

### Phase 1: Nemotron Integration (Week 1)
- [ ] Add `sherpa-onnx` to Python deps
- [ ] Download Nemotron ONNX model + tokens
- [ ] Create `mcp/scripts/nemotron_transcriber.py`
- [ ] Wire into `transcriber.rs` as new engine
- [ ] Test Hindi → Devanagari output

### Phase 2: LLM Post-Processor (Week 1-2)
- [ ] Add `llama.cpp` + small instruct model (gemma-2-2b-it)
- [ ] Create `mcp/scripts/llm_postprocessor.py`
- [ ] Implement Devanagari→Latin + Hindi→Hinglish prompt
- [ ] Test on real Hinglish audio

### Phase 3: WhisperX Alignment (Week 2)
- [ ] Add `whisperx` to Python deps
- [ ] Create `mcp/scripts/whisperx_align.py`
- [ ] Integrate: Nemotron transcript → WhisperX align → SRT
- [ ] Validate frame-accurate timestamps

### Phase 4: Unified Rust API (Week 2-3)
- [ ] Update `TranscriptionEngine` enum: `Nemotron` variant
- [ ] Add `language_hint` parameter to `transcribe()`
- [ ] Deprecate `Apex` variant (keep for compat, mark deprecated)
- [ ] Update `system.capabilities` to report Nemotron status

### Phase 5: Cleanup (Week 3)
- [ ] Remove `mcp/assets/parakeet/` (or archive)
- [ ] Remove `apex_transcriber.py`
- [ ] Remove conda env dependency (`whisper-hindi`)
- [ ] Update docs, AGENTS.md, AGENT_GUIDE.md

---

## Language Hint Mapping

```rust
pub enum LanguageHint {
    Auto,           // Nemotron auto-detect
    Hinglish,       // Hindi + English code-switch → Nemotron hi-IN + LLM
    Hindi,          // Pure Hindi → Nemotron hi-IN + LLM (transliterate only)
    English,        // Nemotron en-US
    Spanish,        // Nemotron es-ES
    // ... other 40 locales
}
```

**Default:** `Auto` — Nemotron detects language, emits `<hi-IN>` tag, pipeline routes to LLM if Hindi detected.

---

## Quality Gates

| Metric | Target | Measurement |
|--------|--------|-------------|
| Hindi WER | ≤ 8% | FLEURS hi-IN test set |
| Hinglish naturalness | ≥ 4/5 | Human eval on 50 samples |
| Word timestamp accuracy | ≤ 200ms median error | vs. manual alignment |
| End-to-end latency (30s audio) | ≤ 10s | CPU/GPU benchmark |
| No regression on English | WER ≤ 8% | FLEURS en-US test set |

---

## Open Questions

1. **LLM model size vs quality:** `gemma-2-2b-it` (2B) vs `llama-3.2-1b-instruct` (1B) vs `qwen2.5-1.5b-instruct` — which balances speed/quality for transliteration?

2. **Streaming vs batch:** Nemotron supports streaming (80ms chunks). Do we need streaming for OpenScript? (Current: batch only)

3. **Sherpa-onnx vs parakeet.cpp:** `parakeet.cpp` is C++ inference for Parakeet TDT. Nemotron ONNX via sherpa-onnx is the path. Any native C++ Nemotron runner?

4. **WhisperX Hindi alignment quality:** WhisperX uses Whisper large-v3 for alignment. Does it align well to Nemotron's Devanagari→Latin output? May need fine-tuned alignment model.

5. **Caching:** Nemotron model is 600M params (~1.2GB). Cache in memory across calls? Or reload per transcription?

---

## Summary

**Single model (Nemotron 3.5 ASR) + LLM post-processing + WhisperX alignment** replaces the entire Apex/Trelis/Parakeet stack.

| Before | After |
|--------|-------|
| 3 models (Apex, Trelis, Parakeet) | 1 model (Nemotron) |
| Conda env + ONNX + Python sidecars | sherpa-onnx + llama.cpp + WhisperX |
| 29.79% Hindi WER (Apex) | **6.81% Hindi WER (Nemotron)** |
| No Hinglish (Apex outputs Devanagari) | **LLM-controlled Hinglish** |
| Broken ONNX decoder | **Working ONNX (sherpa-onnx)** |
| Estimated timestamps | **Frame-accurate (WhisperX)** |

This is the architecture to build.