---
name: content-format-romcom
description: >
  Use for romantic-comedy content: two leads (M/F) moving through
  meet-cute → banter → tension → warm resolution. Chemistry comes from the
  emote PAIRS (flirty↔shy, nervous↔tender). Alternate every scene.
metadata: { "tags": "content-format, romcom, romance, two-leads, banter, alternation" }
---

# Romcom format playbook

## Anatomy (scene structure)

```
1-2 Meet-cute      — the two leads collide (one scene each)
3-5 Banter         — playful escalation, wit exchange (M/F alternating)
6   Tension beat   — a misunderstanding or stakes reveal (SHORT)
7-8 Resolution     — warm, sincere close
```

- **Two leads only** (male + female). Alternate every scene.
- Lines 6–10s. The tension beat stays short so the warm resolution lands.
- `format.alternation: "male_female"`.

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

## Worked example skeleton

```json
{
  "schema": "openscript-video/v1",
  "title": "Romcom — <topic>",
  "format": {"type": "romcom", "alternation": "male_female",
             "default_speed": 1.0, "default_temperature": 0.9,
             "reaction_memes": false, "sticker_mode": "character",
             "music_mood": "calm"},
  "tts": {"backend": "voicedesign"},
  "speakers": {
    "lead_m": {"voice": "romcom_lead_m", "gender": "male",   "position": "top-left"},
    "lead_f": {"voice": "romcom_lead_f", "gender": "female", "position": "top-right"}
  },
  "background": {"type": "procedural", "change_cadence": "speaker"},
  "scenes": [
    {"speaker": "lead_m", "text": "I wasn't looking for <topic> that day. Nobody ever is.", "emote": "nervous"},
    {"speaker": "lead_f", "text": "And yet there we were — me, a coffee, and the worst pickup line I'd ever heard.", "emote": "amused"},
    {"speaker": "lead_m", "text": "Worst? That line was carefully workshopped.", "emote": "flirty"},
    {"speaker": "lead_f", "text": "By whom? A committee of pigeons?", "emote": "shy"},
    {"speaker": "lead_m", "text": "Okay, fair. But you laughed.", "emote": "hopeful"},
    {"speaker": "lead_f", "text": "I laughed AT you. There's a difference.", "emote": "amused"},
    {"speaker": "lead_m", "text": "Still a laugh. That's the first date sorted.", "emote": "sincere"},
    {"speaker": "lead_f", "text": "Fine. One coffee. And you're telling me the real story behind <topic>.", "emote": "tender"}
  ]
}
```
