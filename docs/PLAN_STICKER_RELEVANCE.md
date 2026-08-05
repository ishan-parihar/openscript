# PLAN — Sticker Relevance Fix (agentic intent → validated placement)

**Date:** 2026-08-05
**Status:** Plan (not yet implemented)
**Context:** Discovered during the E2E A2V test (`broll.auto` on a 43-segment Hinglish script). 12 stickers were placed, but their provenance concepts were raw caption words (`"Bhai"`, `"hisaab"`, `"apna"`, `"dekh"`, `"logon"`…) and all 12 were anchored at the same `top-right` position. GIPHY returned reaction GIFs that were unrelated to the spoken content. The user's verdict: stickers are irrelevant.

---

## 1. Root cause

`handle_sticker_auto_assign` (`crates/openscript-mcp/src/tools.rs:16828`) derives the GIPHY query as:

1. a global `sticker_query` override (rarely set), else
2. the first per-segment keyword from `sticker.keywords` (agentic draft), else
3. **raw caption words**: `seg.caption.split_whitespace().filter(|w| w.len() > 3).take(3).join(" ")` — e.g. `"hai logon ki account"`.

Then it calls `api.giphy.com/v1/stickers/search` with `limit=3` and **picks the first downloadable result with no relevance validation**. There is no gate comparing the sticker to the segment's actual meaning, no editorial filter (calm filler segments get stickers too), and no position diversity.

Compare with the b-roll pipeline, which works: `broll.keywords` (agentic draft) → `broll.validate_keywords` (agent picks the best REAL Pexels candidate against the caption) → fetch. Stickers have the draft half but **no validation half** and a weak query source.

---

## 2. Design — intent-first, validated placement

Mirror the proven b-roll agentic loop, but for stickers use an **emotional/intent vocabulary** (GIPHY sticker search is tuned for reactions, not topics).

### Stage A — `sticker.keywords` upgrade (intent + emphasis)

Per segment, the LLM outputs a JSON object instead of a bare keyword list:

```json
{
  "intent": "anger" | "surprise" | "hype" | "celebration" | "sarcasm" | "sad" | "question" | "emphasis" | "none",
  "emphatic": true|false,
  "sticker_keywords": ["angry eyes", "frustrated", "explosion head"]
}
```

Rules:
- `emphatic=false` (calm/filler segments — plain statements, connectors, "hai", "bhai" filler) → **no sticker**.
- `intent=none` + `emphatic=false` → skip before any API call (saves GIPHY quota + avoids spam).
- `sticker_keywords` are **English GIPHY-sticker-friendly** phrases (translated from Hinglish by the LLM), 2–3 per segment, batched like `broll.keywords` (`max_batch_size`).
- Keep the existing `max_stickers` cap (default 12).

### Stage B — NEW tool `sticker.validate_keywords` (relevance gate)

Mirror of `broll.validate_keywords`:

1. For each candidate segment, search GIPHY (`limit=5`, `rating=g`, `bundle=sticker_layering`).
2. Give the LLM the segment's caption + the top-5 results' `title`/`tags` + the segment `intent`.
3. The LLM returns `{approved: bool, best_title: string|null, reason}` — approved only if the sticker genuinely matches the intent and doesn't contradict the caption.
4. **No approved match → skip the segment** (record `skipped` reason). Better no sticker than an irrelevant one.

### Stage C — placement gates in `sticker.auto_assign`

- **Spacing:** never place a sticker in a segment adjacent to another sticker (min 1-segment gap) — avoids sticker spam.
- **Position diversity:** cycle positions (`top-right`, `bottom-right`, `center-left`, `bottom-left`) instead of forcing `top-right` on all; keep clear of the bottom caption safe zone (y ≥ 85% is captions).
- **Duration:** keep the 5s cap; fade in/out as today.
- **Registry:** keep the `asset_id = event_id` convention (already correct from Phase 138).

### Stage D — wire-up

- `sticker.auto` (one-call) pipeline becomes: `segment.analyze` → `sticker.keywords` (intent) → `sticker.validate_keywords` (GIPHY + gate) → download only approved → `Stickers` track.
- `broll.auto` Stage F keeps calling `sticker.auto` but drops the hardcoded `position: "top-right"` and lets the position cycle apply.
- `sticker.auto_assign` keeps its direct mode (explicit `sticker_query` + `position`) for manual override — the gating only applies when it would otherwise use caption-word fallbacks.

### Stage E — observability

- `skipped` reasons (`no_giphy_results`, `relevance_rejected`, `not_emphatic`, `adjacent_spacing`) returned in tool output so `verify.production` / the agent can explain sticker count vs segment count.
- `sticker_count` + `sticker_skipped_count` in the report.

---

## 3. Files touched (implementation phase)

| File | Change |
|------|--------|
| `crates/openscript-mcp/src/tools.rs` | Upgrade `handle_sticker_keywords` (intent JSON), add `handle_sticker_validate_keywords`, rewire `handle_sticker_auto` + `handle_sticker_auto_assign`, add 2 tool definitions + routes (count 96 → 98), update `sticker.auto`'s `position` default handling |
| `crates/openscript-mcp/src/server.rs` | Tool count + sticker family count + A2V trajectory text |
| `crates/openscript-mcp/tests/integration_test.rs` | New tool assertions + count |
| `scripts/smoke_test_mcp.sh` | Tool count 96 → 98 |
| `AGENT_GUIDE.md` | Sticker section: intent→validate→place trajectory |
| `crates/openscript-core/src/sticker.rs` | (if a pure gating helper is extracted for unit tests) |

## 4. Verification plan

1. `cargo build --workspace --exclude openscript-tauri` — zero warnings.
2. `cargo test --workspace --exclude openscript-tauri --lib --bins --tests` — baseline + new tests.
3. Rerun the E2E A2V test (`broll.auto` on `artifacts/drafts/audit_v3_render.hinglish-ggml.srt` + `audit_v3_render_audio.mp3`): expect fewer stickers but each with a relevant provenance concept (e.g. "anger" segments get "angry eyes"), no adjacent placements, varied positions.
4. Extract frames at sticker windows; OCR/probe the sticker region title vs the caption to confirm topical alignment (or human review of a contact sheet).

## 5. Non-goals

- No auto-tagging of stickers back to the script.
- No per-sticker click tracking.
- `sticker.render` (SVG puppet / lip-sync) is a separate feature and untouched.
