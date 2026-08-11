---
name: content-format-listicle
description: >
  Use for listicle-style content: a ranked countdown with curiosity hooks
  ("5 signs you're X", "7 things nobody tells you about Y"). One narrator,
  numbered escalating items, the big reveal mid-list. Follow this playbook for
  listicles, top-N, signs-of, and ranked roundups.
metadata: { "tags": "content-format, listicle, top-list, ranked, signs-of, countdown" }
---

# Listicle format playbook

> **≠ how_to:** steps are DIRECTIVES to do; a listicle is observational ranked
> signs/things. **≠ presentation:** ranked countdown with a tease, not
> sequential points.

## Anatomy (scene structure)

```
1  Hook        — tease the count and the big one ("...and number three is the one nobody sees coming")
2-5 Ranked items — each one idea, escalating; number them OUT LOUD
6  Payoff      — "if N of these fit, here's what to do next"
```

- Number the items out loud ("Sign number one: ...", "Sign number two: ...").
- The middle item is the biggest reveal; the last item closes the loop.
- Contract: exactly 1 speaker, 6–10 scenes (`script.format.validate` enforces it).

## Speaker blueprint (voice.design instructs)

| id | role | gender | voice_design_instruct |
|---|---|---|---|
| `narrator` | Listicle Narrator | auto | "energetic listicle narrator, punchy curiosity-driven delivery, bright engaging voice" |

## Emote vocabulary

`curious`, `shocked`, `amused`, `emphatic`

## Correlated defaults

- `default_speed: 1.05`, `default_temperature: 0.85`
- `reaction_memes: false` · `sticker_mode: "character"` · `music_mood: "neutral"`

## Authoring loop

1. Fetch the canonical playbook + worked-example draft:
   `director.format {type: "listicle", topic: "<topic>"}` or
   `openscript video new --format listicle --topic "<topic>"` — the registry is
   the single source of truth; don't hand-copy JSON from this file.
2. Design `listicle_narrator` with `voice.design`.
3. Write the tease hook, numbered escalating items, and the payoff close.
4. `script.parse` → `script.format.validate` → fix issues → `script.to_video`.
