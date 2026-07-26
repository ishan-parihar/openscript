# V2V & A2V Workflow Architecture Audit

> **Date:** July 23, 2026
> **Scope:** `reelize.timeline` (V2V), `audio.to_video` (A2V), `script.to_video` (golden path reference)
> **Goal:** Audit monolithic vs agentic patterns, identify implementation gaps, create a plan to atomize V2V/A2V into agentic tool-call sequences.

---

## 1. Executive Summary

Both `reelize.timeline` (572 lines) and `audio.to_video` (361 lines) are **monolithic orchestrators** that embed all decision-making and tool-chaining logic inside a single Rust handler. This is architecturally inconsistent with the project's own golden path (`script.to_video`) and the existing agentic tools (`reelize.brief` → `reelize.direct`), where the **AI agent decides the tool sequence** and each tool is an atomic building block.

The core issue: these tools are **deterministic video generators** instead of **complete workflows orchestrated agentically by a pool of tool-calls**.

### Key Findings

| Tool | Lines | Pattern | Agentic? | Issue |
|------|-------|---------|----------|-------|
| `script.to_video` | ~1,980 | 3-phase orchestrator | ✅ Delegates to `handle_script_to_timeline` | Gold standard |
| `reelize.brief` | 283 | Analytical brief | ✅ Agent reads brief, makes decisions | Gold standard |
| `reelize.direct` | ~280 | Agent-directed executor | ✅ Takes agent's structured instructions | Gold standard |
| `reelize.timeline` | 572 | **Monolithic 7-step chain** | ❌ All decisions hardcoded in Rust | **Needs decomposition** |
| `audio.to_video` | 361 | **Monolithic 7-step chain** | ❌ All decisions hardcoded in Rust | **Needs decomposition** |

---

## 2. Detailed Analysis

### 2.1 `reelize.timeline` (Video → Video) — Monolithic Architecture

**Current 7-step chain (all hardcoded in Rust):**

```
Step 1/7: Transcribe → handle_transcribe()                          [atomic tool ✓]
Step 2/7: Group captions → handle_srt_prepare()                      [atomic tool ✓]
Step 3/7: Build timeline → Timeline::new() + populate_segments()     [MISSING: timeline.build]
          + ASS generation → generate_ass()                          [inline, not a tool]
Step 4/7: B-roll → handle_broll_director()                           [atomic tool ✓]
Step 5/7: Music → handle_library_search() + handle_music_assign()    [atomic tools ✓]
          SFX → inline event creation (NOT using sfx.assign!)        [INLINE GAP]
Step 6/7: Animated captions → overlay.generate (if enabled)          [atomic tool ✓]
Step 7/7: Render → render_from_timeline()                            [atomic tool ✓]
```

**Specific problems:**

1. **No agent decision points** — Every step is hardcoded. The agent cannot:
   - Skip b-roll for a specific segment
   - Choose different music per mood
   - Re-order steps (e.g., add music before b-roll)
   - Add voiceover commentary
   - Use `reelize.brief` to analyze first, then direct

2. **SFX is inline, not using `sfx.assign`** — Lines 4910-4960 create SFX events directly via `timeline.add_track_event()` instead of using the atomic `sfx.assign` tool. This means:
   - No SFX index searching
   - No editorial role matching
   - Hardcoded "hook at 0ms, transitions at boundaries, highlight at midpoint"

3. **ASS generation is inline** — The caption generation code (~60 lines) does timestamp remapping with crossfade offsets. This is not available as a standalone tool.

4. **Music path resolution is fragile** — Lines 4860-4900 have a 3-step fallback chain (music_cache → asset dir → first .mp3) that duplicates logic in `handle_music_assign`.

5. **No `timeline.validate` before render** — The tool skips validation despite having a `timeline.validate` tool available.

6. **No `verify.production`** — No quality gate after render.

### 2.2 `audio.to_video` (Audio → Video) — Monolithic Architecture

**Current 7-step chain (all hardcoded in Rust):**

```
Step 1/7: Transcribe → handle_transcribe()                            [atomic tool ✓]
Step 2/7: Group captions → handle_srt_prepare()                        [atomic tool ✓]
Step 3/7: Analyze duration → ffprobe (inline)                          [inline]
          Parse SRT → parse_srt() (inline)                             [inline]
Step 4/7: Fetch backgrounds → handle_broll_fetch()                     [atomic tool ✓]
          Map to segments → inline loop                                [INLINE GAP]
          Generate placeholder → solid-color MP4 (inline)              [INLINE GAP]
Step 5/7: Generate ASS captions → generate_ass()                       [inline, not a tool]
Step 6/7: Music + SFX → inline search + event creation                 [INLINE GAP]
Step 7/7: Render → render_multilayer()                                 [atomic tool ✓]
```

**Specific problems:**

1. **Bypasses the timeline entirely** — `audio.to_video` calls `render_multilayer()` directly with a `MultiLayerRenderSpec`, never creating a `Timeline` object. This means:
   - No `timeline.build`, `timeline.add_segment`, `timeline.validate`, `timeline.render`
   - No `timeline.preview` or `timeline.inspect` for debugging
   - The timeline JSON path is created *after* rendering, not before

2. **No agent decision points** — Same as V2V: every decision is hardcoded.

3. **Placeholder generation is inline** — Creates a solid-color MP4 with ffmpeg when no backgrounds are available. This is not a reusable tool.

4. **SFX is inline** — Only adds a "hook" SFX, no transition/highlight SFX. Uses raw `sfx_hits` array instead of `sfx.assign`.

5. **Music search is inline** — Calls `handle_library_search` directly with hardcoded mood/energy from args, not giving the agent a chance to search and curate.

6. **No `verify.production`** — No quality gate.

### 2.3 `script.to_video` (Golden Path) — Reference Architecture

**3-phase orchestrator:**

```
Phase 1/3: Timeline Assembly
  → handle_script_to_timeline()  [delegates to atomic tools]

Phase 2/3: Layered Composition
  → background.fetch (per scene, multi-broll)
  → sticker.render (per scene)
  → music/stock signal routing

Phase 3/3: Render
  → render_multilayer() or hf.render()
```

**What makes it work:**
- Clean phase separation
- Each phase delegates to atomic sub-handlers
- The agent can inspect between phases (timeline_preview)
- Has production quality gates (verify.production)

---

## 3. Implementation Gap Matrix

### 3.1 Missing Atomic Tools for V2V/A2V Agent-Orchestration

| Gap | Current State | Required Tool | Priority |
|-----|--------------|---------------|----------|
| Caption generation from word-level SRT | Inline in both handlers (~60 lines each) | `captions.generate_ass` | **P0** |
| Timestamp remapping for crossfades | Inline in reelize.timeline (~30 lines) | Part of `captions.generate_ass` | **P0** |
| SFX assignment by editorial role | Inline in both handlers | `sfx.assign` (already exists!) | **P0** — just needs wiring |
| Duration analysis | Inline ffprobe in audio.to_video | `system.duration` or use `timeline.build` | P1 |
| Background-to-segment mapping | Inline loop in audio.to_video | `background.assign` (already exists!) | **P0** — just needs wiring |
| Placeholder background generation | Inline solid-color MP4 creation | `background.placeholder` | P1 |
| Timeline validation before render | Skipped in both handlers | `timeline.validate` (already exists!) | **P0** — just needs wiring |
| Production quality gate | Missing from both | `verify.production` (already exists!) | **P0** — just needs wiring |
| Voiceover/commentary | Not available in V2V/A2V | `voiceover.generate` (already exists!) | P1 — agent can use directly |

### 3.2 DRY Violations Between Handlers

| Duplicated Logic | Location in reelize.timeline | Location in audio.to_video | Solution |
|-----------------|---------------------------|--------------------------|----------|
| ASS generation + word timing | ~60 lines | ~45 lines | Extract to `captions.generate_ass` tool |
| Music search + path resolution | ~50 lines | ~20 lines | Already exists as `music.assign` |
| SFX event creation | ~40 lines | ~10 lines | Already exists as `sfx.assign` |
| Timeline construction + save | ~30 lines | N/A (no timeline) | Use `timeline.build` + `timeline.add_segment` |

---

## 4. Proposed Architecture: Agent-Orchestrated V2V/A2V

### 4.1 New Trajectory — Trajectory E: Video/Audio → Video (Agent-Directed)

```
For V2V (existing video):
  reelize.brief → agent decides → timeline.build → timeline.add_segment(s)
    → srt.prepare → captions.generate_ass
    → broll.director (or broll.fetch + broll.assign per-segment)
    → music.search → music.assign
    → sfx.assign (per editorial role)
    → voiceover.generate (optional)
    → timeline.validate → timeline.render → verify.production

For A2V (audio only):
  transcribe → srt.prepare
    → timeline.build (from audio duration)
    → timeline.add_segment(s)
    → captions.generate_ass
    → broll.fetch → background.assign
    → music.search → music.assign
    → sfx.assign
    → timeline.validate → timeline.render → verify.production
```

### 4.2 New/Upgraded Atomic Tools

| Tool | Type | Description |
|------|------|-------------|
| `captions.generate_ass` | **NEW** | Generate ASS from word-level SRT with crossfade timestamp remapping. Input: srt_path, spec (font/size/style/position), width, height, crossfade_ms, output_timeline_path. Output: ass_path. Replaces inline ASS generation in both handlers. |
| `timeline.from_audio` | **NEW** | Create a timeline from audio file duration. Input: audio_path, aspect, fps, max_duration. Output: timeline_path. Replaces the inline duration analysis + timeline construction in audio.to_video. |
| `background.assign` | **EXISTS** | Already exists but not wired into V2V/A2V handlers. Needs verification that it works with audio-only timelines. |

### 4.3 Refactored Handlers (Thin Wrappers)

After the atomic tools are extracted, both `reelize.timeline` and `audio.to_video` become thin wrappers or **deprecated in favor of agent orchestration**.

**Option A: Keep as convenience wrappers** — Both tools call the same atomic tools internally but still make default decisions. Useful for one-click operation when the agent doesn't need fine control.

**Option B: Deprecate and remove** — Replace with documentation showing the agentic trajectory. The agent calls atomic tools directly. This is more aligned with the AGENTS.md philosophy of agent-directed orchestration.

**Recommendation: Option A with Option B as documentation** — Keep both as "fast path" orchestrators that default to sensible choices, but document the agentic trajectory so agents can bypass them when they need custom decisions.

---

## 5. Implementation Plan

### Phase 1: Extract Atomic Tools (P0 — Unblocks everything)

**Step 1.1: Create `captions.generate_ass` tool**
- Extract the ASS generation + word timing + crossfade remapping logic from both handlers
- Place in `tools.rs` as a new tool definition + handler
- Input: `srt_path`, `spec` (style/font/font_size/color/highlight_color/position/safe_zone/max_words_per_line), `width`, `height`, `crossfade_ms` (optional, for V2V timestamp remapping)
- Output: `ass_path`, `segment_count`
- The handler internally:
  1. Parses word-level SRT
  2. Groups with `group_entries_with_words`
  3. If `crossfade_ms` is provided: remaps timestamps for xfade offsets
  4. Generates ASS with `generate_ass` from `openscript_core::captions`
  5. Writes to disk
  6. Returns `ass_path`

**Step 1.2: Verify `sfx.assign` works standalone**
- The tool already exists. Verify it can be called independently to add SFX events to a timeline.
- No code changes needed, just verification.

**Step 1.3: Verify `music.assign` works standalone**
- The tool already exists. Verify it can be called independently.
- No code changes needed, just verification.

**Step 1.4: Verify `timeline.validate` → `timeline.render` pipeline**
- Both tools exist. Verify they work end-to-end.
- No code changes needed, just verification.

### Phase 2: Refactor Handlers to Use Atomic Tools (P1 — Clean up monoliths)

**Step 2.1: Refactor `handle_reelize_timeline`**
- Replace inline ASS generation with `captions.generate_ass` call
- Replace inline SFX event creation with `sfx.assign` calls
- Replace inline music search/assign with `music.search` + `music.assign` calls
- Add `timeline.validate` call before `render_from_timeline`
- Add `verify.production` call after render
- Target: reduce from 572 lines to ~250 lines

**Step 2.2: Refactor `handle_audio_to_video`**
- Create a proper `Timeline` object instead of building a `MultiLayerRenderSpec` directly
- Use `timeline.build` + `timeline.add_segment` for timeline construction
- Replace inline ASS generation with `captions.generate_ass` call
- Replace inline SFX with `sfx.assign` calls
- Replace inline music with `music.search` + `music.assign` calls
- Switch from `render_multilayer` to `timeline.render` (which uses `render_from_timeline` internally)
- Add `timeline.validate` + `verify.production`
- Target: reduce from 361 lines to ~200 lines

### Phase 3: Documentation & Agent UX (P2 — Agent discoverability)

**Step 3.1: Update AGENT_GUIDE.md**
- Document the agentic V2V trajectory: `reelize.brief → agent decides → timeline.build → ... → timeline.render`
- Document the agentic A2V trajectory: `transcribe → srt.prepare → timeline.build → ... → timeline.render`
- Add `captions.generate_ass` to the tool taxonomy

**Step 3.2: Update server.rs instructions string**
- Add V2V and A2V trajectories to the agent instructions
- Update tool count (87 → 88 for new `captions.generate_ass`)

**Step 3.3: Update integration tests**
- Add test for `captions.generate_ass`
- Verify existing tests still pass

---

## 6. Risk Assessment

| Risk | Impact | Mitigation |
|------|--------|------------|
| Breaking `script.to_video` golden path | **HIGH** | Phase 1 only adds new tools. Phase 2 refactors V2V/A2V only. `script.to_video` is untouched. |
| `timeline.render` behaves differently from `render_multilayer` for A2V | **MEDIUM** | `audio.to_video` currently uses `render_multilayer` with a `MultiLayerRenderSpec`. Switching to `timeline.render` requires verifying that the timeline-based render produces equivalent output. May need a flag. |
| Breaking existing integration tests | **LOW** | Existing tests call tools via MCP protocol. New tools are additions, not modifications. |
| `captions.generate_ass` crossfade remapping edge cases | **MEDIUM** | The remapping logic is already battle-tested in `handle_reelize_timeline`. Extracting it shouldn't introduce new bugs, but needs careful testing with various crossfade values. |

---

## 7. Success Criteria

1. **`captions.generate_ass` tool exists and works** — generates ASS with word-level timing from word-level SRT
2. **Both handlers refactored** — use atomic tools instead of inline logic
3. **All existing tests pass** — no regressions in `script.to_video` or other tools
4. **New integration test** for `captions.generate_ass`
5. **Documentation updated** — AGENT_GUIDE.md and server.rs reflect the agentic V2V/A2V trajectories
6. **Agent can orchestrate V2V/A2V manually** — by calling atomic tools in sequence without using the monolithic handlers

---

## 8. Comparison: Before vs After

### Before (Monolithic)

```
Agent calls reelize.timeline(video_path="clip.mp4")
  → Rust decides everything internally
  → Agent gets: {output_path, warnings}
  → Agent has NO control over b-roll, music, SFX, captions
```

### After (Agentic)

```
Agent calls reelize.brief(video_path="clip.mp4")
  → Agent reads the brief, decides:
    - Which segments to keep
    - What b-roll to add
    - What music mood to use
    - What SFX to place where
    - Whether to add voiceover

Agent calls:
  timeline.build(source_video="clip.mp4")
  timeline.add_segment(timeline_path=..., start=0.5, end=3.2, caption="...")
  timeline.add_segment(timeline_path=..., start=5.1, end=8.7, caption="...")
  srt.prepare(srt_path=...)
  captions.generate_ass(srt_path=..., spec={...}, crossfade_ms=300)
  broll.director(timeline_path=..., orientation="9:16")
  music.search(query="upbeat", mood="energetic")
  music.assign(timeline_path=..., path="music.mp3", ducking=true)
  sfx.assign(timeline_path=..., editorial_role="hook", position_ms=0)
  sfx.assign(timeline_path=..., editorial_role="transition", position_ms=3200)
  timeline.validate(timeline_path=...)
  timeline.render(timeline_path=...)
  verify.production(video_path=..., timeline_path=...)
  → Agent gets full control at every step
```

### Or Agent Uses Convenience Wrapper (Same as Before)

```
Agent calls reelize.timeline(video_path="clip.mp4", music={mood:"upbeat"}, sfx={enabled:true})
  → Internally calls the same atomic tools with sensible defaults
  → Agent gets: {output_path, warnings}
```
