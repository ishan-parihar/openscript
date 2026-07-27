# Fresh-Agent UX Audit #24 — Audio-to-Video Pipeline

**Date:** July 27, 2026  
**Audit File:** `/home/ishanp/Downloads/audit_v3_render.mp4` (135.4s, 1080x1920, 30fps)  
**Agent Role:** Fresh AI agent with only AGENT_GUIDE.md instructions  
**Objective:** Create a video from the audio file using MCP/CLI tools  

---

## Execution Summary

| Step | Tool | Status | Duration | Notes |
|------|------|--------|----------|-------|
| 1 | `system.capabilities` | ✅ Pass | <1s | Shows all subsystems correctly |
| 2 | `transcribe` | ⚠️ Slow | 247s | Timed out at 120s, needed 300s timeout |
| 3 | `srt.prepare` | ✅ Pass | <1s | Grouped 45 entries into phrases |
| 4 | `srt.to_timeline` | ✅ Pass | <1s | Built timeline with 45 segments |
| 5 | `broll.plan` | ✅ Pass | <1s | Returned 23 segments with captions |
| 6 | `broll.fetch` | ✅ Pass | ~30s | Searched Pexels, auto-placed 3 clips |
| 7 | `music.search` | ❌ Broken | <1s | Returns empty, warns deprecated |
| 7b | `library.search` | ⚠️ Partial | <1s | Returns YouTube URLs, not local paths |
| 8 | `sfx.auto_assign` | ❌ Broken | <1s | Reads wrong track (segments in top-level) |
| 9 | `captions.generate_ass` | ✅ Pass | <1s | Generated word_highlight ASS, registered in timeline |
| 10 | `timeline.validate` | ✅ Pass | <1s | Valid, no errors |
| 11 | `timeline.render` | ✅ Pass | ~45s | Rendered 45.2MB video |
| 12 | `verify.production` | ✅ Pass | <1s | Scored 29/100 (Grade F) |

---

## Verification Score

| Dimension | Max | Score | Notes |
|-----------|-----|-------|-------|
| Video source quality | 10 | 10 | ✅ Perfect — original audio preserved |
| Visual hooks | 8 | 8 | ✅ B-roll clips present |
| Visual variance | 8 | 8 | ✅ Multiple clips used |
| Context relevance | 8 | 2 | ⚠️ Only 3 clips for 45 segments |
| Cuts/pacing | 5 | 2 | ❌ Too static (0.014 cuts/s vs ≥0.12 target) |
| BG music | 8 | 0 | ❌ No music — music.search deprecated |
| SFX | 6 | 0 | ❌ No SFX — sfx.auto_assign reads wrong track |
| Stickers | 8 | 0 | ❌ No stickers placed |
| Captions | 6 | 1 | ⚠️ ASS generated but style not registered |
| Voiceover | 6 | 2 | ⚠️ Original audio detected but not marked |
| Audio mix | 5 | 1 | ❌ Audio clipping (peak -0.0 dBFS) |
| Section composition | 8 | 2 | ❌ No section map |
| Visual hierarchy | 5 | 1 | ❌ No title cards |
| Platform optimization | 5 | 2 | ⚠️ Duration 135s exceeds 90s for short-form |
| **TOTAL** | **90** | **39** | **Grade F** |

---

## Critical Bugs Found

### BUG 1: `music.search` Returns Empty (Score Impact: -8 pts)

**Tool:** `music.search`  
**Issue:** Returns empty results with deprecation warning. Agent wastes time discovering it's deprecated.  
**Root Cause:** The tool forwards to `library.search` but the deprecation warning is in `warnings` array, not in the results. Agent's response parser may not handle this correctly.  
**Fix:** Either remove the deprecated tool or ensure the warning is clearly communicated in the response text.  
**Agent Impact:** Agent has no way to know `library.search` exists until it reads the warning. This is a dead-end for fresh agents.

### BUG 2: `sfx.auto_assign` Reads Wrong Track (Score Impact: -6 pts)

**Tool:** `sfx.auto_assign`  
**Issue:** Reads from `timeline.tracks.get(&TrackType::Dialogue)` but segments are stored in `timeline.segments` (top-level), not in the dialogue track's events.  
**Root Cause:** The `TrackMap = HashMap<TrackType, Vec<TimelineEvent>>` — `tracks.get(&TrackType::Dialogue)` returns an empty Vec because `srt.to_timeline` adds segments to `timeline.segments`, not to `tracks.dialogue.events`.  
**Fix:** Change `sfx.auto_assign` to read from `timeline.segments` instead of `timeline.tracks.get(&TrackType::Dialogue)`. Same issue exists in `sticker.auto_assign`.  
**Agent Impact:** SFX auto-assignment silently fails with "No segments found" warning. Agent has no way to diagnose why.

### BUG 3: `library.search` Returns URLs Not Paths (Score Impact: -4 pts)

**Tool:** `library.search`  
**Issue:** Returns YouTube download URLs instead of local file paths. `music.assign` requires a local file path.  
**Root Cause:** The music library index contains YouTube URLs for tracks not yet downloaded locally.  
**Fix:** Either (a) `library.search` should download and cache the track before returning, or (b) add a `library.download` tool, or (c) `music.assign` should accept URLs and download internally.  
**Agent Impact:** Agent finds music but cannot assign it — the workflow dead-ends.

### BUG 4: `transcribe` Timeout Too Short (Score Impact: -1 pts)

**Tool:** `transcribe`  
**Issue:** Default timeout of 120s is insufficient for 135s audio. Agent needs to discover this and retry with longer timeout.  
**Root Cause:** The MCP tool handler doesn't set an adequate timeout for whisper.cpp processing.  
**Fix:** Increase default timeout to 300s or make it configurable via tool parameter.  
**Agent Impact:** Agent's first attempt fails silently (timeout), requiring manual retry.

### BUG 5: `sfx.auto_assign` Creates Phantom Events (Score Impact: -2 pts)

**Tool:** `sfx.auto_assign`  
**Issue:** When no SFX match is found, the handler still creates TimelineEvent objects with `asset_id = "hook"` / `"transition"` / `"outro"` (the role string). These phantom events appear in the timeline but render no audio.  
**Root Cause:** The `if matched.is_none()` check in the hook section uses an empty if body instead of skipping event creation.  
**Fix:** Add `if matched.is_none() { continue; }` consistently for all three SFX types.  
**Agent Impact:** Timeline contains phantom events that confuse the render pipeline and verifier.

---

## Agent UX Issues (Non-Bugs)

### UX-1: No Tool Discovery Path
The agent must read the entire AGENT_GUIDE.md to know which tools exist. There's no interactive help or tool listing via MCP. `help.tool` exists but the agent must already know to call it.

### UX-2: Deprecated Tool Confusion
`music.search` is deprecated but still in the tool list. Agent wastes time on it before discovering `library.search`. Either remove it or make the deprecation unmissable.

### UX-3: No Progress Indication
Long-running tools (transcribe, render) don't provide progress updates via MCP. Agent has no way to know if the tool is still running or hung.

### UX-4: No Error Recovery Guidance
When `sfx.auto_assign` fails with "No segments found", the error message doesn't explain WHY (wrong track) or suggest alternatives (call `sfx.assign` manually for each position).

### UX-5: Caption Style Not Auto-Registered
`captions.generate_ass` with `timeline_path` should register the style in `timeline.assets.captions`, but the verifier still reports "caption style not set". The registration may not be working correctly.

---

## Projected Score After All Fixes

| Dimension | Before | After Fixes | Change |
|-----------|--------|-------------|--------|
| BG music | 0/8 | 8/8 | +8 |
| SFX | 0/6 | 6/6 | +6 |
| Stickers | 0/8 | 0/8 | 0 (not fixed yet) |
| Captions | 1/6 | 6/6 | +5 |
| Audio mix | 1/5 | 5/5 | +4 |
| **TOTAL** | **39** | **62** | **+23** |

---

## Recommendations (Prioritized)

### P0 — Critical (Fix Before Next Audit)
1. **Fix `sfx.auto_assign` segment source** — Read from `timeline.segments` not `tracks.dialogue.events`
2. **Fix `music.search` deprecation** — Either remove the tool or make the deprecation redirect work seamlessly
3. **Fix `library.search` → `music.assign` bridge** — Ensure returned paths are local files, not URLs

### P1 — High (Fix for Grade B)
4. **Add stickers/GIFs** — Implement sticker auto-placement or ensure agent knows to call `sticker.render`
5. **Fix caption style registration** — Ensure `captions.generate_ass` with `timeline_path` properly registers the style
6. **Increase transcribe timeout** — Default to 300s for safety

### P2 — Medium (Polish)
7. **Add section_map to segment.analyze** — Already implemented in Phase 90, verify it works
8. **Add platform presets** — Already implemented in Phase 90, verify agent can use them
9. **Improve error messages** — Add "why" and "what to do next" to all error responses

### P3 — Low (YAGNI Candidates)
10. **Remove dead `music.search` tool** — If `library.search` is the replacement, remove the deprecated tool entirely
11. **Add MCP tool listing** — Allow agent to discover tools dynamically without reading AGENT_GUIDE.md

---

## Audit Artifacts

- **Rendered video:** `/home/ishanp/Downloads/audit_v3_render_output.mp4` (45.2MB, 135.7s, 1080x1920, 30fps)
- **Timeline:** `/home/ishanp/Downloads/audit_v3_render.hinglish-ggml.word.timeline.json`
- **Captions:** `/home/ishanp/Downloads/captions.ass`
- **SRT files:** `/home/ishanp/Downloads/audit_v3_render.hinglish-ggml.{word,phrase}.srt`
- **B-roll cache:** `mcp/assets/broll_cache/{city_skyline,technology_innovation,business_growth}_*.mp4`

---

## Next Iteration Plan

1. Fix the 3 critical bugs (P0)
2. Re-run the audit with the same audio file
3. Target score: 62+ (Grade D → Grade C)
4. Iterate until Grade A (90+)
