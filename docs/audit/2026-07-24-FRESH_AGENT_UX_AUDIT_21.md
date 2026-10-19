# Fresh-Agent UX Audit #21 — A2V Pipeline End-to-End Simulation

**Date:** July 24, 2026
**Agent:** Buffy (parent) simulating a fresh AI agent with zero prior knowledge
**Objective:** Create a video from an audio file using only the MCP server
**MCP Server:** `target/release/mcp-server` (90 tools, openscript-rs v0.1.0)
**Audio Input:** `/tmp/fresh_agent_audit/speech_audio.wav` (2.5 min, extracted from cached YouTube video)

---

## Simulation Protocol

1. Fresh agent receives: MCP binary path + audio file path + objective ("create a video")
2. Agent sends `initialize` → receives serverInfo + instructions
3. Agent calls `system.doctor` → verifies environment readiness
4. Agent calls `audio.to_video` with the audio file
5. Agent attempts `verify.production` on the output
6. All gaps, failures, and friction points are documented

---

## Execution Summary

| Step | Tool Call | Result | Time |
|------|-----------|--------|------|
| 1 | `initialize` | ✅ Success (90 tools) | <1s |
| 2 | `system.doctor` | ✅ `ready_for_production: true` | ~2s |
| 3 | `audio.to_video` (attempt 1) | ❌ Failed: whisper-cli exit code 127 | ~5s |
| 4 | Fix: rebuild whisper.cpp + install libs | ✅ whisper-cli v1.9.1 works | ~3min |
| 5 | `audio.to_video` (attempt 2) | ✅ Rendered: 143.5s, 99.4 MB | ~2min |
| 6 | `verify.production` | ❌ Failed: requires `timeline_path` (not returned by audio.to_video) | N/A |

---

## Video Output Metrics

| Metric | Value | Assessment |
|--------|-------|------------|
| Duration | 143.5s | ✅ Matches input audio |
| Resolution | 1080x1920 (9:16) | ✅ Correct vertical format |
| Codec | H.264 / AAC | ✅ Standard |
| FPS | 30 | ✅ Correct |
| File Size | 99.4 MB | ✅ Reasonable for 2.5min |
| Backgrounds Used | 3 unique Pexels clips | ✅ Dedup working |
| Tracks Rendered | 5 (background, voiceover, music, stickers, captions) | ✅ All tracks active |
| Segments | 10 | ✅ SCENE_SIZE=4 grouping working |
| Caption Readability | 75/100 | ⚠️ Acceptable but not great |
| Caption Coverage | 26.5% | ⚠️ Low — audio has long silent gaps |
| Stickers | 4 GIPHY stickers | ✅ Working |

---

## UX Gaps Found (Critical → Minor)

### 🔴 CRITICAL — Environment Bootstrap Failure

**Gap:** `whisper-cli` shared libraries (`libwhisper.so.1`, `libggml.so.0`) not installed system-wide after container reset.

**Fresh agent experience:** `system.doctor` reports `ready_for_production: true` but `audio.to_video` fails immediately with whisper-cli exit code 127. The agent has no way to fix this — it doesn't know about shared library dependencies.

**Root cause:** whisper.cpp is built during setup but libraries aren't installed to a persistent location. Container resets wipe `/tmp/whisper.cpp/build/`.

**Fix options:**
1. **Add `setup.sh` step:** Copy whisper shared libraries to `~/.local/lib/` and run `ldconfig` during setup
2. **Add rpath to whisper-cli binary:** Build whisper.cpp with `-DCMAKE_INSTALL_RPATH=~/.local/lib` so the binary finds its libs without LD_LIBRARY_PATH
3. **Improve error message:** When whisper-cli fails with exit 127, return a ToolError with actionable instructions ("Run `bash setup.sh` to install whisper.cpp dependencies")

### 🔴 CRITICAL — verify.production Can't Run on audio.to_video Output

**Gap:** `audio.to_video` does not produce a `timeline_path` in its response. `verify.production` requires `timeline_path` as a mandatory argument. Therefore, a fresh agent cannot verify the quality of A2V output.

**Fresh agent experience:** Agent calls `verify.production(video_path=...)` → gets error "Missing required argument: timeline_path". Agent has no way to know that A2V doesn't create a timeline JSON.

**Root cause:** `audio.to_video` renders directly from a `MultiLayerRenderSpec` without persisting a timeline JSON. The `timeline_path` field is absent from the response.

**Fix options:**
1. **Add `timeline_path` to audio.to_video response:** Even if the timeline is ephemeral, serialize it to disk and return the path so agents can inspect/verify it
2. **Make `verify.production` accept `video_path` alone:** Add a video-only verification mode that doesn't need a timeline
3. **Add `verify.render` fallback:** `verify.render` only needs `video_path` — document this as the A2V verification path

### 🟡 HIGH — No Pre-Tested Audio with Speech Content

**Gap:** No test audio file with actual speech is bundled with the project. The first attempt used a sine wave (no speech), which whisper naturally failed on.

**Fresh agent experience:** Agent needs to find or create speech audio before testing. No guidance in AGENT_GUIDE.md about what audio formats are supported.

**Fix:** Add a sample audio file (10-30s of Hindi/Hinglish speech) to `mcp/assets/samples/` for fresh-agent testing.

### 🟡 HIGH — instructions String Overwhelms Fresh Agents

**Gap:** The `initialize` response includes a massive `instructions` string (~4KB) covering all 28 tool families, all workflows, all render engines. A fresh agent must parse this wall of text to find `audio.to_video`.

**Fresh agent experience:** Agent receives 4KB of instructions and must mentally filter to find the A2V path. No progressive disclosure — everything is dumped at once.

**Fix:** Add a `help.workflow` tool that returns only the relevant workflow for a given input type (audio → A2V, video → V2V, script → from-scratch).

### 🟡 HIGH — Caption Coverage Low (26.5%)

**Gap:** The 2.5-minute speech audio has long silent gaps between speech segments. The ASS captions only cover 26.5% of the video duration, leaving 73.5% as blank/placeholder backgrounds.

**Fresh agent experience:** Agent sees a video where most of it is looping stock footage with no captions. Looks unfinished.

**Fix:** Improve `srt.prepare` to detect and handle long silence gaps — either trim them out or add visual-only segments (stock footage + music) during silent periods.

### 🟡 MEDIUM — Stickers Always at Bottom-Right

**Gap:** All 4 GIPHY stickers are positioned at `top_left=(878, 1718)` which is bottom-right. No variety in positioning.

**Fresh agent experience:** All stickers cluster in one corner. Looks repetitive.

**Fix:** Cycle sticker positions across scenes (bottom-right, top-left, center, etc.) based on scene index.

### 🟢 LOW — MCP Server Binary Goes Stale After Code Changes

**Gap:** The release binary (`target/release/mcp-server`) must be manually rebuilt (`cargo build -p openscript-mcp --release`) after every code change. The fresh agent doesn't know this — it may be running a stale binary.

**Fresh agent experience:** Agent runs `system.doctor` which reports `ready_for_production: true`, but the binary is from a previous build and may have different behavior than expected.

**Fix:** Add a `version` field to `system.doctor` output that includes the git commit hash. If the hash doesn't match the source code, warn the agent to rebuild.

### 🟢 LOW — Pexels API Key Validity Not Checked

**Gap:** `system.doctor` reports whether `PEXELS_API_KEY` is set, but not whether it's valid. An expired/invalid key causes b-roll fetching to fail silently.

**Fresh agent experience:** Agent sees `PEXELS_API_KEY: set` in system.doctor, assumes b-roll will work, then gets empty results from `broll.fetch`.

**Fix:** `system.doctor` should make a test Pexels API call and report validity.

### 🟢 LOW — No Progress Feedback During Long Renders

**Gap:** `audio.to_video` takes ~2 minutes to complete but the MCP `notifications/progress` capability isn't surfaced to agents in the stdio transport. The `report_progress` calls exist in the handler but the agent doesn't see them.

**Fresh agent experience:** Agent sends request, waits 2 minutes, gets response. No way to know if it's stuck or progressing.

**Fix:** Surface MCP progress notifications in the stdio transport, or add a `tool.progress` field to the tool response.

### 🟢 LOW — SCENE_SIZE Hardcoded to 4 SRT Entries

**Gap:** Each scene groups exactly 4 SRT entries regardless of their duration. Short entries create very short scenes; long entries create very long ones.

**Fresh agent experience:** Some scenes are 2 seconds, others are 15 seconds. Pacing feels inconsistent.

**Fix:** Add a `max_scene_duration_s` parameter to control scene grouping by duration rather than entry count.

---

## What Worked Well

1. **`system.doctor`** — Correctly reports environment status and next actions
2. **`audio.to_video` one-call pipeline** — Transcribe → group → backgrounds → music → SFX → captions → render in a single call
3. **Cross-scene dedup** — 3 unique Pexels clips across 10 segments (no duplicates)
4. **Stock signal integration** — Generated context-aware queries ("coffee mug steam desk morning", "hand writing notebook paper daylight")
5. **GIPHY stickers** — 4 stickers fetched and overlaid automatically
6. **ASS captions with word-level timing** — 75/100 readability score
7. **Return artifact paths** — `srt_path`, `grouped_srt_path`, `ass_path` all returned for agent inspection
8. **Tool count accuracy** — 90 tools correctly reported in both `server.rs` and `AGENT_GUIDE.md`

---

## Score

| Dimension | Score | Notes |
|-----------|-------|-------|
| Discoverability | 6/10 | Tools exist but instructions wall is overwhelming |
| Environment Readiness | 3/10 | whisper-cli broken after container reset |
| One-Call Success | 8/10 | audio.to_video works end-to-end once env is fixed |
| Output Quality | 7/10 | 1080x1920, 3 unique backgrounds, captions, stickers |
| Verification Loop | 2/10 | verify.production can't run on A2V output |
| Error Recovery | 4/10 | Error messages don't guide agent to fix |

**Overall Fresh-Agent UX Score: 5.0/10**

---

## Recommended Fixes (Priority Order)

1. **[CRITICAL]** Fix whisper-cli library installation in `setup.sh` — copy to `~/.local/lib/` + ldconfig
2. **[CRITICAL]** Add `timeline_path` to `audio.to_video` response OR make `verify.production` work without it
3. **[HIGH]** Add sample speech audio to `mcp/assets/samples/`
4. **[HIGH]** Improve whisper-cli error message to include fix instructions
5. **[HIGH]** Add `help.workflow` tool for progressive disclosure
6. **[MEDIUM]** Improve caption coverage for silent-heavy audio
7. **[MEDIUM]** Cycle sticker positions across scenes
8. **[LOW]** Send MCP progress notifications during long renders
9. **[LOW]** Add duration-based scene grouping option
