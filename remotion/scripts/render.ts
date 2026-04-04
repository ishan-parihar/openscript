#!/usr/bin/env tsx
/**
 * Remotion render script
 * Usage: tsx scripts/render.ts --timeline path/to/timeline.json --out output.mp4
 */

import { bundle } from '@remotion/cli';
import { renderMedia, selectComposition } from 'remotion';
import { fileURLToPath } from 'url';
import { dirname, join } from 'path';
import fs from 'fs';
import { validateTimeline } from '../src/lib/track';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const rootDir = join(__dirname, '..');

async function main() {
  const args = process.argv.slice(2);
  const timelinePath = args.find((_, i) => args[i - 1] === '--timeline');
  const outputPath = args.find((_, i) => args[i - 1] === '--out') || 'output.mp4';
  const codec = args.find((_, i) => args[i - 1] === '--codec') || 'h264';

  if (!timelinePath) {
    console.error('Usage: tsx scripts/render.ts --timeline <path> --out <output.mp4>');
    process.exit(1);
  }

  // Load and validate timeline
  const timelineData = JSON.parse(fs.readFileSync(timelinePath, 'utf-8'));
  const timeline = validateTimeline(timelineData);

  console.log(`[remotion] Timeline loaded: ${timeline.meta.durationMs}ms @ ${timeline.meta.fps}fps`);
  console.log(`[remotion] B-roll clips: ${timeline.sources.brolls.length}`);
  console.log(`[remotion] Track events: ${timeline.track.length}`);

  // Bundle Remotion
  const bundled = await bundle(rootDir, {
    logLevel: 'info',
  });

  // Calculate duration in frames
  const durationInFrames = Math.round(
    (timeline.meta.durationMs * timeline.meta.fps) / 1000
  );

  console.log(`[remotion] Rendering ${durationInFrames} frames...`);

  // Render
  const composition = await selectComposition({
    serveUrl: bundled,
    id: 'MainWithBroll',
    inputProps: { timeline },
  });

  await renderMedia({
    composition,
    serveUrl: bundled,
    codec,
    outputLocation: outputPath,
    inputProps: { timeline },
    framesPerSecond: timeline.meta.fps,
    width: timeline.meta.width,
    height: timeline.meta.height,
    numberOfGifLoops: 0,
    onProgress: (p) => {
      if (p.frame % 30 === 0) {
        console.log(`[remotion] Frame ${p.frame}/${durationInFrames}`);
      }
    },
  });

  console.log(`[remotion] Output: ${outputPath}`);
}

main().catch((err) => {
  console.error('[remotion] Error:', err);
  process.exit(1);
});
