---
name: content-format-meme-reel
description: >
  Use for viral-style meme reels: one fast narrator, short punchy takes (3-7s),
  heavy reaction memes (GIPHY pop-ins). Highest pacing, snappy delivery,
  sarcastic/deadpan tone over earnest.
metadata: { "tags": "content-format, meme, reel, short-form, viral, reaction-meme" }
---

# Meme Reel format playbook

## Anatomy (scene structure)

```
1  Hook        — one line, ZERO context ("Everyone's lying to you about X")
2-4 Rapid takes— 3-4 punchy claims, each 3-7s
5  Punchline   — the spicy takeaway → reaction meme lands here
```

- **Shortest lines of any format.** Every line 3–7 seconds.
- Reaction memes are HEAVY here — `reaction_memes: true` + `sticker_mode:
  "reaction"` (reaction-driven stickers, not character stickers).
- `format.alternation: "none"` (single narrator by default).

## Speaker blueprint (voice.design instructs)

| id | role | gender | voice_design_instruct |
|---|---|---|---|
| `narrator` | Meme Narrator | auto | "meme narrator, quick snappy delivery, slightly sarcastic, high energy male voice" |

(Swap to a female voice for variety: "...slightly sarcastic, high energy
female voice".)

## Emote vocabulary

`sarcastic`, `shocked`, `amused`, `deadpan`

## Correlated defaults

- `default_speed: 1.1`, `default_temperature: 0.85`
- `reaction_memes: true` · `sticker_mode: "reaction"` · `music_mood: "energetic"`

## Worked example skeleton

```json
{
  "schema": "openscript-video/v1",
  "title": "<topic> — Meme Reel",
  "format": {"type": "meme_reel", "alternation": "none",
             "default_speed": 1.1, "default_temperature": 0.85,
             "reaction_memes": true, "sticker_mode": "reaction",
             "music_mood": "energetic"},
  "tts": {"backend": "voicedesign"},
  "speakers": {
    "narrator": {"voice": "meme_narrator", "gender": "auto", "position": "top-left"}
  },
  "background": {"type": "procedural", "change_cadence": "scene"},
  "meme_brolls": {"enabled": true},
  "scenes": [
    {"speaker": "narrator", "text": "Everyone's lying to you about <topic>.", "emote": "sarcastic"},
    {"speaker": "narrator", "text": "The experts? They read one article. I read ZERO and I'm right.", "emote": "amused"},
    {"speaker": "narrator", "text": "Point one: it's not complicated. Point two: nobody wants it to be.", "emote": "deadpan"},
    {"speaker": "narrator", "text": "Point three — the spicy one — the fix is obvious and nobody will say it.", "emote": "shocked"},
    {"speaker": "narrator", "text": "Ignore the noise on <topic>, follow the pattern, and you'll see it too.", "emote": "sarcastic"}
  ]
}
```
