# Fresh Install: Media & Asset Dependencies Plan

**Status:** Implemented (Phase CO–CS foundations) — portable packs + fail-closed + doctor  
**Date:** 2026-07-16  
**Goal:** Every new OpenScript install goes from `git clone` → production-grade render without scavenger hunts or silent gradient “success.”

---

## 0. Executive summary

A cold clone can **build and render**, but production video still fails unless four independent layers are green:

| Layer | What it supplies | If missing |
|-------|------------------|------------|
| **System binaries** | ffmpeg, ffprobe, yt-dlp | No encode / no YT fallback |
| **ML models** | Kokoro ONNX, (opt) Parakeet, Ollama GGUF | No TTS / weak captions / no local LLM |
| **API keys** | Pexels, GIPHY, (opt) Pixabay, OpenRouter | Stock → procedural gradients; no stickers; no vision |
| **Local media indexes** | `music_library_index.json`, portable SFX pack | Silent / synthetic bed; no whooshes |

`setup.sh` already covers **binaries + Kokoro + cargo**. It does **not** own secrets, live API probes, tagged music index build, or portable SFX.

**This machine (2026-07-16):**

| Check | Result |
|-------|--------|
| Pexels key in `~/.openscript/config.json` | Set (0600) |
| GIPHY key | Set; **live search OK** |
| OpenRouter key | Preserved from prior config |
| Pexels live search | OK after **User-Agent** (raw urllib got Cloudflare **403 / 1010**) |
| `music_library_index.json` | **Missing** → music path falls to synthetic / empty |
| SFX index | Present but paths absolute under `$HOME/Videos/Assets/SFX` (works here, **not portable**) |
| `production_ready` (doctor) | **NO** until music library + (optional) portable SFX |

---

## 1. Brainstorm: why “clone and go” fails

### 1.1 Gradient “success” is the wrong failure mode

When Pexels is unset **or** every stock candidate fails lexical/geometry gates, `background.fetch` / multi-broll still **writes a procedural gradient MP4** and the pipeline continues. KPI v3 hard-fails majority procedural *after* the fact, but the agent still gets a file that looks like an output. Greenfield installs feel “broken but green.”

**Fix direction:** fail-closed for production trajectory unless `OPENSCRIPT_ALLOW_PROCEDURAL=1` (or status `"draft"` + `.draft.mp4`).

### 1.2 Keys are not bootstrap-first

Env vars work, but agents and humans forget them. Canonical store is:

```text
env  >  ~/.openscript/config.json (0600)  >  legacy mcp/assets/.openscript_config.json
```

Fresh install needs an explicit **config wizard / merge script**, not “hope you exported PEXELS_API_KEY.”

### 1.3 Music has two universes

| Source | Quality | Fresh install |
|--------|---------|---------------|
| `mcp/assets/music/*.mp3` (20 tracks) | Synthetic / short stock stubs | Committed; **not** production KPI-safe for calm/focus denylist |
| `music_library_index.json` + cache | Tagged YT/CC beds via `library.build` | **Not committed** (paths + size); must be built once |
| Pixabay | Optional | Needs key |

Without `library.build`, director runs often ship **no music** or denylist-failing stubs.

### 1.4 SFX is machine-local

`sfx_index.json` embeds absolute paths (`/home/…/Videos/Assets/SFX`). On a new machine those paths vanish → “index exists” but **0 resolvable files**.

### 1.5 YT B-roll is a footgun

Without Pexels (or when Pexels empty), yt-dlp ranks **music/lofi/playlist** titles as “stock footage.” Need title denylist + **video-only** format selector. With Pexels key, multi-broll must **prefer Pexels portrait first**.

### 1.6 HTTP clients need identity

Pexels (Cloudflare) returns **403 error 1010** for Python’s default urllib User-Agent. Bootstrap and any sidecar probe must set a real UA. Rust `reqwest` usually works; still pin UA on `PexelsClient` for consistency.

### 1.7 Design principle

```text
setup.sh           → code + models + build
bootstrap_media.sh → secrets merge + live probes + media indexes
system.doctor      → single boolean production_ready + next_actions
```

End of setup prints a **doctor table**. “Production ready” is **blocked** until Pexels live OK + music path green (library **or** shipped production beds).

---

## 2. Target zero-to-hero flow

```text
git clone …
cd openscript

# Secrets (never commit)
export PEXELS_API_KEY=… GIPHY_API_KEY=…   # optional: OPENROUTER_API_KEY, PIXABAY_API_KEY
# or:
bash scripts/setup_openscript_config.sh --pexels-key … --giphy-key …

bash setup.sh                              # binaries, Kokoro, cargo
bash scripts/bootstrap_media.sh --with-library   # doctor + music index

# Doctor must print production_ready: YES
openscript … director.run …                # real stock, not gradients
```

**Never store real keys in git.** Example files keep empty strings.

---

## 3. This machine — keys updated (local only)

Applied **only** under `~/.openscript/config.json` (mode **0600**):

| Key | Status |
|-----|--------|
| **Pexels** | Set · live probe OK with UA |
| **GIPHY** | Set · live search OK |
| **OpenRouter** | Kept / merged |
| **Pixabay** | Empty (optional) |

Verified via `scripts/bootstrap_media.sh --probe-only` (does not print secret values).

**Security:** If this chat is shared, **rotate** Pexels/GIPHY/OpenRouter in their dashboards. Keys must never appear in commits, logs, or EXPERIENCE_REPORT artifacts.

---

## 4. Dependency matrix

### A. Required for production-grade shorts

| Dep | Provision | Verify |
|-----|-----------|--------|
| ffmpeg + ffprobe | apt/brew / setup.sh | `ffmpeg -version` |
| yt-dlp | pip/apt | `yt-dlp --version` |
| Kokoro model + voices | setup.sh download | path under `mcp/assets/kokoro/` |
| **Pexels key** | config / env | doctor live search |
| Rust release `mcp-server` | `cargo build -p openscript-mcp --release` | smoke test |
| Config dir | `mkdir -p ~/.openscript` mode 700 | `config.json` 600 |

### B. Strongly recommended

| Dep | Provision | Verify |
|-----|-----------|--------|
| **GIPHY key** | config / env | sticker search |
| **`library.build` music index** | `bootstrap_media.sh --with-library` | `music_library_index.json` non-empty |
| Music cache | first `library.download` | files under cache dir |
| **Portable SFX pack** | ship small pack **or** download zip + reindex relative | ≥20 paths resolve |
| Ollama + qwen3.5-4b | optional local LLM | `llm.complete` |
| OpenRouter | vision cascade | `vision.score_clip` |

### C. Optional / advanced

| Dep | Notes |
|-----|-------|
| Parakeet ONNX | Caption force-align quality |
| Pixabay | Alt music/video |
| mmproj GGUF | Local vision |
| Tauri + GDK | Desktop shell only |

---

## 5. Implementation plan (phased)

### Phase I — Config + doctor ✅ / partial

| ID | Task | Status |
|----|------|--------|
| I.1 | Keys in `~/.openscript/config.json` (this machine) | ✅ |
| I.2 | `setup_openscript_config.sh` multi-key merge (pexels/giphy/pixabay/openrouter) | ✅ |
| I.3 | `bootstrap_media.sh` doctor + probes + UA fix for Pexels | ✅ |
| I.4 | MCP `system.doctor` → `ready_for_production`, checklist, `next_actions` | ⬜ |
| I.5 | `openscript.env.example` + README Secrets blurb | ⬜ next |

### Phase II — First-run asset indexes

| ID | Task | Detail |
|----|------|--------|
| II.1 | `bootstrap --with-library` | Invoke MCP `library.build` when index missing |
| II.2 | **Minimal production music pack** | 3–5 short CC0/lofi beds under `mcp/assets/music_production/` + tags JSON — offline calm/focus without YT |
| II.3 | Portable SFX | Small curated pack under `mcp/assets/sfx_pack/` **or** bootstrap zip URL; reindex **relative** to `OPENSCRIPT_SFX_PATH` |
| II.4 | `sfx.index` portability | Never write absolute `$HOME/...` into committed indexes; resolve at runtime |

### Phase III — Stock path reliability (product code)

| ID | Task | Detail |
|----|------|--------|
| III.1 | **B-roll title denylist** | music / lofi / playlist / hours / focus music |
| III.2 | yt-dlp **video-only** format for B-roll | Avoid audio-only streams ranked as footage |
| III.3 | Prefer Pexels portrait when key present | YT only if Pexels empty after retries |
| III.4 | **Fail-closed stock** | If stock_ratio < 0.5 and not `OPENSCRIPT_ALLOW_PROCEDURAL`, do not claim production success (error or `.draft.mp4`) |
| III.5 | Pin User-Agent on `PexelsClient` | Avoid intermittent Cloudflare blocks |

### Phase IV — One-command install

| ID | Task | Detail |
|----|------|--------|
| IV.1 | `setup.sh` calls `bootstrap_media.sh` after build (probe-only by default) | |
| IV.2 | Env template checked into repo (empty values) | |
| IV.3 | CI: unit tests only; no live keys. Local doctor full probe | |
| IV.4 | One-page `docs/INSTALL.md`: clone → keys → setup → first video | |

### Phase V — Validation

| ID | Task | Pass criteria |
|----|------|---------------|
| V.1 | Fresh worktree + empty HOME config + keys from env | doctor YES |
| V.2 | `director.run` 5-scene desk script | ≥4/5 non-procedural, music present, `verify.production` ≥ **B**, `hard_fails == []` |
| V.3 | No keys | doctor fails; no gradient presented as production success |

---

## 6. `bootstrap_media.sh` contract

```bash
bash scripts/bootstrap_media.sh              # merge env keys + full doctor
bash scripts/bootstrap_media.sh --probe-only
bash scripts/bootstrap_media.sh --with-library
# future: --with-sfx-pack
```

Behavior:

1. Ensure `~/.openscript` (700) + merge config (600) via `setup_openscript_config.sh`
2. Check ffmpeg / ffprobe / yt-dlp
3. Probe Pexels (Authorization + **User-Agent**) and GIPHY
4. Check `music_library_index.json`, `sfx_index.json` path resolution, Kokoro
5. Optional: `library.build` via release `mcp-server`
6. Print:

```text
OpenScript media doctor
  [✓] ffmpeg
  [✓] yt-dlp
  [✓] pexels live
  [✓] giphy live
  [✗] music_library_index  → bootstrap_media.sh --with-library
  [!] sfx paths machine-local → --with-sfx-pack (planned)
  production_ready: NO
```

---

## 7. Remaining gaps by priority

| P | Gap | Fix |
|---|-----|-----|
| **P0** | No `music_library_index.json` on fresh / this machine | Run `--with-library` **or** ship `music_production/` beds (II) |
| **P0** | Gradient still valid “output” when stock fails | Fail-closed / draft (III.4) |
| **P0** | YT “focus music” as B-roll | Denylist + video-only (III.1–2) |
| **P1** | SFX absolute paths | Portable pack + relative reindex (II.3–4) |
| **P1** | `system.doctor` MCP tool | Agents can self-heal without shell script (I.4) |
| **P1** | Wire bootstrap into `setup.sh` | One command (IV.1) |
| **P2** | Pixabay | Document optional |
| **P2** | Parakeet / GGUF | Optional download paths already partial |

---

## 8. Acceptance: `production_ready`

A clean machine is production-ready when **all** hold:

1. Doctor / capabilities: `pexels` live OK, `ffmpeg`, `yt_dlp`
2. Music: `music_library_index.json` **or** tagged `music_production/` beds
3. SFX: ≥20 files resolve (local pack or absolute index on this host)
4. `director.run` on a 5-scene script:
   - ≥4/5 non-procedural backgrounds (Pexels preferred)
   - music path non-null and not synthetic denylist
   - `verify.production.status == pass`, grade ≥ **B**, empty `hard_fails`
5. **No API keys in git history**

---

## 9. Immediate next engineering steps

1. ~~Local keys + multi-key config script + bootstrap doctor~~ ✅  
2. ~~Pexels probe UA fix; pin UA on `PexelsClient`~~ ✅ / in progress  
3. **P0 product:** B-roll denylist + video-only yt-dlp + fail-closed stock (III)  
4. **P0 media:** `library.build` on this machine **or** commit 3 production beds  
5. **P1:** portable SFX + `system.doctor` + hook into `setup.sh`  
6. Cold-agent **v5** with keys → expect real Pexels multi-broll  

---

## 10. Security note

API keys from chat live **only** in:

`~/.openscript/config.json` (mode 0600)

They are **not** committed. Rotate if the conversation is shared. Prefer env injection in CI/CD:

```bash
PEXELS_API_KEY=… GIPHY_API_KEY=… bash scripts/setup_openscript_config.sh
```

---

## 11. Suggested iteration commits (AGENTS.md)

| Iteration | Commit message sketch |
|-----------|----------------------|
| Docs + bootstrap | `Phase CO: Media install plan + bootstrap doctor + Pexels UA` |
| Fail-closed + denylist | `Phase CP: Fail-closed stock + B-roll title denylist` |
| Music pack / library | `Phase CQ: Production music beds + library bootstrap` |
| SFX portable | `Phase CR: Portable SFX pack + relative sfx.index` |
| Doctor MCP | `Phase CS: system.doctor + setup.sh media hook` |

Push after each iteration. Never commit secrets.

---

*Contract for “clone and go” media deps. Local keys applied; remaining work is music index, fail-closed stock, denylist, portable SFX, and setup wiring.*
