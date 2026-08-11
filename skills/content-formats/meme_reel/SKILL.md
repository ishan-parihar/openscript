---
name: content-format-meme-reel
description: >
  Use for viral-style meme reels: one fast narrator, short punchy takes (3-7s),
  heavy reaction memes (GIPHY pop-ins). Highest pacing, snappy delivery,
  sarcastic/deadpan tone over earnest.
metadata: { "tags": "content-format, meme, reel, short-form, viral, reaction-meme" }
---

# Meme Reel format playbook

> **≠ comedy_sketch:** meme_reel is ONE fast narrator with 3-7s takes and
> reaction-driven stickers — no character arc. Sketch needs a duo and a
> setup→punchline beat.

## Anatomy (scene structure)

```
1  Hook        — one line, ZERO context ("Everyone's lying to you about X")
2-4 Rapid takes— 3-4 punchy claims, each 3-7s
5  Punchline   — the spicy takeaway → reaction meme lands here
```

- **Shortest lines of any format.** Every line 3–7 seconds.
- Reaction memes are HEAVY here — `reaction_memes: true` + `sticker_mode:
  "reaction"` (reaction-driven stickers, not character stickers).
- Contract: 1–2 speakers, 4–7 scenes (`script.format.validate` enforces it).

## Speaker blueprint (voice.design instructs)

| id | role | gender | voice_design_instruct |
|---|---|---|---|
| `narrator` | Meme Narrator | auto | "meme narrator, quick snappy delivery, slightly sarcastic, high energy" |

The blueprint is gender-neutral by design (`auto`). To fix the narrator's
gender, append **"male voice"** or **"female voice"** to the instruct when you
design the voice.

## Emote vocabulary

`sarcastic`, `shocked`, `amused`, `deadpan`

## Correlated defaults

- `default_speed: 1.1`, `default_temperature: 0.85`
- `reaction_memes: true` · `sticker_mode: "reaction"` · `music_mood: "energetic"`

## Authoring loop

1. Fetch the canonical playbook + worked-example draft:
   `director.format {type: "meme_reel", topic: "<topic>"}` or
   `openscript video new --format meme_reel --topic "<topic>"` — the registry
   is the single source of truth; don't hand-copy JSON from this file.
2. Design `meme_narrator` with `voice.design` (append a gender to the instruct).
3. Write zero-context hook, 3-4 rapid takes, spicy punchline.
4. `script.parse` → `script.format.validate` → fix issues → `script.to_video`.
