# Agent Guide — OpenScript MCP Tool Catalog

> This document is the canonical reference for AI agents using OpenScript's MCP tools.
> Every agent MUST read this before calling any tool.

---

## Tool Families (27 families, 89 tools)

| Family | Count | Tools |
|--------|-------|-------|
| timeline | 11 | build, load, validate, add_segment, add_track_event, diff, preview, inspect, autofill_broll, render, upgrade |
| script | 6 | schema, parse, generate_voices, build_captions, to_timeline, to_video |
| hf | 5 | classify, lint, validate, snapshot, render |
| tts | 4 | generate, estimate_duration, preview, commentary |
| music | 4 | index, search, assign, ducking.plan |
| broll | 4 | suggest, fetch, assign, plan |
| srt | 4 | read, prepare, apply_edit, to_timeline |
| voice | 3 | profile.add, profile.list, profile.remove |
| sfx | 3 | index, search, assign |
| verify | 4 | audio, captions, render, production |
| library | 3 | search, download, build |
| overlay | 2 | generate, assign |
| background | 3 | fetch, assign, search |
| sticker | 2 | presets, load_preset, render |
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
1. transcribe             — Hinglish SRT from audio
2. srt.prepare            — Group words into caption segments
3. srt.to_timeline        — Create timeline with segments
4. segment.analyze        — Get clean segment data (text + timestamps + duration)
5. [AGENT generates English keywords from Hinglish content]
6. broll.fetch            — Search Pexels with agent-generated English keywords
7. music.assign           — Add background music
8. captions.generate_ass  — Generate styled captions
9. timeline.validate      — Check for errors
10. timeline.render       — Render final video
```

**Key:** The AI agent is the translation layer between Hinglish content and English stock footage. The pipeline provides segmented transcript data; the agent generates English visual keywords; the pipeline executes search with those keywords.

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
| B-roll | `broll.fetch` | `broll.fetch(download=true)` | `broll.assign` |
| Images | `media.search` | `media.download` | `overlay.assign` |
| GIFs | `gif.search` | `gif.download` | `overlay.assign` |
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
| `broll.director` | **DEPRECATED** | Use `broll.plan` + agent keyword generation + `broll.fetch` |
| `music.search` | **DEPRECATED** | Use `library.search` |

---

## Important Notes

1. **Hinglish content:** The agent MUST generate English keywords from Hinglish transcripts. Do NOT use raw Hinglish words for Pexels search — they return garbage clips.

2. **Always validate before render:** Call `timeline.validate` before `timeline.render` to catch errors early.

3. **Agent is the translation layer:** For non-English content, the AI agent reads the transcript, understands context, and generates English visual keywords. The pipeline provides data; the agent decides.

4. **Never hardcode API keys:** All API keys come from env vars: `PEXELS_API_KEY`, `GIPHY_API_KEY`, `PIXABAY_API_KEY`.

5. **Push after every iteration:** Every code change MUST end with `git commit` AND `git push origin main`.
