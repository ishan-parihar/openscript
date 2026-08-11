# OpenScript install (cold-start → production-ready)

One-page path from `git clone` to a real stock short (not gradient drafts).

## 1. Clone & toolchain

```bash
git clone https://github.com/ishan-parihar/openscript.git
cd openscript
# Needs: rustc/cargo, python3, ffmpeg, ffprobe, yt-dlp, node (optional for HF)
```

## 2. API keys (never commit)

```bash
export PEXELS_API_KEY=…          # required for production multi-broll
export GIPHY_API_KEY=…           # stickers (optional but recommended)
export OPENROUTER_API_KEY=…      # optional vision cascade
export PIXABAY_API_KEY=…         # optional music/video

bash scripts/setup_openscript_config.sh
# writes ~/.openscript/config.json mode 0600
```

Template: `openscript.env.example`. Full media plan: `docs/INSTALL_MEDIA_DEPS_PLAN.md`.

## 3. Bootstrap (feature-gated)

`setup.sh` provisions **only the deps for the features that are active** in
`~/.openscript/config.json` (`features.<category>.<name>`, all default ON), or
per-run env overrides. Toggle before installing to skip big downloads:

```bash
bash setup.sh --list-features               # print the toggle table + what each pulls
OPENSCRIPT_FEATURE_TTS_VOICEDESIGN=0 bash setup.sh   # skip the 4.3GB VoiceDesign model
bash setup.sh --feature tts.gepard=0        # skip the heavy CUDA/NeMo gepard venv
bash setup.sh --feature transcription.parakeet_align=0   # skip the 320MB alignment model

# Persist toggles in the config so they apply on every install:
bash scripts/setup_openscript_config.sh --feature tts.voicedesign=0 --feature tts.gepard=0

bash setup.sh                    # now installs only the enabled deps
bash scripts/bootstrap_media.sh  # doctor + optional --with-library
```

The same toggles gate the RUNTIME: a disabled engine/tool returns a clear error
naming the toggle + setup command, and `system.capabilities` reports each
feature's `enabled` state alongside its availability. See `openscript.env.example`
for the full `OPENSCRIPT_FEATURE_<CATEGORY>_<NAME>` list.

Or after build:

```bash
# MCP tool
system.doctor   # ready_for_production + next_actions
```

## 4. What ships in-repo for cold-start

| Asset | Path | Role |
|-------|------|------|
| Production music beds | `mcp/assets/music_production/` | Offline calm/focus beds (not synthetic sine stubs) |
| Portable SFX pack | `mcp/assets/sfx_pack/` + `sfx_index.json` | Relative paths, works on any machine |
| Procedural backgrounds | `mcp/assets/backgrounds/` | Draft/fallback only |
| Kokoro | downloaded by `setup.sh` | TTS |

## 5. First video

```text
system.doctor  →  script.parse  →  director.run / script.to_video
```

**Expect:** Pexels portrait clips when key present; music from `music_production` or `library.build`; SFX whooshes from portable pack.

**Fail-closed:** if ≥50% backgrounds are procedural and `OPENSCRIPT_ALLOW_PROCEDURAL` is unset, output is `*.draft.mp4` with status `draft` (not production success).

## 6. Optional upgrades

```bash
bash scripts/bootstrap_media.sh --with-library   # tagged YT music index (~2 min)
# Large local SFX:
OPENSCRIPT_SFX_PATH=$HOME/Videos/Assets/SFX  # then sfx.index
```

## 7. Security

- Secrets only in env or `~/.openscript/config.json` (0600)
- Never commit filled env files or real keys
- Rotate keys if they were pasted into chat logs
