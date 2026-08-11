---
name: content-format-dialogue
description: >
  Use for interactive-session / interview / Q&A style content: an interviewer
  and an expert exchanging tight back-and-forth rounds. Two speakers ONLY,
  alternating every scene (male interviewer + female expert recommended).
metadata: { "tags": "content-format, dialogue, interview, q-and-a, interactive-session, alternation" }
---

# Dialogue (Interactive Session) format playbook

> **≠ podcast:** dialogue is the FORMAL interviewer/expert session — exactly 2
> speakers, short 5-10s lines, NO reaction memes, neutral music. Podcast is the
> informal 2-4 speaker roundtable with memes.

## Anatomy (scene structure)

```
1  Opening (interviewer)      — frame the session
2-9 Q/A rounds                — interviewer asks (ends with ?), expert answers
10 Closing (interviewer)      — actionable takeaway + thanks
```

- **Two speakers only.** Every other scene changes speaker.
- Lines are short: 5–10 seconds (1–3 sentences). Questions hand the turn back.
- Contract: exactly 2 speakers, 6–10 scenes (`script.format.validate` enforces it).

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

## Authoring loop

1. Fetch the canonical playbook + worked-example draft:
   `director.format {type: "dialogue", topic: "<topic>"}` or
   `openscript video new --format dialogue --topic "<topic>"` — the registry is
   the single source of truth; don't hand-copy JSON from this file.
2. Design `dialogue_interviewer` + `dialogue_expert` with `voice.design`.
3. Write Q/A rounds — every question ends with "?" and hands the turn back.
4. `script.parse` → `script.format.validate` → fix issues → `script.to_video`.
