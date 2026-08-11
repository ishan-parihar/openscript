# OpenScript Comprehensive Implementation Plan

**Date:** 2026-07-06
**Scope:** Whisper→Parakeet migration, Python sidecar consolidation, video quality fixes

---

## Part 1: Whisper → Parakeet TDT Migration

### Current State
- Force-alignment uses `whisper_align.py` which spawns `python3 -c "import whisper"` per scene
- 20-scene script pays 20× cold-start (~200ms each = 4s overhead)
- Whisper Python module is NOT installed in this environment → all scenes fall back to estimated timings
- Parakeet TDT model (640MB ONNX) is downloaded at `mcp/assets/parakeet/` but unused
- Two stub files (`model.onnx`, `tokenizer.json`) are 15-byte "Entry not found" placeholders

### What Stays
- **Apex transcription** (`apex_transcriber.py` + `openscript-transcribe` crate) — this is a DIFFERENT use case (Hinglish speech-to-text for NLE editing). It uses `whisper_timestamped` with a Hinglish-fine-tuned model. No Rust equivalent exists. KEEP AS PYTHON.

### Migration Plan

**Step 1: Fix Parakeet model stubs**
- Re-pull `model.onnx` + `tokenizer.json` from HuggingFace, OR confirm they're not needed (encoder-model.int8.onnx + decoder_joint-model.int8.onnx + vocab.txt are the real files)

**Step 2: Port force-alignment to Rust using `ort` crate**
- Add `ort = "2.0"` dependency to `openscript-tts` or a new `openscript-align` crate
- New module: `crates/openscript-tts/src/parakeet_align.rs`
  - Load encoder + decoder_joint ONNX models once (process-global, like Kokoro sidecar)
  - Read WAV → mel spectrogram → encoder → decoder → RNN-T beam search → word timestamps
  - Output: `Vec<WordTiming> { word, start_ms, end_ms }` (same contract as current)
- Replace `run_whisper_alignment()` in tools.rs to call the Rust module instead of spawning Python

**Step 3: Delete `whisper_align.py`**

**Step 4: Update all references**
- `system.capabilities` probe: check for parakeet ONNX files instead of `import whisper`
- Tool descriptions, server instructions, AGENT_GUIDE.md, AGENTS.md — replace "whisper force-alignment" → "Parakeet force-alignment"
- Rename capability key `whisper_align` → `parakeet_align`

---

## Part 2: Python Sidecar Consolidation

### Current State (6 Python scripts, 1,125 LoC)

| Script | LoC | Category | Recommendation |
|--------|-----|----------|----------------|
| `kokoro_tts_sidecar.py` | 183 | ML (ONNX) | **Port to Rust** (`ort` crate) — model is already ONNX |
| `whisper_align.py` | 74 | ML (Whisper) | **Delete** — replaced by Parakeet in Rust |
| `apex_transcriber.py` | 288 | ML (Hinglish) | **Keep** — no Rust equivalent for Hinglish model |
| `workspace_lint.py` | 416 | Convenience | **Port to Rust** (low priority, dev-only) |
| `test_mcp_e2e.py` | 91 | Dead code | **Delete** — duplicated by Rust integration tests |
| `test_mcp_pexels.py` | 73 | Dead code | **Delete** — no caller, hardcoded dev paths |

### Fragmentation Cost
- 5 language stacks: Rust/Cargo, Python/pip, TypeScript/npm, Node/npx, Shell
- No `requirements.txt` or `pyproject.toml` — Python deps scattered across `setup.sh` + `AGENTS.md` prose
- ~570 LoC of Rust glue code exists solely to talk to Python sidecars (`kokoro_sidecar.rs` = 371 LoC alone)

### Consolidation Plan

**Phase AX: Delete dead code**
- Delete `test_mcp_e2e.py` + `test_mcp_pexels.py` (164 LoC removed)
- No functional change — these have zero callers

**Phase AY: Port Kokoro TTS to Rust**
- Add `ort = "2.0"` to `openscript-tts/Cargo.toml`
- New module: `crates/openscript-tts/src/kokoro_native.rs`
  - Load ONNX model + voices.bin via `ort::Session`
  - Implement text → phonemes (using `phonemizers` crate or a Rust port of Kokoro's phonemizer)
  - Run ONNX inference → f32 PCM samples
  - Encode to WAV via `hound`
- Replace `KokoroEngine::synth_one()` to call native Rust instead of Python sidecar
- Delete `kokoro_sidecar.rs` (371 LoC) + `kokoro_tts_sidecar.py` (183 LoC)
- Net: -554 LoC, eliminates `kokoro-onnx` + `numpy` Python deps

**Phase AZ: Replace whisper alignment with Parakeet (see Part 1)**
- Delete `whisper_align.py` (74 LoC)

**Future: Port workspace_lint.py to Rust**
- Low priority — dev-only, not in runtime path
- Would eliminate last Python dep from CI gate

### After Consolidation
- Python scripts: 6 → 1 (`apex_transcriber.py` only)
- Python LoC: 1,125 → 288 (74% reduction)
- Rust glue removed: ~570 → ~200 LoC (only Apex wrapper remains)
- Add `requirements.txt` for Apex deps so the conda env is reproducible

---

## Part 3: Video Quality Fixes

### Problem 1: Caption words don't follow speaker's voice (P0)

**Root cause:** `theme:"calm"` overrides `word_highlight` → `sentence_fade` in `apply_theme()`. The agent followed AGENT_GUIDE.md which tells agents to use `theme:"calm"` for healing content.

**Fix:** Remove the caption style override from `theme:"calm"`. Keep the warm-gold/cream colors (aesthetic) but keep `word_highlight` as the style (animation). Word-level sync is the expected default for all content types.

**Files to change:**
- `crates/openscript-core/src/script.rs:807-809` — delete the `sentence_fade` override
- `crates/openscript-core/src/script.rs:1159` — update test assertion
- `AGENT_GUIDE.md:184,208-212` — update theme table + caption cheat-sheet

### Problem 2: Procedural background instead of live Pexels footage (P1)

**Root cause:** The Pexels multi-broll gate in `tools.rs:7849` only fires for `type:"gameplay"`. When the agent sets `type:"procedural"` (as AGENT_GUIDE.md suggests for healing), Pexels is completely skipped.

**Fix:** Loosen the gate so Pexels fires for any non-static type. `type:"procedural"` will now try Pexels first, falling back to procedural clips on failure. `type:"static"` remains the explicit opt-out.

**Files to change:**
- `crates/openscript-mcp/src/tools.rs:7849` — `== "gameplay"` → `!= "static"`
- `AGENT_GUIDE.md:189-199,216-232` — rewrite healing example + background section

### Problem 3a: No stickers (P2)

**Root cause:** `theme:"calm"` disables stickers entirely (`script.rs:810-823`). This kills both the SVG puppet path AND the GIPHY sticker path.

**Fix:** Remove the sticker-disabling block from `theme:"calm"`. Stickers stay on by default. Agents who want zero stickers can set `stickers.enabled: false` explicitly.

### Problem 3b: No music (P3)

**Root cause:** `spec.music` is `Option<MusicSpec>` and defaults to `None`. No auto-selection from the 20-track music catalog.

**Fix:** Add `auto_select_music()` that reads `music_index.json` and picks a track by theme:
- `calm` → `relaxing_meditation.mp3`
- `energetic` → `upbeat_pop.mp3` or `electronic_dance.mp3`
- `neutral` → `lofi_chill.mp3`

When `spec.music` is `None`, auto-select based on `output.theme`.

### Problem 3c: No SFX (P4 — higher effort)

**Root cause:** SFX is not wired into `script.to_video` at all. The `sfx.search`/`sfx.assign` tools exist but are only used in `reelize.direct`, not the golden trajectory.

**Fix:** Add SFX auto-assignment to `script.to_video`:
- Hook SFX on first scene start
- Transition SFX between scenes
- Wire SFX paths into `MultiLayerRenderSpec`

This requires adding a `sfx_paths` field to the render spec and FFmpeg mixing logic. Higher effort, can be deferred after P0-P3.

---

## Implementation Phases (Prioritized)

| Phase | Priority | Description | Effort |
|-------|----------|-------------|--------|
| **AU** | P0 | Remove theme:calm caption override (word_highlight default) | Trivial |
| **AV** | P1 | Loosen Pexels gate (procedural also gets live footage) | Trivial |
| **AW** | P2+P3 | Remove sticker-disabling + add default music auto-selection | Medium |
| **AX** | — | Delete dead Python test scripts | Trivial |
| **AY** | — | Port Kokoro TTS to Rust (`ort` crate) | High |
| **AZ** | — | Replace whisper with Parakeet TDT in Rust | High |
| **BA** | P4 | Add SFX to script.to_video | High (deferred) |

Each phase: commit + push after build + test + tsc + lint pass.
