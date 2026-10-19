# Fresh-Agent UX Audit #19 — A2V Pipeline Post-YAGNI Cleanup

**Date:** 2026-07-23  
**Source:** `/home/ishanp/Downloads/audit_v3_render.mp4`  
**Tool Called:** `audio.to_video` via MCP JSON-RPC  
**MCP Server:** `openscript-rs v0.1.0` (release build post-Phase 41 cleanup)

---

## Agent Simulation

A fresh agent was deployed with only:
1. Its role: "Create a video from an audio file"
2. MCP tool location: `./target/release/mcp-server`

The agent used a single tool call:
```json
{
  "tool": "audio.to_video",
  "args": {
    "audio_path": "/home/ishanp/Downloads/audit_v3_render.mp4",
    "preset": "Balanced",
    "aspect": "9:16",
    "burn_captions": true,
    "crf": 20
  }
}
```

**Result:** ✅ Video rendered successfully in one call.

---

## Output Video

| Property | Value |
|----------|-------|
| **Path** | `/home/ishanp/Downloads/fresh_agent_a2v_v2.mp4` |
| **Duration** | 135.4s |
| **Resolution** | 1080×1920 (9:16 portrait) |
| **Codec** | H.264 30fps + AAC |
| **File Size** | 85.8 MB |
| **Bitrate** | ~5.15 Mbps |
| **Segments** | 45 |
| **Backgrounds** | 12 (Pexels stock video) |

---

## Validation Scores

### verify.audio — 100/100 ✅

| Metric | Value | Status |
|--------|-------|--------|
| Quality Score | **100/100** | ✅ |
| Has Dialogue | true | ✅ |
| Peak dB | -0.3 | ✅ |
| RMS LUFS | -17.4 | ✅ |
| Sample Rate | 96000 Hz | ✅ |
| Silence Gaps | None | ✅ |

### verify.captions — 85/100 ⚠️

| Metric | Value | Status |
|--------|-------|--------|
| Caption Count | 45 | ✅ |
| Coverage | 110.8% | ✅ |
| Readability Score | **85/100** | ⚠️ |
| Too Slow (>5000ms) | 3 captions (idx 42, 43, 44) | ⚠️ |

**Issue:** The last 3 captions (indices 42-44) have durations >5000ms (7600ms, 8120ms, 8060ms). This is because the transcription's final segment groups too many words when the audio trail is long. The `srt.prepare` grouping logic should cap individual caption duration.

---

## Pipeline Flow Observed

The agent called a single tool (`audio.to_video`) which internally orchestrated:

1. **Transcribe** — HinglishGgml engine (whisper.cpp + Hindi2Hinglish-Apex-GGML q8)
2. **Group Captions** — `srt.prepare` (word-level → phrase-level)
3. **Build Timeline** — 45 segments with crossfade
4. **Fetch Backgrounds** — 12 Pexels stock videos (vertical, SD)
5. **Assign Backgrounds** — per-segment b-roll
6. **Assign Music** — background music with ducking (-12dB)
7. **Assign SFX** — hook, transitions, highlights
8. **Generate ASS** — Bebas Neue captions with word-level timing
9. **Render** — ffmpeg multilayer with font dir for ASS

**No regressions from Phase 41 cleanup.** The HinglishGgml transcription engine works correctly as the sole engine.

---

## Scoring Summary

| Category | Score | Notes |
|----------|-------|-------|
| **Audio Quality** | 100/100 | Perfect dialogue detection, no clipping |
| **Caption Quality** | 85/100 | 3 slow captions at end, rest excellent |
| **Visual Quality** | — | Requires manual review of stock footage |
| **Agent UX** | 90/100 | Single-call pipeline works end-to-end |
| **Overall** | **8.5/10** | |

---

## Remaining Issues

1. **3 slow captions (>5000ms)** — The last 3 segments group too many words. Fix: add `max_duration_ms` cap to `srt.prepare` grouping logic.

2. **No word-level timestamps** — whisper.cpp `-owts` flag fails silently, falling back to segment-level timing. This degrades word_highlight caption style. Fix: upgrade whisper.cpp to v1.7+ which has native JSON output.

3. **Single background style** — All 12 backgrounds are Pexels stock videos. No procedural backgrounds, no per-scene concept matching. The `extract_broll_concept` function returns empty for most scenes.

---

## Phase 41 Changes Verified

| Change | Status |
|--------|--------|
| Deleted `llm_postprocessor.py` | ✅ No references remain |
| Deleted `nemotron_transcriber.py` | ✅ No references remain |
| Replaced Apex check with HinglishGgml in `system.capabilities` | ✅ Working |
| Fixed `engine: whisper` → `hinglish-ggml` in audio.to_video | ✅ Working |
| Removed `_engine_str` unused variable | ✅ Clean build |
| Updated AGENTS.md and setup.sh | ✅ No stale references |
