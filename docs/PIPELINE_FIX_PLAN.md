# OpenScript Full-Pipeline Fix Plan

**Status:** Implemented (Phases A–F landed 2026-07-15)  
**Date:** 2026-07-15  
**Driven by:** Cold-agent trials v2/v3 + production KPI gaps + provider/config audit  

### Implementation log

| Phase | Status | Notes |
|-------|--------|-------|
| A Validator hard-fails + visual_hooks | **done** | KPI v3.0; majority procedural / parade music / no hooks → hard_fail + grade ≤ D |
| B Providers + stock yield | **done** | YT fan-out, preflight, HARD warnings; Pexels still needs key |
| C Music topic-tagged | **done** | denylist, calm/focus query map, MusicSelection provenance |
| D SFX auto-assign | **done** | tagged sfx_index → multilayer SfxHit mix |
| E Relevance fields | **done** | lexical_score + source_title on backgrounds in manifest |
| F director.run | **done** | one-shot MCP tool + tool count 83 |  

---

## 0. Diagnosis (what is actually broken)

### 0.1 What you saw (v3)

| Symptom | Measured fact |
|---------|----------------|
| “Only synthetic gradient backgrounds” | **4/5** beds were `mcp/assets/backgrounds/procedural_*.mp4`; **1/5** YouTube |
| “Validator should crush this” | Production still **Grade B · 78** — procedural is only a soft score hit (2/14 on source), **not** a hard fail unless **all** clips are procedural |
| “Video relevance still off” | Scene 1 accepted YT with **lex=0.00** (fixed mid-session); remaining scenes had no unique relevant YT → procedural |
| “Providers not working?” | **Pexels / GIPHY / Pixabay keys are unset.** Only working stock path is **yt-dlp YouTube**. Premium providers are offline by config, not by code crash |
| “Parade music” | Cold agent reused a one-off `music_bed.mp3` from a generic “royalty free upbeat” search — **not topic-tagged**. `music_library_index.json` **missing** (no `library.build`). Committed `mcp/assets/music/*` = **synthetic sine stubs** |
| SFX | **261 assets already tagged** (category, editorial_role, tags) in `sfx_index.json` — but **`script.to_video` never auto-assigns SFX**. Paths often point at `$HOME/Videos/Assets/SFX/...` (machine-local) |

### 0.2 Root-cause map

```text
                    ┌─────────────────────┐
                    │  script.to_video    │
                    └──────────┬──────────┘
           ┌───────────────────┼───────────────────┐
           ▼                   ▼                   ▼
    VISUAL BED            MUSIC BED              SFX / HOOKS
           │                   │                   │
   Pexels? ──NO key──►    library index? ──NO──►  sfx_index YES
   YouTube yt-dlp         music_search synthetic   but NOT wired
   + lexical gates        ad-hoc yt-dlp bed        into golden path
   + uniqueness           (wrong mood/topic)
           │                   │                   │
           ▼                   ▼                   ▼
   procedural fill        parade / random bed    silent cuts
   (gradient “noise”)     (audio noise)          (no visual/audio hooks)
           │
           ▼
   verify.production
   source_quality soft (2/14)
   grade still B — ships as “acceptable”
```

**Iron rule for the fix:**  
*Never ship Grade ≥ B with majority procedural beds, untagged random music, or zero visual hooks. Fail closed.*

---

## 1. Goals / non-goals

### Goals

1. **Real stock B-roll** per scene (Pexels preferred, YT secondary) with **relevance + geometry gates**  
2. **Validator hard-fails** majority-procedural / no-hook / wrong-music-mood videos  
3. **Topic-tagged music + SFX** selection inside `script.to_video`  
4. **Provider honesty** in capabilities (what works, what keys, what to run)  
5. One **golden path** that a cold agent can complete without scavenger hunts  

### Non-goals (defer)

- Full egui/libmpv UI migration  
- Replacing Kokoro TTS  
- Perfect offline-only 4K stock library  

---

## 2. Target architecture (signal vs noise end-to-end)

```text
script.parse
    │
    ▼
director.prepare  (new orchestrator step or inside to_video)
    │  ensure_providers()     → keys + yt-dlp + library index + sfx paths
    │  ensure_music_index()   → library.build if missing / stale
    │  ensure_sfx_resolvable()→ rewrite absolute paths / reindex
    ▼
script.to_video
    │
    ├─ TTS + captions (Parakeet if present)
    │
    ├─ VISUAL PIPELINE (per scene)
    │     signal_query = stock_signal.build_scene_stock_query(...)
    │     candidates = Pexels(portrait) ∪ YouTube(id,title)×N queries
    │     rank = lexical(title) × provider_tier × orientation_fit
    │     optional vision.score_clip if openrouter available
    │     accept only if: relevance ≥ τ AND geometry SAR≈1 AND unique hash
    │     if < min_stock_ratio after all scenes → FAIL render (no procedural majority)
    │
    ├─ MUSIC PIPELINE
    │     query = theme + video_keywords + energy
    │     library.search(tags/mood) → library.download
    │     else Pixabay (key) → else curated ytsearch with mood tags
    │     reject if mood distance high OR synthetic path
    │
    ├─ SFX PIPELINE
    │     map scene punctuation → editorial_role (intro/transition/hit/cta)
    │     sfx.search(tags) → timeline SFX events + multilayer mix
    │
    ├─ STICKERS / optional memes (diverse assets)
    │
    └─ RENDER + write render_manifest (full provenance)
           │
           ▼
    verify.production v3
         hard_fail if procedural_ratio ≥ 0.5
         hard_fail if visual_hook_score == 0
         hard_fail if music_topic_mismatch
         hard_fail if stock_relevance_avg < τ
```

---

## 3. Workstreams (implementation phases)

### Phase A — Validator: fail closed on “no visual hooks” (1–2 days)

**Why first:** stops the system from celebrating Grade B garbage.

| # | Change | Detail |
|---|--------|--------|
| A1 | **Hard-fail majority procedural** | If `procedural_count / n ≥ 0.5` (n≥2) → `hard_fails` + force grade ≤ D regardless of other scores |
| A2 | **New dimension: `visual_hooks` (weight 8–10)** | Hooks = real stock (non-procedural) cuts with duration ≥ 1.2s + optional sticker/meme presence in first 3s. Score 0 → hard_fail `HARD: no visual hooks` |
| A3 | **Penalize empty search_query on non-procedural** | Context relevance already exists; require `search_query` + min avg jaccard for Grade ≥ B |
| A4 | **Music hard-fail** | Synthetic path OR missing mood when `video_keywords` present → hard_fail / score 0 on music |
| A5 | **`verify.production` status** | If any hard_fail → `status: "fail"` and CLI exit non-zero when used from director gate |
| A6 | **Unit tests** | 4/5 procedural → hard_fail; 5/5 Pexels unique → no hard_fail; synthetic music → hard_fail |

**Files:** `crates/openscript-core/src/production_quality.rs`, integration tests, `AGENT_GUIDE.md` grade table.

**Exit criteria:** Re-score v3 manifest → **status fail**, grade D/F, hard_fails include procedural majority + weak hooks.

---

### Phase B — Providers: make stock actually work (2–3 days)

**Truth:** code supports Pexels; **keys are missing**. YouTube alone cannot sustain multi-scene relevance under uniqueness constraints.

| # | Change | Detail |
|---|--------|--------|
| B1 | **Config bootstrap** | `setup_openscript_config.sh` prompts for PEXELS / GIPHY / PIXABAY; write `~/.openscript/config.json` |
| B2 | **Capabilities gate for `script.to_video`** | Preflight: if no Pexels and no yt-dlp → refuse. If no Pexels, warn `stock_provider=youtube_only` and raise candidate budget |
| B3 | **Pexels-first multi-broll (already intended)** | Prefer orientation=portrait, duration≥3s, unique ids; apply cover-crop + geometry probe (already done for YT) to Pexels trims |
| B4 | **YouTube query fan-out** | On empty ranked list: retry (1) scene nouns only (2) video_keywords only (3) synonym bank — max 3 searches/scene before procedural |
| B5 | **Ban procedural as default fill** | Env `OPENSCRIPT_ALLOW_PROCEDURAL=1` only; else leave last good stock looped or fail scene with explicit error |
| B6 | **Vision accept/reject (optional)** | If `openrouter` available: mid-frame score; reject if `relevance < 0.45` or `match=false`; cache in manifest |
| B7 | **Provider smoke tool** | `system.providers.probe` downloads 1 Pexels + 1 YT clip, reports geometry + lex score |

**Exit criteria:** With Pexels key set, 5-scene desk script → **≥4/5 non-procedural**, unique hashes, SAR 1:1, production ≥ B **without** hard_fails. Without Pexels, clear preflight message + YT fan-out gets ≥3/5 stock or hard fail.

---

### Phase C — Music: topic-tagged selection (2–3 days)

| # | Change | Detail |
|---|--------|--------|
| C1 | **`library.build` in setup / first-run** | `scripts/setup_openscript_config.sh` or `director.prepare` runs build if index missing (background OK) |
| C2 | **Tag schema for music library** | Each entry: `mood[]`, `energy`, `bpm_est?`, `tags[]` (focus, chill, epic, parade, corporate…), `license`, `source`, `path` |
| C3 | **Topic → music query mapper** | `video_keywords` + `output.theme` → tag filter e.g. desk/focus → `chill|lofi|ambient|focus` **exclude** `march|parade|epic|trailer|sport` |
| C4 | **`auto_select_music` rewrite** | Order: library.search(tags) → library.download → Pixabay mood search → yt-dlp with **tagged** query only (`"lofi study focus no copyright"` not `"upbeat funky"`) |
| C5 | **Reject parade / mismatch** | If title/tags hit denylist for calm/focus themes → skip. Log `music_reject` |
| C6 | **Manifest music provenance** | Store `mood`, `energy`, `tags`, `selection_query`, `source` for KPI |
| C7 | **KPI music_topic_fit** | Weight part of music_variance or new 4–6 pts; hard_fail denylist hits |

**Exit criteria:** Focus/desk script never selects march/parade bed; calm theme → chill/lofi/ambient; synthetic stock never selected when library/Pixabay/yt available.

---

### Phase D — SFX: use the tagged library you already have (1–2 days)

SFX index already has `category`, `editorial_role`, `tags`, `recommended_use`. Gap = **not in golden path** + **paths may be dead**.

| # | Change | Detail |
|---|--------|--------|
| D1 | **Path resolution** | Resolve `$HOME/Videos/Assets/SFX` + `OPENSCRIPT_SFX_PATH`; reindex if >50% missing |
| D2 | **`sfx.auto_assign` in `script.to_video`** | Map: hook open → intro rise; scene cuts → transitions/whoosh; CTA → stinger; optional text-pop |
| D3 | **Tag search** | Use existing tags (`rise`, whoosh, hit) + editorial_role |
| D4 | **Mix** | Multilayer: SFX at −12 to −18 dB, duck under VO slightly |
| D5 | **KPI** | timeline_editor / new `audio_hooks` dimension: 0 SFX on multi-cut short → finding + score cut |

**Exit criteria:** 5-scene short has ≥3 SFX events with resolvable paths; production findings no longer “sfx track empty”.

---

### Phase E — Relevance closed loop (2 days)

| # | Change | Detail |
|---|--------|--------|
| E1 | Keep **scene-first** `stock_signal` (shipped) | Per-scene nouns drive query |
| E2 | **Hard lex gate only** (shipped) | Never accept lex≈0 |
| E3 | **Manifest stores** `lexical_score`, `title`, `provider`, `geometry_ok` | For audit + KPI |
| E4 | **context_relevance KPI uses title/tags** | Not only search_query string vs dialogue |
| E5 | Optional **vision gate** (Phase B6) | When free tier available |

**Exit criteria:** Cold re-run desk script: no lex=0 titles in manifest; avg lexical ≥ 0.2 on accepted stock; visual relevance subjectively improved.

---

### Phase F — Golden-path productization (2 days)

| # | Change | Detail |
|---|--------|--------|
| F1 | **`director.run` MCP tool** | One call: prepare providers → parse → to_video → verify.production → return path + grade + hard_fails |
| F2 | **Refuse render** if preflight fails (no stock path, no music source) | Clear next_actions |
| F3 | **CLI parity** | `openscript director-run`, `library-build`, `system-config-get` |
| F4 | **Progress events** | Per-scene stock status so agents don’t sit 10 min blind |
| F5 | **Docs** | AGENT_GUIDE: required keys, hard-fail table, music/SFX tag conventions |

---

## 4. KPI v3 weight sketch (after Phase A)

| Dimension | Weight | Hard-fail triggers |
|-----------|-------:|--------------------|
| video_source_quality | 14 | majority procedural; all unknown |
| **visual_hooks** (new) | 10 | zero real stock in first 3s / whole video |
| visual_repetition | 14 | REPETITION dominant identity |
| context_relevance | 12 | avg relevance &lt; τ when stock present |
| cuts_pacing | 6 | — |
| music_variance + topic fit | 12 | synthetic; denylist mood mismatch |
| sticker_design | 8 | — |
| section_composition | 8 | — |
| speech_audio | 8 | no dialogue |
| captions | 6 | missing |
| timeline_editor + sfx hooks | 6 | — |
| **Total** | 100 | Any hard_fail → status fail, grade cap D |

---

## 5. Provider matrix (honest)

| Provider | Role | Required | Current env |
|----------|------|----------|-------------|
| **Pexels** | Primary multi-broll (portrait) | `api_keys.pexels` | **Unset** |
| **yt-dlp / YouTube** | Fallback stock + music scrape | binary on PATH | **Works** |
| **GIPHY** | Sticker diversity | `api_keys.giphy` | **Unset** |
| **Pixabay** | Music/video stock | `api_keys.pixabay` | **Unset** |
| **library index** | Tagged music/SFX download | `library.build` once | **Missing** |
| **SFX local** | Hook punctuation | paths resolvable | Index yes, paths may be dead |
| **OpenRouter** | Vision QA | key | **Set** (quota fragile) |
| **Ollama/Qwen** | Text director | local model | **Works** |

**Answer to “aren’t video providers working?”**  
- **YouTube path works** but is a poor multi-scene relevance source under uniqueness+lex gates → falls to procedural.  
- **Pexels is the correct primary provider and is not configured** — so the system is running in degraded mode by definition.

---

## 6. Implementation order & estimates

| Phase | Effort | Dependency | Ship value |
|-------|--------|------------|------------|
| **A Validator hard-fail** | 1–2 d | none | Stops fake success immediately |
| **B Providers + stock yield** | 2–3 d | A (tests) | Real B-roll |
| **C Music tags + selection** | 2–3 d | library.build | No parade on desk videos |
| **D SFX auto-assign** | 1–2 d | path fix | Cut hooks |
| **E Relevance closed loop** | 2 d | B | Context-true visuals |
| **F director.run** | 2 d | A–D | Cold-agent one-shot |

**Total:** ~10–14 engineering days for full stack; **A+B+C** is the minimum viable “not regressed” package (~1 week).

---

## 7. Acceptance tests (definition of done)

### T1 — Procedural majority rejected  
Fixture: 4 procedural + 1 YT → `verify.production.status == fail`, hard_fail present, grade ≤ D.

### T2 — Pexels path (key required)  
5-scene focus script → ≥4 Pexels/YT non-procedural, unique hashes, SAR 1:1, production ≥ B, no hard_fails.

### T3 — Music topic  
`theme=calm` + keywords `[desk, focus]` → selected track tags include chill/lofi/ambient; denylist march/parade not selected.

### T4 — SFX present  
≥3 SFX events, files exist, render includes audible hits (peak check optional).

### T5 — Cold agent  
`director.run` with only config keys set → MP4 path + grade report; no scavenger yt-dlp outside tools.

### T6 — Regression  
Prior sticker GIF loop, cover-crop SAR, scene-first signal unit tests remain green.

---

## 8. Immediate actions (today)

1. **Set `PEXELS_API_KEY`** (and ideally GIPHY) in `~/.openscript/config.json` — unlocks the designed primary stock path.  
2. **Run `library.build`** once (or schedule) to create tagged music index.  
3. **Implement Phase A** (validator hard-fails) so the next bad render cannot claim Grade B.  
4. **Delete / stop reusing** ad-hoc `music_bed.mp3` for calm/focus scripts until C3–C5 land.  
5. **Reindex SFX** with `sfx.index` against a portable `OPENSCRIPT_SFX_PATH`.

---

## 9. Success picture

A cold agent with keys set should produce:

- 9:16 short with **real stock per scene** (no gradient majority)  
- **Music mood matches topic**  
- **SFX on cuts**  
- `verify.production` **pass with Grade ≥ B** and **zero hard_fails**  
- Failures that remain are **honest** (missing key, network) with **next_actions**, not silent procedural + parade music + Grade B

---

## 10. Open decisions (need owner call)

| Decision | Options | Recommendation |
|----------|---------|----------------|
| Procedural allowed at all? | Never / only with env flag / last resort ≤20% | **Env flag + ≤20% soft; ≥50% hard-fail** |
| Fail render vs fail verify only? | Block ffmpeg / only KPI | **Block `script.to_video` success status if stock_ratio low** |
| Music: require library.build? | Yes / allow untagged yt | **Require index OR Pixabay; untagged yt only with explicit `music.allow_untagged`** |
| Vision gate default on? | On if key / always off | **On if openrouter available; skip if 429** |

---

*This plan is the engineering contract for un-regressed director output. Phase A should land first so the validator stops endorsing synthetic-majority videos.*
