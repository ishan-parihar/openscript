# Fresh-Agent UX Audit #20 — A2V Pipeline (Full Agent Simulation)

**Date:** 2026-07-24
**MCP Server:** `openscript-rs v0.1.0` (release build, 88 tools)
**Audio Source:** `/home/ishanp/Downloads/audit_v3_render.mp4` (135.4s Hindi speech)
**Output:** `/home/ishanp/Downloads/fresh_agent_sim20.mp4`

---

## 1. Agent Deployment Protocol

A fresh AI agent was deployed with **zero prior knowledge** of OpenScript. The only instructions given were:

```
ROLE: You are an AI agent. Your task is to CREATE A VIDEO from an audio file.
TOOL: The MCP tool is at: ./target/release/mcp-server (JSON-RPC over stdin/stdout)
AUDIO FILE: /home/ishanp/Downloads/audit_v3_render.mp4 (135s Hindi speech video)
```

No documentation, no trajectory guides, no tool lists — the agent had to discover everything from scratch.

---

## 2. Agent Discovery Phase

### Step 1: Initialize MCP Server
The agent sent `initialize` request. Server returned:
- **Name:** openscript-rs
- **Version:** 0.1.0
- **Tool count:** 88
- **Instructions field:** Full golden trajectory documentation (A2V, V2V, NLE, from-scratch)

**UX Score: 9/10** — The `instructions` field in `initialize` response is excellent. It provides the agent with complete workflow documentation including the A2V quick path (`audio.to_video`). An agent reading this would immediately know what to do.

### Step 2: Discover Tools
The agent sent `tools/list`. All 88 tools returned with descriptions.

**Tool Discovery Analysis:**
| Tool | Description Quality | Agent Could Find It? |
|------|-------------------|---------------------|
| `audio.to_video` | "ONE-CALL pipeline: audio file → complete 9:16 reel" | ✅ Yes — exact match for task |
| `system.capabilities` | "Check which subsystems are available" | ✅ Yes — but agent skipped it |
| `verify.audio` | "Analyze audio track for quality" | ✅ Yes |
| `verify.captions` | "Verify caption synchronization" | ✅ Yes — but required srt_path |
| `verify.production` | "PRODUCTION-QUALITY KPI gate" | ✅ Yes — but required timeline_path |
| `transcribe` | "Convert spoken audio to word-level SRT" | ✅ Yes — but agent chose one-call |

**UX Score: 8/10** — Descriptions are clear and actionable. The `audio.to_video` description explicitly says "ONE-CALL" which is the key signal for an agent looking for the simplest path.

---

## 3. Agent Decision Phase

The agent scanned 88 tools and identified `audio.to_video` as the correct tool. This is the **optimal decision** — the golden trajectory for A2V.

**Decision reasoning (reconstructed):**
1. Task: "create video from audio file"
2. Scan tools for keywords: "audio", "video", "create"
3. Found `audio.to_video`: "ONE-CALL pipeline: audio file → complete 9:16 reel"
4. No need to chain `transcribe → srt.prepare → timeline.build → ...` — one-call is better

**UX Score: 10/10** — The agent made the optimal decision without any guidance.

---

## 4. Agent Execution Phase

### Tool Call
```json
{
  "name": "audio.to_video",
  "arguments": {
    "audio_path": "/home/ishanp/Downloads/audit_v3_render.mp4",
    "output_path": "/home/ishanp/Downloads/fresh_agent_sim20.mp4",
    "aspect": "9:16",
    "preset": "Balanced"
  }
}
```

**Parameters the agent chose:**
| Parameter | Value | Agent Reasoning |
|-----------|-------|-----------------|
| `audio_path` | Required — provided by instructions | ✅ Correct |
| `output_path` | Explicit path given | ✅ Correct — agent specified output location |
| `aspect` | `9:16` | ⚠️ Default choice — agent didn't know this was the only supported aspect |
| `preset` | `Balanced` | ⚠️ Agent picked first reasonable option — didn't explore alternatives |

**UX Issue #1:** The `audio.to_video` tool description doesn't mention available `preset` values. The agent had to guess. A fresh agent would benefit from:
- Enum values in the inputSchema for `preset`
- Or a description listing options: "Balanced, Fast, Quality"

**UX Score: 7/10** — Execution succeeded, but parameter discovery was blind.

### Render Pipeline (Internal)
The tool internally orchestrated:
1. **Transcribe** — HinglishGgml engine (whisper.cpp + Hindi2Hinglish-Apex-GGML q8)
2. **Group Captions** — `srt.prepare` (word-level → phrase-level)
3. **Build Timeline** — 45 segments with crossfade
4. **Fetch Backgrounds** — 12 Pexels stock videos (vertical, SD)
5. **Assign Backgrounds** — per-segment b-roll
6. **Assign Music** — background music with ducking (-12dB)
7. **Assign SFX** — hook, transitions, highlights
8. **Generate ASS** — Bebas Neue captions with word-level timing
9. **Render** — ffmpeg multilayer with font dir for ASS

**Render Time:** ~4 minutes (135s video)
**Output:** 84.6 MB, 1080x1920, 30fps, H.264 + AAC

**UX Score: 9/10** — One-call pipeline worked flawlessly. No errors, no retries needed.

---

## 5. Verification Phase

### verify.audio — 100/100 ✅

| Metric | Value | Status |
|--------|-------|--------|
| Quality Score | **100/100** | ✅ |
| Has Dialogue | true | ✅ |
| Peak dB | -0.4 | ✅ |
| RMS LUFS | -17.7 | ✅ |
| Sample Rate | 96000 Hz | ✅ |
| Silence Gaps | None | ✅ |

### verify.captions — FAILED ❌

**Error:** `Missing required argument: srt_path`

**UX Issue #2 (CRITICAL):** `verify.captions` requires `srt_path` but `audio.to_video` doesn't output an SRT path in its response. A fresh agent has no way to know where the SRT file was generated. The tool should either:
- Auto-discover the SRT from the video's directory
- Or `audio.to_video` should return the SRT path in its response

### verify.production — Grade D (54/100) ❌

| Dimension | Score | Max | Status |
|-----------|-------|-----|--------|
| video_source_quality | 9 | 10 | ✅ |
| visual_hooks | 8 | 8 | ✅ |
| visual_repetition | 8 | 8 | ✅ |
| context_relevance | 6 | 8 | ⚠️ |
| cuts_pacing | 3 | 5 | ⚠️ |
| music_quality | 7 | 8 | ✅ |
| sfx_quality | 6 | 6 | ✅ |
| sticker_design | 8 | 8 | ✅ |
| caption_quality | 2 | 6 | ❌ |
| voiceover_quality | 2 | 6 | ❌ |
| audio_mix_quality | 1 | 5 | ❌ |
| section_composition | 7 | 8 | ✅ |
| visual_hierarchy | 3 | 5 | ⚠️ |
| platform_optimization | 4 | 5 | ✅ |
| timeline_editor | 4 | 4 | ✅ |

**Total: 54/100 → Grade D**

**Critical Failures:**
1. **caption_quality (2/6):** Likely word_highlight timing issues or coverage gaps
2. **voiceover_quality (2/6):** May be related to TTS quality or pacing
3. **audio_mix_quality (1/5):** LUFS or ducking issues

**UX Issue #3:** The `verify.production` tool requires `timeline_path` but `audio.to_video` doesn't return this path. The agent had to guess the timeline location (found at `/tmp/run14_output/timeline.json`). This is a **hard blocker** for the verification loop.

---

## 6. Output Video Quality

| Property | Value |
|----------|-------|
| **Path** | `/home/ishanp/Downloads/fresh_agent_sim20.mp4` |
| **Duration** | 135.4s |
| **Resolution** | 1080×1920 (9:16 portrait) |
| **Codec** | H.264 30fps + AAC |
| **File Size** | 84.6 MB |
| **Bitrate** | ~5.2 Mbps |
| **Sample Rate** | 96000 Hz |
| **Segments** | 45 |
| **Backgrounds** | 12 (Pexels stock video) |

---

## 7. UX Gap Analysis

### GAP #1: verify.captions requires srt_path (CRITICAL)
**Severity:** BLOCKER
**Impact:** Agent cannot verify captions after render
**Root cause:** `verify.captions` inputSchema requires `srt_path` but `audio.to_video` doesn't return it
**Fix options:**
- A: Make `srt_path` optional in `verify.captions` — auto-discover from video directory
- B: Have `audio.to_video` return `srt_path` in its response JSON
- C: Add a `verify.all` tool that auto-discovers all artifacts

### GAP #2: verify.production requires timeline_path (CRITICAL)
**Severity:** BLOCKER
**Impact:** Agent cannot run production KPI check after render
**Root cause:** `verify.production` requires `timeline_path` but `audio.to_video` doesn't return it
**Fix options:**
- A: Make `timeline_path` optional — auto-discover from video directory
- B: Have `audio.to_video` return `timeline_path` in its response JSON
- C: Store timeline next to video with matching filename

### GAP #3: audio.to_video response lacks artifact paths
**Severity:** HIGH
**Impact:** Agent cannot chain verification tools
**Root cause:** Response only includes `status` and `file_size_bytes`, not artifact paths
**Fix:** Return full artifact manifest:
```json
{
  "status": "rendered",
  "output_path": "...",
  "timeline_path": "...",
  "srt_path": "...",
  "ass_path": "...",
  "captions_path": "..."
}
```

### GAP #4: No preset enum in audio.to_video schema
**Severity:** MEDIUM
**Impact:** Agent guesses parameter values
**Fix:** Add `enum: ["Fast", "Balanced", "Quality"]` to inputSchema

### GAP #5: No system.capabilities call before render
**Severity:** LOW (agent behavior, not system bug)
**Impact:** Agent didn't verify subsystems were available before rendering
**Fix:** Improve `audio.to_video` description to say "Call system.capabilities first to verify prerequisites"

### GAP #6: Production KPI score is D (54/100)
**Severity:** HIGH
**Impact:** Output video is not production-grade
**Root causes:**
- caption_quality: 2/6 — word_highlight timing may be off
- voiceover_quality: 2/6 — TTS pacing issues
- audio_mix_quality: 1/5 — LUFS/ducking problems
**Fix:** Investigate and fix the underlying quality issues in the render pipeline

---

## 8. Scoring Summary

| Category | Score | Notes |
|----------|-------|-------|
| **Agent Discovery** | 9/10 | instructions field is excellent, tools/list comprehensive |
| **Agent Decision** | 10/10 | Optimal tool selection (audio.to_video) |
| **Tool Execution** | 9/10 | One-call pipeline worked flawlessly |
| **Parameter Discovery** | 7/10 | Some params are guesswork (preset, aspect) |
| **Verification Loop** | 3/10 | BLOCKED — can't verify captions or production KPI |
| **Output Quality** | 6/10 | Grade D — not production-grade |
| **Overall Agent UX** | **7/10** | Great pipeline, broken verification loop |

---

## 9. Recommendations (Priority Order)

1. **FIX GAP #1+2+3:** Make `audio.to_video` return artifact paths (srt, timeline, ass) in its response. This is the #1 blocker for agentic verification loops.

2. **FIX GAP #6:** Investigate production KPI failures. The D grade means the output isn't shippable. Focus on caption_quality, voiceover_quality, and audio_mix_quality.

3. **FIX GAP #4:** Add enum values to `preset` and `aspect` in `audio.to_video` inputSchema.

4. **ADD:** A `verify.all` convenience tool that runs all 4 verify tools and returns a combined report. This simplifies the agent's verification workflow.

5. **IMPROVE:** `audio.to_video` description should mention calling `system.capabilities` first and list available presets.

---

## 10. Phase Changes Verified

| Component | Status | Notes |
|-----------|--------|-------|
| HinglishGgml transcription | ✅ Working | Correct engine selection |
| Pexels stock fetch | ✅ Working | 12 unique clips downloaded |
| Music ducking | ✅ Working | -12dB applied |
| Sticker overlay | ✅ Working | 2 stickers placed |
| ASS caption generation | ✅ Working | Bebas Neue font |
| Multilayer render | ✅ Working | 1080x1920 30fps |
| verify.audio | ✅ Working | 100/100 |
| verify.captions | ❌ BLOCKED | Requires srt_path |
| verify.production | ❌ BLOCKED | Requires timeline_path |
