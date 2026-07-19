# OpenScript MCP Upgrade Plan — Post-Test Report Fixes

> **Generated from**: End-user AI-agent UX test report (43 tools tested)
> **Test results**: 21 PASS, 2 FAIL (P1+P2), 20 TIMEOUT, ~10 untested

---

## Executive Summary

The test report reveals a codebase with a **solid core** (transcription → timeline → asset indexing pipeline works end-to-end) but **critical gaps in the production rendering layer** and **UX deficiencies for AI agent consumers**. The 20 timeouts are not individual bugs — they're a systemic resource exhaustion pattern. The 2 FAIL items are concrete bugs requiring targeted fixes.

**Root cause of 20/20 timeouts**: All involve either (a) FFmpeg video rendering on a 227s source, (b) Voicebox TTS calls requiring model load, or (c) external API calls (Pexels). The MCP server has no async job pattern — it blocks until completion, then times out.

---

## Phase 1: P1 Bug — `overlay.generate` PupCaps SRT Extension Validation

### Problem

PupCaps `assertFileExtension('.srt')` in `third_party/PupCaps/src/script/cli.ts:44` uses `value.endsWith('.srt')`. The MCP handler at `tools.rs:1427-1492` passes `srt_path` as the first positional arg to the PupCaps CLI, but:

1. **No path sanitization**: Unlike every other tool handler, `handle_overlay_generate` does NOT call `sanitize_input_path()` on `srt_path` or `edl_path`. This means relative paths, malformed paths, or paths with trailing whitespace may be passed directly.
2. **No file existence check**: The handler doesn't verify the SRT file exists before invoking PupCaps.
3. **Style path is relative**: `--style` uses `mcp/styles/{style}.css` (relative), which fails if MCP server CWD ≠ project root.

### Fix

**File**: `crates/openscript-mcp/src/tools.rs` — `handle_overlay_generate`

1. Add `sanitize_input_path()` call on `srt_path` and `edl_path` (consistent with every other handler)
2. Add `Path::new(&srt_path).exists()` check before invoking PupCaps
3. Resolve the `--style` path to an absolute path using `std::env::current_dir().join("mcp/styles/{style}.css")`
4. Capture and surface PupCaps stderr in error messages (currently the error message from ToolError::Ffmpeg may not include the actual PupCaps error)
5. Add timeout handling: wrap the `cmd.output()` in a `tokio::time::timeout()` (default 120s for overlay generation)

### Files Changed
- `crates/openscript-mcp/src/tools.rs` (handler fix)

---

## Phase 2: P2 Bug — `voice.profile.list` Voicebox Model Not Loaded

### Problem

The handler at `tools.rs:1655-1705` calls `client.health_check()` and returns a clean `ToolError::Tts` when `model_loaded == false`. However, **every other TTS tool** (`tts.generate`, `tts.commentary`, `voiceover.generate`, `tts.preview`) does the same check. When Voicebox is down, ALL 10 TTS-dependent tools fail — the test report shows 20 timeouts, many of which are TTS tools that hang waiting for the Voicebox connection.

### Fix

**File**: `crates/openscript-tts/src/client.rs`

1. **Add connection timeout to health_check**: The current `health_check()` likely has a long or default timeout. Add a 5-second connection timeout so failures are fast-fail, not slow-hang.
2. **Add health check caching**: Cache the health check result for 30 seconds. If Voicebox is down, don't re-check on every tool call — fail fast for 30s, then re-check.

**File**: `crates/openscript-mcp/src/tools.rs`

1. **Add fallback TTS engine support**: When Voicebox is unavailable, `tts.estimate_duration` already works (it's pure math — no network). Document that `tts.preview` and `tts.estimate_duration` are safe fallbacks.
2. **Improve error clarity**: When `model_loaded == false`, return a structured error with actionable next steps (current error is good but could include the voicebox URL being checked).

### Files Changed
- `crates/openscript-tts/src/client.rs` (health check timeout + caching)
- `crates/openscript-mcp/src/tools.rs` (error improvement)

---

## Phase 3: Timeout Fix — Async Job Pattern for Long-Running Operations

### Problem

20 tools timeout because they block synchronously on FFmpeg renders, TTS generation, and external API calls. The MCP server uses stdio transport with no built-in timeout management — when an operation takes > MCP client timeout (typically 60-120s), the client disconnects.

**Timeout categories**:
- **Video-heavy (FFmpeg)**: `reelize`, `reelize.timeline`, `reelize.direct`, `timeline.render`, `render`, `srt.apply_edit` — all invoke FFmpeg on potentially long videos
- **TTS-dependent**: `tts.generate`, `tts.preview`, `tts.commentary`, `voiceover.generate`, `voice.profile.*` — all require Voicebox HTTP calls
- **External API**: `broll.fetch`, `broll.director`, `broll.assign` — Pexels API calls
- **Motion**: `motion.render`, `motion.preview`, `motion.compile_check`, `motion.validate`, `motion.design_system` — Remotion compilation + rendering

### Fix Strategy: Three-Tier Approach

#### Tier 1: Add Timeouts to All External Calls (Quick Win)

**File**: `crates/openscript-tts/src/client.rs`
- `health_check()`: Add 5s connection timeout
- `generate()`: Already has 60s polling timeout — extend to 300s for long TTS
- `list_profiles()`, `list_preset_voices()`, `get_profile()`, `add_profile_sample()`: Add 30s timeout each

**File**: `crates/openscript-assets/src/pexels.rs`
- `search()`, `download_best()`, `search_for_slot()`: Add 30s HTTP timeout

**File**: `crates/openscript-mcp/src/tools.rs`
- Wrap `handle_overlay_generate` in `tokio::time::timeout(Duration::from_secs(120), ...)`
- Wrap `handle_timeline_render` in `tokio::time::timeout(Duration::from_secs(600), ...)` for long videos
- Wrap `handle_reelize`, `handle_reelize_timeline`, `handle_reelize_direct` in `tokio::time::timeout(Duration::from_secs(600), ...)`
- Wrap `handle_render`, `handle_srt_apply_edit` in `tokio::time::timeout(Duration::from_secs(300), ...)`

#### Tier 2: Improve Progress Reporting for Long Operations

Currently, `report_progress()` is called at coarse milestones (e.g., 0%, 20%, 100%). For operations taking minutes, the MCP client needs more frequent updates to avoid timeout.

**File**: `crates/openscript-ffmpeg/src/render.rs`
- In `spawn_ffmpeg_with_progress()`, emit progress notifications every 5% instead of relying on FFmpeg's own sparse output
- Pass a progress callback to report real-time percentage

**File**: `crates/openscript-mcp/src/server.rs`
- Ensure `report_progress()` resets the MCP client timeout on each call (already implemented per line 61 comment — verify it works)

#### Tier 3: Add `max_duration` Caps to Prevent Resource Exhaustion

Many tools accept `max_duration` but the implementation may not enforce it strictly for renders.

**File**: `crates/openscript-mcp/src/tools.rs`
- `handle_reelize`: Enforce `max_duration` cap before FFmpeg render (skip segments beyond cap)
- `handle_reelize_timeline`: Same enforcement
- `handle_timeline_build`: Already respects `max_duration` (confirmed)
- `handle_edl.build`: Already respects `max_duration` (confirmed)

### Files Changed
- `crates/openscript-tts/src/client.rs` (timeouts on all HTTP calls)
- `crates/openscript-assets/src/pexels.rs` (HTTP timeouts)
- `crates/openscript-mcp/src/tools.rs` (timeout wrappers + max_duration enforcement)
- `crates/openscript-ffmpeg/src/render.rs` (finer-grained progress reporting)

---

## Phase 4: AI Agent UX Improvements (from report findings)

### 4.1: `sfx.assign` Silent Failure When No Matching Asset

**Problem** (report item #8): When no matching SFX exists, `sfx.assign` returns `asset_path: null` without any warning. AI agents won't know the SFX wasn't actually placed.

**Fix**: 
- Return `"warning": "No matching SFX found for role '{role}'. Event created with placeholder."` in the response JSON
- Add a `matched: boolean` field to the response
- File: `crates/openscript-mcp/src/tools.rs` — `handle_sfx_assign`

### 4.2: `music.ducking.plan` Empty Result Clarity

**Problem** (report item #7): Returns empty array when no dialogue/voiceover events exist. This is expected behavior but confusing for agents.

**Fix**:
- Add `"note": "No dialogue or voiceover events found on timeline. Add segments first for ducking to be calculated."` to the response
- File: `crates/openscript-mcp/src/tools.rs` — `handle_music_ducking_plan`

### 4.3: `broll.assign` Placeholder Path Handling

**Problem**: The `broll.assign` handler (line 2347-2364) resolves paths from cache but falls back to a glob pattern string if no file is found. The resolved path may not exist, but the event is still created with `asset_id: "placeholder"`.

**Fix**:
- Return `"warning": "B-roll asset not found, using placeholder"` when path doesn't exist
- File: `crates/openscript-mcp/src/tools.rs` — `handle_broll_assign`

### 4.4: Motion Tools Timeout (No Node.js/Remotion Availability Check)

**Problem**: `motion.compile_check`, `motion.validate`, `motion.preview`, `motion.render`, `motion.design_system` all timeout. The `motion.get_info` tool works (PASS), proving Remotion is installed. The timeouts are likely due to:
1. Long compilation times for TypeScript
2. Remotion render taking > default MCP timeout
3. `motion.design_system` is actually a pure computation tool — it shouldn't timeout at all (no external calls)

**Fix for `motion.design_system`**:
- This tool is implemented at `motion/design_system.rs` and does NOT call external services. The timeout is likely because the handler is being routed through the wrong code path or the handler itself has a hidden blocking call.
- Verify the handler implementation doesn't spawn external processes.

**Fix for `motion.compile_check`, `motion.validate`, `motion.preview`, `motion.render`**:
- These all spawn `npx tsc` or `npx remotion render` subprocesses
- Add `tokio::time::timeout` wrappers (60s for compile_check/validate, 300s for preview, 600s for render)
- Add progress reporting during render (parse Remotion CLI output)

### Files Changed
- `crates/openscript-mcp/src/tools.rs` (4.1, 4.2, 4.3, 4.4)
- `crates/openscript-mcp/src/motion/design_system.rs` (investigate timeout cause)

---

## Phase 5: Documentation / Tool Description Fixes

### 5.1: Tool descriptions mention fallback behavior that doesn't exist

Several tool descriptions reference "documented fallback" behavior for when Voicebox is down. Ensure these descriptions accurately reflect the current state:
- `tts.preview` — works without Voicebox (pure math) ✅
- `tts.estimate_duration` — works without Voicebox (pure math) ✅
- `voice.profile.list_presets` — requires Voicebox ⚠️

### 5.2: Add `requires_voicebox` metadata to TTS tool definitions

Add to the tool schema a `"requires_voicebox": true` field for tools that depend on the Voicebox server, so agents can check health before attempting.

### Files Changed
- `crates/openscript-mcp/src/tools.rs` (tool definitions)

---

## Implementation Order (Priority)

| Priority | Phase | Effort | Impact |
|----------|-------|--------|--------|
| **P0** | Phase 1: `overlay.generate` PupCaps bug fix | 1 hour | Fixes P1 — animated captions broken |
| **P0** | Phase 3.1: Add HTTP timeouts to TTS client | 2 hours | Fixes 8+ timeout failures |
| **P1** | Phase 2: Voicebox health check improvements | 1 hour | Faster failure, clearer errors |
| **P1** | Phase 3.2: FFmpeg progress reporting | 2 hours | Prevents render timeouts |
| **P1** | Phase 4.1-4.3: Agent UX warnings | 1 hour | Prevents silent failures |
| **P2** | Phase 3.3: max_duration caps | 1 hour | Prevents resource exhaustion |
| **P2** | Phase 4.4: Motion tool timeouts | 2 hours | Fixes 5+ timeout failures |
| **P2** | Phase 5: Documentation updates | 1 hour | Better agent discoverability |

**Total estimated effort**: ~11 hours

---

## Verification Plan

After each phase, re-run the test suite against the same 227s video:

1. **Phase 1**: `overlay.generate` should PASS — animated captions work
2. **Phase 2**: `voice.profile.list` returns structured error when model not loaded
3. **Phase 3**: All 20 timeout tools either PASS or return structured error within timeout
4. **Phase 4**: Agent UX improvements visible in JSON responses (warnings, matched fields)
5. **Phase 5**: Tool descriptions updated with accurate dependency info
