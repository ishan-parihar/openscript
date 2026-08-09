# Keyword Pipeline Audit — Fragmentation, Segment-Coupling Gaps, and the Unified `keywords` Module Plan

**Date:** 2026-08-09 · **Scope:** every keyword-drafting, keyword-fallback, and query-shaping path in `crates/openscript-mcp` (42 call sites across 7 files), across all three workflows (script→video, audio→video, video→video).

---

## 1. Executive summary

The keyword-suggesting infrastructure is **10 overlapping implementations spread across 7 files**, with **no single owner** for "given a segment, produce the right search keywords." The result:

1. **Keywords are weakly coupled to the segments they serve** — the most-used fallback picks the *first three words* of a caption, and anchor selection degrades to *position-based rotation* when content doesn't match a hardcoded topic bank.
2. **Same failure → different quality per entry point** — LLM-down produces 4 different quality levels depending on which workflow called it.
3. **Hardcoded bias** — the heuristic pipeline is ASCII-only (drops every non-Latin script), detects only 6 topic categories (everything else collapses to "Lifestyle" = coffee mug/desk anchors), and assumes Hinglish by default.
4. **A documented feature is not implemented** — `video_keywords` claims "auto-extracted from title if omitted" but no code does it.
5. **One "unification" is semantically wrong** — `broll.auto` feeds *visual b-roll keywords* into the *GIPHY sticker* search, skipping the reaction/intent drafting that stickers actually need.

Section 5 proposes a single `keywords` module that owns the whole lifecycle, with an implementation plan.

---

## 2. Current architecture map (the fragmentation)

| # | Implementation | Location | Role |
|---|----------------|----------|------|
| 1 | `handle_broll_keywords` | `tools/tools_broll.rs:760` | Batched LLM draft → `{keywords}` (visual) per segment. Used by `broll.auto`, `script.to_video`, routed as `broll.keywords` |
| 2 | `handle_sticker_keywords` | `tools/tools_sticker.rs:175` | Batched LLM draft → `{intent, emphatic, sticker_keywords}` (reaction/emotion) |
| 3 | `llm_draft_keywords` | `tools.rs:3698` | Single-caption LLM draft (repair/validate fallback path) |
| 4 | `extract_broll_concept` | `tools.rs:1803` | Naive fallback: **first 3 non-stopwords** of the caption |
| 5 | `signal_tokens_from_scene` | `stock_signal.rs:455` | Heuristic token extractor (scene-first + topic keywords) |
| 6 | `build_scene_stock_query` | `stock_signal.rs:528` | Heuristic query builder (topic anchors + orientation) |
| 7 | `translate_hinglish_visuals` | `stock_signal.rs` | Fixed ~90-entry Hinglish→English dictionary (LLM-down fallback) |
| 8 | `safe_search_query` + `UNSAFE_KEYWORD_MAP` + `enrich_query_for_theme` | `tools.rs:6254/6276/6292` | Content-safety rewrite (blood→calm nature, etc.) — **applied ONLY in `script.to_video`** |
| 9 | `llm_validate_candidates` | `llm.rs` | LLM relevance gate over real Pexels candidates (`broll.validate_keywords`) |
| 10 | `extract_keywords` | `tools.rs:6308` | **Dead code** (`#[allow(dead_code)]`) — legacy query builder |

Query-shaping rules (how keywords become a search string) are re-implemented **4 different ways**: `broll.fetch` (filter `len>=3`, join, `take(max_kw)`), `script.to_video` (take 3, join), `broll.validate_keywords` (search each keyword separately, take 2), `background.fetch` (take 4 from signal, no topic bias).

---

## 3. Gaps: keywords ↔ segments (the relevance failures you reported)

### G1. The main fallback is content-blind (first-3-words)
`extract_broll_concept` (tools.rs:1803) filters stopwords then takes the **first 3 remaining words**:

> `"I want to tell you about the stock market today"` → **`want tell stock`**
> `"So the first thing you need to understand about influence is"` → **`thing need understand`**

The first words of a sentence are almost always the weakest signal. This is the LLM-down fallback for `broll.keywords`, `llm_draft_keywords`, and `broll.suggest`, so **every outage degrades relevance to positional garbage**.

### G2. Anchor selection degrades to position-based rotation
`pick_visual_anchor` (stock_signal.rs:493): when the scene's signal doesn't match ≥2 keys in a topic bank, it falls back to `bank[scene_idx % bank.len()]` — **scene 4 of every video gets bank[4]**, regardless of content. For abstract topics (psychology, politics, influence — your Chase Hughes corpus) `detect_topic` returns `Lifestyle`, so every unmatched scene gets a *cycled* Lifestyle anchor (sunrise-window, coffee-mug, notebook, phone…) unrelated to what's being said.

### G3. Batch drafts lose the segment↔keyword link silently
`handle_broll_keywords` asks the LLM to echo segment ids back. When ids mismatch or the model renumbers, the code falls through a silent ladder (`id` → `seg_N` → `seg_NNN`) and, if nothing matches, **silently swaps in the heuristic fallback for that segment only**. There is no:
- count check ("N segments in, N keyword sets out"),
- per-segment validation that the returned keywords belong to that caption,
- retry of missing ids.

### G4. Draft prompt omits the segment's timing and position
`broll.keywords`/`sticker.keywords` send only `[id] N: "caption"`. No duration (a 2.5s window needs a very different clip than an 8s one), no scene index, no context that the segment is the video's hook vs its close. The LLM cannot tailor specificity or shot type to the segment.

### G5. `script.to_video` skips the relevance gate entirely
`broll.auto` runs draft → `broll.validate_keywords` (LLM checks real Pexels candidates against the caption) → fetch. `script.to_video` runs draft → **straight to acquisition** (tools_script.rs:1199-1217). The only gate is `llm_q.len() >= 2` — a hallucinated abstract keyword ("critical thinking") passes and goes to search.

### G6. `background.fetch` builds its query with **zero topic context**
tools_broll.rs:2033: `signal_tokens_from_scene(&query, &[])` — empty `video_keywords`, so `detect_topic` returns `Lifestyle` for everything and the anchor is picked from the Lifestyle bank only. This is the V2V path's query source.

---

## 4. Gaps: operational quality / universality (the bias)

### Q1. The tokenizer is ASCII-only → non-Latin scripts produce NOTHING
`tokenize` (stock_signal.rs:439): `text.split(|c: char| !c.is_ascii_alphabetic())`. **Every non-Latin script (Devanagari, Arabic, Cyrillic, CJK) tokenizes to zero tokens.** A Hindi-Devanagari, Arabic, or Russian video gets an empty signal → pure Lifestyle-anchor rotation → irrelevant footage. The pipeline is English/Latin-Hinglish-only *by construction*.

### Q2. Only 6 topic categories — everything else collapses to Lifestyle
`detect_topic` (stock_signal.rs:185) covers Space, Science, Nature, Marine, Tech, Lifestyle. Psychology, sociology, politics, finance, sports, fitness, food, fashion, gaming, music, education, travel… **all → Lifestyle** → coffee-mug/desk/notebook anchors. This is the single largest source of "b-roll not relevant" for abstract/pop-psych content.

### Q3. Schema lie: "video_keywords auto-extracted from title" is not implemented
`script.to_video` schema (tools_script.rs:20-21) documents auto-extraction. There is **zero code** doing it. Omitting `video_keywords` → empty array → `detect_topic` → Lifestyle → generic anchors. Silent quality trap.

### Q4. Hinglish is the hardcoded default language
`broll.keywords` and `sticker.keywords` default `language = "hinglish"` and the prompt asserts `Source language detected: hinglish` unless overridden. Non-Hinglish content is told the wrong source language. The 90-entry `HINGLISH_VISUAL_MAP` is also politics-heavy — non-political Hinglish (tech, sports, daily vlog) mostly passes through untranslated, and it only handles Latin-script Hinglish (not Devanagari).

### Q5. Numeric signal is discarded
`tokenize` drops digits (`is_ascii_alphabetic` only) and requires ≥3 alpha chars. "5 habits", "rule of 72", "10x", "9/11" lose their number words — sometimes the only concrete signal in a finance/history scene.

### Q6. Content-safety rewrite is workflow-dependent
`safe_search_query`/`UNSAFE_KEYWORD_MAP` (blood→calm nature, fear→courage, etc.) runs **only in `script.to_video`**. `background.fetch`, `broll.fetch`, and the heuristic paths ship raw keywords that can trigger tonally-wrong or unsafe stock. Same content → different safety in different workflows.

### Q7. Two duplicate stopword lists + duplicated Hinglish words
`NOISE_TOKENS` (stock_signal.rs) and the `stopwords` array inside `extract_broll_concept` (tools.rs:1804) overlap heavily and both hardcode the same Hinglish function words. They drift independently (they already have different contents). The Hinglish dict, the noise list, and the concept extractor each carry their own copy of "hai/ka/ki/ke…".

### Q8. Sticker "unification" is semantically wrong (the sticker-relevance bug)
`broll.auto` (tools_broll.rs:1690) passes its **validated visual b-roll keywords** ("crowd protest", "government building") as `sticker_keywords` to `sticker.auto`, which **skips the sticker intent draft entirely** when `shared_keywords` is present (tools_sticker.rs:656-668). GIPHY is then searched for *scene nouns*, not *reactions* — "crowd protest" returns generic crowd GIFs instead of "mind blown"/"facepalm". One keyword source is right; reusing the *wrong type* of keyword is not. The draft pass should emit **both** `visual` and `reactions` per segment.

---

## 5. The unified `keywords` module design

New file: **`crates/openscript-mcp/src/keywords.rs`** — the single owner of the keyword lifecycle. Every workflow (script/A2V/V2V, broll, sticker, repair, probe, asset) calls *one* entry point.

### 5.1 Core types

```rust
pub struct SegmentInput {
    pub segment_id: String,
    pub caption: String,
    pub language_hint: Option<String>,   // auto-detected when None
    pub duration_s: f64,                 // 0 when unknown
    pub scene_idx: usize,
    pub context: SceneContext,           // title, video_keywords, covered concepts
}

pub struct SceneKeywords {
    pub segment_id: String,
    pub visual: Vec<String>,             // stock-footage keywords (Pexels/Pixabay/YT)
    pub reactions: Vec<String>,          // GIPHY sticker keywords (reaction/meme)
    pub intent: Option<String>,
    pub emphatic: bool,
    pub source: KeywordSource,           // LLM | heuristic | hybrid
    pub backend: String,
    pub confidence: f64,                 // draft-quality gate (Q-level)
}
```

### 5.2 One orchestrator, two outputs

```rust
pub async fn draft_scene_keywords(inputs: &[SegmentInput]) -> Vec<SceneKeywords>
```

- **One batched LLM call** per ≤15 segments producing a single JSON object per segment:
  `{id, visual:[...], reactions:[...], intent, emphatic}` — kills the broll/sticker split **and** the wrong keyword reuse (G8), and gives the prompt duration+position+topic context (G4).
- **Strict id echo-back**: the LLM must return ids; missing ids are **re-drafted in a follow-up call** for exactly those segments (G3), never silently swapped.
- **Draft-quality gate** (G5): a `confidence` score (keyword count, visual-word ratio, length caps) — below floor → route through the heuristic, above floor → use as-is. `script.to_video` gets the same validation discipline without a full Pexels round-trip.

### 5.3 Universal heuristic fallback (replaces 4 different fallbacks)

```rust
pub fn extract_salient_keywords(caption: &str) -> Vec<String>
```

- **Unicode-aware tokenization** (Q1, Q5): split on any non-letter (keep Devanagari/Arabic/Cyrillic/CJK + digits), case-fold via Unicode, keep ≥2-char tokens.
- **Salience, not position** (G1): score tokens by length, capitalization (proper nouns), digit-content, and phrase frequency; take the top-N — never "first three words".
- **Language-neutral stopwords** via a single consolidated list (Q7), plus script-aware handling (function words in the detected script).
- One fallback for **all** entry points — the LLM-down path is now identical everywhere (G-fragmentation B2).

### 5.4 Data-driven topic registry (de-biasing)

Replace the 6-category `detect_topic` with a registry table (Q2) covering Psychology/Society, Politics, Finance, Sports, Fitness, Food, Fashion, Gaming, Music, Education, Travel, Space, Science, Nature, Marine, Tech — each with seeds + anchor bank. Table-driven so it grows without code edits. The heuristic remains the *last-resort* path; the LLM draft is primary.

### 5.5 Shared post-processors (one definition, applied everywhere)

- `sanitize_query(q, theme)` — consolidates `safe_search_query` + `enrich_query_for_theme` (Q6); now applied by every workflow.
- `keywords_to_query(keywords, max_terms, orientation, theme)` — the single query shaper (B5).
- `auto_detect_language(caption)` + `source_language` in the draft prompt (Q4) — removes the Hinglish default.
- `auto_extract_video_keywords(title)` — implements the documented-but-missing title extraction (Q3).

### 5.6 Rewiring (what calls what after the refactor)

| Caller | Before | After |
|--------|--------|-------|
| `handle_broll_keywords` | own LLM draft + own fallback | `keywords::draft_scene_keywords(...).visual` |
| `handle_sticker_keywords` | own LLM draft + own fallback | `keywords::draft_scene_keywords(...).reactions/intent/emphatic` |
| `broll.auto` + `sticker.auto` | broll draft → *reuse* for stickers | **one** `draft_scene_keywords` call; stickers consume `reactions` |
| `script.to_video` | inline draft + heuristic | `draft_scene_keywords` → `visual` → `sanitize_query` → `keywords_to_query` |
| `background.fetch` | `signal_tokens_from_scene(&query, &[])` (no topic) | `draft_scene_keywords` (or accepted `keywords` param) → shared pipeline |
| `llm_draft_keywords` (repair/validate) | own single-caption draft | `keywords::draft_scene_keywords` |
| `extract_broll_concept`, `extract_keywords`, duplicated stopword lists | scattered | **deleted** (YAGNI); consolidated into `keywords.rs` |

---

## 6. Implementation plan (phased)

**Phase A — foundation (keywords.rs + tests).** Unicode-aware tokenizer, consolidated stopwords, `extract_salient_keywords`, topic registry, `sanitize_query`, `keywords_to_query`, `auto_detect_language`, `auto_extract_video_keywords`, `draft_scene_keywords` (LLM + fallback + id-echo + quality gate). Unit tests mirror the existing `stock_signal` test style (pure, deterministic). *No callers changed yet — old paths keep working.*

**Phase B — rewire the drafters.** Point `handle_broll_keywords`, `handle_sticker_keywords`, `llm_draft_keywords`, `script.to_video`, `background.fetch` at the unified module. Delete `extract_broll_concept`, `extract_keywords`, and the old fallback bodies. `broll.auto`/`sticker.auto` switch to one draft call with `reactions`.

**Phase C — parity + gate.** Run the full validation gate (build zero-warning, 398+ tests, lint, smoke test), then **re-render the 5 Chase Hughes samples** through `script.to_video` and audit per-scene queries vs the v2 baseline (the timeline `tags[0]` audit evidence).

**Phase D — docs + push.** Update `AGENT_GUIDE.md` (tool descriptions), this doc, commit & push per iteration.

**Estimated size:** ~900-1200 LoC new module, ~400 LoC deleted/replaced, 7 files rewired. Build/test impact contained to `openscript-mcp`.

---

## 7. Risk notes

- **Behavior change on LLM-down paths** is intended (better fallbacks) but must be validated by the sample re-render, not just unit tests.
- **`sticker.auto` keyword semantics change** (visual → reactions) — this is the *fix* for sticker relevance, but expect a different sticker mix in the next render; the GIPHY relevance gate (`sticker.validate_keywords`) still filters.
- The topic registry is a heuristic; the LLM draft is primary, so registry quality only matters when the LLM cascade is down.
