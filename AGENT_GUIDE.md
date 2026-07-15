# OpenScript Agent Guide — Golden Trajectory for Video Creation

## Tool Taxonomy (80 tools)

> **Always start a new environment with `system.capabilities`.** It reports
> ffmpeg, Kokoro (real ONNX + voices.bin), Parakeet, Pexels/GIPHY/Pixabay keys,
> music/SFX indices, yt-dlp, HyperFrames, and the **LLM/vision cascade**
> (`llm.local` Ollama Qwen3.5-4B GGUF + `llm.openrouter` free multimodal).
> Do not assume TTS works until `kokoro.available` is true and `model_path`
> exists on disk.

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
| `verify.production` | **Production KPI gate** — stock visuals, real music, stickers, memes, speech, captions → grade A–F. Optional `vision_rescore=true` re-scores B-roll with vision cascade |

### 8b. LLM & VISION (3 tools)
Local GGUF + OpenRouter free multimodal cascade for director reasoning and clip QA.

| Tool | When to use |
|------|------------|
| `llm.complete` | Text completion via local Ollama `qwen3.5-4b` (GGUF at `~/Downloads/Qwen3.5-4B-Q4_K_M.gguf`) → OpenRouter `google/gemma-4-31b-it:free` → `nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free` |
| `vision.analyze_clip` | Extract a frame + describe setting/time-of-day/subjects (multimodal OpenRouter free models; local text fallback) |
| `vision.score_clip` | Score stock-clip relevance vs scene dialogue + `video_keywords` (0–1 + match/reason) |

**Setup**
- Local: `ollama serve` + `ollama run qwen3.5-4b` (or `bash scripts/import_local_gguf.sh` after downloading the Unsloth GGUF)
- Vision free fallbacks: set `OPENROUTER_API_KEY`
- Override models: `OPENSCRIPT_LOCAL_MODEL`, `OPENSCRIPT_OPENROUTER_VISION_MODEL`, `OPENSCRIPT_OPENROUTER_VISION_FALLBACK`, `OPENSCRIPT_GGUF_PATH`

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

## Golden Trajectories

### Trajectory A: From-Scratch Video (NEW — preferred for AI agents)
```
1. script.parse          → Validate the script JSON
2. script.to_video       → ONE-CALL: script → MP4 (includes TTS, captions,
                           multi-broll backgrounds, GIPHY stickers, music)
```
**That's it.** 2 tools. The `script.to_video` orchestrator handles everything:
- Kokoro TTS per scene with Parakeet force-alignment
- Word-highlight captions (4 styles)
- Multi-broll: unique Pexels stock footage per scene
- GIPHY sticker overlays per speaker
- Background music with sidechain ducking
- Timeline preview (token-efficient for agent inspection)

### Trajectory B: From-Scratch with Manual Control
```
1. script.parse             → Validate script
2. script.generate_voices   → TTS + Parakeet alignment
3. media.search             → Find PNG sticker images (optional)
4. gif.search               → Find GIPHY animated stickers (optional)
5. background.fetch         → Download per-scene backgrounds (optional)
6. script.build_captions    → Build ASS from word timings
7. script.to_timeline       → Assemble EDL v2 timeline
8. timeline.preview         → Inspect the composition
9. timeline.validate        → Check for issues
10. composition.render      → Render to MP4
11. verify.render           → Quality check
```

### Trajectory C: NLE Editing (existing footage)
```
1. transcribe               → Speech-to-text from raw video
2. srt.prepare              → Group words into phrases
3. edl.build                → Build edit decision list
4. timeline.build           → Create EDL v2 timeline
5. broll.director           → AI b-roll suggestions + downloads
6. music.search             → Find background music
7. music.assign             → Assign music with ducking
8. sfx.assign               → Add sound effects
9. timeline.render          → Render to MP4
10. verify.render           → Quality check
```

### Trajectory D: HyperFrames Composition
```
1. hf.classify              → Check if composition is HF-native or needs interop
2. hf.lint                  → Static checks
3. hf.validate              → Runtime checks in headless Chrome
4. hf.snapshot              → Visual smoke test (capture frames)
5. hf.render                → Render to MP4
```

---

## Video Keywords (topic-aware search)

**Critical for video relevance.** Set `video_keywords` at the top level of your script to provide 3-5 topic keywords that represent the WHOLE video. These keywords are prepended to every Pexels (background) and GIPHY (sticker + meme) search query to ensure results are topically relevant, not just sentence-relevant.

```json
{
  "title": "3 Surprising Facts About the Human Brain",
  "video_keywords": ["brain", "neuroscience", "neurons", "science"],
  ...
}
```

**Without `video_keywords`:** A scene saying "inhale for four seconds" in a brain video → Pexels searches "inhale four seconds" → returns cigarette/breathing exercise clips (irrelevant to brain topic).

**With `video_keywords`:** Same scene → Pexels searches "brain neurons inhale" → returns neuroscience/neuron clips (relevant to the video topic).

If `video_keywords` is omitted, the system auto-extracts keywords from the title (non-stopwords, length > 3, max 5).

---

## Script JSON Format (the single source of truth)

```json
{
  "title": "3 Surprising Facts About the Human Brain",
  "video_keywords": ["brain", "neuroscience", "neurons", "science"],
  "meta": {"aspect": "9:16", "fps": 30, "width": 1080, "height": 1920},
  "tts": {"backend": "kokoro", "default_speed": 1.0},
  "speakers": {
    "alice": {
      "voice": "kokoro:af_heart",
      "position": "bottom-left",
      "scale": 0.35
    }
  },
  "background": {
    "type": "gameplay",
    "query": "city skyline",
    "change_cadence": "scene"
  },
  "music": {"path": "mcp/assets/music/lofi_chill.mp3", "ducking": true},
  "captions": {"style": "word_highlight", "highlight_color": "#00ff88"},
  "stickers": {"enabled": true, "lip_sync": "amplitude"},
  "scenes": [
    {"speaker": "alice", "text": "Hello world!"}
  ]
}
```

---

## Theme Presets (one-field emotional tone)

Set `output.theme` to apply correlated defaults for captions + stickers without hand-tuning each field. Individual field values always override the theme.

| Theme | Use case | Caption highlight | Caption text | Caption style | Stickers |
|-------|----------|-------------------|--------------|---------------|----------|
| `"neutral"` (default) | Gaming, edu-shorts, memes | `#00ff88` (neon green) | `#ffffff` (white) | `word_highlight` | enabled |
| `"calm"` | Healing, meditation, therapy, nervous-system content | `#E8B86D` (warm gold) | `#F5F0E8` (cream) | `word_highlight` | enabled |
| `"energetic"` | Same as neutral (explicit) | `#00ff88` (neon green) | `#ffffff` (white) | `word_highlight` | enabled |

**For healing/calming content, set `"theme": "calm"`.** This changes the caption colors to warm tones while keeping word-level sync animation (word_highlight) — the default for ALL content types. Stickers stay enabled so GIPHY can find calming imagery.

Example (minimal healing script — uses live Pexels footage by default):
```json
{
  "speakers": {"alice": {"voice": "kokoro:af_heart"}},
  "scenes": [
    {"speaker": "alice", "text": "Breathe in for four seconds."}
  ],
  "output": {"theme": "calm"}
}
```

---

## Caption Styles (4 styles)

| Style | Visual | Best for |
|-------|--------|----------|
| `word_highlight` | Current word highlighted in accent color + 10% scale-up, rest in base color | **Universal default** — TikTok/Reels, talking-head, edu-shorts, healing content |
| `sentence_fade` | Whole sentence fades in/out softly | Documentary narration where viewer reads ahead |
| `karaoke_fill` | Words fill with color left-to-right as spoken | Music videos, sing-along |
| `subtitle_rail` | Static subtitle bar at bottom, minimal animation | Interview, formal captioning |

**`word_highlight` is the default for all themes.** It produces word-by-word animation synced with the speaker's voice — the expected behavior for vertical video. Use `sentence_fade` only when you specifically want full-sentence readability over word-sync.

---

## Background Selection (mood-aware)

Use `background.search` to filter procedural backgrounds by mood before passing them to `script.to_video`'s `fallback_pool`. Without this, `type:"procedural"` grabs ALL `.mp4`s in `mcp/assets/backgrounds/` — which mixes calming clips (particles_blue, aurora_green) with jarring ones (tunnel_neon, gradient_rainbow).

```
background.search {mood: "calm"}  →  6 calming clip paths
```

Then pass those paths as `background.fallback_pool` in your script.

| Mood | Clip count | Examples |
|------|-----------|----------|
| `calm` | 6 | particles_blue, waves_teal, aurora_green, bokeh_warm, waves_purple, waves_orange |
| `neutral` | 8 | procedural_01 through procedural_10, abstract_pink |
| `energetic` | 2 | gradient_rainbow, tunnel_neon |
| `dark` | 1 | geometric_dark |
| `uplifting` | 2 | waves_orange, abstract_pink |

---

## Sticker/GIF Scaling Guide

Stickers are small transparent GIF overlays (from GIPHY) that identify the speaker. The default scale is **35% of canvas width** (378px on a 1080px canvas). Here's how to choose the right scale and position:

| Use case | Scale | Position | Notes |
|----------|-------|----------|-------|
| Speaker identifier (default) | 0.35 | `top-left` | Standard TikTok-style speaker sticker |
| Larger speaker (prominent) | 0.45 | `top-left` | When the speaker is the focus |
| Small corner badge | 0.20 | `bottom-right` | Subtle branding/icon |
| Centered reaction | 0.40 | `center` | When the sticker is the main visual |

**Position options:** `top-left`, `top-right`, `bottom-left`, `bottom-right`, `top-center`/`center-top`, `bottom-center`/`center-bottom`, `center`.

**Important:** Stickers are NOT meme b-rolls. Stickers are small persistent overlays that identify the speaker. Meme b-rolls are full-screen video clips that briefly replace the background (set `"meme_brolls": {"enabled": true}`).

---

## Meme B-Rolls (GIPHY as video-clip provider)

Meme b-rolls are **full-screen video clips** from GIPHY that briefly replace the background — like TikTok reaction cuts. They are NOT stickers. They are a separate video-clip provider alongside Pexels and YouTube.

### How it works
1. For each scene, the system detects the emotional beat from the scene text (e.g. "surprising" → "mind blown reaction", "funny" → "laughing reaction")
2. GIPHY's `/v1/gifs/translate` endpoint returns the best-matching GIF
3. The GIF is downloaded as **MP4** (not GIF) for full-screen video quality
4. The MP4 is composited as a **full-screen background cut** for 2.5 seconds (configurable)
5. Captions remain visible ON TOP of the meme cut (correct layering: background → meme → captions → stickers)

### Usage
```json
{
  "meme_brolls": {
    "enabled": true,
    "duration_s": 2.5,
    "offset_s": 0.3
  }
}
```

### GIPHY as a video provider
GIPHY provides `images.original.mp4` — a proper video format that FFmpeg can decode and scale to full-screen. This makes GIPHY a first-class video-clip provider alongside:
- **Pexels** — stock footage (per-scene keyword search)
- **YouTube** — stock footage (via yt-dlp, may be bot-blocked)
- **GIPHY** — reaction/meme clips (per-scene emotion search, MP4 format)

---

## Key Design Decisions

1. **Kokoro is the default TTS** — ONNX sidecar, 24kHz, many preset voices (`voices.list`)
2. **Pexels is the primary stock video source** — requires `PEXELS_API_KEY` (env or config)
3. **GIPHY for stickers + meme b-rolls** — requires `GIPHY_API_KEY`
4. **Multi-broll per scene** — `video_keywords` + per-scene text for topic-relevant clips
5. **Parakeet force-alignment** — real per-word timestamps when models are installed; else even-spacing with warnings
6. **Timeline preview** — token-efficient tree view for agent inspection
7. **Sentence separation** — captions split on . ! ? — no leaking
8. **`music.search` boolean filters are optional** — omit `loopable` / `intro_friendly` / `cta_friendly` unless you need them (defaults no longer force empty results)
9. **Stock `mcp/assets/music/*.mp3` are synthetic fallbacks** — prefer `library.search` / `stock.search` for production music
