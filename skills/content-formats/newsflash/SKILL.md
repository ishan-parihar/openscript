---
name: content-format-newsflash
description: >
  Use for urgent breaking-news style content: fast factual briefing with
  verification tiers (what we can verify vs what's unconfirmed), a status
  line, and a follow signal. Serious and neutral, no persuasion.
metadata: { "tags": "content-format, newsflash, breaking-news, urgent, briefing, factual" }
---

# Newsflash format playbook

> **≠ presentation:** no persuasion arc — urgency, verification tiers, and a
> follow signal. **≠ meme_reel:** serious and factual, no comedy.

## Anatomy (scene structure)

```
1  Hook        — "We're coming to you with a breaking update on X — here's what we know"
2-4 Updates    — 3-4 factual lines, short
5  Verify split— "what we can verify" vs "what's still unconfirmed"
6  What-next   — status + follow signal
```

- Label verification tiers out loud; keep lines short and factual.
- Contract: exactly 1 speaker, 3–7 scenes (`script.format.validate` enforces it).

## Speaker blueprint (voice.design instructs)

| id | role | gender | voice_design_instruct |
|---|---|---|---|
| `narrator` | News Anchor | auto | "urgent news anchor, crisp factual delivery, serious measured urgency" |

## Emote vocabulary

`neutral`, `grave`, `urgent`, `calm`

## Correlated defaults

- `default_speed: 1.08`, `default_temperature: 0.8`
- `reaction_memes: false` · `sticker_mode: "none"` · `music_mood: "neutral"`

## Authoring loop

1. Fetch the canonical playbook + worked-example draft:
   `director.format {type: "newsflash", topic: "<topic>"}` or
   `openscript video new --format newsflash --topic "<topic>"` — the registry
   is the single source of truth; don't hand-copy JSON from this file.
2. Design `newsflash_narrator` with `voice.design`.
3. Write the breaking hook, factual updates, verify-split, and follow signal.
4. `script.parse` → `script.format.validate` → fix issues → `script.to_video`.
