# Fresh-Agent UX Audit — Simulation 5 (Photosynthesis)

**Date:** 2026-07-19  
**Topic:** "The Science of Photosynthesis"  
**Video:** `output/photosynthesis.mp4` (43.9 MB, 1080×1920, 43.8s)  
**MCP Binary:** `target/release/mcp-server` (rebuilt after stock_signal.rs fix)

---

## Executive Summary

| Metric | Result |
|--------|--------|
| **Video Quality** | Grade A — 1080p vertical, H.264, AAC audio, word-highlight captions |
| **B-roll Relevance** | ⚠️ **Science topic → "microscope view" anchor for ALL 5 scenes** |
| **Production Score** | Not yet run through verify.production |
| **UX Friction** | High — schema discovery, voice IDs, Parakeet models, music/SFX fallbacks |

---

## B-roll Relevance Analysis

### Search Queries Generated (from rebuilt binary)

| Scene | Query | Anchor |
|-------|-------|--------|
| 1 | `photosynthesis the process which plants microscope view vertical video` | microscope view |
| 2 | `the chloroplasts chlorophyll captures photons microscope view vertical video` | microscope view |
| 3 | `carbon dioxide enters the leaf microscope view vertical video` | microscope view |
| 4 | `the energy stored glucose powers microscope view vertical video` | microscope view |
| 5 | `photosynthesis connects the sun every microscope view vertical video` | microscope view |

### Root Cause

**Topic classification:** `video_keywords = ["photosynthesis", "plants", "chlorophyll", "sunlight", "biology"]` → **Science** (correct)

**Anchor selection:** Science anchor bank has "microscope view" as entry #1. All 5 scenes have signal tokens matching `["microscope", "cell", "biology", "specimen"]` from the scene text (chloroplasts, chlorophyll, biology, leaf, stomata, Calvin cycle, glucose, cell).

**Result:** Same anchor for every scene → repetitive b-roll, misses visual diversity of photosynthesis (leaves, sunlight, chloroplasts, stomata, glucose, plant growth).

---

## Video Quality Assessment

| Component | Status |
|-----------|--------|
| Resolution | ✅ 1080×1920 (9:16) |
| Codec | ✅ H.264 / AAC |
| Captions | ✅ Word-highlight (Bebas Neue, green highlight) |
| TTS | ✅ Kokoro af_heart, 5 scenes |
| Stickers | ✅ GIPHY narrator (top-left, scale 0.35) |
| Music | ✅ Library track (snowfall, ducked -12dB) |
| SFX | ✅ 5 CTA/highlight SFX placed |
| Caption Sync | ⚠️ Approximate (Parakeet ONNX models missing) |

---

## UX Friction Points (Fresh-Agent Perspective)

### 1. Schema Discovery — **Critical**
- `script.parse` expects `text` field, not `narration` — only discoverable by trial/error
- `speakers` object required with `voice` in `{provider}_{voice}` format (e.g., `kokoro:af_heart`)
- `video_keywords` auto-extracted from title but override not documented
- No `script.example` tool to show valid JSON

### 2. Voice Profile IDs — **High**
- Must know exact voice IDs from `voices.json` (`kokoro:af_heart`, not just `af_heart`)
- No `voice.profile.list` call in golden path — agent must guess or search

### 3. Parakeet Models — **High**
- Force-alignment fails silently with warning, falls back to estimated timings
- No auto-install, no `system.doctor` action to fetch models
- Caption sync is approximate — user sees warning but no fix path

### 4. Music/SFX Fallbacks — **Medium**
- Empty directories → procedural gradient backgrounds, no stock audio
- No clear signal in output that stock audio was skipped
- `allow_procedural` parameter undocumented in tool schema

### 5. Build Step Not Integrated — **Critical**
- Fix committed (stock_signal.rs) but **simulation 5 used stale binary**
- Agent must manually `cargo build --release` — not in golden path
- No `system.doctor` check for binary freshness

### 6. Anchor Diversity — **High** (New Finding)
- Science topic → single "microscope view" anchor for all scenes
- Need per-scene visual diversity: leaf surface, sunlight, chloroplasts, stomata, plant growth
- Current anchor bank too narrow for biology subtopics

---

## Comparison: Simulation 3 (Black Holes) vs Simulation 5 (Photosynthesis)

| Aspect | Sim 3 (Black Holes) | Sim 5 (Photosynthesis) |
|--------|---------------------|------------------------|
| Topic Detection | Space ✅ | Science ✅ |
| Anchor Diversity | galaxy timelapse, nebula deep field | microscope view ×5 |
| Query Quality | Space-relevant terms | Biology terms + microscope |
| Root Cause | Fixed by seed word expansion | Anchor bank too narrow |

---

## Recommended Fixes (Priority Order)

### P0 — Build Integration
- Add `cargo build --release -p openscript-mcp --bin mcp-server` to golden path docs
- `system.doctor` should verify binary timestamp > latest commit

### P0 — Anchor Bank Expansion (Science → Biology)
Add biology-specific anchors to Science category:
```rust
("leaf surface timelapse", vec!["leaf", "plant", "photosynthesis", "green"]),
("chloroplast closeup", vec!["chloroplast", "chlorophyll", "thylakoid", "granum"]),
("sunlight through leaves", vec!["sunlight", "leaf", "canopy", "rays"]),
("stomata microscopic", vec!["stomata", "pore", "gas exchange", "leaf"]),
("plant growth timelapse", vec!["plant", "growth", "seedling", "sprout"]),
("glucose molecule", vec!["glucose", "molecule", "energy", "chemical"]),
```

### P1 — Schema Documentation
- Add `script.example` tool returning valid ScriptSpec JSON
- Document `voice` format in `script.parse` description
- Document `allow_procedural` parameter

### P1 — Parakeet Model Auto-Fetch
- `system.doctor` action: download ONNX models if missing
- Or bundle models in release artifacts

### P2 — Music/SFX Visibility
- Output field: `music_source: "library|pixabay|procedural|none"`
- Output field: `sfx_source: "library|none"`

### P2 — Per-Scene Anchor Variation
- Modify `pick_visual_anchor` to prefer unused anchors in multi-scene videos
- Track used anchors across scenes, rotate through bank

---

## Verdict

**Simulation 5 Grade: B+**

- ✅ Video renders correctly, all pipeline stages execute
- ✅ Topic detection works (Science)
- ❌ **Anchor diversity failure** — same "microscope view" for all 5 biology scenes
- ❌ **Build step gap** — stale binary used despite fix commit
- ❌ **Schema opacity** — fresh agent cannot discover required fields without errors

**Next Iteration:** Expand Science anchor bank with biology-specific entries, verify anchor rotation across scenes, integrate build step into golden path.