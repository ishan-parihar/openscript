---
name: content-format-podcast
description: >
  Use for podcast-style content: a host and one or more guests in alternating
  conversation turns. Recommended alternation: male host + female guest (or
  vice versa), switching every scene. Follow this playbook when the user wants
  a podcast, a conversation, a talk-show, or a discussion-style video.
metadata: { "tags": "content-format, podcast, conversation, host-guest, alternation" }
---

# Podcast format playbook

## Anatomy (scene structure)

```
1  Hook (host)         — one line that earns the next 60 seconds
2  Intro (host)        — what today's episode is about
3-9 Topic rounds       — Q/A pairs: host asks, guest answers (3-5 rounds)
10 Takeaway (guest)    — the one-line summary
11 CTA (host)          — follow / next episode tease
```

- Scenes alternate host/guest. **Never give one speaker 3+ consecutive scenes.**
- Lines are 8–15 seconds of speech (2–4 sentences).
- `format.alternation: "male_female"`.

## Speaker blueprint (voice.design instructs)

| id | role | gender | voice_design_instruct |
|---|---|---|---|
| `host` | Host | male | "warm, energetic podcast host, natural conversational cadence, bright male voice" |
| `guest` | Guest | female | "calm, articulate podcast guest, thoughtful female voice, measured and precise" |

Swap genders for variety (female host / male guest). For 3-4 speakers add a
`cohost` / `second_guest` with a distinct instruct (different age/pitch).

## Emote vocabulary

`warm`, `curious`, `amused`, `serious` (host) · `thoughtful`, `excited`,
`sincere`, `surprised` (guest)

## Correlated defaults

- `default_speed: 1.02`, `default_temperature: 0.88` (set via format block)
- `reaction_memes: true` (reaction GIFs at punchline moments)
- `sticker_mode: "character"` (each speaker gets a character sticker, L/R)
- `music_mood: "energetic"` · `background.change_cadence: "speaker"`

## Worked example skeleton

```json
{
  "schema": "openscript-video/v1",
  "title": "Podcast — <topic>",
  "format": {"type": "podcast", "alternation": "male_female",
             "default_speed": 1.02, "default_temperature": 0.88,
             "reaction_memes": true, "sticker_mode": "character",
             "music_mood": "energetic"},
  "tts": {"backend": "voicedesign"},
  "speakers": {
    "host":  {"voice": "podcast_host",  "gender": "male",   "position": "top-left"},
    "guest": {"voice": "podcast_guest", "gender": "female", "position": "top-right"}
  },
  "background": {"type": "procedural", "change_cadence": "speaker"},
  "scenes": [
    {"speaker": "host",  "text": "Welcome to the show — today we're digging into <topic>.", "emote": "warm"},
    {"speaker": "guest", "text": "Honestly? The real story behind <topic> is much stranger.", "emote": "thoughtful"},
    {"speaker": "host",  "text": "Where does <topic> actually begin?", "emote": "curious"},
    {"speaker": "guest", "text": "It starts with a handful of people noticing something that didn't fit.", "emote": "excited"},
    {"speaker": "host",  "text": "And the takeaway for our listeners?", "emote": "serious"},
    {"speaker": "guest", "text": "One line: <topic> isn't what we were told.", "emote": "sincere"}
  ]
}
```

Design `podcast_host` + `podcast_guest` with `voice.design` first (instructs
above), then run `script.parse` → `script.format.validate` → `script.to_video`.
