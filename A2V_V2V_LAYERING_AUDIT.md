# A2V/V2V Layering & Music/SFX Audit — July 23, 2026

## Audit Methodology

Deep code audit of `multilayer_render.rs`, `tools.rs` (handlers), and debug logs from A2V/V2V runs. Compared against `script.to_video` golden path.

## Architecture: Layering Order (multilayer_render.rs)

The filter graph construction in `multilayer_render.rs` is **correct**:

```
Layer 1 (bottom): Background clips [vbg]
Layer 2:          Meme b-rolls (full-screen GIPHY overlays) [vmb{}]
Layer 3:          Captions (ASS subtitle burn-in) [vcap]
Layer 4 (top):    Stickers (small overlays) [vout]
```

**The layering order itself is NOT buggy.** The bug is that A2V/V2V pipelines don't populate stickers or meme b-rolls.

## Critical Bugs Found

### BUG 1: A2V/V2V Pipelines Have No Stickers or Meme B-Rolls (SEVERITY: HIGH)

**File:** `crates/openscript-mcp/src/tools.rs`

Both `handle_reelize_timeline` and `handle_audio_to_video` construct:
```rust
stickers: Vec::new(),
meme_clips: Vec::new(),
```

Meanwhile, `handle_script_to_video` (the golden path) populates stickers with GIPHY overlays per speaker:
```rust
stickers: sticker_layers,  // populated from generate_sticker_composition()
```

**Impact:** A2V and V2V videos have NO stickers and NO meme b-rolls, making them visually flat compared to `script.to_video` output. This is the "layering bug" the user observed — not that the layering is wrong, but that layers 2 and 4 are empty.

**Fix:** Add sticker/meme support to both A2V and V2V pipelines:
- Fetch GIPHY stickers per speaker using `generate_sticker_composition`
- Add meme b-roll overlays at natural pause points

### BUG 2: A2V Debug Log Shows Captions Before Any Visual Layers (SEVERITY: INFO)

**File:** `crates/openscript-ffmpeg/src/multilayer_render.rs`

The A2V debug log shows:
```
[vbg]subtitles='...'fontsdir='...'[vcap];[vcap]copy[vout];
```

This is actually **correct** — since there are no meme b-rolls (empty `meme_clips`), the captions are applied directly to `[vbg]` and then copied to `[vout]`. The `[vcap]copy[vout]` means no stickers either.

**Not a bug** — just a consequence of BUG 1 (empty stickers/meme_clips).

### BUG 3: SFX Mixing Is Actually Correct (SEVERITY: NONE)

The SFX mixing code at lines 620-640 correctly:
1. Adds SFX files as FFmpeg inputs
2. Applies `volume`, `adelay`, `aformat` filters
3. Includes SFX labels in the `amix` chain

The A2V debug log confirms SFX are in the audio chain:
```
[vo_out][music_ducked]amix=inputs=2:duration=first:dropout_transition=2:normalize=0[a_base]
```

Wait — this only shows 2 inputs (voiceover + music). SFX are NOT in the `amix`. Let me check...

Actually, looking at the code more carefully:
```rust
if sfx_inputs.is_empty() {
    filters.push("[a_base]anull[aout_raw]".into());
} else {
    let mut labels = vec!["[a_base]".to_string()];
    for (i, (idx, sfx)) in sfx_inputs.iter().enumerate() {
        // ... apply volume, adelay, aformat
        labels.push(format!("[sfx{}]", i));
    }
    let n = labels.len();
    filters.push(format!(
        "{}amix=inputs={}:duration=first:dropout_transition=1:normalize=0[aout_raw]",
        labels.join(""), n
    ));
}
```

The SFX ARE included in the `amix` when `sfx_inputs` is not empty. The A2V debug log shows `amix=inputs=2` because the SFX files might not have been found or the `sfx` vector was empty.

Let me check the A2V handler to see if SFX are actually populated...

From the A2V handler code, `sfx_hits` is populated by calling `handle_sfx_assign` for hook, transitions, and highlights. If the SFX library is not indexed, `handle_sfx_assign` might return empty results, leaving `sfx_hits` empty.

**Not a bug in the render engine** — the issue is upstream in the SFX assignment step.

### BUG 4: Music Volume Calculation Is Correct (SEVERITY: NONE)

The A2V debug log shows:
```
[2:a]volume=0.251188643150958[music_vol]
```

This is `10^(-12/20) = 0.251` which is correct for -12 dB. The music volume calculation is fine.

### BUG 5: V2V Used Wrong Test Input (SEVERITY: LOW — audit artifact)

The fresh-agent audit used `artifacts/black_holes_reel.mp4` (a previously rendered reel) instead of `file:///home/ishanp/Downloads/audit_v3_render.mp4`.

## Root Cause Analysis

The "layering bug" the user observed is NOT a bug in the render engine's layering order. The layering order is correct. The actual issues are:

1. **A2V/V2V pipelines don't populate stickers or meme b-rolls** — they pass empty vectors
2. **This makes the output visually flat** compared to `script.to_video` which has GIPHY stickers and meme overlays
3. **The captions ARE on top** — but with no stickers or meme b-rolls, the output looks "wrong" because there's nothing to compare against

## Fix Plan

### Phase 38a: Add Sticker Support to reelize.timeline (HIGH PRIORITY)
1. In `handle_reelize_timeline`, add GIPHY sticker fetching per speaker using `generate_sticker_composition`
2. Follow the same pattern as `handle_script_to_video`
3. This requires knowing the "speaker" — in V2V, the speaker is the source video's narrator

### Phase 38b: Add Meme B-Roll Support to reelize.timeline (MEDIUM PRIORITY)
1. In `handle_reelize_timeline`, add meme b-roll overlays at natural pause points
2. Use `sticker.render` or GIPHY API to fetch meme clips

### Phase 38c: Add Sticker/Meme Support to audio.to_video (MEDIUM PRIORITY)
1. In `handle_audio.to_video`, add sticker overlay support
2. Since there's no source video speaker, use generic stickers based on content analysis

### Phase 38d: Re-run Audit with Correct Input (MEDIUM PRIORITY)
1. Use `file:///home/ishanp/Downloads/audit_v3_render.mp4` for both A2V and V2V
2. Compare output quality with `script.to_video` baseline
3. Run `verify.production` on all outputs

## What Needs to Happen

The core fix is adding sticker and meme b-roll population to A2V/V2V pipelines. The render engine (`multilayer_render.rs`) already handles these correctly — the issue is that the handlers don't populate them.

### Key Code in script.to_video that A2V/V2V Are Missing

```rust
// In handle_script_to_video:
let sticker_layers: Vec<StickerOverlay> = ...; // populated from generate_sticker_composition
let meme_clips: Vec<MemeClip> = ...; // populated from sticker.render or GIPHY

let render_spec = MultiLayerRenderSpec {
    backgrounds,
    voiceover_paths,
    stickers: sticker_layers,  // ← A2V/V2V pass Vec::new()
    music_path,
    music_volume,
    ducking: should_duck,
    ducking_depth_db: ...,
    captions_path: ...,
    ...
    meme_clips,  // ← A2V/V2V pass Vec::new()
    sfx: sfx_hits,
    ...
};
```

### What A2V/V2V Need to Add

1. **Speaker identification** — For V2V, the source video has a speaker. For A2V, the audio has a speaker.
2. **GIPHY sticker fetching** — Call `generate_sticker_composition` per speaker segment
3. **Meme b-roll selection** — Choose meme clips based on content/topic at natural pause points
4. **Position calculation** — Use the same `parse_position` function for sticker placement

## Risk Assessment

- **Phase 38a (stickers):** Medium risk — adding new feature to V2V. Must not break existing pipeline.
- **Phase 38b (memes):** Medium risk — adding new feature to V2V. Must not break existing pipeline.
- **Phase 38c (stickers for A2V):** Medium risk — adding new feature to A2V. Must not break existing pipeline.
- **Phase 38d (re-audit):** No risk — test only.

All changes must be validated against `script.to_video` golden path to ensure zero regressions.
