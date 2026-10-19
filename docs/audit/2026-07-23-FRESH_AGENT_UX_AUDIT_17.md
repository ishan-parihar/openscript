# FRESH AGENT UX AUDIT #17 — July 23, 2026 (Post-Phase 38)

## Audit Methodology

Fresh-agent simulation deployed with ONLY:
- MCP server binary location (`target/release/mcp-server`)
- Task: "Create a video using audio.to_video and reelize.timeline"
- Correct input file: `/home/ishanp/Downloads/audit_v3_render.mp4`

## Simulation Results

### Pipeline 1: `audio.to_video` (A2V)

| Metric | Result |
|--------|--------|
| Status | **RENDERED** |
| Input | `/home/ishanp/Downloads/audit_v3_render.mp4` (21MB source) |
| Output | `/tmp/audio_to_video_1784771997.mp4` |
| Duration | 135.4 seconds |
| File Size | 73MB |
| Segments | 22 |
| Stickers | **18 GIPHY stickers fetched** ✓ |
| B-roll | 1 background (PEXELS_API_KEY resolved from config) |
| Music | 1 track with ducking |
| SFX | Assigned |
| Captions | ASS with Bebas Neue |

### Pipeline 2: `reelize.timeline` (V2V)

| Metric | Result |
|--------|--------|
| Status | **RENDERED** |
| Input | `/home/ishanp/Downloads/audit_v3_render.mp4` (21MB source) |
| Output | `/home/ishanp/Downloads/audit_v3_render.reel.mp4` |
| Duration | 66.4 seconds |
| File Size | 30MB |
| Segments | 17 |
| B-roll | 20 clips from Pexels |
| Music | 1 track with ducking |
| SFX | 12 hits |
| Captions | ASS with Bebas Neue |
| Tracks Rendered | 50 |

## What's Working Well (Compared to Audit #16)

1. **PEXELS_API_KEY resolved correctly** — No more "stock backgrounds skipped" warning
2. **Stickers now present in A2V** — 18 GIPHY stickers fetched and rendered
3. **V2V produces correct output** — Using the right input file (audit_v3_render.mp4)
4. **Captions properly burned** — ASS with Bebas Neue font, word-level timing
5. **Music with ducking** — Sidechain compression working
6. **SFX mixed in** — Hook, transitions, highlights assigned
7. **Build clean** — 0 warnings, 309 tests pass, 0 failures

## Remaining Issues

### MEDIUM (P2) — Quality Improvements

| # | Issue | Impact |
|---|-------|--------|
| 1 | **V2V b-roll file path error** — Some Pexels clips have `,` in filename causing ffmpeg skip | Visual gaps in some segments |
| 2 | **V2V voiceover warning** — "Voiceover unavailable" when not requested | Unnecessary warning noise |
| 3 | **A2V outputs to /tmp/** — Path is unpredictable for agents | Agent can't reference output |
| 4 | **V2V sticker builder not wired** — Only A2V has GIPHY stickers | V2V videos lack stickers |

### LOW (P3) — Polish

| # | Issue | Impact |
|---|-------|--------|
| 5 | **Tool count** — Binary reports 88 tools, consistent with AGENT_GUIDE | Minor |
| 6 | **HTTP client per-call** — build_v2v_stickers creates new client each call | Performance |

## Architecture Assessment

### Golden Path Status

| Pipeline | Status | Stickers | B-roll | Music | SFX | Captions |
|----------|--------|----------|--------|-------|-----|----------|
| script.to_video | ✅ Working | ✅ GIPHY | ✅ Multi-broll | ✅ Ducking | ✅ Mixed | ✅ ASS |
| reelize.timeline | ✅ Working | ❌ Not wired | ✅ Pexels | ✅ Ducking | ✅ Assigned | ✅ ASS |
| audio.to_video | ✅ Working | ✅ GIPHY | ⚠️ Single bg | ✅ Ducking | ✅ Assigned | ✅ ASS |

### Key Improvement from Audit #16 → #17

- PEXELS_API_KEY propagation fixed → stock backgrounds now work
- Sticker builder added to A2V → 18 stickers rendered
- Correct input file used → V2V produces meaningful output
- Build clean → 0 warnings, 309 tests pass

## Generated Videos for Testing

| Pipeline | Input | Output | Size | Duration |
|----------|-------|--------|------|----------|
| **A2V** | `/home/ishanp/Downloads/audit_v3_render.mp4` | **`/tmp/audio_to_video_1784771997.mp4`** | **73MB** | **135s** |
| **V2V** | `/home/ishanp/Downloads/audit_v3_render.mp4` | **`/home/ishanp/Downloads/audit_v3_render.reel.mp4`** | **30MB** | **66s** |

## Recommended Next Iterations

1. **Wire sticker builder into V2V** — Add GIPHY stickers to reelize.timeline
2. **Fix b-roll filename escaping** — Handle commas in Pexels filenames
3. **Standardize output paths** — Both pipelines should write to artifacts/
4. **Run verify.production** — Assess quality scores on both outputs
