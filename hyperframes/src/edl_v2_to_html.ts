#!/usr/bin/env tsx
/**
 * EDL v2 → HyperFrames HTML compiler.
 *
 * Compiles an OpenScript EDL v2 timeline JSON into a HyperFrames HTML
 * composition. This is the HyperFrames equivalent of
 * remotion/src/lib/edl_v2_compiler.tsx — it replaces the Remotion React
 * component tree with HTML + a single GSAP timeline.
 *
 * Usage:
 *   tsx edl_v2_to_html.ts --timeline path/to/timeline.json --out path/to/index.html
 *
 * The emitted HTML uses the HyperFrames data-* contract:
 *   - Root <html> carries data-composition-id, data-start, data-duration,
 *     data-fps, data-width, data-height.
 *   - Each <video> / <div> scene carries data-start, data-duration,
 *     data-track-index.
 *   - A single <script> at the bottom registers one paused gsap.timeline()
 *     on window.__timelines[<composition-id>].
 *
 * Translation mapping (Remotion → HyperFrames):
 *   <AbsoluteFill>              → <div id="stage">
 *   <Sequence from durationInFrames> → data-start / data-duration (in seconds)
 *   useCurrentFrame() + interpolate() → gsap.timeline().to(..., delay, duration)
 *   <Video src> muted           → <video src muted>
 *   <Audio src>                  → <audio src> (or omitted — FFmpeg mixes audio)
 *   crossfade transitionIn/Out   → gsap opacity tween (duration = frames / fps)
 */

import fs from "fs";
import path from "path";

// ---------------------------------------------------------------------------
// Types — mirror remotion/src/lib/track.ts (simplified TS timeline)
// ---------------------------------------------------------------------------

interface Transition {
  in: number; // frames
  out: number; // frames
}

interface BrollClip {
  id: string;
  src: string;
  durationMs?: number;
  width?: number;
  height?: number;
}

type TimelineEvent =
  | { type: "video"; role: "main"; startMs: number; endMs: number }
  | { type: "broll"; id: string; startMs: number; endMs: number; transition?: Transition };

interface TimelineMeta {
  fps: number;
  width: number;
  height: number;
  durationMs: number;
}

interface Timeline {
  meta: TimelineMeta;
  sources: {
    main: string;
    brolls: BrollClip[];
  };
  track: TimelineEvent[];
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const msToSeconds = (ms: number): number => ms / 1000;
const framesToSeconds = (frames: number, fps: number): number => frames / fps;

function escapeAttr(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;");
}

function validateTimeline(data: unknown): Timeline {
  const t = data as Timeline;
  if (!t.meta || !t.meta.fps || !t.meta.durationMs) {
    throw new Error("Invalid timeline: missing meta.fps or meta.durationMs");
  }
  if (!t.sources || typeof t.sources.main !== "string") {
    throw new Error("Invalid timeline: missing sources.main");
  }
  if (!Array.isArray(t.track)) {
    throw new Error("Invalid timeline: track is not an array");
  }
  return t;
}

// ---------------------------------------------------------------------------
// Compiler
// ---------------------------------------------------------------------------

interface CompileOptions {
  compositionId?: string;
  outputDir?: string; // for resolving relative asset paths
}

function compileTimeline(timeline: Timeline, opts: CompileOptions = {}): string {
  const { meta, sources, track } = timeline;
  const compositionId = opts.compositionId || "main-with-broll";
  const durationSec = msToSeconds(meta.durationMs);

  // Partition events by type
  const videoEvents = track.filter((e): e is Extract<TimelineEvent, { type: "video" }> => e.type === "video");
  const brollEvents = track.filter((e): e is Extract<TimelineEvent, { type: "broll" }> => e.type === "broll");

  // B-roll ID → src map
  const brollMap = new Map<string, string>();
  sources.brolls.forEach((b) => brollMap.set(b.id, b.src));

  // --- Build the HTML ---
  const videoScenes: string[] = [];
  const brollScenes: string[] = [];
  const timelineTweens: string[] = [];

  // Main video: one element per video event segment
  videoEvents.forEach((event, i) => {
    const startSec = msToSeconds(event.startMs);
    const durSec = msToSeconds(event.endMs - event.startMs);
    videoScenes.push(
      `    <video class="video-layer" id="main-${i}"
      data-start="${startSec.toFixed(4)}"
      data-duration="${durSec.toFixed(4)}"
      data-track-index="0"
      src="${escapeAttr(sources.main)}"
      muted playsinline></video>`
    );
    // Opacity 1 during the segment, 0 outside
    timelineTweens.push(
      `  tl.set("#main-${i}", { opacity: 0 }, 0);`
    );
    timelineTweens.push(
      `  tl.to("#main-${i}", { opacity: 1, duration: 0.001, ease: "none" }, ${startSec.toFixed(4)});`
    );
    const endSec = startSec + durSec;
    timelineTweens.push(
      `  tl.to("#main-${i}", { opacity: 0, duration: 0.001, ease: "none" }, ${endSec.toFixed(4)});`
    );
  });

  // If no video events, show main video for the full duration
  if (videoEvents.length === 0) {
    videoScenes.push(
      `    <video class="video-layer" id="main-video"
      data-start="0"
      data-duration="${durationSec.toFixed(4)}"
      data-track-index="0"
      src="${escapeAttr(sources.main)}"
      muted playsinline></video>`
    );
    timelineTweens.push(`  tl.to("#main-video", { opacity: 1, duration: 0.001, ease: "none" }, 0);`);
  }

  // B-roll: one element per broll event, with crossfade in/out
  brollEvents.forEach((event, i) => {
    const src = brollMap.get(event.id);
    if (!src) return; // skip missing broll

    const startSec = msToSeconds(event.startMs);
    const durSec = msToSeconds(event.endMs - event.startMs);
    const transition = event.transition || { in: 6, out: 6 };
    const fadeInSec = framesToSeconds(transition.in, meta.fps);
    const fadeOutSec = framesToSeconds(transition.out, meta.fps);

    brollScenes.push(
      `    <video class="broll-layer" id="broll-${i}"
      data-start="${startSec.toFixed(4)}"
      data-duration="${durSec.toFixed(4)}"
      data-track-index="1"
      src="${escapeAttr(src)}"
      muted playsinline></video>`
    );

    // Crossfade: 0 → 1 over fadeInSec, hold, 1 → 0 over fadeOutSec
    timelineTweens.push(
      `  tl.to("#broll-${i}", { opacity: 1, duration: ${fadeInSec.toFixed(4)}, ease: "power2.inOut" }, ${startSec.toFixed(4)});`
    );
    const holdEnd = startSec + durSec - fadeOutSec;
    timelineTweens.push(
      `  tl.to("#broll-${i}", { opacity: 1, duration: ${(holdEnd - startSec - fadeInSec).toFixed(4)}, ease: "none" }, ${(startSec + fadeInSec).toFixed(4)});`
    );
    timelineTweens.push(
      `  tl.to("#broll-${i}", { opacity: 0, duration: ${fadeOutSec.toFixed(4)}, ease: "power2.inOut" }, ${holdEnd.toFixed(4)});`
    );
  });

  // --- Assemble the HTML ---
  const html = `<!DOCTYPE html>
<html lang="en"
  data-composition-id="${compositionId}"
  data-start="0"
  data-duration="${durationSec.toFixed(4)}"
  data-fps="${meta.fps}"
  data-width="${meta.width}"
  data-height="${meta.height}"
>
<head>
  <meta charset="utf-8" />
  <title>${compositionId}</title>
  <style>
    * { margin: 0; padding: 0; box-sizing: border-box; }
    html, body { overflow: hidden; background: #000; }
    #stage {
      position: relative;
      width: ${meta.width}px;
      height: ${meta.height}px;
      background: #000;
      overflow: hidden;
    }
    .video-layer, .broll-layer {
      position: absolute;
      inset: 0;
      width: 100%;
      height: 100%;
      object-fit: cover;
      opacity: 0;
    }
  </style>
</head>
<body>
  <div id="stage">
${videoScenes.join("\n")}
${brollScenes.join("\n")}
  </div>

  <script src="https://cdnjs.cloudflare.com/ajax/libs/gsap/3.12.5/gsap.min.js"></script>
  <script>
    const tl = gsap.timeline({ paused: true });
${timelineTweens.join("\n")}

    window.__timelines = window.__timelines || {};
    window.__timelines["${compositionId}"] = tl;
  </script>
</body>
</html>
`;

  return html;
}

// ---------------------------------------------------------------------------
// CLI entry point
// ---------------------------------------------------------------------------

async function main() {
  const args = process.argv.slice(2);
  const timelinePath = args.find((_, i) => args[i - 1] === "--timeline");
  const outputPath = args.find((_, i) => args[i - 1] === "--out") || "index.html";
  const compositionId = args.find((_, i) => args[i - 1] === "--composition-id") || "main-with-broll";

  if (!timelinePath) {
    console.error("Usage: tsx edl_v2_to_html.ts --timeline <path> --out <path> [--composition-id <id>]");
    process.exit(1);
  }

  const timelineData = JSON.parse(fs.readFileSync(timelinePath, "utf-8"));
  const timeline = validateTimeline(timelineData);

  console.log(`[hf-compiler] Timeline loaded: ${timeline.meta.durationMs}ms @ ${timeline.meta.fps}fps`);
  console.log(`[hf-compiler] B-roll clips: ${timeline.sources.brolls.length}`);
  console.log(`[hf-compiler] Track events: ${timeline.track.length}`);

  const html = compileTimeline(timeline, { compositionId });

  const outDir = path.dirname(outputPath);
  if (outDir) {
    fs.mkdirSync(outDir, { recursive: true });
  }
  fs.writeFileSync(outputPath, html);

  console.log(`[hf-compiler] Output: ${outputPath}`);
}

// Run if invoked directly
if (import.meta.url === `file://${process.argv[1]}`) {
  main().catch((err) => {
    console.error("[hf-compiler] Error:", err);
    process.exit(1);
  });
}

export { compileTimeline, validateTimeline };
