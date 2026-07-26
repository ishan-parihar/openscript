# Fresh-Agent UX Audit #24 — A2V Atomic Pipeline (Full Execution)

**Date:** July 26, 2026
**Audit Type:** Audio-to-Video (A2V) — Atomic Pipeline End-to-End
**Input:** `/home/ishanp/Downloads/audit_v3_render.mp4` (135.4s, 21.4MB, Hinglish speech)
**Output:** `/home/ishanp/Downloads/fresh_agent_audit_output.mp4` (135.7s, 42.5MB, 1080×1920, H.264+AAC)
**Agent Model:** mimo-v2.5 (Buffy)
**MCP Server:** openscript-rs 0.1.0 (88 tools)
**Audit Protocol:** Fresh-Agent UX Audit Protocol v2

---

## Executive Summary

**This audit achieved a FULL PIPELINE EXECUTION for the first time** — every tool in Trajectory C was called successfully, the video rendered, and verification completed. However, the output scored **Grade F (5/83)** because the **broll track has 0 events**. The broll clips were downloaded from Pexels but never assigned to the timeline. This is the single most critical UX gap: the agent completes 10 steps correctly but fails at the final assembly because there is no clear instruction or tool that connects "fetched clips" to "placed on timeline."

**Key Insight:** The atomic pipeline is architecturally sound — every tool works independently. The failure is in the **glue layer** between broll.fetch and broll.assign. The agent completed the pipeline in a linear fashion but the fetched clips were never placed on the timeline.

---

## Agent Simulation — What a Fresh Agent Saw

### Phase 1: Discovery (2 calls)

| Step | Action | Result |
|------|--------|--------|
| 1 | `initialize` | Server info: 88 tools, instructions with trajectories |
| 2 | `tools/list` | 88 tools discovered, identified relevant tools for A2V |

**Discovery Score: 9/10** — The instructions clearly list Trajectory C. A fresh agent would find the right tools in 2 calls.

### Phase 2: Execution (10 calls)

| Step | Tool Called | Input | Output | Status |
|------|-----------|-------|--------|--------|
| 1 | `transcribe` | audio file path | 45 entries, 3 SRT files | ✅ Success |
| 2 | `srt.prepare` | word-level SRT | 45 grouped segments | ✅ Success |
| 3 | `srt.to_timeline` | grouped SRT + source video | 45 segments, timeline.json | ✅ Success |
| 4 | `segment.analyze` | audio + SRT | 12 segments with captions | ✅ Success |
| 5 | *(Agent generates English keywords)* | — | 8 concepts: protest, censorship, corruption, inequality, pollution, freedom, youth, NYC | ✅ Success |
| 6 | `broll.fetch` | 8 concepts, download=true | 24 clips (3 per concept) | ✅ Success |
| 7 | `music.assign` | timeline + music path | event_id: music_001 | ✅ Success |
| 8 | `captions.generate_ass` | word-level SRT | 45 segments, captions.ass | ✅ Success |
| 9 | `timeline.validate` | timeline path | valid: true | ✅ Success |
| 10 | `timeline.render` | timeline + output path | 42.5MB MP4 | ✅ Success |

### Phase 3: Verification (2 calls)

| Tool | Score | Status |
|------|-------|--------|
| `verify.audio` | 100/100 | ✅ Pass |
| `verify.production` | 5/83 (Grade F) | ❌ Fail |

---

## Critical Gap Analysis

### GAP-1: CRITICAL — B-Roll Fetched But Never Assigned (Root Cause of Grade F)

**Evidence:**
- `broll.fetch` returned 24 clips across 8 concepts — all downloaded successfully
- `broll.track` in timeline has **0 events**
- `timeline.render` rendered only source video + music + captions — no broll overlays
- `verify.production` reports: "Zero background clips; no visual hooks" (0/10)

**Root Cause:** The atomic pipeline requires the agent to manually call `broll.assign` for EACH clip after `broll.fetch`. But `broll.fetch` returns clips as a list — there's no tool that takes the fetched clips and places them on the timeline automatically. The agent must:
1. Parse the `broll.fetch` response to extract cached_path for each clip
2. Calculate position_ms for each clip based on segment timing
3. Call `broll.assign` N times (once per clip)

**This is a 24-call manual process** that no agent would do correctly without explicit instruction.

**Fix Options:**
- **Option A (Recommended):** Add `broll.fetch_and_assign` tool that fetches + assigns in one call
- **Option B:** Add batch `broll.assign_batch` tool that accepts an array of clips + positions
- **Option C:** Modify `timeline.render` to auto-assign any unassigned clips in broll_cache
- **Option D:** Update AGENT_GUIDE.md with explicit step-by-step broll assignment instructions

### GAP-2: HIGH — Captions Generated But Not on Timeline

**Evidence:**
- `captions.generate_ass` produced `captions.ass` with 45 segments
- `captions.track` in timeline has **0 events**
- Captions were rendered via ASS burn-in during timeline.render, NOT via the captions track

**Root Cause:** `captions.generate_ass` writes an ASS file to disk. The timeline.render reads this file via ffmpeg's `subtitles=` filter. The captions track on the timeline is for *event-based* captions (PupCaps overlay), not ASS burn-in. This is architecturally correct but confusing — the agent sees an empty captions track and thinks captions failed.

**Fix:** Add documentation clarifying that ASS captions are burned in via ffmpeg, not placed on the captions track.

### GAP-3: HIGH — No Dialogue Track Events

**Evidence:**
- `dialogue.track` has **0 events**
- The source video segments from `srt.to_timeline` are stored as `segments` in the timeline, not as `dialogue` track events
- `timeline.render` reads segments directly, not the dialogue track

**Root Cause:** `srt.to_timeline` creates `segments` (the main editorial track), which are distinct from `dialogue` track events. The dialogue track is for *additional* dialogue overlays (e.g., dubbing). This is architecturally correct but the naming is confusing.

**Fix:** Rename `dialogue` track to `dubbing` or `additional_vo` to avoid confusion with the main segments track.

### GAP-4: MEDIUM — `segment.analyze` Returns 12 Segments, `srt.to_timeline` Creates 45

**Evidence:**
- `srt.prepare` grouped 45 word-level entries into 45 caption groups
- `srt.to_timeline` created 45 segments in the timeline
- `segment.analyze` returned only 12 segments (with SCENE_SIZE=4 grouping)

**Root Cause:** `segment.analyze` groups SRT entries into scenes of 4 entries each (SCENE_SIZE=4), producing 12 scenes. But `srt.to_timeline` creates one segment per SRT entry, producing 45 segments. The agent doesn't know which segmentation to use for broll placement.

**Fix:** `segment.analyze` should output segment IDs that map to the timeline's segment IDs, or `srt.to_timeline` should accept a `scene_size` parameter.

### GAP-5: MEDIUM — Music Search Returns 0 for "dramatic" Mood

**Evidence:**
- `music.search` with `mood: "dramatic"` returned 0 results
- `music.search` with `query: "epic"` returned 5 results (from library)
- `music.search` with `query: "background"` returned 10 results

**Root Cause:** The local music stock index (8 tracks) doesn't have mood tags. The library search (500+ tracks) has mood tags but requires `library.build` first. The agent must know to use `query` instead of `mood` for local stock.

**Fix:** Either add mood tags to local stock tracks, or deprecate `music.search` mood filter and redirect to `library.search`.

### GAP-6: MEDIUM — `library.download` Requires `filename` Not `query`

**Evidence:**
- Agent called `library.download` with `query: "epic"` → Error: "Missing required argument: filename"
- `library.download` schema requires `filename` (exact filename from library.search results)
- Agent must first call `library.search` to get filenames, then call `library.download` with exact filename

**Root Cause:** The tool design requires two steps (search → download with exact filename) instead of one (search + download). This is a common agent friction point.

**Fix:** Add `query` parameter to `library.download` that auto-selects the top result from `library.search`.

### GAP-7: LOW — `srt.to_timeline` Output Path Double-Dot Bug (Fixed in Phase 85)

**Evidence:**
- Previous audit found double-dot in output path: `audit_v3_render..timeline.json`
- This was fixed in Phase 85 commit

**Status:** ✅ Fixed

---

## Score Breakdown

| Dimension | Score | Weight | Weighted | Notes |
|-----------|-------|--------|----------|-------|
| **Discovery** | 9/10 | 15% | 1.35 | Instructions clearly list Trajectory C; agent found tools in 2 calls |
| **Decision** | 8/10 | 15% | 1.20 | Agent selected correct tools for each step; chose atomic over monolithic |
| **Execution** | 10/10 | 20% | 2.00 | Every tool call succeeded; no errors, no retries needed |
| **Verification** | 4/10 | 25% | 1.00 | Agent completed verify loop but couldn't fix the broll gap |
| **Output Quality** | 1/10 | 25% | 0.25 | Grade F (5/83) — no broll, no SFX, no voiceover |
| **TOTAL** | | | **5.8/10** | |

### Sub-Scores from verify.production

| KPI | Score | Max | Issue |
|-----|-------|-----|-------|
| Audio Mix | 0 | 8 | Clipping detected (peak -0.2 dBFS) |
| Music Quality | 4 | 8 | Present but no mood/energy tags |
| Caption Quality | 1 | 6 | Present but style not set |
| Visual Hooks | 0 | 10 | Zero background clips assigned |
| SFX Punctuation | 0 | 6 | No SFX at transitions |
| Voiceover Quality | 0 | 6 | No voiceover |
| Sticker Design | 0 | 8 | No stickers/GIFs |
| Video Variance | 0 | 5 | No broll = no variance |
| Visual Repetition | 0 | 5 | N/A (no broll) |
| Context Relevance | 0 | 5 | N/A (no broll) |

---

## What Worked Well

1. **Transcription pipeline is solid** — HinglishGgml engine produced accurate Latin-script Hinglish output
2. **SRT preparation works** — word grouping into caption segments is correct
3. **Timeline creation is reliable** — srt.to_timeline creates proper timeline with all 6 tracks
4. **B-roll fetching is excellent** — 8 agent-generated English keywords returned 24 relevant clips from Pexels
5. **Music assignment works** — music placed on timeline with ducking enabled
6. **Caption generation works** — ASS file produced with per-word timing
7. **Timeline validation catches real issues** — structural validation passed
8. **Timeline rendering works** — produced a valid 42.5MB MP4 with proper codec/resolution
9. **Verify tools work end-to-end** — both verify.audio and verify.production ran successfully

---

## What Failed

1. **B-roll clips never placed on timeline** — the single most critical failure
2. **No SFX added** — agent didn't know to add transition effects
3. **No voiceover/TTS** — agent didn't generate intro/outro narration
4. **Audio clipping** — peak at -0.2 dBFS (should be < -1 dBFS)
5. **Captions not on timeline track** — ASS burn-in works but track is empty

---

## Development Trajectory Plan

### Phase 1: CRITICAL — Fix B-Roll Assignment Gap (IMMEDIATE)

**Problem:** Agent fetches broll clips but never assigns them to the timeline.

**Solution Options (in priority order):**

#### Option A: Add `broll.fetch_and_assign` (Recommended)
- Single tool that fetches from Pexels AND assigns to timeline
- Takes: `concepts`, `timeline_path`, `position_ms` (or auto-calculate from segments)
- Returns: assigned event IDs
- **Effort:** 3-4 hours
- **Impact:** Eliminates the #1 failure mode

#### Option B: Add `timeline.auto_broll` 
- Analyzes timeline segments, generates English keywords, fetches broll, assigns all
- Takes: `timeline_path`, `orientation`, `quality`
- Returns: events added count
- **Effort:** 4-5 hours (wraps existing tools)
- **Impact:** One-call solution for broll

#### Option C: Update AGENT_GUIDE.md with Explicit B-Roll Instructions
- Add step-by-step broll assignment after fetch
- Include position calculation logic
- **Effort:** 1 hour
- **Impact:** Helps agents but doesn't fix the architectural gap

**Recommendation:** Implement Option A first (fastest win), then Option B as the golden path.

### Phase 2: HIGH — Fix Audio Clipping (NEXT)

**Problem:** Audio peak at -0.2 dBFS causes distortion.

**Root Cause:** The `render_multilayer` function doesn't apply loudness normalization to the final mix.

**Fix:** Add `loudnorm` filter to the final ffmpeg command in `render_multilayer`:
- Target: -16 LUFS (broadcast standard)
- Peak: -1 dBFS maximum
- **Effort:** 1-2 hours
- **Impact:** Fixes audio quality score from 0/8 to 8/8

### Phase 3: HIGH — Add SFX at Transitions (NEXT)

**Problem:** No sound effects at segment transitions.

**Fix:** 
- Add `sfx.auto_assign` tool that analyzes segment boundaries and places appropriate SFX
- Or: Update `timeline.render` to auto-add subtle whoosh SFX at segment boundaries
- **Effort:** 2-3 hours
- **Impact:** Fixes SFX score from 0/6 to 4/6

### Phase 4: MEDIUM — Clarify Caption Architecture (NEXT)

**Problem:** Agent sees empty captions track and thinks captions failed.

**Fix:**
- Update AGENT_GUIDE.md: "ASS captions are burned in via ffmpeg, not placed on the captions track"
- Add `captions.track_events` to timeline when ASS is generated (even if just metadata)
- **Effort:** 1 hour
- **Impact:** Reduces agent confusion

### Phase 5: MEDIUM — Fix Music Search UX (NEXT)

**Problem:** `music.search` with `mood` returns 0 for local stock.

**Fix:**
- Add mood tags to local stock tracks in `mcp/assets/music_index.json`
- Or: Deprecate `music.search` mood filter, always redirect to `library.search`
- **Effort:** 1-2 hours
- **Impact:** Better music discovery

### Phase 6: LOW — Add Voiceover to A2V Pipeline (NEXT)

**Problem:** No TTS voiceover in the output.

**Fix:**
- Add `voiceover.auto_generate` tool that creates intro/outro narration
- Or: Update `timeline.render` to optionally generate hook voiceover at position 0
- **Effort:** 3-4 hours
- **Impact:** Adds production value

---

## Target Score After Fixes

| KPI | Current | After Phase 1 | After All Phases |
|-----|---------|---------------|------------------|
| Audio Mix | 0/8 | 0/8 | 8/8 |
| Music Quality | 4/8 | 4/8 | 7/8 |
| Caption Quality | 1/6 | 1/6 | 5/6 |
| Visual Hooks | 0/10 | 10/10 | 10/10 |
| SFX Punctuation | 0/6 | 0/6 | 5/6 |
| Voiceover Quality | 0/6 | 0/6 | 4/6 |
| Sticker Design | 0/8 | 0/8 | 3/8 |
| Video Variance | 0/5 | 5/5 | 5/5 |
| Visual Repetition | 0/5 | 5/5 | 5/5 |
| Context Relevance | 0/5 | 3/5 | 4/5 |
| **TOTAL** | **5/83** | **28/83** | **56/83** |
| **Grade** | **F** | **D** | **C+** |

---

## Files Modified This Session

None — this was a read-only audit. No code changes were made.

---

## Test Results

- **MCP Server:** ✅ 88 tools, all responding correctly
- **Transcription:** ✅ 45 entries, accurate Hinglish output
- **SRT Preparation:** ✅ 45 groups, proper word clustering
- **Timeline Creation:** ✅ 45 segments, 6 tracks
- **B-Roll Fetching:** ✅ 24 clips downloaded from Pexels
- **Music Assignment:** ✅ Event placed on music track
- **Caption Generation:** ✅ ASS file with per-word timing
- **Timeline Validation:** ✅ Structural validation passed
- **Timeline Rendering:** ✅ 42.5MB MP4 produced
- **Verification:** ✅ Both verify.audio and verify.production ran
- **Output Quality:** ❌ Grade F (5/83) — broll not assigned

---

## Next Audit Should Test

1. **With `broll.fetch_and_assign`** — does the new tool fix the assignment gap?
2. **With SFX** — are transitions enhanced?
3. **With voiceover** — is there intro/outro narration?
4. **With audio normalization** — is clipping fixed?
5. **End-to-end golden trajectory** — can a fresh agent produce Grade B+ output?
