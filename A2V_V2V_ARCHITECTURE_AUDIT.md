# Architecture Audit: A2V/V2V Pipeline vs Golden Path

**Date:** 2026-07-23
**Scope:** Why `audio.to_video` produces garbage and how to fix the architecture
**Input analyzed:** `tools.rs` (88 MCP handlers), `multilayer_render.rs`, `server.rs`

---

## Executive Summary

The `audio.to_video` handler is a **370-line monolithic black box** that reimplements logic already available as atomic MCP tools. The agent has **zero control** over intermediate steps — no ability to inspect transcripts, adjust captions, swap backgrounds, tweak music, or validate output. The `script.to_video` golden path (1,856 lines) is also monolithic but at least has `timeline.validate()`. Neither calls `verify.production` at the end.

**The core architectural violation:** OpenScript has 88 atomic tools designed for agent orchestration, but `audio.to_video` bypasses all of them by reimplementing the pipeline inline. The agent is reduced to a single `tools/call` with no agency.

---

## 1. The Three Pipelines Compared

### 1.1 `script.to_video` (Golden Path — 1,856 lines)

```
script.parse → script.to_video (monolithic orchestrator)
  ├── Phase 1: handle_script_to_timeline → Timeline struct
  ├── Phase 2: Multi-broll backgrounds (deduplication, per-scene)
  ├── Phase 2: GIPHY stickers (per-speaker presets)
  ├── Phase 2: Meme b-roll clips
  ├── Phase 2: timeline.validate() ← HAS VALIDATION
  ├── Phase L: render_multilayer OR hyperframes
  └── Returns: timeline_preview, output_path
```

**What it does RIGHT:**
- Builds a proper `Timeline` struct (EDL v2) with tracks
- Calls `timeline.validate()` before rendering
- Multi-broll with deduplication (unique Pexels clips per scene)
- Per-speaker sticker presets with configurable position/scale
- Can branch to HyperFrames render engine
- Returns `timeline_preview` for agent inspection

**What it does WRONG:**
- Still monolithic — agent cannot intervene between phases
- Does NOT call `verify.production` at the end
- Does NOT call `verify.captions` to validate caption quality
- Phase 2 is ~800 lines of inline logic that should be atomic tools

### 1.2 `audio.to_video` (The Problem — 370 lines)

```
audio.to_video (monolithic black box)
  ├── Step 1: handle_transcribe (inline call)
  ├── Step 2: handle_srt_prepare (inline call)
  ├── Step 3: ffprobe duration analysis
  ├── Step 4: handle_broll_fetch → single Pexels clip, looped
  ├── Step 5: handle_captions_generate_ass (inline call)
  ├── Step 6: music search (local assets only)
  ├── Step 7: sfx search (sfx_index.json only)
  ├── Step 7: build_v2v_stickers (GIPHY, hardcoded params)
  ├── Step 7: render_multilayer
  └── Returns: {status, output_path, file_size, segment_count}
```

**What it does WRONG (Critical):**
1. **No Timeline struct** — builds `MultiLayerRenderSpec` directly, bypassing the timeline system
2. **No `timeline.validate()`** — no pre-render quality check
3. **No `verify.production`** — no post-render quality gate
4. **No `verify.captions`** — no caption quality validation
5. **Single background** — fetches 1 Pexels clip, loops it for entire duration (no per-scene backgrounds)
6. **Music: local only** — searches `mcp/assets/music/` (20 tracks), not Pixabay/YouTube
7. **SFX: static index** — reads `sfx_index.json`, no dynamic search
8. **Stickers: hardcoded** — position always "bottom-right", scale always 0.15
9. **No agent control** — agent cannot inspect transcript, adjust captions, swap backgrounds
10. **No timeline_preview** — agent cannot inspect intermediate state
11. **Captions may be broken** — `handle_captions_generate_ass` is called but the ASS file path may not be passed correctly to `render_multilayer`

### 1.3 `reelize.timeline` (V2V — ~2,000 lines)

```
reelize.timeline (monolithic orchestrator)
  ├── handle_transcribe (inline call)
  ├── handle_srt_prepare (inline call)
  ├── timeline.add_segment (per SRT entry)
  ├── broll.director (inline call)
  ├── music.assign (inline call)
  ├── sfx.assign (inline call)
  ├── captions.generate_ass (inline call)
  ├── render_from_timeline (not render_multilayer!)
  └── Returns: {status, output_path, timeline_path}
```

**What it does RIGHT:**
- Builds a proper Timeline struct
- Uses `render_from_timeline` (not `render_multilayer`)
- Has broll.director integration
- Has music.assign and sfx.assign

**What it does WRONG:**
- Still monolithic — no agent intervention possible
- No `timeline.validate()` call
- No `verify.production` call
- No `verify.captions` call

---

## 2. The Fragmentation Problem

The user correctly identified fragmentation between the three pipelines:

| Capability | script.to_video | audio.to_video | reelize.timeline |
|-----------|----------------|----------------|-----------------|
| **Transcription** | ✅ (via script.generate_voices) | ✅ (inline handle_transcribe) | ✅ (inline handle_transcribe) |
| **SRT Grouping** | ✅ (via script.build_captions) | ✅ (inline handle_srt_prepare) | ✅ (inline handle_srt_prepare) |
| **Timeline Build** | ✅ (Timeline struct) | ❌ (no Timeline) | ✅ (Timeline struct) |
| **Caption Generation** | ✅ (ASS with word timing) | ✅ (ASS with word timing) | ✅ (ASS with word timing) |
| **Background Fetch** | ✅ (per-scene, deduplicated) | ⚠️ (single, looped) | ✅ (via broll.director) |
| **Music** | ✅ (local + library) | ⚠️ (local only) | ✅ (via music.assign) |
| **SFX** | ✅ (via sfx.assign) | ⚠️ (sfx_index.json only) | ✅ (via sfx.assign) |
| **Stickers** | ✅ (per-speaker presets) | ⚠️ (hardcoded) | ✅ (sticker_layers) |
| **timeline.validate()** | ✅ | ❌ | ❌ |
| **verify.production** | ❌ | ❌ | ❌ |
| **verify.captions** | ❌ | ❌ | ❌ |
| **Agent control** | ❌ (monolithic) | ❌ (monolithic) | ❌ (monolithic) |
| **Render engine** | FFmpeg OR HyperFrames | FFmpeg only | FFmpeg only |

**Key insight:** All three pipelines reimplement the same steps (transcribe → group → caption → background → music → sfx → render) but with different quality levels and different inline logic. This is the fragmentation.

---

## 3. The Caption Bug

The user reported captions are not working properly. Root causes:

### 3.1 `audio.to_video` caption flow:
```
handle_transcribe → grouped_srt_path
  → handle_captions_generate_ass → ass_path
    → MultiLayerRenderSpec.captions_path = Some(ass_path)
      → render_multilayer → ffmpeg subtitles filter
```

**Potential bugs:**
1. `handle_captions_generate_ass` is called with the SRT path, but the ASS output path may collide with other files
2. The ASS file is written to the same directory as the SRT — if the directory is read-only or the path is wrong, it silently fails
3. `render_multilayer` uses `subtitles='path'` filter — if the ASS path contains special characters or spaces, ffmpeg fails silently
4. The `fonts_dir` is resolved via `resolve_fonts_dir()` which checks `OPENSCRIPT_FONTS_DIR` env var — if not set, it falls back to `$CWD/mcp/fonts` which may not exist
5. **No `verify.captions` call** — the pipeline never validates that captions actually rendered correctly

### 3.2 `script.to_video` caption flow:
```
script.generate_voices → voiceover_manifest
  → script.build_captions → captions_path (ASS)
    → MultiLayerRenderSpec.captions_path = Some(captions_path)
      → render_multilayer → ffmpeg subtitles filter
```

**Why this works better:**
- `script.build_captions` is a dedicated tool with proper ASS generation
- The caption path is passed through the voiceover manifest
- `timeline.validate()` checks that captions exist before rendering

### 3.3 The fix:
The caption system needs:
1. A `verify.captions` call AFTER rendering to validate captions are visible
2. Proper error handling when ASS generation fails (not just `warnings.push`)
3. A fonts directory that actually exists and contains Bebas Neue
4. Proper escaping of the ASS path in the ffmpeg filter

---

## 4. The Validation Gap

The biggest architectural gap: **no runtime validation** in A2V/V2V pipelines.

### What `script.to_video` has (that A2V/V2V lack):
- `timeline.validate()` — checks timeline structure before render
- Returns `timeline_preview` — agent can inspect intermediate state

### What ALL THREE lack:
- `verify.captions` — validate caption quality after render
- `verify.production` — validate overall production quality after render
- `verify.render` — validate render output (duration, resolution, codec)

### The fix:
Every pipeline MUST end with:
```
render → verify.render → verify.captions → verify.production
```

---

## 5. The Refactor Plan

### Phase 1: Delete the monolithic `audio.to_video` handler

Replace with a THIN orchestrator that calls atomic tools via `route_tool()`:

```rust
async fn handle_audio_to_video(args: Value) -> Result<Value, ToolError> {
    // 1. Transcribe
    let srt = route_tool("transcribe", json!({"video_path": audio_path})).await?;
    
    // 2. Group SRT
    let grouped = route_tool("srt.prepare", json!({"srt_path": srt_path})).await?;
    
    // 3. Generate captions
    let ass = route_tool("captions.generate_ass", json!({
        "srt_path": grouped_path,
        "style": "word_highlight",
        "font": "Bebas Neue"
    })).await?;
    
    // 4. Fetch backgrounds (per-scene, not single!)
    let backgrounds = route_tool("background.fetch", json!({
        "concepts": concepts_from_transcript
    })).await?;
    
    // 5. Search music
    let music = route_tool("music.search", json!({
        "query": transcript_concept,
        "mood": mood
    })).await?;
    
    // 6. Search SFX
    let sfx = route_tool("sfx.search", json!({
        "role": "intro"
    })).await?;
    
    // 7. Build timeline
    let timeline = route_tool("timeline.build", json!({
        "segments": segments,
        "backgrounds": backgrounds,
        "music": music,
        "sfx": sfx
    })).await?;
    
    // 8. Validate
    let validation = route_tool("timeline.validate", json!({
        "timeline_path": timeline_path
    })).await?;
    
    // 9. Render
    let render = route_tool("timeline.render", json!({
        "timeline_path": timeline_path
    })).await?;
    
    // 10. Verify
    let verify = route_tool("verify.production", json!({
        "video_path": output_path,
        "timeline_path": timeline_path
    })).await?;
    
    Ok(json!({
        "status": "rendered",
        "output_path": output_path,
        "verification": verify
    }))
}
```

### Phase 2: Delete the monolithic `reelize.timeline` handler

Same approach — thin orchestrator calling atomic tools.

### Phase 3: Add `verify.production` to `script.to_video`

Add at the end of the Phase 3/3 render step:
```rust
let verification = route_tool("verify.production", json!({
    "video_path": &output_path,
    "timeline_path": &timeline_path
})).await;
```

### Phase 4: Fix caption rendering

1. Ensure `fonts_dir` resolves correctly (check `mcp/fonts/` exists)
2. Add `verify.captions` call after every render
3. Fix ASS path escaping in ffmpeg filter
4. Add `tracing::warn!` when caption generation fails

### Phase 5: Per-scene backgrounds for A2V

The current A2V pipeline fetches 1 Pexels clip and loops it. This is unacceptable for 135s+ videos. Fix:
1. Split transcript into semantic scenes (using LLM or keyword extraction)
2. Fetch unique Pexels clips per scene
3. Use `background.fetch` with per-scene concepts
4. Deduplicate (track used Pexels IDs)

### Phase 6: Music/SFX from Pixabay

The current A2V pipeline only searches local assets (20 tracks). Fix:
1. Use `stock.search` for Pixabay music (requires `PIXABAY_API_KEY`)
2. Use `sfx.search` for dynamic SFX instead of static `sfx_index.json`
3. Fall back to local assets when API keys are not set

---

## 6. What the Agent Should See

After the refactor, the agent calling `audio.to_video` should:

1. **See intermediate state** — transcript, grouped SRT, ASS file, backgrounds, music, SFX
2. **Be able to intervene** — adjust captions, swap backgrounds, change music
3. **Get validation results** — timeline.validate + verify.production scores
4. **Get a timeline_preview** — inspect the timeline before rendering
5. **Get a quality report** — verify.production grade, caption score, audio LUFS

Currently, the agent sees ONLY: `{status: "rendered", output_path: "...", file_size: 75MB}`

---

## 7. Priority Order

| Priority | Task | Impact |
|----------|------|--------|
| **P0** | Fix caption rendering (ASS path, fonts_dir, verify.captions) | Captions are broken |
| **P0** | Add verify.production to all three pipelines | No quality gate |
| **P1** | Refactor audio.to_video to thin orchestrator | Agent has no control |
| **P1** | Add per-scene backgrounds for A2V | Single background for 135s is garbage |
| **P2** | Refactor reelize.timeline to thin orchestrator | Agent has no control |
| **P2** | Add Pixabay music/SFX to A2V | Local-only music is limited |
| **P3** | Add timeline.validate to reelize.timeline | Pre-render quality check |
| **P3** | Add verify.captions to script.to_video | Post-render caption check |

---

## 8. The Anti-Pattern

```
❌ CURRENT (monolithic black box):
   audio.to_video → agent calls ONE tool → gets output → no control

✅ TARGET (agentic orchestration):
   agent calls transcribe → inspects transcript
   agent calls srt.prepare → adjusts grouping
   agent calls captions.generate_ass → tweaks style
   agent calls background.fetch → swaps clips
   agent calls music.search → chooses track
   agent calls timeline.validate → checks quality
   agent calls render → produces video
   agent calls verify.production → validates output
```

The agent should ORCHESTRATE, not delegate to a monolithic pipeline.
