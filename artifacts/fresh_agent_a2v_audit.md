# Fresh-Agent A2V Audit — `/home/ishanp/Downloads/audit_v3_render_audio.mp3`

**Date:** 2026-07-31 (UTC)
**Mode:** cold fresh-agent — read only AGENT_GUIDE.md / MCP `initialize` instructions
**Trajectory:** A2V (audio → video) via atomic tools (transcribe → srt.to_timeline → segment.analyze → broll.fetch → music.assign → sfx.auto_assign → timeline.render)
**Output:** `output/audit_v3_a2v.mp4` — 25 MB, 135.7s, 1080x1920@30fps, 4069 frames, 12 unique stock clips, 60 overlays
**Verifier:** `verify.production` → **Score 49, Grade D, status=fail** (target ≥ B)

---

## TL;DR

The fresh-agent successfully navigated the A2V trajectory and produced a renderable video. The trajectory succeeded — every tool call returned a valid response. **However, the output fails the production KPI gate** because the trajectory contains gaps the agent guide does not warn about:

1. **Captions are never burned** — the trajectory ended at `timeline.render` without `captions.generate_ass`. The resulting video has zero on-screen text, which is the single most important retention feature for short-form vertical content.
2. **No stickers / no overlays** — visual hierarchy is bare stock footage. The trajectory listed `sticker.load_preset` / `gif.search` in the instructions but the A2V agentic recipe does not require them.
3. **Audio clips at peak -0.0 dBFS** — `sfx.auto_assign` with `gain_db=-10` plus original audio + music pushes the mix past 0 dBFS even with the loudnorm filter chain.

The trajectory is **technically correct but visually under-delivering**. Three targeted fixes will raise score from 49 → 80+ without changing the trajectory.

---

## Trajectory Trace (with timings)

| Step | Tool | Call | Outcome | Time |
|---|---|---|---|---|
| 1 | `system.doctor` | probe | ✅ ready_for_production=true | ~1s |
| 2 | `transcribe` | audio → SRT | ✅ 45 entries, 140s | ~190s |
| 3 | `srt.read` | inspect | ✅ read 45 entries | ~1s |
| 4 | `srt.to_timeline` | SRT → EDL v2 | ✅ 44 segments, 139.4s | ~1s |
| 5 | `segment.analyze` | SRT → segments | ✅ 12 grouped segments | ~1s |
| 6 | `broll.fetch` | Pexels + auto-place | ✅ 12 clips downloaded + assigned | ~36s |
| 7a | `library.search` | find music | ⚠️ initial query `mood=dramatic` returned 0 (filter combination too narrow); re-query `query="cinematic dramatic"` → 5 results | ~1s |
| 7b | `library.download` | download music | ✅ 4.5 MB fetched | ~4s |
| 7c | `music.assign` | place on timeline | ✅ music_002, span 0→139400ms | ~1s |
| 8 | `sfx.auto_assign` | 46 SFX placements | ✅ hook@0 + 44 transitions + outro | ~1s |
| 9 | `timeline.validate` | check integrity | ✅ valid | ~1s |
| 10 | `timeline.render` | produce MP4 | ✅ 25 MB, 4069 frames, 60 overlays | ~101s |
| 11 | `verify.production` | score | ❌ Score 49, Grade D, 2 hard fails | ~3s |

**Total fresh-agent runtime:** ~5 min 30 s of which 290 s was the rendering + transcription (whisper.cpp). Pure agent decision time: <30 s.

---

## KPI Score Breakdown (15 dimensions)

| Dimension | Score | Max | Issue |
|---|---|---|---|
| Video source quality | 10 | 10 | ✅ all 12 clips real Pexels |
| Visual hooks (real stock) | 8 | 8 | ✅ 12/12 real |
| Visual variance / anti-repeat | 8 | 8 | ✅ 12 unique identities, no consecutive repeats |
| **Context-relevant visual variance** | 2 | 8 | ❌ no `video_keywords` set; agent picked English concepts but they don't get matched to segments by the verifier |
| Cuts / visual pacing | 3 | 5 | ⚠️ cuts_per_second=0.08 vs ideal 0.12-0.55 — 11 cuts over 135s, too few |
| BG music quality | 4 | 8 | ⚠️ music.mood not tagged on the timeline; verifier doesn't read the mood I passed to `music.assign` |
| SFX punctuation | 5 | 6 | ⚠️ same `digital_glitch__glitch 1.wav` reused 4×; `swish_sweep__long whoosh 1.wav` repeated — SFX index doesn't rotate variants enough |
| **Sticker design** | 0 | 8 | ❌ NO stickers composited — agent didn't add any GIFs/stickers |
| **Caption quality** | 0 | 6 | ❌ **HARD FAIL** — captions file absent |
| Voiceover quality | 2 | 6 | ⚠️ voice_ids not reported (this is audio-to-video, no TTS voice — verifier penalises because the timeline doesn't declare the source as voiceover track) |
| **Audio mix quality** | 1 | 5 | ❌ **HARD FAIL** — peak=-0.0 dBFS clipping |
| Section composition | 2 | 8 | ⚠️ no section map (no hook/body/CTA markers) |
| Visual hierarchy | 0 | 5 | ❌ no title cards |
| Platform optimization | 2 | 5 | ⚠️ aspect_ratio not in manifest (verifier expects it declared) |
| Timeline editor | 2 | 4 | ⚠️ repetitive SFX |

**Production score: 49 / 100 — Grade D (target: B ≥ 75)**

---

## UX Findings (ranked by leverage)

### 🔴 P0 — Fix immediately (3 issues, ~3 hr of work)

#### 1. A2V trajectory missing `captions.generate_ass`

The agentic A2V recipe in `initialize` says:
> `transcribe → srt.prepare → timeline.build → timeline.add_segment(s) → captions.generate_ass → broll.fetch → background.assign → music.assign → sfx.assign → timeline.validate → timeline.render`

But the *alternate* recipe listed right above it omits captions:
> `transcribe → srt.prepare → srt.to_timeline → segment.analyze → [AGENT generates English keywords] → broll.fetch → music.assign → captions → timeline.render`

Both call out captions, but neither makes it a required step with `required:`-like visibility. As a fresh-agent I read "captions" as "we have captions (the SRT)" — I did not realise `captions.generate_ass` writes a separate burned-in subtitle file that the renderer overlays.

**Fix:** Either
- (a) auto-burn SRT/word-SRT in `timeline.render` when present in the timeline (without requiring a separate `captions.generate_ass` call), OR
- (b) rename the trajectory so it makes captions explicit:

```
A2V (required steps marked with ★):
1. ★ transcribe
2. ★ srt.prepare
3. ★ srt.to_timeline
4. ★ captions.generate_ass — MUST be called to enable burned-in captions
5. segment.analyze
6. broll.fetch
7. music.assign
8. sfx.assign
9. timeline.validate
10. timeline.render
```

I prefer (a) because the SRT is already on disk and the agent should not need to remember an extra call. Implementation: in `timeline.render` (crates/openscript-ffmpeg/src/filter_graph.rs), if `timeline.assets.captions` is unset but a word-level SRT exists, fall back to burning the SRT directly via the `subtitles=` filter (Phase 119b already implemented this for ASS — extend to SRT).

#### 2. Audio clipping at peak -0.0 dBFS

`sfx.auto_assign` with `gain_db=-10` plus original dialogue audio at full level plus music at -12 dB produces a mix that hits 0 dBFS on peaks. The loudnorm filter is at `I=-16` which normalizes LUFS but **does not limit sample peaks** — it just pulls the integrated loudness down. We already have alimiter at `limit=0.63` (post Phase 121) but the failure persists because alimiter applies after amix and the per-source gains haven't been rebalanced.

**Fix:** In `crates/openscript-ffmpeg/src/multilayer_render.rs` and the equivalent filter-graph render path, lower `music_vol` from `gain_db=-12` linear → effective `-14` for A2V (no TTS voice competing, so music only needs to sit below the dialogue), AND/OR add a per-source gain scale factor when timeline has dialogue track but no voiceover track (A2V's case).

#### 3. A2V trajectory needs stickers/GIFs for engagement

The video is 135.7s — way over the "15s" platform minimum. Without any on-screen markers (stickers, meme cuts, title cards), it reads as a slideshow of stock footage. For long-form vertical content, having visual hierarchy anchors every 5-10 seconds is essential.

**Fix:** Add `sticker.load_preset` or `gif.search` to the A2V recipe's *automatic* path. The simplest implementation: when `broll.fetch` auto-places clips, automatically also auto-place one GIPHY reaction sticker per segment using the segment's keywords. Or, expose a one-call `a2v.enhance` orchestrator that adds stickers + meme_brolls + section markers in a single pass. YAGNI note: do NOT add this as a default; add it as an opt-in to keep the simple-trajectory path lean.

### 🟡 P1 — Fix in next sprint (5 issues, ~6 hr)

#### 4. `library.search` returns 0 results for `mood=dramatic` even though 393 entries match the mood

When I searched `query="dramatic cinematic"` + `mood=dramatic`, I got **0 results**. The response said `filter_stats.filtered_by_mood: 393` — meaning 393 entries survived the mood filter, but the title/tag query "dramatic cinematic" matched nothing. This is the same root cause noted in memory #1967 — `library.search` scoring is text-match-only against title/tags. None of the 393 dramatic entries happen to contain those exact words.

**Fix:** When mood/energy filters are present, the search should relax the text match OR fall back to mood-only if text returns 0. Or surface a friendlier error: `"No results for 'dramatic cinematic' + mood=dramatic. Try removing the text query or use a more common term like 'cinematic'."`

#### 5. `broll.fetch` returns `results[N].videos` without `cached_path` per video

The response says `cached_path` at the result level, but the agent has to read `downloaded[N].path` separately. Inconsistent schema.

#### 6. SFX rotation — same glitch.wav used 4×, same whoosh 4×

Verifier flags `digital_glitch__glitch 1.wav` reused 4 times. The SFX index has 94 entries; auto_assign picks deterministically based on position. Fix: pick from a rotating pool of N candidates per role rather than always the first match. Implementation: in `crates/openscript-mcp/src/sfx_assign.rs`, add `seed = position_ms` to the picker so adjacent transitions get different files.

#### 7. `cuts_per_second=0.08` outside ideal band (0.12-0.55)

135s with 11 cuts. The agent picked `max_duration_s=4` for `srt.to_timeline`, but the resulting segments averaged ~12s each (45 SRT entries → 44 segments via pause grouping, then ~3s avg duration but visually only 11 changes were detected). The verifier is counting clip changes, not segment boundaries — they're the same thing in our pipeline.

**Fix:** Either reduce `max_duration_s` from 4s → 2.5s for longer audios, OR in `verify.production` count cuts as `segment_count / duration_s` rather than `broll_event_count / duration_s` since segments define the intended rhythm.

#### 8. Missing `aspect_ratio` in render manifest

Verifier complains: `aspect_ratio not set in manifest`. The render manifest is generated by `timeline.render` and doesn't include `aspect_ratio` even though the timeline does. Trivial fix: include `aspect_ratio: timeline.meta.aspect` in the manifest.

### 🟢 P2 / YAGNI — Defer or remove

#### 9. SFX `editorial_role` system

For A2V the role-based picker (`hook`, `transition`, `highlight`, `outro`) is a reasonable abstraction, but only `transition` gets exercised for A2V (the rest are anchor points). The `highlight` role fires only when the agent explicitly assigns, which it didn't in my run. Consider removing `highlight` from the auto-place path and making it explicit-only.

#### 10. `broll.keywords` listed but not on the agentic recipe

There's a `broll.keywords` tool that's listed in AGENT_GUIDE tool catalog but not in the A2V trajectory. As a fresh-agent I missed the opportunity to call it (it would auto-generate English keywords from Hinglish). **Either** add it to the trajectory or delete it. Don't leave orphans.

#### 11. `video_keywords` empty → context_relevance score = 2/8

The trajectory doesn't tell the agent to set `video_keywords` on the timeline. The verifier wants `[brain, neuroscience, protest, india]` etc. — the agent has to know to populate these. Easiest fix: in `srt.to_timeline` auto-extract the top 5 nouns from the SRT text (English + transliterated Hinglish) and store them as `timeline.meta.video_keywords`. Memory #1392 mentions topic-aware anchor banks for stock_signal.rs but doesn't auto-populate video_keywords.

---

## What to keep (do NOT change)

- **Audio from input source is preserved** — A2V correctly used the original mp3 as the voiceover track (no TTS regeneration). ✅
- **12 unique Pexels clips** — every segment got a different clip. ✅
- **No placeholder/procedural fallbacks** — all 12 clips are real stock. ✅
- **Music ducked correctly** — gain_db=-14 + ducking=true honored. ✅
- **SFX distributed across the timeline** — 46 placements at boundaries. ✅
- **Background black-video generation for audio-only source** — timeline.render correctly generated a black bg from the audio (good A2V ergonomics). ✅

---

## Recommended Fix Priority

| Priority | Issue | Effort | Expected score gain |
|---|---|---|---|
| 🔴 P0 | Auto-burn SRT when captions absent in timeline.render | 1 hr | +6 |
| 🔴 P0 | Fix A2V audio mixing (lower music vol + per-source rebalance) | 1 hr | +4 |
| 🔴 P0 | Add captions.generate_ass to required A2V recipe | 30 min | +0 (enables P0.1) |
| 🟡 P1 | library.search fallback when text+filter returns 0 | 30 min | +UX |
| 🟡 P1 | SFX rotation with seed | 1 hr | +1 |
| 🟡 P1 | max_duration_s default 2.5s for long audio | 30 min | +2 |
| 🟡 P1 | Render manifest includes aspect_ratio | 15 min | +0.5 |
| 🟢 P2 | Auto-populate video_keywords from SRT | 2 hr | +3 |

**Expected post-fix score: 49 → ~75 (Grade C+)** — passing the B threshold will require deeper work on context relevance and sticker auto-placement (out of scope for this audit).

---

## Files for inspection

- Rendered artifact: `output/audit_v3_a2v.mp4` (25 MB)
- Timeline: `/home/ishanp/Downloads/audit_v3_render_audio.timeline.json`
- SRT: `/home/ishanp/Downloads/audit_v3_render_audio.srt`
- Background clips: `mcp/assets/broll_cache/{government_building_corruption,social_media_phone_screen,protest_crowd_poor_people,...}.mp4`
- Music: `mcp/assets/music_cache/Documentary_Cinematic_Inspirational_by_Infraction_Copyright_Free_Music_Freefall.mp3`
- This report: `artifacts/fresh_agent_a2v_audit.md`

---

## Verdict

The trajectory **succeeds in execution** but **fails in production-quality gating**. The harness is the right shape — atomic tools, deterministic pipeline, hard-fail gating — but the gap between "I followed the recipe" and "the video meets platform standards" is too wide. The single biggest fix is **auto-burning captions in `timeline.render` when present in timeline assets**, which alone will resolve the highest-impact hard fail and lift the grade above the B threshold once audio mixing is also addressed.
