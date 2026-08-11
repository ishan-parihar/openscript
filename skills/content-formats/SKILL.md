---
name: content-formats
description: >
  Use this skill to choose and apply a CONTENT FORMAT to any script-to-video
  creation. The harness supports seven formats — presentation (default linear
  explainer), podcast (host + guest, M/F alternation), dialogue (interactive
  Q&A session), comedy_sketch (setup → punchline + reaction memes), romcom
  (two-lead beat structure), meme_reel (fast punchy takes + memes), and
  documentary (measured chapters). Each format shapes the SCRIPT the agent
  authors: speaker count, male/female alternation, pacing, reactions, music
  mood — before any rendering. Read this router first, pick the format that
  matches the user's intent, then load skills/content-formats/<format>/SKILL.md
  for the full playbook.
metadata: { "tags": "content, format, podcast, dialogue, comedy, romcom, meme, documentary, script-authoring, routing" }
---

# Content Formats — start here

OpenScript's `script.to_video` pipeline renders whatever a `ScriptSpec` says.
The **content format** (`format` block in the script, or `director.format`
via MCP / `openscript video new` via CLI) tells the agent HOW to author the
script: how many speakers, whether to alternate male/female voices, how long
the lines should be, where reaction memes land, and what music mood fits.

## Choosing a format

| User intent | Format | Speakers | Alternation |
|---|---|---|---|
| Explainer / one-narrator rundown | `presentation` | 1 | none |
| Two people talking about a topic | `podcast` | 2–4 | **male_female** |
| Interview / Q&A / interactive session | `dialogue` | 2 | **male_female** |
| Sketch comedy, jokes, punchlines | `comedy_sketch` | 2 | **male_female** |
| Love story with two leads | `romcom` | 2 | **male_female** |
| Short viral punchline reels | `meme_reel` | 1–2 | none |
| Serious, measured long-form | `documentary` | 1–2 | none |

## The alternation rule

For podcast / dialogue / comedy / romcom, alternate a **male and a female
voice every scene**. This is what makes multi-speaker content engaging and
stimulating. The speaker blueprint in each format skill gives you ready-to-use
`voice.design` personality strings (Qwen3 VoiceDesign synthesizes the voice
directly — no cloning needed). Set `format.alternation: "male_female"` in the
script so `script.format.validate` enforces it.

## Authoring loop

1. Load the format skill for the chosen format.
2. Get the speaker blueprint → design the voices with `voice.design`
   (or `character.create` + `character.design_emotion` for emotional range).
3. Write the script following the format's scene structure; alternate speakers.
4. `script.parse` → `script.format.validate` → fix issues → `script.to_video`.

## Quick reference

- MCP: `director.format {type, topic}` returns the full playbook JSON.
- CLI: `openscript video new --format podcast --topic "..."` scaffolds a draft.
- Schema: `format` block accepts `{type, alternation, min_speakers, max_speakers,
  min_scenes, max_scenes, default_speed, default_temperature, reaction_memes,
  sticker_mode, music_mood}` — all optional, all default-on only when the agent
  left the field unset.
