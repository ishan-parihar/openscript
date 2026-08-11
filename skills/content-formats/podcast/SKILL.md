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

> **≠ dialogue:** podcast is the INFORMAL 2-4 speaker entertainment roundtable
> with reaction memes on punchlines. Dialogue is the formal interviewer/expert
> Q&A with exactly 2 speakers and no memes.

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
- Contract: 2–4 speakers, 6–14 scenes (`script.format.validate` enforces it).

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

- `default_speed: 1.02`, `default_temperature: 0.88`
- `reaction_memes: true` (reaction GIFs at punchline moments)
- `sticker_mode: "character"` (each speaker gets a character sticker, L/R)
- `music_mood: "energetic"` · `background.change_cadence: "speaker"`

## Authoring loop

1. Fetch the canonical playbook + worked-example draft:
   `director.format {type: "podcast", topic: "<topic>"}` or
   `openscript video new --format podcast --topic "<topic>"` — the registry is
   the single source of truth; don't hand-copy JSON from this file.
2. Design `podcast_host` + `podcast_guest` with `voice.design` using the
   instructs above (they synthesize DIRECTLY on Qwen3 VoiceDesign — no cloning).
3. Fill scene texts with real substance on the topic, alternating speakers.
4. `script.parse` → `script.format.validate` → fix issues → `script.to_video`.
