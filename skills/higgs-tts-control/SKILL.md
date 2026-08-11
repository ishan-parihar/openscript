---
name: higgs-tts-control
description: >
  Use when generating, auditing, or designing speech for the Higgs Audio v3 TTS
  engine (voice profiles with provider=higgs, scripts with tts.backend=higgs) —
  especially for expressive/emotional/character-driven lines, control-token
  injection (emotion / style / sound-effect / prosody), the canonical one-shot
  chunking config, or diagnosing voice quality issues in any script.to_video /
  audio-to-video / video-to-video pipeline. Higgs is a 4B conversational TTS
  with 100+ languages, zero-shot voice cloning, and 43 inline control tags
  (bosonai/higgs-tts-3-4b). This skill documents the tag catalog, the exact
  placement rules from the model's PROMPTING.md, how the OpenScript script
  schema maps onto them, the canonical synthesis parameters, and the known
  generation envelope (one-shot stability limits, degenerate-loop guards,
  retry-on-degenerate). Read this before writing scripts or prompts for any
  higgs voice so every line gets context-relevant emotion/style/prosody.
metadata: { "tags": "tts, voice, higgs, control-tags, emotion, prosody, script-to-video, audio, expressive" }
---

# Higgs Audio v3 — control tokens & canonical config

Higgs Audio v3 is the **expressive 4B conversational TTS** in OpenScript
(`tts.backend = "higgs"`, profiles registered with `provider=higgs`). It
speaks, not just reads: **100+ languages, zero-shot voice cloning, and inline
control over emotion, style, prosody, pauses, and sound effects** via 43
control tags. 24 kHz, 25 fps, 8-codebook Higgs v2 codec.

This skill is the authoritative reference for:
1. **Writing text with control tags** — the 43-tag catalog + placement rules.
2. **The canonical synthesis config** — one-shot chunking, temperature, top_k,
   and the guardrails that keep long draws stable.
3. **Script-schema mapping** — which `ScriptSpec`/`SceneSpec` fields become
   tags, so `script.to_video` / `script.generate_voices` produce
   context-relevant delivery automatically.
4. **Per-module guidance** — the same rules applied across script→video,
   audio→video, and video→video pipelines.

---

## 1. The 43 control tags (from PROMPTING.md)

Every tag is `<|category:tag|>`. There are **two placements**:

- **Sentence-level** — emotion, style, and prosody `speed_* / pitch_* /
  expressive_*`. Put at the **start of the sentence**; it colors the whole
  sentence.
- **Inline** — sound effects (`sfx`) and prosody `pause / long_pause`. Insert
  at the **exact position** in the sentence where the effect should occur.

**`sfx` gotcha:** format is `<|sfx:tag|>onomatopoeia, then the line` — the tag
comes **first**, immediately followed by the onomatopoeia with **no space**
between them.

### Emotion (21) — sentence-level
`affection` · `amusement` · `anger` · `arousal` · `awe` · `bitterness` ·
`confusion` · `contemplation` · `contentment` · `determination` · `disgust` ·
`elation` · `enthusiasm` · `fear` · `helplessness` · `longing` · `pride` ·
`relief` · `sadness` · `shame` · `surprise`

```text
<|emotion:elation|>Welcome aboard, we are absolutely thrilled to have you here!
<|emotion:anger|>This cannot stand — not one more day.
<|emotion:pride|>We built this from nothing, and look at it now.
```

### Style (3) — sentence-level
`singing` · `shouting` · `whispering`

```text
<|style:whispering|>Come closer, I have a little secret to share.
<|style:shouting|>Everyone out of the building — now!
<|style:singing|>La la la, nothing can bring us down today.
```

### Sound effects (9) — inline, tag first + onomatopoeia, NO space
`cough` · `laughter` · `crying` · `screaming` · `burping` · `humming` · `sigh`
· `sniff` · `sneeze`

```text
<|sfx:cough|>Ahem, welcome everyone, let's get started.
<|sfx:laughter|>Haha, so glad you could make it!
<|sfx:sigh|>Haah, what a day — but let's move on.
```

### Prosody (10)
- Sentence-level: `speed_very_slow` · `speed_slow` · `speed_fast` ·
  `speed_very_fast` · `pitch_low` · `pitch_high` · `expressive_high` ·
  `expressive_low`
- Inline: `pause` (~400-700 ms) · `long_pause` (~700-1500 ms)

```text
<|prosody:speed_slow|>Take your time, there's really no need to rush.
<|prosody:expressive_high|>And that — that is the moment everything changed!
Hello there <|prosody:pause|> and welcome to the show.
```

### Stacking
You can stack tags in one sentence (a leading emotion + an inline sfx):

```text
<|emotion:elation|><|sfx:laughter|>Haha, welcome, welcome, we're so happy you're here!
```

### Tips (from PROMPTING.md)
- `speed_very_slow` only slows the model to roughly ~5s slower; for slower
  delivery, insert `<|prosody:long_pause|>` between phrases instead.
- **Only the 43 tags are recognized** — anything else degrades output or gets
  **read literally aloud**. Never inject free-form parenthetical delivery
  instructions like `(in a low voice)` into higgs text.

---

## 2. Canonical synthesis config (the audited default)

Probed empirically on the OpenScript GPU stack (RTX 2060 SUPER, int4 ONNX).
**This is the canonical configuration to use unless a scene demands otherwise:**

| Knob | Canonical value | Why |
|---|---|---|
| `chunking` | `"one_shot"` (auto up to ~500 chars) | Higgs is a conversational model; a **single autoregressive draw** keeps tonality/flow continuous across the whole line. Sentence-chunking re-anchors to the reference at every seam and resets the delivery — the "tonality breaks per sentence" bug. |
| `temperature` | `0.8` | Reference recommendation for cloning. 0.7 = flatter/safer; 0.9 = more inflection but ~2x the degenerate-draw rate. |
| `top_k` | `50` | Reference recommendation. |
| `top_p` | off (`None`) | Optional; `0.9` matches the sglang-omni passthrough when more smoothing is wanted. |
| `default_speed` | scene `speed` (if set) | Mapped to a `<|prosody:speed_*|>` tag — the model paces NATURALLY. Never rubber-band with ffmpeg when the value is ≥1.08 or ≤0.92. |
| `pitch` | scene `pitch` (if set) | Mapped to `<|prosody:pitch_low|>` (≤0.9) / `<|prosody:pitch_high|>` (≥1.1). |
| `emote` | scene `emote` | Mapped to emotion/style/sfx tags inside the sidecar. |
| `tone` | scene `tone` | Scanned for delivery keywords → style/expressive tags; the rest is dropped (never read aloud). |
| `control_tags` | scene `control_tags` | RAW passthrough for inline effects the structured fields can't express. |
| `max_new_tokens` | auto (length-proportional) | Capped at 768 per sentence-chunk, 2048 for one-shot. Degenerate-loop guards + retries backstop. |

### Empirical config matrix (measured on GPU, RTX 2060 SUPER, int4 ONNX)

Measured with the sidecar serve protocol over two scenes — **S1** = realistic
35-word scene (~9 s), **S2** = long 75-word scene (~20 s). `below4k%` = energy
below 4 kHz (clean speech 75-95%; hiss/degraded 20-40%); `zcps` =
zero-crossings/sec (voiced <3000; noise >8000); `wall_s` = real time incl.
retries.

| config | dur_s | below4k% | zcps | chunks | wall_s | read |
|---|---|---|---|---|---|---|
| S1 sentence t0.80 | 9.64 | **86.4** | 2666.9 | 1 | 69.3 | full ✓ |
| S1 auto t0.80 | 12.04 | 67.8 | 5156.6 | 1 | 89.4 | full ✓ |
| S1 one_shot t0.80 | 23.72 | 66.0 | 5324.5 | 1 | 340.7 | overrun ✗ |
| S1 one_shot t0.70 | 23.72 | 40.5 | 4707.5 | 1 | 408.5 | overrun ✗ |
| S1 one_shot t0.90 | 9.32 | 71.6 | 3511.4 | 1 | 76.1 | full ✓ |
| S1 one_shot tags | 13.84 | 51.4 | 4726.5 | 1 | 299.1 | full ✓ |
| S1 one_shot top_p0.9 | 9.2 | 74.8 | 3700.7 | 1 | 136.2 | full ✓ |
| S2 one_shot t0.80 | 15.6 | 38.8 | 5773.7 | 1 | 268.1 | truncated ✗ |
| S2 sentence t0.80 | 14.48 | 53.4 | 3521.5 | 1 | 376.5 | truncated ✗ |
| S2 auto t0.80 | **21.28** | 58.7 | 3618.2 | 1 | 363.8 | **full ✓** |

**Reading the matrix:**
- `sentence` is the most spectrally stable on short scenes (86.4% below-4k,
  exact duration) — but re-anchors per chunk, so tonality breaks at seams.
- `one_shot` gives continuous tonality but is stochastic: the same text can
  end cleanly (t0.90) or overrun into a tone loop (t0.70/t0.80, 23.7 s for a
  9 s scene) — the degenerate-loop guard + temperature-cooling retries exist
  precisely for this.
- `auto` (one_shot ≤ ~500 chars, sentence beyond) read the FULL 75-word long
  scene (21.28 s) where pure one_shot truncated — auto is the best
  long-scene default.
- `top_p: 0.9` stabilizes one_shot draws (74.8%, clean termination, no
  overrun) at ~2x wall cost. **The canonical config: `auto` chunking,
  t0.8, top_k 50, top_p off — with `one_shot` for dialogue/expressive
  lines under ~40 words.**

### Generation envelope (critical — know the limits)

- **One-shot stability**: clean single draws up to roughly **500 rows ≈ 20 s ≈
  ~50 words**. Past that, the AR model stochastically degenerates into
  sustained-tone loops (identical code vectors) — caught by the repetition
  guard (`HIGGS_REPEAT_BREAK_AFTER`, default 25 frames = 1 s of identical
  vectors) and recovered by **retry-on-degenerate** (up to 2 retries with
  fresh sampling).
- **Scene sizing guidance**: keep individual scenes **10-40 words** (5-15 s)
  for best quality. For long-form narration, prefer many short scenes over one
  giant draw — each scene is its own timeline event anyway, and short scenes
  sidestep the degeneration envelope entirely.
- **The 8192-token context** is for the *prompt* (reference codes + text); it
  does not make generation arbitrarily long. Longer text in one draw = more
  opportunity for a tone loop, which is why `auto` mode falls back to sentence
  chunking beyond ~500 chars.

### Degenerate-draw diagnostics

A draw is flagged degenerate (and retried with a 0.1-cooler temperature, floor
0.5) when ANY of these fire:
1. The reference sampler never ran its cb0-EOC wind-down (repetition guard
   fired early or the token cap hit).
2. BOC/EOC ids leak into the de-delayed codes (misaligned termination).
3. The decoded waveform is dead-air silence (RMS < 0.008).
4. **Spectral-noise check** (`spectral_noise_check`): zcps > 8000 OR <40%
   energy below 4 kHz on the highest-energy window — a draw that terminates
   cleanly but is broadband hiss (the AR model can lock onto a hiss pattern
   whose vectors dodge the identical-vector guard). A mid-window pause is
   excluded via an RMS guard so clean draws with a mid-sentence break aren't
   false-flagged.
5. The chunk raised ANY exception (ORT OOM/GPU `Fail` included) — a failed
   draw is just another degenerate attempt.

If a chunk is STILL degenerate after all 3 cooled attempts, the sidecar ships
its last output with `status: "warning"` + `degenerate: true`, and the Rust
caller logs a loud `[tts/higgs] degraded draw shipped` warning — the output
may be noisy/truncated. Agents should re-audit the scene or lower its
`temperature`. Watch the sidecar log
(`/tmp/higgs_tts_sidecar.log`, `HIGGS_LOG`) for:
```
[higgs_tts_sidecar] degenerate chunk 1 attempt 2/3 (wind_down=False rms=0.15 rows=345 leaked=0); retrying
[higgs_tts_sidecar] WARNING: chunk 1 still degenerate after 3 attempts ...; using last output
```
Repeated `still degenerate after 3 attempts` = the text is likely past the
stable envelope — split the scene or lower the temperature.

---

## 3. Script-schema mapping (how tags get injected automatically)

`script.to_video` → `script.generate_voices` → `tts_generate_routed` → the
higgs sidecar. Every scene's **performance direction** becomes control tags:

| Script field | Where it goes | Example |
|---|---|---|
| `scenes[].emote` | `<\|emotion:X\|>` / `<\|style:X\|>` / `<\|sfx:X\|>` | `"emote": "angry"` → `<\|emotion:anger\|>` |
| `scenes[].tone` | keyword scan → style/expressive tags | `"tone": "low whisper, flat"` → `<\|style:whispering\|> <\|prosody:expressive_low\|>` |
| `scenes[].control_tags` | RAW verbatim prefix | `"control_tags": "<\|sfx:cough\|>Ahem,"` |
| `scenes[].speed` | `<\|prosody:speed_*\|>` (≥1.08 / ≤0.92) | `"speed": 1.2` → `<\|prosody:speed_fast\|>` |
| `scenes[].pitch` | `<\|prosody:pitch_low/high\|>` (≤0.9 / ≥1.1) | `"pitch": 0.8` → `<\|prosody:pitch_low\|>` |
| `tts.default_speed` | speed tag when scene has no `speed` | — |
| `scenes[].temperature` | sampling temp (0.6-0.9 sane range) | `"temperature": 0.85` |

**Example script excerpt:**

```json
{
  "tts": { "backend": "higgs", "voice": "ishan" },
  "speakers": { "narrator": { "voice": "ishan" } },
  "scenes": [
    {
      "speaker": "narrator",
      "text": "Welcome, and thank you for being here tonight.",
      "emote": "elation",
      "temperature": 0.85
    },
    {
      "speaker": "narrator",
      "text": "But first, a confession.",
      "emote": "whisper",
      "tone": "hushed, slow"
    },
    {
      "speaker": "narrator",
      "text": "That rumor? Completely true.",
      "emote": "laugh",
      "control_tags": "<|sfx:laughter|>Haha,",
      "speed": 1.1
    }
  ]
}
```

**What NOT to do:**
- Do NOT put `(in an angry voice)` / `[sad tone]` inside `scenes[].text` —
  Higgs reads it **aloud**.
- Do NOT set `tts.default_speed` to an in-between value like 1.05 for higgs
  and expect a tag — the neutral band (0.92-1.08) emits no tag; only ≥1.08 /
  ≤0.92 map, and values in the band fall back to ffmpeg atempo (fine, just
  less natural).

---

## 4. Per-module pipeline guidance

### script.to_video (from-scratch creation)
- Author scenes 10-40 words; give each a deliberate `emote` (or `tone`) so
  every line is attuned — a flat script yields a flat read even on higgs.
- Use `control_tags` for inline pauses/beats: `"<|prosody:pause|> mid,"` at
  the point where a beat belongs.
- For dialogue formats (podcast/comedy/romcom) alternate speakers AND emotes —
  higgs control tags make every speaker's lines land with distinct delivery.
- The `one_shot` chunking is set automatically (sidecar default `auto`); don't
  override to `sentence` unless you hit the degenerate envelope.

### audio-to-video (voice cloning for existing footage)
- The voice profile (`provider=higgs`, registered via `voice.profile.add` with
  `ref_audio` + **exact** `ref_text`) anchors the clone. A mismatched
  `ref_text` produces a hissy/metallic clone — always whisper-transcribe the
  reference clip and store the exact transcript.
- Emote/speed/pitch/control_tags work identically here — pass them through
  `tts.generate` or the scene fields.

### video-to-video (NLE / re-voice)
- Same tag rules; the timeline's `voiceover` events carry the text, and
  `tts.generate` with `emotion`/`tone` applies the delivery.
- Watch `pause_ms`: for higgs, prefer `control_tags` with
  `<|prosody:pause|>`/`<|prosody:long_pause|>` inline over post-hoc silence
  insertion — the model's pause is naturally voiced.

### Other TTS engines (contrast)
| Engine | Instruction channel |
|---|---|
| `higgs` | 43 control tags (this skill) + one_shot chunking |
| `voicedesign` | Free-form NL instruct per line (personality + emotion) |
| `gepard` / `audio8` | Emotion-TAKE reference recordings (registered per-emotion refs) |
| `kokoro` | Preset voices, no per-line control |

---

## 5. Quick reference — writing a higgs line

```
1. Pick the emotion (21) → <|emotion:X|> at line start.
2. Pick the style (3) → <|style:X|> if whisper/shout/sing.
3. Add sfx inline → <|sfx:tag|>onomatopoeia, no space after the tag.
4. Add prosody → speed_*/pitch_*/expressive_* at start; pause inline.
5. Keep the scene ≤ ~40 words; set scene temperature 0.7-0.9.
6. Never put delivery instructions in the spoken text.
```

Env vars: `HIGGS_TEMPERATURE`, `HIGGS_TOP_K`, `HIGGS_TOP_P`,
`HIGGS_MAX_NEW_TOKENS`, `HIGGS_REPEAT_BREAK_AFTER`, `HIGGS_DEVICE`,
`HIGGS_MODEL_DIR`, `HIGGS_LOG`.
