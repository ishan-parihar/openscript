#!/usr/bin/env bash
# Generate the b-roll coverage-gap regression fixture.
#
# The fixture proves the Phase A + Phase B behavior of the segmentation
# upgrade (docs/SEGMENTATION_UPGRADE_PLAN.md):
#   1. The renderer plays a short clip exactly ONCE (never loops to fill).
#   2. timeline.validate / verify.production report the uncovered tail as a
#      `broll_gaps` entry with the segment id, required/available durations,
#      and a directive to re-run keyword generation for a longer clip.
#
# Outputs:
#   test_fixtures/broll_gap_clip.mp4          — 2s synthetic clip (testsrc)
#   test_fixtures/broll_gap.timeline.json     — 1 segment (4s) assigned that clip
#
# Usage: bash scripts/make_broll_gap_fixture.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="$ROOT/test_fixtures"
CLIP="$OUT_DIR/broll_gap_clip.mp4"
TIMELINE="$OUT_DIR/broll_gap.timeline.json"

echo "Generating 2s synthetic clip (video + silent audio): $CLIP"
ffmpeg -y -loglevel error \
  -f lavfi -i "testsrc=size=1080x1920:rate=30:duration=2" \
  -f lavfi -i "anullsrc=r=44100:cl=stereo" \
  -t 2 \
  -c:v libx264 -preset veryfast -pix_fmt yuv420p \
  -c:a aac -shortest \
  "$CLIP"

echo "Writing timeline: $TIMELINE"
cat > "$TIMELINE" <<'EOF'
{
  "version": "2.0",
  "created_at": "2026-08-04T00:00:00Z",
  "updated_at": "2026-08-04T00:00:00Z",
  "source": "test_fixtures/broll_gap_clip.mp4",
  "raw_render": false,
  "target": { "fps": 30, "aspect": "9:16", "max_duration": null, "width": null, "height": null },
  "effects": { "burn_captions": true, "audio": { "loudnorm": false }, "caption_style": null },
  "directives": {
    "ducking": [],
    "transitions": [],
    "mix": { "master_gain_db": 0.0, "limiter_threshold_db": -1.0, "normalize_to_lufs": -16.0 },
    "render_backend": "auto"
  },
  "segments": [
    { "id": "seg_001", "start": 0.0, "end": 4.0, "caption": "gap test segment", "crossfade_ms": 80, "semantic_role": "hook" }
  ],
  "tracks": {
    "dialogue": [],
    "voiceover": [],
    "broll": [
      {
        "id": "broll_001",
        "asset_id": "broll_0",
        "start_ms": 0,
        "end_ms": 4000,
        "offset_ms": 0,
        "gain_db": 0.0,
        "fade_in_ms": 0,
        "fade_out_ms": 0,
        "tags": ["gap test"],
        "provenance": { "tool": "broll.fetch", "concept": "gap test" },
        "event_type": "broll",
        "concept": "gap test",
        "source_provider": "fallback_pool",
        "transition_style": "cut",
        "crop_mode": "center",
        "orientation": "9:16",
        "motion_intensity": "medium"
      }
    ],
    "music": [],
    "sfx": [],
    "captions": []
  },
  "assets": {
    "broll": {
      "broll_0": {
        "path": "test_fixtures/broll_gap_clip.mp4",
        "concept": "gap test",
        "source_duration_s": 2.0
      }
    },
    "music": {},
    "sfx": {},
    "voices": {},
    "captions": {}
  }
}
EOF

echo "Fixture ready. Expect: segment needs 4.0s, clip provides 2.0s → gap 2.0s."
echo "Verify with: ./target/debug/openscript timeline-validate --timeline-path test_fixtures/broll_gap.timeline.json"
