---
name: content-format-debate
description: >
  Use for adversarial "versus" content: two claimers argue opposite sides of a
  motion with direct rebuttals, then land a verdict. Recommended alternation:
  male claimant A + female claimant B, switching every scene.
metadata: { "tags": "content-format, debate, versus, opposition, motion, rebuttal" }
---

# Debate format playbook

> **≠ dialogue:** dialogue is COOPERATIVE Q&A (interviewer + expert). Debate is
> two OPPOSING claimers with rebuttals that answer the other side directly,
> ending in a verdict.

## Anatomy (scene structure)

```
1  Motion        — the claim being argued ("The motion tonight: X is the biggest problem we're ignoring")
2  Position A    — claimant A states the case
3  Rebuttal B    — claimant B answers directly
4-7 Escalation   — 2-3 more rounds; each rebuttal answers the LAST point
8  Verdict       — one side lands the closing claim
```

- Every scene alternates sides. Rebuttals address the other side's argument
  ("those numbers only work if...").
- Contract: 2–3 speakers, 8–14 scenes (`script.format.validate` enforces it).

## Speaker blueprint (voice.design instructs)

| id | role | gender | voice_design_instruct |
|---|---|---|---|
| `claim_a` | Claimant A (For) | male | "sharp articulate debater, confident male voice, persuasive crisp delivery" |
| `claim_b` | Claimant B (Against) | female | "sharp articulate debater, confident female voice, quick-witted and forceful" |

## Emote vocabulary

`confident`, `forceful`, `skeptical`, `resolved` (A) ·
`sharp`, `dismissive`, `emphatic`, `triumphant` (B)

## Correlated defaults

- `default_speed: 1.03`, `default_temperature: 0.9`
- `reaction_memes: true` (at the verdict) · `sticker_mode: "character"`
- `music_mood: "energetic"`

## Authoring loop

1. Fetch the canonical playbook + worked-example draft:
   `director.format {type: "debate", topic: "<topic>"}` or
   `openscript video new --format debate --topic "<topic>"` — the registry is
   the single source of truth; don't hand-copy JSON from this file.
2. Design `debate_claim_a` + `debate_claim_b` with `voice.design`.
3. Write the motion, alternating positions/rebuttals, and the verdict close.
4. `script.parse` → `script.format.validate` → fix issues → `script.to_video`.
