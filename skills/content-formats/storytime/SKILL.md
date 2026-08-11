---
name: content-format-storytime
description: >
  Use for first-person personal narrative content: hook → setup → rising
  tension → turning point → lesson. One intimate storyteller with an emotional
  arc. Follow this playbook for personal stories, origin stories, and
  first-person lessons.
metadata: { "tags": "content-format, storytime, narrative, personal-story, first-person, arc" }
---

# Storytime format playbook

> **≠ documentary:** storytime is a FIRST-PERSON lived journey with an
> emotional arc — documentary is third-person evidence with chapter markers.

## Anatomy (scene structure)

```
1  Hook          — grab in one line with a concrete, odd detail ("I had forty dollars and a terrible plan")
2  Setup         — how it started, the warning signs ignored
3  Rising tension — things go sideways, scene by scene
4  Turning point — the peak (present tense works best here)
5  Lesson        — one honest reflection; past tense frame
```

- Open with a concrete odd detail; build tension scene by scene.
- Peak at the turning point, then land one honest lesson.
- Contract: exactly 1 speaker, 5–9 scenes (`script.format.validate` enforces it).

## Speaker blueprint (voice.design instructs)

| id | role | gender | voice_design_instruct |
|---|---|---|---|
| `narrator` | Storyteller | auto | "warm storytelling narrator, intimate conversational delivery, slight dramatic range" |

## Emote vocabulary

`neutral`, `thoughtful`, `surprised`, `sincere`

## Correlated defaults

- `default_speed: 0.98`, `default_temperature: 0.85`
- `reaction_memes: false` · `sticker_mode: "character"` · `music_mood: "calm"`

## Authoring loop

1. Fetch the canonical playbook + worked-example draft:
   `director.format {type: "storytime", topic: "<topic>"}` or
   `openscript video new --format storytime --topic "<topic>"` — the registry
   is the single source of truth; don't hand-copy JSON from this file.
2. Design `storytime_narrator` with `voice.design`.
3. Write the hook, escalating tension, turning point, and honest lesson.
4. `script.parse` → `script.format.validate` → fix issues → `script.to_video`.
