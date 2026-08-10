# OpenScript

<!-- T2I HERO SPEC — Subject: an AI film director in a control room — raw footage reels on the left, a storyboard timeline in the center (six tracks: dialogue, voiceover, captions, b-roll, music, SFX), a polished vertical 9:16 reel emerging on the right with verification checkmarks. Composition: left-to-right pipeline; director silhouette as the orchestrator. Palette: cinema dark #0f0f14 → projector cyan #22d3ee → warm stage amber #f59e0b → success green #34d399. Style: flat vector with film-grain texture, glowing cuts, no text. 16:9. -->

AI-directed video editing pipeline — raw footage to polished 9:16 reel. Transcription → creative brief → timeline → FFmpeg render → verified output. Rust + Python + TypeScript. **109 MCP tools** (verified: 103 static + 6 dynamic in `openscript-mcp/src/tools.rs`). 9 Rust crates.

[![Rust](https://img.shields.io/badge/Rust-2021-orange?logo=rust)](https://www.rust-lang.org/)
[![Python](https://img.shields.io/badge/Python-3.11+-blue?logo=python)](https://www.python.org/)
[![TypeScript](https://img.shields.io/badge/TypeScript-5-blue?logo=typescript)](https://www.typescriptlang.org/)
[![FFmpeg](https://img.shields.io/badge/FFmpeg-6+-red?logo=ffmpeg)](https://ffmpeg.org/)
[![MCP](https://img.shields.io/badge/MCP-Server-violet?logo=modelcontextprotocol)](https://modelcontextprotocol.io/)
[![Tools](https://img.shields.io/badge/tools-109-brightgreen)](https://github.com/ishan-parihar/openscript)
[![License](https://img.shields.io/badge/License-MIT-green)](LICENSE)

[![Rust](https://img.shields.io/badge/Rust-2021-orange?logo=rust)](https://www.rust-lang.org/)
[![Python](https://img.shields.io/badge/Python-3.11+-blue?logo=python)](https://www.python.org/)
[![TypeScript](https://img.shields.io/badge/TypeScript-5-blue?logo=typescript)](https://www.typescriptlang.org/)
[![FFmpeg](https://img.shields.io/badge/FFmpeg-6+-red?logo=ffmpeg)](https://ffmpeg.org/)
[![MCP](https://img.shields.io/badge/MCP-Server-violet?logo=modelcontextprotocol)](https://modelcontextprotocol.io/)
[![License](https://img.shields.io/badge/License-MIT-green)](LICENSE)

---

## What It Does

Most "AI video" tools generate content from text prompts. OpenScript takes your **real raw footage** and edits it into a professional vertical video reel. An AI agent acts as the director — selecting b-roll concepts, choosing music mood, placing SFX — while the engine handles the technical execution.

## How it compares

| Capability | **OpenScript** | Descript | Runway / Pika | Adobe Premiere AI |
|---|---|---|---|---|
| **Edits your raw footage** | ✅ real MP4/TS → polished 9:16 reel | ✅ | ❌ text-to-video | ✅ |
| **Agent as director** | ✅ 109 MCP tools, agent chooses b-roll/music/SFX/cuts | ❌ GUI-first | ❌ prompt-only | ❌ |
| **Deterministic EDL** | ✅ 6-track Edit Decision List (dialogue/vox/captions/b-roll/music/SFX) | ⚠️ | ❌ | ✅ |
| **Post-render verification** | ✅ audio levels, caption sync, render fidelity checks | ❌ | ❌ | ❌ |
| **Hinglish-optimized transcription** | ✅ Whisper w/ word-level timestamps | ⚠️ | ❌ | ⚠️ |
| **Self-hosted / MCP-native** | ✅ 9 Rust crates, runs headless, agent-driven | ❌ SaaS | ❌ SaaS | ❌ desktop |
| **Music ducking + SFX library** | ✅ 261 SFX, 16 music tracks, mood search | ✅ | ❌ | ✅ |

Descript is a *human editor in software*; OpenScript is an *agent editor with an engine* — the AI director plans, the deterministic pipeline executes, and verification gates the render.

### The Pipeline

```text
┌─────────────┐    ┌──────────────┐    ┌──────────────────┐    ┌──────────────┐    ┌─────────────┐
│  Raw        │    │  Transcription│    │  Creative Brief  │    │  Multi-Track │    │  Verified    │
│  Footage    │───▶│  (Whisper)   │───▶│  (AI Director)  │───▶│  Timeline   │───▶│  9:16 Reel │
│  (MP4/TS)   │    │  + timestamps │    │  - B-roll picks  │    │  (EDL v2)    │    │  (MP4)     │
│             │    │  Hinglish-    │    │  - Music mood   │    │  6 tracks:   │    │  ✓ Levels   │
│             │    │  optimized    │    │  - SFX timing   │    │  dial/vox/  │    │  ✓ Captions  │
│             │    │               │    │  - Voiceover    │    │  caps/b-roll/│    │  ✓ Sync     │
│             │    │               │    │  - Cut points   │    │  music/sfx   │    │             │
└─────────────┘    └──────────────┘    └──────────────────┘    └──────────────┘    └─────────────┘
```

### Key Features

| Feature | Details |
|---------|---------|
| **Apex Transcription** | Hinglish-optimized Whisper with word-level timestamps |
| **Creative Brief** | AI agent directs b-roll, music mood, SFX placement — like a human editor |
| **6-Track EDL** | Dialogue · Voice-over · Captions · B-roll · Music · SFX |
| **Voice-over Engine** | TTS with voice profile registry and duration estimation |
| **Sound Library** | 261 indexed SFX + 16 music tracks with mood/role-based search |
| **FFmpeg Rendering** | Automatic audio ducking, caption burn-in, quality validation |
| **Post-render Verification** | Audio level checks, caption sync verification, render fidelity audit |

## Quick Start

```bash
# Clone and install
git clone https://github.com/ishan-parihar/openscript.git
cd openscript
pip install -e ".[dev]"

# Register the MCP server
mcp install src/openscript_mcp/server.py

# Create a project from raw footage
openscript new my_video --footage /path/to/raw_footage/
openscript brief my_video --prompt "Short LinkedIn tip about Rust async patterns"
openscript render my_video --preset mobile-9x16
openscript verify my_video  # post-render validation
```

## MCP Tools (109 tools — verified from `tools.rs`: 103 static + 6 dynamic `hf.*`)

```
openscript.list_projects     ━▶ List all video projects
openscript.create_project    ━▶ Initialize new project from raw footage
openscript.ingest_footage    ━▶ Transcribe + index raw clips
openscript.generate_brief    ━▶ AI creative brief for b-roll/music/SFX
openscript.build_timeline    ━▶ Compile EDL from brief + footage
openscript.render            ━▶ FFmpeg render to MP4
openscript.verify            ━▶ Post-render audit (levels, sync, quality)
openscript.search_clips      ━▶ Semantic search across transcribed clips
openscript.search_sfx        ━▶ Search 261 SFX by mood/role
openscript.search_music      ━▶ Search 16 tracks by genre/mood
openscript.set_voice_profile ━▶ Configure TTS voice for voiceovers
openscript.export_edl        ━▶ Export timeline to EDL v2 format
openscript.analyze           ━▶ Spectral analysis of final output
openscript.report            ━▶ Full project status report
[...96 more tools for editing, transitions, captions, audio mixing, TTS, stock, verification, etc.]
```

## Architecture

```
openscript/
├── crates/
│   ├── openscript-core/       # Core editing state, EDL compiler
│   ├── openscript-mcp/        # MCP server (109 tools)
│   ├── openscript-ffmpeg/     # FFmpeg wrapper, render pipeline
│   ├── openscript-transcribe/ # Whisper transcription, Hinglish model
│   └── openscript-tts/        # Voice-over engine
├── src/                       # Python orchestration + AI agent
├── scripts/                   # Utility scripts
└── pyproject.toml
```

### Tech Stack

| Layer | Technology |
|-------|-----------|
| **Core Engine** | Rust (9 crates, musl static binaries) |
| **Orchestration** | Python 3.11 (crewai-style agent flow) |
| **Transcription** | Whisper (Hinglish-optimized) |
| **Rendering** | FFmpeg 6+ with automatic audio ducking |
| **Voice-over** | Coqui TTS + ElevenLabs voice profiles |
| **Protocol** | MCP (Model Context Protocol) |
| **Storage** | SQLite (per-project EDL state) |

## Project Structure

```text
openscript/
├── crates/
│   ├── openscript-core/         # Core editing state, EDL compiler
│   ├── openscript-mcp/          # MCP server (43 tools)
│   ├── openscript-ffmpeg/       # FFmpeg wrapper, render pipeline
│   ├── openscript-transcribe/   # Whisper transcription, Hinglish model
│   └── openscript-tts/          # Voice-over engine
├── src/                         # Python orchestration + AI agent
│   ├── director/                # AI director (brief generation)
│   ├── editor/                  # Timeline assembly
│   ├── verifier/                # Post-render validation
│   └── assets/                  # 261 SFX + 16 music tracks
├── scripts/                     # Utility scripts
├── tests/                       # Integration tests
└── pyproject.toml
```

## Development

```bash
# Run tests
cargo test --workspace

# Run the MCP dev server
mcp dev src/openscript_mcp/server.py

# Build all Rust crates
cargo build --release
```

## License

MIT — see [LICENSE](LICENSE).

---

Developed by [Ishan Parihar](https://github.com/ishan-parihar)
