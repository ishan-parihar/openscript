# OpenScript

**AI-directed video editing pipeline — from raw footage to polished short-form content.**

[![Rust](https://img.shields.io/badge/Rust-1.80+-orange.svg)](https://www.rust-lang.org/)
[![Python](https://img.shields.io/badge/Python-3.12+-blue.svg)](https://www.python.org/)
[![TypeScript](https://img.shields.io/badge/TypeScript-5.0+-3178C6.svg)](https://www.typescriptlang.org/)
[![MCP](https://img.shields.io/badge/MCP-Server-5B8DEF.svg)](https://modelcontextprotocol.io/)
[![License](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE)

---

## What It Does

OpenScript turns raw video into edited short-form content through a structured pipeline: **transcribe → analyze → edit decision list → multi-track timeline → render**. It exposes **43 MCP tools** so AI coding agents can direct the entire editing workflow — selecting b-roll, composing voiceovers, mixing music with ducking, and rendering final video — without human intervention.

Two editing modes:
- **`reelize.timeline`** — One-call pipeline: raw video → complete 9:16 reel with captions, b-roll, music, and SFX
- **`reelize.brief` → `reelize.direct`** — Two-step workflow: AI analyzes footage and returns a structured brief, then executes creative direction with full control over every track

## Key Features

- **Multi-Track Timeline (EDL v2)** — Six independent tracks: dialogue, voiceover, captions, b-roll, music, SFX. Full validation and backward compatibility with EDL v1.
- **Transcription Pipeline** — Apex (Oriserve/Whisper-Hindi2Hinglish-Apex) speech-to-text with word-level timestamps, phrase-level grouping, and human-editable SRT workflow. Optimized for Hinglish content.
- **TTS Voiceover Engine** — Voice profile registry with `faster-qwen3-tts` integration, caching, duration estimation, and multi-speaker commentary generation.
- **Asset Libraries** — 261 indexed SFX and 16 music tracks with editorial-role and mood-based search. Pexels API integration for stock b-roll with director-mode concept extraction.
- **FFmpeg Rendering** — Multi-track audio mixing, automatic ducking, ASS caption burning with Bebas Neue font, preview/standard/quality modes.
- **Remotion Composition** — TypeScript-based renderer for rich visual compositions with crossfade transitions and layered animations.
- **MCP Server (43 Tools)** — Rust-native Model Context Protocol server with stdio transport, progress notifications, and type-safe tool schemas.
- **Terminal TUI** — `ratatui`-based interactive interface for timeline browsing and editing.
- **Verification Layer** — Post-render quality checks for audio levels, caption synchronization, and render fidelity.

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        MCP Server (43 tools)                     │
│  AI Agents ─────────────────────────────────────────────────────│
└──────┬──────────┬──────────┬──────────┬──────────┬──────────────┘
       │          │          │          │          │
  ┌────▼───┐ ┌───▼────┐ ┌──▼─────┐ ┌──▼──────┐ ┌▼──────────┐
  │ Core   │ │Trans-  │ │ FFmpeg │ │  TTS    │ │  Assets   │
  │Timeline│ │ cribe  │ │ Render │ │ Engine  │ │  (SFX/    │
  │  & SRT │ │        │ │        │ │         │ │  Music)   │
  └────┬───┘ └───┬────┘ └──┬─────┘ └──┬──────┘ └┬──────────┘
       │          │          │          │          │
       └──────────┴──────────┴──────────┴──────────┘
                              │
                    ┌─────────▼──────────┐
                    │   Remotion (TS)    │
                    │   Composition +    │
                    │   Animation Engine │
                    └─────────┬──────────┘
                              │
                         ┌────▼────┐
                         │  MP4    │
                         │  Output │
                         └─────────┘
```

## Tech Stack

| Layer | Technology | Purpose |
|-------|-----------|---------|
| Core | Rust (8 crates) | Timeline schema, SRT parsing, type-safe EDL |
| Transcription | Apex (Whisper-Hindi2Hinglish) | Speech-to-text (Hinglish-optimized) |
| TTS | faster-qwen3-tts | Voiceover generation |
| Rendering | FFmpeg + Remotion | Video composition and output |
| Assets | JSON-indexed libraries | SFX (261), Music (16), Pexels b-roll |
| AI Interface | MCP Server (Rust) | 43 tools for agent-directed editing |
| TUI | ratatui + crossterm | Terminal-based timeline editor |
| Scripts | Python | Pipeline orchestration and helpers |

## Quick Start

### Prerequisites

- Rust 1.80+
- Python 3.12+
- FFmpeg installed
- Node.js 20+ (for Remotion rendering and Tauri frontend, optional)

### Build

```bash
cargo build --release
```

### MCP Server

```bash
# Start the MCP server (Rust implementation, stdio transport)
cargo run -p openscript-mcp --bin mcp-server
```

### Run Full Pipeline

```bash
# One-command: raw video → 9:16 reel with captions, b-roll, music, SFX
cargo run -p openscript-cli -- reelize input.mp4
```

## MCP Tools (43)

| Category | Tools |
|----------|-------|
| **Core Pipeline** | `transcribe`, `srt.read`, `srt.prepare`, `srt.apply_edit`, `edl.build`, `render`, `reelize`, `overlay.generate` |
| **AI Director** | `reelize.brief`, `reelize.direct` — Analyze footage, then execute creative direction |
| **Timeline Management** | `timeline.build`, `timeline.load`, `timeline.validate`, `timeline.upgrade`, `timeline.add_segment`, `timeline.add_track_event`, `timeline.diff`, `timeline.preview`, `timeline.render` |
| **Voice / TTS** | `voice.profile.add`, `voice.profile.list`, `voice.profile.remove`, `tts.generate`, `tts.estimate_duration`, `tts.preview`, `tts.commentary` |
| **SFX Library** | `sfx.index`, `sfx.search`, `sfx.assign` |
| **Music Library** | `music.index`, `music.search`, `music.assign`, `music.ducking.plan` |
| **B-Roll** | `broll.suggest`, `broll.fetch`, `broll.assign`, `broll.director`, `timeline.autofill_broll` |
| **Voiceover** | `voiceover.generate` |
| **Orchestration** | `reelize.timeline` — Single-call end-to-end pipeline |
| **Verification** | `verify.audio`, `verify.captions`, `verify.render` — Post-render QA |

## Workflows

### One-Call Pipeline

```
reelize.timeline(video_path)
  ├── Transcribe (Apex)
  ├── Build timeline with segments
  ├── B-roll director (Pexels)
  ├── Assign background music + ducking
  ├── Assign SFX (hook, transitions, highlights)
  ├── Generate captions (Bebas Neue, centered)
  └── Render final video
```

### AI Director Mode

```
reelize.brief(video_path)
  └── Returns: segments, timing, word counts, topic clusters, b-roll concepts

# AI agent reviews brief, makes creative decisions

reelize.direct(video_path, segments, broll, sfx, music, voiceover, captions)
  └── Returns: rendered reel with full creative control
```

## Project Structure

```
openscript/
├── crates/
│   ├── openscript-core/      # Timeline schema, SRT parsing, core types
│   ├── openscript-mcp/       # MCP server (43 tools, stdio transport)
│   │   └── src/bin/
│   │       ├── mcp-server.rs # MCP server binary
│   │       ├── audit_tools.rs
│   │       └── generate_reel.rs
│   ├── openscript-ffmpeg/    # FFmpeg filter graphs, rendering, subtitles
│   ├── openscript-transcribe/# Whisper/Apex transcription
│   ├── openscript-tts/       # TTS client, voice profiles
│   ├── openscript-assets/    # SFX/music indexing, Pexels b-roll
│   ├── openscript-ui/        # ratatui TUI (app + rendering)
│   └── openscript-cli/       # CLI entry point
├── mcp/
│   ├── scripts/              # Python pipeline helpers
│   ├── assets/               # Indexed SFX, music, voice configs
│   ├── fonts/                # Bebas Neue for caption burning
│   └── styles/               # PupCaps CSS presets
├── remotion/
│   └── src/                  # TypeScript composition engine
├── third_party/              # faster-qwen3-tts (TTS sidecar)
└── LICENSE                   # MIT License
```

## Code Metrics

| Component | Lines of Code |
|-----------|--------------|
| Rust (8 crates) | ~16,000 |
| Python (project) | ~1,060 |
| TypeScript (Remotion) | ~760 |
| **Total (project)** | **~17,800** |

## Testing

```bash
# Run the full test suite
./RUN_TESTS.sh

# Or individual test targets
cargo test --workspace
```

Test coverage includes unit tests for core types, integration tests for the MCP server, E2E pipeline validation, and asset library verification.

## Development Status

Core timeline system, MCP server, FFmpeg rendering, TTS pipeline, and asset libraries are production-ready. Remotion composition and TUI are functional with ongoing refinement.

| Component | Status |
|-----------|--------|
| Core timeline (EDL v2) | Production |
| MCP server (43 tools) | Production |
| Transcription (Apex) | Production |
| FFmpeg rendering | Production |
| TTS voiceover | Production |
| Asset libraries | Production |
| Verification layer | Production |
| Remotion composition | Beta |
| Terminal TUI | Beta |

---

## Why OpenScript

Traditional video editing requires manual timeline work in Premiere, DaVinci, or Final Cut. OpenScript flips this: **you direct, the AI edits**. Feed it raw footage, tell it what kind of reel you want, and it handles transcription, timing, b-roll selection, audio mixing, caption burning, and rendering — all through a structured, type-safe pipeline.

Built as a demonstration of what's possible when AI agents have **real tools** instead of just text interfaces.

---

Built by [Ishan Parihar](https://github.com/ishan-parihar)

If you find this project useful, [consider supporting its development](https://rzp.io/rzp/ishan-parihar) ☕
