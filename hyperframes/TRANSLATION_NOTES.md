# Translation Notes — MainWithBroll.tsx → HyperFrames

Ported `remotion/src/compositions/MainWithBroll.tsx` to
`hyperframes/compositions/main-with-broll/index.html` using the
`remotion-to-hyperframes` skill.

## Source lint result

The Remotion source is **clean** — no blockers detected:

| Rule | Status |
|------|--------|
| `r2hf/use-state` | ✅ not present |
| `r2hf/use-reducer` | ✅ not present |
| `r2hf/use-effect-deps` | ✅ not present |
| `r2hf/async-metadata` | ✅ not present |
| `r2hf/third-party-react-ui` | ✅ not present |

Warnings (translated after dropping):

| Rule | Action |
|------|--------|
| `r2hf/use-memo` | Dropped. `useMemo` in `CrossFadeVideo` and `BrollLayer` was a performance optimization; HF's single-timeline model doesn't need memoization. Inlined the computation. |

## Translation mapping

| Remotion | HyperFrames |
|----------|-------------|
| `<AbsoluteFill>` | `<div id="stage">` with `position: relative; width; height` |
| `<Sequence from durationInFrames>` | `data-start` / `data-duration` (in seconds, not frames) |
| `useCurrentFrame()` + `interpolate()` | `gsap.timeline().to(..., delay, duration)` |
| `<Video src muted>` | `<video src muted playsinline>` |
| `<Audio src>` | Omitted — audio is mixed by FFmpeg in the OpenScript pipeline, not by the visual renderer |
| `crossfade transitionIn=6 transitionOut=6` (frames) | GSAP opacity tween, `duration = frames / fps` seconds |
| `useMemo(() => ..., [deps])` | Dropped — inlined |

## What translated cleanly

- **AbsoluteFill + Sequence → div + data-\*:** Direct mapping. HF's `data-start`/`data-duration` replace Remotion's `from`/`durationInFrames`.
- **interpolate opacity → GSAP opacity tween:** `interpolate(frame, [from, from+transitionIn], [0, 1])` becomes `tl.to(el, { opacity: 1, duration: transitionIn/fps }, from/fps)`.
- **Multiple video events on same track:** Each becomes a separate `<video>` element with opacity tweens that don't overlap. Matches Remotion's `<Sequence>` behavior.
- **B-roll crossfade over main:** B-roll `<video>` sits on a higher z-index (later in DOM), opacity tweens create the crossfade. Same visual result.

## Gaps (with caveats)

### 1. Audio track omitted

**Source:** `<Audio src={sources.main} />` in `MainWithBroll.tsx` plays the main video's audio.

**HF translation:** Omitted. In OpenScript, audio mixing is handled by the FFmpeg pipeline (`openscript-ffmpeg/src/render.rs`), not by the visual renderer. The HF composition produces silent video; FFmpeg mixes the audio tracks separately.

**Impact:** None for OpenScript's pipeline — the visual render and audio mix are separate steps. If using HF standalone (without FFmpeg), you'd need to add an `<audio>` element.

### 2. Video seeking behavior

**Source:** Remotion's `<Video>` component handles frame-accurate seeking of the source video.

**HF translation:** HF's runtime seeks `<video>` elements via `currentTime`. This is frame-accurate for most content but may differ from Remotion's frame-exact extraction for variable-frame-rate footage.

**Impact:** Negligible for constant-frame-rate sources (the OpenScript pipeline assumes CFR).

### 3. `useMemo` dropped

**Source:** `useMemo` in `CrossFadeVideo` and `BrollLayer` memoized opacity calculations and the broll map.

**HF translation:** Dropped — inlined. HF's single-timeline model computes all values upfront when the GSAP timeline is built, so memoization is unnecessary.

**Impact:** None. Performance is equivalent or better (no React re-render overhead).

### 4. B-roll map lookup timing

**Source:** `brollMap.get(event.id)` runs at render time in React.

**HF translation:** B-roll `src` is resolved at compile time (in `edl_v2_to_html.ts`) and baked into the `src` attribute. Missing b-roll IDs are skipped at compile time (not at render time).

**Impact:** A missing b-roll ID produces no element in the HF output, whereas in Remotion it produces a `<Sequence>` with no content. Same visual result (nothing renders).

## What did NOT translate (out of scope)

- **The Remotion Studio props panel** — visual prop editing in HF Studio needs different infrastructure; out of scope.
- **`calculateMetadata`** — not used in `MainWithBroll.tsx` (duration comes from the timeline prop, not async metadata).
- **HDR rendering** — HF supports HDR but the source doesn't use it.

## Verification

To verify the port is faithful, run the SSIM eval harness from the
`remotion-to-hyperframes` skill:

```bash
# Render Remotion baseline
cd remotion && npx remotion render MainWithBroll out/baseline.mp4

# Render HF translation
cd ../hyperframes/compositions/main-with-broll && npx hyperframes render --output ../../out/hf.mp4

# SSIM diff
../../scripts/render_diff.sh ../../remotion/out/baseline.mp4 ../../out/hf.mp4 ../../out/diff
```

Threshold: ~0.02 below `p05` of the source's complexity tier. The validated
baselines from the `remotion-to-hyperframes` skill (T1: 0.974, T2: 0.985)
suggest this composition should pass cleanly.
