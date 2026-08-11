---
name: content-format-comedy-sketch
description: >
  Use for comedy-sketch content: a deadpan straight man and an animated comic,
  setup → escalation → punchline, with a reaction meme GIF landing ON the
  punchline. Two speakers, alternating (M/F recommended).
metadata: { "tags": "content-format, comedy, sketch, punchline, reaction-meme, alternation" }
---

# Comedy Sketch format playbook

> **≠ meme_reel:** sketch is a TWO-speaker setup→escalation→punchline arc with
> a reaction meme landing on the punchline beat. Meme_reel is a solo
> rapid-fire narrator.

## Anatomy (scene structure)

```
1  Setup (straight)      — deadpan premise ("So you're saying X is serious?")
2  Escalation beat 1     — comic takes it to absurdity
3  Escalation beat 2     — comic doubles down
4  Escalation beat 3     — comic goes fully unhinged
5  Punchline (comic)     — SHORT (3-5s), clipped → reaction meme lands HERE
6  Button (straight)     — one dry line to close
```

- Setup lines 5–8s; **punchline clipped at 3–5s**. The contrast between the
  two voices IS the comedy: deadpan vs. big energy.
- The punchline scene carries `emote: "surprised"` (or `"excited"`).
- Contract: 2–3 speakers, 6–10 scenes (`script.format.validate` enforces it).

## Speaker blueprint (voice.design instructs)

| id | role | gender | voice_design_instruct |
|---|---|---|---|
| `straight` | Straight Man | male | "deadpan comedic straight man, dry male voice, monotone under pressure" |
| `comic` | Comic | female | "energetic comedian, animated female voice, quick delivery, big energy" |

## Emote vocabulary

`deadpan`, `exasperated`, `flat`, `resigned` (straight) ·
`excited`, `shocked`, `triumphant`, `mocking` (comic)

## Correlated defaults

- `default_speed: 1.05`, `default_temperature: 0.9`
- `reaction_memes: true` (punchline reaction GIFs are the payoff)
- `sticker_mode: "character"` · `music_mood: "energetic"`

## Authoring loop

1. Fetch the canonical playbook + worked-example draft:
   `director.format {type: "comedy_sketch", topic: "<topic>"}` or
   `openscript video new --format comedy_sketch --topic "<topic>"` — the
   registry is the single source of truth; don't hand-copy JSON from this file.
2. Design `comedy_straight` + `comedy_comic` with `voice.design`.
3. Write short setup lines, clipped punchlines, and put the reaction meme on
   the punchline scene.
4. `script.parse` → `script.format.validate` → fix issues → `script.to_video`.
