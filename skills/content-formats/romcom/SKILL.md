---
name: content-format-romcom
description: >
  Use for romantic-comedy content: two leads (M/F) moving through
  meet-cute → banter → tension → warm resolution. Chemistry comes from the
  emote PAIRS (flirty↔shy, nervous↔tender). Alternate every scene.
metadata: { "tags": "content-format, romcom, romance, two-leads, banter, alternation" }
---

# Romcom format playbook

> **≠ dialogue:** romcom is an EMOTIONAL beat structure (meet-cute→banter→
> tension→resolution) driven by emote pairs with calm music. Dialogue is
> intellectual Q&A.

## Anatomy (scene structure)

```
1-2 Meet-cute      — the two leads collide (one scene each)
3-5 Banter         — playful escalation, wit exchange (M/F alternating)
6   Tension beat   — a misunderstanding or stakes reveal (SHORT)
7-8 Resolution     — warm, sincere close
```

- **Two leads only** (male + female). Alternate every scene.
- Lines 6–10s. The tension beat stays short so the warm resolution lands.
- Contract: exactly 2 speakers, 8–10 scenes (`script.format.validate` enforces it).

## Speaker blueprint (voice.design instructs)

| id | role | gender | voice_design_instruct |
|---|---|---|---|
| `lead_m` | Male Lead | male | "charming romantic lead, warm sincere male voice, slight playful edge" |
| `lead_f` | Female Lead | female | "bright romantic lead, soft warm female voice, quick wit" |

## Emote vocabulary (the chemistry map)

`flirty`, `nervous`, `hopeful`, `sincere` (lead_m) ·
`shy`, `amused`, `warm`, `tender` (lead_f)

Pair them: `flirty` ↔ `shy`, `nervous` ↔ `tender`, `hopeful` ↔ `warm`.
The tension beat uses `nervous`/`serious` on both.

## Correlated defaults

- `default_speed: 1.0`, `default_temperature: 0.9`
- `reaction_memes: false` · `sticker_mode: "character"` · `music_mood: "calm"`

## Authoring loop

1. Fetch the canonical playbook + worked-example draft:
   `director.format {type: "romcom", topic: "<topic>"}` or
   `openscript video new --format romcom --topic "<topic>"` — the registry is
   the single source of truth; don't hand-copy JSON from this file.
2. Design `romcom_lead_m` + `romcom_lead_f` with `voice.design`.
3. Write the beat structure; keep the tension beat short; resolve warm.
4. `script.parse` → `script.format.validate` → fix issues → `script.to_video`.
