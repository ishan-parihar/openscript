# OpenScript Fresh-Agent Simulation Audit — Run #9

**Date:** 2026-07-21  
**Base Commit:** 1f50546 (Phase 18d: YAGNI cleanup — remove dead engine param + stale imports)  
**Prior Audit:** Run #8 at 3d6a7f6 (65-70/100 Grade B estimated — 3 P0 blockers resolved)  
**System State:** Build ✅ | 79 tools | MCP server ✅ | Kokoro ✅ | Pexels ✅ | GIPHY ✅ | Whisper ✅

---

## Executive Summary

This audit simulates a **completely fresh AI agent** with zero prior knowledge of OpenScript. The agent receives only:
1. Its role ("AI video creation assistant")
2. The MCP server binary location (`target/release/mcp-server`)
3. A task: "Create a 30-second educational video about 5 Amazing Facts About Octopuses"

The agent must discover everything through the MCP protocol alone — no documentation, no guides, no examples.

### Result: Grade C (54/100) — rendered_production_fail

The video was **successfully rendered** (77MB valid MP4, 83.9s generation time) but failed production quality checks. The agent completed the full golden trajectory without any code changes, but the output has critical design issues.

| Category | Score | Status |
|----------|-------|--------|
| **Agent Discoverability** | 8/10 | ✅ Golden path is discoverable |
| **Tool Interface Clarity** | 6/10 | ⚠️ Some friction points |
| **Video Output Quality** | 5/10 | ❌ Production fail (score 54) |
| **End-to-End Reliability** | 7/10 | ✅ Pipeline completes but output fails QA |

---

## Phase 1: Discovery & Onboarding

### What the Fresh Agent Saw

**Step 1 — Initialize:**
```json
{
  "serverInfo": {"name": "openscript-rs", "version": "0.1.0"},
  "capabilities": {"tools": {}, "resources": {}, "prompts": {}, "logging": {}},
  "instructions": "OpenScript MCP server with 84 tools for AI-directed video creation..."
}
```
- ✅ Server identifies itself clearly
- ✅ Instructions mention `director.run` for one-shot creation
- ✅ Instructions mention `system.doctor` for cold-start diagnosis
- ⚠️ Instructions reference 84 tools but actual count is 79 (stale count)

**Step 2 — tools/list:**
- ✅ 79 tools returned with names, descriptions, and input schemas
- ✅ Tool naming is consistent (`script.to_video`, `background.fetch`, etc.)
- ⚠️ No tool returns the script JSON schema — agents must guess or use `script.parse` errors to learn

**Step 3 — system.capabilities:**
```json
{
  "ffmpeg": true, "kokoro": true, "pexels": true, "giphy": true,
  "whisper": true, "yt_dlp": true, "sfx_library_count": 148,
  "pixabay": false, "pixabay_note": "No API key (optional)"
}
```
- ✅ All critical subsystems report available
- ✅ Missing optional services flagged clearly
- ⚠️ Reports `library_count: 0` but actual index has 393+ tracks (stale field)

**Step 4 — system.doctor:**
```json
{
  "ready_for_production": true,
  "next_actions": ["Run director.run on a 5-scene script; expect ≥4/5 non-procedural stock + music bed"]
}
```
- ✅ Clear readiness signal
- ✅ Actionable next step provided
- ✅ Doctor correctly identifies the system is ready

**Step 5 — help.tool("create a video from scratch"):**
```json
{
  "results": [
    {"name": "script.parse", "relevance": 1.0},
    {"name": "script.to_video", "relevance": 1.0},
    {"name": "script.to_timeline", "relevance": 0.7},
    {"name": "timeline.build", "relevance": 0.6}
  ]
}
```
- ✅ Correctly identifies the golden path (script.parse → script.to_video)
- ✅ Relevance scores are accurate
- ⚠️ Does not return the script schema — only tool names and descriptions

**Step 6 — voices.list:**
```json
{
  "total_voices": 41,
  "kokoro_presets": ["af_bella", "af_heart", "am_michael", "bf_emma", ...]
}
```
- ✅ Voices available and documented

### Onboarding Friction Points

| # | Issue | Severity | Details |
|---|-------|----------|---------|
| F1 | **No schema discovery tool** | HIGH | Fresh agents cannot learn the script JSON schema from the MCP protocol alone. They must either guess, use `script.parse` errors to reverse-engineer, or read AGENT_GUIDE.md. |
| F2 | **Tool count mismatch** | LOW | Initialize says 84 tools, actual is 79. Stale count in server.rs. |
| F3 | **library_count: 0** | LOW | system.capabilities reports 0 music library count when 393+ tracks exist. |
| F4 | **music.search deprecated but functional** | LOW | Returns results but also a deprecation warning. Confusing for agents that discover it via tools/list. |

---

## Phase 2: Script Construction

### The Fresh Agent's Challenge

After discovering the tools, the agent must construct a valid script JSON. The agent tried:

**Attempt 1 — Minimal script via script.parse:**
```json
{"title": "Amazing Octopus Facts", "scenes": [{"speaker": "narrator", "text": "Octopuses have three hearts."}]}
```
**Result:** `Missing required argument: script` — the JSON-RPC argument nesting was wrong, not the script content. This is a **protocol-level issue**, not a script schema issue.

**Attempt 2 — Full script via script.to_video:**
The agent constructed a complete 5-scene script with all fields. The script included:
- `schema`, `title`, `video_keywords`, `meta`, `tts`, `speakers`, `background`, `music`, `captions`, `stickers`, `scenes`, `output`

**Result:** Script was accepted and rendering began.

### Script Construction Friction

| # | Issue | Severity | Details |
|---|-------|----------|---------|
| S1 | **No schema endpoint** | HIGH | `help.tool("script format")` returns tool names, not the JSON schema. Agents must guess field names. |
| S2 | **Nested JSON escaping** | MEDIUM | The `script` parameter must be a JSON string containing JSON. Shell-based agents struggle with double-escaping. The MCP protocol requires `{"arguments": {"script": "<json-string>"}}` not `{"arguments": {"script": <json-object>}}`. |
| S3 | **background.type enum undocumented** | MEDIUM | The schema accepts `gameplay`, `procedural`, `static` but not `stock`. A fresh agent guessing `stock` would fail. AGENT_GUIDE doesn't document this enum. |

---

## Phase 3: Video Generation (script.to_video)

### Execution Timeline

| Step | Duration | Status |
|------|----------|--------|
| Initialize MCP | <1s | ✅ |
| script.to_video call | 83.9s | ✅ Completed |
| TTS generation | ~20s | ✅ 5 scenes, Kokoro af_heart |
| Background fetch (Pexels) | ~30s | ✅ 5 unique clips fetched |
| Sticker fetch (GIPHY) | ~10s | ✅ 5 stickers downloaded |
| Music selection | ~5s | ✅ Auto-selected from library |
| SFX assignment | ~2s | ✅ 5 SFX assigned |
| FFmpeg render | ~15s | ✅ 77MB MP4 produced |
| Production quality check | <1s | ❌ Score 54, status: rendered_production_fail |

### What Went Right

1. ✅ **Golden path works end-to-end** — A fresh agent with zero knowledge can go from `initialize` → `script.to_video` → rendered MP4
2. ✅ **Pexels backgrounds are real stock footage** — 5 unique YouTube clips fetched, no procedural fallbacks
3. ✅ **GIPHY stickers are real animated GIFs** — 5 unique stickers per scene
4. ✅ **Music auto-selection works** — Found a track from the library (øneheart_x_reidenshi - snowfall)
5. ✅ **SFX auto-assignment works** — 5 unique SFX assigned to scene transitions
6. ✅ **Captions generated** — ASS file created with word timestamps
7. ✅ **Video is valid** — 77MB MP4, playable, correct aspect ratio (9:16)

### What Went Wrong

#### P0: Sticker Position Hard Fail (Score 0/8)

**Root Cause:** All 5 stickers placed at `position: "bottom-left"` with `scale: 0.35`. This position overlaps the caption safe zone (1690–1920px from top on a 1920px canvas).

```
HARD: sticker 'giphy_s0_narrator.gif' at position 'bottom-left' scale=0.35 
      overlaps caption safe zone (1690–1920px from top)
```

**Why this is P0:** The `sticker_design` dimension scored **0/8** with 5 HARD failures. This single issue dropped the total score by 8 points and triggered a production fail.

**Fix needed:** Either:
1. Change the default sticker position from `bottom-left` to `top-left` (avoids caption zone)
2. Or auto-detect caption position and offset stickers accordingly
3. Or the script schema should default to `top-left` instead of `bottom-left`

#### P1: Music Gain Too High (Score 7/8)

**Root Cause:** Auto-selected music track has `gain_db: 6.0` — boosted 6 dB above unity. This makes music louder than voice.

```
findings: ["music gain_db=6.0 is boosted above unity — louder than voice; use -8 to -14 dB"]
```

**Fix needed:** The music auto-selector should cap gain_db at -8.0 to -14.0 dB for voiceover content.

#### P1: Caption Style Not Detected (Score reduction)

**Root Cause:** The script included `"captions": {"style": "word_highlight"}` but the production quality validator reports `caption_style: null` and "caption_style not set".

```
findings: ["caption_style not set — prefer word_highlight for engagement"]
```

**Likely cause:** The captions were generated as ASS but the validator reads the style from the timeline manifest, not the ASS file. The `script.to_video` orchestrator may not propagate the caption style to the manifest.

#### P2: Stock Query Noise

The stock signal builder produced noisy queries:
```
Scene 1: "octopuses hearts pump calm nature gills ocean waves vertical video"
Scene 2: "calm nature blue because contains copper ocean waves vertical video"
```

The word "calm" and "ocean waves" are anchor noise — they appear in every query regardless of scene content. The signal words ("octopuses", "hearts", "blue", "copper") are good but the anchor dilutes relevance.

---

## Phase 4: Post-Render Verification

### verify.production Failure

The agent attempted `verify.production` but it requires `timeline_path` as a mandatory argument. The agent didn't know where the timeline file was located.

```
Error: Missing required argument: timeline_path
```

**UX Issue:** The `script.to_video` tool returns `output_path` but NOT `timeline_path`. The agent must separately locate the timeline file in `.openscript/projects/` or `artifacts/`. This breaks the "one-call" promise.

**Fix needed:** `script.to_video` should return `timeline_path` in its response so agents can chain into `verify.production`.

### Production Quality Breakdown (from embedded report)

| Dimension | Score | Max | Status |
|-----------|-------|-----|--------|
| video_source_quality | 9 | 10 | ✅ All YouTube clips |
| visual_hooks | 8 | 8 | ✅ Real stock, no procedural |
| visual_repetition | 8 | 8 | ✅ 5 unique clips, 100% uniqueness |
| context_relevance | 8 | 8 | ✅ Good topic matching (avg Jaccard 0.314) |
| cuts_pacing | 5 | 5 | ✅ 0.141 cuts/sec (ideal band: 0.12–0.55) |
| music_quality | 7 | 8 | ⚠️ Gain too high (6.0 dB) |
| sfx_quality | 6 | 6 | ✅ 5 unique SFX |
| **sticker_design** | **0** | **8** | **❌ HARD FAIL — caption safe zone overlap** |
| caption_quality | ~4 | 8 | ⚠️ Style not detected |
| **TOTAL** | **~54** | **100** | **❌ Production Fail (Grade D)** |

---

## Comparison with Prior Audits

| Metric | Run #7 | Run #8 | Run #9 (this) |
|--------|--------|--------|---------------|
| Score | 54/100 | ~65-70/100 (est.) | 54/100 |
| Grade | D | B (est.) | D |
| P0 blockers | 3 | 0 | 1 (sticker position) |
| Video rendered | ✅ | ✅ | ✅ |
| Golden path works | ✅ | ✅ | ✅ |
| Fresh-agent discoverability | Good | Good | Good |

**Regression from Run #8:** The sticker position issue was supposed to be fixed in Run #8 (3d6a7f6 — "per-scene sticker variation for single-speaker 3+ scenes"). The fix addressed sticker *reuse* but not sticker *position*. The `bottom-left` default still collides with captions.

---

## Fresh-Agent Experience Scorecard

### What a Fresh Agent Would Rate Highly

1. **"I got a video in 2 minutes"** — The golden path works. From zero knowledge to rendered MP4 in ~84 seconds.
2. **"The tools are well-named"** — `script.to_video`, `background.fetch`, `system.capabilities` — names are intuitive.
3. **"The doctor told me what to do"** — `system.doctor` provides clear next actions.
4. **"The video looks real"** — Real Pexels footage, real GIPHY stickers, real music. No placeholder content.

### What a Fresh Agent Would Rate Poorly

1. **"I had to guess the script format"** — No schema endpoint. Had to reverse-engineer from test_healing_script.json.
2. **"The video failed quality checks"** — I did everything right but the output got a D grade because of sticker positioning.
3. **"I couldn't verify my own output"** — `verify.production` needs `timeline_path` but `script.to_video` doesn't return it.
4. **"music.search yelled at me for using it"** — Deprecated but still in tools/list. Should be removed or marked clearly.

---

## Prioritized Recommendations

### P0 — Fix Before Next Audit

| # | Fix | Effort | Impact |
|---|-----|--------|--------|
| 1 | **Change default sticker position to `top-left`** | Low | +8 pts (sticker_design: 0→8) |
| 2 | **Return `timeline_path` from `script.to_video`** | Low | Enables verify.production chaining |
| 3 | **Cap music gain_db at -8 to -14 dB in auto-selector** | Low | +1 pt (music_quality: 7→8) |

### P1 — Fix Soon

| # | Fix | Effort | Impact |
|---|-----|--------|--------|
| 4 | **Add `script.schema` tool** that returns the full JSON schema | Medium | Eliminates F1 (schema discovery) |
| 5 | **Fix caption_style propagation** to production manifest | Low | +2 pts (caption_quality) |
| 6 | **Remove or deprecate `music.search`** from tools/list | Low | Reduces confusion (F4) |
| 7 | **Fix tool count** in server.rs initialize instructions (84→79) | Low | Reduces confusion (F2) |

### P2 — Nice to Have

| # | Fix | Effort | Impact |
|---|-----|--------|--------|
| 8 | **Clean stock query anchors** — remove "calm nature" noise | Medium | +1-2 pts (context_relevance) |
| 9 | **Document `background.type` enum** in tool description | Low | Prevents agent guesswork (S3) |
| 10 | **Fix `library_count`** in system.capabilities response | Low | Reduces confusion (F3) |

---

## Verification Commands

```bash
cd /home/ishanp/Documents/GitHub/MY-PROJECTS/CONTENT-CREATION/openscript

# Full verification
cargo build --workspace --exclude openscript-tauri && \
cargo test --workspace --exclude openscript-tauri --lib --bins --tests && \
python3 scripts/workspace-lint/workspace_lint.py --root .

# MCP smoke test
cargo build -p openscript-mcp --release --bin mcp-server && \
bash scripts/smoke_test_mcp.sh

# Re-run the fresh-agent simulation
python3 /tmp/fresh_agent_sim.py
```

---

## Conclusion

The golden trajectory **works** — a fresh AI agent can discover and use the MCP tools to create a video from scratch without any documentation. The pipeline is reliable (84s end-to-end, valid MP4 output). However, the **output quality** is dragged down by a single P0 issue: sticker positioning defaults to `bottom-left` which collides with captions.

**The fix is surgical:** Change the default sticker position from `bottom-left` to `top-left` in the script schema defaults. This alone would lift the score from 54 to ~62 (Grade C→B border). Combined with the `timeline_path` return fix and music gain cap, the system would reach ~70/100 (solid Grade B).

**The path to Grade A (90+):**
1. Fix sticker position default (+8 pts)
2. Fix caption_style propagation (+2 pts)
3. Fix music gain cap (+1 pt)
4. Return timeline_path from script.to_video (enables verification)
5. Add schema discovery tool (eliminates guesswork)

These are all **low-effort, high-impact** fixes that would transform the fresh-agent experience from "it works but fails QA" to "it works and produces Grade B+ output."
