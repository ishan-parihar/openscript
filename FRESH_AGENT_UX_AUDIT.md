# OpenScript Fresh-Agent UX Audit Report

**Date:** 2026-07-06
**Method:** Deployed a fresh general-purpose AI agent with NO prior context, told only:
  > "Create a short vertical video (under 30 seconds, 9:16) on the topic: '3 daily practices to rewire your nervous system'. The CLI is at /home/z/my-project/openscript/target/debug/openscript. Produce an MP4 and report your experience."

**Outcome:** Agent produced a 25.09s healing video at `healing_nervous_system.mp4` (Kokoro TTS, 5 calming procedural backgrounds, meditation music bed with ducking, warm-gold word-highlight captions). Render time 1m51s.

**Headline finding:** The from-scratch golden trajectory (`script.parse` → `script.to_video`) genuinely works — the agent reached a validated, rendering script within ~6 minutes of landing cold. But five specific UX gaps degraded both the **trustworthiness** of the output (silent warnings) and the **healing efficacy** of the defaults (neon-green captions, single loud font, randomly-sampled background pool mixing calm and neon clips).

---

## 1. The Five UX Gaps (Ranked by Impact)

### GAP #1 — Silent warning failure (CRITICAL — TRUST)
**What happened:** Whisper force-alignment failed on every scene (`ModuleNotFoundError: No module named 'whisper'`). The render logged this to stderr 5 times. But the final JSON result had `"warnings": null` — the API **lied about warnings**. A fresh agent who didn't scrape stderr would believe captions were perfectly aligned.

**Why this matters:** The single most important UX contract is "the response tells the truth." If `warnings: null` can hide 5 silent failures, every downstream agent decision based on `warnings` is unreliable. The agent trusted the JSON, shipped the video, and only noticed the alignment was estimated because it happened to read stderr.

**Fix:** `script.to_video` must collect stderr-emitted warnings (whisper failure, pexels fallback, music synthetic, etc.) into the `warnings` array of the response. Every `tracing::warn!` or `eprintln!` in the orchestrator's call tree should add to this array.

### GAP #2 — No mood tags on backgrounds (CRITICAL — EFFICACY)
**What happened:** `mcp/assets/backgrounds/` mixes calming clips (`procedural_particles_blue`, `procedural_aurora_green`, `procedural_waves_teal`, `procedural_bokeh_warm`) with jarring ones (`procedural_tunnel_neon`, `procedural_geometric_dark`, `procedural_gradient_rainbow`). `music_index.json` tags every track with `mood` and `energy`; backgrounds have **no such index**. The default procedural path picks *all* `.mp4`s in the folder — so a healing video can randomly get a neon tunnel behind it.

**Why this matters:** Music has a mood index → agents can filter `mood:"calm"` and get appropriate results. Backgrounds don't → agents must curate `fallback_pool` by hand-reading filenames, or read source code to know which clips are calming. This is the single biggest "I had to read source code to avoid a tonal landmine" moment in the audit.

**Fix:** Build `mcp/assets/backgrounds_index.json` mirroring `music_index.json`'s schema (`filename`, `mood`, `energy`, `motion_intensity`, `palette`, `description`). Add a `background.search` MCP tool that filters by mood/energy. Make `script.to_video` filter the procedural pool by `mood:"calm"` when the script's overall tone (inferred from voice speed < 1.0 + calm music) suggests a calming video — or accept an explicit `mood` field on `BackgroundSpec`.

### GAP #3 — `loop_` serde field silently swallows `"loop"` (HIGH — SILENT FAILURE)
**What happened:** The Rust struct field is `loop_` (Rust keyword collision), with no `#[serde(rename = "loop")]`. The agent wrote `"loop": true` in JSON; serde silently ignored the unknown key and used the default. The agent only noticed because the parse-output echoed `"loop_": true`.

**Why this matters:** A fresh agent writing `"loop": false` to disable looping would get `true` with **zero feedback**. This is exactly the class of bug the audit's "verify responses tell the truth" principle exists to catch. It's a one-line fix per field, but the pattern likely affects every Rust-keyword-collision field in the codebase (`type_`, `match_`, etc.).

**Fix:** Add `#[serde(alias = "loop")]` (or `#[serde(rename = "loop")]`) to `BackgroundSpec::loop_`. Audit all `_*` fields in `openscript-core` for the same pattern. Add a test that round-trips every keyword-collision field through JSON.

### GAP #4 — Defaults tuned for meme/edu content, fight healing topics (HIGH — EFFICACY)
**What happened:** Defaults the agent had to push back on:
- Caption highlight color: `#00ff88` (neon green — "gaming highlight" aesthetic)
- Only bundled font: Bebas Neue all-caps (loud, sporty)
- Default stickers: `enabled: true`, `lip_sync: "amplitude"`, `default_person` preset — a cartoon puppet narrating nervous-system regulation
- No "breath / pause" primitive — every scene is continuous speech

**Why this matters:** The user's stated objective is "accelerate evolution and healing in the individual." The tool's defaults are calibrated for a different audience (TikTok edu-shorts, gaming recaps). A fresh agent who trusts the defaults will produce something tonally wrong for healing content.

**Fix:** Add a `theme: "calm" | "energetic" | "neutral"` field to `OutputSpec` that presets caption color, font choice, sticker disable, and caption style. Provide a `caption_styles` visual cheat-sheet (4 thumbnail PNGs) in `AGENT_GUIDE.md` so agents can see what `word_highlight` vs `sentence_fade` vs `karaoke_fill` vs `subtitle_rail` actually look like. Add a `pause_ms` field to scenes for silent beats.

### GAP #5 — `verify.*` tools have no CLI mirrors (MEDIUM — DISCOVERY)
**What happened:** `AGENT_GUIDE.md` documents `verify.audio`, `verify.captions`, `verify.render` as MCP tools. But the CLI only mirrors the script-flow subcommands (`script-parse`, `script-generate-voices`, `script-build-captions`, `background-fetch`, `sticker-load-preset`, `sticker-render`, `script-to-timeline`, `script-to-video`). To run a post-render caption-timing verify from the CLI, the agent had to fall back to `ffprobe` manually.

**Why this matters:** A fresh agent using the CLI (the documented first-contact path) cannot verify its output without dropping into MCP protocol. This breaks the "one CLI binary does everything" mental model.

**Fix:** Add `verify-audio`, `verify-captions`, `verify-render` CLI subcommands mirroring the MCP tools. (Also: `list-tools`, `system-capabilities`, `help-tool` would be valuable CLI mirrors for fresh-agent discovery.)

### GAP #6 (BONUS) — Long-lived Kokoro sidecar restarted per scene (HIGH — PERFORMANCE)
**What happened:** The agent saw `Kokoro long-lived sidecar started (eliminates per-call cold-start)` **5 times** — once per scene. Either the sidecar isn't actually persisting across scenes, or `script.generate_voices` is calling `synth_one` in a way that bypasses the shared sidecar pool. This cost ~35s of the 1m51s render (5 × ~7s cold-start).

**Why this matters:** We built the long-lived sidecar specifically to eliminate this overhead (Phase AF). If it's not actually being used, the perf win is lost. The log message is also misleading — it says "started" when it should say "reusing."

**Fix:** Trace the call path from `script.generate_voices` → `KokoroClient::generate` → `KokoroEngine::synth_one` → `acquire_or_init`. Confirm the `SharedSidecar` Arc is the same instance across calls. The likely bug: each `KokoroClient` clones the config but the engine `OnceCell` is per-client, so each scene creates a fresh `KokoroEngine` with a fresh `SharedSidecar`.

---

## 2. What Worked Well (Keep These)

- **`openscript --help` is genuinely good.** One-liner per subcommand, immediately surfaces `script-to-video` as "One-call from-scratch video creation." This single line set the agent's entire trajectory.
- **`AGENT_GUIDE.md` is the right shape.** Tool taxonomy table, golden trajectories A–D, complete annotated script JSON example. The agent never needed to open the 39KB `AGENTS.md`.
- **`my_video_script.json` example** ("Octopus Facts in 12 Seconds") — copying its shape and swapping content was trivial.
- **`script.parse` returns the parsed script with defaults applied** — the agent could verify its intent was correctly understood before rendering.
- **`timeline_preview` tree in the response** — token-efficient, readable in one glance, shows all layers + timing.
- **The 2-tool golden trajectory (`script.parse` → `script.to_video`)** is genuinely achievable cold. The agent did it in ~6 minutes.

---

## 3. YAGNI Cuts (Features That Got in the Way)

- **SVG puppet sticker system** (default_person / robot / cat presets with mouth shapes, emotes, lip-sync modes, idle bob) — large, well-built feature that fights most non-meme use cases. For healing content it's actively wrong. Consider: (a) make `stickers.enabled` default to `false`, (b) split sticker presets into "meme" vs "minimalist" categories, (c) document the aesthetic mismatch explicitly.
- **Three render engines** (ffmpeg multilayer / HyperFrames / Remotion) — for a 25-second talking-caption video, the agent shouldn't need to evaluate three engines. The default (`ffmpeg`) is correct for 90% of cases. Consider: hide HyperFrames/Remotion behind an "advanced" section in `AGENT_GUIDE.md`.
- **`background.source` field** ("youtube") — only meaningful when `type=="gameplay"`. Sits on every background config doing nothing for procedural/static. Remove or make conditional.
- **`schema` version field** — auto-defaulted, never validated. Cargo-cult.

---

## 4. Concrete Recommendations (Prioritized)

| # | Recommendation | Effort | Impact |
|---|---|---|---|
| 1 | Collect stderr warnings into `script.to_video` response `warnings` array | Low | Critical (trust) |
| 2 | Build `backgrounds_index.json` with mood/energy tags + `background.search` tool | Medium | Critical (efficacy) |
| 3 | Add `#[serde(alias = "loop")]` to `loop_` field; audit all `_*` fields | Low | High (silent failure) |
| 4 | Add `theme: "calm"\|"energetic"\|"neutral"` preset to `OutputSpec` | Medium | High (efficacy) |
| 5 | Add CLI mirrors for `verify-*`, `system-capabilities`, `help-tool` | Low | Medium (discovery) |
| 6 | Fix Kokoro sidecar persistence across scenes | Medium | High (performance) |
| 7 | Add `pause_ms` field to scenes for silent beats | Low | Medium (efficacy) |
| 8 | Add caption-style visual cheat-sheet to `AGENT_GUIDE.md` | Low | Medium (discovery) |
| 9 | Make `stickers.enabled` default to `false` | Trivial | Medium (efficacy) |
| 10 | Hide HyperFrames/Remotion behind "advanced" section in guide | Trivial | Low (YAGNI) |

---

## 5. Healing Efficacy Verdict

**The tool allowed the agent to make something genuinely calming, but only because the agent pushed back on defaults.** A less careful agent would have shipped:
- Neon-green caption highlights (default)
- A cartoon puppet narrating (default stickers.on)
- Random background pool including neon tunnels (default procedural)
- Loud all-caps Bebas Neue (only font)
- Continuous speech with no breath pauses (no primitive for it)

The defaults are not malicious — they're tuned for the most common TikTok use case (edu-shorts, gaming recaps). But the user's stated objective ("accelerate evolution and healing in the individual") requires defaults that serve *calm* content by default, or at minimum a one-field way to opt into a calming preset.

**The single highest-leverage change is GAP #4: a `theme: "calm"` preset** that flips caption color to warm gold, disables stickers, picks sentence_fade over word_highlight, and filters the background pool to mood:calm. One field, four correlated defaults, healing-tonal correctness.

---

## 6. Next Steps

This report kicks off an iterative cycle:
1. Fix GAPs #1, #2, #3, #6 (trust + efficacy + performance) — commit each as a separate phase
2. Add `theme` preset (GAP #4) — the single highest-leverage efficacy change
3. Add CLI mirrors for `verify.*` (GAP #5)
4. Re-run the fresh-agent simulation with the same prompt
5. Compare the second agent's experience + video quality against the first
6. Iterate until the fresh agent can produce a healing-tonal video *without* pushing back on defaults
