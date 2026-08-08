# OpenScript System Architecture v2 — Asset-Development Pipeline & Fallback Discipline

**Status:** PLAN (approved decisions locked in) — not yet implemented
**Date:** 2026-08-08
**Owner:** OpenScript R&D
**Companion docs:** `docs/SEGMENTATION_ARCHITECTURE.md`, `docs/MEDIA_SEARCH_AUDIT.md`, `docs/PIPELINE_FIX_PLAN.md`

---

## 0. Progress tracker (active / pending / completed)

| # | Item | Status |
|---|------|--------|
| 1 | Root-cause audit: why procedural cuts shipped | ✅ COMPLETED (Part A) |
| 2 | Decision lock: procedural policy, library storage, curation surface, YouTube tier | ✅ COMPLETED (user decisions) |
| 3 | Phase 1 — Fallback discipline: unified acquisition chain + procedural loud-warning | ⏳ PENDING |
| 4 | Phase 2 — Asset-development pipeline backend (`asset.*` MCP/CLI surface) | ⏳ PENDING |
| 5 | Phase 3 — Generation integration: library tier + YouTube opt-in | ⏳ PENDING |
| 6 | Phase 4 — Unified `openscript` CLI surface + docs | ⏳ PENDING |
| 7 | Phase 5 — Tauri Asset-Library curation UI (deferred, after backend) | ⏳ PENDING (later) |

---

## 1. Executive summary

Three independent problems were reported, and investigation shows they share one root: **scene background acquisition has drifted into two divergent implementations, and the "last resort" (procedural) is reached before every legitimate fallback is actually exhausted.**

1. **Procedural cuts shipped** because `script.to_video`'s *inline* fallback chain omits Pixabay entirely (only standalone `background.fetch` has it), and because scenes whose YouTube search returns garbage get ranked down to 0 candidates → silent procedural.
2. **YouTube relevance underperforms** because YouTube is a social platform, not a curated stock library — its metadata is clickbait, and the current gate (title-lexical × duration) has no source prior and no non-fail-open vision check.
3. **There is no user asset library.** The asset surface today is read-only provider fetching (`background.fetch`, `broll.*`, `stock.fetch`). Nothing lets a user ingest, curate, rate, or index their own footage — so the pipeline cannot learn from what the user actually likes.

The plan fixes all three with a **two-pipeline architecture**:

- **Asset-Development Pipeline** (NEW): `asset.*` tool family + index + curation workflow. Users ingest local clips, probe candidate pools across Pexels/Pixabay/YouTube, rate relevance & quality, and promote winners into a gitignored in-repo library.
- **Video-Generation Pipeline** (hardened): one unified acquisition function with a strict provider hierarchy **user_library → Pexels → Pixabay → YouTube (opt-in) → fallback_pool → procedural**, loud-warning procedural policy, and per-provider exhaustion diagnostics.

The two pipelines are decoupled: generation *reads* the library index; asset-development *writes* it. Same pattern as `music_index.json` / `sfx_index.json` today, extended to footage.

---

## 2. Part A — Root cause: why procedural cuts were used

### A.1 The two chains have drifted (primary bug)

`script.to_video` does **not** call `background.fetch`. It has its own inline per-scene loop (`crates/openscript-mcp/src/tools.rs:14089–14560`) whose comment literally states the intended priority:

```
// Priority: Pexels (if key) → YouTube via yt-dlp (no key) → procedural (last resort).
```

Compare `background.fetch` (`tools.rs:11760–12170`), which implements:

```
Pexels → Pixabay (1.5, signal-ranked) → YouTube (signal-ranked) → fallback_pool → procedural
```

**`script.to_video` never tries Pixabay.** When `PEXELS_API_KEY` is unset or returns 0 relevant results for a scene, and the YouTube path fails to produce a passing candidate, the scene falls straight to procedural — **even when `PIXABAY_API_KEY` is set and Pixabay would have returned professional film footage.** This directly violates the user rule "procedural must never be used until all fallbacks are useless." A valid fallback was *never even attempted*.

### A.2 Ranked-0 candidates → silent procedural (query-quality gap)

The vision audit (Round 7, `vision_audit_v157`) showed scenes 4–5 reporting `ranked 0 candidates`: the YouTube search returned 8 raw results that were **garbage** ("Lymphatic Drainage In Your Bed", a gaming video, "Ice Bath & Sauna Effects"), the lexical gate correctly rejected them, and the scene fell to procedural. The gate works; the *queries* and *search surface* are the failure. See `stock_signal.rs:892–928` — `rank_yt_candidates` = `lexical_relevance(title, signal) × duration_preference(duration)` on ~8–12 flat-playlist entries, with no channel filter, no duration cap for lectures, and a vision check that only fires on 1–2 thumbnails.

### A.3 Vision gate is fail-open on quota exhaustion

The L2/L3 vision gates (OpenRouter multimodal / opencode-zen) silently degrade: on OpenRouter 429 (free-tier daily cap) with no `opencode` key, `vision_score` falls back to `lexical_score`. Round 7 ran entirely fail-open. The gate stops *filtering* exactly when relevance assurance matters most.

### A.4 Procedural is too easy to reach

`mcp/assets/backgrounds/` ships **18 pre-generated procedural clips** (`procedural_*.mp4`, `backgrounds_index.json`). The last-resort branch (`tools.rs:14501–14560`) rotates them and pushes a `render_warnings` entry, but:
- the warning is buried among other warnings (not unmissable),
- the production hard-fail only triggers at **≥50% procedural** scenes — 2/6 procedural scenes rendered as `status: rendered` without a hard fail,
- nothing blocks procedural when < 50% even though the user's rule is "never until all fallbacks are useless."

### A.5 No source prior anywhere

`StockCandidate` (`stock_pool.rs:22–39`) carries `provider` but ranking applies **no provider weight** — a clickbait YouTube title and a curated Pexels title compete on the same lexical scale. The dedup tie-break comment (`stock_pool.rs:13`: "Pexels → Pixabay → YouTube") is insertion-order only, not scoring.

---

## 3. Part B — Fallback discipline (Phase 1, P0)

### B.1 One unified acquisition function (kill the drift)

Create `crates/openscript-mcp/src/scene_media.rs` with a single async orchestrator:

```rust
pub struct SceneMediaRequest {
    pub query: String,
    pub signal_tokens: Vec<String>,   // from stock_signal::signal_tokens_from_scene
    pub scene_text: String,
    pub duration_s: f64,
    pub min_duration_s: f64,          // SEGMENTATION_ARCHITECTURE floor
    pub max_duration_s: f64,          // 0 = no cap
    pub aspect: String,               // "9:16" | "16:9" | "1:1"
    pub cache_dir: String,
    pub enable_youtube: bool,         // NEW: opt-in per user decision
    pub used_video_ids: &mut HashSet<String>,   // cross-engine dedup
    pub used_content_hashes: &mut HashSet<String>, // byte-fingerprint dedup
}

pub struct SceneMediaOutcome {
    pub clip_path: String,
    pub source: String,          // user_library | pexels | pixabay | youtube | fallback_pool | procedural
    pub provider_id: String,
    pub lexical_score: f64,
    pub vision_score: Option<f64>,
    pub vision_reason: Option<String>,
    pub exhausted: Vec<String>,  // NEW: every provider tried + why it failed
    pub needs_looping: bool,
}
```

`script.to_video`'s inline loop **is replaced by a call to this function per scene**. `background.fetch` becomes a thin wrapper over it. One chain, one behavior, one set of diagnostics — this is the single highest-value refactor in the plan.

### B.2 Strict provider hierarchy with per-provider acceptance thresholds

```
Tier 1  user_library   lexical ≥ 0.05   (curated assets — trust them)
Tier 2  pexels         lexical ≥ 0.10   (requires PEXELS_API_KEY)
Tier 3  pixabay        lexical ≥ 0.10   (requires PIXABAY_API_KEY; video_type=film only)
Tier 4  youtube        lexical ≥ 0.25 AND vision PASS   (ONLY if enable_youtube)
Tier 5  fallback_pool  any existing non-procedural path (caller-supplied)
Tier 6  procedural     NEVER unless tiers 1–5 all exhausted  → loud warning + score penalty
```

- Thresholds are config constants, not hardcoded magic numbers (AGENTS.md style).
- Tier 4 requires a **passing vision frame-gate and non-fail-open**: if vision is unavailable (no key, 429), YouTube candidates are **rejected, not assumed good** (fail-closed). YouTube is opt-in anyway, so this costs nothing in the default path.
- Provider-exhaustion tracking: every attempt appends to `exhausted` (e.g. `pexels: 0 results`, `pixabay: key unset — SKIPPED (was never tried)`, `youtube: opted-out`, `youtube: ranked 0/8 candidates (lexical < 0.25)`). This makes the "why procedural" question answerable per scene in one log line.

### B.3 Procedural policy — loud warning + production score penalty (user decision)

Keep rendering (no hard-fail), but make it unmissable:

1. Per-scene warning prefixed `⚠️ PRODUCTION_FAIL` (not buried in generic warnings).
2. The tool-level `status` becomes `"warning"` (not `"rendered"`/`"success"`) **whenever ≥1 procedural scene ships and `OPENSCRIPT_ALLOW_PROCEDURAL=1` is not set** — agents must acknowledge it. (Current behavior only downgrades at ≥50%.)
3. `verify.production` already scores `stock_visuals`; a procedural scene zeroes that dimension per scene (keep) and the aggregate production grade is capped at `B` (not `A`) when any procedural scene exists.
4. `exhausted` is echoed back in the tool response so the caller sees exactly which fallbacks were consumed before procedural.

### B.4 Diagnostics (cheap, high value)

One `tracing::info!` line per scene, from `SceneMediaOutcome.exhausted`:

```
[scene_media] scene=4 q="calm head pulls..." exhausted=[pexels:0_results, pixabay:key_unset_SKIPPED, youtube:ranked_0_of_8(lex<0.25), yt_fanout:ranked_0_of_8] → procedural
```

This converts the "why did this happen" post-mortem into a runtime self-diagnosis.

---

## 4. Part C — YouTube tier: opt-in for generation, engine for asset-development (user decision)

### C.1 Generation: YouTube off by default

- New config knob: script field `background.enable_youtube: false` (default) + env `OPENSCRIPT_YT_FOR_GENERATION=1`.
- When off, tiers 1–3 + fallback_pool run; YouTube is skipped entirely (logged as `youtube: opted-out` in `exhausted`).
- When on, Tier 4 rules apply (stricter lexical + non-fail-open vision).

### C.2 Asset-development: YouTube always available as an acquisition engine

`asset.probe` (Part D) uses Pexels + Pixabay + YouTube regardless of the generation flag. YouTube's job there is **suggesting candidates for the user's library** — a human/agent curates before anything enters the library, so raw YouTube noise is filtered by curation rather than by fragile title-matching. This matches the user decision: "for the asset-development workflows it can be used for suggesting and developing the local library pool."

### C.3 Ranking improvements (apply when YouTube is enabled)

- **Source prior:** YouTube candidates multiply `lexical` by `0.7` before the threshold (configurable `yt_prior`). Pexels/Pixabay get `1.0`. This encodes "Pexels/Pixabay are much better" as an explicit weight rather than a vibe.
- **Duration filter:** drop candidates < 20s (too short to trim) and > 600s (lectures/long-format) before ranking; keep the existing lecture penalty.
- **Channel filter:** denylist channel names matching `lecture|podcast|course|tutorial|full movie|gameplay|music` (extend `is_broll_title_denylisted`).
- **Vision non-fail-open:** thumbnail frame gate must return PASS to select; on vision unavailability → reject (fail-closed), since YouTube is the riskiest tier.

---

## 5. Part D — Asset-Development Pipeline (Phase 2, backend-first)

> User decision: **backend first** (MCP + CLI surface). Tauri curation UI is Phase 5, deferred.
> User decision: library lives **in-repo at `mcp/assets/user_library/`, gitignored** (survives portability; the index JSON is the committed-schema, the media is not).

### D.1 Data model — `mcp/assets/user_library_index.json`

Mirrors `music_index.json` conventions (version + assets array + search_aliases) extended for footage:

```json
{
  "version": 2,
  "created_at": "2026-08-08T00:00:00Z",
  "root": "mcp/assets/user_library",
  "total_assets": 0,
  "assets": [
    {
      "id": "ul_0001",
      "path": "mcp/assets/user_library/morning_desk_01.mp4",
      "content_hash": "sha256:9f86d0…",
      "source": "user_upload" | "pexels" | "pixabay" | "youtube",
      "provider_id": "pexels_4521" | "yt_AbC123" | null,
      "duration_s": 12.4,
      "width": 1080,
      "height": 1920,
      "aspect": "9:16",
      "title": "Morning desk routine — coffee, notebook, warm light",
      "keywords": ["morning", "desk", "coffee", "routine", "notebook"],
      "mood": "calm",
      "energy": "low",
      "motion_intensity": "slow",
      "quality_rating": 4.5,
      "relevance": { "morning": 0.9, "desk": 0.85, "coffee": 0.8 },
      "tags": ["vertical", "clean", "no_people", "warm_light"],
      "curation_status": "candidate" | "approved" | "rejected",
      "usage_count": 3,
      "last_used_at": null,
      "rated_at": "2026-08-08T01:00:00Z"
    }
  ],
  "search_aliases": { "morning": ["sunrise", "early", "desk"], "calm": ["relaxed", "zen"] }
}
```

### D.2 Tool family (`asset.*` — 6 tools, all routing through `route_tool`)

| Tool | Purpose | Key outputs |
|------|---------|-------------|
| `asset.library.status` | Schema version, root, counts by source/status/quality | `total_assets`, `by_source`, `by_status` |
| `asset.ingest` | Scan a dir (default `mcp/assets/user_library`) + optional explicit file list; ffprobe metadata, content-hash fingerprint, auto-tags from filename; writes index. Idempotent (hash-dedup). | `indexed`, `skipped_duplicates`, `errors` |
| `asset.probe` | **Curation pool:** query Pexels+Pixabay+YouTube for N candidates matching keywords; return thumbnails + metadata **without downloading** full video (reuses `stock_pool` normalization + `broll.probe` machinery). The "pool of video samples" for classification. | `candidates[]` (thumbnail, duration, provider, page_url, direct_url, lexical) |
| `asset.rate` | Classify a probe candidate OR local file: relevance 0–1 per keyword, quality 0–5, mood/energy/motion, tags, `curation_status` (approved/rejected). Persists to index. | `asset_id`, `quality_rating`, `status` |
| `asset.import` | Download a probed external candidate (or copy a local file) into `user_library/`, fingerprint, index as `candidate`. | `asset_id`, `path` |
| `asset.search` | **Consumption side:** rank library assets by signal tokens × quality × status. Used by the generation pipeline (Tier 1) and by the Tauri gallery later. | `assets[]` ranked, `match_score` per asset |

Optional add-on: `asset.report` — counts, duplicates, unused assets, avg quality by source (cheap stats for the user).

### D.3 Curation workflow (the loop the user described)

```
1. ingest   — user drops footage into mcp/assets/user_library → asset.ingest
              (or pulls from providers)
2. probe    — "find clips for: morning desk routine" → asset.probe
              → 12–30 candidates with thumbnails from Pexels + Pixabay + YouTube
3. rate     — classify each: relevance to the keyword, quality 0–5, mood tags
              → asset.rate (approved / rejected / candidate)
4. import   — approved external candidates → asset.import (downloaded into library)
5. (loop)   — asset.ingest rescans → index grows → pipeline gets smarter
```

This is **strictly separate from the generation pipeline**: `asset.*` never renders, never fetches scene backgrounds. Generation only *consumes* the index via `asset.search` (Tier 1 in Part B.2).

### D.4 Local indexing (reuse, don't reinvent)

- Content-hash fingerprinting: reuse the existing `file_content_fingerprint` + `used_content_hashes` byte-hash already used for scene dedup (`tools.rs`).
- Probe: reuse `openscript_ffmpeg::probe::probe` (duration, resolution) already used in `background.fetch`.
- Scan/JSON-index pattern: reuse `library_indexer.rs` / `music.rs` structure (the schema conventions above come straight from `music_index.json`).
- Auto-tagging: filename stem → keywords (e.g. `morning_desk_01.mp4` → `morning`, `desk`); optional LLM keyword expansion via the existing `broll.keywords` LLM path (Phase 4 nicety).

### D.5 Generation integration (Phase 3) — the library becomes Tier 1

In `scene_media.rs`, before any provider call:

```rust
if let Ok(mut hits) = asset::search(&signal_tokens, QualityFloor::Approved_3_0).await {
    // pick the best-approved, longest-unused match; mark usage_count++
    // → scene_media uses it; source = "user_library"
}
```

- `asset.search` rank = `relevance[token] mean × (quality_rating / 5) × freshness` (least-recently-used wins ties).
- Only `curation_status == "approved"` and `quality_rating >= 3.0` assets are eligible for generation. A user's 2-star clips never reach a render.
- **This is the "weightage for better providers" the user asked for**: user's own approved footage beats Pexels, which beats Pixabay, which beats YouTube — all explicit and configurable.

---

## 6. Part E — Unified architecture reflection: `mcp/` vs a unified `mcp/cli`

### E.1 What exists today (the split the user flagged)

```
crates/openscript-mcp/     Rust: MCP server + 98 tool handlers in ONE giant tools.rs (~15k lines)
crates/openscript-cli/     Rust: clap CLI — thin wrapper over route_tool()
mcp/scripts/               Python ML sidecars (kokoro, audio8, whisper_align, apex, parakeet)
mcp/assets/                media + generated JSON indices (backgrounds, stickers, music, sfx, voices)
mcp/styles/ mcp/fonts/     caption CSS + fonts
```

The CLI is already the right *shape* (thin wrapper over `route_tool()`, per AGENTS.md), but it is not a first-class surface: no subcommand tree, no human-friendly flags, no parity docs. And the `mcp/` directory name now conflates "MCP transport" with "media runtime assets + sidecars" — a naming/mental-model smell.

### E.2 Target: one binary, two front-ends, one core

```
                   ┌───────────────────────────────┐
                   │  route_tool(name, args)       │  ← THE single tool surface
                   │  crates/openscript-mcp        │
                   └───────┬───────────────┬───────┘
                           │               │
              ┌────────────▼────┐   ┌──────▼────────────────┐
              │ openscript CLI  │   │ MCP server (stdio)     │
              │ (clap, humans+  │   │ JSON-RPC ↔ route_tool  │
              │  agents)        │   │ (transport adapter)    │
              └─────────────────┘   └────────────────────────┘
```

1. **One binary** `openscript` with two front-ends:
   - `openscript <tool.name> '{"json":...}'` — raw tool invocation (agent parity with MCP).
   - `openscript asset ingest --dir ...`, `openscript broll fetch --query ...`, `openscript script to-video --script path.json` — human-friendly subcommands that expand to the same `route_tool` calls.
   - `openscript mcp-serve` — the stdio MCP transport (today's `mcp-server` binary becomes a subcommand).
2. **Rename mentally (and in docs):** `mcp/` → "runtime media + sidecars" directory. No code moves required in Phase 1; documentation + AGENTS.md/AGENT_GUIDE.md §Repository Layout updated so the mental model is `crates/` = Rust core, `mcp/` = media assets + Python sidecars, `openscript` = one binary.
3. **Refactor `tools.rs`** (98 handlers, ~15k lines) into per-family modules in Phase 4:
   ```
   crates/openscript-mcp/src/tools/
     mod.rs            (route_tool dispatch + tool_definitions assembly)
     script_tools.rs   (script.*, background.*)
     asset_tools.rs    (asset.* — NEW)
     broll_tools.rs    (broll.*, segment.*, stock.*)
     sticker_tools.rs  (sticker.*)
     timeline_tools.rs (timeline.*, music.*, sfx.*)
     voice_tools.rs    (voice.*, tts.*)
     hf_tools.rs       (hf.*, composition.*)
   ```
   Pure mechanical move first (verification: identical test count), then per-family polish. This directly serves "remove deadweight & redundancies" and makes the new `asset.*` family land in a home of its own.
4. **CLI parity test:** a script that walks `tools/list` and asserts every tool is reachable via `openscript <name>` — the CLI and MCP can never drift again.

### E.3 What does NOT change

- The crate dependency graph (AGENTS.md §1): `openscript-mcp` remains the integrator; `openscript-core` stays a leaf.
- Python sidecars stay Python (ML-only), invoked exclusively via Rust subprocess.
- The golden trajectory `script.parse → script.to_video` is untouched.

---

## 7. Implementation phases (each ends with the full R&D gate: build → test → lint → commit → push)

### Phase 1 (P0) — Fallback discipline
1. `scene_media.rs`: unified `fetch_scene_background` with the 6-tier hierarchy + `exhausted` diagnostics.
2. Rewire `script.to_video` multi-broll loop → per-scene `fetch_scene_background`. **Pixabay enters the generation chain for the first time.**
3. `background.fetch` becomes a thin wrapper.
4. Procedural policy: `⚠️ PRODUCTION_FAIL` per-scene warning, `status: "warning"` when procedural ships without `OPENSCRIPT_ALLOW_PROCEDURAL=1`, verify.production grade capped at B.
5. Unit tests: tier ordering (mocked clients), exhausted-array shape, procedural-warning string.
6. Gate: build clean, 248+ tests, lint, commit `Phase 1: Unified scene-media acquisition — Pixabay in chain, procedural loud-warning`.

### Phase 2 (P1) — Asset-development backend
1. `mcp/assets/user_library/` (gitignored) + `user_library_index.json` schema + migration/versioning.
2. `asset.library.status`, `asset.ingest`, `asset.probe`, `asset.rate`, `asset.import`, `asset.search` — definitions, routes, handlers in `tools/asset_tools.rs` (create the module).
3. Probe reuses `stock_pool` (normalized candidates + thumbnails) for Pexels/Pixabay and the hardened `youtube_search_candidates` for YouTube.
4. Tool-count updates: `server.rs`, `AGENT_GUIDE.md`, `integration_test.rs`, `smoke_test_mcp.sh` (per AGENTS.md §4 checklist).
5. Gate: build, tests (+asset.* integration tests), lint, commit `Phase 2: Asset-development pipeline — asset.* tool family + user library index`.

### Phase 3 (P1) — Generation integration + YouTube opt-in
1. `asset.search` becomes Tier 1 in `scene_media.rs` (approved, quality ≥ 3.0, LRU freshness).
2. `background.enable_youtube` default `false`; `OPENSCRIPT_YT_FOR_GENERATION=1` override.
3. YouTube ranking upgrades (source prior ×0.7, duration/channel filters, non-fail-open vision).
4. E2E: render a script with a seeded user library; assert library scenes beat providers; assert procedural requires exhaustion.
5. Gate: full suite + smoke test + one rendered sample, commit `Phase 3: Library tier in generation + YouTube opt-in with hard gates`.

### Phase 4 (P2) — Unified CLI + tools.rs decomposition
1. `tools.rs` split into `tools/*` modules (mechanical, test-count-identical).
2. `openscript asset ingest|probe|rate|import|search`, `openscript mcp-serve`, `openscript <tool.name>` subcommands.
3. CLI↔MCP parity script in `scripts/`.
4. AGENTS.md/AGENT_GUIDE.md §Repository Layout + tool taxonomy updates; README workflow docs.
5. Gate: build, tests, tsc (frontend untouched but verify), lint, commit `Phase 4: Unified openscript CLI surface + tools module decomposition`.

### Phase 5 (P3, later) — Tauri curation UI
1. Asset-Library screen in the existing `AssetBrowser` area: thumbnail grid from `asset.probe`, inline rate/quality/tag controls calling `asset.rate`/`asset.import`, "My Library" tab backed by `asset.search`.
2. Wire into `store/assets.ts` + `lib/tauri.ts` wrappers + `commands/assets.rs` (thin pass-throughs per AGENTS.md §5).
3. Gate: tsc, Tauri build (GDK permitting), commit.

---

## 8. Risks & mitigations

| Risk | Mitigation |
|------|-----------|
| Pixabay added to generation chain changes render behavior | `exhausted` diagnostics + `background_sources` in every response; visible per-scene provenance in `render_manifest.json` |
| `status: "warning"` on any procedural scene breaks downstream agents | Document in tool descriptions ("status warning if any scene fell to procedural"); agents surface it as a to-do |
| Asset library grows stale / duplicates | Content-hash dedup at ingest; `asset.report` duplicates view; LRU freshness in `asset.search` |
| YouTube opt-in off by default changes existing agent flows | Description strings updated; `broll.probe`/`asset.probe` still expose YouTube for curation so nothing is lost |
| `tools.rs` split introduces regressions | Mechanical move gated on identical test count + smoke test; no behavior change in Phase 4 |

---

## 9. Open questions (non-blocking)

1. Quality floor for generation eligibility — `quality_rating >= 3.0` reasonable default? (Config `ASSET_QUALITY_FLOOR`.)
2. Should `asset.probe` LLM-expand keywords (reuse `broll.keywords` LLM path) in Phase 2 or Phase 4? (Default: Phase 4, filename auto-tags first.)
3. Max library size guard / eviction policy — needed only if the library grows beyond a few thousand clips (defer).
