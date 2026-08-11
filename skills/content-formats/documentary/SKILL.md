---
name: content-format-documentary
description: >
  Use for serious, measured long-form content: a documentary-style narrator
  walking through evidence chapters. Longer sentences, lower temperature,
  calm music, minimal stickers. Authoritative and somber.
metadata: { "tags": "content-format, documentary, narration, evidence, chapters, measured" }
---

# Documentary format playbook

> **≠ presentation:** documentary is a CHAPTERED long-form evidence narrative —
> 3-5 clause lines, grave/somber emotes, no stickers, calm music. Presentation
> is the short persuasive explainer.

## Anatomy (scene structure)

```
1  Opening thesis     — "There's a story hidden inside X that nobody was told"
2-5 Evidence chapters — Chapter one / two / three ... (each one idea)
6  Synthesis          — the conclusion that ties the chapters together
7  Closing reflection — a lingering, thoughtful end
```

- 1–2 narrators. Longer sentences (3–5 clauses), slower pacing.
- Chapter markers in the text keep the structure audible.
- Contract: 1–2 speakers, 5–8 scenes (`script.format.validate` enforces it).

## Speaker blueprint (voice.design instructs)

| id | role | gender | voice_design_instruct |
|---|---|---|---|
| `narrator` | Documentary Narrator | auto | "calm authoritative documentary narrator, deep measured voice, serious but warm undertone" |

## Emote vocabulary

`neutral`, `grave`, `hopeful`, `somber`

## Correlated defaults

- `default_speed: 0.95`, `default_temperature: 0.75` (steady, serious delivery)
- `reaction_memes: false` · `sticker_mode: "none"` · `music_mood: "calm"`

## Authoring loop

1. Fetch the canonical playbook + worked-example draft:
   `director.format {type: "documentary", topic: "<topic>"}` or
   `openscript video new --format documentary --topic "<topic>"` — the registry
   is the single source of truth; don't hand-copy JSON from this file.
2. Design `documentary_narrator` with `voice.design`.
3. Write the thesis, numbered chapters in the text, and a synthesis.
4. `script.parse` → `script.format.validate` → fix issues → `script.to_video`.
