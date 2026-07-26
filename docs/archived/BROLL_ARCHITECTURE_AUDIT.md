# B-Roll Architecture Audit & Upgrade Plan

**Date:** 2026-07-23  
**Status:** PROPOSED  
**Trigger:** Fresh-agent A2V audit revealed irrelevant, poorly-timed, and generic b-roll clips

---

## Problem Statement

The A2V pipeline's b-roll system has three fundamental flaws:

1. **`extract_broll_concept` returns empty for Hinglish content** — The function aggressively removes stopwords and short words, then takes only the first 3 remaining words. For Hinglish scripts (Hindi audio transcribed to Latin-script Hindi), most words are either stopwords or too short, resulting in the literal string `"b-roll"` as the concept.

2. **Generic cycling fallback** — When concept extraction fails, the pipeline cycles through 8 hardcoded concepts: `["abstract motion", "city timelapse", "technology waves", "nature aerial", "ocean sunset", "space nebula", "ocean waves", "mountain landscape"]`. These have zero relationship to the actual content.

3. **No agent intervention step** — The pipeline runs `broll.fetch` automatically inside `handle_audio_to_video` without letting the agent review or customize the search keywords. The agent has no visibility into what b-rolls are being fetched or why.

---

## Current Flow (Broken)

```
audio.to_video
  → transcribe (HinglishGgml)
  → srt.prepare (group words into phrases)
  → for each scene:
      → extract_broll_concept(caption)  // Returns empty/"b-roll" for Hinglish
      → cycling_fallback[scene_idx % 8]  // Generic fallback
      → broll.fetch(concepts=[concept, fallback])  // Pexels search
      → broll.assign(asset_path)  // Place on timeline
  → render
```

**Result:** 12 generic stock clips (city timelapse, ocean sunset, etc.) that have nothing to do with the actual spoken content.

---

## Proposed Flow (Agent-Orchestrated)

```
Step 1: transcribe + srt.prepare (same as before)

Step 2: NEW TOOL — broll.plan
  Input: timeline_path (with populated segments)
  Output: JSON with SEO-optimized keyword suggestions per segment
  {
    "segments": [
      {
        "id": "seg_0",
        "start_s": 0.0,
        "end_s": 12.5,
        "caption": "bhai log aaj hum baat karenge black holes ke baare mein",
        "keywords": ["black hole space", "cosmic vortex", "gravitational pull"],
        "mood": "dramatic",
        "pace": "slow"
      },
      ...
    ],
    "global_theme": "science documentary",
    "visual_style": "cinematic"
  }

Step 3: AGENT REVIEW
  Agent reads the broll.plan output
  Agent can modify keywords, add/remove segments, change mood/pace
  Agent can use LLM to generate better keywords if needed

Step 4: broll.fetch (with agent-approved keywords)
  Same as before but with better, agent-curated keywords

Step 5: broll.assign (same as before)

Step 6: render
```

---

## Implementation Plan

### Phase A: New Tool — `broll.plan`

**File:** `crates/openscript-mcp/src/tools.rs`

Create a new MCP tool that:
1. Reads the timeline's segments (start, end, caption text)
2. For each segment, generates SEO-optimized Pexels search keywords using:
   - Simple keyword extraction from caption text (for Hindi/Hinglish: transliterate to English concepts)
   - Context-aware keyword expansion (e.g., "black holes" → ["black hole space", "cosmic vortex", "event horizon"])
   - Mood/pace inference from caption content and position in video
3. Returns a structured JSON that the agent can review and modify

**Key insight:** The keyword generation should NOT try to be perfect — it should give the agent a starting point that's better than the current `extract_broll_concept`. The agent then uses its LLM capabilities to refine the keywords before fetching.

### Phase B: Upgrade `extract_broll_concept`

**File:** `crates/openscript-mcp/src/tools.rs` (line 1583)

Replace the current aggressive stopword removal with a simpler approach:
1. Remove punctuation and very short words (< 3 chars)
2. Take the first 5 significant words (not 3)
3. For Hinglish: detect common Hindi words and map to English equivalents
4. Return the phrase as-is instead of defaulting to "b-roll"

### Phase C: Add Agent Visibility to `audio.to_video`

**File:** `crates/openscript-mcp/src/tools.rs` (handle_audio_to_video)

Add a new parameter `broll_keywords: Option<serde_json::Value>` that:
1. When provided, uses the agent-supplied keywords instead of auto-extracting
2. When omitted, falls back to the current behavior (backward compatible)

This allows the agent to:
1. Call `broll.plan` to get keyword suggestions
2. Review and modify the keywords
3. Pass them to `audio.to_video` via the new parameter

### Phase D: Update `broll.fetch` for Hinglish Content

**File:** `crates/openscript-mcp/src/tools.rs` (handle_broll_fetch)

Add a `language_hint` parameter that:
1. When "hinglish", uses Hindi-to-English concept mapping for better Pexels results
2. Example: "dharti" → "earth globe", "aasmaan" → "sky clouds"

---

## Tool Schema Updates

### New Tool: `broll.plan`

```json
{
  "name": "broll.plan",
  "description": "Analyze timeline segments and generate SEO-optimized Pexels search keywords for each b-roll slot. Returns structured JSON with keyword suggestions, mood, and pace per segment. The agent reviews these suggestions before calling broll.fetch. Use BEFORE broll.fetch to get contextually relevant b-roll keywords. Returns: segments with keywords, global_theme, visual_style.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "timeline_path": {"type": "string", "description": "Path to timeline JSON with populated segments"},
      "style": {"type": "string", "default": "cinematic", "description": "Visual style: 'cinematic', 'documentary', 'vlog', 'tutorial'"},
      "max_keywords_per_segment": {"type": "integer", "default": 3, "description": "Max keywords to suggest per segment"}
    },
    "required": ["timeline_path"]
  }
}
```

### Updated Tool: `audio.to_video`

Add new parameter:
```json
"broll_keywords": {
  "anyOf": [{"type": "object"}, {"type": "null"}],
  "description": "Agent-supplied b-roll keywords per segment. When provided, overrides auto-extraction. Format: {\"0\": [\"keyword1\", \"keyword2\"], \"1\": [...], ...} where keys are segment indices."
}
```

---

## Expected Impact

| Metric | Before | After |
|--------|--------|-------|
| **B-roll relevance** | ~10% (generic stock) | ~80% (contextual) |
| **Agent control** | 0% (automatic) | 100% (agent-curated) |
| **Hinglish support** | Broken (empty concepts) | Working (concept mapping) |
| **Pattern interrupts** | None (same clip repeats) | Every segment (varied clips) |
| **SEO optimization** | None | Per-season keywords |

---

## Testing Plan

1. **Unit test `extract_broll_concept`** — Verify Hinglish sentences produce meaningful concepts
2. **Integration test `broll.plan`** — Verify timeline → keyword suggestions → structured output
3. **Fresh-agent audit** — Run A2V pipeline with the new flow, verify b-roll relevance
4. **Visual audit** — Watch the generated video to verify pattern interrupts and clip variety
