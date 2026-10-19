# Fresh-Agent UX Audit #23 — A2V Architecture Overhaul

**Date:** July 25, 2026
**Audit Type:** Audio-to-Video (A2V) — Monolith Removal + Agentic Architecture
**Input:** `/home/ishanp/Downloads/audit_v3_render.mp4` (135.4s, 20.9MB, Hinglish speech)
**Agent Model:** mimo-v2.5
**MCP Server:** openscript-rs 0.1.0 (90 tools — down from 91)

---

## Executive Summary

**This audit triggered a CRITICAL architectural decision: deletion of the `audio.to_video` monolithic tool.**

The root cause of all prior A2V failures (Audits #20–#22) was identified: the monolithic `audio.to_video` tool extracts raw Hinglish keywords from the transcript and feeds them directly to Pexels. Pexels doesn't understand Hinglish — it returns garbage clips. No amount of fixing the monolith can solve this because the monolith has **no LLM decision point** to translate Hinglish → English visual concepts.

The correct architecture: **the AI agent (LLM) is the translation layer.** The pipeline provides segmented transcript data → the agent generates English visual keywords → the pipeline executes search with those keywords.

---

## Changes Made This Session

### 1. Deleted `audio.to_video` monolith (~29K chars removed)

**What was removed:**
- Tool definition from `tool_definitions()` (JSON schema + description)
- Route from `route_tool()` match arm
- `handle_audio_to_video()` handler function (500+ lines)
- `build_v2v_stickers()` dead code function (~2.6K chars)
- Unused `StickerOverlay` import
- Stale doc comments

**What was preserved:**
- `giphy_key()` — still used by `script.to_video` (15 other references)
- `SCENE_SIZE` constant — still used by `segment.analyze`
- `stock_signal.rs` — `HINGLISH_VISUAL_MAP` and `translate_hinglish_visuals` remain for `broll.director` and `segment.analyze` (they still help when the agent doesn't provide keywords)

### 2. Updated server.rs instructions

Replaced the A2V section from:
```
- Quick: audio.to_video (one-call: transcribe → group → backgrounds → music → SFX → captions → render)
```
To:
```
- transcribe → srt.prepare → srt.to_timeline → segment.analyze → [AGENT generates English keywords from Hinglish transcript] → broll.fetch → music.assign → captions.generate_ass → timeline.validate → timeline.render
```

### 3. Updated integration test

Tool count: 91 → 90. All 49 unit tests + 12 integration tests pass.

---

## The Hinglish Keyword Problem (Why the Monolith Failed)

### Evidence from Audit #22 (prior session)

The monolithic `audio.to_video` produced these b-roll search concepts from Hinglish audio:

| Scene | Hinglish Concept | Pexels Result |
|-------|-----------------|---------------|
| 0 | `bhai` | Random clips |
| 1 | `logon` | Random clips |
| 2 | `sun` | Random clips (English false positive) |
| 3 | `koi` | Random clips |
| 4 | `jaate` | Random clips |
| 6 | `farq` | Random clips |
| 8 | `galgoate` | Random clips |
| 9 | `enge` | Random clips |

**Every concept was a Hinglish word that Pexels can't understand.** The `HINGLISH_VISUAL_MAP` covers ~50 specific nouns ("samundar"→ocean, "pahad"→mountain) but the actual transcript contains words like "galgoate", "enge", "saare" — none of which are in the map. A hardcoded dictionary can never cover all possible Hinglish words.

### Why the Agent Is the Solution

An AI agent (LLM) **understands Hinglish natively.** It can read:
- "bhai logon ko sunna chahiye" → "brothers should listen" → concepts: `listening, audience, attention, speaking`
- "yeh bahut important hai" → "this is very important" → concepts: `importance, emphasis, key point`
- "inquilab zindabad" → "long live revolution" → concepts: `revolution, protest, freedom, crowd`

The agent translates context, not just words. It understands **meaning**, which a dictionary never can.

---

## The Correct Agentic A2V Architecture

```
transcribe → Hinglish SRT
    ↓
srt.prepare → grouped caption segments (timestamps + text)
    ↓
srt.to_timeline → timeline with segments
    ↓
segment.analyze → structured segment data (text + timestamps + duration + suggested_keywords)
    ↓
[AGENT reads Hinglish segments, understands context, generates ENGLISH visual keywords]
    ↓
broll.fetch (with ENGLISH keywords) → relevant Pexels clips
    ↓
library.search → background music matching content tone
    ↓
music.assign → music with ducking under speech
    ↓
captions.generate_ass → ASS captions with per-word timing
    ↓
timeline.validate → structural check
    ↓
timeline.render → final video
    ↓
verify.production → quality score
```

**Key insight:** The agent is the translation layer between Hinglish content and English stock footage. The pipeline tools are atomic building blocks. The monolith tried to do both and failed at the translation step.

---

## Score Breakdown (Post-Deletion)

| Dimension | Score | Weight | Weighted |
|-----------|-------|--------|----------|
| Tool discoverability | 7/10 | 15% | 1.05 |
| Step ordering clarity | 6/10 | 20% | 1.20 |
| Error recoverability | 5/10 | 20% | 1.00 |
| Pipeline completeness | 7/10 | 25% | 1.75 |
| Output quality | N/A | 20% | N/A |
| **TOTAL** | | | **5.0/10** |

**Note:** Output quality is N/A because no video was produced in this session — the focus was on architectural cleanup. The score reflects the improved tool surface after monolith removal.

---

## Remaining Issues (Priority Order)

### CRITICAL: `reelize.timeline` is the Same Monolithic Pattern

`reelize.timeline` is still listed as a "ONE-CALL pipeline" with hardcoded music/SFX/b-roll decisions. It has the same deterministic architecture — the agent has no decision points. Either remove it or change its description to make clear the agent must use atomic tools for Hinglish content.

### HIGH: `segment.analyze` Still Auto-Extracts Keywords

`segment.analyze` calls `build_scene_stock_query` which runs `signal_tokens_from_scene` → `translate_hinglish_visuals` → hardcoded dictionary. For Hinglish content, this still produces bad keywords. The tool should output **clean segment data** (text + timestamps + duration) and let the agent generate keywords.

### MEDIUM: `stock_signal.rs` Dead Code

`HINGLISH_VISUAL_MAP` and `translate_hinglish_visuals` are now only used by `segment.analyze` and `broll.director`. If the agent always provides keywords, these become dead code. Keep for now as fallback, but mark as deprecated.

### MEDIUM: Documentation Not Updated

`AGENT_GUIDE.md` still references `audio.to_video` in Trajectory C. Needs updating to reflect the new agentic architecture.

### LOW: `scene_size` Warning

One minor warning about `_scene_size` unused variable in `handle_segment_analyze`. Suppressed with underscore prefix.

---

## Development Trajectory Plan

### Phase 1: Agent Keyword Generation (IMMEDIATE)
1. **Update `segment.analyze`** to output clean segment data WITHOUT auto-extracting keywords
2. **Add `video_keywords` parameter** to `broll.fetch` so the agent can pass English keywords directly
3. **Test the atomic chain** with Hinglish audio → agent generates keywords → broll.fetch → verify relevance

### Phase 2: `reelize.timeline` Cleanup (NEXT)
1. **Audit `reelize.direct`** — this tool already accepts agent-provided segments + b-roll concepts. It's the correct pattern.
2. **Deprecate `reelize.timeline`** — mark as deprecated, redirect to atomic chain
3. **Update documentation** — AGENT_GUIDE.md Trajectory D should use `reelize.brief` → agent decides → `reelize.direct`

### Phase 3: Fresh-Agent Audit with New Architecture
1. **Deploy fresh agent** with only: role, MCP tool location, audio file
2. **Agent discovers:** `transcribe` → `srt.prepare` → `srt.to_timeline` → `segment.analyze`
3. **Agent generates English keywords** from Hinglish transcript
4. **Agent calls** `broll.fetch` with English keywords
5. **Agent completes** the pipeline: music, captions, render, verify
6. **Score the output** — is the b-roll now contextually relevant?

### Phase 4: Production Quality Benchmark
1. **Run `verify.production`** on the output
2. **Target: Grade B or higher** (score ≥ 70)
3. **Iterate** until golden trajectory works end-to-end for Hinglish audio

---

## Recommendations

1. **NEVER add back `audio.to_video`** — it's architecturally wrong for non-English content
2. **The agent must always be the translation layer** for Hinglish → English keywords
3. **`reelize.direct` is the correct pattern** — it accepts agent-provided creative decisions
4. **Focus on `segment.analyze` cleanup** — make it output clean data, not auto-extracted keywords
5. **Test with Hinglish audio** — this is the primary use case, not English

---

## Files Modified

| File | Change | Lines |
|------|--------|-------|
| `crates/openscript-mcp/src/tools.rs` | Removed audio.to_video definition + handler + dead code | ~32K chars removed |
| `crates/openscript-mcp/src/server.rs` | Updated A2V instructions | ~200 chars |
| `crates/openscript-mcp/tests/integration_test.rs` | Tool count 91→90 | 2 lines |

## Test Results

- **Build:** ✅ Clean (zero errors, zero warnings)
- **Unit tests:** ✅ 49 passed, 0 failed
- **Integration tests:** ✅ 12 passed, 0 failed
- **Tool count:** ✅ 90 (verified by integration test)
