# Fresh-Agent UX Audit #26 — Post-Phase-87 Verification

**Date:** 2026-07-27
**Input:** `/home/ishanp/Downloads/audit_v3_render.mp4` (Hinglish audio, 2:30)
**Output:** `/home/ishanp/Downloads/audit26_video.mp4` (25.9MB)
**Grade:** F (39/100)

---

## What Changed Since Audit #25

| Issue | Audit #25 | Audit #26 | Status |
|-------|-----------|-----------|--------|
| B-roll key mismatch (zero broll) | ❌ 0 events in video | ✅ 8 events with correct `broll_X` keys | **FIXED** |
| Caption animation (no fade) | ❌ Static color/scale only | ✅ `\fad(80,80)` tags added | **FIXED** |
| B-roll timing (all at 0-3000ms) | ❌ All events stacked at start | ✅ Events spread across video (0-13.2s, etc.) | **FIXED** |
| Music search returns nothing | ❌ No results | ❌ Still no results | NOT FIXED |
| Audio clipping | ❌ Peak -0.2 dBFS | ❌ Still clipping | NOT FIXED |
| No SFX | ❌ None | ❌ None | NOT FIXED |

---

## Pipeline Execution Log

| Step | Tool | Result |
|------|------|--------|
| 1 | `system.capabilities` | ✅ 88 tools |
| 2 | `transcribe` | ✅ 45 entries |
| 3 | `srt.to_timeline` (with source_video) | ✅ Timeline built, validation passes |
| 4 | `segment.analyze` | ✅ 12 segments |
| 5 | Agent generates 8 English keywords | 🧠 Content understanding |
| 6 | `broll.fetch` (auto-place, 8 concepts × 1 clip) | ✅ 8 clips placed on broll track |
| 7 | `captions.generate_ass` (word_highlight + \fad) | ✅ ASS with fade animation |
| 8 | `music.search` ("dramatic suspense") | ❌ Returns empty — music index issue |
| 9 | `timeline.validate` | ✅ Valid, no errors |
| 10 | `timeline.render` | ✅ 25.9MB rendered |
| 11 | `verify.production` | Grade F (39/100) |

---

## Verification Score Breakdown

| Dimension | Score | Max | Status |
|-----------|-------|-----|--------|
| Video source quality | 10 | 10 | ✅ |
| Visual hooks | 8 | 8 | ✅ |
| Visual variance | 8 | 8 | ✅ |
| Context relevance | 2 | 8 | ⚠️ No video_keywords |
| Cuts/pacing | 2 | 5 | ⚠️ 0.05 cps (too static) |
| **BG music** | **0** | **8** | **❌ HARD FAIL** |
| **SFX** | **0** | **6** | **❌ HARD FAIL** |
| Stickers | 0 | 8 | ❌ None |
| Captions | 1 | 6 | ⚠️ Style not detected |
| Voiceover | 0 | 6 | ❌ Not detected |
| **Audio mix** | **1** | **5** | **❌ HARD FAIL (clipping)** |
| Section composition | 2 | 8 | ⚠️ No section map |
| Visual hierarchy | 1 | 5 | ⚠️ Missing titles |
| Platform optimization | 2 | 5 | ⚠️ Duration too long |
| Timeline editor | 2 | 4 | ⚠️ Empty music/SFX tracks |
| **TOTAL** | **39** | **100** | **Grade F** |

---

## Remaining Gaps (Priority Order)

### Sprint 1 — Fix music workflow (biggest score impact: +8 points)

| # | Issue | Fix |
|---|-------|-----|
| 1 | `music.search` returns no results | Investigate why — index may not be built, or tool may be deprecated in favor of `library.search` |
| 2 | `music.search` described as "DEPRECATED — forwards to library.search" | Agent should use `library.search` directly |
| 3 | `library.search` returns tracks but path extraction fails | Check response schema |

### Sprint 2 — Fix HARD FAILs (+10 points)

| # | Issue | Fix |
|---|-------|-----|
| 4 | Audio clipping (peak -0.2 dBFS) | Set loudnorm TP to -3.0 or add post-mix alimiter |
| 5 | No SFX at transitions | Create `sfx.auto_assign` tool or add SFX in timeline.build |
| 6 | Caption style not detected by verifier | Fix `verify.production` to read ASS file style |

### Sprint 3 — Production quality (+20 points)

| # | Issue | Fix |
|---|-------|-----|
| 7 | No stickers/GIFs | Use `sticker.render` or `gif.search` + `overlay.assign` |
| 8 | No voiceover | The original audio IS the voiceover — verifier should detect it |
| 9 | No section map | `segment.analyze` should output section_map |
| 10 | Duration too long (150s) | Agent should set target duration or trim segments |

---

## Projected Score After Fixes

| Sprint | Score | Grade |
|--------|-------|-------|
| Current | 39 | F |
| After Sprint 1 (music) | 47 | F |
| After Sprint 2 (clipping + SFX) | 57 | D |
| After Sprint 3 (stickers + voiceover) | 77 | B |

---

## Output Artifacts

| File | Location |
|------|----------|
| Rendered video | `/home/ishanp/Downloads/audit26_video.mp4` (25.9MB) |
| Timeline | `/home/ishanp/Downloads/audit26_timeline.json` |
| Captions ASS | `/home/ishanp/Downloads/audit26_captions.ass` |
| Full results | `/home/ishanp/Downloads/audit26_results.json` |
| Audit report | `FRESH_AGENT_UX_AUDIT_26.md` |
