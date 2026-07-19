# motion.render End-to-End Failure Report

**Date**: 2026-04-11
**Author**: End-user (AI agent via OpenScript MCP)
**Severity**: P0 (Critical — tool lies about output, delivers broken result)

## Executive Summary

`motion.render` was asked to produce a 95-second (2850 frames) kinetic typography documentary. The tool returned a response claiming success with `frame_count: 2850`, `duration_ms: 95000`. The actual output file was **30 seconds** (900 frames) with **silent audio**. The tool fabricated its success metadata. The end user received a video that was 31% of the requested length with no sound.

This report documents 7 distinct failure modes, their root causes, and actionable fixes.

---

## Failure 1: Fabricated Response Metadata (P0)

### What Happened
- I passed `duration_in_frames: 2850` to `motion.render`
- The tool returned: `{"duration_ms": 95000, "frame_count": 2850, "status": "rendered"}`
- Actual output: `{"duration": "30.058667", "nb_frames": "900"}` (verified via ffprobe)

### Root Cause
`crates/openscript-mcp/src/motion/renderer.rs` lines 101-108:

```rust
let duration_ms = ((args.duration_in_frames as f64) / (args.fps as f64) * 1000.0) as u64;

Ok(RenderResult {
    output_path: output_str,
    duration_ms,           // ← CALCULATED from INPUT, not measured from OUTPUT
    file_size: metadata.len(),
    frame_count: args.duration_in_frames,  // ← INPUT value, not actual
})
```

The renderer **never reads the actual output file** to determine duration or frame count. It returns what was *requested*, not what was *produced*.

### Impact
- Agent believes render succeeded with correct parameters
- Agent reports success to user
- User receives broken output
- No verification layer catches this — `verify.render` would catch it but is optional and no agent is told to run it

### Fix
```rust
// Measure actual output after render
let probe = Command::new("ffprobe")
    .args(["-v", "error", "-show_entries", "format=duration", "-show_entries",
           "stream=nb_frames", "-of", "json", &output_str])
    .output().await?;
// Parse and return ACTUAL values, not requested ones
```

---

## Failure 2: `duration_in_frames` Parameter Is Silently Ignored (P0)

### What Happened
- I passed `duration_in_frames: 2850` (95 seconds)
- Remotion rendered exactly 900 frames (30 seconds)
- No error, no warning, no truncation notice

### Root Cause
`renderer.rs` line 71-78:
```rust
let render_output = Command::new("npx")
    .args([
        "remotion",
        "render",
        "HotMotion",
        &output_str,
        "--log-level=error",
    ])
```

The CLI command does **not** pass `--duration-in-frames` or `--frames`. Remotion reads `durationInFrames={900}` from `remotion/src/RemotionRoot.tsx` line 27:

```tsx
<Composition
  id="HotMotion"
  component={HotMotion}
  durationInFrames={900}  // ← HARDCODED
  fps={30}
  width={1080}
  height={1920}
  defaultProps={{ tokens: {}, slides: [] }}
/>
```

The `duration_in_frames` parameter accepted by `motion.render` is a **phantom parameter** — it exists in the tool schema but has zero effect on the actual render.

### Impact
- Any request > 30 seconds is silently truncated
- Agent has no way to know the composition is capped at 30s
- `motion.get_info` does not report composition duration limits
- `motion.compile_check` passes because it only checks TypeScript, not composition config

### Fix Options

**Option A (Recommended)**: Pass `--duration-in-frames` to the Remotion CLI:
```rust
.args([
    "remotion", "render", "HotMotion", &output_str,
    &format!("--duration-in-frames={}", args.duration_in_frames),
    "--log-level=error",
])
```

**Option B**: Dynamically update `RemotionRoot.tsx` before render to match requested duration.

**Option C**: Reject requests exceeding the composition's registered duration with a clear error message.

---

## Failure 3: No Pre-Render Validation of Sequence Overflow (P1)

### What Happened
- My composition had Sequences extending to frame 2850 (outro at 2700-2850)
- `motion.compile_check` passed with 0 errors
- `motion.validate` would have caught this IF it checked composition duration, but it's a heuristic check, not a composition-aware one

### Root Cause
`motion.compile_check` runs `tsc --noEmit` — it checks TypeScript type correctness only. It has no awareness of:
- The composition's registered `durationInFrames`
- Whether Sequences extend beyond the composition boundary
- Whether `from + durationInFrames` exceeds the composition limit

### Impact
- Sequences at frames 210-2850 compile fine but are silently discarded at render time
- Agent gets false confidence from "0 errors" compile check

### Fix
`motion.compile_check` (or a new `motion.validate_duration` tool) should:
1. Read `RemotionRoot.tsx` to extract the composition's `durationInFrames`
2. Parse the TSX to find the maximum `from + durationInFrames` across all Sequences
3. Warn if any Sequence extends beyond the composition boundary

---

## Failure 4: Silent Audio Track Not Detected or Warned (P1)

### What Happened
- Output file contains an AAC audio stream (48kHz, stereo, 317kbps)
- Audio stream is silence (no Audio elements in composition)
- User reports "no music or sound"

### Root Cause
Remotion renders a silent audio track when the composition has no `<Audio>` elements but the render codec defaults to including audio. The renderer does not:
- Check if the composition contains any Audio elements
- Warn that the output will be silent
- Offer to skip the audio track to reduce file size

### Impact
- Users expect sound in a "documentary" format
- No warning is issued that the output is silent
- File includes unnecessary audio stream

### Fix
After render, probe the audio stream for silence:
```bash
ffmpeg -i output.mp4 -af "volumedetect" -f null /dev/null
# If max_volume < -60dB, warn: "Output contains silent audio track"
```

Or better: add a pre-render check that parses the TSX for `<Audio` or `<OffthreadAudio` elements and warns if none found.

---

## Failure 5: No TTS Fallback Guidance When Voicebox Is Down (P2)

### What Happened
- I requested a "voiceover documentary"
- `openscript_voice_profile_list` returned: `Voicebox model is not loaded`
- I had to manually pivot to Remotion motion graphics
- The tool suite offered no guidance on alternatives

### Root Cause
The TTS tools return an error but provide no guidance. There's no `tts.status` or `tts.health` tool. The motion tools don't suggest themselves as fallback.

### Impact
- Agent wastes time discovering TTS is down
- No clear path from "voiceover requested" → "fallback to text-based"
- User expectation mismatch

### Fix
1. Add `tts.health` tool that returns voicebox status
2. When TTS fails, suggest: "Voicebox unavailable. Consider `motion.render` for text-based kinetic typography, or `reelize.timeline` if you have source video with natural dialogue."

---

## Failure 6: `motion.get_info` Doesn't Report Composition Duration (P2)

### What Happened
- `motion.get_info` returned compositions, fonts, node version, remotion version
- It did NOT report the `durationInFrames` for each composition

### Root Cause
The tool queries Remotion project capabilities but skips composition metadata like duration.

### Impact
- Agent cannot know that HotMotion is capped at 30 seconds
- Agent assumes `duration_in_frames` parameter works as documented

### Fix
`motion.get_info` should return:
```json
{
  "compositions": [
    {
      "id": "HotMotion",
      "durationInFrames": 900,
      "durationSeconds": 30,
      "fps": 30,
      "width": 1080,
      "height": 1920
    }
  ]
}
```

---

## Failure 7: `motion.render` Ignores Stderr on Success (P2)

### What Happened
- Remotion CLI may have emitted warnings about frame truncation
- `renderer.rs` only checks `status.success()` — it captures stderr only on failure
- Any truncation warnings are silently discarded

### Root Cause
`renderer.rs` lines 84-95:
```rust
if !render_output.status.success() {
    let stderr = String::from_utf8_lossy(&render_output.stderr);
    // ... only captured on failure
}
// On success: no stderr capture, no warnings forwarded
```

### Fix
Always capture and forward stderr/stdout as warnings in the response:
```rust
let warnings = if !render_output.stderr.is_empty() {
    Some(String::from_utf8_lossy(&render_output.stderr).to_string())
} else { None };
```

---

## Reproduction Steps

1. Call `motion.render` with `duration_in_frames: 2850` (any value > 900)
2. Observe tool returns `frame_count: 2850, duration_ms: 95000`
3. Run `ffprobe output.mp4` — actual is 900 frames, 30 seconds
4. Observe audio stream exists but contains silence

## Files Affected

| File | Issue |
|------|-------|
| `crates/openscript-mcp/src/motion/renderer.rs` | Fabricated metadata, silent stderr, no duration CLI flag |
| `remotion/src/RemotionRoot.tsx` | Hardcoded 900 frames for HotMotion |
| `crates/openscript-mcp/src/motion/validator.rs` | No duration overflow check |
| `crates/openscript-mcp/src/tools.rs` | `motion.get_info` missing composition duration |

## Recommended Priority Order

1. **Fix fabricated metadata** (Failure 1) — measure actual output, don't lie
2. **Wire up duration_in_frames** (Failure 2) — pass it to Remotion CLI or reject gracefully
3. **Expose composition duration** (Failure 6) — `motion.get_info` must report limits
4. **Add pre-render duration validation** (Failure 3) — catch sequence overflow before render
5. **Detect silent audio** (Failure 4) — warn when output has no sound
6. **Forward render warnings** (Failure 7) — don't swallow stderr on success
7. **TTS health endpoint** (Failure 5) — guide agents when TTS is unavailable
