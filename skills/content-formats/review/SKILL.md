---
name: content-format-review
description: >
  Use for review / verdict content: one narrator evaluates a SINGLE subject
  with context → pros → cons → verdict → rating out of ten. Conversational,
  opinionated, decisive.
metadata: { "tags": "content-format, review, critique, verdict, rating, pros-cons" }
---

# Review format playbook

> **≠ listicle:** a review covers ONE subject with pros/cons + a rating — a
> listicle enumerates N ranked things. **≠ how_to:** evaluates, doesn't
> instruct.

## Anatomy (scene structure)

```
1  Hook        — "Today I'm reviewing X — and after three weeks, I have a verdict"
2  Setup       — what it is, who it's for, what it promises
3  Pros        — what it nails
4  Cons        — friction, gaps, price
5  Verdict     — binary: "worth it / skip it"
6  Rating      — the score out of ten
```

- Give a rating out of ten in the final line; keep the verdict binary.
- Contract: exactly 1 speaker, 5–9 scenes (`script.format.validate` enforces it).

## Speaker blueprint (voice.design instructs)

| id | role | gender | voice_design_instruct |
|---|---|---|---|
| `narrator` | Reviewer | auto | "engaging reviewer, clear opinionated delivery, conversational confident voice" |

## Emote vocabulary

`neutral`, `impressed`, `skeptical`, `decisive`

## Correlated defaults

- `default_speed: 1.0`, `default_temperature: 0.85`
- `reaction_memes: false` · `sticker_mode: "character"` · `music_mood: "neutral"`

## Authoring loop

1. Fetch the canonical playbook + worked-example draft:
   `director.format {type: "review", topic: "<topic>"}` or
   `openscript video new --format review --topic "<topic>"` — the registry is
   the single source of truth; don't hand-copy JSON from this file.
2. Design `review_narrator` with `voice.design`.
3. Write context, pros/cons, the binary verdict, and the rating close.
4. `script.parse` → `script.format.validate` → fix issues → `script.to_video`.
