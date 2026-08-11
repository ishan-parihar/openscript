---
name: content-format-documentary
description: >
  Use for serious, measured long-form content: a documentary-style narrator
  walking through evidence chapters. Longer sentences, lower temperature,
  calm music, minimal stickers. Authoritative and somber.
metadata: { "tags": "content-format, documentary, narration, evidence, chapters, measured" }
---

# Documentary format playbook

## Anatomy (scene structure)

```
1  Opening thesis     — "There's a story hidden inside X that nobody was told"
2-5 Evidence chapters — Chapter one / two / three ... (each one idea)
6  Synthesis          — the conclusion that ties the chapters together
7  Closing reflection — a lingering, thoughtful end
```

- 1–2 narrators. Longer sentences (3–5 clauses), slower pacing.
- Chapter markers in the text keep the structure audible.
- `format.alternation: "none"` (a second narrator is optional for texture).

## Speaker blueprint (voice.design instructs)

| id | role | gender | voice_design_instruct |
|---|---|---|---|
| `narrator` | Documentary Narrator | auto | "calm authoritative documentary narrator, deep measured voice, serious but warm undertone" |

## Emote vocabulary

`neutral`, `grave`, `hopeful`, `somber`

## Correlated defaults

- `default_speed: 0.95`, `default_temperature: 0.75` (steady, serious delivery)
- `reaction_memes: false` · `sticker_mode: "none"` · `music_mood: "calm"`

## Worked example skeleton

```json
{
  "schema": "openscript-video/v1",
  "title": "<topic> — Documentary",
  "format": {"type": "documentary", "alternation": "none",
             "default_speed": 0.95, "default_temperature": 0.75,
             "reaction_memes": false, "sticker_mode": "none",
             "music_mood": "calm"},
  "tts": {"backend": "voicedesign"},
  "speakers": {
    "narrator": {"voice": "documentary_narrator", "gender": "auto", "position": "top-left"}
  },
  "background": {"type": "procedural", "change_cadence": "scene"},
  "stickers": {"enabled": false},
  "scenes": [
    {"speaker": "narrator", "text": "There's a story hidden inside <topic> that almost nobody was told.", "emote": "neutral"},
    {"speaker": "narrator", "text": "Chapter one: the evidence nobody collected. It was there all along.", "emote": "grave"},
    {"speaker": "narrator", "text": "Chapter two: the institutions that looked away, and why they did.", "emote": "somber"},
    {"speaker": "narrator", "text": "Chapter three: the people who saw it clearly anyway — and what they did next.", "emote": "hopeful"},
    {"speaker": "narrator", "text": "The conclusion writes itself: <topic> was never one story. It was a thousand.", "emote": "neutral"}
  ]
}
```
