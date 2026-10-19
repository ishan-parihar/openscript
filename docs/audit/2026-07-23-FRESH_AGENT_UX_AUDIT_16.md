# FRESH AGENT UX AUDIT #16 — July 23, 2026

## Audit Methodology

A "fresh agent" simulation was deployed — the agent was given ONLY:
- The MCP server binary location (`target/release/mcp-server`)
- A task: "Create a video using audio.to_video and reelize.timeline"
- No prior knowledge of the codebase, tool schemas, or pipeline internals

The agent interacted exclusively via JSON-RPC 2.0 over stdin/stdout, exactly as an AI agent would when connecting to the MCP server.

## Simulation Results

### Pipeline 1: `audio.to_video` (A2V — Audio to Video)

| Metric | Result |
|--------|--------|
| Status | **RENDERED** (after bug fix) |
| Input | `artifacts/speech_test.wav` (5s, 16kHz mono PCM) |
| Output | `/tmp/audio_to_video_1784766611.mp4` |
| Duration | 5.0 seconds |
| Segments | 2 |
| B-roll | Skipped (PEXELS_API_KEY not set in test env) |
| Music | Skipped |
| SFX | Assigned |
| Captions | Generated (ASS) |

**CRITICAL BUG FOUND & FIXED:** The pipeline initially failed with:
```
SRT parse failed: IO error: No such file or directory (os error 2)
```
Root cause: `handle_audio_to_video` read the SRT file content with `read_to_string`, then passed the **content string** to `parse_srt()`, which expects a **file path** (`AsRef<Path>`). The function tried to open the SRT text content as a filename, which obviously doesn't exist.

**Fix (Phase 37a):** Removed the redundant `read_to_string` and passed `grouped_srt_path` directly to `parse_srt`. 5 lines removed, 2 added. Committed as `83852d2`.

### Pipeline 2: `reelize.timeline` (V2V — Video to Video)

| Metric | Result |
|--------|--------|
| Status | **RENDERED** |
| Input | `artifacts/black_holes_reel.mp4` (13.4MB, ~68s source) |
| Output | `artifacts/black_holes_reel.reel.mp4` (18MB) |
| Duration | 68.07 seconds |
| Resolution | 1080x1920 (9:16, H.264 High, 30fps) |
| Segments | 15 |
| B-roll | 20 clips fetched from Pexels |
| Music | 1 track assigned |
| SFX | 12 hits (hook, transitions, highlights) |
| Captions | ASS with Bebas Neue font |
| Tracks rendered | 48 |
| Warnings | Voiceover unavailable (OPENSCRIPT_TTS_URL not set) |

## UX Friction Points Discovered

### CRITICAL (P0) — Blocks Pipeline Execution

| # | Issue | Impact | Status |
|---|-------|--------|--------|
| 1 | **`parse_srt` content-vs-path bug** — A2V passed SRT content as file path | Pipeline crash | **FIXED** (Phase 37a) |
| 2 | **`PEXELS_API_KEY` not detected in test env** — `system.capabilities` shows it set, but A2V warns it's missing | Stock backgrounds silently skipped | Investigate env var propagation |
| 3 | **MCP binary goes stale silently** — The release binary at `target/release/mcp-server` was 2 commits behind, missing `audio.to_video` and `reelize.timeline` entirely. No version check or tool-count assertion at startup | Agent calls unknown tool, gets `Unknown tool: audio.to_video` error | Add startup version/tool-count validation |

### HIGH (P1) — Degrades Quality or Agent Trust

| # | Issue | Impact |
|---|-------|--------|
| 4 | **No progress feedback during long renders** — The `report_progress` calls exist but agents can't see them (MCP protocol doesn't stream notifications back to the caller during a `tools/call`) | Agent times out or user thinks it's stuck |
| 5 | **A2V warning about PEXELS_API_KEY is misleading** — `system.capabilities` reports `pexels: true`, but A2V says key is not set. Likely a config path vs env var mismatch | Agent trusts capabilities but then gets warnings |
| 6 | **V2V output goes to CWD, not artifacts/** — `black_holes_reel.reel.mp4` was written to the project root, not `artifacts/` | File discovery is confusing |
| 7 | **No output file validation** — Neither pipeline verifies the rendered file exists, is non-zero, or is playable before returning success | Agent may report success for corrupt/incomplete files |

### MEDIUM (P2) — Agent UX Friction

| # | Issue | Impact |
|---|-------|--------|
| 8 | **`audio.to_video` outputs to `/tmp/`** — Temp path is unpredictable, agent can't reference it later | Agent has to guess output location |
| 9 | **V2V voiceover warning is confusing** — "Voiceover unavailable" when the agent never requested voiceover | Unnecessary warning noise |
| 10 | **No `system.doctor` in the A2V path** — A2V doesn't call `system.doctor` before starting, so it doesn't verify prerequisites | Prerequisite failures surface mid-pipeline |
| 11 | **Tool count mismatch** — Binary reports 89 tools, AGENT_GUIDE says 88, server.rs says 88 | Minor but erodes trust |

### LOW (P3) — Polish & YAGNI

| # | Issue | Impact |
|---|-------|--------|
| 12 | **SFX performance** — A2V and V2V call `handle_sfx_assign` N+1 times (load/save per event) instead of batching | Slower than necessary |
| 13 | **`_burn_captions` dead variable** — Prefixed with `_` but never deleted | YAGNI violation |
| 14 | **Debug log written to `/tmp/`** — `audio_to_video_*.debug.log` is ephemeral | Agent can't read debug logs |

## Architecture Observations

### What Works Well
1. **`reelize.timeline` is a solid one-call pipeline** — Transcribe, group, build timeline, fetch b-roll, assign music/SFX, generate captions, render. All atomic tools called in sequence. This is the golden path for V2V.
2. **`audio.to_video` follows the same pattern** — Transcribe, group, build timeline, fetch backgrounds, assign music/SFX, generate captions, render. Consistent architecture.
3. **`system.capabilities` is the right first call** — Agent correctly discovers what's available before starting.
4. **Tool delegation pattern is correct** — Both pipelines delegate to atomic tools (`handle_transcribe`, `handle_srt_prepare`, `handle_broll_fetch`, etc.) rather than reimplementing logic.
5. **Captions system works** — Both pipelines generate ASS captions with word-level timing.

### What Needs Improvement
1. **Binary staleness is a silent killer** — There's no version or tool-count check at startup. If the binary is stale, agents get cryptic "Unknown tool" errors.
2. **Progress streaming is broken for MCP** — `report_progress` writes to stderr but agents can't see it during a `tools/call`. Need streaming notifications or a progress query endpoint.
3. **Config propagation is inconsistent** — `system.capabilities` reports API keys as set, but individual handlers sometimes can't find them.
4. **Output path management is ad-hoc** — Some tools write to CWD, some to `/tmp/`, some to `artifacts/`. No consistent convention.

## Generated Videos for Testing

| Pipeline | Input | Output | Size |
|----------|-------|--------|------|
| V2V | `artifacts/black_holes_reel.mp4` | `artifacts/black_holes_reel.reel.mp4` | 18MB |
| A2V | `artifacts/speech_test.wav` | `/tmp/audio_to_video_1784766611.mp4` | ~5s |

## Recommended Priority Actions

### Immediate (Next Iteration)
1. **Add MCP startup version check** — Compare binary tool count against expected count, warn if stale
2. **Fix PEXELS_API_KEY propagation** — Ensure env var and config path are consistent
3. **Standardize output paths** — All pipelines should write to `artifacts/` by default

### Short-term (Next 3 Iterations)
4. **Add progress streaming** — Implement MCP `notifications/progress` for long-running tools
5. **Add output file validation** — Verify file exists, is non-zero, and has expected duration before returning success
6. **Batch SFX events** — Load timeline once, add all events, save once
7. **Add `system.doctor` call at pipeline start** — Both A2V and V2V should verify prerequisites before starting

### Medium-term (Roadmap)
8. **Clean up warnings** — Remove unnecessary "voiceover unavailable" when voiceover wasn't requested
9. **Fix tool count** — Update all references to match actual count
10. **Delete dead code** — `_burn_captions` and other YAGNI violations

## Iteration Summary

| Phase | Description | Status |
|-------|-------------|--------|
| 36a-36h | V2V/A2V architecture upgrade (atomic tools, handler refactoring, SFX wiring, captions fallback) | ✅ Complete |
| 37a | Fix parse_srt content-vs-path bug in A2V | ✅ Complete |
| 37b | (Next) Add MCP startup version check | Pending |
| 37c | (Next) Fix PEXELS_API_KEY propagation | Pending |
| 37d | (Next) Standardize output paths to artifacts/ | Pending |
