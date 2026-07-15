# OpenScript

**AI-directed video creation & editing — agents as directors, from script (or raw footage) to polished short-form MP4.**

[![Rust](https://img.shields.io/badge/Rust-1.80+-orange.svg)](https://www.rust-lang.org/)
[![Python](https://img.shields.io/badge/Python-3.11+-blue.svg)](https://www.python.org/)
[![TypeScript](https://img.shields.io/badge/TypeScript-5.0+-3178C6.svg)](https://www.typescriptlang.org/)
[![MCP](https://img.shields.io/badge/MCP-Server-5B8DEF.svg)](https://modelcontextprotocol.io/)
[![License](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE)

---

## What It Does

OpenScript exposes **76 MCP tools** (plus a CLI mirror) so AI agents can direct short-form video end-to-end:

1. **From scratch (golden path):** write a script JSON → one call → vertical MP4 with TTS, captions, multi-scene backgrounds, optional stickers/memes, music, ducking.
2. **NLE on existing footage:** transcribe → timeline → b-roll/music/SFX → render.

Canonical agent docs: [`AGENT_GUIDE.md`](./AGENT_GUIDE.md). Engineering protocol: [`AGENTS.md`](./AGENTS.md).

### Golden trajectories

| Path | Flow | When |
|------|------|------|
| **A — From scratch** | `system.capabilities` → `script.parse` → **`script.to_video`** | Create a new short from a script |
| **B — NLE** | `transcribe` → `timeline.build` → `broll.director` → `timeline.render` | Edit existing video |
| **C — Discovery** | `system.capabilities` → `help.tool` | New environment / unsure which tool |

Do **not** chain intermediate tools unless you need fine control. `script.to_video` is the default.

---

## Key Features

- **Script → video orchestrator** — Kokoro TTS, word-timed ASS captions (4 styles), unique backgrounds per scene, GIPHY stickers/meme cuts, music + sidechain ducking
- **Multi-track timeline (EDL v2)** — dialogue, voiceover, captions, b-roll, music, SFX
- **Transcription** — Apex (Hinglish-oriented Whisper) with word/phrase SRT
- **Asset search** — SFX (261), music index, Pexels / GIPHY / Pixabay / YouTube / library
- **Render engines** — FFmpeg multilayer (default), HyperFrames (HTML+GSAP), Remotion escape hatch
- **Agent meta-tools** — `system.capabilities`, `help.tool` (trajectory-aware ranking)
- **QA** — `verify.audio` / `verify.captions` / `verify.render`
- **CLI + MCP + Tauri shell** — same `route_tool()` surface

---

## Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│              MCP Server / CLI  (76 tools, stdio)                  │
│  AI agents  ── script.to_video / timeline.* / hf.* / verify.* ── │
└──────┬──────────┬──────────┬──────────┬──────────┬───────────────┘
       │          │          │          │          │
  ┌────▼───┐ ┌───▼────┐ ┌──▼─────┐ ┌──▼──────┐ ┌▼──────────┐
  │ Core   │ │Trans-  │ │ FFmpeg │ │  TTS    │ │  Assets   │
  │Timeline│ │ cribe  │ │ Multi- │ │ Kokoro  │ │ Pexels/   │
  │  & SRT │ │ Apex   │ │ layer  │ │ Voicebox│ │ GIPHY/SFX │
  └────────┘ └────────┘ └────────┘ └─────────┘ └───────────┘
                              │
                    ┌─────────▼──────────┐
                    │ HyperFrames /      │
                    │ Remotion (optional)│
                    └─────────┬──────────┘
                              ▼
                            MP4
```

---

## Tech Stack

| Layer | Technology | Purpose |
|-------|-----------|---------|
| Core | Rust workspace (8 crates) | Timeline, tools, render, TTS, assets |
| TTS | Kokoro ONNX (default) + optional Voicebox | Voiceover |
| STT | Apex / Whisper-Hindi2Hinglish | Transcription |
| Alignment | Parakeet TDT (optional) | Word-level caption sync |
| Render | FFmpeg + HyperFrames + Remotion | Composition |
| AI interface | MCP (stdio) + CLI | Agent / human control |

---

## Quick Start

### Prerequisites

- Rust 1.80+
- Python **3.11–3.13** (kokoro-onnx needs a supported version; 3.13 requires recent kokoro-onnx)
- FFmpeg + ffprobe
- Optional: Node 20+ (Remotion / Tauri frontend), yt-dlp, API keys

### Bootstrap (recommended)

```bash
bash setup.sh                 # models, deps, build, smoke test
# or
bash setup.sh --skip-models   # if you already have Kokoro weights
```

### Env / API keys

Set via environment or `mcp/assets/.openscript_config.json`:

| Variable | Used for |
|----------|----------|
| `PEXELS_API_KEY` | Stock video backgrounds / b-roll |
| `GIPHY_API_KEY` | Stickers + meme b-rolls |
| `PIXABAY_API_KEY` | Stock music/video search |
| `KOKORO_MODEL` / `KOKORO_VOICES` | Override default ONNX paths |

Kokoro model paths expected by default:

- `mcp/assets/kokoro/onnx/kokoro-v1.0.onnx`
- `mcp/assets/kokoro/voices/voices-v1.0.bin`

### MCP server

```bash
cargo run -p openscript-mcp --bin mcp-server
# or after build:
./target/release/mcp-server
```

### CLI (from-scratch golden path)

```bash
./target/debug/openscript system-capabilities
./target/debug/openscript script-parse --script my_script.json
./target/debug/openscript script-to-video \
  --script my_script.json \
  --output-path out.mp4 \
  --output-dir artifacts
```

### NLE (existing footage)

Use MCP tools `transcribe` → `reelize.timeline` / `timeline.*`, or the legacy reelize CLI flows documented in older tooling.

---

## MCP Tools (76)

High-level map (full “when to use” tables: [`AGENT_GUIDE.md`](./AGENT_GUIDE.md)):

| Category | Examples |
|----------|----------|
| **Script creation** | `script.parse`, `script.to_video`, `script.to_timeline`, `script.generate_voices`, `script.build_captions` |
| **Discovery** | `system.capabilities`, `help.tool`, `voices.list` |
| **Timeline** | `timeline.build`, `timeline.preview`, `timeline.inspect`, `timeline.render`, `timeline.to_hyperframes`, … |
| **Background / b-roll** | `background.fetch`, `background.search`, `broll.director`, `broll.fetch` |
| **Media / stickers** | `gif.search`, `gif.download`, `media.search`, `media.download`, `overlay.assign`, `sticker.*` |
| **Music / SFX / library** | `music.search`, `music.assign`, `sfx.search`, `library.search`, `library.build` |
| **NLE / reelize** | `transcribe`, `srt.*`, `edl.build`, `reelize`, `reelize.timeline`, `reelize.brief`, `reelize.direct` |
| **Render** | `composition.render`, `hf.classify`, `hf.lint`, `hf.validate`, `hf.snapshot`, `hf.render`, `render` |
| **QA** | `verify.audio`, `verify.captions`, `verify.render` |
| **Stock / YouTube** | `stock.search`, `stock.fetch`, `youtube.search`, `youtube.download` |

---

## Minimal script JSON

```json
{
  "title": "3 Surprising Facts About the Human Brain",
  "video_keywords": ["brain", "neuroscience", "neurons"],
  "speakers": {
    "alice": { "voice": "kokoro:af_heart", "position": "top-left", "scale": 0.35 }
  },
  "scenes": [
    { "speaker": "alice", "text": "Your brain has about 86 billion neurons." }
  ],
  "output": { "theme": "neutral" }
}
```

See `AGENT_GUIDE.md` for themes, caption styles, meme b-rolls, and sticker scaling.

---

## Project Structure

```
openscript/
├── crates/
│   ├── openscript-core/       # Timeline, SRT, script types
│   ├── openscript-mcp/        # MCP server + 76 tool handlers
│   ├── openscript-ffmpeg/     # Filter graphs, multilayer render
│   ├── openscript-tts/        # Kokoro + Voicebox clients
│   ├── openscript-transcribe/ # Apex wrapper
│   ├── openscript-assets/     # Music/SFX/Pexels
│   ├── openscript-cli/        # CLI
│   ├── openscript-tauri/      # Desktop shell
│   └── openscript-ui/         # Legacy ratatui TUI
├── mcp/scripts/               # Kokoro / Apex / Parakeet sidecars
├── mcp/assets/                # Models, music, backgrounds, indices
├── hyperframes/               # Default motion-graphics path
├── remotion/                  # Escape-hatch React renderer
├── AGENT_GUIDE.md             # Agent tool catalog
└── AGENTS.md                  # Engineering protocol
```

---

## Testing

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo build --workspace --exclude openscript-tauri
cargo test --workspace --exclude openscript-tauri --lib --bins --tests
cargo build -p openscript-mcp --release --bin mcp-server
bash scripts/smoke_test_mcp.sh
```

Baseline: **239+** library/integration tests (see `AGENTS.md` §6).

---

## License

MIT — see [LICENSE](./LICENSE).
