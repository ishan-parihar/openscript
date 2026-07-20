# OpenScript — Hinglish STT, Handy Integration, and Title Cards Brainstorm

**Date:** 2026-07-20  
**Context:** Fresh-agent audit Run #8 identified Parakeet caption quality as the top P1 gap (+10 pts to Grade A). User wants to evaluate Hinglish-specific models and explore title cards via HyperFrames.

---

## Part 1: Hinglish STT Model Comparison

### Current State
- **Apex/Parakeet ONNX** — encoder + decoder models at `mcp/assets/parakeet/`, wrapper at `mcp/scripts/apex_transcriber.py`
- Decoder is malfunctioning (produces 0 words); Whisper fallback active
- All captions are **estimated** (no frame-accurate word timestamps)

### Candidate 1: `Trelis/whisper-hinglish-preview`

| Property | Value |
|----------|-------|
| Base model | `whisper-large-v3` → fine-tuned from `ARTPARK-IISc/whisper-large-v3-vaani-hindi` |
| Parameters | **2B** |
| Format | **Safetensors + Transformers** (PyTorch) |
| Output | Romanized Latin text (`"yaar mai kal office nahi jaunga"`) |
| License | Apache 2.0 |
| Downloads | 4,007/month |
| Hinglish WER | **13.67%** (CoSHE-500 conversational), **12.73%** (hiacc-adult) |
| Hindi WER | **12.86%** (Common Voice), **12.57%** (FLEURS) |
| English WER | **6.93%** (FLEURS-en) |
| API | HuggingFace Transformers pipeline, or `POST router.trelis.com/api/v1/transcribe` |
| Platform | **Cross-platform** (Linux, macOS, Windows) — needs CUDA or CPU PyTorch |

**Pros:**
- Best open-weights Hinglish WER (13.67% vs whisper-large-v3's 29.74%)
- Outputs Romanized Latin directly — matches OpenScript's caption style
- Uses `<|mixedcode|>` token for code-switched utterances
- Active development (Trelis is a commercial entity investing in this)
- Standard Transformers API — drop-in with current Python sidecar pattern

**Cons:**
- 2B params — needs ~4GB VRAM (CUDA) or runs slower on CPU
- No built-in word-level timestamps (Whisper models output text, not word timings)
- Would need WhisperX or whisper-timestamped for word alignment (adds complexity)

### Candidate 2: `shrimalmadhur/whisperkit-hinglish`

| Property | Value |
|----------|-------|
| Base model | `Oriserve/Whisper-Hindi2Hinglish-Apex` → `whisper-large-v3-turbo` |
| Parameters | **0.8B** |
| Format | **CoreML** (`.mlmodelc`) |
| Output | Romanized Latin text |
| License | Apache 2.0 |
| Downloads | 19/month |
| Platform | **Apple Silicon only** (CoreML) |

**Pros:**
- Lighter model (0.8B vs 2B)
- Derived from the same Oriserve model OpenScript already uses via Apex

**Cons:**
- **CoreML only** — will NOT work on Linux (our primary build target)
- 19 downloads/month vs Trelis's 4,007 — much less community validation
- Apple-only is a dealbreaker for a cross-platform pipeline

### Verdict

**`Trelis/whisper-hinglish-preview` is the clear winner.** The CoreML model is Apple-only and dead on arrival for OpenScript's Linux-first pipeline.

**However**, neither model solves the word-timestamp problem. Both output text only. For frame-accurate captions, we'd need:
1. Run Trelis model for transcription quality
2. Run WhisperX or whisper-timestamped for word alignment
3. Or use the existing Parakeet ONNX decoder once it's fixed

**Recommendation:** Integrate Trelis as a **quality upgrade** for the transcription step, not as a replacement for word-timestamp generation. The pipeline would be:

```
Trelis (transcribe) → WhisperX (word align) → ASS captions
```

This gives us best-in-class Hinglish transcription quality + frame-accurate word timing.

---

## Part 2: Handy Integration Analysis

### What Handy Is

Handy is a **desktop STT application** — a Tauri app (Rust backend + React frontend) that:
- Listens to microphone input via global keyboard shortcut
- Transcribes speech locally using Whisper or Parakeet
- Pastes the transcribed text into whatever app you're typing in

### Why It Doesn't Fit OpenScript

| Handy's Architecture | OpenScript's Need |
|----------------------|-------------------|
| Desktop app (Tauri) | Library/CLI (no GUI) |
| Microphone input | File input (pre-recorded audio/video) |
| Real-time streaming | Batch processing |
| Global keyboard shortcuts | MCP tool calls |
| Text paste into active window | SRT/ASS subtitle file output |
| User-facing app | Pipeline component |

Handy is a **user-facing application**, not an **embeddable library**. It has:
- No Rust library crate to depend on
- No Python module to import
- No CLI API for batch file processing
- No MCP tool interface

The `transcribe-cpp` and `transcribe-rs` libraries inside Handy ARE useful, but they're private to the Handy repo — not published crates. We'd have to:
1. Fork Handy
2. Extract the transcription libraries
3. Build our own integration

This is not worth the effort when we already have:
- `openscript-transcribe` crate (Apex wrapper)
- Whisper fallback via `whisper_timestamped`
- The Trelis model as a quality upgrade path

### Verdict

**Do not integrate Handy.** It's the wrong abstraction level. Instead:
- If we want Parakeet's CPU-optimized transcription, use `transcribe-rs` patterns directly (it's just ONNX inference)
- If we want better Hinglish quality, use Trelis via the existing Python sidecar pattern

---

## Part 3: Title Cards via HyperFrames

### The Opportunity

HyperFrames is **underused**. Currently it only renders the final video composition (main video + b-roll layers). But the HTML+GSAP architecture is perfect for **title card overlays** — animated text, hooks, CTAs, lower thirds.

### Architecture

The `edl_v2_to_html.ts` compiler already generates:
```html
<html data-composition-id="..." data-duration="30" data-fps="30" data-width="1080" data-height="1920">
  <div id="stage">
    <video class="video-layer" ...></video>
    <video class="broll-layer" ...></video>
  </div>
  <script>
    const tl = gsap.timeline({ paused: true });
    // ... tweens ...
    window.__timelines["main-with-broll"] = tl;
  </script>
</html>
```

Title cards would add **overlay `<div>` elements** with GSAP-animated text:
```html
<div class="title-card" id="hook-card" data-start="0" data-duration="3">
  <h1>WHY BLACK HOLES MATTER</h1>
  <p>3 things you didn't know</p>
</div>
```

With GSAP tweens:
```javascript
tl.fromTo("#hook-card", 
  { opacity: 0, y: 50 }, 
  { opacity: 1, y: 0, duration: 0.5, ease: "power2.out" }, 
  0);
tl.to("#hook-card", 
  { opacity: 0, y: -30, duration: 0.3, ease: "power2.in" }, 
  2.7);
```

### Title Card Types

| Type | Position | Duration | Purpose |
|------|----------|----------|---------|
| **Hook** | Top/center, large text | 2-4s at start | Grab attention in first 3s |
| **Topic** | Center, medium text | 1-2s per scene | Scene/section title |
| **Payoff** | Center, large text | 2-3s at end | Key takeaway / conclusion |
| **CTA** | Bottom, persistent | Last 3-5s | Subscribe, follow, link |
| **Lower Third** | Bottom third | 3-5s | Speaker name, source citation |

### GSAP Animation Patterns for Title Cards

```javascript
// Hook: slide up + fade in
tl.fromTo("#hook", { opacity: 0, y: 80 }, { opacity: 1, y: 0, duration: 0.6, ease: "back.out(1.2)" }, 0);

// Topic: typewriter effect
tl.fromTo("#topic", { clipPath: "inset(0 100% 0 0)" }, { clipPath: "inset(0 0% 0 0)", duration: 0.8, ease: "power2.out" }, 0);

// CTA: pulse + glow
tl.fromTo("#cta", { scale: 0.8, opacity: 0 }, { scale: 1, opacity: 1, duration: 0.4, ease: "elastic.out(1, 0.5)" }, 0);

// Payoff: zoom in
tl.fromTo("#payoff", { scale: 1.5, opacity: 0 }, { scale: 1, opacity: 1, duration: 0.8, ease: "power3.out" }, 0);
```

### Implementation Path

**Phase 1: Extend `edl_v2_to_html.ts`** (low effort)
- Add `title_cards` array to the Timeline type
- Each card: `{ type: "hook"|"topic"|"payoff"|"cta"|"lower_third", text: string, startMs: number, endMs: number, style?: object }`
- Compiler generates `<div>` elements + GSAP tweens

**Phase 2: Add `script.to_video` integration** (medium effort)
- `script.parse` output includes `title_cards` array (hook from `script.hook`, CTA from `script.cta`, topic cards from scene titles)
- `script.to_timeline` passes title cards through to the timeline
- `script.to_video` → `edl_v2_to_html.ts` → HyperFrames renders them

**Phase 3: HyperFrames title card templates** (medium effort)
- Create reusable HTML+CSS templates in `hyperframes/compositions/title-cards/`
- Templates: `hook-slide-up.html`, `topic-typewriter.html`, `cta-pulse.html`
- Agent selects template based on content type
- Templates use CSS custom properties for easy customization:
  ```css
  :root {
    --card-font: 'Bebas Neue', sans-serif;
    --card-color: #ffffff;
    --card-bg: rgba(0,0,0,0.6);
    --card-size: 64px;
  }
  ```

**Phase 4: Font integration** (trivial)
- OpenScript already has `mcp/fonts/BebasNeue-Regular.ttf` (used for ASS captions)
- HyperFrames HTML can reference it via `@font-face`:
  ```css
  @font-face {
    font-family: 'Bebas Neue';
    src: url('../fonts/BebasNeue-Regular.ttf');
  }
  ```

### Why HyperFrames Over FFmpeg drawtext

| HyperFrames (HTML+GSAP) | FFmpeg drawtext |
|-------------------------|-----------------|
| CSS font rendering (subpixel, ligatures) | Basic text rendering |
| GSAP animations (elastic, bounce, typewriter) | Fade in/out only |
| Responsive layout (flexbox, grid) | Fixed position coordinates |
| Easy to iterate (edit HTML, re-render) | Rebuild filter graph |
| Can use any web font | Limited font support |
| HTML+CSS design tools available | Manual positioning |

### Estimated Impact

| Metric | Current | With Title Cards |
|--------|---------|-----------------|
| Production Score | ~70/100 | **~80/100** (+10 pts) |
| Hook retention | Unknown | First 3s engagement boost |
| CTA compliance | 0% (no CTA) | Measurable follows/subscribes |
| Professional polish | Medium | High |

---

## Summary

| Topic | Recommendation | Priority |
|-------|---------------|----------|
| Hinglish STT | **Trelis/whisper-hinglish-preview** — best open-weights Hinglish model, cross-platform | P1 |
| Handy integration | **Skip** — wrong abstraction level, desktop app not library | N/A |
| Title Cards | **HyperFrames templates** — extend EDL compiler, add animated text overlays | P1 |

### Next Steps

1. **Trelis integration:** Create `mcp/scripts/trelis_whisper.py` sidecar, wire into `transcribe` tool as quality upgrade
2. **Title card templates:** Create `hyperframes/compositions/title-cards/` with hook/topic/CTA templates
3. **EDL compiler extension:** Add `title_cards` array support to `edl_v2_to_html.ts`
4. **Pipeline integration:** Wire `script.parse` output → `edl_v2_to_html.ts` → HyperFrames render
