# A2V B-Roll Architecture Review — Refined Plan

**Date:** 2026-07-24
**Status:** PROPOSAL — Awaiting review before implementation

---

## 1. Problem Statement

The `audio.to_video` pipeline uses `extract_broll_concept()` (tools.rs:1599) — a simple stopword filter that takes the first 3 non-stopword words from each caption. For Hinglish content, most words get filtered, leaving generic/repetitive queries. This produces the **"single video on loop"** problem: all scenes get nearly identical b-roll because the keyword extraction is too simplistic.

Meanwhile, `stock_signal::build_scene_stock_query()` (stock_signal.rs:408) already implements sophisticated topic-aware query building with visual anchors, signal tokens, and topic detection — but it's only wired into `script.to_video`, not `audio.to_video`.

---

## 2. Current Architecture (Broken)

```
audio.to_video (ONE-CALL)
  │
  ├─ transcribe(audio) → SRT with word timestamps
  ├─ srt.prepare(SRT) → grouped segments
  │
  ├─ FOR EACH segment:
  │   ├─ extract_broll_concept(caption)     ← SIMPLE STOPWORD FILTER (tools.rs:1599)
  │   │   └─ takes first 3 non-stopword words
  │   │   └─ for Hinglish: "bhai sarkaar phati" → query "bhai sarkaar phati"
  │   │
  │   ├─ handle_broll_fetch(concepts)        ← PEXELS SEARCH
  │   │   └─ takes FIRST result (results_arr[0])
  │   │   └─ looped: true (short clip fills entire scene)
  │   │
  │   └─ background_clips.push(clip)
  │
  └─ render_multilayer(spec with clips)
```

**Root causes:**
1. `extract_broll_concept()` is a naive stopword filter — no topic awareness, no visual anchors
2. `stock_signal::build_scene_stock_query()` exists but is NOT used in A2V pipeline
3. Agent has no way to see segments before keywords are auto-generated
4. No content-hash dedup — same clip can be used for multiple scenes

---

## 3. Proposed Architecture (Refined)

### Core Principle: **Separate ANALYSIS from EXECUTION**

The agent needs two things:
1. **What segments exist** (analysis) — timestamps, text, suggested keywords
2. **What clips to use** (execution) — agent creates keywords, then fetches + renders

### New Tool: `segment.analyze`

```json
{
  "name": "segment.analyze",
  "description": "Analyze a transcript or audio file and return structured segments with ideal clip durations, suggested b-roll keywords, and visual anchors. This is a PURE ANALYSIS tool — it does NOT fetch any b-roll or render any video. Use this to understand what segments exist before creating keywords for broll.fetch. Returns: segments array with id, start_s, end_s, duration_s, caption, suggested_keywords, visual_anchor, topic_category.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "audio_path": {"type": "string", "description": "Path to audio/video file to analyze"},
      "srt_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Pre-existing SRT (skip transcription)"},
      "video_keywords": {"type": "array", "items": {"type": "string"}, "description": "Whole-video topic keywords for context-aware analysis"},
      "theme": {"type": "string", "default": "neutral", "description": "Content theme for visual anchor selection"},
      "aspect": {"type": "string", "default": "9:16", "description": "Target aspect ratio for orientation bias"}
    },
    "required": ["audio_path"]
  }
}
```

**Internal processing:**
1. Transcribe audio (if no SRT provided)
2. Group words into segments via `srt.prepare`
3. For each segment, call `stock_signal::build_scene_stock_query()` with the segment text + video_keywords
4. Return segments with:
   - `id`, `start_s`, `end_s`, `duration_s`
   - `caption` (text content)
   - `suggested_keywords` (from stock_signal signal tokens)
   - `visual_anchor` (from stock_signal topic-aware anchor bank)
   - `topic_category` (Space, Science, Nature, Marine, Tech, Lifestyle)

### Modified Tool: `audio.to_video`

**Remove:** `extract_broll_concept()` call (tools.rs:1546-1562)
**Add:** `broll.keywords` parameter support (already exists in schema!)

```rust
// BEFORE (broken):
let base_concept = extract_broll_concept(caption);  // naive stopword filter
if significant_words.len() >= 2 {
    let phrase = format!("{} {}", significant_words[0], significant_words[1]);
    concepts.push(phrase);
}

// AFTER (fixed):
let stock_q = if let Some(ref custom_q) = agent_broll_keywords.get(scene_idx) {
    // Agent provided keywords — use directly
    SceneStockQuery {
        query: custom_q.clone(),
        signal_tokens: tokenize(custom_q),
        visual_anchor: custom_q.clone(),
        scene_idx,
    }
} else {
    // No agent keywords — use sophisticated stock_signal analysis
    stock_signal::build_scene_stock_query(
        scene_text,
        &video_keywords,
        &theme,
        &aspect,
        scene_idx,
    )
};
```

### Data Flow: Two Paths

#### Path A: Quick One-Call (No Agent Intervention)

```
audio.to_video(audio_path)
  │
  ├─ transcribe(audio) → SRT
  ├─ srt.prepare(SRT) → grouped segments
  │
  ├─ FOR EACH segment:
  │   ├─ stock_signal::build_scene_stock_query(text, keywords, theme, aspect, idx)
  │   │   └─ topic detection → visual anchor → signal tokens → query
  │   │
  │   ├─ handle_broll_fetch(query) → Pexels search
  │   │   └─ content-hash dedup (reject previously used clips)
  │   │   └─ take best unique result
  │   │
  │   └─ background_clips.push(clip)
  │
  └─ render_multilayer(spec)
```

#### Path B: Agentic Multi-Step (Full Agent Control)

```
Step 1: segment.analyze(audio_path)
  → returns: [{id, start_s, end_s, caption, suggested_keywords, visual_anchor}, ...]

Step 2: AGENT reviews segments
  → reads suggested_keywords
  → creates custom keywords using LLM reasoning
  → e.g., "coffee beans roasting closeup" instead of "hai bhai aavaaz"

Step 3: FOR EACH segment:
  broll.fetch(concepts=[agent_keyword], download=true)
  → returns: [{concept, cached_path, videos}, ...]

Step 4: audio.to_video(audio_path, broll.keywords=[agent_keywords])
  → uses agent's keywords directly (bypasses auto-extraction)
  → renders final video
```

---

## 4. Key Design Decisions

### 4.1 Why NOT just fix `extract_broll_concept`?

`extract_broll_concept` is fundamentally flawed — it operates on raw caption text with no context. Even with better stopword filtering, it can't:
- Detect topic category (Space vs Marine vs Tech)
- Pick visual anchors from topic-specific banks
- Weight visual nouns higher than generic words
- Apply theme-aware query enrichment

`stock_signal::build_scene_stock_query()` does ALL of this. The fix is to wire it in, not patch the naive version.

### 4.2 Why a NEW `segment.analyze` tool instead of reusing `broll.plan`?

`broll.plan` requires a **timeline** to already exist. For A2V, we start from raw audio — no timeline yet. `segment.analyze` works from audio/SRT directly, before any timeline is built.

### 4.3 Why keep `broll.keywords` on `audio.to_video`?

For the one-call golden path, the agent shouldn't need to call 3 tools. `audio.to_video` with `broll.keywords` lets the agent provide keywords in a single call while still getting the full pipeline.

### 4.4 Content-Hash Dedup

Add a `HashSet<String>` tracking content hashes of downloaded clips. Before assigning a clip to a scene, check if its hash was already used. If yes, skip to next result or refetch with diversified query.

---

## 5. Implementation Phases

### Phase 1: Wire `stock_signal` into `audio.to_video` (TRIVIAL)

**Files:** `crates/openscript-mcp/src/tools.rs`
**Changes:**
- Remove `extract_broll_concept()` call at line 1546-1562
- Replace with `stock_signal::build_scene_stock_query()` call
- Auto-extract `video_keywords` from transcript if not provided
- Add content-hash dedup tracking

**Effort:** 1-2 hours
**Risk:** Low — stock_signal is battle-tested in `script.to_video`

### Phase 2: Add `segment.analyze` tool (MEDIUM)

**Files:** `crates/openscript-mcp/src/tools.rs` (new handler + tool definition)
**Changes:**
- Add `segment.analyze` to `tool_definitions()` array
- Add `handle_segment_analyze()` handler
- Wire into `route_tool()` dispatcher
- Update tool count in `server.rs`, `AGENT_GUIDE.md`, `smoke_test_mcp.sh`
- Add integration test

**Effort:** 3-4 hours
**Risk:** Medium — new tool, needs thorough testing

### Phase 3: Content-Hash Dedup (MEDIUM)

**Files:** `crates/openscript-mcp/src/tools.rs`
**Changes:**
- Add `content_hash` field to `BackgroundClip` struct
- Track used hashes in `audio.to_video` and `broll.fetch`
- Skip clips with duplicate hashes
- Refetch with diversified query if top result is a duplicate

**Effort:** 2-3 hours
**Risk:** Medium — needs careful edge case handling

### Phase 4: Auto-Extract `video_keywords` from Transcript (LOW)

**Files:** `crates/openscript-mcp/src/tools.rs`
**Changes:**
- After transcription, extract top-5 visual nouns from all segments
- Use as `video_keywords` when agent doesn't provide them
- Feed into `stock_signal::build_scene_stock_query()` for topic detection

**Effort:** 1-2 hours
**Risk:** Low — straightforward NLP extraction

---

## 6. Expected Outcomes

| Metric | Before | After |
|--------|--------|-------|
| B-roll diversity | 1-2 unique clips across all scenes | Unique clip per scene |
| Keyword quality | Generic stopword-filtered | Topic-aware with visual anchors |
| Agent control | None (auto-generated) | Full control via `segment.analyze` + `broll.keywords` |
| Hinglish support | Broken (words filtered out) | Works (stock_signal handles noise tokens) |
| Production KPI | Grade D (54/100) | Expected Grade B+ (75+) |

---

## 7. Testing Strategy

1. **Unit tests:** Add tests for `segment.analyze` output structure
2. **Integration test:** Call `segment.analyze` → verify segments have unique keywords
3. **End-to-end:** Run `audio.to_video` on test audio → verify `verify.production` grade improves
4. **Regression:** Ensure `script.to_video` still works (shared `stock_signal` code)
5. **Fresh-agent audit:** Re-run Audit #20 protocol → measure improvement

---

## 8. Open Questions

1. Should `segment.analyze` also return `video_keywords` auto-extracted from the full transcript? (Yes — helps agent understand global context)
2. Should we add a `broll.dedup` parameter to `audio.to_video` to toggle dedup behavior? (Probably not — dedup should always be on)
3. How to handle the case where Pexels returns 0 results for a scene? (Fall back to `stock_signal` visual anchor bank rotation)
