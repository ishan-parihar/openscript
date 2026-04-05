# OpenScript

**AI-directed video editing pipeline — from raw footage to polished short-form content.**

[![Rust](https://img.shields.io/badge/Rust-1.80+-orange.svg)](https://www.rust-lang.org/)
[![Python](https://img.shields.io/badge/Python-3.12+-blue.svg)](https://www.python.org/)
[![TypeScript](https://img.shields.io/badge/TypeScript-5.0+-3178C6.svg)](https://www.typescriptlang.org/)
[![MCP](https://img.shields.io/badge/MCP-Server-5B8DEF.svg)](https://modelcontextprotocol.io/)
[![License](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE)

---

## What It Does

OpenScript turns raw video into edited short-form content through a structured pipeline: **transcribe → analyze → edit decision list → multi-track timeline → render**. It exposes 41 MCP tools so AI coding agents can direct the entire editing workflow — selecting b-roll, composing voiceovers, mixing music with ducking, and rendering final video — without human intervention.

## Key Features

- **Multi-Track Timeline (EDL v2)** — Six independent tracks: dialogue, voiceover, captions, b-roll, music, SFX. Full validation and backward compatibility with EDL v1.
- **Transcription Pipeline** — Whisper-based speech-to-text with language detection, phrase-level grouping, and human-editable SRT workflow.
- **TTS Voiceover Engine** — Voice profile registry with `faster-qwen3-tts` integration, caching, and duration estimation.
- **Asset Libraries** — 261 indexed SFX and 16 music tracks with editorial-role and mood-based search. Pexels API integration for stock b-roll.
- **FFmpeg Rendering** — Multi-track audio mixing, automatic ducking, ASS caption burning, preview/standard/quality modes.
- **Remotion Composition** — TypeScript-based renderer for rich visual compositions with crossfade transitions and layered animations.
- **MCP Server (41 Tools)** — Model Context Protocol server for AI agent control across the entire pipeline.
- **Terminal TUI** — `ratatui`-based interactive interface for timeline browsing and editing.

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        MCP Server (41 tools)                     │
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
| Transcription | Whisper / Apex | Speech-to-text (Hinglish-optimized) |
| TTS | faster-qwen3-tts | Voiceover generation |
| Rendering | FFmpeg + Remotion | Video composition and output |
| Assets | JSON-indexed libraries | SFX (261), Music (16), Pexels b-roll |
| AI Interface | MCP Server (Rust) | 41 tools for agent-directed editing |
| TUI | ratatui + crossterm | Terminal-based timeline editor |
| Scripts | Python | Pipeline orchestration and helpers |

## Quick Start

### Prerequisites

- Rust 1.80+
- Python 3.12+
- FFmpeg installed
- Node.js 18+ (for Remotion rendering, optional)

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

## MCP Tools (41)

| Category | Tools |
|----------|-------|
| **Core Pipeline** | `transcribe`, `srt.read`, `srt.prepare`, `srt.apply_edit`, `edl.build`, `render`, `reelize`, `overlay.generate` |
| **Timeline Management** | `timeline.build`, `timeline.load`, `timeline.validate`, `timeline.upgrade`, `timeline.add_segment`, `timeline.add_track_event`, `timeline.diff`, `timeline.preview`, `timeline.render` |
| **Voice / TTS** | `voice.profile.add`, `voice.profile.list`, `voice.profile.remove`, `tts.generate`, `tts.estimate_duration`, `tts.preview`, `tts.commentary` |
| **SFX Library** | `sfx.index`, `sfx.search`, `sfx.assign` |
| **Music Library** | `music.index`, `music.search`, `music.assign`, `music.ducking.plan` |
| **B-Roll** | `broll.suggest`, `broll.fetch`, `broll.assign`, `broll.director`, `timeline.autofill_broll` |
| **Voiceover** | `voiceover.generate` |
| **Orchestration** | `reelize.timeline` |
| **Verification** | `verify.audio`, `verify.captions`, `verify.render` |

## Project Structure

```
openscript/
├── crates/
│   ├── openscript-core/      # Timeline schema, SRT parsing, core types
│   ├── openscript-mcp/       # MCP server with 41 tools
│   ├── openscript-ffmpeg/    # FFmpeg filter graphs, rendering, subtitles
│   ├── openscript-transcribe/# Whisper/Apex transcription
│   ├── openscript-tts/       # TTS client, voice profiles
│   ├── openscript-assets/    # SFX/music indexing, Pexels b-roll
│   ├── openscript-ui/        # ratatui TUI (app + rendering)
│   └── openscript-cli/       # CLI entry point
├── mcp/
│   ├── scripts/              # Python pipeline helpers
│   └── assets/               # Indexed SFX, music, voice configs
├── remotion/
│   └── src/                  # TypeScript composition engine
├── third_party/              # faster-qwen3-tts (TTS sidecar)
└── LICENSE                   # MIT License
```

## Code Metrics

| Component | Lines of Code |
|-----------|--------------|
| Rust (8 crates) | ~12,700 |
| Python (project) | ~1,060 |
| TypeScript (Remotion) | ~760 |
| **Total (project)** | **~14,500** |

## Testing

```bash
# Run the full test suite
./RUN_TESTS.sh

# Or individual test targets
cargo test --workspace
python3 mcp/test_implementation.py
python3 mcp/test_e2e_workflow.py
```

Test coverage includes unit tests for core types, integration tests for the MCP server, E2E pipeline validation, and asset library verification.

## Development Status

Active development. Core timeline system, MCP server, FFmpeg rendering, and TTS pipeline are production-ready. Remotion composition and TUI are functional with ongoing refinement.

| Component | Status |
|-----------|--------|
| Core timeline (EDL v2) | Production |
| MCP server (41 tools) | Production |
| Transcription | Production |
| FFmpeg rendering | Production |
| TTS voiceover | Production |
| Asset libraries | Production |
| Remotion composition | Beta |
| Terminal TUI | Beta |

---

## Why OpenScript

Traditional video editing requires manual timeline work in Premiere, DaVinci, or Final Cut. OpenScript flips this: **you direct, the AI edits**. Feed it raw footage, tell it what kind of reel you want, and it handles transcription, timing, b-roll selection, audio mixing, caption burning, and rendering — all through a structured, type-safe pipeline.

Built as a demonstration of what's possible when AI agents have **real tools** instead of just text interfaces.

---

Built by [Ishan Parihar](https://github.com/ishan-parihar)
