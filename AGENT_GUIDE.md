# Agent Guide — OpenScript MCP Tool Catalog

> This document is the canonical reference for AI agents using OpenScript's MCP tools.
> Every agent MUST read this before calling any tool.

---

## Tool Families (28 families, 104 tools)

| Family | Count | Tools |
|--------|-------|-------|
| timeline | 11 | build, load, validate, add_segment, add_track_event, diff, preview, inspect, autofill_broll, render, upgrade |
| script | 6 | schema, parse, generate_voices, build_captions, to_timeline, to_video |
| hf | 6 | classify, lint, validate, snapshot, render |
| tts | 4 | generate, estimate_duration, preview, commentary |
| music | 4 | index, search, assign, ducking.plan |
| broll | 9 | suggest, fetch, assign, plan, keywords, validate_keywords, repair, auto, probe |
| srt | 4 | read, prepare, apply_edit, to_timeline |
| voice | 3 | profile.add, profile.list, profile.remove |
| sfx | 3 | index, search, assign |
| verify | 4 | audio, captions, render, production |
| library | 3 | search, download, build |
| overlay | 2 | generate, assign |
| background | 3 | fetch, assign, search |
| asset | 6 | library.status, ingest, probe, rate, import, search |
| sticker | 7 | presets, load_preset, render, keywords, validate_keywords, auto, auto_assign |
| stock | 2 | fetch, search |
| youtube | 2 | search, download |
| media | 2 | search, download |
| gif | 2 | search, download |
| system | 2 | config.get, config.set, capabilities, doctor |
| help | 1 | tool |
| captions | 1 | generate_ass |
| transcribe | 1 | transcribe |
| edl | 1 | build |
| render | 1 | render |
| voiceover | 1 | generate |
| voices | 1 | list |
| composition | 1 | render |

---

## Trajectory E — Asset Development (user-curated footage library)

**Separate from the generation pipeline.** `asset.*` tools WRITE the library index
(`mcp/assets/user_library_index.json`; media lives in gitignored
`mcp/assets/user_library/`); generation only READS it as its Tier-1 footage source.

```
asset.library.status  →  asset.ingest  (index local footage: ffprobe + content-hash dedup + auto-keywords)
                    →  asset.probe    (curation pool: Pexels + Pixabay + YouTube candidates WITH thumbnails, no download)
                    →  asset.rate     (classify: relevance 0-1 per keyword, quality 0-5, mood/energy/motion, approved/rejected)
                    →  asset.import   (download approved external candidates into the library)
                    →  asset.search   (consumption side — generation Tier 1; approved + quality >= 3.0 only)
```

Only `approved` assets with `quality_rating >= 3.0` are eligible for generation.
YouTube is always available to `asset.probe`/`broll.probe` (acquisition engine),
independent of the generation opt-in flag.

### Background acquisition fallback discipline (scene_media)

All scene backgrounds flow through ONE chain (`scene_media::fetch_scene_background`):

```
user_library → Pexels → Pixabay → YouTube (opt-in only) → fallback_pool → procedural (NEVER silent)
```

- Every tier attempt is recorded in the response's `exhausted` array — "why procedural" is always answerable.
- YouTube is opt-in for generation: `background.enable_youtube: true` in the script,
  or `OPENSCRIPT_YT_FOR_GENERATION=1`. It requires a stricter lexical bar AND a passing
  vision frame-gate (non-fail-open).
- Procedural is the last resort; when it ships without `OPENSCRIPT_ALLOW_PROCEDURAL=1`,
  `script.to_video` returns status `rendered_with_procedural` (loud warning + production-score penalty).

---

## Trajectory A — From-Scratch Video Creation (Golden Path)

```
1. system.doctor          — Check production readiness
2. script.parse           — Validate script JSON
3. script.to_video        — ONE-CALL: script → MP4
   (handles TTS, captions, b-roll, stickers, music, rendering)
```

**Manual control (optional):**
```
script.generate_voices → script.build_captions → background.fetch → script.to_timeline → composition.render
```

---

## Trajectory B — NLE Editing (Existing Footage)

```
1. transcribe             — Word-level SRT from video
2. srt.prepare            — Group words into caption segments
3. timeline.build         — Create multi-track timeline
4. timeline.add_segment   — Add segments to timeline
5. broll.fetch            — Search Pexels for b-roll (AGENT generates English keywords)
6. music.assign           — Add background music
7. captions.generate_ass  — Generate styled captions
8. timeline.validate      — Check for errors
9. timeline.render        — Render final video
```

---

## Trajectory C — Audio to Video (A2V)

**Agentic pipeline (RECOMMENDED):**
```
0. broll.auto             — ONE-CALL FINALIZED: runs the ENTIRE A2V trajectory for you
                           (segment.analyze → broll.keywords → broll.validate_keywords →
                           broll.fetch → timeline.validate → broll.repair loop until 0 gaps,
                           then sticker.auto agentic GIPHY stickers → captions.generate_ass).
                           Feed it an SRT + audio; get back a fully covered timeline with
                           stickers and styled captions, ready for music + render.
1. transcribe             — Hinglish SRT from audio
2. srt.prepare            — Group words into caption segments
3. segment.analyze        — Sentence-aware segments (2–6s, docs/SEGMENTATION_ARCHITECTURE.md)
4. broll.keywords         — STAGE 1 (draft): agent translates Hinglish → English visual keywords
5. broll.validate_keywords— STAGE 2 (relevance-validation): agent scores REAL Pexels candidates
                           (video names/durations) against the spoken caption → final keywords + best video
6. broll.fetch            — Download + auto-place the validated clips on the timeline
7. sticker.auto           — ONE-CALL sticker pipeline (parallel to broll): segment → sticker.keywords
                           (agent drafts INTENT + EMPHATIC keywords) → sticker.validate_keywords
                           (GIPHY relevance gate: only approved stickers) → download → place on the
                           Stickers track (spacing gate + position cycling, positioned PiP)
8. music.assign           — Add background music
9. captions.generate_ass  — Generate styled captions (word_highlight ASS, registered in timeline)
10. timeline.validate     — Check for errors (segmentation bounds + BROLL_GAP coverage)
11. timeline.render       — Render final video (b-roll full-frame + stickers as positioned PiP overlays)
```

**Key:** The keyword generation is AGENTIC, not deterministic — it never relies on a hardcoded
Hinglish dictionary. Stage 1 drafts keywords from the spoken meaning; Stage 2 validates those
drafts against the stock footage that actually exists (video names + durations), so a draft that
Pexels can't serve is corrected before download. `timeline.preview` is the timeline-viewer context
layer: it returns the full composition layer stack (bottom→top with per-event concept/asset/timing),
the b-roll coverage gaps, and the used clip ids — call it to understand the whole operational flow
before and after repair.

---

## Trajectory D — Video to Video (V2V)

**Agentic pipeline (RECOMMENDED):**
```
1. transcribe             — Hinglish SRT from video
2. srt.prepare            — Group words into caption segments
3. srt.to_timeline        — Create timeline with segments
4. segment.analyze        — Get clean segment data
5. [AGENT generates English keywords from Hinglish content]
6. broll.fetch            — Search Pexels with agent-generated English keywords
7. music.assign           — Add background music
8. captions.generate_ass  — Generate styled captions
9. timeline.validate      — Check for errors
10. timeline.render       — Render final video
```

**Alternative (for editorial control):**
```
1. reelize.brief          — Analyze footage, get segment data
2. [AGENT decides which segments to keep, what b-roll to add]
3. reelize.direct         — Execute agent's creative instructions
4. verify.production      — Score output quality
```

---

## Media Search & Download

| Type | Search | Download | Assign |
|------|--------|----------|--------|
| B-roll | `broll.auto` (one-call) or `broll.keywords` → `broll.validate_keywords` → `broll.fetch` | `broll.fetch(download=true)` | `broll.assign` / `broll.repair` (gap healing) |
| Images | `media.search` | `media.download` | `overlay.assign` |
| GIFs / stickers | `sticker.keywords` (intent+emphatic) → `sticker.validate_keywords` (GIPHY relevance gate) → `gif.search` | `gif.download` | `sticker.auto_assign` / `sticker.auto` → Stickers track (positioned PiP) |
| Music | `library.search` | `library.download` | `music.assign` |
| SFX | `sfx.search` | — | `sfx.assign` |
| YouTube | `youtube.search` | `youtube.download` | — |

---

## Music Sources

| Source | Tracks | When to Use |
|--------|--------|-------------|
| `music.search` | 20 local stock | Fast, limited selection |
| `library.search` | 500+ YouTube-scraped | Real licensed music (run `library.build` first) |
| `stock.search` | Pixabay API | Real music + video (needs `PIXABAY_API_KEY`) |

---

## Quality Checks

```
verify.audio       — Audio quality (loudness, dialogue, silence)
verify.captions    — Caption sync (coverage, gaps, readability)
verify.render      — Technical integrity (duration, aspect, file size)
verify.production  — Full production quality (stock, music, captions)
```

**B-roll non-redundancy (Phase 143):**
The same stock clip must NEVER appear twice in one video — dedup happens at the
Pexels **video id** level, not just the cache path. A clip cached under two
query slugs (`crowd_people_aavaaz_35340082.mp4` vs `crowd_people_yah_35340082.mp4`)
is still the same footage and is rejected: `broll.fetch`/`broll.repair` exclude
already-used ids via the timeline (`used_broll_video_ids`), and `timeline.validate`
flags `BROLL_REPEAT` for both exact-path and same-id-different-slug duplicates.
`broll.fetch` warns when a concept's Pexels pool is exhausted and used ids must
be re-used. `background.fetch` accepts `used_video_ids` (accumulate the
`pexels_id` values from prior calls) so the golden path also never re-fetches
the same clip under a different query.

**B-roll coverage loop-closure (Phase A+B of docs/SEGMENTATION_UPGRADE_PLAN.md):**
Clips now play exactly ONCE — the renderer never loops to fill a short clip's window.
If `verify.production` (or `timeline.validate`) returns `broll_gaps`, each entry names the
segment, the required vs available duration, and an action directive. The preferred closure is
**`broll.repair`** — one call that re-runs the whole agentic loop for exactly those gaps, with the
entire timeline as context (layer stack, all segments, already-covered concepts, already-used
clips + gap timestamps):
1. Call `broll.repair(timeline_path, max_segments=N)` — it drafts fresh keywords (agent),
   searches Pexels, validates candidates against the spoken caption (agent), downloads the
   chosen clip, and replaces the event + asset. Non-looping (clip must cover the window) and
   non-redundant (already-used Pexels ids are excluded).
2. Re-run `timeline.preview` / `timeline.validate` — re-check `broll_gaps` is empty.
3. Repeat `broll.repair` for any remaining gaps (limit `max_segments` per pass), then re-render.
   Manual fallback: re-run `broll.keywords` for that segment's caption, then `broll.fetch` with
   those keywords, `download_n >= ceil(segments/concepts)` for distinct clips — never accept
   the same clip re-styled with a different zoom/pan as "new" footage.

**Composition audit + segmentation enforcement (Phase 134):**
`verify.production` now returns a `composition` block — the post-generation
meta-cognitive audit: every layer (background_broll, meme_overlay, captions,
stickers, voiceover, music, sfx) with its z-order, event count, and time range,
plus `present_order` and `missing`. Read it before judging a render: a video
whose `missing` lists `captions` or `music` is diagnosable in one step.
Segmentation is enforced per docs/SEGMENTATION_ARCHITECTURE.md:
- `timeline.validate` returns `DURATION:` errors when any segment ends past the
  source media (the master clock) — SRT tail hallucination / trailing silence
  must never produce b-roll windows past the audio end (the "audio 2:15, video
  2:41" black-tail regression). `srt.to_timeline` and `segment.analyze` clamp
  segments/scenes at the source duration automatically; the render's `-shortest`
  caps output at the audio end as a final backstop.
- `timeline.validate` returns `SEGMENTATION:` errors when any segment is outside
  [2.0s, 6.0s] — the short-form retention bounds.
- `verify.production` scores `segmentation_pacing` (max 8): long cuts (>6s)
  bleed attention and are penalized with a "split at the longest internal pause"
  directive; sub-min cuts (<2s) are penalized with a "merge with adjacent"
  directive.
- `visual_repetition` emits an `ANTI-REPEAT:` finding with the distinct-clip
  ratio and a `broll.fetch download_n` directive when the same clip pool is
  stretched over too many cuts.

---

## Discovery Tools

```
system.doctor       — Cold-start production readiness
system.capabilities — Probe available subsystems
help.tool           — Natural-language tool discovery
```

---

## Deprecated Tools

| Tool | Status | Replacement |
|------|--------|-------------|
| `audio.to_video` | **DELETED** | Use atomic pipeline: transcribe → srt.prepare → srt.to_timeline → segment.analyze → broll.fetch → timeline.render |
| `reelize.timeline` | **DELETED** | Use atomic pipeline: transcribe → srt.prepare → srt.to_timeline → segment.analyze → broll.fetch → timeline.render |
| `broll.director` | **DELETED** | Use `broll.plan` + agent keywords + `broll.fetch` |
| `music.search` | **Re-added (Phase 107)** | Queries music_index.json by mood/energy/keyword/tags, returns local paths |

---

## Important Notes

1. **Hinglish content:** The agent MUST generate English keywords from Hinglish transcripts. Do NOT use raw Hinglish words for Pexels search — they return garbage clips.

2. **Always validate before render:** Call `timeline.validate` before `timeline.render` to catch errors early.

3. **Agent is the translation layer:** For non-English content, the AI agent reads the transcript, understands context, and generates English visual keywords. The pipeline provides data; the agent decides.

4. **Never hardcode API keys:** All API keys come from env vars: `PEXELS_API_KEY`, `GIPHY_API_KEY`, `PIXABAY_API_KEY`.

5. **Push after every iteration:** Every code change MUST end with `git commit` AND `git push origin main`.
