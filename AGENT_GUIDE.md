# OpenScript Agent Guide — Golden Trajectory for Video Creation

## Tool Taxonomy (75 tools, 8 categories)

### 1. SCRIPT CREATION (3 tools)
Create a video from scratch — define scenes, speakers, backgrounds.

| Tool | When to use |
|------|------------|
| `script.parse` | Validate a script JSON before production |
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

### 8. QUALITY & VERIFICATION (3 tools)
Post-render quality checks.

| Tool | When to use |
|------|------------|
| `verify.audio` | Check audio levels, silence, sample rate |
| `verify.captions` | Verify caption timing against video |
| `verify.render` | Full render verification (duration, resolution, tracks) |

---

## Golden Trajectories

### Trajectory A: From-Scratch Video (NEW — preferred for AI agents)
```
1. script.parse          → Validate the script JSON
2. script.to_video       → ONE-CALL: script → MP4 (includes TTS, captions,
                           multi-broll backgrounds, GIPHY stickers, music)
```
**That's it.** 2 tools. The `script.to_video` orchestrator handles everything:
- Kokoro TTS per scene with whisper force-alignment
- Word-highlight captions (4 styles)
- Multi-broll: unique Pexels stock footage per scene
- GIPHY sticker overlays per speaker
- Background music with sidechain ducking
- Timeline preview (token-efficient for agent inspection)

### Trajectory B: From-Scratch with Manual Control
```
1. script.parse             → Validate script
2. script.generate_voices   → TTS + whisper alignment
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

## Script JSON Format (the single source of truth)

```json
{
  "title": "My Video",
  "meta": {"aspect": "9:16", "fps": 30, "width": 1080, "height": 1920},
  "tts": {"backend": "kokoro", "default_speed": 1.0},
  "speakers": {
    "alice": {
      "voice": "kokoro:af_heart",
      "position": "bottom-left",
      "scale": 0.2
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

## Key Design Decisions

1. **Kokoro is the default TTS** — no external dependency, 24kHz, 54 voices
2. **Pexels is the primary video source** — API key hardcoded, portrait/landscape
3. **GIPHY for stickers** — transparent GIFs, sticker_layering bundle
4. **Multi-broll per scene** — extract_keywords() finds relevant stock per scene
5. **Whisper force-alignment** — real per-word timestamps (not even-spacing)
6. **Timeline preview** — token-efficient tree view for agent inspection
7. **Sentence separation** — captions split on . ! ? — no leaking
