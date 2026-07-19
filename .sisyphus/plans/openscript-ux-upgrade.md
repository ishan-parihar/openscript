# OpenScript MCP UX Upgrade Plan — End User Failure Response

**Date:** April 11, 2026  
**Source:** `.sisyphus/feedback/end-user-failure-analysis.md`  
**Scope:** Fix critical bugs, close implementation gaps, improve end-user experience for AI agents directing video editing via MCP tools.

---

## Problem Statement

The OpenScript MCP system is architecturally sound (multi-track timeline, 43 tools, verification layer) but produced a suboptimal 46.5s output when the user expected a polished <90s reel with chapters, motion graphics, music, SFX, and captions. The attempt required 20+ manual workarounds. The failures cluster into **8 categories** with **32 actionable items** (8 P0, 12 P1, 8 P2, 4 P3).

---

## Phase 1: P0 Bug Fixes — Restore Expected Behavior

*Goal: Every tool should do what its description says. No silent failures.*

### 1.1 Fix `overlay.generate` — Remove "retimed" subcommand bug

**File:** `crates/openscript-mcp/src/tools.rs` line ~1450

**Bug:** `.arg("retimed")` is passed as a positional argument to PupCaps CLI, but PupCaps has no subcommands. The string "retimed" is parsed as the `<file>` argument, fails the `.srt` extension check, and crashes with `Error: File should have extension .srt!`.

**Fix:**
```rust
// DELETE this line:
.arg("retimed")
```

**Validation:** Run `overlay.generate` with a test SRT file. Confirm MOV output is generated without errors.

### 1.2 Fix `motion.render` — Respect `duration_in_frames` runtime parameter

> **Note:** `motion.preview` and `motion.validate` already exist in the codebase (`tools.rs` lines 753-765, 859-860). Only the duration bug needs fixing.
>
> **AUDIT FINDING:** The root cause is NOT a Remotion limitation. The Remotion CLI flag is `--duration=N`, NOT `--duration-in-frames=N`. Verified in `/remotion/node_modules/@remotion/renderer/dist/options/override-duration.js` line 7: `const cliFlag = 'duration'`. The Rust code at `renderer.rs:78` sends `--duration-in-frames=90` which Remotion silently ignores (unrecognized flag).

**Fix:** Change the flag name in `crates/openscript-mcp/src/motion/renderer.rs` line 78:

```rust
// BEFORE (buggy):
&format!("--duration-in-frames={}", args.duration_in_frames),

// AFTER (correct):
&format!("--duration={}", args.duration_in_frames),
```

**Validation:** Render a 3s motion graphic (90 frames @ 30fps). Confirm output is ~3s, not 30s.

### 1.3 Add `srt.remap` tool — Remap SRT timestamps for assembled timelines

**Context:** `openscript_core::srt::retime_srt()` already exists (line 136 of `crates/openscript-core/src/srt/mod.rs`). It accepts source SRT entries + EDL segment pairs and outputs retimed entries. `write_srt()` also exists (line 107). What's missing is an MCP tool that wires them together.

**AUDIT FINDING:** Type mismatch between functions. `retime_srt()` returns `Vec<(f64, f64, String)>` (start, end, text) but `write_srt()` expects `&[(String, f64, f64)]` (text, start, end). The handler must transpose tuples:
```rust
let transposed: Vec<(String, f64, f64)> = retimed.iter()
    .map(|(s, e, t)| (t.clone(), *s, *e))
    .collect();
write_srt(&transposed, &output_path)?;
```

**New tool: `srt.remap`**
- **Inputs:** `source_srt_path`, `timeline_path` (or `edl_path`), `output_path` (optional), `gap_merge` (default 0.25)
- **Process:** Parse SRT → extract segments from timeline → call `retime_srt()` → transpose tuples → call `write_srt()`
- **Returns:** `output_path`, `entry_count`, `coverage_percent`

**File changes:**
- `crates/openscript-mcp/src/tools.rs` — Add tool schema + handler `handle_srt_remap()`
- Route in `route_tool()`

### 1.4 Add system.init — Proactive health check

**Context:** `openscript_tts::client::health_check()` already exists. The `reelize.timeline` handler already has inline environment diagnostics at lines 3266-3272 (PEXELS_API_KEY, OPENSCRIPT_TTS_URL checks) but these are inline and only run within that handler.

**AUDIT FINDING:** `voice.profile.list` already calls `health_check()` at tools.rs:1670 and returns a hard error if the model isn't loaded. The problem is that this is reactive (only fails when called) rather than proactive (check at session start).

**Fix:** Add a `system.init` MCP tool that runs all health checks proactively:
1. Pings Voicebox at `OPENSCRIPT_TTS_URL/health` (default `http://127.0.0.1:17493/health`)
2. Lists available voice profiles
3. Checks Pexels API key availability
4. Checks SFX/music index status
5. Checks Apex transcription health

**Returns:** Structured capability report:
```json
{
  "tts": { "available": true, "profiles_count": 3, "model_loaded": true },
  "broll": { "available": true, "api_key_set": true },
  "sfx": { "available": true, "indexed_count": 261 },
  "music": { "available": true, "indexed_count": 16 },
  "transcription": { "available": true, "apex_healthy": true }
}
```

**File changes:**
- `crates/openscript-mcp/src/tools.rs` — Add `system.init` tool schema + handler
- Handler checks all external dependencies and returns capability report

### 1.6 Add `music.verify` tool — Verify music presence and volume

**Context:** Failure analysis Section 4 explicitly requires P0 music verification. No tool currently checks if background music is present and properly mixed.

**New tool: `music.verify`**
- **Inputs:** `video_path`, `expected_mood` (optional), `target_gain_db` (optional, default -12)
- **Process:** Use ffprobe to detect music track presence, measure music loudness separately from dialogue, verify ducking behavior
- **Returns:** `has_music` (boolean), `music_loudness_lufs`, `dialogue_loudness_lufs`, `ducking_detected` (boolean), `quality_score` (0-100)
- **Failure threshold:** If music was expected (per timeline) but `has_music=false` → fail

**File changes:**
- `crates/openscript-mcp/src/tools.rs` — Add tool schema + handler `handle_music_verify()`
- Uses ffprobe for audio stream analysis, ffmpeg for loudness measurement

### 1.7 Fix `sfx.recalculate` to include audio re-mixing during concatenation

**Context:** SFX events exist in timeline JSON but are never rendered into the final audio when using manual concatenation. The failure analysis requires BOTH timestamp recalculation AND audio re-mixing.

**Fix:** The `sfx.recalculate` tool must:
1. Recalculate SFX positions based on assembled timeline (timestamp remap)
2. Generate a mixed audio file that includes SFX at the correct positions
3. Return both the updated timeline AND the mixed audio path

**Implementation:** Use ffmpeg's `amix` filter to layer SFX onto the dialogue/music mix at recalculated timestamps.

**Validation:** Assign 5 SFX events, recalculate for an assembled timeline, render output, confirm all 5 SFX are audible via `verify.audio`.

### 1.5 Fix verification tools — Compare against assembled output, not source timeline

**Current behavior:** `verify.render` compares the rendered video against the source timeline. When the user manually assembles a custom reel (e.g., 46.5s from a 145s source), it reports a 98s delta as a "warning."

**Fix:**
- Add `assembled_timeline_path` optional parameter to `verify.render`
- If provided, compare against the assembled timeline instead of the source
- If not provided, keep existing behavior but add a clear warning: "Comparing against source timeline — if you assembled a custom reel, pass assembled_timeline_path for accurate verification"

**Similarly for `verify.captions`:**
- Accept an assembled SRT path alongside the source SRT
- Compare caption coverage against the actual video duration, not the source duration

**File changes:**
- `crates/openscript-mcp/src/tools.rs` — Modify `handle_verify_render()` and `handle_verify_captions()` schemas and handlers

---

## Phase 2: P1 Quality Gates — Catch Issues Before They Compound

*Goal: Stop the pipeline at the first sign of trouble. No cascading failures.*

### 2.1 Add intermediate quality gates after each major step

**New mechanism:** Inline gate checks within `reelize.timeline` handler (NOT a separate MCP tool — avoids tool sprawl). Add a `quality_gates: boolean` parameter to `reelize.timeline` (default `true`).

**Gate behavior:** 
- **Hard fail** (abort pipeline, return structured error): Transcription <10 words, 0 SRT entries, 0 timeline segments, render output 0 bytes, caption coverage <80%
- **Warn** (continue but flag in final response): 0 music tracks, 0 SFX events, duration <50% of target
- **Override:** Allow `quality_gates_override: ["captions", "music"]` array to skip specific gates

**Gates:**
| Step | Gate Check | Threshold | Behavior |
|------|-----------|-----------|----------|
| Transcription | Word count | <10 words | FAIL |
| SRT prepare | Entry count | 0 entries | FAIL |
| Timeline build | Segment count | 0 segments | FAIL |
| Music assign | Track count | 0 tracks | WARN |
| SFX assign | Event count | 0 events | WARN |
| Render | File size | 0 bytes | FAIL |
| Captions | Coverage % | <80% | FAIL |
| Final output | Duration vs target | <50% of target | WARN |

**Implementation:** Add `run_quality_gate()` helper function in `tools.rs` that takes step name, metric, threshold, and fail/warn behavior. Call after each pipeline step. Returns `GateResult { passed: bool, metric_value, threshold, severity: "fail"|"warn", message }`.

### 2.2 Add output quality contract

**New mechanism:** Define minimum quality thresholds that must be met before declaring success.

**Quality contract (hard failures):**
- Caption coverage >80%
- Audio loudness -14 ±2 LUFS
- No silent gaps >1s (except intentional)
- Resolution 1080×1920
- Duration within ±10% of target (if target specified)

**Implementation:** Add `verify.quality_contract` tool that runs all checks and returns a pass/fail report. Integrate as the final step of `reelize.timeline`.

### 2.3 Add `sfx.recalculate` — Recalculate SFX positions for assembled timelines

> **Note:** Audio re-mixing during concatenation was moved to 1.7. This tool handles timestamp remapping only.

**Problem:** SFX are assigned at source timeline positions. When segments are re-ordered or title cards inserted, SFX positions become invalid.

**New tool: `sfx.recalculate`**
- **Inputs:** `source_timeline_path`, `assembled_timeline_path` (EDL v2 format), `output_path` (optional)
- **Process:** 
  1. Load source timeline's SFX track events
  2. For each SFX event, determine which source segment it belonged to
  3. Map to the corresponding position in the assembled timeline using segment start offsets
  4. Write recalibrated events to output timeline
- **Returns:** `output_timeline_path`, `recalculated_events` (count), `unmapped_events` (events that couldn't be mapped)

**File changes:**
- `crates/openscript-mcp/src/tools.rs` — Add tool schema + handler `handle_sfx_recalculate()`
- Uses existing EDL v2 timeline schema from `openscript-core`

### 2.4 Add `audio.concat` tool — Video+audio concatenation with codec normalization

**Problem:** Remotion-rendered title cards have AAC audio incompatible with source content codec. The ffmpeg concat demuxer fails. User had to manually separate video and audio pipelines.

**New tool: `audio.concat`**
- **Inputs:** `clips` (array of `{video_path, audio_path, duration_ms}`), `output_path` (optional), `normalize_codecs` (boolean, default true)
- **Process:**
  1. If `normalize_codecs=true`: Run each clip through `ffmpeg -i input -c:v libx264 -c:a pcm_s16le -y normalized.mp4` first
  2. Detect codec mismatches via ffprobe before attempting concat (skip normalization if all clips match)
  3. Concatenate video with filter_complex: `ffmpeg -i clip1 -i clip2 ... -filter_complex "[0:v][1:v]...concat=n=N:v=1:a=0[outv]" -map "[outv]" video.mp4`
  4. Concatenate audio separately: `ffmpeg -i clip1 -i clip2 ... -filter_complex "[0:a][1:a]...concat=n=N:v=0:a=1[outa]" -map "[outa]" audio.wav`
  5. Mux together: `ffmpeg -i video.mp4 -i audio.wav -c:v copy -c:a aac output.mp4`
- **Returns:** `output_path`, `duration_ms`, `clips_concatenated` (count), `codec_normalization_applied` (boolean)

**File changes:**
- `crates/openscript-ffmpeg/src/render.rs` — Add `concat_clips()` function with codec normalization
- `crates/openscript-mcp/src/tools.rs` — Add tool schema + handler `handle_audio_concat()`

**Validation:** Concatenate 3 Remotion title cards + 2 source clips with mixed codecs. Confirm output plays seamlessly with no codec errors.

### 2.5 Add Kokoro preset voice fallback

**Problem:** When Voicebox is unavailable, no fallback exists.

**Fix:** When `voice.profile.list` returns empty or fails, automatically suggest Kokoro preset voices via `voice.profile.list_presets` with `engine=kokoro`. Include available preset voice names in the error response.

### 2.6 Add state tracking for multi-step pipelines

**New mechanism:** `pipeline.state` tool — file-based state with session-scoped isolation.

**Storage:** State files stored in `/tmp/openscript-state-{pipeline_id}.json` (session-scoped via UUID). No file locking needed — each pipeline has a unique ID. Cleanup on server restart (temp directory).

- **Create state:** `pipeline.state` action=create → returns state file path + pipeline_id
- **Update state:** `pipeline.state` action=update, state_file=..., step=..., status=..., output={...}
- **Query state:** `pipeline.state` action=query, state_file=... → returns completed/remaining steps
- **Cleanup:** `pipeline.state` action=cleanup, state_file=... → deletes state file

**State schema:**
```json
{
  "pipeline_id": "uuid",
  "created_at": "ISO8601",
  "video_path": "...",
  "steps": {
    "transcribe": { "status": "done", "output": { "srt_path": "..." } },
    "srt_prepare": { "status": "done", "output": { "grouped_srt": "..." } },
    "timeline_build": { "status": "done", "output": { "timeline_path": "..." } },
    "broll": { "status": "pending" },
    "music": { "status": "pending" },
    "sfx": { "status": "pending" },
    "voiceover": { "status": "pending" },
    "render": { "status": "pending" },
    "verify": { "status": "pending" }
  }
}
```

---

## Phase 3: P2 Enhancements — Polish and Preview

*Goal: Make the system more usable and transparent.*

### 3.1 Add visual caption preview

**New tool: `verify.captions_preview`**
- Extracts 3-5 frames from the rendered video at caption positions
- Returns base64-encoded PNGs or file paths
- Allows the AI agent (or user) to verify caption positioning and styling without watching the full video

### 3.2 Add SFX/music preview (audio-only)

**New tool: `audio.preview_mix`**
- Generates a short (5-10s) audio-only preview of the mixed timeline
- Includes dialogue, voiceover, music with ducking, and SFX
- Returns WAV file path
- Much faster than full video render for verifying audio mix

### 3.3 Add pipeline templates

**Pre-configured workflows:**
- `"basic_reel"` — Transcription + timeline + captions + render
- `"with_broll"` — basic_reel + b-roll director
- `"with_voiceover"` — basic_reel + TTS commentary
- `"with_title_cards"` — timeline + motion graphics + concat
- `"full_production"` — All tracks enabled

Each template defines which steps to run, their order, and default parameters.

### 3.4 Add font availability check before caption generation

**Before:** `overlay.generate` and ASS caption burning assume Bebas Neue is available.
**After:** Check `mcp/fonts/BebasNeue-Regular.ttf` exists before proceeding. If missing, return a clear error with instructions.

---

## Phase 4: P3 Nice-to-Haves

### 4.1 Add pipeline status endpoint
- `pipeline.status` — returns progress percentage, current step, ETA

### 4.2 Add SFX timing visualization
- Visual representation of SFX placement on the timeline (ASCII or JSON)

### 4.3 Add concat failure auto-recovery
- When ffmpeg concat fails, automatically try alternative approaches (filter_complex, separate audio/video concat, etc.)

### 4.4 Add music extension under title card gaps
- When title cards create silent gaps, automatically extend background music to fill them

---

## Already Implemented — No Work Needed

The following items were listed in the failure analysis as needed but **already exist in the codebase**:

| Feature | Location | Notes |
|---------|----------|-------|
| `motion.preview` | `tools.rs` lines 765, 860, 4739 | Renders single frame as PNG — already works |
| `motion.validate` | `tools.rs` lines 753, 859, 4716 | Validates TSX before render — already works |
| `motion.compile_check` | `tools.rs` lines 779, 861 | TypeScript compilation check — already works |
| `retime_srt()` | `srt/mod.rs` line 136 | Core function exists; just needs MCP tool wrapper |
| `write_srt()` | `srt/mod.rs` line 107 | SRT serializer exists — no need to build |
| `health_check()` | `client.rs` line 126 | Voicebox health check exists — just needs proactive calling |

---

## Implementation Order

| Phase | Items | Est. Effort | Dependencies |
|-------|-------|-------------|-------------|
| **Phase 1** (P0 bugs) | 1.1, 1.2, 1.3, 1.4, 1.5, 1.6, 1.7 | 2-3 days | None — independent fixes. 1.1 and 1.2 are one-line changes |
| **Phase 2** (Quality gates) | 2.1, 2.2, 2.3, 2.4, 2.5, 2.6 | 5-6 days | Phase 1 complete |
| **Phase 3** (Polish) | 3.1, 3.2, 3.3, 3.4 | 2-3 days | Phase 1.5 (verification fixes) complete |
| **Phase 4** (Nice-to-have) | 4.1-4.4 | 1-2 days | Phase 2 complete |

---

## Tool Inventory — New vs Modified

| Tool | Action | Phase | File(s) Changed |
|------|--------|-------|-----------------|
| `overlay.generate` | **Fix** (remove "retimed" arg, line 1450) | 1.1 | `tools.rs` |
| `motion.render` | **Fix** (change `--duration-in-frames=` to `--duration=`) | 1.2 | `renderer.rs:78` |
| `srt.remap` | **New** (uses existing `retime_srt` + `write_srt`, transpose tuples) | 1.3 | `tools.rs` |
| `system.init` | **New** (proactive health check) | 1.4 | `tools.rs` |
| `verify.render` | **Modify** (add `assembled_timeline_path` param) | 1.5 | `tools.rs` |
| `verify.captions` | **Modify** (add `assembled_srt_path` param) | 1.5 | `tools.rs` |
| `music.verify` | **New** (ffprobe-based music detection) | 1.6 | `tools.rs` |
| `sfx.recalculate` | **New** (timestamp remap + audio re-mix) | 1.7 | `tools.rs`, `openscript-ffmpeg` |
| Quality gates | **Inline** (helper in `reelize.timeline` handler) | 2.1 | `tools.rs` |
| `verify.quality_contract` | **New** | 2.2 | `tools.rs` |
| `sfx.recalculate` | **New** (timestamp-only variant) | 2.3 | `tools.rs` |
| `audio.concat` | **New** (codec normalization + concat) | 2.4 | `tools.rs`, `openscript-ffmpeg/render.rs` |
| `voice.profile.list` | **Modify** (suggest Kokoro presets on failure) | 2.5 | `tools.rs` |
| `pipeline.state` | **New** | 2.6 | `tools.rs`, new `pipeline_state.rs` |
| `verify.captions_preview` | **New** | 3.1 | `tools.rs`, `openscript-ffmpeg` |
| `audio.preview_mix` | **New** | 3.2 | `tools.rs`, `openscript-ffmpeg` |
| Pipeline templates | **New** (Rust enums, not config files) | 3.3 | `tools.rs` |
| Font check | **Add** to existing tools | 3.4 | `tools.rs` |

---

## Risk Assessment

| Risk | Impact | Mitigation |
|------|--------|-----------|
| `retime_srt()` type mismatch not handled | Medium — SRT write fails silently | Handler must transpose tuples before calling `write_srt()` |
| `srt.remap` produces empty output | Medium — no captions for assembled timeline | Add validation: if `entry_count == 0`, return error with diagnostic |
| New tools increase MCP server complexity | Medium — `tools.rs` is ~4800 lines | Keep each tool self-contained; consider extracting handlers to separate modules |
| Voicebox health check adds startup latency | Low — async check | Run health check in parallel with other init |
| Quality gates blocking legitimate edge cases | Medium — e.g., valid 10s reel with 70% captions fails | Add `quality_gates_override` array parameter |
| Codec normalization doubles render time | Medium — every clip needs ffmpeg pass | Detect codec mismatch via ffprobe first; normalize only when needed |
| `system.init` false positives | Low — health check may fail due to transient network issues | Retry with exponential backoff (3 attempts, 1s interval) |

---

## Success Criteria

After all phases are complete, verify against the Tool Inventory (above) as the checklist:

1. **Bug-free core tools:** `overlay.generate` produces MOV output; `motion.render` respects duration; `srt.remap` produces valid remapped SRT
2. **Health visibility:** `system.init` reports all external dependencies (Voicebox, Pexels, SFX/music index) before pipeline starts
3. **Accurate verification:** `verify.render` and `verify.captions` compare against the actual assembled output, not the source timeline
4. **Quality gates catch failures:** `reelize.timeline` with `quality_gates=true` aborts on transcription failure, empty SRT, or <80% caption coverage
5. **Quality contract enforced:** `verify.quality_contract` returns pass/fail for all 5 thresholds (caption coverage, audio loudness, silent gaps, resolution, duration)
6. **Music verified:** `music.verify` detects missing music, wrong volume, or absent ducking
7. **SFX survive assembly:** `sfx.recalculate` produces recalibrated SFX positions AND mixed audio output
8. **Codec-normalized concat:** `audio.concat` handles mixed codecs without manual ffmpeg workarounds
9. **Fallback paths exist:** When Voicebox is unavailable, Kokoro presets are offered; when b-roll API key missing, pipeline continues with warnings
10. **State tracked:** `pipeline.state` provides progress visibility across multi-step workflows
