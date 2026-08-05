# AUDIT REPORT — Caption Sync, B-roll Repetition, LLM Backend Refactor

**Date:** 2026-08-06
**Status:** Audit complete — refactor plan ready for review (not yet implemented)
**Source data:** `output/e2e_a2v_phase140.mp4` (135.7s) + `artifacts/e2e_a2v_phase140.timeline.json` (43 segments) from the 2026-08-06 full A2V E2E run.

---

## 1. Finding A — Captions are out of sync with the voice (word-level)

### 1.1 What the data says

The **phrase-level** timing is exact:

| Source | Value |
|--------|-------|
| SRT (`audit_v3_render.hinglish-ggml.srt`) | 43 cues, 0.000s → **135.400s** |
| Source audio duration | **135.40s** |
| Timeline segments (43) | 0.0 → **135.4s**, boundaries == SRT cue boundaries (verified seg_011…seg_016) |
| ASS events | 499 (43 phrases × ~11 words) |

So captions as a whole ARE placed on the right phrases. **The desync is intra-phrase**: within each ~3s phrase, the per-word green highlight runs on **even-spaced fake timings**.

### 1.2 Root cause

1. The A2V path feeds `captions.generate_ass` the **phrase-level** SRT (`broll.auto` Stage F → `srt_path`).
2. The SRT parser treats each phrase cue as a single synthetic "word" → `normalize_word_timings()` (`crates/openscript-core/src/captions.rs`) splits it with `estimate_word_timings()`: `per_word = total_duration / word_count` — **equal slices**.
3. Natural speech is not uniform: "sarkaar" takes ~2× "bhai". The highlight drifts hundreds of ms per word and accumulates within each phrase — the visible "captions not synced with the voice".

### 1.3 Why the pipeline already has the fix — it just isn't threaded

The transcription engine (`transcribe`) already returns a **word-level SRT** (`word_srt_path`). `captions.generate_ass` is *designed* for word-level input ("Generate ASS subtitle file from word-level SRT with per-word timing") and `group_entries_with_words()` (`crates/openscript-core/src/srt/mod.rs:104`) groups word entries into phrases **preserving the real timestamps**. The A2V path simply passes the phrase SRT instead of the word SRT, so real timings never reach the ASS.

### 1.4 Refactor plan — A2V captions from real word timings

| # | Step | File(s) | Notes |
|---|------|---------|-------|
| A1 | Add optional `word_srt_path` to `captions.generate_ass` schema + handler. When set, parse it, `group_entries_with_words` (max_words/max_chars from spec), and generate from those segments — real per-word timings. Phrase SRT remains the fallback path. | `crates/openscript-mcp/src/tools.rs` | Backward compatible (`#[serde(default)]`-style optional arg). |
| A2 | `broll.auto` Stage F: accept `word_srt_path` arg and pass it through to `captions.generate_ass`. Production A2V flow becomes `transcribe` → `broll.auto(phrase_srt + word_srt_path)` → captions carry real word timing. | `crates/openscript-mcp/src/tools.rs` | Transcribe already returns both paths. |
| A3 | Improve the *fallback* estimator: `estimate_word_timings` splits evenly; change to **char-length-proportional** slices (words with more letters get proportionally more time). Removes most drift even when only a phrase SRT exists (e.g. pre-made SRTs like the test one). | `crates/openscript-core/src/captions.rs` | Pure change; unit-testable. |
| A4 | Tests: (a) word-SRT → ASS preserves given word timings verbatim (no normalization rewrite); (b) char-proportional estimator distributes by length; (c) A2V E2E regression — ASS word timings are NOT uniform. | `crates/openscript-core/src/captions.rs`, `crates/openscript-mcp/tests/integration_test.rs` | Golden-string assertions. |

**Definition of done:** regenerate `captions.ass` from the word SRT for the audit audio; extract 3 frames inside one phrase; the highlighted word changes at real syllable boundaries, not clock ticks.

---

## 2. Finding B — B-roll clips repeat later in the sequence

### 2.1 What the data says

- 42 unique clip paths across 43 segments.
- **1 confirmed duplicate:** `crowd_people_nahin_35340082.mp4` → events `broll_29` **and** `broll_37` (positions 29 and 37 — 8 segments apart, exactly the "same video repeats later" the user saw).

### 2.2 Root cause

- `used_broll_video_ids(timeline)` **exists** (`tools.rs:7962`) but is only consumed by the **LLM paths**: `broll.validate_keywords` (avoid set) and `broll.repair`. 
- `handle_broll_fetch` (`tools.rs:4133`) — the deterministic path — searches Pexels per concept and takes the first covering result **with no exclusion of already-used video ids**.
- With the LLM down (current env), `broll.keywords` uses the deterministic fallback (`translate_hinglish_visuals` + `extract_broll_concept`), which produced the same concept ("crowd …") for two segments → same first Pexels result → same clip placed twice. Non-redundancy is a prompt-level guarantee in the LLM path and absent entirely in the deterministic path.

### 2.3 Refactor plan — non-repeating b-roll

| # | Step | File(s) | Notes |
|---|------|---------|-------|
| B1 | Extract a pure picker `pick_broll_candidate(candidates, used_ids) -> Option<PexelsVideo>` that returns the first candidate **not** in `used_ids` (falling back to the next result when the top hit is used; `None` only when every candidate is used). | `crates/openscript-mcp/src/tools.rs` | Unit-testable. |
| B2 | In `handle_broll_fetch`, load `used_broll_video_ids(&timeline)` once at the start; maintain a **run-local used set** (ids chosen earlier in the same fetch pass); call `pick_broll_candidate` instead of taking the first result. | `crates/openscript-mcp/src/tools.rs` | Covers both cross-run repeats (the bug) and within-run repeats. |
| B3 | `handle_broll_assign` — apply the same used-set exclusion if it also picks candidates (mirror B2). | `crates/openscript-mcp/src/tools.rs` | Grep shows assign is thin; confirm during implementation. |
| B4 | Defensive: add a `BROLL_REPEAT` check to `timeline.validate`/`verify.production` — same clip path on 2+ events → error listing the events. Gives the agent an explicit signal if a repeat ever slips through. | `crates/openscript-mcp/src/tools.rs` | Mirrors the BROLL_GAP validator. |
| B5 | Tests: unit test for `pick_broll_candidate` (used id skipped, all-used → None); integration assertion that a fresh `broll.fetch` run on the audit timeline yields 43 unique paths. | `tools.rs` tests mod + integration_test.rs | |

**Definition of done:** re-run the A2V E2E with the LLM down; assert `len(set(clip_paths)) == 43`.

---

## 3. Finding C — Redundant Ollama backend

### 3.1 Current cascade (`crates/openscript-mcp/src/llm.rs`)

`backend_force = "auto"` order today: **opencode → local Ollama → openrouter** (opencode is already first, but local Ollama sits between it and the fallback).

Current env reality: local Ollama is broken/irrelevant (`qwen3.5-4b` 404, `deepseek-v4-flash:cloud` 401) and OpenRouter free tier is rate-limited — so the cascade burns 2 failing probes per call. The user wants exactly **opencode zen → openrouter fallback**, no Ollama.

opencode is already configured as zen: `opencode_base_url = https://opencode.ai/zen/v1`, `opencode_model = default_opencode_model()` (config + `OPENCODE_MODEL`/`OPENCODE_API` env overrides).

### 3.2 Refactor plan — remove Ollama, keep opencode zen → openrouter

| # | Step | File(s) | Notes |
|---|------|---------|-------|
| C1 | Delete `run_local` + the `local_up` probe + `local_vision` handling in `chat_complete_with_backend`. `"auto"` becomes: opencode → (if `prefer_openrouter_vision` and image) openrouter → openrouter. The `"local"` force arm is removed (or aliased to opencode with a deprecation warning). | `crates/openscript-mcp/src/llm.rs` | Single function to trim. |
| C2 | Drop `LlmCascade` local fields (`local_model`, `local_base_url`, `gguf_path`, `mmproj_path`) + `resolve_gguf_path`/`resolve_mmproj_path` re-exports. | `crates/openscript-mcp/src/llm.rs` | |
| C3 | `config.rs`: remove `llm.local_model/local_base_url/gguf_path/mmproj_path/local_vision` fields + their resolvers + env lookups (`OPENSCRIPT_LOCAL_MODEL`, `OPENSCRIPT_LLM_URL`, `OPENSCRIPT_LOCAL_VISION`, `OPENSCRIPT_GGUF_PATH`, `OPENSCRIPT_MMPROJ_PATH`) and remove them from `config_public_view()`. | `crates/openscript-mcp/src/config.rs` | Serde defaults removed; old config files still parse (unknown keys ignored). |
| C4 | `probe_llm_capabilities()`: report `opencode` (zen: key set? base_url, model) + `openrouter` only. | `crates/openscript-mcp/src/llm.rs` | `system.capabilities` reflects the new truth. |
| C5 | `server.rs` instructions + `system.capabilities` text: drop Ollama/Ollama-import mentions. | `crates/openscript-mcp/src/server.rs` | |
| C6 | Docs/scripts: AGENTS.md §16 (Ollama "getting unstuck" + `import_local_gguf.sh`), `llm.rs` module doc comment, `scripts/import_local_gguf.sh` (mark deprecated or delete), `setup.sh` references. `docs/INSTALL.md` LLM section. | `AGENTS.md`, `llm.rs`, `scripts/`, `docs/INSTALL.md` | |
| C7 | Tests: `llm` cascade — "auto" with only OpenRouter key → openrouter backend; "auto" with opencode key → opencode first; `backend_force="local"` → clean error. Update any test asserting local behavior. | `crates/openscript-mcp/src/llm.rs` tests / integration | Mock at the HTTP boundary (`openai_chat` behind a trait or injected base-url). |

**Runtime config for the user:**
```json
// ~/.openscript/config.json
{ "api_keys": { "opencode": "<OPENCODE_ZEN_KEY>", "openrouter": "<OR_KEY>" },
  "llm": { "opencode_base_url": "https://opencode.ai/zen/v1", "opencode_model": "<zen-model>" } }
// or env: OPENCODE_API / OPENCODE_MODEL
```

**Definition of done:** `llm.complete` succeeds via opencode zen with zero probes to 127.0.0.1:11434; `system.capabilities` shows only opencode + openrouter.

---

## 4. Verification matrix (after implementation)

1. `cargo build --workspace --exclude openscript-tauri` — zero warnings.
2. `cargo test --workspace --exclude openscript-tauri --lib --bins --tests` — baseline 357 + new (A4, B5, C7).
3. `python3 scripts/workspace-lint/workspace_lint.py --root .` — 0 errors.
4. `bash scripts/smoke_test_mcp.sh` — 97 tools, green.
5. E2E re-run (LLM down): 43/43 unique b-roll clips; captions generated from word SRT with non-uniform word timings; `timeline.validate` reports no BROLL_REPEAT.

## 5. Suggested execution order

1. **B1–B5 (b-roll non-repeat)** — smallest, self-contained, directly fixes a user-visible defect; unblocks re-running the E2E audit cleanly.
2. **A1–A4 (caption sync)** — needs the word SRT in the A2V flow; verify with the audit audio.
3. **C1–C7 (LLM refactor)** — independent; do last so E2E reruns can still use the deterministic fallbacks while A/B land.

## 6. Non-goals

- No re-architecting of the caption engine (PupCaps/HTML overlays stay an escape hatch).
- No Pexels API changes; used-id exclusion is purely client-side bookkeeping.
- Ollama removal does NOT touch the Python ML sidecars (Kokoro/Whisper/Apex) — those are TTS/STT, not the LLM director cascade.
