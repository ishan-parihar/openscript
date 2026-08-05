# OpenScript

![Python](https://img.shields.io/badge/Python-3.11+-blue?logo=python)
![License](https://img.shields.io/badge/License-MIT-green)
![PyPI](https://img.shields.io/pypi/v/openscript?logo=pypi)
![Remotion](https://img.shields.io/badge/Remotion-4.x-black?logo=remotion)
![FFmpeg](https://img.shields.io/badge/FFmpeg-6+-red?logo=ffmpeg)


**Turn any script into a narrated video — AI voice, subtitles, B-roll, music, done.**

![OpenScript output](https://github.com/ishan-parihar/openscript/raw/main/assets/readme/openscript-output.png)

---

## What it does

| Input | Output |
|-------|--------|
| Markdown script | MP4 video (1080p/4K) |
| Sections → scenes | AI narration (Audio8 cloned voice / Kokoro / Edge / OpenAI TTS) |
| Code blocks → syntax highlights | Auto-generated subtitles (SRT/VTT) |
| Image refs → B-roll | Background music (Suno/UDIO) |
| Metadata → chapters | YouTube-ready chapters + description |

---

## Quick start

```bash
# Install
pipx install openscript

# Or with uv
uv tool install openscript

# First video in 60 seconds
openscript init my-video
# Edit my-video/script.md
openscript render my-video
# → my-video/output/final.mp4
```

---

## Example script

```markdown
---
title: "Rust Async in 3 Minutes"
voice: "af_heart"
music: "ambient-tech"
---

# Intro

Rust async isn't magic. It's a state machine.

## The Future Trait

```rust
trait Future {
    type Output;
    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output>;
}
```

*[B-roll: rust-logo.png]*

## The Executor

Executors drive futures to completion. `tokio::spawn` polls until `Ready`.

*[B-roll: tokio-diagram.png]*

---

## Architecture

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│   Script    │────▶│   Parser    │────▶│  Composer   │────▶│  Renderer   │
│  (Markdown) │     │  (AST +     │     │  (Remotion  │     │  (FFmpeg/   │
│             │     │   metadata) │     │   + TTS)    │     │   Remotion) │
└─────────────┘     └─────────────┘     └─────────────┘     └─────────────┘
                           │                   │                   │
                           ▼                   ▼                   ▼
                    Frontmatter           Scene graph         Video + audio
                    validation            composition         synchronization
```

---

## Features

| Category | Details |
|----------|---------|
| **TTS** | Audio8 (local zero-shot voice clone), Kokoro (local presets), Edge TTS, OpenAI, ElevenLabs |
| **Subtitles** | Word-level timing, style presets, multi-language |
| **B-roll** | Auto-fetch from Pexels/Unsplash, local images, code animations |
| **Music** | Suno, UDIO, local files, ducking under narration |
| **Code** | Syntax highlighting (syntect), line-by-line reveal, terminal recording |
| **Export** | MP4 (H.264/HEVC), WebM, ProRes, vertical/horizontal |

---

## Commands

| Command | Description |
|---------|-------------|
| `openscript init <name>` | Scaffold new project |
| `openscript render <name>` | Full render pipeline |
| `openscript preview <name>` | Fast preview (no B-roll/music) |
| `openscript voice <name>` | Generate narration only |
| `openscript subtitle <name>` | Generate SRT/VTT only |

---


## Visual proof

| Script → Video | Subtitle styling | Code animation |
|:---:|:---:|:---:|
| ![Script to video](https://github.com/ishan-parihar/openscript/raw/main/assets/readme/script-to-video.png) | ![Subtitles](https://github.com/ishan-parihar/openscript/raw/main/assets/readme/subtitles.png) | ![Code animation](https://github.com/ishan-parihar/openscript/raw/main/assets/readme/code-animation.png) |

| B-roll integration | Music ducking | Chapter markers |
|:---:|:---:|:---:|
| ![B-roll](https://github.com/ishan-parihar/openscript/raw/main/assets/readme/broll.png) | ![Music](https://github.com/ishan-parihar/openscript/raw/main/assets/readme/music.png) | ![Chapters](https://github.com/ishan-parihar/openscript/raw/main/assets/readme/chapters.png) |

## Requirements

- Python 3.11+
- FFmpeg 6+
- Node.js 20+ (for Remotion)
- 4 GB RAM minimum

---

## License

MIT — see [LICENSE](LICENSE).
