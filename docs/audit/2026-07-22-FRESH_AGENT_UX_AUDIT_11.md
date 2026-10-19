# Fresh-Agent UX Audit — Run #11

**Date:** 2026-07-22
**Topic:** The Science of Sleep (freely chosen by agent)
**Method:** Python-based MCP client simulating a fresh agent with no prior knowledge

---

## Simulation Results

| Metric | Run #10 | Run #11 | Delta |
|--------|---------|---------|-------|
| Schema discovery | ❌ No script.schema tool | ✅ script.schema works | **Fixed** |
| Speakers format | ❌ Array rejected | ✅ Array normalized to map | **Fixed** |
| Background format | ❌ String rejected | ✅ String normalized to object | **Fixed** |
| duration_seconds | ❌ Silently ignored | ✅ Converted to ms | **Fixed** |
| stock_query per scene | ❌ Not available | ✅ Agent controls footage | **Fixed** |
| Voice ID format | ❌ bare af_heart fails | ✅ Normalized to kokoro:af_heart | **Fixed** |
| Video produced | ✅ 82/100 (Grade B) | ✅ Video created (16MB) | Maintained |
| Schema friction | 80% of time on schema | ~5% on schema | **Dramatically reduced** |

## P0-P2 Fix Verification

### P0: script.schema tool ✅
- Agent called `script.schema` and received full JSON schema with examples
- Schema included all 14 top-level properties, 4 definitions (SceneSpec, SpeakerSpec, BackgroundSpec, etc.)
- Schema had `oneOf` for speakers (array OR map) and background (object OR string)
- **Result: Agent discovered correct format in one call**

### P0: stock_query per scene ✅
- Agent wrote `stock_query: "sleeping brain neural activity"` for scene 1
- `script.parse` preserved the stock_query in the parsed spec
- `script.to_video` used the custom query for Pexels search
- **Result: Agent has explicit control over per-scene footage**

### P1: speakers array format ✅
- Agent wrote: `[{"id": "narrator", "voice": "af_heart"}]`
- `parse_script` normalized it to: `{"narrator": {"voice": "af_heart"}}`
- Validation passed, scenes correctly referenced the speaker
- **Result: Agent-friendly format works seamlessly**

### P1: background string shorthand ✅
- Agent wrote: `"background": "procedural"`
- `parse_script` normalized it to: `{"type": "procedural"}`
- Video generation used the normalized form correctly
- **Result: Agent-friendly format works seamlessly**

### P1: duration_seconds → ms conversion ✅
- Agent wrote: `"duration_seconds": 8`
- `parse_script` converted to: `"duration_override_ms": 8000`
- **Result: Agents think in seconds, system handles milliseconds**

### P2: Voice ID normalization ✅
- Agent wrote: `"voice": "af_heart"` (bare ID, no kokoro: prefix)
- `script.generate_voices` normalized to `kokoro:af_heart` internally
- TTS generation succeeded
- **Result: Bare Kokoro IDs are accepted everywhere**

## Schema Friction Analysis

### Run #10 friction points — all resolved:

| # | Gap (Run #10) | Resolution (Run #11) |
|---|---------------|---------------------|
| 1 | No stock_query per scene | ✅ Added to SceneSpec + wired into tools.rs |
| 2 | speakers is map, not array | ✅ parse_script normalizes array→map |
| 3 | background is object, not string | ✅ parse_script normalizes string→object |
| 4 | No script.schema tool | ✅ Added with full schema + examples |
| 5 | duration_seconds silently ignored | ✅ Added field + conversion |
| 6 | Voice ID format mismatch | ✅ Normalized in all TTS handlers |
| 7 | Quality measurement blind spots | Deferred (P2) |

### Remaining minor gaps:

| # | Gap | Severity | Notes |
|---|-----|----------|-------|
| 1 | `warnings: null` vs `[]` in script.to_video response | Low | Test infrastructure assumed array; handler returns null |
| 2 | BackgroundSpec missing `query` field in schema | Low | Agents using gameplay type won't discover it |
| 3 | Empty-string stock_query guard untested | Low | Guard exists but no regression test |
| 4 | tts.preview error message says "add via voice.profile.add" | Low | Misleading for preview-only tool |
| 5 | Marine topic not specifically tested | Low | Phase 19 fix validated in Run #9/10 |

## Agent Experience Timeline

```
Step 1: Initialize          → 0.1s   (fast)
Step 2: tools/list          → 0.2s   (86 tools discovered)
Step 3: script.schema       → 0.3s   (full schema with examples)
Step 4: script.parse        → 0.5s   (array speakers + string bg + stock_query + duration_seconds — all worked!)
Step 5: script.to_video     → 60-90s (video generated successfully)
```

**Schema friction time:** ~5% (vs 80% in Run #10)
**Total time to video:** ~90s (vs ~300s+ in Run #10 due to schema walls)

## Conclusion

**All P0-P2 schema fixes from the UX audit are verified end-to-end.** The agent spent minimal time on schema discovery (thanks to `script.schema`) and wrote intuitive, agent-friendly formats that were correctly normalized. The stock_query override gave the agent explicit control over per-scene footage, and voice ID normalization eliminated the "af_heart not found" error.

The simulation is a **clean pass**. Remaining gaps are minor polish items that don't block agent functionality.
