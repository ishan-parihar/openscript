---
name: content-format-how-to
description: >
  Use for how-to / tutorial / instructional content: one narrator walking
  through numbered, actionable steps with direct commands. Encouraging,
  practical delivery. Follow this playbook when the user wants a tutorial,
  a how-to, a walkthrough, a guide, or step-by-step instructions.
metadata: { "tags": "content-format, how-to, tutorial, instructional, steps, guide" }
---

# How-To / Tutorial format playbook

> **≠ presentation:** how_to is NUMBERED actionable steps with direct commands
> ("Step one: do X"). Presentation persuades; how_to instructs.

## Anatomy (scene structure)

```
1  Hook        — what you'll learn and why it matters ("no fluff, just the steps")
2-5 Steps      — 3-6 numbered steps, each ONE concrete action
6  Recap       — chain the steps into a single action sentence
```

- **Number the steps out loud in the text** ("Step one: ...", "Step two: ...")
  so the structure is audible, not just visual.
- Each step is one concrete action; 8-12s per line.
- Contract: exactly 1 speaker, 4–9 scenes (`script.format.validate` enforces it).

## Speaker blueprint (voice.design instructs)

| id | role | gender | voice_design_instruct |
|---|---|---|---|
| `narrator` | Instructor | auto | "clear instructional guide voice, direct and encouraging, friendly measured delivery" |

## Emote vocabulary

`neutral`, `confident`, `encouraging`, `emphatic`

## Correlated defaults

- `default_speed: 1.0`, `default_temperature: 0.8`
- `reaction_memes: false` (keep focus on the steps)
- `sticker_mode: "character"` · `music_mood: "neutral"`

## Authoring loop

1. Fetch the canonical playbook + worked-example draft:
   `director.format {type: "how_to", topic: "<topic>"}` or
   `openscript video new --format how_to --topic "<topic>"` — the registry is
   the single source of truth; don't hand-copy JSON from this file.
2. Design `how_to_narrator` with `voice.design`.
3. Write the hook, numbered steps (each one concrete action), and a recap that
   chains the steps into one sentence.
4. `script.parse` → `script.format.validate` → fix issues → `script.to_video`.
