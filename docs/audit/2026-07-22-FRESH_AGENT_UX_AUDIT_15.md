# Fresh-Agent UX Audit Run #15 — Comprehensive Codebase Audit & Development Plan

**Date:** July 22, 2026  
**Trigger:** User reports no BG music or SFX in output video; requests two new workflows  
**Scope:** Full codebase audit of music/audio pipeline, transcription, broll, and workflow orchestration

---

## Executive Summary

### What Works
- **Workflow 1 (From-Scratch):** `script.to_video` produces a real MP4 (18.7MB, 26.5s, 1080×1920) ✅
- **Stock video search:** 8/8 relevance (Phase 19 fix verified) ✅
- **Voice generation:** Kokoro TTS produces clear voiceover audio ✅
- **Schema discoverability:** `script.schema` + flexible parsing (Phase 21 fixes) ✅
- **Transcription:** Whisper engine works for word-level SRT ✅
- **SFX library:** 148 indexed assets across 8 categories ✅

### What's Broken
| Issue | Severity | Root Cause |
|-------|----------|------------|
| **No BG music in output** | P0 | Music index has 0 assets — no music files on disk |
| **No SFX in output** | P0 | SFX index has 148 assets but `auto_select_sfx_hits()` may not be finding them |
| **Audio-to-Video workflow** | P1 | No single-call orchestrator for audio → transcript → video |
| **Video Editing workflow** | P1 | No attention-retention clip-spacing algorithm |
| **reelize.timeline missing music/SFX** | P1 | Pipeline doesn't auto-assign music/SFX despite having the tools |

---

## 1. Music/Audio Layer Audit

### 1.1 Music Index — EMPTY (Root Cause of No BG Music)

```
Music Index State:
  total_assets: 0
  music_paths: []
  moods: {}
  energy_levels: {}
```

**Why:** The `music_index.json` is committed to git with 0 assets. The `music.index` tool scans `$HOME/Videos/Assets/Music` (or `OPENSCRIPT_MUSIC_PATH` env var) but no music files exist at that path on this machine.

**Impact:** In `handle_script_to_video`, `music_path` resolves to `None` → no music track is added to the `MultiLayerRenderSpec` → ffmpeg renders voice-only.

**Fix Required:**
1. Download copyright-free music tracks from Pixabay (the `library.download` tool exists but needs music files)
2. Or seed the music index with bundled tracks
3. Or make `script.to_video` auto-fetch music from Pixabay when local library is empty

### 1.2 SFX Pipeline — INDEXED BUT NOT MIXED

The SFX index has 148 assets across 8 categories:
- `ambience`, `cta`, `emotion`, `highlights`, `intros`, `outros`, `text_popups`, `transitions`

**The `auto_select_sfx_hits()` function** in `handle_script_to_video` generates SFX hit positions, but:
- It may be selecting hits that don't match any indexed SFX
- The SFX files are at `$HOME/Videos/Assets/SFX` which may not exist
- The `sfx.assign` handler searches by editorial role but may fail silently

**Fix Required:**
1. Verify SFX files actually exist on disk (not just in the index)
2. Add fallback: if SFX files missing, skip silently with warning (don't crash)
3. Bundle a small set of essential SFX (whoosh, boom, transition) in `mcp/assets/sfx/`

### 1.3 Audio Mixing in render_multilayer

The `MultiLayerRenderSpec` in `crates/openscript-ffmpeg/src/multilayer_render.rs` supports:
- `music_path: Option<String>` — background music file
- `music_volume: f64` — linear volume (derived from gain_db)
- `ducking: bool` — sidechain compression under speech
- `ducking_depth_db: f64` — how much to duck
- `sfx: Vec<SfxHit>` — sound effects with timing

**The mixing works** when music_path is provided. The problem is upstream — no music file is being resolved.

---

## 2. Workflow Gap Analysis

### Workflow 1: From-Scratch Creation ✅ WORKING

```
script.schema → script.parse → script.to_video → MP4
```

**Status:** Golden trajectory works. First successful video produced in Run #14.
**Remaining gaps:** Music/SFX (see §1), production grade push to A.

### Workflow 2: Audio-to-Video ❌ NOT IMPLEMENTED

**User scenario:** "I have an audio recording (podcast, speech, voice note). Turn it into a reel."

**Required pipeline:**
```
audio.mp3 → transcribe → srt.prepare → timeline.build → [analyze segments] 
→ broll.director → music.assign → sfx.assign → timeline.render → MP4
```

**Existing tools that cover each step:**
| Step | Tool | Status |
|------|------|--------|
| Transcribe | `transcribe` | ✅ Works (Whisper engine) |
| Group captions | `srt.prepare` | ✅ Works |
| Build timeline | `timeline.build` | ✅ Works |
| Add segments | `timeline.add_segment` | ✅ Works |
| B-roll | `broll.director` | ✅ Works (needs PEXELS_API_KEY) |
| Music | `music.assign` | ✅ Works (needs music file) |
| SFX | `sfx.assign` | ✅ Works (needs SFX index) |
| Render | `timeline.render` | ✅ Works |

**What's MISSING:** A single-call orchestrator `audio.to_video` that chains these steps automatically. The agent currently has to call 7+ tools manually.

**Implementation plan:**
1. New tool: `audio.to_video` — ONE-CALL pipeline
2. Accepts: `audio_path`, `aspect`, `preset`, `broll`/`music`/`sfx` config objects
3. Internally: transcribe → srt.prepare → timeline.build → analyze_segments → broll.director → music.assign → sfx.assign → timeline.render
4. Returns: `output_path`, `timeline_path`, `segments_count`, `duration_s`

### Workflow 3: Video Editing with Attention Retention ❌ PARTIALLY IMPLEMENTED

**User scenario:** "I have an existing video. Clip it for maximum attention retention, add broll, music, SFX."

**Required pipeline:**
```
video.mp4 → transcribe → reelize.brief → [attention analysis] 
→ clip-spacing algorithm → timeline.build → broll.director → music.assign 
→ sfx.assign → timeline.render → MP4
```

**Existing tools that cover each step:**
| Step | Tool | Status |
|------|------|--------|
| Transcribe | `transcribe` | ✅ Works |
| Analyze footage | `reelize.brief` | ✅ Works |
| Build timeline | `timeline.build` | ✅ Works |
| AI-directed editing | `reelize.direct` | ✅ Works |
| Auto-edit pipeline | `reelize.timeline` | ⚠️ Partial (no music/SFX auto-assign) |
| B-roll | `broll.director` | ✅ Works |
| Render | `timeline.render` | ✅ Works |

**What's MISSING:**
1. **Attention-retention clip-spacing algorithm** — no tool analyzes speech cadence, pause patterns, or energy to determine optimal clip boundaries
2. **`reelize.timeline` doesn't auto-assign music/SFX** — the tool description says it does, but the code may not be wiring them through
3. **No `video.to_reel` orchestrator** that chains brief → attention analysis → clip → broll → music → SFX → render

**Implementation plan:**
1. New algorithm: `attention.analyze` — analyzes transcript + amplitude for attention patterns
2. New tool: `video.to_reel` — ONE-CALL pipeline for existing video editing
3. Fix `reelize.timeline` to actually assign music/SFX (verify the wiring)

---

## 3. Development Plan — Phased Implementation

### Phase 28: Fix Music/Audio Layer (P0)

**Objective:** Ensure background music and SFX appear in rendered videos.

| Task | Priority | Effort |
|------|----------|--------|
| 28a: Seed music library — download 20 copyright-free tracks from Pixabay | P0 | 30min |
| 28b: Verify SFX files exist on disk, add fallback for missing files | P0 | 15min |
| 28c: Add `auto_fetch_music` to `script.to_video` when local library empty | P0 | 45min |
| 28d: Test end-to-end with music + SFX | P0 | 15min |

### Phase 29: Audio-to-Video Workflow (P1)

**Objective:** One-call pipeline from audio file to complete reel.

| Task | Priority | Effort |
|------|----------|--------|
| 29a: Design `audio.to_video` tool schema | P1 | 20min |
| 29b: Implement `handle_audio_to_video` orchestrator | P1 | 2hrs |
| 29c: Wire transcription → timeline → broll → music → SFX → render | P1 | 1hr |
| 29d: Add to tool_definitions + AGENT_GUIDE.md | P1 | 15min |
| 29e: Test with sample audio file | P1 | 15min |

### Phase 30: Video Editing Workflow (P1)

**Objective:** One-call pipeline from existing video to attention-optimized reel.

| Task | Priority | Effort |
|------|----------|--------|
| 30a: Implement `attention.analyze` — speech cadence + pause detection | P1 | 1.5hrs |
| 30b: Design `video.to_reel` tool schema | P1 | 20min |
| 30c: Implement `handle_video_to_reel` orchestrator | P1 | 2hrs |
| 30d: Wire brief → attention → clip → broll → music → SFX → render | P1 | 1hr |
| 30e: Fix `reelize.timeline` music/SFX wiring | P1 | 30min |
| 30f: Test with sample video | P1 | 15min |

### Phase 31: Production Quality Push (P2)

**Objective:** Push production grade from B+ to A.

| Task | Priority | Effort |
|------|----------|--------|
| 31a: Cap gain_db propagation verification | P2 | 15min |
| 31b: Caption style validation through full pipeline | P2 | 30min |
| 31c: Return timeline_path from script.to_video | P2 | 15min |
| 31d: Run fresh-agent simulation Run #15 to verify all fixes | P2 | 1hr |

---

## 4. Architecture Decisions

### Music Source Strategy
- **Primary:** Local library at `$HOME/Videos/Assets/Music` (indexed via `music.index`)
- **Fallback:** Pixabay API download (via `library.download`) when local is empty
- **Bundle:** Ship 5-10 essential tracks in `mcp/assets/music/` for cold-start

### SFX Source Strategy  
- **Primary:** Local library at `$HOME/Videos/Assets/SFX` (indexed via `sfx.index`, 148 assets)
- **Bundle:** Ship essential SFX (whoosh, boom, transition, pop) in `mcp/assets/sfx/` for cold-start
- **Fallback:** Skip SFX silently with warning when files missing

### Workflow Orchestrator Pattern
All three workflows follow the same pattern:
```
orchestrator_tool(args) → 
  1. Input validation
  2. Step-by-step execution with report_progress()
  3. Error recovery (skip non-critical steps)
  4. Final render with verification
  5. Return output_path + metadata
```

### Attention-Retention Algorithm
The clip-spacing algorithm should analyze:
1. **Speech energy** — amplitude envelope from source audio
2. **Pause detection** — silence gaps > 0.5s as natural cut points
3. **Word density** — high-density segments = keep, low-density = trim
4. **Semantic boundaries** — topic shifts from transcript analysis
5. **Cadence rhythm** — match clip length to speech rhythm (2-4s clips for high energy, 4-8s for calm)

---

## 5. Tool Surface Summary

### Current: 86 tools
### After Phase 28-30: ~90 tools

New tools to add:
| Tool | Phase | Description |
|------|-------|-------------|
| `audio.to_video` | 29 | One-call audio-to-reel pipeline |
| `video.to_reel` | 30 | One-call video editing with attention retention |
| `attention.analyze` | 30 | Speech cadence + pause analysis for clip spacing |

Tools to fix:
| Tool | Phase | Issue |
|------|-------|-------|
| `script.to_video` | 28 | Auto-fetch music when library empty |
| `reelize.timeline` | 30 | Wire music/SFX assignment |

---

## 6. Risk Assessment

| Risk | Mitigation |
|------|------------|
| Pixabay API rate limits | Cache downloaded tracks, retry with backoff |
| SFX files missing at runtime | Graceful degradation with warning |
| Attention algorithm accuracy | Start with simple pause detection, iterate |
| Music/SFX add render time | Profile ffmpeg filter graph complexity |
| Breaking existing workflows | All changes are additive; no existing tools modified |

---

## 7. Success Criteria

### Phase 28 (Music/Audio Fix)
- [ ] Output MP4 has background music audible at -12dB
- [ ] Output MP4 has SFX at transitions/highlights
- [ ] `verify.audio` score >= 70/100

### Phase 29 (Audio-to-Video)
- [ ] `audio.to_video` produces complete reel from audio file
- [ ] Transcription accuracy >= 90% (Whisper)
- [ ] B-roll context relevance >= 7/8

### Phase 30 (Video Editing)
- [ ] `video.to_reel` produces attention-optimized reel
- [ ] Clip boundaries align with speech pauses
- [ ] Output duration <= source duration (cuts applied)

### Phase 31 (Production Quality)
- [ ] `verify.production` grade >= A
- [ ] All 307+ tests pass
- [ ] Zero build warnings

---

**Audit Score:** N/A (diagnostic audit, not scoring)  
**Next Action:** Phase 28 — Fix music/audio layer (P0)
