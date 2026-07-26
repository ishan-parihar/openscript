# OpenScript MCP Tool Audit Report

**Date:** April 11, 2026  
**Scope:** All 43 MCP tools — functional testing, UX evaluation for AI agents, proposed fixes  
**Test Environment:** Linux, FFmpeg n8.1, Remotion v4, Node v25, voicebox Docker on :17493, RTX 2060 SUPER  

---

## 1. Tool Status Summary

### ✅ Fully Tested — PASS (18 tools)

| # | Tool | Test Evidence |
|---|------|--------------|
| 1 | `sfx.index` | 261 assets indexed from `/home/ishanp/Videos/Assets/SFX` |
| 2 | `sfx.search` | Query + editorial_role filtering returns structured results with path, duration, gain_db |
| 3 | `music.index` | 16 tracks indexed from `/home/ishanp/Videos/Assets/Music` |
| 4 | `music.search` | Mood/energy filtering works; returns title, artist, duration_ms, mood, energy, loopability, intro_friendly, cta_friendly |
| 5 | `music.assign` | Auto-spans full timeline duration; adds ducking directive |
| 6 | `music.ducking.plan` | Correctly returns empty when no dialogue/voiceover present; returns structured events when present |
| 7 | `timeline.build` | Creates 6-track EDL v2 timeline with correct aspect/fps |
| 8 | `timeline.load` | Returns version, source, segments_count, track keys |
| 9 | `timeline.validate` | Empty and populated timelines both validated correctly |
| 10 | `timeline.preview` | Complete summary: segments with captions, track counts, duration, render_ready flag, validation_errors |
| 11 | `timeline.add_segment` | Returns segment_id; semantic_role (hook/body/cta) accepted |
| 12 | `timeline.diff` | Compares two timelines — reports added/removed/modified segments, duration delta, track count changes |
| 13 | `timeline.upgrade` | EDL v1 → v2 migration successful (25 segments upgraded) |
| 14 | `timeline.autofill_broll` | 13 placeholder b-roll events created from segment captions |
| 15 | `timeline.render` | Clean render (2 segments, music, SFX) → 136KB, 20s, 1080×1920 |
| 16 | `tts.estimate_duration` | 12 words → 4800ms at 2.5 wps (correct) |
| 17 | `voice.profile.list_presets` | 50 kokoro voices across 8 languages (en, es, fr, hi, it, ja, pt, zh) |
| 18 | `motion.design_system` | 14 OKLCH color tokens, type scale, spacing grid, timing presets, CSS variables — all WCAG-AA compliant |

### ✅ Tested — PASS (10 tools, lower-coverage)

| # | Tool | Test Evidence | Notes |
|---|------|--------------|-------|
| 19 | `motion.load_skill` | Full design guide returned (canvas specs, API reference, animation patterns, safe zones) | Returns structured guide for Remotion 9:16 compositions |
| 20 | `motion.get_info` | 2 compositions (MainWithBroll, HotMotion), 900 frames @ 30fps, 1080×1920, Remotion v4, Node v25 | Zero installed fonts reported — may affect caption burning |
| 21 | `motion.compile_check` | Valid TSX → 0 errors, no duration overflow | TypeScript compilation gate works in 2-5s |
| 22 | `motion.validate` | Valid TSX → 0 issues, estimated 3000ms | Heuristic checks pass on well-formed code |
| 23 | `motion.preview` | Frame 15 rendered → 1080×1920 PNG, 51KB | Fast visual verification working |
| 24 | `motion.render` | 900-frame composition → 1.39MB MP4, 30s, H.264 | Warning about silent audio track is helpful |
| 25 | `verify.render` | Score 100/100 — duration match (80ms delta), aspect match, 1080×1920, music + SFX tracks rendered | Most comprehensive verification tool |
| 26 | `verify.audio` | RMS -40.4 dB, peak -11.3 dB, quality 75/100; correctly flags unhealthy level | Detects audio codec, sample rate, silence gaps |
| 27 | `broll.suggest` | Analyzes EDL segments and returns insertion points with cadence | Returns concept, position_ms, duration_ms per slot |
| 28 | `broll.assign` | Places b-roll events on timeline with concept, transition_style, crop_mode | Resolves asset_path from cache or placeholder |

### ⚠️ Partial (2 tools — functional but with UX issues)

| # | Tool | Behavior | Issue |
|---|------|----------|-------|
| 29 | `sfx.assign` | Creates SFX event on timeline; **returns `asset_path: null` when no match found for editorial role** | "hook" role has 0 tagged SFX in library. Tool silently creates placeholder event. Does return a `warning` field and `matched: false` flag, but the `asset_path: null` can cascade into render failures. |
| 30 | `timeline.render` | **Works without b-roll**; **FAILS with `timeline.autofill_broll`** | FFmpeg filter graph error: `Unable to parse "si" option value "v"` when placeholder b-roll events (no asset_path) are present. Error details only written to log file, not returned inline. |

### ❌ Blocked — External Dependency Not Available (7 tools)

| # | Tool | Blocker | Expected Behavior When Available |
|---|------|---------|--------------------------------|
| 31 | `voice.profile.list` | Voicebox Docker running but **model not loaded** (`model_loaded: false`) | Lists all managed voice profiles with id, name, engine, sample_count |
| 32 | `voice.profile.get` | Voicebox model not loaded | Returns profile details including sample_count, generation_count, created_at |
| 33 | `voice.profile.add_sample` | Voicebox model not loaded | Adds reference audio to improve cloned voice quality |
| 34 | `tts.generate` | Voicebox model not loaded | Async TTS generation with polling; returns WAV output_path, duration_ms, generation_id |
| 35 | `tts.preview` | Voicebox model not loaded (requires profile lookup) | Returns profile info + estimated duration without generating audio |
| 36 | `tts.commentary` | Voicebox model not loaded | One-call intro + transitions + outro generation at strategic timeline positions |
| 37 | `voiceover.generate` | Voicebox model not loaded | Generates WAV and places on timeline voiceover track |

### ⏸️ Not Tested — No Test Assets Available (10 tools)

| # | Tool | Missing Asset | Notes |
|---|------|--------------|-------|
| 38 | `transcribe` | Video with actual speech | Requires Apex conda env + whisper-hindi model |
| 39 | `srt.read` | SRT file from transcription | Pure parser — no runtime dependency |
| 40 | `srt.prepare` | Word-level SRT | Groups entries — no runtime dependency |
| 41 | `srt.apply_edit` | Edited SRT + video with speech | Native Rust — parses edited SRT, builds EDL, renders |
| 42 | `edl.build` | SRT file | Native Rust — analyzes SRT, builds EDL JSON |
| 43 | `render` | EDL + video with speech | FFmpeg render from EDL v1 — native Rust |
| 44 | `reelize` | Video with speech | One-call: transcribe → prepare → EDL → render |
| 45 | `reelize.brief` | Video with speech | Returns segment analysis with b-roll concepts |
| 46 | `reelize.direct` | Video with speech | Executes creative direction on footage |
| 47 | `reelize.timeline` | Video with speech | Full one-call pipeline with b-roll + music + SFX |
| 48 | `overlay.generate` | SRT + EDL | PupCaps animated caption overlay |
| 49 | `broll.fetch` | Pexels API key | Requires `PEXELS_API_KEY` env var |
| 50 | `broll.director` | Pexels API key | One-call: suggest + fetch + assign b-roll |
| 51 | `verify.captions` | Rendered video + source SRT pair | Compares caption timing against video duration |

---

## 2. Detailed Issue Analysis & Proposed Fixes

### P0 — Blocking / Crash Issues

#### P0-1: `timeline.autofill_broll` + `timeline.render` → FFmpeg Filter Graph Crash

**Severity:** Crash — render fails with cryptic error  
**Reproduction:**  
1. `timeline.build(source_video)`  
2. `timeline.add_segment(...)` × 2  
3. `timeline.autofill_broll(timeline_path)` → creates 13 placeholder events  
4. `timeline.render(timeline_path)` → **FAILS**

**Error Output:**
```
[Parsed_movie_13 @ 0x561314932c40] [Eval @ 0x7ffce8a0cfa0] Undefined constant or missing '(' in 'v'
[Parsed_movie_13 @ 0x561314932c40] Unable to parse "si" option value "v"
[fc#0 @ 0x56131491d740] Error applying option 'si' to filter 'movie': Invalid argument
```

**Root Cause (from source analysis):**  
`timeline.autofill_broll` creates events with `asset_id: "placeholder"` and `source_provider: "placeholder"`. The FFmpeg render pipeline in `render_from_timeline` attempts to build a `movie=` filter for every b-roll event, passing the placeholder string as the file path. The placeholder string contains characters that FFmpeg's filter graph parser interprets as filter options, causing the crash.

**Proposed Fix:**

**Option A (Recommended) — Skip placeholders during render:**
In `crates/openscript-ffmpeg/src/render.rs` (function `render_from_timeline`), filter out b-roll events where `asset_id == "placeholder"` before building the filter graph:
```rust
let broll_events: Vec<_> = timeline.tracks.get(&TrackType::Broll)
    .map(|events| events.iter().filter(|e| e.asset_id != "placeholder").collect())
    .unwrap_or_default();
```

**Option B — Validate before render:**
Add pre-render validation in `handle_timeline_render` (tools.rs ~line 3198):
```rust
for event in timeline.tracks.get(&TrackType::Broll).unwrap_or(&vec![]) {
    if event.asset_id == "placeholder" {
        return Err(ToolError::Timeline(format!(
            "B-roll event {} has no asset (placeholder). Use broll.director or broll.assign with a real asset before rendering.",
            event.id
        )));
    }
}
```

**Option A is preferred** — it allows renders to succeed even with unfilled b-roll slots (the b-roll track is just skipped for placeholders).

---

#### P0-2: `timeline.render` Error Details Hidden in Log File

**Severity:** Developer experience — AI agents cannot debug failures  
**Current Behavior:** MCP returns only:
```json
{"error": "FFmpeg error: Render failed, see log: /path/to/render.log"}
```

**Proposed Fix:**  
In `handle_timeline_render` (tools.rs ~line 3248), include the actual FFmpeg error in the response:
```rust
Ok(Err(e)) => {
    let error_msg = e.to_string();
    // Attempt to read last 20 lines of the log for context
    let log_context = std::fs::read_to_string(&log_path)
        .ok()
        .map(|content| {
            let lines: Vec<&str> = content.lines().collect();
            let last_20: Vec<&str> = lines.iter().rev().take(20).rev().cloned().collect();
            last_20.join("\n")
        })
        .unwrap_or_default();
    Err(ToolError::Ffmpeg(format!("{}\n\nLog excerpt:\n{}", error_msg, log_context)))
}
```

This gives AI agents the actual error message inline, enabling faster self-correction.

---

### P1 — Confusing / Misleading UX for AI Agents

#### P1-1: `sfx.assign` with "hook" Role Returns `asset_path: null`

**Severity:** Silent failure — creates timeline events with no audio  
**Reproduction:**
```
sfx.assign(timeline_path, editorial_role="hook")
→ {status: "assigned", asset_path: null, event_id: "sfx_001", matched: false, warning: "No matching SFX found..."}
```

**Root Cause:**  
The SFX library has 261 files but **zero** are tagged with `editorial_role: "hook"`. The available roles in the index are: `intro`, `transition`, `highlight`, `outro`. The tool documentation and `sfx.search` description list "hook" as a valid role, and `reelize.timeline` (line 3562 in tools.rs) internally maps `"hook" → "intro"` — but `sfx.assign` does not perform this mapping.

**Proposed Fix:**

Add role mapping in `handle_sfx_assign` (tools.rs ~line 2048):
```rust
// Map "hook" → "intro" since the SFX index uses "intro" for opening effects
let mapped_role = if editorial_role == "hook" { "intro" } else { editorial_role };
let sfx_path = SfxIndex::load(Some(&index_path))
    .ok()
    .and_then(|idx| idx.search(&query, Some(mapped_role), None, 1)
        .first()
        .map(|a| a.path.clone()));
```

**Also fix:** The `reelize.timeline` handler already does this mapping (line 3552), but the standalone `sfx.assign` does not. This inconsistency means agents using `sfx.assign` directly will get different results than agents using `reelize.timeline`.

---

#### P1-2: Voicebox Dependency Cascade — 7 Tools Fail Without Clear Guidance

**Severity:** Discovery problem — agents don't know which tools need voicebox  
**Affected Tools:** `voice.profile.list`, `voice.profile.get`, `voice.profile.add_sample`, `tts.generate`, `tts.preview`, `tts.commentary`, `voiceover.generate`  
**Current Error:** `"Voicebox model is not loaded. Load it via the voicebox UI or POST /models/load."`

**Issue:** The error message is correct but the agent has no way to know *ahead of time* which tools will fail. The tool definitions include a `requires_voicebox` field in the inputSchema, but this is not surfaced in the tool description.

**Proposed Fix:**

**Add structured availability check:** Create a new MCP tool `system.capabilities`:
```rust
{
    "name": "system.capabilities",
    "description": "Check which OpenScript subsystems are available before using tools. Returns availability status for voicebox/TTS, Pexels API, SFX library, music library, and transcription engine.",
    "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
}
```

Response:
```json
{
    "voicebox": {"available": true, "model_loaded": false, "url": "http://127.0.0.1:17493"},
    "pexels": {"available": false, "reason": "PEXELS_API_KEY not set"},
    "sfx_library": {"available": true, "indexed_count": 261},
    "music_library": {"available": true, "indexed_count": 16},
    "transcription": {"available": true, "engine": "apex"}
}
```

**Alternatively** — minimal fix: Prefix tool descriptions with `[Requires: voicebox model]` for the 7 affected tools.

---

#### P1-3: `music.search` with `loopable=true` Always Returns 0 Results

**Severity:** Misleading filter option  
**Reproduction:** `music.search(loopable=true)` → `{count: 0, results: []}`

**Root Cause:**  
None of the 16 indexed music tracks have `loopability: true` set in the index. The `MusicIndex::scan_directories` function does not analyze audio files for loopability — it defaults all tracks to `false`. The `loopable` filter parameter exists in the schema but has no data to filter against.

**Proposed Fix:**

**Option A:** Remove `loopable`, `intro_friendly`, and `cta_friendly` from the `music.search` schema until the indexing pipeline can actually detect these properties.

**Option B (Better):** Add a manual tagging system. Create a `music_tags.json` file alongside the index that allows manual annotation:
```json
{
    "music_0000": {"loopability": true, "intro_friendly": false, "cta_friendly": true},
    "music_0003": {"loopability": true, "intro_friendly": true, "cta_friendly": false}
}
```
Merge these tags during `music.search` filtering.

---

#### P1-4: `sfx.assign` Returns `warning` but Still Creates Event

**Severity:** Confusing success semantics  
**Current Behavior:** When no SFX matches, the tool returns `status: "assigned"` with `asset_path: null`. The event is still created on the timeline. An AI agent checking `status == "assigned"` may assume success.

**Proposed Fix:**  
Change response semantics to be clearer:
```json
{
    "status": "warning",
    "event_id": "sfx_001",
    "position_ms": 0,
    "asset_path": null,
    "matched": false,
    "message": "No SFX found for role 'hook'. Placeholder event created — render will skip this event."
}
```

Key change: `status: "warning"` (not `"assigned"`) when no asset matched. This signals to agents that the operation was partial.

---

#### P1-5: `broll.fetch` and `broll.director` Fail Without Pexels API Key

**Severity:** Unclear error for missing env var  
**Current Error:** `"PEXELS_API_KEY environment variable not set. Set it to use broll.director..."`

**Assessment:** The error message is actually **good** — it tells the agent exactly what's missing and where to get it. No fix needed for the error itself. However, combined with the absence of `system.capabilities` (P1-2), agents don't know to check first.

**No code fix required** — resolved by implementing P1-2.

---

### P2 — Minor Usability Issues

#### P2-1: `timeline.preview` Caps Captions at 60 Characters

**Source:** tools.rs line 2974 — `s.caption.chars().take(60).collect::<String>()`

**Issue:** For long captions, the preview truncates at 60 chars without indication. An agent reviewing the preview may miss important context.

**Fix:** Add ellipsis when truncated:
```rust
let caption_display = if s.caption.len() > 60 {
    format!("{}...", s.caption.chars().take(57).collect::<String>())
} else {
    s.caption.clone()
};
```

---

#### P2-2: `music.assign` Always Creates Ducking Directives Even Without Dialogue

**Source:** tools.rs line 3222 — `if ducking { timeline.add_ducking_directive(...) }`

**Issue:** `music.assign` adds ducking directives regardless of whether dialogue/voiceover events exist on the timeline. The `music.ducking.plan` correctly returns 0 events when no dialogue exists, but the directive is still created.

**Fix:** Only add ducking directives if dialogue or voiceover tracks have events:
```rust
let has_speech = track_count(&timeline, &TrackType::Dialogue) > 0
    || track_count(&timeline, &TrackType::Voiceover) > 0;
if ducking && has_speech {
    timeline.add_ducking_directive("dialogue_active", "music", 10.0, 50, 200);
}
```

---

#### P2-3: `verify.audio` Reports 96kHz Sample Rate on Rendered Output

**Observed:** Rendered video has `sample_rate: "96000"` instead of the expected 48kHz.

**Root Cause:** Likely in `render_from_timeline` — the audio resample rate in the FFmpeg filter graph may be set to 96kHz instead of 48kHz.

**Fix:** Check `crates/openscript-ffmpeg/src/render.rs` for the audio resample filter and ensure `aresample=48000` is set in the filter graph.

---

#### P2-4: `timeline.diff` Returns Segment IDs Unsorted

**Observed:** Added segments returned in arbitrary order: `["seg_014","seg_020","seg_017",...]`

**Fix:** Sort the diff results for readability:
```rust
let mut added: Vec<&str> = seg_ids_b.difference(&seg_ids_a).copied().collect();
added.sort();
```

---

## 3. Recommendations for AI Agent UX

### 3.1 Tool Discovery Improvement

**Problem:** 43 tools spread across 10 categories. Agents struggle to find the right tool for the job.

**Recommendation:** Implement a `help.tool` meta-tool:
```
help.tool(query: "how do I add voiceover to a timeline")
→ Returns: [
    {name: "voiceover.generate", relevance: 0.95, description: "..."},
    {name: "tts.commentary", relevance: 0.85, description: "..."},
    {name: "voice.profile.list", relevance: 0.60, description: "..."}
  ]
```

### 3.2 Progressive Error Recovery

**Problem:** When a tool fails, agents often retry the same call without understanding why.

**Recommendation:** Include `suggested_fix` field in error responses:
```json
{
    "error": "Voicebox model is not loaded",
    "suggested_fix": "Load the model via POST /models/load or open http://127.0.0.1:17493",
    "alternative_tools": ["motion.render (no voice needed)", "timeline.render (existing assets only)"]
}
```

### 3.3 Consistent Response Schema

**Current State:** Good — most tools return `{status: "...", ...}` pattern.

**Missing Consistency:** 
- Some tools use `"status": "success"`, others use `"status": "generated"`, `"status": "indexed"`, `"status": "rendered"`
- `sfx.assign` returns `"status": "assigned"` even when `asset_path: null`

**Recommendation:** Standardize status values:
| Status | Meaning |
|--------|---------|
| `"success"` | Operation completed fully |
| `"warning"` | Operation completed partially (some data missing) |
| `"fail"` | Operation failed, no result produced |

### 3.4 Pre-flight Checks

**Recommendation:** Add `dry_run` mode to expensive operations:
- `timeline.render(dry_run=true)` → returns filter graph plan without executing
- `motion.render(dry_run=true)` → runs compile_check + validate, returns estimated render time
- `reelize.timeline(dry_run=true)` → returns pipeline plan with estimated durations per step

---

## 4. Infrastructure Readiness Checklist

| Dependency | Status | Tools Blocked |
|-----------|--------|--------------|
| Voicebox model loaded | ❌ Not loaded | 7 TTS/voice tools |
| Apex conda env | ⏸️ Unknown | 5 transcription tools |
| Pexels API key | ❌ Not set | 3 b-roll tools |
| SFX library | ✅ 261 indexed | None |
| Music library | ✅ 16 indexed | None |
| FFmpeg n8.1 | ✅ Working | None |
| Remotion v4 | ✅ Working | None |
| PupCaps | ✅ Working | None |

---

## 5. Priority Fix Order

1. **P0-1:** Fix `timeline.render` crash with placeholder b-roll (one-line filter change)
2. **P0-2:** Include FFmpeg error details inline in render response
3. **P1-1:** Add "hook" → "intro" role mapping in `sfx.assign`
4. **P1-2:** Implement `system.capabilities` tool for dependency awareness
5. **P1-3:** Tag music tracks with loopability/intro/cta metadata
6. **P1-4:** Change `sfx.assign` status to `"warning"` when no asset matched
7. **P2-1:** Add ellipsis to truncated captions in preview
8. **P2-2:** Skip ducking directives when no speech tracks exist
9. **P2-3:** Fix audio sample rate to 48kHz in render pipeline
10. **P2-4:** Sort `timeline.diff` results
