# Segmentation + B-roll Coverage Upgrade Plan

> **Status:** Audit complete — plan ready for implementation approval
> **Date:** 2026-08-04
> **Related docs:** [`docs/SEGMENTATION_ARCHITECTURE.md`](./SEGMENTATION_ARCHITECTURE.md)
> **Related fixes:** Phase 132 (PTS alignment + loop + probe) — already landed

---

## 1. Audit Summary (What I Found)

Three issues, confirmed against the actual code:

### 1.1 Clips loop to fill the window (the "videos are looping" bug)

**Location:** `crates/openscript-ffmpeg/src/filter_graph.rs` — `build_post_trim()`, b-roll chain (~line 846–895).

**Current behavior:**

| Source duration known? | Logic | Result |
|---|---|---|
| Known, clip ≥ segment | `loop_count = 1` | ✅ plays once |
| Known, clip < segment | `loop_count = ceil(seg/clip)+1` (capped 8) | 🔁 **loops to fill the gap** |
| Unknown (`None`) | `loop_count = 3` fallback | 🔁 **every clip plays 3×** |

The user wants: **clips play exactly ONCE.** If the source clip is shorter than the
segment window, the renderer must NOT loop to fill — it should let the clip end and
leave a gap, which the validator then flags so a second keyword-generation pass can
fetch a longer clip.

**Audited fixture data** (audit_v3, 44 segments / 12 clips): all 12 cached clips are
7.0–68.8s long, all 44 segments are 2.4–4.8s. So with the Phase 132 probe fix the
known-duration path emits `loop=1` **for this fixture**. The looping the user is seeing
comes from the **unknown-duration fallback (`loop=3`)** when the probe map misses a path
(e.g. asset re-registered under a different key, `broll.fetch` writing paths that don't
match probe keys), and from real A2V runs where downloaded clips are shorter than their
segments.

### 1.2 The validator has no "gap" error — only a static-frame heuristic

**Location:** `crates/openscript-mcp/src/tools.rs` — `probe_broll_motion` (6465),
`probe_broll_motion_per_clip` (6562), `handle_verify_production` (6647);
`crates/openscript-core/src/production_quality.rs` — `score_broll_motion` (755).

**Current behavior:**
- `verify.production` detects **static frames** post-hoc (frame-hash uniqueness per clip,
  `longest_static_run_s`). A clip that ends early → overlay holds last frame → static run →
  `score_broll_motion` emits `"STATIC HARD: longest static run Ns"`.
- It does **not** know *why* the static run happened. It cannot say
  "segment seg_042 needs 3.5s but the clip provides 2.1s — re-run keyword generation
  for this segment."

**What the user wants:** the validator must produce an **actionable error** for the gap —
segment id, required duration, available duration, and a directive to re-run the
keyword-generation → `broll.fetch` pass for exactly those segments. That error becomes
the loop-closure signal for the agent (audit protocol's "pass onto another pass of
keyword generation").

### 1.3 SEGMENTATION_ARCHITECTURE.md was only partially implemented

**Locations:** `crates/openscript-mcp/src/tools.rs` — `handle_srt_to_timeline` (2657),
`handle_segment_analyze` (5525), `const SCENE_SIZE: usize = 4` (24);
`crates/openscript-core/src/srt/mod.rs` — `group_entries_with_words_max_duration` (113).

**Audit vs. the doc:**

| Doc requirement | Status |
|---|---|
| Sentence-aware segmentation (pause >300ms) | ✅ `srt.to_timeline` has it (opt-in via `max_duration_s`) |
| Min duration merge (<2.0s merge with next) | ⚠️ only in `srt.to_timeline` (opt-in) |
| Max duration split (>6.0s split at pause) | ⚠️ partial — groups by `max_duration_s` but doesn't split a single long entry |
| **`segment.analyze` (the A2V trajectory's main segmentation tool)** | ❌ **still hardcoded `SCENE_SIZE = 4` chunking — NO min/max enforcement, NO pause detection** |
| B-roll clip duration matching (trim/loop/speed_ramp) | ❌ renderer only trims; loop was the "fill" mechanism |
| Min/max as **default** behavior | ❌ `scene_size=1` (one entry per segment) is the default; `max_duration_s` is opt-in |

The doc says the default should be sentence-aware with 2–6s enforcement. Today the only
tool that enforces min/max is `srt.to_timeline`, and only when the agent passes
`max_duration_s`. `segment.analyze` — which is the exact tool the A2V trajectory
(`transcribe → segment.analyze → broll.keywords → broll.fetch`) uses — still produces
unbounded 4-entry chunks.

---

## 2. Implementation Plan

### Phase A — Renderer: always play clips once (kill looping)

**File:** `crates/openscript-ffmpeg/src/filter_graph.rs` (`build_post_trim`)

1. Remove the unknown-duration `loop_count = 3` fallback → always emit `loop=1`
   (single play; `loop=1` is also ffmpeg's default, so we can even omit the param).
2. Remove the `ceil(seg/clip)+1` repeat path entirely. When `src_dur < clip_duration_s`,
   emit a `tracing::warn!` with segment window + source duration + gap seconds so the
   render log records the deficiency.
3. Keep the PTS-shift (`setpts=PTS-STARTPTS+{start_s}/TB`) — that fix stays.
4. Keep `loop=0` guard (never emit infinite).

**Result:** a short clip plays once and the overlay window beyond the clip end holds the
last frame (ffmpeg `eof_action=repeat` default) — a visible, validator-detectable gap,
never a disguised loop.

**Tests:** update the 6 Phase 132 b-roll tests + add: `loop=1` always emitted (no `loop=3`,
no `loop=4+`); short-source test now asserts `loop=1` and a warn path.

### Phase B — Validator: actionable gap errors (loop-closure signal)

**Files:** `crates/openscript-mcp/src/tools.rs`, `crates/openscript-core/src/production_quality.rs`

1. **New data-driven check** in `verify.production` (and a lighter version in
   `timeline.validate`): for each b-roll track event, probe its asset (`crate::probe::probe`
   is already in `openscript-ffmpeg`; `timeline.validate` runs in core, so do the probe in
   the MCP layer and pass it in) and compare `asset_duration_s` vs
   `(end_ms - start_ms)/1000`.
2. When `asset_duration_s < segment_duration - tolerance` (e.g. tolerance 0.25s), emit a
   **hard-fail finding** with:
   - `segment_id` (event id), `concept` (from event kind), `asset_id`, `asset_path`
   - `required_s`, `available_s`, `gap_s`
   - `action`: `"re-run broll.keywords + broll.fetch for segment <id> — need clip ≥ <required>s"`
3. Add a new dimension `broll_coverage` (or fold into `score_broll_motion` detail) so the
   JSON report carries `broll_gaps: [...]` and the agent can machine-read exactly which
   segments need a re-fetch. Include in `next_actions`.
4. Wire it into `hard_fails` so the production grade drops when coverage is broken
   (mirroring the existing `STATIC HARD` gate).

**Result:** the audit loop closes — validator says *which* segments, *why*, and *what to do*;
the agent re-runs keyword generation for those segments, fetches longer clips, re-renders.

### Phase C — Segmentation: enforce min/max per SEGMENTATION_ARCHITECTURE.md

**Files:** `crates/openscript-mcp/src/tools.rs`, `crates/openscript-core/src/srt/mod.rs`

1. **Fix `handle_segment_analyze` (line 5525)** — the A2V main path:
   - Replace `SCENE_SIZE=4` chunking with the sentence-aware grouping
     (`group_entries_with_words_max_duration` with max_gap 0.3s).
   - Apply `min_duration_s` (default 2.0) merge and `max_duration_s` (default 6.0) split
     to every segment, matching `srt.to_timeline`'s behavior.
   - Accept optional `min_duration_s` / `max_duration_s` args (defaults per doc).
2. **Harden `srt.to_timeline`'s sentence-aware mode:**
   - Fix the merge bug: currently merging a short segment into the *next* only checks the
     previous segment's duration; after merge the combined segment can exceed `max_duration_s`.
     Re-check post-merge and re-split if needed.
   - Add "split long at longest internal pause" for entries that exceed `max_duration_s`
     (doc §3.3 Step 4) instead of keeping them as-is.
3. **Make enforcement the default:** when `srt.to_timeline` is called without
   `max_duration_s`, default to sentence-aware with doc defaults (2.0 / 6.0) instead of
   `scene_size=1`, OR at minimum have `segment.analyze` (the A2V tool) enforce always.
4. **B-roll fetch duration hint:** `broll.fetch` auto-place already sizes events to the
   segment window (`end_ms = start_ms + seg_dur`); add `source_duration_s` hint from the
   Pexels API response into the asset record so `verify.production` can compare without
   re-probing (fallback: probe at verify time).

**Result:** every A2V segment lands in [min, max]; long cuts disappear; caption pacing
matches the doc's retention research.

### Phase D — Regression fixture + docs

1. Add a fixture where clips are **shorter** than segments (e.g. a 1.5s clip on a 4s
   segment) to prove: render plays once (no loop), verifier flags the gap with
   `broll_gaps` actionable entries.
2. Update `AGENT_GUIDE.md` (tool descriptions for `segment.analyze`, `srt.to_timeline`,
   `verify.production`) and `AGENTS.md` if protocol changes (tool counts).
3. Keep `docs/SEGMENTATION_ARCHITECTURE.md` as the design doc; point it to this plan's
   implementation status.

---

## 3. Validation Criteria (Definition of Done)

- [ ] `cargo build --workspace --exclude openscript-tauri` — zero warnings
- [ ] `cargo test --workspace --exclude openscript-tauri --lib --bins --tests` — all pass
      (existing 330 + new coverage/loop/segmentation tests)
- [ ] Filter-graph tests assert `loop=1` for known-duration AND unknown-duration paths
      (no `loop=3`, no multi-loop)
- [ ] `verify.production` on a short-clip fixture returns `broll_gaps` with segment id,
      required/available durations, and `action: re-run keyword generation`
- [ ] `segment.analyze` output: all segments within [2.0s, 6.0s] on the audit audio
- [ ] Production render of audit_v3: clips play once (frame-hash: clip content changes,
      then holds — no content re-loop)
- [ ] Workspace lint: 0 errors
- [ ] Commit + push per §7 protocol

---

## 4. Risk Notes

- **`timeline.validate` is sync** (`crates/openscript-core`): the asset-duration probe is
  async. Keep `timeline.validate` structural-only; add the coverage check in the async MCP
  layer (`handle_timeline_validate` / `verify.production`) so we don't pull ffprobe into core.
- **`loop=1` + short clip → held last frame:** this is intentional (the gap must be
  visible) but will make *raw* frames static in the tail. The validator's
  `longest_static_run_s` will flag it — which is the desired loop-closure behavior, but
  verify and render logs must both name the segment so the agent doesn't chase a phantom.
- **Backward compat:** `scene_size` param stays (legacy); the sentence-aware path becomes
  the default only in `segment.analyze` (A2V) first, then `srt.to_timeline` in a follow-up
  if no regressions.
