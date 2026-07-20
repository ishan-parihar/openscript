# OpenScript Fresh-Agent Simulation Audit — Run #7

**Date:** 2026-07-20  
**Base Commit:** 0cc4d39 (Phase 1-5: validator audio & layer validation fixes)  
**Simulation Script:** `test_healing_script.json` (6 scenes, "calm" theme, breathwork/nervous system topic)  
**Generated Video:** `output/next_iter.mp4` (34.67s, 1080×1920, H.264/AAC)  
**Total Pipeline Time:** ~74 seconds

---

## Executive Summary

| Metric | Target | Actual | Status |
|--------|--------|--------|--------|
| Production Score (verify.production) | ≥ 95 (Grade A) | **54 (Grade D)** | ❌ FAIL |
| Parakeet Warnings | 0 | **6** | ❌ FAIL |
| SFX Track Events | ≥ 2 | **6 (but audio not verified)** | ⚠️ PARTIAL |
| Sticker Uniqueness (6 scenes) | ≥ 3 unique | **1 unique** | ❌ FAIL |
| Caption Sync (Parakeet) | Frame-accurate | **Estimated only** | ❌ FAIL |
| Title Cards (Hook/CTA) | ≥ 2 | **0** | ❌ FAIL |
| Audio Clipping | < -1 dBFS | **-0.3 dBFS** | ❌ HARD FAIL |
| Wall-Clock Time | ≤ 90s | **~74s** | ✅ PASS |

**Overall:** The validator fixes (Phases 1-5) are working — they correctly catch the issues. But the **core pipeline has 3 P0 blockers** that prevent Grade A output.

---

## Critical Findings (P0 — Must Fix)

### 1. Parakeet Force-Alignment Produces 0 Words
**Evidence:** 6 warnings: `"Parakeet returned no words, using estimated word timings"`

**Root Cause:** The Parakeet TDT ONNX decoder outputs token **8194 (`<dur:3>`)** repeatedly from step 0, never emitting content tokens. The vocab file was missing duration tokens (8193-8197) — fixed by adding them — but the model still produces only duration tokens.

**Possible Causes:**
- Decoder ONNX model (`decoder_joint-model.int8.onnx`) is corrupted/wrong version
- Model expects different initial state or input format
- Vocabulary mismatch despite fix (model trained with different tokenization)

**Impact:** Captions use estimated timings → visual sync drift. `verify.captions` coverage_ratio = 0.

**Fix Required:** Re-download Parakeet models from `istupakov/parakeet-tdt-0.6b-v3-onnx` on Hugging Face, or debug decoder graph.

---

### 2. Audio Clipping — HARD FAIL (Grade Cap D)
**Evidence:** `verify.production` peak_dbfs = **-0.3 dBFS** (threshold: -1 dBFS)

**Root Cause:** Combined voiceover + music + SFX exceeds headroom. The `loudnorm` filter in `render_multilayer` targets -16 LUFS but doesn't prevent instantaneous peaks from clipping.

**Impact:** Video will distort on YouTube/TikTok/Reels transcoding. `verify.production` caps grade at **D** regardless of other scores.

**Fix Required:** Add true-peak limiter (`alimiter=limit=0.95:attack=5:release=50`) after `loudnorm` in `FilterGraphBuilder::with_multilayer` or `render_multilayer`.

---

### 3. Script-Declared SFX Ignored by Pipeline
**Evidence:** Script declares:
```json
"sfx": [
  {"role": "intro", "trigger": "scene_change"},
  {"role": "highlight", "trigger": "scene_change"}
]
```
But `handle_script_to_video` **never reads `spec.sfx`**. It calls `auto_select_sfx_hits(scene_durations)` which picks SFX from `sfx_index.json` by `editorial_role` tags only.

**Impact:** User-authored SFX cues are silently dropped. The 6 SFX events in timeline are auto-selected, not script-driven.

**Fix Required:** In `handle_script_to_video`, parse `spec.sfx` and convert to `SfxHit` with timing based on `trigger: "scene_change"` → scene boundaries, `at_ms` → absolute time.

---

## High Priority (P1 — Should Fix)

### 4. Same Sticker Asset Repeated for All Scenes
**Evidence:** Timeline shows `giphy_narrator.gif` used for all 6 scenes. `verify.production` sticker_design: `"same sticker asset repeated for all speakers — weak identity"` (score 8/8 but with finding).

**Root Cause:** GIPHY search runs **once per speaker** (`speaker_stickers` map keyed by speaker name). Single speaker → one query → one GIF downloaded → reused.

**Fix Required:** In `handle_script_to_video`, query GIPHY **per scene** using `scene.emote` + `video_keywords` when single speaker. Rotate through top 3-4 results.

---

### 5. No On-Screen Title Cards
**Evidence:** `section_composition` score 7/8: `"no on-screen title/cards in any section"`, `visual_hierarchy`: `"no title cards — add title_text to hook/payoff sections"`.

**Root Cause:** `sections_from_timeline` (in `production_quality.rs`) doesn't generate `title_text` for Hook/Payoff/CTA sections.

**Fix Required:** Auto-generate title cards:
- Hook: first 3-4 words of scene 1
- Payoff: key phrase from middle scene
- CTA: last 3 words of final scene
Render as separate caption events with `subtitle_rail` style (larger, centered, top of safe zone).

---

### 6. Music Production Pack Missing
**Evidence:** Warning: `"Music path not found: mcp/assets/music_production/calm_1.mp3 — auto-select"`. Directory exists but is empty.

**Impact:** Falls back to library search (worked — selected calm lo-fi track), but production pack was designed as zero-dependency default.

**Fix Required:** Re-populate `mcp/assets/music_production/` with 4 beds (3 calm + 1 neutral, 22s each) and update `index.json`. Or remove pack references and make library auto-select the documented default.

---

### 7. Background Overlaps
**Evidence:** Timeline shows:
- Scene 1: 0.0s - 4.4s
- Scene 2: 4.0s - 7.8s  → **Overlap 4.0s-4.4s**
- Scene 3: 8.0s - 15.6s
- Scene 4: 15.0s - 21.7s → **Overlap 15.0s-15.6s**

**Root Cause:** `scene_durations` from manifest don't match background trim durations exactly. Background fetch uses `dur.to_string()` for `-t` but scene boundaries may have fractional ms drift.

**Fix Required:** Ensure background trim duration exactly matches voiceover segment duration per scene. Use `scene_durations[scene_idx]` for both voiceover and background.

---

## Medium Priority (P2 — Nice to Have)

### 8. Schema Discovery Requires Trial/Error
**Evidence:** Fresh agent had to read source code for:
- `speakers` HashMap required (not optional)
- `voice` format: `"kokoro:af_heart"` (not just `"af_heart"`)
- `emote` values: free string but affects sticker search
- `background.type` enum: `gameplay` | `procedural` | `static` (NOT `stock`)

**Fix Required:** Add `script.example` CLI command that outputs annotated minimal `ScriptSpec` JSON (see Phase 3 in plan).

---

### 9. Missing `pause_ms` in SceneSpec
**Evidence:** Breathwork script has no pauses between practices. No field exists for "beat" silence.

**Fix Required:** Add `pause_ms: Option<i64>` to `SceneSpec`. When `output.theme == "calm"`, auto-insert 800ms between scenes.

---

### 10. Caption `max_words_per_line` Default Should Be 4
**Evidence:** Script sets 4 explicitly. Default in code is 5 → too wide for 9:16 safe zone.

**Fix Required:** Change default in `CaptionsSpec::default()` to 4.

---

## Verification Results

### Build & Tests
```bash
cargo build --workspace --exclude openscript-tauri --release  # ✅ PASS (1 warning: unused music_gain)
cargo test --workspace --exclude openscript-tauri --lib --bins --tests  # ✅ 298 tests PASS
bash scripts/smoke_test_mcp.sh  # ✅ PASS (84 tools, hf.classify correct)
python3 scripts/workspace-lint/workspace_lint.py --root .  # ✅ PASS (0 errors, 2 warnings on output/ MP4s)
```

### verify.production Output (Key Dimensions)
| Dimension | Score/Max | Findings |
|-----------|-----------|----------|
| video_source_quality | 9/10 | 6 unique YouTube clips, content-relevant |
| visual_hooks | 8/8 | All real stock, no procedural |
| visual_repetition | 8/8 | 6 unique identities, max consecutive 1 |
| context_relevance | 8/8 | Jaccard 0.29 avg, topic-aware |
| cuts_pacing | 5/5 | 0.144 cuts/s in band |
| music_quality | 8/8 | Calm lo-fi, ducking active |
| sfx_quality | 6/6 | 6 events, 6 unique assets |
| sticker_design | 8/8 | **Finding: same asset all scenes** |
| caption_quality | 1/6 | **style not detected** (Parakeet fail) |
| voiceover_quality | 2/6 | voice_ids not reported, emote misaligned |
| audio_mix_quality | 1/5 | **HARD FAIL: peak -0.3 dBFS** |
| section_composition | 7/8 | No title cards, no memes |
| visual_hierarchy | 3/5 | No title cards, no reaction memes |
| platform_optimization | 4/5 | Duration 34s in sweet spot |
| timeline_editor | 4/4 | All tracks utilized |

**Grade: D** (capped by audio clipping hard gate)

---

## Files Modified During Audit
- `mcp/assets/parakeet/vocab.txt` — Added 5 missing duration tokens (8193-8197)
- `artifacts/next_iter.mp4` — Generated test video (moved to artifacts/)
- `artifacts/next_iter.mp4.debug.log` — Render debug log

---

## Next Iteration Plan (from `/tmp/openscript_fresh_agent_plan.md`)

### Phase 1: Parakeet Force-Alignment (P0)
- [ ] Re-download Parakeet ONNX models from HF `istupakov/parakeet-tdt-0.6b-v3-onnx`
- [ ] Verify `encoder-model.int8.onnx` + `decoder_joint-model.int8.onnx` + `vocab.txt` work end-to-end
- [ ] Add `system.doctor` check for model file integrity (not just existence)
- [ ] Test: `script-parse → script-to-video → verify-captions` → 0 Parakeet warnings

### Phase 2: SFX Auto-Assignment from Script (P0)
- [ ] Read `spec.sfx` in `handle_script_to_video`
- [ ] Map `role: "intro"/"highlight"/"transition"/"outro"` → SFX library categories
- [ ] Map `trigger: "scene_change"` → scene boundary timestamps
- [ ] Generate `SfxHit` for `MultiLayerRenderSpec`
- [ ] Test: `verify.production` shows `sfx_count ≥ 2` with script-driven timing

### Phase 3: Schema Discovery — `script.example` CLI (P0)
- [ ] Add `script-example` subcommand to `openscript-cli`
- [ ] Output annotated minimal `ScriptSpec` JSON with comments
- [ ] Document all required fields and enum values

### Phase 4: Sticker Variation for Single Speaker (P1)
- [ ] Query GIPHY per scene using `scene.emote` + `video_keywords`
- [ ] Rotate through top 3-4 results per scene
- [ ] Fallback to speaker preset if GIPHY fails

### Phase 5: Pause/Beats Support (P1)
- [ ] Add `pause_ms: Option<i64>` to `SceneSpec`
- [ ] Insert silence in voiceover concat when set
- [ ] Auto-set 800ms for `output.theme == "calm"`

### Phase 6: Music Auto-Select & Background Defaults (P1)
- [ ] If `music.path` omitted, query `music_library` by `output.theme` mood
- [ ] Change `BackgroundSpec.type` default from `"gameplay"` → `"procedural"`
- [ ] Document enum values in schema

### Phase 7: Title Cards Auto-Generation (P2)
- [ ] In `sections_from_timeline`, generate `title_text` for Hook/Payoff/CTA
- [ ] Render as separate caption events with `subtitle_rail` style

---

## Success Criteria for Next Simulation

| Metric | Target |
|--------|--------|
| Production Score | ≥ 95 (Grade A) |
| Parakeet Warnings | 0 |
| SFX Track (script-driven) | ≥ 2 events |
| Sticker Uniqueness (6 scenes) | ≥ 3 unique assets |
| Caption Sync | Visual verification — word-highlight matches speech |
| Title Cards | Hook + CTA have on-screen titles |
| Schema Discovery | `script-example` outputs valid JSON |
| Audio Clipping | Peak ≤ -1 dBFS |
| Wall-Clock Time | ≤ 90 seconds |

---

## Appendix: Key Code Locations

| Fix | File | Function/Line |
|-----|------|---------------|
| Parakeet model download | `setup.sh` | Add wget/curl for HF models |
| Parakeet doctor check | `crates/openscript-transcribe/src/transcriber.rs` | `check_apex_health()` pattern |
| SFX from script | `crates/openscript-mcp/src/tools.rs` | `handle_script_to_video` ~L9560 |
| Sticker per-scene | `crates/openscript-mcp/src/tools.rs` | `handle_script_to_video` ~L10150 |
| Audio limiter | `crates/openscript-ffmpeg/src/filter_graph.rs` | `FilterGraphBuilder::with_multilayer` |
| Title cards | `crates/openscript-core/src/production_quality.rs` | `sections_from_timeline` |
| `script.example` CLI | `crates/openscript-cli/src/main.rs` | New subcommand |
| `pause_ms` field | `crates/openscript-core/src/script.rs` | `SceneSpec` struct |

---

## Environment Notes
- **MCP Binary:** `target/release/openscript` (rebuilt at 0cc4d39)
- **Parakeet Models:** Present at `mcp/assets/parakeet/` but decoder malfunctioning
- **Music Library:** `mcp/assets/music_library_index.json` (418 tracks, mood/energy tagged)
- **Backgrounds Index:** `mcp/assets/backgrounds_index.json` (22 clips with mood tags)
- **API Keys:** PEXELS_API_KEY, GIPHY_API_KEY, OPENROUTER_API_KEY configured
- **Workspace Lint:** Clean (0 errors) — output/ MP4 warnings expected