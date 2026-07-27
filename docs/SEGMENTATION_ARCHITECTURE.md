# Segmentation Architecture for OpenScript

> **Status:** Design document — not yet implemented
> **Last updated:** 2026-07-27
> **Purpose:** Define how OpenScript should segment transcripts into b-roll cuts for optimal viewer retention on short-form platforms.

---

## 1. The Problem

The current `srt.to_timeline` handler uses a hardcoded `SCENE_SIZE = 4` constant that groups every 4 consecutive SRT entries into one segment. This produces:

| Metric | Current | Problem |
|--------|---------|---------|
| Segment duration | 10–27 seconds | Far exceeds the 2–4 second ideal for short-form content |
| Sentence boundaries | 9/12 segments break mid-sentence | Captions appear disjointed from audio |
| Adaptive sizing | None | Same grouping for a 30s clip and a 5-minute video |
| Min/max enforcement | None | 2.4s segments and 27.6s segments coexist |

**Result:** Static b-roll shots linger 3–10× too long, causing viewer drop-off. Captions break mid-sentence, reducing readability.

---

## 2. Industry Standards (Research-Based)

### 2.1 Optimal Shot Duration

| Platform | Ideal ASL (Average Shot Length) | Max Static Shot | Sweet Spot |
|----------|--------------------------------|-----------------|------------|
| TikTok | 2–3 seconds | 3 seconds | 21–34s total |
| Instagram Reels | 2–4 seconds | 3 seconds | 7–15s (reach) / 25–45s (saves) |
| YouTube Shorts | 2–4 seconds | 4 seconds | 30–45s total |

### 2.2 Pacing Principles

1. **Every 2–4 seconds, something must change** — a new visual, a zoom, a text overlay, or a b-roll cut.
2. **Sentence-based editing** — cut at natural sentence endings (periods, question marks, exclamation points).
3. **Breath-based cuts** — remove inhalation pauses between sentences to tighten pacing.
4. **Retention reset at 25–30 seconds** — a pattern break (zoom, topic change, b-roll jump) to re-engage viewers before the natural drop-off point.
5. **Ruthless trimming** — remove any second that doesn't provide information or emotion.

---

## 3. Proposed Architecture: Sentence-Aware Adaptive Segmentation

### 3.1 Core Principle

**Segment at sentence boundaries, not at fixed word counts.**

The new system replaces `SCENE_SIZE` (fixed 4-word grouping) with a sentence-aware parser that:

1. Detects sentence endings from punctuation and pause patterns
2. Enforces min/max duration per segment
3. Splits long sentences into sub-segments
4. Merges short fragments into adjacent segments

### 3.2 Segment Duration Targets

| Parameter | Value | Rationale |
|-----------|-------|-----------|
| `target_duration_s` | 4.0 | Matches ideal ASL for short-form |
| `min_duration_s` | 2.0 | Below this, merge with adjacent segment |
| `max_duration_s` | 6.0 | Above this, split at longest internal pause |
| `ideal_word_range` | 8–15 words | ~4s at 2.5 words/second (natural speaking pace) |

### 3.3 Segmentation Algorithm

```
Input: SRT entries (word-level, with timestamps)
Output: Segments (with start, end, caption, semantic_role)

Step 1: Detect pause boundaries (PRIMARY signal)
  - Calculate gap between consecutive SRT entries: gap = entry[i].start - entry[i-1].end
  - Mark boundaries where gap > 300ms (natural breath/pause)
  - This is the PRIMARY segmentation signal — pauses indicate sentence endings

Step 2: Detect punctuation boundaries (SECONDARY signal)
  - Parse each SRT entry's text for sentence endings: '.', '?', '!', '…'
  - Mark these as secondary boundary signals
  - Note: Hinglish/whisper output often lacks clean punctuation, so this is supplementary

Step 3: Group entries into raw segments
  - Group consecutive entries from one boundary to the next
  - Each group becomes a "raw segment" with start/end times and concatenated caption

Step 4: Enforce duration constraints (simple, not recursive)
  For each raw segment:
    if duration < min_duration_s (2.0s):
      MERGE with next segment (concatenate captions)
    if duration > max_duration_s (6.0s):
      SPLIT at the longest internal pause (>200ms) or comma
      If no suitable split point exists, keep as-is (agent can handle via broll pacing)

Step 5: Assign semantic roles (AGENT-DRIVEN, not position-based)
  - Agent reads all segments and assigns roles based on CONTENT:
    - "hook": attention-grabbing opening (not necessarily first segment)
    - "body": main content segments
    - "cta": call-to-action (not necessarily last segment)
  - Default: first=hook, middle=body, last=cta (agent can override)

Step 6: Validate output
  - All segments within [min_duration_s, max_duration_s]
  - No segment breaks mid-sentence (validate against punctuation boundaries)
  - Total duration matches source audio (within 5%)
```

### 3.4 Example: Audit Audio (135s, 45 SRT entries)

**Current output (scene_size=4):**
```
seg_0:  0.0s–13.2s (13.2s) ← TOO LONG, mid-sentence break
seg_1: 13.2s–26.0s (12.8s) ← TOO LONG, mid-sentence break
...
seg_10: 120.0s–147.6s (27.6s) ← FAR TOO LONG
seg_11: 147.6s–150.0s ( 2.4s) ← too short
```

**Proposed output (sentence-aware):**
```
seg_0:   0.0s– 3.2s ( 3.2s) "Bhai sarkaar ki phati badhiya."     ← sentence end
seg_1:   3.2s– 6.1s ( 2.9s) "Sarkaar shuruaat isi patti se karti hai"  ← sentence end
seg_2:   6.1s– 9.8s ( 3.7s) "aur logon ki account ban kar rahe hain."  ← sentence end
seg_3:   9.8s–13.2s ( 3.4s) "Kuchh nahin badal sakta bhai."       ← sentence end
seg_4:  13.2s–16.5s ( 3.3s) "In hai bhai aavaaz kisi ki bachi nahin hai" ← sentence end
...
seg_35: 120.0s–124.2s (4.2s) ← body segment
seg_36: 124.2s–128.5s (4.3s) ← body segment
seg_37: 128.5s–132.0s (3.5s) ← CTA segment
```

**Result:** ~35 segments × ~4s each = 140s (matches 135s audio with overlap/crossfade)

### 3.5 B-Roll Keyword Generation

Each segment's caption is translated to a visual keyword by the agent:

```
Segment caption: "Bhai sarkaar ki phati badhiya."
Agent keyword:   "angry politician press conference"
```

**Keyword quality rules:**
- Use the first 3–5 meaningful words from the caption
- Translate Hinglish to English visual concepts
- Prefer concrete nouns over abstract concepts
- Map to Pexels search-friendly terms

### 3.6 B-Roll Clip Duration Matching

Each Pexels clip is typically 5–15 seconds. The system must:

1. **Trim clips** to match segment duration (e.g., 4s segment → trim 8s clip to 4s)
2. **Speed-ramp** if clip is shorter than segment (e.g., 3s clip at 0.75× speed for 4s segment)
3. **Loop** if clip is much shorter (e.g., 2s clip looped twice for 4s segment)

---

## 4. Implementation Plan

### Phase 1: Sentence-Aware Segmentation (Priority: P0)

**Files to modify:**
- `crates/openscript-mcp/src/tools.rs` — `handle_srt_to_timeline()`
- `crates/openscript-core/src/srt.rs` — Add `group_into_sentences()` helper

**Changes:**
1. Add `sentence_aware` mode to `srt.to_timeline` (default: true)
2. Keep `scene_size` as fallback for backward compatibility
3. Add `min_duration_s` and `max_duration_s` parameters
4. Implement sentence boundary detection from SRT text punctuation

### Phase 2: Adaptive Duration Enforcement (Priority: P1)

**Changes:**
1. Merge segments shorter than `min_duration_s` with neighbors
2. Split segments longer than `max_duration_s` at longest pause
3. Validate all segments fall within [min, max] range

### Phase 3: B-Roll Clip Duration Matching (Priority: P2)

**Changes:**
1. In `broll.fetch` auto-placement, trim/loop clips to match segment duration
2. Add `clip_duration_mode` parameter: "trim" (default), "loop", "speed_ramp"

---

## 5. Migration Path

### Backward Compatibility

The existing `scene_size` parameter remains functional:
- `scene_size=1` → one segment per SRT entry (legacy behavior)
- `scene_size=4` → current default (fixed grouping)
- `sentence_aware=true` → new default (sentence-boundary detection)

### Deprecation Plan

1. **Phase 1:** Add `sentence_aware` parameter, default `false`
2. **Phase 2:** Change default to `true`, add deprecation warning for `scene_size`
3. **Phase 3:** Remove `scene_size` constant, keep parameter for compat

---

## 6. Validation Criteria

A segmentation is considered "correct" when:

| Criterion | Threshold |
|-----------|-----------|
| Segment duration | All segments within [2.0s, 8.0s] |
| Sentence boundaries | 0 segments break mid-sentence |
| Total duration | Sum of segment durations within 5% of source audio |
| Word count per segment | 8–15 words (natural speaking pace) |
| B-roll coverage | Every segment has a matching b-roll clip |
| Retention reset | At least one visual change every 25–30s |

---

## 7. Reference: Professional Editing Benchmarks

| Metric | Amateur | Professional | OpenScript Target |
|--------|---------|--------------|-------------------|
| Average shot length | 8–15s | 2–4s | **3–4s** |
| Max static shot | 20s+ | 3–5s | **8s** |
| Sentence breaks honored | 30% | 95% | **100%** |
| B-roll per minute | 2–3 clips | 15–20 clips | **15+ clips** |
| Retention at 30s | 30% | 70% | **60%+** |
