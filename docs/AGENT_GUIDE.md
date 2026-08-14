# Agent Guide — OpenScript MCP Tool Catalog

> This document is the canonical reference for AI agents using OpenScript's MCP tools.
> Every agent MUST read this before calling any tool.

---

## Tool Families (30 families, 113 tools)

| Family | Count | Tools |
|--------|-------|-------|
| timeline | 12 | build, load, validate, presentation, add_segment, add_track_event, diff, preview, inspect, autofill_broll, render, upgrade |
| script | 7 | schema, parse, format.validate, generate_voices, build_captions, to_timeline, to_video |
| director | 2 | run, format |
| hf | 6 | classify, lint, validate, snapshot, render |
| tts | 4 | generate, estimate_duration, preview, commentary |
| music | 4 | index, search, assign, ducking.plan |
| broll | 9 | suggest, fetch, assign, plan, keywords, validate_keywords, repair, auto, probe |
| srt | 4 | read, prepare, apply_edit, to_timeline |
| voice | 4 | profile.add, profile.list, profile.remove, design |
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
| video | 1 | to_video |

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

### Choosing the TTS audio model (script → video)

`script.to_video` / `script.generate_voices` pick the TTS engine in this order:

1. **Speaker voice pin** — a speaker whose `voice` references a registered
   profile (e.g. `"ishan"`, `"air_analyst"`) routes by that profile's
   own `provider` field (audio8 / voicedesign / higgs / kokoro / sidecar).
   This always wins — a `voicedesign`-provider character voice synthesizes
   DIRECTLY on the Qwen3 VoiceDesign model even if `tts.backend` says otherwise.
2. **Script `tts.backend`** — the engine default for the whole video
   (`kokoro` | `audio8` | `voicedesign` | `higgs` | `indextts` | `sidecar`).
   `indextts` = IndexTTS-2.5 (emotion-aware zero-shot clone, 22.05 kHz,
   en/zh/ja/es/ar, ~5.7 GB, provisioned by `bash scripts/setup_indextts.sh`,
   bilibili license — research/non-commercial).
3. **Script `tts.voice`** — a default voice profile id; a speaker whose voice
   is the literal string `"default"` resolves to it.
4. **User config** — `~/.openscript/config.json` → `tts.default_backend` and
   `tts.default_voice`, or env `OPENSCRIPT_TTS_BACKEND` / `OPENSCRIPT_TTS_VOICE`.
   (env wins over config file; explicit script fields win over both).
5. **Built-in default** — `kokoro` / `kokoro:af_heart`.

Example: to make every script use the Audio8 clone by default, set
`OPENSCRIPT_TTS_BACKEND=audio8 OPENSCRIPT_TTS_VOICE=ishan` and write
speakers with `"voice": "default"`.

### Designing novel character voices (`voice.design`)

`voice.design` creates a **brand-new fictional voice from a text description** —
no reference audio needed (Qwen3-TTS-1.7B-VoiceDesign, ONNX int4, Apache-2.0,
~4.3 GB, provisioned by `bash scripts/setup_voicedesign.sh`). This is how you
build comic / story casts where the characters don't exist on tape:

```
voice.design {
  instruct: "Male, 17, tenor, gaining confidence — breath support deepens when nervous",
  text:     "Give every small business the voice of a big one.",   // sample line
  language: "english",
  profile_id: "hero_teen"      // OPTIONAL: auto-register as a reusable voicedesign profile
}
```

- **No `profile_id`** → returns just the persona WAV (`artifacts/voices/designed_*.wav`).
- **With `profile_id`** → saves a `provider: voicedesign` voice profile, so the
  character voice is reusable via `tts.generate` or a script speaker
  `"voice": "hero_teen"` — every line then synthesizes DIRECTLY on the Qwen3
  VoiceDesign model (personality + per-line emotion/tone instruct, NO cloning).
- Generation knobs: `max_tokens` (2048), `temperature` (0.9), `top_k` (50), `seed`.
- `system.capabilities` reports `voicedesign.available` (sidecar + model present).

Official workflow for a stable comic cast: VoiceDesign designs the persona
(→ register as a `voicedesign` profile as above) → each script line is generated
BY the voice-design model with the character's personality + the scene's
emotion/tone — the voice stays locked while every line gets its own delivery.

### Expressive 100+ language TTS (`higgs` — Higgs Audio v3, 4B ONNX GenAI int4)

`provider: higgs` / `tts.backend: "higgs"` runs the self-contained
`onnx-community/higgs-audio-v3-tts-4b` `cuda_int4` export (~3.6 GB,
provisioned by `bash scripts/setup_higgs.sh` → `.venv-higgs`). Higgs is a 4B
conversational TTS with **100+ languages, zero-shot voice cloning, and inline
control tokens** for emotion / prosody / style / sfx (24 kHz, 25 fps,
8-codebook Higgs v2 codec). The int4 llm_decoder is a plain ONNX QDQ graph
run under ordinary onnxruntime with a manual KV-cache loop.

- **Voice cloning**: `voice.profile.add { profile_id, ref_audio, ref_text, provider: "higgs" }`
  — the reference is encoded into the prompt (`<|tts|> <|ref_text|> tok(ref)
  <|ref_audio|> [codes] <|text|> tok(text) <|audio|>`), so `tts.generate` /
  `script.generate_voices` speak with the cloned voice.
- **Per-line emotion**: the scene `emote` maps to a real Higgs control tag
  (`<|emotion:anger|>`, `<|style:whispering|>`, `<|sfx:laughter|>`, …) — 21
  emotions, 3 styles, 9 sfx, plus prosody speed/pitch/pause tags. Free-form
  `tone`/instruct text is deliberately NOT injected (Higgs reads unrecognized
  text aloud).
- **Languages**: Hindi / Hinglish, English, Chinese, Spanish, French, German,
  Arabic, Bengali, Tamil, Telugu, Marathi, Gujarati, Urdu, Punjabi, and 90+
  more (no language flag — multilingual text tokenizes natively).
- `system.capabilities` reports `higgs.available` (sidecar + model present).
- **License**: research / non-commercial (Boson Higgs Audio v3 license) —
  monetized use requires a commercial license from Boson.

### Emotion-take presets (per-line tonality — `voice.profile.add` `emotions`)

A clone profile is a **tonality template**, not a single flat timbre. Attach
separate reference recordings of the same speaker delivering each emotion, then
any line can be spoken in that emotion:

```
voice.profile.add {
  profile_id: "ishan", provider: "audio8",
  ref_audio: "base.wav", ref_text: "...",
  emotions: {
    "angry":   { ref_audio: "ishan_angry.wav",   ref_text: "..." },
    "whisper": { ref_audio: "ishan_whisper.wav", ref_text: "...", cfg_scale: 1.3 }
  }
}
```

- Scene `emote` (free-form: `"happy"`, `"angry"`, `"whisper"`, ...) selects the
  matching take at synthesis; `tts.generate` takes an `emotion` arg.
- Engine mechanics: audio8 takes auto-register as `{profile_id}@{emotion}`
  compound voices; the scene emote selects the take at synthesis.
- Unmatched emotion → falls back to the base reference (never fails).
- Per-scene `speed` / `pitch` overrides (previously silently dropped for clone
  engines) are now applied post-synthesis.

### Expression knobs (prosody / emotion control)

Clone engines support explicit sampling knobs for inflection and emotional
nuance. Higher temperature = more prosodic variation; lower = flatter/more
robotic (0.3 is the robotic zone — the old flat default). Production-grade
clones sit at **0.6–0.8 temperature** (default 0.7).

```
tts.generate { temperature: 0.8, top_k: 50, top_p: 0.9, cfg_scale: 1.0 }
```

- **`tts.generate`**: `temperature` / `top_k` / `top_p` / `cfg_scale` — all
  optional; explicit values win over emotion takes and engine defaults.
- **Script-level** (`tts` block): `default_temperature` / `default_top_k` /
  `default_cfg_scale` apply to every scene.
- **Per-scene** (`scenes[].temperature`): overrides the script default for one
  line (e.g. a whisper scene at 0.5, an outburst at 0.9).
- **Precedence**: explicit request/scene value → emotion take's own cfg_scale
  → engine default. A script-level `default_cfg_scale` is NOT applied when the
  scene uses an emotion take (the take's tuned value wins).
- **Loudness**: every sidecar now normalizes each output WAV to −16 LUFS
  (two-pass ffmpeg loudnorm, original sample rate preserved) so all scenes are
  uniform — emotion takes used to come out 4–14 dB quieter and get buried
  under the music bed. Chunk seams are equal-power crossfaded (no mute dips).

### Character-first workflow (two-part — `character.*`)

For story/comic content, define characters FIRST (schema + properties), design
each one's base voice and per-emotion takes, THEN write the transcript against
those characters. The pipeline is deliberately two-part:

**PART 1 — Character development (voice-design):**
```
character.create {
  character_id: "detective", name: "Detective Marlow", role: "protagonist",
  personality: "grumpy old detective, low gravelly voice, slight rasp, slow deliberate pace",
  sample_text: "The evidence never lies.", language: "english"
}
character.design_emotion { character_id: "detective", emotion: "angry",
  sample_text: "You messed with the wrong precinct!" }
character.design_emotion { character_id: "detective", emotion: "whisper",
  sample_text: "Keep your voice down..." }
character.list   # inspect the cast + their emotional ranges
```
`character.create` designs the base voice via VoiceDesign and registers it as a
`voicedesign` profile (`detective`) — scene lines synthesize DIRECTLY on the
Qwen3 model, never through a cloning engine. Each `character.design_emotion`
designs one emotional delivery and stores its `instruct` (personality + emotion)
so a scene's `emote` attunes the line's tonality at synthesis time.

**PART 2 — Transcript development (script-design):** speakers reference the
character; each scene's `emote` picks the emotional take:
```json
{ "speakers": { "detective": { "voice": "detective", "preset": "default_person" } },
  "scenes": [
    { "speaker": "detective", "text": "I said the evidence never lies.", "emote": "angry" },
    { "speaker": "detective", "text": "Now keep quiet about this.", "emote": "whisper" }
  ] }
```
Audio generation then produces each line in the character's voice AND the
scene's emotion. Characters persist in `.openscript/characters.json`;
`character.remove` drops both the schema entry and the base voice profile.

### Content formats — shape HOW you author the script

The `format` block (or `director.format`) tells the agent how to structure the
script: speaker count, **male/female alternation**, pacing, reactions, music
mood. Formats: `presentation` (default), `podcast`, `dialogue`, `comedy_sketch`,
`romcom`, `meme_reel`, `documentary`, `how_to`, `listicle`, `storytime`,
`debate`, `newsflash`, `review`.

Each format has a **unique signature** (structure_kind, speaker range, pacing,
reactions, sticker mode, music mood) enforced by a CI test. Use the
`differentiator` field from `director.format {type:"list"}` to pick correctly:

| Format | Family | ≠ (don't confuse with) |
|---|---|---|
| `presentation` | solo_narrated | documentary — short persuasive explainer, one idea per line |
| `documentary` | solo_narrated | presentation — chaptered long-form evidence narrative |
| `podcast` | duo_conversational | dialogue — informal 2-4 speaker roundtable WITH memes |
| `dialogue` | duo_conversational | podcast — formal interviewer/expert Q&A, NO memes |
| `comedy_sketch` | duo_comedic | meme_reel — duo setup→punchline arc, meme on the punchline |
| `meme_reel` | solo_comedic | comedy_sketch — solo rapid-fire takes, reaction stickers |
| `romcom` | duo_dramatic | dialogue — emotional beat structure via emote pairs |
| `how_to` | solo_narrated | presentation — numbered actionable steps, instructs not persuades |
| `listicle` | solo_narrated | how_to — ranked observational signs/things, not actionable steps |
| `storytime` | solo_narrated | documentary — first-person lived journey, not third-person evidence |
| `debate` | duo_conversational | dialogue — adversarial claimers + verdict, not cooperative Q&A |
| `newsflash` | solo_narrated | presentation — urgent verified-fact briefing, no persuasion |
| `review` | solo_narrated | listicle — one subject, pros/cons + rating, not N ranked things |

**Alternation rule:** for `podcast` / `dialogue` / `comedy_sketch` / `romcom`,
alternate a male and a female voice every scene — set
`format.alternation: "male_female"` and declare `gender` on each speaker
(voicedesign profiles also carry `gender`; Kokoro `af_`/`am_`/`bf_`/`bm_`
prefixes infer automatically). `script.format.validate` enforces it.

**Workflow:**
```
director.format { type: "podcast", topic: "AI agents" }  # get the playbook
  → speaker blueprint (gender + voice.design instructs), scene structure, defaults
voice.design / character.create  →  build the speaker voices
script.parse → script.format.validate → script.to_video
```

**CLI:** `openscript video new --format podcast --topic "AI agents"` scaffolds
a draft script; `openscript format list` enumerates formats. Full playbooks
are also loadable as skills: `skills/content-formats/<format>/SKILL.md`.

Correlated defaults (applied only when the agent left the field unset):
- `podcast`: 2–4 speakers, speed 1.02, temp 0.88, reaction memes, energetic music
- `dialogue`: exactly 2 speakers, temp 0.9, neutral music
- `comedy_sketch`: 2 speakers, speed 1.05, punchline reaction memes, energetic
- `romcom`: 2 speakers (M/F leads), emote chemistry pairs, calm music
- `meme_reel`: 1–2 speakers, speed 1.1, heavy reaction memes, energetic
- `documentary`: 1–2 speakers, speed 0.95, temp 0.75, no stickers, calm

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
4. broll.keywords         — STAGE 1 (draft): unified keywords module — ONE batched LLM call emits
                           BOTH visual stock keywords AND GIPHY reaction keywords per segment
                           (id-echo + missing-id redraft + salience fallback; unicode-aware;
                           auto-detects source language; auto-extracts video_keywords from title)
5. broll.validate_keywords— STAGE 2 (relevance-validation): agent scores REAL Pexels candidates
                           (video names/durations) against the spoken caption → final keywords + best video
6. broll.fetch            — Download + auto-place the validated clips on the timeline
7. sticker.auto           — ONE-CALL sticker pipeline (parallel to broll): consumes the SAME unified
                           draft's reaction keywords (never visual b-roll nouns) → sticker.validate_keywords
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

**One-call (RECOMMENDED) — `video.to_video`:** the visual layer ALTERNATES
stock b-roll ↔ the ORIGINAL footage per transcript segment — `[broll → video →
broll → video → …]`. Everything from the A2V pipeline remains (captions,
stickers, music, SFX, voiceover); only the visual layer alternates, segregated
by the transcript segmentation. The original video is the renderer's base
layer and its audio is the master clock.
```
video.to_video {
  video_path: "input.mp4",
  alternation: { enabled: true, pattern: "every_other" }  // or broll_ratio: 0.5
}
```
Planned roles (`broll` / `source` per segment) are persisted to
`directives.presentation.visual_roles`; source-role segments get NO b-roll
event, so the original footage shows there.

**Manual / fine-grained:**
```
1. timeline.presentation  — plan/query visual roles (mode=alternate, pattern,
                            every_n, broll_ratio)
2. transcribe             — Hinglish SRT from video
3. srt.to_timeline        — timeline with segments (source = the video)
4. broll.auto { alternation: {enabled: true} }  — stock ONLY on broll-role segs
5. music.assign / captions.generate_ass / sticker.auto — across ALL segments
6. timeline.validate      — checks intent + coverage + BROLL_ON_SOURCE
7. timeline.render        — base = original video → alternation renders
```

**Alternative (full-coverage b-roll over everything):** the classic A2V stack
(`broll.auto` without alternation, or `reelize.brief` → `reelize.direct`),
which is Trajectory C with the visual layer fully covered by stock.

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
verify.audio       — Audio quality (loudness, dialogue, silence, per-scene variance)
verify.captions    — Caption sync (coverage, gaps, readability)
verify.render      — Technical integrity (duration, aspect, file size)
verify.production  — Full production quality (stock, music, captions)
```

**Per-scene loudness-variance KPI (Phase 170):** pass the per-scene voiceover
WAVs to `verify.audio` (via `scene_wavs` array or a `script.generate_voices`
`voiceover_manifest`) to measure each scene's integrated LUFS and get a
`loudness` block: `spread_db`, `variance_ok` (threshold 6.0 dB), and
`per_scene_lufs`. A >6 dB spread adds an issue and −20 pts (a >12 dB spread,
the pre-fix mute range, −35 total). Every scene should sit within a few dB of
−16 LUFS — the TTS sidecars normalize at the source; if this KPI fires,
re-generate voices (or re-design emotion takes through the fixed voicedesign
sidecar) instead of shipping a video where lines are buried under the music
bed.

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

### Feature toggles (config-driven gating)

Every subsystem is toggleable via `~/.openscript/config.json` →
`features.<category>.<name>` (all default **ON**; env override
`OPENSCRIPT_FEATURE_<CATEGORY>_<NAME>=0|1`). The SAME toggles drive:

1. **Cold-start installs** — `setup.sh` provisions only the deps for enabled
   features (`bash setup.sh --list-features`; `--feature cat.name=0` to toggle
   for one run; persist via `setup_openscript_config.sh --feature cat.name=0`).
2. **Runtime gating** — a disabled engine/tool returns a clear error naming the
   toggle + setup command instead of a missing-dep failure. Gates today:
   - `tts.{kokoro,audio8,voicedesign,higgs,sidecar}` — TTS router
   - `transcription.hinglish_ggml` — `transcribe`
   - `media.{pexels,giphy,pixabay}` — key resolvers fail closed (handlers
     degrade to their existing "key not set" path)
   - `media.youtube` — `youtube.search` / `youtube.download`
   - `llm.{opencode,openrouter}` — LLM/vision cascade
   - `render.{ffmpeg,hyperframes,remotion,nvenc}`, `frontend` — reporting
     (nvenc auto-degrades to CPU regardless)

`system.capabilities` returns a `features` block (every toggle with `enabled`,
the env override name, the config path, and the setup command) and marks each
subsystem `available:false` with a feature-disabled reason when toggled off.

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
