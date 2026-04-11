/// Returns a comprehensive motion design guide as a static markdown string.
/// Used by the MCP to teach LLMs how to write Remotion compositions for 9:16 shorts.
pub fn load_motion_skill() -> &'static str {
    r#"# Motion Graphics Design Guide for OpenScript + Remotion

You are writing a Remotion composition for 9:16 vertical video (short-form content: TikTok, Reels, Shorts).
Your component will be compiled to `hot-composition.tsx` and rendered with Remotion's CLI.

---

## 0. Iterative Development Workflow (READ THIS FIRST)

Don't write your entire composition and hope it renders. Use the iterative workflow:

```
1. motion.get_info            → What fonts, compositions, and capabilities are available?
2. motion.load_skill          → Learn the rules and patterns
3. motion.design_system       → Get brand tokens (colors, typography, spacing, TIMING)
4. Write background + layout only
5. motion.compile_check       → Type errors? Fix them. (2-5s)
6. motion.preview frame=0     → Layout wrong? Fix it. (2-5s)
7. Add text elements
8. motion.compile_check       → Type errors? Fix them.
9. motion.preview frame=15    → Text readable and positioned? Good.
10. Add animations
11. motion.compile_check      → Type errors? Fix them.
12. motion.preview frame=45   → Animation timing right? Adjust frame ranges.
13. motion.render             → Final MP4 output (high confidence of success)
```

**Why this matters:** Each full render takes 30-120 seconds. A compile_check takes 2-5s. A preview takes 2-5s. Iterating with fast feedback is 10x more efficient than "write everything → render → debug".

---

## 1. Canvas Specs — 9:16 Vertical Video

### Resolution & Framerate
- **Width:** 1080px
- **Height:** 1920px
- **Framerate:** 30fps
- **Pixel aspect ratio:** square (1:1)

### Platform Safe Zones

Elements that must avoid UI overlays (like/dislike buttons, captions, profile pictures):

| Platform | Top safe margin | Bottom safe margin |
|----------|----------------|-------------------|
| TikTok   | 240px          | 300px             |
| Reels    | 200px          | 280px             |
| Shorts   | 120px          | 160px             |

**Rule of thumb:** Keep critical text/content in the center 1080×1320 area (y: 300 to 1620).

```tsx
// Safe zone helpers
const TOP_SAFE = 300;     // max(240, 200, 120) + padding
const BOTTOM_SAFE = 1920 - 300; // 1920 - max(300, 280, 160) - padding
const CENTER_X = 540;
const CENTER_Y = 960;
```

---

## 2. Remotion API Reference

### Core Components

| Import | Purpose |
|--------|---------|
| `AbsoluteFill` | Full-canvas container (1080×1920). Use as root wrapper. |
| `Sequence` | Time-scoped container. Wraps elements that appear for a duration. |
| `OffthreadVideo` | Performant video element. Always prefer over `<Video>` for GPU rendering. |
| `Video` | Standard video element. Use only when you need frame-accurate control. |
| `Audio` | Audio-only element for background music/SFX. |
| `Img` | Image element. Use for static assets, logos, backgrounds. |

### Hooks & Utilities

| Import | Purpose |
|--------|---------|
| `useCurrentFrame()` | Returns the current frame number (0-based). Call every render. |
| `useVideoConfig()` | Returns `{ width, height, fps, durationInFrames, id }`. |
| `interpolate()` | Maps input range to output range with easing. `interpolate(frame, [0, 30], [0, 1], { extrapolateRight: 'clamp' })` |
| `spring()` | Spring physics animation. `spring({ frame, fps, config: { damping: 200, stiffness: 100 } })` |
| `random(seed)` | Deterministic random number generator. `random(props.seed)` |
| `Easing` | Easing functions: `bezier()`, `circle()`, `cubic()`, `ease()`, `elastic()`, `exp()`, `linear()`, `poly()`, `quad()`, `sin()` |
| `calculateMetadata()` | Export-time metadata calculation. Used in `RemotionRoot.tsx`. |
| `continueRender(handle)` / `delayRender()` | For async operations (font loading, data fetches). |

### Sequence Props
```tsx
<Sequence
  from={30}             // Frame at which this sequence starts (0-indexed)
  durationInFrames={90} // How many frames this sequence lasts
  name="intro-text"     // Optional name for debugging
  layout="none"         // Optional: 'none' | 'absolute-fill' | 'fit'
>
  {children}
</Sequence>
```

---

## 3. Design Tokens — JS Objects in JSX

Design tokens are plain JavaScript objects imported from the motion design system. Use them directly as style values:

```tsx
import { colors, typography, spacing, layout } from '../design_system';

// Colors are hex strings: { primary: '#FF6B35', secondary: '#004E89', ... }
// Typography is a size/weight map: { h1: { fontSize: 72, fontWeight: '800' }, body: { fontSize: 36, ... } }
// Spacing is a scale: { xs: 8, sm: 16, md: 32, lg: 64, xl: 128 }
// Layout provides safe zones and positioning helpers

// Use in JSX:
<div style={{
  color: colors.primary,
  fontSize: typography.h1.fontSize,
  fontWeight: typography.h1.fontWeight,
  padding: spacing.md,
  marginTop: spacing.lg,
}}>
  Hello World
</div>

// Combine with interpolate for animated colors:
const opacity = interpolate(frame, [0, 30], [0, 1], { extrapolateRight: 'clamp' });
<div style={{ opacity, color: colors.accent }}>Animated text</div>
```

### Motion Timing Tokens (Remotion-native, in frames at 30fps)

The design system also provides timing presets — use these directly in your `Sequence` and `interpolate` calls:

```tsx
import { timing } from '../design_system';

// Speed presets (in frames)
timing.speed.micro     // 8 frames  = 0.27s — quick flash
timing.speed.fast      // 15 frames = 0.5s  — snappy entrance
timing.speed.medium    // 30 frames = 1.0s  — standard animation
timing.speed.slow      // 60 frames = 2.0s  — deliberate reveal
timing.speed.deliberate // 90 frames = 3.0s — slow build

// Stagger delays (in frames)
timing.stagger.tight     // 4 frames  between elements
timing.stagger.standard  // 8 frames  between elements
timing.stagger.relaxed   // 15 frames between elements

// Easing curves
timing.easing.in_out   // "Easing.bezier(0.42, 0, 0.58, 1)" — smooth
timing.easing.snappy   // "Easing.cubic" — sharp
timing.easing.bounce   // "Easing.elastic(1.5)" — bouncy
timing.easing.smooth   // "Easing.sin" — gentle
timing.easing.linear   // "Easing.linear" — constant

// Use timing in interpolate:
const opacity = interpolate(frame, [0, timing.speed.fast], [0, 1], {
  extrapolateRight: 'clamp',
  easing: Easing.bezier(0.42, 0, 0.58, 1),
});
```

---

## 4. Animation Patterns

### Fade In / Out
```tsx
const frame = useCurrentFrame();
const fadeIn = interpolate(frame, [0, 30], [0, 1], { extrapolateRight: 'clamp' });
const fadeOut = interpolate(frame, [60, 90], [1, 0], { extrapolateLeft: 'clamp', extrapolateRight: 'clamp' });

return (
  <AbsoluteFill style={{ opacity: fadeIn * fadeOut }}>
    <Text>Fades in over 1s, fades out over 1s</Text>
  </AbsoluteFill>
);
```

### Slide Up / Down / Left / Right
```tsx
const frame = useCurrentFrame();
const slideY = interpolate(frame, [0, 30], [100, 0], { extrapolateRight: 'clamp' });
const slideX = interpolate(frame, [0, 30], [-200, 0], { extrapolateRight: 'clamp' });

return (
  <AbsoluteFill style={{
    transform: `translateY(${slideY}px) translateX(${slideX}px)`,
  }}>
    <Text>Slides in from bottom-left</Text>
  </AbsoluteFill>
);
```

### Typewriter Text Reveal
```tsx
const frame = useCurrentFrame();
const words = ["Build", "amazing", "motion", "graphics"];
const framesPerWord = 15;

return (
  <AbsoluteFill style={{ justifyContent: 'center', alignItems: 'center' }}>
    {words.map((word, i) => {
      const wordStart = i * framesPerWord;
      const visible = frame >= wordStart;
      const opacity = interpolate(frame, [wordStart, wordStart + 10], [0, 1], {
        extrapolateRight: 'clamp',
      });
      return (
        <span key={word} style={{
          opacity,
          marginRight: 16,
          fontSize: 48,
          fontWeight: '700',
          color: '#fff',
        }}>
          {visible ? word : '\u00A0'}
        </span>
      );
    })}
  </AbsoluteFill>
);
```

### Spring Scale Entrance
```tsx
import { spring, useCurrentFrame, AbsoluteFill } from 'remotion';

const frame = useCurrentFrame();
const scale = spring({
  frame,
  fps: 30,
  config: { damping: 200, stiffness: 100, mass: 0.5 },
});

return (
  <AbsoluteFill style={{
    justifyContent: 'center',
    alignItems: 'center',
    transform: `scale(${scale})`,
  }}>
    <Text style={{ fontSize: 72 }}>Pop in!</Text>
  </AbsoluteFill>
);
```

### Staggered List Reveals
```tsx
const frame = useCurrentFrame();
const items = ["First point", "Second point", "Third point"];
const staggerDelay = 10; // frames between each item
const itemDuration = 20;

return (
  <AbsoluteFill style={{ padding: 80 }}>
    {items.map((item, i) => {
      const startFrame = i * staggerDelay;
      const opacity = interpolate(frame, [startFrame, startFrame + itemDuration], [0, 1], {
        extrapolateRight: 'clamp',
      });
      const translateY = interpolate(frame, [startFrame, startFrame + itemDuration], [40, 0], {
        extrapolateRight: 'clamp',
      });
      return (
        <div key={i} style={{
          opacity,
          transform: `translateY(${translateY}px)`,
          fontSize: 40,
          marginBottom: 32,
          color: '#fff',
        }}>
          {item}
        </div>
      );
    })}
  </AbsoluteFill>
);
```

### Crossfade Between Slides
```tsx
const frame = useCurrentFrame();
const slideDuration = 90; // 3 seconds per slide at 30fps

return (
  <>
    {/* Slide 1: visible frames 0-90, fades out 75-90 */}
    <Sequence from={0} durationInFrames={slideDuration + 15}>
      <AbsoluteFill style={{
        backgroundColor: '#1a1a2e',
        opacity: interpolate(frame, [75, 90], [1, 0], {
          extrapolateLeft: 'clamp',
          extrapolateRight: 'clamp',
        }),
      }}>
        <Text style={{ fontSize: 60, color: '#fff' }}>Slide 1</Text>
      </AbsoluteFill>
    </Sequence>

    {/* Slide 2: fades in 75-90, fully visible 90-180 */}
    <Sequence from={75} durationInFrames={slideDuration}>
      <AbsoluteFill style={{
        backgroundColor: '#16213e',
        opacity: interpolate(frame, [0, 15], [0, 1], {
          extrapolateRight: 'clamp',
        }),
      }}>
        <Text style={{ fontSize: 60, color: '#fff' }}>Slide 2</Text>
      </AbsoluteFill>
    </Sequence>
  </>
);
```

### Progress Bar Animation
```tsx
const frame = useCurrentFrame();
const progress = interpolate(frame, [0, 150], [0, 100], { extrapolateRight: 'clamp' });

return (
  <AbsoluteFill style={{ padding: 80 }}>
    <div style={{
      width: '100%',
      height: 12,
      backgroundColor: '#333',
      borderRadius: 6,
      overflow: 'hidden',
    }}>
      <div style={{
        width: `${progress}%`,
        height: '100%',
        backgroundColor: '#FF6B35',
        borderRadius: 6,
        transition: 'width 0.033s linear', // ~1 frame at 30fps
      }} />
    </div>
    <Text style={{ fontSize: 36, color: '#fff', marginTop: 16, textAlign: 'center' }}>
      {Math.round(progress)}%
    </Text>
  </AbsoluteFill>
);
```

### Background Transitions
```tsx
const frame = useCurrentFrame();

return (
  <>
    <Sequence from={0} durationInFrames={90}>
      <AbsoluteFill style={{ backgroundColor: '#0a1628' }} />
    </Sequence>
    <Sequence from={90} durationInFrames={90}>
      <AbsoluteFill style={{ backgroundColor: '#1a0a28' }} />
    </Sequence>
    <Sequence from={180} durationInFrames={90}>
      <AbsoluteFill style={{ backgroundColor: '#281a0a' }} />
    </Sequence>
  </>
);
```

---

## 5. Timing Patterns

### Sequencing Multiple Elements
Calculate frame positions before writing JSX:

```tsx
const fps = 30;
const introDuration = 2 * fps;     // 2 seconds = 60 frames
const contentDuration = 5 * fps;   // 5 seconds = 150 frames
const outroDuration = 1.5 * fps;   // 1.5 seconds = 45 frames
const totalFrames = introDuration + contentDuration + outroDuration; // 255 frames

// Timeline:
// [0-60]     Intro animation
// [60-210]   Main content
// [210-255]  Outro / fade to black
```

### Staggered Element Timing
```tsx
const fps = 30;
const staggerFrames = 6; // 200ms between each element
const fadeInFrames = 15; // 500ms fade-in per element

const elements = [
  { content: "Hook", startFrame: 0 },
  { content: "Point 1", startFrame: staggerFrames },
  { content: "Point 2", startFrame: staggerFrames * 2 },
  { content: "CTA", startFrame: staggerFrames * 3 },
];
```

---

## 6. Text Rendering for Mobile

### Minimum Font Sizes (on 1080×1920 canvas)
| Usage | Minimum | Recommended |
|-------|---------|-------------|
| Body text | 28px | 36px |
| Sub-heading | 36px | 48px |
| Heading / Title | 48px | 72px |
| Hero / Hook | 72px | 96px+ |

### Readability Rules
- Use `fontWeight: '700'` or higher for headings
- Always use high-contrast text on backgrounds (white on dark, dark on light)
- Add `textShadow` for text over video backgrounds
- Center-align for hooks and CTAs; left-align for body copy
- Max 2 lines for hook text, 3 lines for body

```tsx
// Good: readable text over video
<Text style={{
  fontSize: 48,
  fontWeight: '800',
  color: '#ffffff',
  textShadow: '0 2px 8px rgba(0,0,0,0.5)',
  textAlign: 'center',
  paddingHorizontal: 40,
}}>
  This Changes Everything
</Text>
```

---

## 7. Common Mistakes

| Mistake | Caught By | Fix |
|---------|-----------|-----|
| `interpolate` without `extrapolateRight: 'clamp'` | validate | Values go out of range → elements fly off screen. Always clamp. |
| Forgetting `<Sequence>` wrapping | validate | All animated elements must be in a Sequence with proper `from`/`durationInFrames`. |
| Wrong component export name | validate | Your component must be exported as `export default function HotMotion(props)` — exact name matters. |
| Using `useState` or `useEffect` | compile_check | Remotion compositions are pure functions of frame. No React state. |
| Not accounting for safe zones | preview | Text at y:50 will be covered by TikTok UI. Use TOP_SAFE=300 minimum. |
| Asset paths not starting with `/` or `./` | validate | Remotion resolves paths relative to the remotion/ directory. Use absolute or relative paths. |
| Forgetting to import React hooks from remotion | compile_check | Import `useCurrentFrame`, `useVideoConfig` from `'remotion'`, not `'react'`. |
| Using `setTimeout` or async code | compile_check | Remotion renders are synchronous per frame. Use `delayRender()` for async setup only. |
| TypeScript type errors (wrong prop types) | compile_check | Check the `line` and `column` in the error. Fix the type mismatch. |
| Missing module imports | compile_check | The error will show which module can't be resolved. Add the import. |
| Animation too fast to perceive | preview | Increase frame ranges on interpolate. At 30fps, 15 frames = 0.5s. |
| Text unreadable (size/color/contrast) | preview | Use type_scale tokens. Ensure high contrast with background. |
| Elements positioned off-screen | preview | Check transform values against safe zones (y: 300-1620). |

---

## 8. Export Workflow

You have 7 motion tools available. Use them in this order:

| Tool | Purpose | Speed | When to Use |
|------|---------|-------|-------------|
| `motion.get_info` | Query project capabilities (fonts, compositions, versions) | Instant | First — know what's available |
| `motion.load_skill` | Load this design guide | Instant | First — learn the rules |
| `motion.design_system` | Generate design tokens from a brand color | Instant | Before writing composition |
| `motion.validate` | Fast heuristic TSX check (imports, exports, JSX) | <1s | Quick sanity check |
| `motion.compile_check` | TypeScript compilation check (type errors, missing imports) | 2-5s | Authoritative gate before preview |
| `motion.preview` | Render single frame as PNG for visual verification | 2-5s | Check layout, text, colors at keyframes |
| `motion.render` | Full MP4 video render (1080×1920, H.264) | 30-120s | Final output — only when confident |

### motion.render Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `tsx_code` | string | required | The full TSX source code for the composition |
| `output_path` | string \| null | `artifacts/motion_<timestamp>.mp4` | Where to save the rendered video |
| `duration_in_frames` | number | 900 (30s @ 30fps) | Total frames to render |
| `fps` | number | 30 | Frames per second |

### motion.preview Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `tsx_code` | string | required | The full TSX source code |
| `frame_number` | number | required | Which frame to render (0-based). Preview different frames to verify animation keyframes. |
| `output_path` | string \| null | optional | Output PNG path (auto-generated if omitted) |

### motion.compile_check Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `tsx_code` | string | required | The full TSX source code to type-check |

Returns structured errors with `file`, `line`, `column`, `message`, and `code` — much more actionable than raw stderr.

### Output Format
- **Format:** MP4 (H.264)
- **Resolution:** 1080×1920
- **Codec:** libx264
- **Audio:** AAC (if Audio elements present)

---

## 9. Complete Example — 5-Second Intro Graphic

```tsx
import React from 'react';
import {
  AbsoluteFill,
  Sequence,
  useCurrentFrame,
  useVideoConfig,
  interpolate,
  spring,
  Easing,
} from 'remotion';

export default function HotMotion(props: any) {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();

  // Background fade in
  const bgOpacity = interpolate(frame, [0, 15], [0, 1], { extrapolateRight: 'clamp' });

  // Spring scale for main text
  const scale = spring({
    frame: frame - 10,
    fps,
    config: { damping: 200, stiffness: 100 },
  });

  // Slide up for subtitle
  const subtitleY = interpolate(frame, [20, 50], [60, 0], { extrapolateRight: 'clamp' });
  const subtitleOpacity = interpolate(frame, [20, 40], [0, 1], { extrapolateRight: 'clamp' });

  // Progress bar
  const progress = interpolate(frame, [0, 150], [0, 100], { extrapolateRight: 'clamp' });

  return (
    <AbsoluteFill style={{ backgroundColor: '#0a1628', overflow: 'hidden' }}>
      {/* Animated background */}
      <Sequence from={0} durationInFrames={150}>
        <AbsoluteFill
          style={{
            opacity: bgOpacity,
            backgroundColor: '#16213e',
          }}
        />
      </Sequence>

      {/* Decorative line */}
      <Sequence from={10} durationInFrames={140}>
        <AbsoluteFill
          style={{
            justifyContent: 'center',
            alignItems: 'center',
            opacity: interpolate(frame, [10, 25], [0, 1], { extrapolateRight: 'clamp' }),
          }}
        >
          <div
            style={{
              width: 120,
              height: 4,
              backgroundColor: '#FF6B35',
              borderRadius: 2,
              marginBottom: 40,
            }}
          />
        </AbsoluteFill>
      </Sequence>

      {/* Main heading with spring entrance */}
      <Sequence from={10} durationInFrames={140}>
        <AbsoluteFill
          style={{
            justifyContent: 'center',
            alignItems: 'center',
            transform: `scale(${scale})`,
          }}
        >
          <div style={{
            fontSize: 80,
            fontWeight: '900',
            color: '#ffffff',
            textAlign: 'center',
            paddingHorizontal: 60,
            textShadow: '0 4px 12px rgba(0,0,0,0.3)',
          }}>
            OPEN SCRIPT
          </div>
        </AbsoluteFill>
      </Sequence>

      {/* Subtitle sliding up */}
      <Sequence from={20} durationInFrames={130}>
        <AbsoluteFill
          style={{
            justifyContent: 'center',
            alignItems: 'center',
            transform: `translateY(${subtitleY}px)`,
            opacity: subtitleOpacity,
          }}
        >
          <div style={{
            fontSize: 36,
            fontWeight: '400',
            color: '#a0aec0',
            marginTop: 160,
            textAlign: 'center',
          }}>
            AI-Directed Video Editing
          </div>
        </AbsoluteFill>
      </Sequence>

      {/* Progress bar at bottom */}
      <Sequence from={0} durationInFrames={150}>
        <AbsoluteFill style={{ justifyContent: 'flex-end', padding: 80 }}>
          <div style={{
            width: '80%',
            alignSelf: 'center',
            height: 6,
            backgroundColor: 'rgba(255,255,255,0.1)',
            borderRadius: 3,
            overflow: 'hidden',
          }}>
            <div style={{
              width: `${progress}%`,
              height: '100%',
              backgroundColor: '#FF6B35',
              borderRadius: 3,
            }} />
          </div>
        </AbsoluteFill>
      </Sequence>
    </AbsoluteFill>
  );
}
```

This composition:
1. Fades in a dark blue background (frames 0-15)
2. Springs in "OPEN SCRIPT" heading (frames 10-30)
3. Slides up a subtitle (frames 20-50)
4. Animates a progress bar across the full 5 seconds (frames 0-150)
5. Total: 150 frames = 5 seconds at 30fps

---

## Quick Checklist Before Submitting TSX

- [ ] Called `motion.get_info` to know available fonts and capabilities
- [ ] Called `motion.design_system` to get brand tokens + timing presets
- [ ] `export default function HotMotion(props)` — correct export
- [ ] Imports `useCurrentFrame` and `useVideoConfig` from `'remotion'`
- [ ] Uses `AbsoluteFill` as the root container
- [ ] All animated elements wrapped in `<Sequence>` with `from` and `durationInFrames`
- [ ] All `interpolate` calls include `extrapolateRight: 'clamp'`
- [ ] `motion.compile_check` passes with 0 errors
- [ ] `motion.preview` at key frames shows correct layout and text
- [ ] Text sizes are readable on mobile (min 28px body, min 48px heading)
- [ ] Content respects safe zones (y: 300-1620 for critical elements)
- [ ] No useState, useEffect, or other stateful React hooks
- [ ] Total frames match expected duration (frames = seconds × 30)
"#
}
