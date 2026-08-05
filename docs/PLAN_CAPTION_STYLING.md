# PLAN — Caption Styling System (fonts, highlight colors, animations)

**Date:** 2026-08-05
**Status:** Plan (implementation candidates marked `[P0]/[P1]/[P2]`)
**Context:** Captions now default to **center screen** (position mapping: center→Alignment 5,
bottom→Alignment 2 safe zone, top→Alignment 8). The user wants a real *styling system* —
varying font styles, highlight colors, and animation styles — not just the current
single word_highlight look.

---

## 1. Current state

`CaptionsSpec` (`crates/openscript-core/src/script.rs`) already carries the raw knobs:
`style`, `font`, `font_size`, `color`, `highlight_color`, `position`, `safe_zone`,
`max_words_per_line`. `generate_ass` (`crates/openscript-core/src/captions.rs`) burns
these into ASS with 4 style generators:

| Style | Behaviour | Animation vocabulary |
|-------|-----------|----------------------|
| `word_highlight` | Full line, current word scaled + recolored | `\fad(80,80)` + `\fscx110\fscy110` pop |
| `sentence_fade` | Whole sentence, fades on change | `\fad(200,300)` only |
| `karaoke_fill` | Word-by-word color fill | `\k` karaoke timing |
| `subtitle_rail` | Lower-third box (drawing layer + text) | none |

**Gaps discovered while auditing:**
1. **No bundled looks.** Everything is bespoke per-call; an agent must know the exact
   color hex + font name to get a good look. No presets, no discoverability.
2. **`safe_zone` is dead** — parsed in the spec but never honored by `generate_ass`
   (margins are hardcoded `canvas_h/20` or `canvas_h/6`).
3. **Animation vocabulary is tiny** — `\fad` and a static scale pop. No slide, bounce,
   typewriter, pop-in, or transform sequences, and no per-style `\t` animation.
4. **No outline/shadow/spacing/case controls** — the Style line hardcodes
   `Outline=5, Shadow=2, ScaleX/Y=100, Spacing=0, Bold=1`.
5. **Font availability is unmanaged** — we ship only `BebasNeue-Regular.ttf`; a spec
   asking for "Inter" silently falls back to whatever libass finds (or nothing).
6. **No style consistency** across tools: `captions.generate_ass`, `reelize.direct`,
   `script.to_video`, and `broll.auto` each carry their own caption defaults.

---

## 2. Design — a preset-first styling system

The core idea: **named presets bundle all knobs into a "look"**, and every knob stays
overridable. Agents pick a preset by name (discoverable via a new `captions.presets`
tool); the ASS generator renders the look; the renderer stays untouched (subtitles
burn-in already sits last in the z-order).

### 2.1 `CaptionPreset` — the bundled look

```json
{
  "name": "tiktok_green",
  "description": "Bold condensed font, white text, green pop highlight, scale-pop animation",
  "style": "word_highlight",
  "font": "Bebas Neue",
  "font_size": 84,
  "color": "#ffffff",
  "highlight_color": "#00ff88",
  "position": "center",
  "animation": "pop",
  "outline": 5,
  "outline_color": "#000000",
  "shadow": 2,
  "bold": true,
  "uppercase": false,
  "spacing": 0
}
```

Preset registry: `mcp/assets/caption_presets.json` (committed, like `voices.json`),
loaded by a new Rust module `openscript-core/src/caption_presets.rs`.

**Ship a starter palette (6 presets):**
- `tiktok_green` — the current viral look (Bebas Neue, white + #00ff88, pop)
- `neon_drift` — pink #ff2d95 + cyan #00f0ff, slide-up animation, thicker glow outline
- `bold_impact` — Anton, all-caps, yellow #ffd60a highlight, scale-pop, no shadow
- `minimal_white` — Inter, thin outline, subtle fade, karaoke fill, calm
- `pastel_soft` — Poppins, cream #F5F0E8 text, pastel highlight, bounce-in
- `news_rail` — subtitle_rail lower-third box, sans-serif, red accent

### 2.2 Extended `CaptionsSpec`

Add (all `#[serde(default)]` for backward compat):
- `preset: Option<String>` — when set, loads and fills every other field (later fields in the same call override).
- `animation: String` — `"pop" | "fade" | "slide_up" | "bounce" | "typewriter" | "none"` (default from style).
- `animation_ms: u32` — per-word/pop duration (default 80).
- `outline: u32`, `outline_color: String`, `shadow: u32`, `bold: bool`, `uppercase: bool`, `spacing: i32`.
- **Honor `safe_zone`**: bottom margin becomes `canvas_h * (1.0 - safe_zone)` (default 0.85 → 288px below baseline) instead of the hardcoded `/20`.

### 2.3 Animation engine — pure ASS first (`[P0]`)

Extend `generate_ass` to emit real libass transform sequences — no new deps, works in
the existing `subtitles=` burn path:

| Animation | ASS technique |
|-----------|---------------|
| `pop` | current `\fscx110\fscy110` on the active word |
| `fade` | `\fad(in,out)` tuned by `animation_ms` |
| `slide_up` | `\t(\start,\end,\fry0\fry0)`-free approach: `\move` from y+40 to y at line start, or `\t(0,300,\fsp10)` — **use `\pos` + `\move` on the first event** of the line |
| `bounce` | two `\t` transforms: `\t(0,150,\fscx115\fscy115)` then `\t(150,300,\fscx100\fscy100)` |
| `typewriter` | karaoke `\k` (already built) with the reveal color = `color`, fill = `highlight_color` |
| `none` | plain text, no overrides |

`word_highlight` stays the default style; `animation` is orthogonal to `style` where
sensible (e.g. `sentence_fade` + `slide_up` = whole-line slide).

### 2.4 Font management (`[P1]`)

- Commit 2–3 more TTFs to `mcp/fonts/` (Anton, Poppins, Inter) so presets actually
  resolve. `resolve_fonts_dir()` already points ffmpeg at `mcp/fonts` via `fontsdir`.
- `captions.presets` reports each preset's font + whether the TTF ships locally
  (`mcp/fonts/<name>.ttf`) so agents don't pick a font that won't render.
- Fallback chain in `generate_ass`: preset font → bundled TTF → `"Bebas Neue"`.

### 2.5 MCP surface (`[P0]`)

1. **NEW tool `captions.presets`** — lists `caption_presets.json` with name, description,
   and the fields each preset sets. Input: `{}` or optional `style` filter.
2. **Extend `captions.generate_ass`** schema: `preset`, `animation`, `animation_ms`,
   `outline`, `outline_color`, `shadow`, `bold`, `uppercase`, `spacing`.
   When `preset` is given, defaults come from the preset; explicit params win.
3. `broll.auto` + `reelize.direct` + `script.to_video` keep passing `style`/`position`
   only — the core default (`center`) now flows through everywhere.

### 2.6 Tauri UI (`[P2]`, optional)

A "Caption look" selector in the caption panel: preset cards (name + mini preview swatch)
→ fills the invoke payload. Skippable — the MCP surface delivers the feature first.

---

## 3. Implementation order

1. **`[P0]` Extend `CaptionsSpec` + `generate_ass`**
   - `captions.rs`: honor `safe_zone` for margins; add outline/shadow/bold/uppercase/spacing
     to both Style lines; implement `animation` (pop/fade/slide_up/bounce/typewriter/none)
     in `generate_word_highlight` + `generate_sentence_fade`; `preset` resolution.
   - Golden-string unit tests per animation + per position combo (mirror
     `test_generate_ass_position_mapping`).
2. **`[P0]` Preset registry**
   - `openscript-core/src/caption_presets.rs` (`CaptionPreset` struct + `load_presets()`),
     `mcp/assets/caption_presets.json` (6 presets), wire into `generate_ass` via `preset`.
3. **`[P0]` MCP tools**
   - `captions.presets` (definition + route + handler, ~40 LoC), extend
     `captions.generate_ass` schema + handler defaults. Tool count 97 → 98; update
     `server.rs`, `smoke_test_mcp.sh`, `integration_test.rs`, `AGENT_GUIDE.md`.
4. **`[P1]` Fonts** — add Anton/Poppins/Inter TTFs to `mcp/fonts/`, preset `font` fields
   point at them; `captions.presets` advertises `font_shipped` bool.
5. **`[P2]` Tauri preset picker** — caption panel preset cards.

---

## 4. Verification

1. `cargo build --workspace --exclude openscript-tauri` — zero warnings.
2. `cargo test --workspace --exclude openscript-tauri --lib --bins --tests` — new golden
   ASS tests for: each animation emits the expected `\t`/`\move`/`\k` sequence; preset
   resolution (explicit param overrides preset); safe-zone margin math.
3. Manual burn check per preset: `ffmpeg -f lavfi -i color -vf "subtitles=…:fontsdir=mcp/fonts"`
   → extract frame → pixel-bbox scan confirms text in the center band (y 40–60%).
4. Smoke test: `captions.presets` listed, 98 tools.

## 5. Non-goals

- No GPU/WebGL caption path; PupCaps (`overlay.generate`) remains the escape hatch for
  heavy HTML/GSAP motion graphics and is *not* the preset engine.
- No auto color-from-video sampling (frame-dominant-color → highlight) — noted as a
  future `vision.score_clip` integration, out of scope here.
- No per-word font mixing (multiple fonts in one caption line).
