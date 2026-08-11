# HyperFrames Composition Directory

This directory holds HyperFrames (HTML + GSAP) video compositions for the
OpenScript pipeline. It is the **default authoring/rendering surface** for
agent-created video; the sibling `../remotion/` directory holds the legacy
Remotion (React) compositions and the PR #214 interop escape hatch.

## Relationship to `remotion/`

| Path | Engine | Role |
|------|--------|------|
| `hyperframes/` | HyperFrames (HTML + GSAP) | **Default** — agent-authored compositions, lint/validate/snapshot/render via `npx hyperframes` |
| `../remotion/` | Remotion (React) | Legacy compositions + escape hatch for stateful components via PR #214 interop |

The MCP layer routes via a classifier (see `composition.render` tool):
compositions that need `useState`/`useEffect`/3rd-party React UI → Remotion
interop; everything else → HyperFrames (default).

## Prerequisites

- Node.js >= 22
- FFmpeg (already required by OpenScript)
- `npx hyperframes` (auto-installed on first run, or run `npm install -g hyperframes`)

## Workflow

```bash
# Scaffold a new composition
npx hyperframes init my-video
cd my-video

# Author the HTML composition (see skills/hyperframes/SKILL.md)

# Lint — catches missing data-composition-id, overlapping tracks, unregistered timelines
npx hyperframes lint --json

# Validate — loads in headless Chrome, reports runtime errors + WCAG contrast
npx hyperframes validate --json

# Snapshot — visual smoke test, captures frames at given timestamps
npx hyperframes snapshot --frames 9

# Render — produce the final MP4
npx hyperframes render --quality high --output out.mp4
```

## MCP Tools

The following MCP tools wrap the CLI for agent use:

| Tool | Wraps | Purpose |
|------|-------|---------|
| `hf.lint` | `npx hyperframes lint --json` | Static checks |
| `hf.validate` | `npx hyperframes validate --json` | Runtime checks in headless Chrome |
| `hf.snapshot` | `npx hyperframes snapshot` | Visual smoke test |
| `hf.render` | `npx hyperframes render` | Produce MP4 |

All tools accept a `project_dir` argument (defaults to `./hyperframes`).