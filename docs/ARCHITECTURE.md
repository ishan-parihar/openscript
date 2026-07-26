# OpenScript Architecture — Canonical Reference

> **This is the single source of truth for OpenScript's pipeline architecture.**
> All other architecture/audit/brainstorm docs are **superseded** by this document.
> Last updated: July 26, 2026 (Audit #24 findings incorporated).

---

## 1. The Ideal Pipeline (A2V — Audio to Video)

The system's designed flow for converting an audio file into a production-grade video:

```
┌─────────────────────────────────────────────────────────────────┐
│ STEP 1: TRANSCRIPTION                                            │
│   transcribe(audio) → word-level SRT with timestamps            │
└──────────────────────────┬──────────────────────────────────────┘
                           ▼
┌─────────────────────────────────────────────────────────────────┐
│ STEP 2: SEMANTIC SEGMENTATION                                   │
│   srt.prepare(word_srt) → grouped caption segments              │
│   Each segment = ideal cut duration for b-roll (2-5s each)     │
│   Engine determines min/max duration per phrase                  │
└──────────────────────────┬──────────────────────────────────────┘
                           ▼
┌─────────────────────────────────────────────────────────────────┐
│ STEP 3: AGENT KEYWORD GENERATION                                │
│   segment.analyze(audio, srt) → structured segments             │
│   {id, start_s, end_s, duration_s, caption} × N segments       │
│                                                                  │
│   AI AGENT reads Hinglish/foreign captions                      │
│   Agent generates ENGLISH visual keywords per segment           │
│   Keywords match the exact number of segments needed            │
└──────────────────────────┬──────────────────────────────────────┘
                           ▼
┌─────────────────────────────────────────────────────────────────┐
│ STEP 4: STOCK FOOTAGE EXTRACTION + AUTO-PLACEMENT               │
│   broll.fetch(keywords, download=true) → clips from Pexels      │
│   Each clip already matches segment duration                    │
│   Clips auto-placed on timeline at correct position/duration    │
│   timeline.assets.broll[clip_id] = {path, concept, timing}     │
└──────────────────────────┬──────────────────────────────────────┘
                           ▼
┌─────────────────────────────────────────────────────────────────┐
│ STEP 5: POST-PROCESSING LAYER (independent, composable)         │
│   captions.generate_ass(srt) → ASS burn-in on video            │
│   music.assign(track) → background music with ducking           │
│   sfx.assign(transitions) → whoosh/pop at segment boundaries    │
│   sticker.render(speakers) → speaker overlays (optional)        │
└──────────────────────────┬──────────────────────────────────────┘
                           ▼
┌─────────────────────────────────────────────────────────────────┐
│ STEP 6: VALIDATE + RENDER                                       │
│   timeline.validate(timeline) → structural check                │
│   timeline.render(timeline) → final MP4                          │
│   verify.production(video) → quality score + grade              │
└─────────────────────────────────────────────────────────────────┘
```

---

## 2. The Timeline Data Model

The `Timeline` JSON is the central artifact. Every tool reads from or writes to it.

```rust
struct Timeline {
    source: PathBuf,           // source video/audio path
    segments: Vec<Segment>,    // ordered cuts (start/end times in seconds)
    tracks: TrackMap,          // HashMap<TrackType, Vec<TimelineEvent>>
    assets: AssetRegistry,     // maps asset_id → {path, metadata}
    directives: Directives,    // ducking, transitions, mix config
    effects: Effects,          // burn_captions, loudnorm
}

struct Segment {
    id: String,
    start: f64,              // seconds from source
    end: f64,                // seconds from source
    caption: String,
    crossfade_ms: u32,
}

enum TrackType {
    Dialogue,    // source video segments (the main track)
    Broll,       // overlay clips from Pexels/stock
    Music,       // background music
    Sfx,         // sound effects at transitions
    Voiceover,   // TTS narration
    Captions,    // caption events (for PupCaps overlay)
}
```

**Critical design rule:** `timeline.render` reads ALL tracks from the timeline JSON. If a track is empty, that layer is simply absent from the output. The render engine (`FilterGraphBuilder::from_timeline`) iterates each track and builds the corresponding ffmpeg filter graph nodes.

---

## 3. Three Implementation Gaps (Audit #24 Findings)

### Gap 1: B-Roll Fetched But Never Auto-Placed

**What should happen:** Agent provides keywords per segment → engine downloads clips matching each segment's duration → clips are automatically placed on the broll track at the correct position.

**What actually happens:**
- `broll.fetch` downloads clips to cache, returns `cached_path`
- `broll.assign` exists but is never called between fetch and render
- `timeline.tracks[Broll]` remains empty → no visual overlays in output
- Agent would need to manually call `broll.assign` N times (once per clip), calculating position_ms and duration_ms from segment timing

**Root cause:** `broll.fetch` is a search/download tool. It produces clips on disk but does not write to the timeline. There is no bridge between "clips downloaded" and "clips placed on timeline."

**Fix required:** `broll.fetch` should accept `timeline_path` and auto-place each downloaded clip on the broll track at the matching segment's position/duration, registering the asset in `timeline.assets.broll`.

### Gap 2: Captions Generated But Not Registered in Timeline

**What should happen:** `captions.generate_ass` generates an ASS file AND registers it in `timeline.assets.captions["ass"]["path"]` so `timeline.render` can find it.

**What actually happens:**
- `captions.generate_ass` writes `.ass` file to disk, returns `ass_path`
- It does NOT accept `timeline_path` or update the timeline JSON
- `render_from_timeline` reads captions from `timeline.assets.captions.get("ass")` — this is always empty
- No subtitle burn-in in the output

**Root cause:** `captions.generate_ass` is stateless — it produces a file but doesn't connect it to the timeline. The render engine expects the ASS path to be registered in `timeline.assets.captions`.

**Fix required:** `captions.generate_ass` should accept an optional `timeline_path`, generate the ASS file, and register `{"path": "captions.ass"}` in `timeline.assets.captions`.

### Gap 3: Segmentation Mismatch Between Tools

**What should happen:** One consistent segmentation that both the agent and renderer agree on.

**What actually happens:**
- `segment.analyze` groups by `SCENE_SIZE=4` (4 SRT entries per scene) → 12 segments
- `srt.to_timeline` creates one segment per SRT entry → 45 segments
- Agent generates keywords for 12 segments but timeline has 45 — no 1:1 mapping

**Root cause:** Two independent code paths produce different segmentations. The agent doesn't know which segment IDs to use for b-roll placement.

**Fix required:** Either unify segmentation (one `scene_size` parameter controls both) or make `segment.analyze` output segment IDs that map to `srt.to_timeline` segment IDs.

---

## 4. What Works Correctly

| Component | Status | Notes |
|-----------|--------|-------|
| `transcribe` | ✅ Working | HinglishGgml produces accurate Latin-script output |
| `srt.prepare` | ✅ Working | Word grouping into caption segments is correct |
| `srt.to_timeline` | ✅ Working | Creates proper timeline with all 6 tracks |
| `broll.fetch` | ✅ Working | Downloads clips from Pexels with agent keywords |
| `music.assign` | ✅ Working | Places music on timeline with ducking |
| `sfx.assign` | ✅ Working | Places SFX at transitions |
| `timeline.validate` | ✅ Working | Structural validation catches real issues |
| `timeline.render` | ✅ Working | Produces valid MP4 with proper codec/resolution |
| `verify.*` tools | ✅ Working | All verification tools run end-to-end |
| `FilterGraphBuilder` | ✅ Working | Correctly reads all tracks and builds ffmpeg filters |

---

## 5. The `script.to_video` Golden Path (Works Because It Chains Internally)

`script.to_video` succeeds where atomic tools fail because it chains all steps inside a single handler:

```rust
// Inside handle_script_to_video():
// 1. Creates timeline + segments
// 2. For each scene: fetches broll AND assigns to timeline
// 3. Generates ASS AND registers in timeline.assets.captions
// 4. Assigns music AND registers in timeline.assets.music
// 5. Calls timeline.render with fully populated timeline
```

The atomic tool chain breaks because each tool is stateless — it produces artifacts on disk but doesn't register them in the timeline JSON.

---

## 6. Documentation Map

### Canonical documents (read these):

| Document | Purpose |
|----------|---------|
| `docs/ARCHITECTURE.md` | **THIS FILE** — pipeline design, data model, gaps |
| `AGENTS.md` | Engineering protocol (code style, testing, git workflow) |
| `AGENT_GUIDE.md` | MCP tool catalog for AI agents |
| `crates/openscript-core/src/timeline/schema.rs` | Timeline data model (source of truth) |

### Superseded documents (do not rely on these):

| Document | Superseded by |
|----------|--------------|
| `docs/V2V_A2V_ARCHITECTURE_AUDIT.md` | This file §3 |
| `A2V_V2V_ARCHITECTURE_AUDIT.md` (root) | This file §3 |
| `A2V_V2V_LAYERING_AUDIT.md` (root) | This file §3 |
| `A2V_BROLL_ARCHITECTURE_REVIEW.md` (root) | This file §3 |
| `BROLL_ARCHITECTURE_AUDIT.md` (root) | This file §3 |
| `BRAINSTORM_MONOLITH_A2V_GAPS.md` (root) | This file §3 |
| `BRAINSTORM_STT_ARCHITECTURE.md` (root) | Superseded by HinglishGgml implementation |
| `BRAINSTORM_HINGLISH_STT_AND_TITLE_CARDS.md` (root) | Superseded by implementation |
| `VIDEO_SEARCH_UPGRADE.md` (root) | Superseded by Pexels integration |
| `MEME_BROLL_DESIGN.md` (root) | Superseded by sticker.render |
| `docs/PIPELINE_FIX_PLAN.md` | Phases A-F completed; gaps in §3 are the remaining work |
| `docs/INSTALL_MEDIA_DEPS_PLAN.md` | Phase I-IV partially complete; not architecture |
| `FRESH_AGENT_UX_AUDIT_1-23.md` | Audit #24 is the current baseline |
| `COMPREHENSIVE_AUDIT_REPORT.md` | Superseded by per-audit reports |
| `TOOL_AUDIT_REPORT.md` | Historical; tool count now tracked in integration tests |

---

## 7. Decision Log

| Decision | Rationale | Date |
|----------|-----------|------|
| Delete `audio.to_video` monolith | Architecturally wrong for non-English content — hardcoded Hinglish→English dictionary can't cover all words. Agent is the correct translation layer. | July 25, 2026 |
| Agent generates English keywords | LLM understands Hinglish context natively; dictionary can't. Agent reads segments → understands meaning → generates visual keywords. | July 25, 2026 |
| `broll.fetch` as search/download tool | Correct separation of concerns: search ≠ placement. But needs bridge to auto-place on timeline. | July 26, 2026 |
| `captions.generate_ass` as file generator | Correct separation of concerns: generation ≠ registration. But needs `timeline_path` param for registration. | July 26, 2026 |
| Keep `script.to_video` as golden path | Works end-to-end for from-scratch creation. Atomic tools are for NLE editing and agent-directed workflows. | Ongoing |
| Timeline JSON as central artifact | Every tool reads/writes the same JSON. Render engine reads all tracks from it. This is the correct architecture — gaps are in tool integration, not design. | Ongoing |
