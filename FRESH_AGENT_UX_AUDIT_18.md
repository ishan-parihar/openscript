# Fresh-Agent UX Audit #18 — A2V Pipeline

**Date:** 2026-07-23
**Input:** `/home/ishanp/Downloads/audit_v3_render.mp4` (135s, 1080x1920, h264+aac)
**Tool Called:** `audio.to_video` via MCP JSON-RPC

---

## Execution Summary

| Metric | Value |
|--------|-------|
| **Pipeline** | audio.to_video (MCP tool) |
| **Preset** | Balanced |
| **Aspect** | 9:16 (vertical) |
| **Status** | ✅ rendered |
| **Duration** | 135.4s |
| **File Size** | 75.7 MB |
| **Resolution** | 1080x1920 |
| **Frame Rate** | 30 fps |
| **Total Frames** | 4,062 |
| **Video Codec** | h264 |
| **Audio Codec** | aac (96kHz, 2ch) |
| **Bit Rate** | 4.47 Mbps |
| **Segments** | 23 |
| **Stickers** | 18 (GIPHY) |
| **Backgrounds** | 1 (Pexels) |
| **Warnings** | None |

## Output File

- **Location:** `/home/ishanp/Downloads/a2v_audit_output.mp4`
- **Also at:** `/tmp/audio_to_video_1784786618.mp4`

## Audio Quality

- **Integrated Loudness:** -15.10 LUFS (input) → -24.04 LUFS (output)
- **Normalization:** Dynamic (EBU R128)
- **Caption Burn-in:** Confirmed — no separate subtitle stream, captions burned into video frames

## Visual Quality (Frame Analysis)

| Timestamp | File Size | Bright Pixels (%) | Caption Visible |
|-----------|-----------|-------------------|-----------------|
| 10s | 253 KB | 11.9% | ✅ Yes |
| 30s | 210 KB | 14.4% | ✅ Yes |
| 60s | 208 KB | 14.5% | ✅ Yes |
| 90s | 205 KB | 14.1% | ✅ Yes |
| 120s | 213 KB | 12.1% | ✅ Yes |

## Ranking Score

| Category | Score | Notes |
|----------|-------|-------|
| **Pipeline Execution** | 9/10 | Clean execution, no errors, all warnings resolved |
| **Audio Quality** | 8/10 | Loudness normalized to -24 LUFS, clean aac codec |
| **Caption Burn-in** | 8/10 | Captions visible across all frames (~12-15% bright pixels) |
| **Sticker Overlay** | 7/10 | 18 GIPHY stickers fetched and placed, hardcoded position |
| **Background** | 6/10 | Only 1 background used (full audio treated as single scene) |
| **Duration Match** | 10/10 | Output matches input duration exactly (135.4s) |
| **Format** | 9/10 | Correct 9:16 vertical, 30fps, h264+aac |

### **Overall Score: 8.1 / 10**

## Identified Issues

1. **Single background** — The entire 135s audio is treated as one scene with one Pexels background. No scene segmentation or multiple backgrounds.
2. **Hardcoded sticker position** — All stickers placed at bottom-right with 0.15 scale. No per-segment customization.
3. **No meme b-rolls** — The A2V pipeline doesn't include full-screen meme/transition clips.
4. **Sticker fetch blocks render** — GIPHY stickers are fetched synchronously just before rendering.

## What Worked Well

1. **Zero-touch execution** — Fresh agent called one tool, got a complete video.
2. **Captions burned in** — Word-level kinetic captions visible throughout.
3. **Audio normalized** — Proper loudness normalization applied.
4. **No warnings** — Clean execution with no edge-case failures.
5. **Correct input file** — Used audit_v3_render.mp4 as specified.

## Recommendations for Next Iteration

1. Add scene segmentation to split long audio into multiple scenes with different backgrounds.
2. Make sticker position/scale configurable per segment.
3. Add meme b-roll transitions between scenes.
4. Move sticker fetch earlier in the pipeline (before render spec construction).
