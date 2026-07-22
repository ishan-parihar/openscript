# OpenScript Agent Guide — Golden Trajectory for Video Creation

## Tool Taxonomy (84 tools)

> **Always start a new environment with `system.doctor` (or `system.capabilities`).**
> `system.doctor` returns `ready_for_production` + `next_actions` for cold-start.
> `system.capabilities` reports ffmpeg, Kokoro, Parakeet, Pexels/GIPHY/Pixabay,
> music/SFX (including `music_production` pack), yt-dlp, HyperFrames, and the
> LLM/vision cascade. Do not assume TTS works until `kokoro.available` is true.
>
> **User config lives at `~/.openscript/config.json`** (mode 0600). Manage it with
> `system.config.get` / `system.config.set` or `scripts/setup_openscript_config.sh`.
> Env vars still override. **Never put API keys in the git repo.**

### 1. SCRIPT CREATION (5 tools)
Create a video from scratch — define scenes, speakers, backgrounds.

| Tool | When to use |
|------|------------|
| `script.parse` | Validate a script JSON before production |
| `script.generate_voices` | TTS only (when fine-tuning) |
| `script.build_captions` | ASS captions only (when fine-tuning) |
| `script.to_timeline` | Build timeline from script (TTS + captions + backgrounds) |
| `script.to_video` | **ONE-CALL**: script JSON → finished MP4 |

### 2. VOICE & TTS (5 tools)
Generate speech, manage voice profiles.

| Tool | When to use |
|------|------------|
| `tts.generate` | Generate a single TTS audio clip |
| `tts.estimate_duration` | Estimate audio length before generation |
| `tts.preview` | Preview voice profile without generating |
| `voice.profile.list` | List available voices |
| `voice.profile.add` | Register a new voice profile |

### 3. CAPTIONS & SUBTITLES (2 tools)
Generate word-synced captions from TTS audio.

| Tool | When to use |
|------|------------|
| `script.generate_voices` | Generate all TTS + whisper word timestamps |
| `script.build_captions` | Build ASS subtitle file from word timings |

### 4. BACKGROUNDS & B-ROLL (6 tools)
Source background video clips.

| Tool | When to use |
|------|------------|
| `background.fetch` | Download a background clip (Pexels → YouTube → fallback) |
| `background.assign` | Assign clips to scenes by cadence |
| `broll.suggest` | Suggest b-roll insertion points from EDL |
| `broll.fetch` | Search Pexels for b-roll by concept |
| `broll.assign` | Place a b-roll clip on the timeline |
| `broll.director` | AI director: auto-suggest + fetch + assign b-roll |

### 5. MEDIA & STICKERS (5 tools)
Search and download images, GIFs, and stock footage.

| Tool | When to use |
|------|------------|
| `media.search` | Search Pexels Images + Openverse for PNG overlays |
| `gif.search` | Search GIPHY for transparent animated stickers |
| `stock.search` | Search Pixabay for music/videos without downloading |
| `stock.fetch` | Download stock music/videos from Pixabay |
| `sticker.load_preset` | Load an SVG puppet preset for animated stickers |

### 6. VIDEO RENDERING (4 tools)
Render the final video.

| Tool | When to use |
|------|------------|
| `composition.render` | Unified renderer (HF native / Remotion interop / legacy) |
| `timeline.render` | Render NLE timeline (existing footage editing) |
| `hf.render` | Render HyperFrames HTML composition to MP4 |
| `hf.classify` | Classify Remotion source for HF native vs interop |

### 7. TIMELINE MANAGEMENT (8 tools)
Inspect, validate, and modify timelines.

| Tool | When to use |
|------|------------|
| `timeline.build` | Create a fresh EDL v2 timeline |
| `timeline.preview` | Get a readable summary of timeline contents |
| `timeline.validate` | Check for structural errors |
| `timeline.load` | Load and inspect a timeline JSON |
| `timeline.add_segment` | Add a video segment |
| `timeline.add_track_event` | Add an event to any track |
| `timeline.diff` | Compare two timeline versions |
| `timeline.autofill_broll` | Auto-fill b-roll from segment captions |

### 8. QUALITY & VERIFICATION (4 tools)
Post-render quality checks.

| Tool | When to use |
|------|------------|
| `verify.audio` | Technical: audio levels, silence, sample rate |
| `verify.captions` | Caption timing checks |
| `verify.render` | **Technical only** — duration/aspect/file integrity (score 100 ≠ beautiful video) |
| `verify.production` | **Production KPI v3** — hard-fails majority procedural, missing visual hooks, parade music on calm/focus. Grade A–F. Optional `vision_rescore=true` |
| `director.run` | **ONE-SHOT** preflight + parse + to_video + verify.production (cold agents) |

### 8b. LLM & VISION (3 tools)
Local GGUF + OpenRouter free multimodal cascade for director reasoning and clip QA.

| Tool | When to use |
|------|------------|
| `llm.complete` | Text completion via local Ollama `qwen3.5-4b` (GGUF at `~/Downloads/Qwen3.5-4B-Q4_K_M.gguf`) → OpenRouter `google/gemma-4-31b-it:free` → `nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free` |
| `vision.analyze_clip` | Extract a frame + describe setting/time-of-day/subjects (multimodal OpenRouter free models; local text fallback) |
| `vision.score_clip` | Score stock-clip relevance vs scene dialogue + `video_keywords` (0–1 + match/reason) |

**Setup**
- Write config: `bash scripts/setup_openscript_config.sh` (creates `~/.openscript/config.json`)
- Local: `ollama serve` + `bash scripts/import_local_gguf.sh` (Unsloth Qwen3.5-4B GGUF)
- Vision free fallbacks: `api_keys.openrouter` in config or `OPENROUTER_API_KEY`
- Inspect: `system.config.get` / `system.capabilities` → `openscript_config` + `llm`
- Force backend: `llm.complete` with `backend: "local"` or `"openrouter"`

### Stock B-roll signal vs noise (Phase CM)

Multi-broll selection (`script.to_video`) now gates **signal vs noise**:

1. **Query sanitizer** (`stock_signal::build_scene_stock_query`) — strips listicle noise (`Swap one`, etc.), keeps visual nouns + `video_keywords`, attaches a **context-matched** visual anchor (phone → smartphone lock screen, not a fixed rotation).
2. **Lexical title rank** — YouTube candidates ranked by title overlap with signal tokens; low-overlap viral noise is deprioritized (`OPENSCRIPT_STOCK_MIN_LEXICAL`, default `0.12`).
3. **Cover-crop geometry** — `scale=W:H:force_original_aspect_ratio=increase,crop=W:H,setsar=1` (no stretch). Old `scale=W:H,crop=W:H` left non-square SAR and distorted 16:9 stock inside 9:16 frames.
4. **Geometry probe** — rejects outputs with non-square SAR or wrong display aspect before accept.

Set rich `video_keywords` on every script. Prefer concrete scene nouns (phone, coffee, notebook) over abstract dialogue.

#### Production KPI baseline v2 (`verify.production` + `render_manifest.json`)

**Architecture:** quality is computed in `openscript_core::production_quality` from a
`RenderManifest` (authoritative multi-broll / stickers / memes / sections) **plus**
timeline editor utilization. `script.to_video` writes `render_manifest.json` next to
the timeline so re-validation does not lose multi-scene stock paths.

| Grade | Score | Meaning |
|-------|------:|---------|
| **A** | 85–100 | Delivery-ready director stack |
| **B** | 70–84 | Acceptable social short |
| **C** | 55–69 | Watchable draft |
| **D/F** | <55 | Not production |

| Dimension | Weight | What it measures |
|-----------|-------:|------------------|
| `video_source_quality` | 14 | Pexels > YouTube > local stock > unknown > **procedural=0** |
| **`visual_repetition`** | **16** | **Content-hash / video-id uniqueness** — same YT video under different paths is a HARD fail |
| **`context_relevance`** | **12** | Search query vs scene text + `video_keywords` (topic-aware variance) |
| `cuts_pacing` | 8 | **cuts/sec** band (ideal **0.12–0.55**/s) using identity transitions |
| `music_variance` | 10 | Real bed + ducking + mood/energy tags + audible gain |
| `sticker_design` | 10 | Scale **0.20–0.45**, caption-safe position, uniqueness, GIF motion |
| `section_composition` | 10 | Hook/body/cta text, **title cards**, meme placement in body |
| `speech_audio` | 8 | Dialogue + loudness |
| `captions` | 8 | ASS/SRT present |
| `timeline_editor` | 4 | Multi-track use, unique visuals, gaps/overlaps, SFX |

**Critical:** path uniqueness is **not** enough. v2.1 fingerprints clip bytes + YouTube IDs so “5 cuts of the same night-phone video” scores `visual_repetition=0` with a **REPETITION** hard fail.

Also returns: `cuts_per_second`, `video_source_mix`, `timeline_editor` findings, per-clip `content_hash` / `video_id` / `search_query` on the render manifest.

**Fetch path:** multi-broll uses `ytsearchN` + per-ID download + content-hash reject + per-scene query diversifiers (sunrise / coffee desk / notebook / …).

`script.to_video` embeds full `production_quality` report. Status becomes
`rendered_below_production_grade` / `rendered_production_fail` when KPIs fail —
**never treat `verify.render=100` as ship quality.**

---

## Workflows: Agent-Orchestrated Video Creation

The three supported workflows are NOT hardcoded pipelines — they are **golden trajectories** that tell the agent which atomic tools to call and in what order. The **agent** decides what tools to call, in what order, with what parameters, adapting to each situation.

### Trajectory A — From-Scratch Video

**Input:** A topic/script the agent creates.
**Output:** A complete video with TTS voiceover, backgrounds, captions, music, SFX.

```
1. system.capabilities          → verify TTS + Pexels + render work
2. script.schema                → discover the script JSON schema
3. script.parse                 → validate the agent's script
4. script.to_video              → ONE-CALL: TTS + captions + backgrounds + stickers + music + render
5. verify.production            → score the output
```

**Agent intelligence points:**
- Agent chooses speaker voices, scene descriptions, background keywords
- Agent decides caption style (word_highlight, sentence_fade, karaoke_fill, subtitle_rail)
- Agent selects music mood/energy based on content tone
- Agent can retry with different presets if verify.production scores low

### Trajectory B — NLE Editing of Existing Footage

**Input:** An existing video to edit (cut, caption, b-roll, music).
**Output:** An edited reel with optimized segments, b-roll overlays, music, captions.

```
1. system.capabilities          → verify transcription + render work
2. reelize.brief                → analyze footage: segments, timing, word counts, b-roll concepts
3. (AGENT DECIDES)              → read the brief, choose which segments to keep/edit
4. reelize.direct               → execute AI-directed production with chosen segments + b-roll + music
5. verify.production            → score the output
```

**Agent intelligence points:**
- Agent analyzes the brief to decide editorial strategy (tight cuts vs relaxed pacing)
- Agent can remove low-energy segments to increase retention
- Agent selects b-roll that complements (not repeats) the original footage
- Agent uses vision.score_clip to verify b-roll relevance before rendering
- Agent loops: render → verify → adjust → re-render until quality threshold met

### Trajectory C — From Audio File

**Input:** An existing audio file (podcast, speech, voice note).
**Output:** A complete video reel with b-roll, captions, music, SFX.

```
1. system.capabilities          → verify transcription + render work
2. transcribe                   → Whisper/Nemotron transcription → SRT
3. srt.prepare                  → group words into readable caption segments
4. timeline.build               → create empty multi-track timeline from audio
5. timeline.add_segment         → add segments from SRT timestamps + text
6. broll.director               → AI-directed b-roll: search Pexels + download + assign
7. library.search               → find background music matching content tone
8. music.assign                 → assign music with ducking under speech
9. sfx.index                    → (run once) build SFX library index
10. sfx.assign                   → hook SFX at 0ms, transitions between segments
10. timeline.validate            → check for timing errors
11. timeline.render              → final multi-track render
12. verify.production            → score the output
```

**Agent intelligence points:**
- Agent chooses transcription engine (whisper for 99 langs, nemotron-onnx for cache-aware)
- Agent decides which segments to keep based on transcription content
- Agent selects b-roll concepts based on what the speaker is discussing
- Agent picks music mood/energy from audio content analysis
- Agent can skip b-roll if content is screen-recorded (not applicable)
- Agent can add voiceover.generate for intro/outro narration

### Trajectory D — From Existing Video (Re-edit for Retention)

**Input:** An existing video to re-edit for attention retention.
**Output:** An optimized reel with b-roll overlays, music, captions.

```
1. system.capabilities          → verify tools work
2. transcribe                   → transcribe existing video audio
3. srt.prepare                  → group into caption segments
4. reelize.brief                → analyze footage: segments, timing, word counts, b-roll concepts
5. (AGENT DECIDES)              → read the brief, choose which segments to keep/edit
6. timeline.build               → create timeline from source video
7. timeline.add_segment         → add curated segments from brief analysis
8. broll.director               → search + download + assign contextually relevant clips
9. library.search               → find music matching content energy
10. music.assign                 → assign music with ducking
11. sfx.index                    → (run once) build SFX library index
12. sfx.assign                   → attention hooks at segment transitions
13. timeline.validate            → check for errors
14. timeline.render              → final render
15. verify.production            → score and optionally re-edit
```

**Agent intelligence points:**
- Agent analyzes the brief to decide editorial strategy (tight cuts vs relaxed pacing)
- Agent can remove low-energy segments to increase retention
- Agent selects b-roll that complements (not repeats) the original footage
- Agent can add tts.commentary for transitions between segments
- Agent uses vision.score_clip to verify b-roll relevance before rendering
- Agent loops: render → verify → adjust → re-render until quality threshold met

### Trajectory E — HyperFrames Composition

**Input:** A HyperFrames project (HTML + GSAP).
**Output:** A rendered video from composition.

```
1. hf.classify                  → auto-classify the project structure
2. hf.lint                      → check for errors
3. hf.validate                  → validate composition
4. hf.snapshot                  → capture frame strips
5. hf.render                    → render to MP4
```

### Key Principle: Agent Orchestration, Not Hardcoded Pipelines

These workflows are **documentation**, not code. The agent:
1. Reads the workflow to understand the tool sequence
2. Adapts the sequence based on content and context
3. Makes intelligent decisions at each step
4. Can skip, retry, or substitute steps as needed
5. Uses verify.production to iterate until quality is acceptable

**If the sequence were deterministic, it should just be a CLI command.** The entire point of MCP is that the agent decides what tools to call.
