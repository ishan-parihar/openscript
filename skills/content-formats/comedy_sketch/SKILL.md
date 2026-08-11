---
name: content-format-comedy-sketch
description: >
  Use for comedy-sketch content: a deadpan straight man and an animated comic,
  setup → escalation → punchline, with a reaction meme GIF landing ON the
  punchline. Two speakers, alternating (M/F recommended).
metadata: { "tags": "content-format, comedy, sketch, punchline, reaction-meme, alternation" }
---

# Comedy Sketch format playbook

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
- The punchline scene carries `emote: "surprised"` (or `"excited"`) and the
  reaction meme pops in on that exact scene.
- `format.alternation: "male_female"`.

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

## Worked example skeleton

```json
{
  "schema": "openscript-video/v1",
  "title": "Sketch — <topic>",
  "format": {"type": "comedy_sketch", "alternation": "male_female",
             "default_speed": 1.05, "default_temperature": 0.9,
             "reaction_memes": true, "sticker_mode": "character",
             "music_mood": "energetic"},
  "tts": {"backend": "voicedesign"},
  "speakers": {
    "straight": {"voice": "comedy_straight", "gender": "male",   "position": "top-left"},
    "comic":    {"voice": "comedy_comic",    "gender": "female", "position": "top-right"}
  },
  "background": {"type": "procedural", "change_cadence": "speaker"},
  "meme_brolls": {"enabled": true},
  "scenes": [
    {"speaker": "straight", "text": "So you're telling me <topic> is a serious problem?", "emote": "deadpan"},
    {"speaker": "comic",    "text": "Serious? It's a CRISIS. A man lost his entire morning to it.", "emote": "excited"},
    {"speaker": "comic",    "text": "Simple. We ban <topic>, and if that fails, we blame the weather.", "emote": "triumphant"},
    {"speaker": "straight", "text": "That is not a solution.", "emote": "flat"},
    {"speaker": "comic",    "text": "It's better than what the experts came up with.", "emote": "shocked"}
  ]
}
```
