---
name: content-format-dialogue
description: >
  Use for interactive-session / interview / Q&A style content: an interviewer
  and an expert exchanging tight back-and-forth rounds. Two speakers ONLY,
  alternating every scene (male interviewer + female expert recommended).
metadata: { "tags": "content-format, dialogue, interview, q-and-a, interactive-session, alternation" }
---

# Dialogue (Interactive Session) format playbook

## Anatomy (scene structure)

```
1  Opening (interviewer)      — frame the session
2-9 Q/A rounds                — interviewer asks (ends with ?), expert answers
10 Closing (interviewer)      — actionable takeaway + thanks
```

- **Two speakers only.** Every other scene changes speaker.
- Lines are short: 5–10 seconds (1–3 sentences). Questions hand the turn back.
- `format.alternation: "male_female"`.

## Speaker blueprint (voice.design instructs)

| id | role | gender | voice_design_instruct |
|---|---|---|---|
| `interviewer` | Interviewer | male | "engaging interviewer, curious male voice, quick and attentive" |
| `expert` | Expert | female | "warm expert voice, clear female delivery, precise and confident" |

## Emote vocabulary

`curious`, `playful`, `serious`, `reassuring` (interviewer) ·
`confident`, `thoughtful`, `emphatic`, `warm` (expert)

## Correlated defaults

- `default_speed: 1.0`, `default_temperature: 0.9` (higher temperature = more
  inflection — the back-and-forth needs it)
- `reaction_memes: false` (keep the focus on the exchange)
- `sticker_mode: "character"` · `music_mood: "neutral"`

## Worked example skeleton

```json
{
  "schema": "openscript-video/v1",
  "title": "Session — <topic>",
  "format": {"type": "dialogue", "alternation": "male_female",
             "default_speed": 1.0, "default_temperature": 0.9,
             "reaction_memes": false, "sticker_mode": "character",
             "music_mood": "neutral"},
  "tts": {"backend": "voicedesign"},
  "speakers": {
    "interviewer": {"voice": "dialogue_interviewer", "gender": "male",   "position": "top-left"},
    "expert":      {"voice": "dialogue_expert",      "gender": "female", "position": "top-right"}
  },
  "background": {"type": "procedural", "change_cadence": "speaker"},
  "scenes": [
    {"speaker": "interviewer", "text": "What's the one thing everyone gets wrong about <topic>?", "emote": "curious"},
    {"speaker": "expert",      "text": "That it's complicated. <topic> is simple once you see the pattern.", "emote": "confident"},
    {"speaker": "interviewer", "text": "Walk me through the pattern, piece by piece.", "emote": "serious"},
    {"speaker": "expert",      "text": "Step one: notice it. Step two: name it. Step three: test it.", "emote": "emphatic"},
    {"speaker": "interviewer", "text": "And if someone takes one action after this session?", "emote": "reassuring"},
    {"speaker": "expert",      "text": "Write your own one-line model of <topic> tonight — and change it when you know better.", "emote": "warm"}
  ]
}
```
