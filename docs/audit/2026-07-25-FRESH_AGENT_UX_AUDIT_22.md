# Fresh-Agent UX Audit #22 — A2V Atomic Tool Chain

**Date:** July 25, 2026  
**Audit Type:** Audio-to-Video (A2V) via atomic tool chain  
**Input:** `/home/ishanp/Downloads/audit_v3_render.mp4` (135.4s, 20.9MB, Hinglish speech)  
**Agent Model:** mimo-v2.5  
**Audit Method:** Each MCP tool called individually via stdio — no monolithic orchestrators  

---

## Tool Sequence Attempted

| Step | Tool | Result | Details |
|------|------|--------|---------|
| 1 | `transcribe` | ✅ PASS | SRT generated at `/home/ishanp/Downloads/audit_v3_render.srt` |
| 2 | `srt.prepare` | ✅ PASS | Grouped SRT created |
| 3 | `timeline.build` | ✅ PASS | Timeline JSON created |
| 4 | `segment.analyze` | ✅ PASS | Generated keywords per segment |
| 5 | `broll.plan` | ❌ FAIL | **Empty segment count** — timeline had no populated segments |
| 6 | `music.search` | ⚠️ WARN | Search returned results but path extraction failed |
| 7 | `music.assign` | ⏭️ SKIP | Skipped due to missing music path |
| 8 | `sfx.search` | ⚠️ WARN | No SFX library indexed |
| 9 | `captions.generate_ass` | ✅ PASS | ASS file generated |
| 10 | `timeline.validate` | ✅ PASS | Reported valid (false positive — empty timeline) |
| 11 | `timeline.render` | ❌ FAIL | **"No segments in EDL"** — ffmpeg error |
| 12 | `verify.render` | ❌ SKIP | No video to verify |

**Final Score: 5/12 steps passed (42%)**  
**Video Produced: NO**

---

## Critical UX Gaps Identified

### GAP-1: CRITICAL — Empty Timeline After `timeline.build` (Score Impact: -40%)

**Problem:** `timeline.build` creates a valid but EMPTY timeline. The agent then has no clear path to populate it with segments from the SRT. The trajectory in AGENTS.md says `timeline.add_segment` but:

1. The agent must parse the SRT to extract start/end times and caption text for EACH segment
2. There is NO tool that converts SRT entries → timeline segments automatically
3. The agent must call `timeline.add_segment` N times (once per SRT entry) — this is O(N) tool calls
4. There is no batching mechanism

**Root Cause:** Missing `srt.to_timeline` tool that converts SRT → timeline segments in one call.

**Fix:** Add `srt.to_timeline` tool: `transcribe → srt.prepare → srt.to_timeline → ...`

### GAP-2: HIGH — `broll.plan` Returns Empty When Timeline Has No Segments (Score Impact: -15%)

**Problem:** `broll.plan` reads segments from the timeline JSON. If the timeline is empty (which it always is after `timeline.build`), broll.plan returns 0 segments. The tool should either:
- Auto-populate segments from the SRT, OR
- Return a clear error message: "Timeline has no segments. Call timeline.add_segment first."

**Fix:** Add validation in `broll.plan` that returns actionable error when timeline is empty.

### GAP-3: HIGH — `timeline.validate` Reports Valid on Empty Timeline (Score Impact: -10%)

**Problem:** `timeline.validate` reported the empty timeline as "valid". An empty timeline with no segments should be flagged as invalid for rendering.

**Fix:** Add check: timeline must have at least 1 segment to be "valid".

### GAP-4: MEDIUM — No Music Path Extraction from `music.search` (Score Impact: -10%)

**Problem:** `music.search` returned results but the agent couldn't extract a usable file path. The tool returns a JSON array of results but the path field format varies.

**Fix:** Ensure `music.search` returns consistent `path` field in all results.

### GAP-5: MEDIUM — `sfx.search` Returns Empty When No Library Indexed (Score Impact: -5%)

**Problem:** SFX library was not indexed, so `sfx.search` returned empty. The agent has no way to know this without calling `sfx.index` first.

**Fix:** `sfx.search` should return a hint: "No SFX library found. Call sfx.index first."

### GAP-6: LOW — No Agent Guidance on Step Ordering (Score Impact: -5%)

**Problem:** The agent received 90 tools with no clear guidance on which to call in what order. The AGENTS.md trajectory is helpful but the MCP server's `initialize` instructions don't reference it.

**Fix:** Add trajectory hints to `system.capabilities` or `help.tool` output.

---

## What Worked

1. **Transcription pipeline** — `transcribe` → SRT generation worked flawlessly
2. **SRT preparation** — `srt.prepare` grouped captions correctly
3. **Timeline creation** — `timeline.build` created valid JSON structure
4. **Segment analysis** — `segment.analyze` generated meaningful keywords
5. **Caption generation** — `captions.generate_ass` produced ASS file
6. **MCP protocol** — JSON-RPC over stdio worked reliably

---

## Score Breakdown

| Dimension | Score | Weight | Weighted |
|-----------|-------|--------|----------|
| Tool discoverability | 6/10 | 15% | 0.90 |
| Step ordering clarity | 3/10 | 20% | 0.60 |
| Error recoverability | 2/10 | 20% | 0.40 |
| Pipeline completeness | 4/10 | 25% | 1.00 |
| Output quality | 0/10 | 20% | 0.00 |
| **TOTAL** | | | **2.90/10** |

**Fresh-Agent Score: 2.9/10** (down from 5.0 in Audit #21 — the atomic chain is harder than the monolithic orchestrator)

---

## Recommendations (Priority Order)

1. **Add `srt.to_timeline` tool** — Converts SRT → timeline segments in one call. This is the #1 blocker.
2. **Add `timeline.populate` tool** — Reads SRT, creates segments, fetches backgrounds, assigns music — all in one atomic call that the agent can choose to use.
3. **Fix `timeline.validate`** — Reject empty timelines.
4. **Fix `broll.plan`** — Return actionable error when timeline has no segments.
5. **Add `audio.to_video` back as a DOCUMENTED escape hatch** — For agents that don't want to orchestrate 15+ atomic calls.
6. **Improve error messages** — Every tool should return actionable next-step hints on failure.

---

## Comparison with Previous Audits

| Audit | Score | Method | Video Produced |
|-------|-------|--------|----------------|
| #21 | 5.0/10 | Monolithic `audio.to_video` | YES |
| #22 | 2.9/10 | Atomic tool chain | NO |

**Insight:** The monolithic orchestrator works better for fresh agents because it hides the complexity. The atomic chain gives MORE control but requires MORE knowledge. The solution is to support BOTH: atomic tools for experienced agents, monolithic orchestrator for fresh agents.
