# Plan: Fix motion.render End-to-End Failure (7 Failure Modes)

**Source**: `.sisyphus/feedback/motion-render-end-to-end-failure-report.md`
**Date**: 2026-04-11
**Scope**: `crates/openscript-mcp/src/motion/` + `remotion/src/RemotionRoot.tsx` + `crates/openscript-mcp/src/tools.rs`

---

## Executive Summary

`motion.render` has 7 failure modes that collectively produce a lying tool — it reports success with fabricated metadata while delivering truncated, silent output. The root cause chain: `duration_in_frames` is never passed to Remotion CLI (hardcoded 900 frames in RemotionRoot.tsx), the renderer calculates metadata from input rather than measuring output, and no validation layer catches the overflow.

**Fix strategy**: Measure actual output, wire duration to Remotion CLI, expose composition limits, validate before render.

---

## Phase 1: Core Fixes (P0 — Stop the Lying)

### Task 1.1: Measure Actual Output in renderer.rs

**File**: `crates/openscript-mcp/src/motion/renderer.rs` (lines 97-108)

**Problem**: Lines 101-107 calculate `duration_ms` and `frame_count` from input args, not from the actual rendered file.

**Fix**:
1. After the render succeeds and before returning `RenderResult`, run `ffprobe` on the output file:
   ```
   ffprobe -v error -show_entries format=duration -show_entries stream=nb_frames,width,height -of json <output_path>
   ```
2. Parse the JSON response using `serde_json` to extract actual `duration` (seconds as string) and `nb_frames`.
3. Add a new field `warnings: Vec<String>` to `RenderResult` to surface post-render findings.
4. If ffprobe fails or returns empty, fall back to input-calculated values but add a warning.
5. Replace the fabricated values with actual measured values.

**New imports needed**: `serde_json`, `std::process::Command` (or `tokio::process::Command`).

**Struct changes**:
```rust
pub struct RenderResult {
    pub output_path: String,
    pub duration_ms: u64,
    pub file_size: u64,
    pub frame_count: u32,
    pub warnings: Vec<String>,  // NEW
}
```

**Handler update** in `tools.rs` (line 4705): Add `"warnings": result.warnings` to the JSON response.

---

### Task 1.2: Wire `duration_in_frames` to Remotion CLI

**Files**: `crates/openscript-mcp/src/motion/renderer.rs` (lines 71-78), `remotion/src/RemotionRoot.tsx` (line 27)

**Problem**: The Remotion CLI command at renderer.rs:71-78 does not pass `--duration-in-frames`. RemotionRoot.tsx hardcodes `durationInFrames={900}`.

**Root cause analysis**: Remotion 4.x supports `--duration-in-frames` CLI flag (confirmed via GitHub issue #4917 → PR #6573, merged). The `--fps` flag is also supported.

**Fix in renderer.rs**:
Add two new args to the CLI command:
```rust
.args([
    "remotion", "render", "HotMotion", &output_str,
    &format!("--duration-in-frames={}", args.duration_in_frames),
    &format!("--fps={}", args.fps),
    "--log-level=error",
])
```

**No changes needed to RemotionRoot.tsx** — the CLI flag overrides the composition's registered duration in Remotion 4.x. The hardcoded 900 becomes the default when no CLI flag is provided.

**Note**: If for some reason the CLI flag is not available in the installed `@remotion/cli` version, fall back to Option C (reject with error if requested > 900).

---

### Task 1.3: Capture and Forward Render Warnings

**File**: `crates/openscript-mcp/src/motion/renderer.rs` (lines 84-95)

**Problem**: stderr is only captured on failure. Successful renders may have warnings that are silently discarded.

**Fix**:
1. Always capture stdout and stderr after a successful render.
2. Parse for warning patterns (e.g., "truncated", "exceeds", "warning", "deprecated").
3. Push any non-empty stderr/stdout content to `RenderResult.warnings`.

---

## Phase 2: Validation & Visibility (P1/P2)

### Task 2.1: Add Duration Overflow Validation to compile_check

**File**: `crates/openscript-mcp/src/motion/compiler.rs`

**Problem**: `compile_check_tsx` only runs `tsc --noEmit` — no awareness of composition duration limits or Sequence overflow.

**Fix**: Add a new function `validate_duration_overflow(tsx_code: &str, composition_duration: u32) -> Vec<ValidationIssue>`:

1. Parse `RemotionRoot.tsx` to extract the composition's `durationInFrames` (fallback to 900 if CLI override expected).
2. Use the existing `estimate_duration` logic from `validator.rs` to find the maximum `from + durationInFrames` across all Sequences in the TSX code.
3. If max_sequence_end > composition_duration, return a warning.
4. Integrate this check into `compile_check_tsx` as a post-complication validation step — it adds warnings to the result even when tsc passes.

**New struct** (or reuse from validator):
```rust
pub struct DurationOverflowIssue {
    pub max_sequence_end: u32,
    pub composition_limit: u32,
    pub overflow_frames: u32,
}
```

Add to `CompileCheckResult`:
```rust
pub duration_overflow: Option<DurationOverflowIssue>,  // NEW
```

---

### Task 2.2: Expose Composition Duration in motion.get_info

**File**: `crates/openscript-mcp/src/motion/info.rs`

**Problem**: `MotionInfo.compositions` is `Vec<String>` — just names. No metadata about duration, fps, dimensions.

**Fix**:
1. Create a new struct:
   ```rust
   pub struct CompositionInfo {
       pub id: String,
       pub duration_in_frames: u32,
       pub fps: u32,
       pub width: u32,
       pub height: u32,
   }
   ```
2. Change `MotionInfo.compositions` from `Vec<String>` to `Vec<CompositionInfo>`.
3. In `discover_compositions`, parse `RemotionRoot.tsx` to extract `<Composition>` blocks and their props:
   - Use regex or line-by-line parsing to find `<Composition` blocks
   - Extract `id`, `durationInFrames`, `fps`, `width`, `height` attributes
   - Each `<Composition>` block spans multiple lines — track open/close angle brackets to determine block boundaries
4. For the HotMotion composition (written at runtime), the effective duration is determined by the CLI override — note this in the response.
5. Update `handle_motion_get_info` in `tools.rs` to serialize the new structure.

---

### Task 2.3: Detect Silent Audio in Output

**File**: `crates/openscript-mcp/src/motion/renderer.rs`

**Problem**: Remotion renders a silent audio track when no `<Audio>` elements exist. No warning is issued.

**Fix** (add to post-render ffprobe step in Task 1.1):
1. After ffprobe extracts metadata, check if an audio stream exists.
2. If audio stream exists, run:
   ```
   ffmpeg -i <output> -af volumedetect -f null /dev/null
   ```
3. Parse `max_volume` from stderr. If `< -60dB`, add warning: "Output contains silent audio track (max_volume: X dB). Consider adding <Audio> or <OffthreadAudio> elements."
4. Also check the TSX code for `<Audio` or `<OffthreadAudio` before render — if none found, add a pre-render note.

---

## Phase 3: Agent Guidance (P2)

### Task 3.1: Update Tool Descriptions

**File**: `crates/openscript-mcp/src/tools.rs` (tool schema definitions)

**Changes**:
1. `motion.render` description: Add note that `duration_in_frames` is now respected and actual output metrics are measured.
2. `motion.compile_check` description: Add note that it now checks for Sequence duration overflow.
3. `motion.get_info` description: Update to mention that composition metadata (duration, fps, dimensions) is included.

---

## Implementation Order & Dependencies

```
Task 1.1 (measure output) ─────────────┐
                                        ├── Task 1.3 (forward warnings) ──┐
Task 1.2 (wire duration) ──────────────┤                                  │
                                        │                                  ▼
Task 2.1 (duration overflow check) ────┤                           RenderResult with
                                        │                           actual metrics + warnings
Task 2.2 (composition info) ───────────┤
                                        │
Task 2.3 (silent audio detection) ─────┘

Task 3.1 (update descriptions) ──────── Depends on all above
```

**Recommended execution order**:
1. **1.2** (wire duration) — unblocks everything else; if this doesn't work, we need Option C
2. **1.1** (measure output) — fixes the lying metadata
3. **1.3** (forward warnings) — depends on 1.1's warning infrastructure
4. **2.2** (composition info) — independent, can be parallel
5. **2.1** (duration overflow) — depends on 2.2's composition parsing
6. **2.3** (silent audio) — depends on 1.1's ffprobe infrastructure
7. **3.1** (descriptions) — cosmetic, last

---

## Testing Strategy

1. **Unit**: Test ffprobe output parsing with mock JSON responses
2. **Unit**: Test RemotionRoot.tsx composition block parser with various formats
3. **Integration**: Call `motion.render` with `duration_in_frames: 2850` and verify actual output is 2850 frames via ffprobe
4. **Integration**: Call `motion.get_info` and verify HotMotion composition reports `durationInFrames: 900`
5. **E2E**: Render a composition with Sequences extending to frame 1800 with `duration_in_frames: 900` — verify compile_check warns about overflow

---

## Risk Assessment

| Risk | Likelihood | Mitigation |
|------|-----------|------------|
| `--duration-in-frames` not available in installed Remotion version | Low (Remotion 4.x confirmed) | Fall back to error: "Requested X frames but composition supports max 900" |
| ffprobe not installed | Low (FFmpeg is a prerequisite) | Graceful fallback with warning |
| RemotionRoot.tsx parsing breaks on format changes | Medium | Use robust multi-line block parser, not regex |
| Silent audio detection adds render time | Low | volumedetect is fast (<1s for 30s video) |
