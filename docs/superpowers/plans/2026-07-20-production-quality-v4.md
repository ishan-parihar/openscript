# Production Quality Scoring v4.0 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Upgrade `production_quality.rs` from v3.0 (100 pts across 11 dimensions) to v4.0 (100 pts across 15 dimensions) by adding 4 new scoring dimensions (SFX, voiceover quality, audio mix quality, visual hierarchy, platform optimization) and expanding 3 existing ones (music, sticker, captions), then wiring all new weights into `evaluate_production_quality`.

**Architecture:** All scoring logic lives exclusively in `crates/openscript-core/src/production_quality.rs`. Each new scorer is a private `fn score_X(...)` that returns `DimensionScore`. The aggregate `evaluate_production_quality` calls them all and sums to 100. No changes to `tools.rs` or any other crate are needed — the `verify.production` MCP tool already deserializes `ProductionQualityReport` generically.

**Tech Stack:** Rust 2021, `serde_json`, `std` only (no new dependencies), `thiserror` (already in workspace), `#[cfg(test)]` unit tests inline.

## Global Constraints

- `cargo build --workspace --exclude openscript-tauri` must pass with **zero warnings** after every task.
- `cargo test --workspace --exclude openscript-tauri --lib --bins --tests` must pass with **≥248 tests** after every task.
- Git remote is `github`, not `origin`. Push command: `git push github main`.
- Post-iteration gate: `bash scripts/post-iteration.sh`.
- No `unwrap()`, `expect()`, `panic!()` in production code paths. Use `?` or explicit `.unwrap_or*`.
- No new crate dependencies. Use only what's already in `Cargo.toml`.
- `kpi_version` field in `ProductionQualityReport` must be bumped to `"4.0.0"` only in Task 8 (the integration task), not before.
- The `rich_manifest_scores_high` existing test asserts `production_score >= 70`. After weight redistribution in Task 8 it must still pass — the test manifest will be updated in Task 8 to include SFX, voiceover, and audio data.

---

## Files Modified

| File | Role |
|------|------|
| `crates/openscript-core/src/production_quality.rs` | All changes live here |

No other files require modification.

---

## v4.0 Weight Table (sums to 100)

| Dimension id | v3 max | v4 max | Delta |
|---|---|---|---|
| `video_source_quality` | 12 | 10 | -2 |
| `visual_hooks` | 10 | 8 | -2 |
| `visual_repetition` | 12 | 8 | -4 |
| `context_relevance` | 12 | 8 | -4 |
| `cuts_pacing` | 6 | 5 | -1 |
| `music_quality` | 10 | 8 | -2 (renamed+expanded) |
| `sfx_quality` | — | 6 | NEW |
| `sticker_design` | 8 | 8 | 0 (expanded) |
| `caption_quality` | 6 | 6 | 0 (renamed+expanded) |
| `voiceover_quality` | — | 6 | NEW |
| `audio_mix_quality` | — | 5 | NEW |
| `section_composition` | 8 | 8 | 0 |
| `visual_hierarchy` | — | 5 | NEW |
| `platform_optimization` | — | 5 | NEW |
| `timeline_editor` | 8 | 4 | -4 |
| **Total** | **100** | **100** | |

---

## Task 1: Add `score_sfx_quality` dimension (6 pts)

**Files:**
- Modify: `crates/openscript-core/src/production_quality.rs`

**Interfaces:**
- Consumes: `manifest.sfx_count: usize` (already in `RenderManifest`) and `timeline: &Timeline` (for `TrackType::Sfx` events).
- Produces: `DimensionScore { id: "sfx_quality", max: 6 }` consumed by `evaluate_production_quality` in Task 8.
- New function signature: `fn score_sfx_quality(sfx_count: usize, timeline: &Timeline) -> DimensionScore`

**Context:** `EventKind::Sfx` fields are confirmed: `editorial_role`, `category`, `subcategory`, `duration_ms`, `sample_rate`, `peak_db`, `loudness_lufs`, `recommended_gain_db`, `recommended_use`, `safe_overlay`. Use `recommended_gain_db` for the gain check (not `gain_db`).

- [ ] **Step 1: Write the failing tests**

Add these inside `#[cfg(test)] mod tests` at the bottom of `production_quality.rs`, before the closing `}`:

```rust
#[test]
fn sfx_quality_no_sfx_scores_zero() {
    let tl = empty_timeline();
    let d = score_sfx_quality(0, &tl);
    assert_eq!(d.score, 0);
    assert_eq!(d.max, 6);
    assert!(d.findings.iter().any(|f| f.contains("sfx") || f.contains("SFX")));
}

#[test]
fn sfx_quality_present_unique_scores_high() {
    use crate::timeline::{EventKind, TimelineEvent};
    use crate::types::TrackType;
    let mut tl = empty_timeline();
    for (i, asset_id) in ["whoosh_a", "pop_b"].iter().enumerate() {
        tl.tracks.entry(TrackType::Sfx).or_default().push(TimelineEvent {
            id: format!("sfx_{}", i),
            asset_id: asset_id.to_string(),
            start_ms: (i as i64) * 4000,
            end_ms: (i as i64) * 4000 + 500,
            kind: EventKind::Sfx { editorial_role: "transition".to_string(), category: String::new(), subcategory: String::new(), duration_ms: 400, sample_rate: 44100, peak_db: -10.0, loudness_lufs: -18.0, recommended_gain_db: -10.0, recommended_use: String::new(), safe_overlay: true },
        });
    }
    let d = score_sfx_quality(2, &tl);
    assert!(d.score >= 4, "two unique SFX assets should score >=4, got {}", d.score);
}

#[test]
fn sfx_quality_repeated_asset_penalized() {
    use crate::timeline::{EventKind, TimelineEvent};
    use crate::types::TrackType;
    let mut tl = empty_timeline();
    for i in 0..4 {
        tl.tracks.entry(TrackType::Sfx).or_default().push(TimelineEvent {
            id: format!("sfx_{}", i),
            asset_id: "whoosh_a".to_string(),
            start_ms: (i as i64) * 3000,
            end_ms: (i as i64) * 3000 + 400,
            kind: EventKind::Sfx { editorial_role: "transition".to_string(), category: String::new(), subcategory: String::new(), duration_ms: 400, sample_rate: 44100, peak_db: -10.0, loudness_lufs: -18.0, recommended_gain_db: -10.0, recommended_use: String::new(), safe_overlay: true },
        });
    }
    let d = score_sfx_quality(4, &tl);
    assert!(d.findings.iter().any(|f| f.contains("repetitive") || f.contains("repeat")));
    assert!(d.score <= 4, "repetitive sfx should score <=4, got {}", d.score);
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cd /home/ishanp/Documents/GitHub/MY-PROJECTS/CONTENT-CREATION/openscript
export PATH="$HOME/.cargo/bin:$PATH"
cargo test -p openscript-core --lib -- sfx_quality 2>&1 | tail -20
```

Expected: `error[E0425]: cannot find function 'score_sfx_quality'`

- [ ] **Step 3: Implement `score_sfx_quality`**

Add this function after `score_music_variance` / `score_music_quality` and before `score_sticker_design`:

```rust
/// Weight 6 — SFX punctuation presence, variety, and gain compliance.
fn score_sfx_quality(sfx_count: usize, timeline: &Timeline) -> DimensionScore {
    let mut findings = Vec::new();

    let sfx_events: Vec<_> = timeline
        .tracks
        .get(&TrackType::Sfx)
        .cloned()
        .unwrap_or_default();

    if sfx_count == 0 && sfx_events.is_empty() {
        findings.push(
            "HARD: no SFX at any transition — add whoosh/pop/riser via sfx.assign".into(),
        );
        return DimensionScore {
            id: "sfx_quality".into(),
            label: "SFX punctuation & variety".into(),
            score: 0,
            max: 6,
            detail: serde_json::json!({ "sfx_count": 0, "unique_assets": 0 }),
            findings,
        };
    }

    let mut s = 2; // base for having any SFX

    let unique_sfx: HashSet<_> = sfx_events.iter().map(|e| e.asset_id.as_str()).collect();
    let unique_count = unique_sfx.len().max(sfx_count.min(1));

    if unique_count >= 3 {
        s += 2;
    } else if unique_count >= 2 {
        s += 1;
    } else {
        findings.push(format!(
            "repetitive sfx: only {} unique asset(s) — rotate through >=3 different sounds",
            unique_count
        ));
    }

    let mut asset_counts: HashMap<&str, usize> = HashMap::new();
    for e in &sfx_events {
        *asset_counts.entry(e.asset_id.as_str()).or_insert(0) += 1;
    }
    let max_repeat = asset_counts.values().copied().max().unwrap_or(0);
    if max_repeat > 2 {
        findings.push(format!(
            "sfx asset repeated {}x — a real editor rotates different SFX per transition",
            max_repeat
        ));
        s = (s - 1).max(0);
    }

    let mut gain_violations = 0usize;
    for e in &sfx_events {
        if let EventKind::Sfx { recommended_gain_db, .. } = &e.kind {
            if *recommended_gain_db > -3.0 || *recommended_gain_db < -20.0 {
                gain_violations += 1;
            }
        }
    }
    if gain_violations > 0 {
        findings.push(format!(
            "{} sfx event(s) with gain outside -20 to -3 dB — risk of clipping or inaudibility",
            gain_violations
        ));
    } else if !sfx_events.is_empty() {
        s += 1;
    }

    let coverage = (sfx_count.max(sfx_events.len()) >= 2) as i32;
    s += coverage;

    DimensionScore {
        id: "sfx_quality".into(),
        label: "SFX punctuation & variety".into(),
        score: s.min(6),
        max: 6,
        detail: serde_json::json!({
            "sfx_count": sfx_count,
            "timeline_sfx_events": sfx_events.len(),
            "unique_assets": unique_count,
            "max_repeat": max_repeat,
            "gain_violations": gain_violations,
        }),
        findings,
    }
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p openscript-core --lib -- sfx_quality 2>&1 | tail -20
```

Expected: `sfx_quality_no_sfx_scores_zero ... ok`, `sfx_quality_present_unique_scores_high ... ok`, `sfx_quality_repeated_asset_penalized ... ok`

- [ ] **Step 5: Build**

```bash
cargo build --workspace --exclude openscript-tauri 2>&1 | grep -E "^error|^warning|Finished"
```

Expected: `Finished` with zero warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/openscript-core/src/production_quality.rs
git commit -m "Phase 1a: Add score_sfx_quality dimension (6 pts) with tests"
git push github main
```

---

## Task 2: Rename + expand `score_music_variance` → `score_music_quality` (8 pts)

**Files:**
- Modify: `crates/openscript-core/src/production_quality.rs`

**Interfaces:**
- Same signature as before: `fn score_music_quality(music: Option<&MusicLayerInfo>, theme: Option<&str>, video_keywords: &[String]) -> DimensionScore`
- `id` changes from `"music_variance"` to `"music_quality"`. `max` stays `8` (same as goal; was 10 in v3 but we first rename it here and set max to 8, the weight table target).
- New checks: gain sweet-spot bonus, source provider check.
- Update the call site in `evaluate_production_quality` from `score_music_variance` → `score_music_quality`.

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn music_quality_no_music_scores_zero() {
    let d = score_music_quality(None, None, &[]);
    assert_eq!(d.id, "music_quality");
    assert_eq!(d.score, 0);
    assert_eq!(d.max, 8);
    assert!(d.findings.iter().any(|f| f.contains("HARD")));
}

#[test]
fn music_quality_gain_too_loud_penalized() {
    std::fs::write("/tmp/test_music_loud.mp3", vec![4u8; 9000]).unwrap();
    let m = MusicLayerInfo {
        path: "/tmp/test_music_loud.mp3".to_string(),
        gain_db: 2.0,
        ducking: true,
        mood: Some("upbeat".into()),
        energy: Some("high".into()),
        tags: vec!["pop".into()],
        selection_query: Some("morning".into()),
        source: Some("pixabay".into()),
    };
    let d = score_music_quality(Some(&m), None, &[]);
    std::fs::remove_file("/tmp/test_music_loud.mp3").ok();
    assert!(d.findings.iter().any(|f| f.contains("gain") || f.contains("loud") || f.contains("unity")));
}

#[test]
fn music_quality_sweet_spot_scores_high() {
    std::fs::write("/tmp/test_music_sweet.mp3", vec![5u8; 9000]).unwrap();
    let m = MusicLayerInfo {
        path: "/tmp/test_music_sweet.mp3".to_string(),
        gain_db: -12.0,
        ducking: true,
        mood: Some("calm".into()),
        energy: Some("low".into()),
        tags: vec!["lofi".into()],
        selection_query: Some("lofi chill".into()),
        source: Some("library".into()),
    };
    let d = score_music_quality(Some(&m), None, &[]);
    std::fs::remove_file("/tmp/test_music_sweet.mp3").ok();
    assert!(d.score >= 6, "sweet spot music should score >=6/8, got {}", d.score);
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p openscript-core --lib -- music_quality 2>&1 | tail -20
```

Expected: compile error — `score_music_quality` does not exist yet.

- [ ] **Step 3: Replace `score_music_variance` with `score_music_quality`**

Replace the entire `score_music_variance` function with:

```rust
/// Weight 8 — music bed quality: presence, non-synthetic, topic fit,
/// ducking, gain compliance, mood/energy tags, source provider.
fn score_music_quality(
    music: Option<&MusicLayerInfo>,
    theme: Option<&str>,
    video_keywords: &[String],
) -> DimensionScore {
    let mut findings = Vec::new();
    let calm_ctx = is_calm_focus_context(theme, video_keywords);
    let score = match music {
        None => {
            findings.push("HARD: no background music bed".into());
            0
        }
        Some(m) if is_synthetic_music_file(&m.path) => {
            findings.push(
                "HARD: music is synthetic sine-wave placeholder (mcp/assets/music stock)".into(),
            );
            0
        }
        Some(m) if !Path::new(&m.path).exists() => {
            findings.push(format!("HARD: music path missing on disk: {}", m.path));
            0
        }
        Some(m)
            if calm_ctx
                && music_hits_denylist(
                    &m.path,
                    m.mood.as_deref(),
                    &m.tags,
                    m.selection_query.as_deref(),
                ) =>
        {
            findings.push(
                "HARD: music topic mismatch — parade/march/hype bed on calm/focus content".into(),
            );
            0
        }
        Some(m) => {
            let mut s = 3; // base for real, present, non-synthetic music

            if m.ducking {
                s += 1;
            } else {
                findings.push("music ducking disabled — will fight dialogue during speech".into());
            }

            // Gain sweet spot: -18 to -6 dB
            if (-18.0..=-6.0).contains(&m.gain_db) {
                s += 1;
            } else if m.gain_db > 0.0 {
                findings.push(format!(
                    "music gain_db={:.1} is above unity — louder than voice; use -8 to -14 dB",
                    m.gain_db
                ));
            } else if m.gain_db < -24.0 {
                findings.push(format!(
                    "music gain_db={:.1} may be inaudible; target -12 to -8 dB",
                    m.gain_db
                ));
            }

            if m.mood.as_ref().map(|x| !x.is_empty()).unwrap_or(false) {
                s += 1;
            } else {
                findings.push("music.mood not tagged — library.search uses mood for curation".into());
            }

            if m.energy.as_ref().map(|x| !x.is_empty()).unwrap_or(false)
                || !m.tags.is_empty()
            {
                s += 1;
            } else {
                findings.push("music.energy and tags both empty — reduces topic-fit scoring".into());
            }

            if m.source.as_ref().map(|s| s != "unknown" && !s.is_empty()).unwrap_or(false) {
                s += 1;
            }

            s.min(8)
        }
    };
    DimensionScore {
        id: "music_quality".into(),
        label: "BG music quality & topic fit".into(),
        score,
        max: 8,
        detail: serde_json::json!({ "music": music, "calm_focus_context": calm_ctx }),
        findings,
    }
}
```

- [ ] **Step 4: Fix call site** (find `score_music_variance` in `evaluate_production_quality` ~line 1302 and rename to `score_music_quality`)

- [ ] **Step 5: Run all tests**

```bash
cargo test -p openscript-core --lib -- music_quality 2>&1 | tail -20
cargo build --workspace --exclude openscript-tauri 2>&1 | grep -E "^error|^warning|Finished"
cargo test --workspace --exclude openscript-tauri --lib --bins --tests 2>&1 | tail -10
```

Expected: 3 new tests pass, >=248 total, zero warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/openscript-core/src/production_quality.rs
git commit -m "Phase 1b: Rename score_music_variance -> score_music_quality, expand to 8 pts"
git push github main
```

---

## Task 3: Expand `score_sticker_design` (add overlap, off-screen, always-on detection)

**Files:**
- Modify: `crates/openscript-core/src/production_quality.rs`

**Interfaces:**
- New function: `fn score_sticker_design_with_duration(stickers: &[StickerLayerInfo], duration_ms: i64) -> DimensionScore`
- Old `fn score_sticker_design(stickers: &[StickerLayerInfo]) -> DimensionScore` becomes a wrapper calling `score_sticker_design_with_duration(stickers, 0)`.
- The call site in `evaluate_production_quality` (Task 8) will switch to `score_sticker_design_with_duration`.

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn sticker_overlap_flagged() {
    let stickers = vec![
        StickerLayerInfo {
            path: "a.gif".into(), start_ms: 0, end_ms: 8000,
            position: "top-left".into(), scale: 0.35,
        },
        StickerLayerInfo {
            path: "b.gif".into(), start_ms: 3000, end_ms: 11000,
            position: "top-right".into(), scale: 0.30,
        },
    ];
    let d = score_sticker_design_with_duration(&stickers, 11000);
    assert!(
        d.findings.iter().any(|f| f.contains("overlap") || f.contains("compete")),
        "overlapping stickers should be flagged: {:?}", d.findings
    );
}

#[test]
fn sticker_empty_position_flagged() {
    let stickers = vec![StickerLayerInfo {
        path: "a.gif".into(), start_ms: 0, end_ms: 5000,
        position: "".into(), scale: 0.30,
    }];
    let d = score_sticker_design_with_duration(&stickers, 5000);
    assert!(
        d.findings.iter().any(|f| f.contains("position") || f.contains("off-screen") || f.contains("undefined")),
        "empty position should be flagged: {:?}", d.findings
    );
}

#[test]
fn sticker_always_on_flagged() {
    let stickers = vec![StickerLayerInfo {
        path: "a.gif".into(), start_ms: 0, end_ms: 20000,
        position: "top-left".into(), scale: 0.30,
    }];
    let d = score_sticker_design_with_duration(&stickers, 20000);
    assert!(
        d.findings.iter().any(|f| f.contains("always") || f.contains("whole video") || f.contains("100%")),
        "always-on sticker should be flagged: {:?}", d.findings
    );
}
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p openscript-core --lib -- "sticker_overlap\|sticker_empty\|sticker_always" 2>&1 | tail -20
```

Expected: compile error on `score_sticker_design_with_duration`.

- [ ] **Step 3: Refactor `score_sticker_design`**

Replace the entire existing `score_sticker_design` function with:

```rust
/// Weight 8 — sticker design (full analysis with duration context).
fn score_sticker_design_with_duration(
    stickers: &[StickerLayerInfo],
    duration_ms: i64,
) -> DimensionScore {
    let mut findings = Vec::new();
    if stickers.is_empty() {
        findings.push("no stickers/GIFs composited".into());
        return DimensionScore {
            id: "sticker_design".into(),
            label: "Sticker design principles".into(),
            score: 0,
            max: 8,
            detail: serde_json::json!({ "sticker_count": 0 }),
            findings,
        };
    }

    let mut s = 3;
    let unique: HashSet<_> = stickers.iter().map(|x| x.path.as_str()).collect();
    if unique.len() > 1 || stickers.len() == 1 {
        s += 2;
    } else {
        findings.push("same sticker asset repeated for all speakers — weak identity".into());
    }

    let mut scale_ok = 0;
    let mut pos_risk = 0;
    let mut animated = 0;
    let mut off_screen = 0usize;

    for st in stickers {
        if (0.20..=0.45).contains(&st.scale) {
            scale_ok += 1;
        } else {
            findings.push(format!("sticker scale={:.2} outside design band 0.20-0.45", st.scale));
        }
        let pos_lower = st.position.to_lowercase();
        if pos_lower.is_empty() || pos_lower == "off" || pos_lower == "hidden" || pos_lower == "none" {
            off_screen += 1;
            findings.push(format!(
                "sticker position '{}' is off-screen or undefined — set a valid position",
                st.position
            ));
        } else if pos_lower.contains("bottom") {
            pos_risk += 1;
            findings.push(format!("sticker position '{}' may collide with caption rail", st.position));
        }
        if st.path.ends_with(".gif") || st.path.ends_with(".webp") {
            animated += 1;
        }
    }

    if scale_ok * 2 >= stickers.len() { s += 2; }
    if pos_risk == 0 && off_screen == 0 {
        s += 2;
    } else if off_screen > 0 {
        s = (s - 1).max(0);
    }
    if animated > 0 {
        s += 1;
    } else {
        findings.push("no animated GIF stickers — static PNG only reduces visual energy".into());
    }

    // Temporal overlap check
    let mut overlap_pairs = 0usize;
    for i in 0..stickers.len() {
        for j in (i + 1)..stickers.len() {
            let a = &stickers[i];
            let b = &stickers[j];
            let overlap = a.end_ms.min(b.end_ms) - a.start_ms.max(b.start_ms);
            if overlap > 500 {
                overlap_pairs += 1;
            }
        }
    }
    if overlap_pairs > 0 {
        findings.push(format!(
            "{} sticker pair(s) overlap >500ms simultaneously — competing for attention",
            overlap_pairs
        ));
        s = (s - 1).max(0);
    }

    // Always-on: sticker spanning >=90% of video
    if duration_ms > 0 {
        for st in stickers {
            let span = st.end_ms - st.start_ms;
            if span as f64 >= duration_ms as f64 * 0.90 {
                findings.push(format!(
                    "sticker '{}' is always-on ({:.0}% of video) — dynamic placement increases engagement",
                    st.path.split('/').last().unwrap_or(&st.path),
                    span as f64 / duration_ms as f64 * 100.0
                ));
                break;
            }
        }
    }

    DimensionScore {
        id: "sticker_design".into(),
        label: "Sticker design principles".into(),
        score: s.min(8),
        max: 8,
        detail: serde_json::json!({
            "sticker_count": stickers.len(),
            "unique_assets": unique.len(),
            "animated_count": animated,
            "scale_ok_count": scale_ok,
            "bottom_position_risk": pos_risk,
            "off_screen_count": off_screen,
            "overlap_pairs": overlap_pairs,
            "design_band_scale": [0.20, 0.45],
        }),
        findings,
    }
}

/// Backward-compat wrapper — no duration = no always-on check.
fn score_sticker_design(stickers: &[StickerLayerInfo]) -> DimensionScore {
    score_sticker_design_with_duration(stickers, 0)
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p openscript-core --lib -- sticker 2>&1 | tail -20
```

Expected: `sticker_overlap_flagged`, `sticker_empty_position_flagged`, `sticker_always_on_flagged`, and existing `sticker_bottom_position_flagged` all pass.

- [ ] **Step 5: Build and full test run**

```bash
cargo build --workspace --exclude openscript-tauri 2>&1 | grep -E "^error|^warning|Finished"
cargo test --workspace --exclude openscript-tauri --lib --bins --tests 2>&1 | tail -10
```

- [ ] **Step 6: Commit**

```bash
git add crates/openscript-core/src/production_quality.rs
git commit -m "Phase 1c: Expand score_sticker_design — overlap, off-screen, always-on detection"
git push github main
```

---

## Task 4: Expand `score_captions` → `score_caption_quality` (6 pts, expanded)

**Files:**
- Modify: `crates/openscript-core/src/production_quality.rs`

**Interfaces:**
- New function: `fn score_caption_quality(captions_path: Option<&str>, coverage_ratio: f64, style: Option<&str>, chars_per_second: Option<f64>, words_per_line: Option<f64>) -> DimensionScore`
- New `RenderManifest` fields (add with `#[serde(default)]`):
  - `caption_style: Option<String>`
  - `caption_coverage_ratio: f64`
  - `caption_words_per_line: Option<f64>`
  - `caption_chars_per_second: Option<f64>`
- Old `score_captions` kept as `#[allow(dead_code)]` shim.

- [ ] **Step 1: Add fields to `RenderManifest`** (after `sfx_count: usize`)

```rust
/// Caption style: "word_highlight", "sentence_fade", "karaoke", "burn_in".
#[serde(default)]
pub caption_style: Option<String>,
/// Fraction of voiceover duration covered by captions (0.0–1.0).
#[serde(default)]
pub caption_coverage_ratio: f64,
/// Average words per caption line.
#[serde(default)]
pub caption_words_per_line: Option<f64>,
/// Average characters per second (reading speed).
#[serde(default)]
pub caption_chars_per_second: Option<f64>,
```

- [ ] **Step 2: Write failing tests**

```rust
#[test]
fn caption_quality_missing_scores_zero() {
    let d = score_caption_quality(None, 0.0, None, None, None);
    assert_eq!(d.id, "caption_quality");
    assert_eq!(d.score, 0);
    assert!(d.findings.iter().any(|f| f.contains("absent") || f.contains("missing")));
}

#[test]
fn caption_quality_low_coverage_penalized() {
    let path = "/tmp/cap_low_cov.ass";
    std::fs::write(path, b"[Script Info]\n").unwrap();
    let d = score_caption_quality(Some(path), 0.30, None, None, None);
    std::fs::remove_file(path).ok();
    assert!(d.findings.iter().any(|f| f.contains("coverage")));
    assert!(d.score <= 4, "30% coverage should score <=4, got {}", d.score);
}

#[test]
fn caption_quality_fast_cps_penalized() {
    let path = "/tmp/cap_fast.ass";
    std::fs::write(path, b"[Script Info]\n").unwrap();
    let d = score_caption_quality(Some(path), 0.95, None, Some(30.0), None);
    std::fs::remove_file(path).ok();
    assert!(
        d.findings.iter().any(|f| f.contains("fast") || f.contains("CPS") || f.contains("unreadable")),
        "fast CPS should be flagged: {:?}", d.findings
    );
}

#[test]
fn caption_quality_full_marks() {
    let path = "/tmp/cap_full.ass";
    std::fs::write(path, b"[Script Info]\n").unwrap();
    let d = score_caption_quality(
        Some(path), 0.95, Some("word_highlight"), Some(12.0), Some(2.5),
    );
    std::fs::remove_file(path).ok();
    assert!(d.score >= 5, "full marks caption should score >=5/6, got {}", d.score);
}
```

- [ ] **Step 3: Run to verify failure**

```bash
cargo test -p openscript-core --lib -- caption_quality 2>&1 | tail -20
```

- [ ] **Step 4: Implement `score_caption_quality`**

Replace the `score_captions` function with:

```rust
/// Weight 6 — caption quality: presence, style, coverage, readability.
fn score_caption_quality(
    captions_path: Option<&str>,
    coverage_ratio: f64,
    style: Option<&str>,
    chars_per_second: Option<f64>,
    words_per_line: Option<f64>,
) -> DimensionScore {
    let mut findings = Vec::new();

    let present = captions_path
        .map(|p| !p.is_empty() && Path::new(p).exists())
        .unwrap_or(false);

    if !present {
        findings.push("HARD: captions file absent — word-highlight captions required for retention and accessibility".into());
        return DimensionScore {
            id: "caption_quality".into(),
            label: "Caption quality & coverage".into(),
            score: 0,
            max: 6,
            detail: serde_json::json!({ "present": false }),
            findings,
        };
    }

    let mut s = 1; // base for file existing

    // Coverage
    if coverage_ratio >= 0.90 {
        s += 2;
    } else if coverage_ratio >= 0.70 {
        s += 1;
        findings.push(format!(
            "caption coverage {:.0}% — target >=90% of speech duration",
            coverage_ratio * 100.0
        ));
    } else if coverage_ratio > 0.0 {
        findings.push(format!(
            "caption coverage {:.0}% is low — many speech segments uncaptioned",
            coverage_ratio * 100.0
        ));
    }

    // Reading speed
    if let Some(cps) = chars_per_second {
        if cps > 25.0 {
            findings.push(format!(
                "caption CPS={:.1} exceeds 25 — unreadable at normal viewing speed; target <=20 CPS",
                cps
            ));
            s = (s - 1).max(0);
        } else if cps <= 20.0 {
            s += 1;
        }
    }

    // Words per line
    if let Some(wpl) = words_per_line {
        if wpl > 5.0 {
            findings.push(format!(
                "caption avg {:.1} words/line — prefer <=4 words for short-form readability",
                wpl
            ));
        } else {
            s += 1;
        }
    }

    // Style
    match style {
        Some(st) if st == "word_highlight" || st == "karaoke" => { s += 1; }
        Some(st) if !st.is_empty() => { /* acceptable */ }
        _ => {
            findings.push("caption_style not set — prefer word_highlight for engagement".into());
        }
    }

    DimensionScore {
        id: "caption_quality".into(),
        label: "Caption quality & coverage".into(),
        score: s.min(6),
        max: 6,
        detail: serde_json::json!({
            "present": true,
            "captions_path": captions_path,
            "coverage_ratio": coverage_ratio,
            "style": style,
            "chars_per_second": chars_per_second,
            "words_per_line": words_per_line,
        }),
        findings,
    }
}

#[allow(dead_code)]
fn score_captions(captions_path: Option<&str>) -> DimensionScore {
    let cov = if captions_path.is_some() { 1.0 } else { 0.0 };
    score_caption_quality(captions_path, cov, None, None, None)
}
```

- [ ] **Step 5: Run tests and build**

```bash
cargo test -p openscript-core --lib -- caption_quality 2>&1 | tail -20
cargo build --workspace --exclude openscript-tauri 2>&1 | grep -E "^error|^warning|Finished"
cargo test --workspace --exclude openscript-tauri --lib --bins --tests 2>&1 | tail -10
```

- [ ] **Step 6: Commit**

```bash
git add crates/openscript-core/src/production_quality.rs
git commit -m "Phase 2a: Expand score_captions -> score_caption_quality (coverage, CPS, style)"
git push github main
```

---

## Task 5: Add `score_voiceover_quality` (6 pts)

**Files:**
- Modify: `crates/openscript-core/src/production_quality.rs`

**Interfaces:**
- Add to `RenderManifest` after `caption_chars_per_second`:

```rust
/// Average TTS pace in words per minute (ideal 130–160).
#[serde(default)]
pub voiceover_wpm: Option<f64>,
/// Voice IDs used per speaker slot.
#[serde(default)]
pub voice_ids: Vec<String>,
/// True when TTS emote tags align to content sentiment.
#[serde(default)]
pub emote_alignment_ok: bool,
```

- New function: `fn score_voiceover_quality(has_dialogue: bool, voiceover_count: usize, wpm: Option<f64>, voice_ids: &[String], emote_ok: bool) -> DimensionScore`

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn voiceover_quality_no_dialogue_scores_zero() {
    let d = score_voiceover_quality(false, 0, None, &[], false);
    assert_eq!(d.id, "voiceover_quality");
    assert_eq!(d.score, 0);
    assert_eq!(d.max, 6);
}

#[test]
fn voiceover_quality_ideal_wpm_scores_high() {
    let d = score_voiceover_quality(
        true, 3, Some(145.0),
        &["af_heart".to_string(), "bm_lewis".to_string()],
        true,
    );
    assert!(d.score >= 5, "ideal voiceover should score >=5/6, got {}", d.score);
}

#[test]
fn voiceover_quality_too_fast_penalized() {
    let d = score_voiceover_quality(
        true, 2, Some(220.0),
        &["af_heart".to_string()],
        false,
    );
    assert!(
        d.findings.iter().any(|f| f.contains("fast") || f.contains("WPM") || f.contains("wpm")),
        "fast WPM should be flagged: {:?}", d.findings
    );
    assert!(d.score <= 4, "fast WPM should score <=4, got {}", d.score);
}
```

- [ ] **Step 2: Run to verify failure**

```bash
cargo test -p openscript-core --lib -- voiceover_quality 2>&1 | tail -20
```

- [ ] **Step 3: Implement `score_voiceover_quality`**

Add after `score_caption_quality`:

```rust
/// Weight 6 — voiceover quality: presence, WPM pacing, voice consistency, emote alignment.
fn score_voiceover_quality(
    has_dialogue: bool,
    voiceover_count: usize,
    wpm: Option<f64>,
    voice_ids: &[String],
    emote_alignment_ok: bool,
) -> DimensionScore {
    let mut findings = Vec::new();

    if !has_dialogue || voiceover_count == 0 {
        findings.push("no voiceover detected — add TTS via script.generate_voices".into());
        return DimensionScore {
            id: "voiceover_quality".into(),
            label: "Voiceover quality & pacing".into(),
            score: 0,
            max: 6,
            detail: serde_json::json!({ "has_dialogue": has_dialogue }),
            findings,
        };
    }

    let mut s = 2; // base for having voiceovers

    if let Some(w) = wpm {
        if (130.0..=160.0).contains(&w) {
            s += 2;
        } else if (110.0..=180.0).contains(&w) {
            s += 1;
            findings.push(format!("voiceover WPM={:.0} slightly outside ideal 130-160 band", w));
        } else if w > 180.0 {
            findings.push(format!(
                "voiceover WPM={:.0} too fast (>180) — listeners can't keep up; target 130-160", w
            ));
        } else {
            findings.push(format!(
                "voiceover WPM={:.0} too slow (<110) — loses audience; target 130-160", w
            ));
        }
    }

    let unique_voices: HashSet<_> = voice_ids.iter().collect();
    if voice_ids.is_empty() {
        findings.push("voice_ids not reported — cannot verify voice consistency".into());
    } else if unique_voices.len() < voice_ids.len() {
        findings.push("duplicate voice IDs across speakers — each speaker should have a unique voice".into());
    } else {
        s += 1;
    }

    if emote_alignment_ok {
        s += 1;
    } else {
        findings.push("emote tags not aligned to content — use generate_voices with emote hints for natural prosody".into());
    }

    DimensionScore {
        id: "voiceover_quality".into(),
        label: "Voiceover quality & pacing".into(),
        score: s.min(6),
        max: 6,
        detail: serde_json::json!({
            "has_dialogue": has_dialogue,
            "voiceover_count": voiceover_count,
            "wpm": wpm,
            "unique_voices": unique_voices.len(),
            "emote_alignment_ok": emote_alignment_ok,
        }),
        findings,
    }
}
```

- [ ] **Step 4: Run tests and build**

```bash
cargo test -p openscript-core --lib -- voiceover_quality 2>&1 | tail -20
cargo build --workspace --exclude openscript-tauri 2>&1 | grep -E "^error|^warning|Finished"
cargo test --workspace --exclude openscript-tauri --lib --bins --tests 2>&1 | tail -10
```

- [ ] **Step 5: Commit**

```bash
git add crates/openscript-core/src/production_quality.rs
git commit -m "Phase 2b: Add score_voiceover_quality dimension (6 pts)"
git push github main
```

---

## Task 6: Add `score_audio_mix_quality` (5 pts)

**Files:**
- Modify: `crates/openscript-core/src/production_quality.rs`

**Interfaces:**
- Add to `RenderManifest` after `emote_alignment_ok`:

```rust
/// Integrated loudness in LUFS (EBU R128). Target: -16 ± 2.
#[serde(default)]
pub lufs: Option<f64>,
/// True peak level in dBFS. Must be < -1.
#[serde(default)]
pub peak_dbfs: Option<f64>,
/// Measured music ducking depth in dB during speech. Target: >=10 dB.
#[serde(default)]
pub ducking_depth_db: Option<f64>,
```

- New function: `fn score_audio_mix_quality(lufs: Option<f64>, peak_dbfs: Option<f64>, ducking_depth_db: Option<f64>, music_gain_db: f64, has_dialogue: bool) -> DimensionScore`

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn audio_mix_no_data_partial_score() {
    let d = score_audio_mix_quality(None, None, None, -12.0, true);
    assert_eq!(d.id, "audio_mix_quality");
    assert_eq!(d.max, 5);
    assert!(d.score <= 3);
}

#[test]
fn audio_mix_clipping_penalized() {
    let d = score_audio_mix_quality(Some(-14.0), Some(0.5), Some(12.0), -12.0, true);
    assert!(
        d.findings.iter().any(|f| f.contains("clip") || f.contains("peak")),
        "clipping should be flagged: {:?}", d.findings
    );
}

#[test]
fn audio_mix_lufs_out_of_range_penalized() {
    let d = score_audio_mix_quality(Some(-6.0), Some(-2.0), Some(10.0), -12.0, true);
    assert!(
        d.findings.iter().any(|f| f.contains("LUFS") || f.contains("loudness")),
        "LUFS violation should be flagged: {:?}", d.findings
    );
}

#[test]
fn audio_mix_ideal_scores_high() {
    let d = score_audio_mix_quality(Some(-16.0), Some(-3.0), Some(14.0), -12.0, true);
    assert!(d.score >= 4, "ideal audio mix should score >=4/5, got {}", d.score);
}
```

- [ ] **Step 2: Run to verify failure**

```bash
cargo test -p openscript-core --lib -- audio_mix 2>&1 | tail -20
```

- [ ] **Step 3: Implement `score_audio_mix_quality`**

```rust
/// Weight 5 — audio mix quality: LUFS compliance, clipping, ducking depth, gain balance.
fn score_audio_mix_quality(
    lufs: Option<f64>,
    peak_dbfs: Option<f64>,
    ducking_depth_db: Option<f64>,
    music_gain_db: f64,
    has_dialogue: bool,
) -> DimensionScore {
    let mut findings = Vec::new();
    let mut s = 0i32;

    // Music gain compliance
    if (-18.0..=-6.0).contains(&music_gain_db) {
        s += 1;
    } else {
        findings.push(format!("music gain_db={:.1} outside -18 to -6 dB safe band", music_gain_db));
    }

    // Peak level
    match peak_dbfs {
        Some(pk) if pk > -1.0 => {
            findings.push(format!(
                "HARD: audio clipping detected (peak={:.1} dBFS > -1 dBFS) — will distort on platforms",
                pk
            ));
        }
        Some(_) => { s += 1; }
        None => {
            findings.push("peak_dbfs not measured — run verify.render to check clipping".into());
        }
    }

    // LUFS compliance: -18 to -14 is the sweet spot
    match lufs {
        Some(l) if l > -14.0 => {
            findings.push(format!(
                "HARD: LUFS={:.1} exceeds -14 — too loud; normalize to -16 +/- 2",
                l
            ));
        }
        Some(l) if l < -18.0 => {
            findings.push(format!("LUFS={:.1} too quiet; target -16 +/- 2", l));
        }
        Some(_) => { s += 2; }
        None => {
            findings.push("lufs not measured — add loudnorm filter or run EBU R128 analysis".into());
        }
    }

    // Ducking effectiveness
    if has_dialogue {
        match ducking_depth_db {
            Some(d) if d >= 10.0 => { s += 1; }
            Some(d) if d >= 6.0 => {
                findings.push(format!("ducking depth {:.1} dB — target >=10 dB for clear speech", d));
            }
            Some(d) => {
                findings.push(format!(
                    "ducking depth {:.1} dB insufficient — music may mask speech (need >=10 dB)", d
                ));
            }
            None => {
                findings.push("ducking_depth_db not measured — verify sidechain ducking is active".into());
            }
        }
    } else {
        s += 1; // no speech = no ducking needed
    }

    DimensionScore {
        id: "audio_mix_quality".into(),
        label: "Audio mix quality (LUFS, peak, ducking)".into(),
        score: s.min(5),
        max: 5,
        detail: serde_json::json!({
            "lufs": lufs,
            "peak_dbfs": peak_dbfs,
            "ducking_depth_db": ducking_depth_db,
            "music_gain_db": music_gain_db,
            "has_dialogue": has_dialogue,
        }),
        findings,
    }
}
```

- [ ] **Step 4: Run tests and build**

```bash
cargo test -p openscript-core --lib -- audio_mix 2>&1 | tail -20
cargo build --workspace --exclude openscript-tauri 2>&1 | grep -E "^error|^warning|Finished"
cargo test --workspace --exclude openscript-tauri --lib --bins --tests 2>&1 | tail -10
```

- [ ] **Step 5: Commit**

```bash
git add crates/openscript-core/src/production_quality.rs
git commit -m "Phase 2c: Add score_audio_mix_quality dimension (5 pts)"
git push github main
```

---

## Task 7: Add `score_visual_hierarchy` (5 pts) + `score_platform_optimization` (5 pts)

**Files:**
- Modify: `crates/openscript-core/src/production_quality.rs`

**Interfaces:**
- Add to `RenderManifest` after `ducking_depth_db`:

```rust
/// Output aspect ratio (e.g. "9:16", "16:9", "1:1").
#[serde(default)]
pub aspect_ratio: Option<String>,
```

- New functions:
  - `fn score_visual_hierarchy(stickers: &[StickerLayerInfo], memes: &[MemeLayerInfo], sections: &[SectionInfo], captions_present: bool) -> DimensionScore`
  - `fn score_platform_optimization(duration_ms: i64, aspect_ratio: Option<&str>) -> DimensionScore`

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn visual_hierarchy_no_elements_low_score() {
    let d = score_visual_hierarchy(&[], &[], &[], false);
    assert_eq!(d.id, "visual_hierarchy");
    assert_eq!(d.max, 5);
    assert!(d.score <= 1);
}

#[test]
fn visual_hierarchy_full_stack_scores_high() {
    let stickers = vec![StickerLayerInfo {
        path: "a.gif".into(), start_ms: 0, end_ms: 5000,
        position: "top-left".into(), scale: 0.30,
    }];
    let memes = vec![MemeLayerInfo { path: "m.mp4".into(), start_ms: 5000, end_ms: 8000 }];
    let sections = vec![SectionInfo {
        role: SectionRole::Hook, start_ms: 0, end_ms: 5000,
        text: "Hook".into(), title_text: Some("BIG TITLE".into()),
    }];
    let d = score_visual_hierarchy(&stickers, &memes, &sections, true);
    assert!(d.score >= 4, "full stack should score >=4/5, got {}", d.score);
}

#[test]
fn platform_opt_vertical_short_scores_high() {
    let d = score_platform_optimization(30000, Some("9:16"));
    assert_eq!(d.id, "platform_optimization");
    assert_eq!(d.max, 5);
    assert!(d.score >= 4, "30s vertical should score >=4/5, got {}", d.score);
}

#[test]
fn platform_opt_landscape_too_long_penalized() {
    let d = score_platform_optimization(180000, Some("16:9"));
    assert!(d.findings.iter().any(|f| f.contains("9:16") || f.contains("aspect")));
    assert!(d.findings.iter().any(|f| f.contains("duration") || f.contains("long") || f.contains("90s")));
    assert!(d.score <= 2, "landscape+too-long should score <=2, got {}", d.score);
}
```

- [ ] **Step 2: Run to verify failure**

```bash
cargo test -p openscript-core --lib -- "visual_hierarchy\|platform_opt" 2>&1 | tail -20
```

- [ ] **Step 3: Implement `score_visual_hierarchy`**

```rust
/// Weight 5 — visual hierarchy: layered elements with clear focal points.
fn score_visual_hierarchy(
    stickers: &[StickerLayerInfo],
    memes: &[MemeLayerInfo],
    sections: &[SectionInfo],
    captions_present: bool,
) -> DimensionScore {
    let mut findings = Vec::new();
    let mut s = 0i32;

    let has_title_card = sections.iter().any(|sec| {
        sec.title_text.as_ref().map(|t| !t.is_empty()).unwrap_or(false)
    });
    if has_title_card { s += 1; }
    else { findings.push("no title cards — add title_text to hook/payoff sections".into()); }

    if !memes.is_empty() { s += 1; }
    else { findings.push("no reaction meme cuts — memes create motion hierarchy above static stickers".into()); }

    if !stickers.is_empty() { s += 1; }
    else { findings.push("no stickers — mid-level motion layer missing from visual hierarchy".into()); }

    if captions_present { s += 1; }
    else { findings.push("no captions — text anchor layer missing from visual hierarchy".into()); }

    let hook_has_visual = sections.iter().any(|sec| {
        matches!(sec.role, SectionRole::Hook) && sec.start_ms < 3000
    }) && (!stickers.is_empty() || has_title_card);
    if hook_has_visual { s += 1; }
    else { findings.push("hook lacks immediate visual element (sticker or title card in first 3s)".into()); }

    DimensionScore {
        id: "visual_hierarchy".into(),
        label: "Visual hierarchy (layers & focus)".into(),
        score: s.min(5),
        max: 5,
        detail: serde_json::json!({
            "has_title_card": has_title_card,
            "has_memes": !memes.is_empty(),
            "has_stickers": !stickers.is_empty(),
            "captions_present": captions_present,
            "hook_has_visual": hook_has_visual,
        }),
        findings,
    }
}
```

- [ ] **Step 4: Implement `score_platform_optimization`**

```rust
/// Weight 5 — platform optimization: aspect ratio, duration sweet spot.
fn score_platform_optimization(duration_ms: i64, aspect_ratio: Option<&str>) -> DimensionScore {
    let mut findings = Vec::new();
    let mut s = 0i32;

    match aspect_ratio {
        Some(ar) if ar == "9:16" => { s += 2; }
        Some(ar) if ar == "1:1" => {
            s += 1;
            findings.push("1:1 aspect — 9:16 vertical preferred for Shorts/Reels/TikTok".into());
        }
        Some(ar) => {
            findings.push(format!("aspect ratio '{}' not optimal — use 9:16", ar));
        }
        None => {
            s += 1; // assume correct if not reported
            findings.push("aspect_ratio not set in manifest — verify render config".into());
        }
    }

    let duration_s = duration_ms / 1000;
    if (15..=60).contains(&duration_s) {
        s += 2;
    } else if (60..=90).contains(&duration_s) {
        s += 1;
        findings.push(format!("duration {}s acceptable; 15-60s maximizes algorithm boost", duration_s));
    } else if duration_s < 15 {
        findings.push(format!("duration {}s too short — platform minimum ~15s", duration_s));
    } else {
        findings.push(format!("duration {}s exceeds 90s — keep short-form under 90s for retention", duration_s));
    }

    s += 1; // first-frame quality: award by default (no vision analysis available)

    DimensionScore {
        id: "platform_optimization".into(),
        label: "Platform optimization (ratio, duration)".into(),
        score: s.min(5),
        max: 5,
        detail: serde_json::json!({
            "aspect_ratio": aspect_ratio,
            "duration_s": duration_s,
            "sweet_spot_s": [15, 60],
        }),
        findings,
    }
}
```

- [ ] **Step 5: Run tests and build**

```bash
cargo test -p openscript-core --lib -- "visual_hierarchy\|platform_opt" 2>&1 | tail -20
cargo build --workspace --exclude openscript-tauri 2>&1 | grep -E "^error|^warning|Finished"
cargo test --workspace --exclude openscript-tauri --lib --bins --tests 2>&1 | tail -10
```

- [ ] **Step 6: Commit**

```bash
git add crates/openscript-core/src/production_quality.rs
git commit -m "Phase 3: Add score_visual_hierarchy (5 pts) + score_platform_optimization (5 pts)"
git push github main
```

---

## Task 8: Integration — wire all dimensions into `evaluate_production_quality` (v4.0 weights, 100 pts)

**Files:**
- Modify: `crates/openscript-core/src/production_quality.rs`

This task: (a) reduces `max` on 5 existing scorers, (b) replaces the aggregate body, (c) updates hard-fail logic, (d) adds v4.0 grade caps, (e) bumps `kpi_version`, (f) updates the existing `rich_manifest_scores_high` test.

**Weight verification (must sum to 100):**
10 + 8 + 8 + 8 + 5 + 8 + 6 + 8 + 6 + 6 + 5 + 8 + 5 + 5 + 4 = **100** ✓

- [ ] **Step 1: Update max values in 5 existing scorers**

In `score_video_source`:
- Change `max: 12` → `max: 10`
- Change `score = (avg * 12.0).round() as i32` → `score = (avg * 10.0).round() as i32`
- The `score.min(2)` procedural cap stays as-is.

In `score_visual_hooks`:
- Change `max: 10` → `max: 8`
- Change `real_ratio * 7.0` → `real_ratio * 6.0`
- Change `s += 3` (hook bonus) → `s += 2`
- Change `score.clamp(0, 10)` → `score.clamp(0, 8)`
- Change `score.min(4)` → `score.min(3)` (the `multi-scene with <2 real stock` cap)

In `score_visual_repetition`:
- Change `max: 12` → `max: 8`
- Change `uniqueness * 12.0` → `uniqueness * 8.0`
- Change `score.min(12)` → `score.min(8)`

In `score_context_relevance`:
- Change `max: 12` → `max: 8`
- Update the score ladder: `12 → 8`, `9 → 6`, `6 → 4`, `4 → 3`, `2 → 2`

In `score_cuts_pacing`:
- Change `max: 6` → `max: 5`
- Change ideal-band score `6` → `5`, slightly-outside `4` → `3`

- [ ] **Step 2: Replace the aggregate body in `evaluate_production_quality`**

Replace from `let theme = ...` through `let dimensions = vec![...]` with:

```rust
let theme = manifest.theme.as_deref();
let music_gain = manifest.music.as_ref().map(|m| m.gain_db).unwrap_or(-12.0);
let captions_present = captions_path
    .as_deref()
    .map(|p| !p.is_empty() && Path::new(p).exists())
    .unwrap_or(false);

let d_source  = score_video_source(&backgrounds);
let d_hooks   = score_visual_hooks(&backgrounds, duration_ms);
let d_repeat  = score_visual_repetition(&backgrounds);
let d_context = score_context_relevance(&backgrounds, &sections, &manifest.video_keywords);
let (d_cuts, cps) = score_cuts_pacing(&backgrounds, duration_ms);
let d_music   = score_music_quality(music.as_ref(), theme, &manifest.video_keywords);
let d_sfx     = score_sfx_quality(manifest.sfx_count, timeline);
let d_sticker = score_sticker_design_with_duration(&manifest.stickers, duration_ms);
let d_cap     = score_caption_quality(
    captions_path.as_deref(),
    manifest.caption_coverage_ratio,
    manifest.caption_style.as_deref(),
    manifest.caption_chars_per_second,
    manifest.caption_words_per_line,
);
let d_vo      = score_voiceover_quality(
    manifest.has_dialogue,
    manifest.voiceover_count,
    manifest.voiceover_wpm,
    &manifest.voice_ids,
    manifest.emote_alignment_ok,
);
let d_audio   = score_audio_mix_quality(
    manifest.lufs,
    manifest.peak_dbfs,
    manifest.ducking_depth_db,
    music_gain,
    manifest.has_dialogue,
);
let d_section = score_section_composition(&sections, &manifest.memes);
let d_hier    = score_visual_hierarchy(
    &manifest.stickers,
    &manifest.memes,
    &sections,
    captions_present,
);
let d_plat    = score_platform_optimization(duration_ms, manifest.aspect_ratio.as_deref());
let timeline_editor = score_timeline_editor(timeline);

// Scale utilization 0-100 → 0-4 pts (down from 0-8 in v3)
let editor_score = ((timeline_editor.utilization_score as f64) * 0.04).round() as i32;
let d_editor = DimensionScore {
    id: "timeline_editor".into(),
    label: "Timeline editor efficacious use".into(),
    score: editor_score.min(4),
    max: 4,
    detail: serde_json::to_value(&timeline_editor).unwrap_or(serde_json::json!({})),
    findings: timeline_editor.findings.clone(),
};

// v4.0: 10+8+8+8+5+8+6+8+6+6+5+8+5+5+4 = 100
let dimensions = vec![
    d_source, d_hooks, d_repeat, d_context, d_cuts,
    d_music, d_sfx, d_sticker, d_cap, d_vo,
    d_audio, d_section, d_hier, d_plat, d_editor,
];
```

- [ ] **Step 3: Update the `hard_fails` loop**

Find the `matches!` block and update to include new dimension IDs:

```rust
if d.score == 0
    && matches!(
        d.id.as_str(),
        "video_source_quality"
            | "visual_hooks"
            | "visual_repetition"
            | "music_quality"
            | "sfx_quality"
            | "speech_audio"
            | "caption_quality"
    )
{
    hard_fails.push(format!("{}: {}", d.id, f));
}
```

- [ ] **Step 4: Add v4.0 grade-cap logic** (insert after the existing hard-fail cap block)

```rust
// v4.0 grade caps for new hard gates
let sfx_hard = dimensions.iter()
    .find(|d| d.id == "sfx_quality")
    .map(|d| d.score == 0 && d.findings.iter().any(|f| f.contains("HARD")))
    .unwrap_or(false);
let lufs_hard = dimensions.iter()
    .find(|d| d.id == "audio_mix_quality")
    .map(|d| d.findings.iter().any(|f| f.contains("LUFS") && f.contains("HARD")))
    .unwrap_or(false);
let clip_hard = dimensions.iter()
    .find(|d| d.id == "audio_mix_quality")
    .map(|d| d.findings.iter().any(|f| f.contains("clipping") && f.contains("HARD")))
    .unwrap_or(false);
let cap_cps_hard = dimensions.iter()
    .find(|d| d.id == "caption_quality")
    .map(|d| d.findings.iter().any(|f| f.contains("CPS") && f.contains("unreadable")))
    .unwrap_or(false);

if sfx_hard && production_score > 69 {
    production_score = 69;
    hard_fails.push("SFX hard gate: no SFX -> grade capped C".into());
}
if lufs_hard && production_score > 69 {
    production_score = 69;
    hard_fails.push("LUFS hard gate: loudness out of -14 to -18 range -> grade capped C".into());
}
if cap_cps_hard && production_score > 69 {
    production_score = 69;
    hard_fails.push("Caption hard gate: CPS > 25 (unreadable) -> grade capped C".into());
}
if clip_hard && production_score > 54 {
    production_score = 54;
    hard_fails.push("Clipping hard gate: peak > -1 dBFS -> grade capped D".into());
}
// Recompute grade after all caps
grade = production_grade(production_score).to_string();
```

- [ ] **Step 5: Bump kpi_version**

Change `kpi_version: "3.0.0".into()` → `kpi_version: "4.0.0".into()`

- [ ] **Step 6: Update the module-level doc comment** (lines 1-16)

```rust
//! Production Quality Model — architecture-level KPIs for AI-directed shorts.
//!
//! `verify.render` is **technical integrity** only. This module scores whether the
//! timeline/render actually uses the editor like a director.
//!
//! Weights sum to 100. Grade bands:
//!   A 85–100 · B 70–84 · C 55–69 · D 40–54 · F <40
//!
//! v4.0 (2026-07-20):
//! - sfx_quality (6 pts) — SFX punctuation & variety
//! - music_quality (8 pts, expanded from music_variance)
//! - caption_quality (6 pts, expanded from captions)
//! - voiceover_quality (6 pts, new)
//! - audio_mix_quality (5 pts, new) — LUFS, peak, ducking
//! - visual_hierarchy (5 pts, new)
//! - platform_optimization (5 pts, new)
//! - hard gates: no-SFX->C, CPS>25->C, LUFS out-of-range->C, clipping->D
```

- [ ] **Step 7: Update `rich_manifest_scores_high` test**

Replace the `let mut tl = empty_timeline()` line and manifest initialization in that test:

```rust
fn rich_manifest_scores_high() {
    use crate::timeline::{EventKind, TimelineEvent};
    use crate::types::TrackType;
    let mut tl = empty_timeline();
    // Add two unique SFX events so sfx_quality scores >=4
    for (i, id) in ["whoosh_a", "pop_b"].iter().enumerate() {
        tl.tracks.entry(TrackType::Sfx).or_default().push(TimelineEvent {
            id: format!("sfx_{}", i),
            asset_id: id.to_string(),
            start_ms: (i as i64) * 8000,
            end_ms: (i as i64) * 8000 + 400,
            kind: EventKind::Sfx { editorial_role: "transition".to_string(), category: String::new(), subcategory: String::new(), duration_ms: 400, sample_rate: 44100, peak_db: -10.0, loudness_lufs: -18.0, recommended_gain_db: -10.0, recommended_use: String::new(), safe_overlay: true },
        });
    }
    // ... (rest of existing test body unchanged) ...
```

And update the last line of the manifest struct init:

```rust
// Change:
theme: None, sfx_count: 0, }
// To:
theme: None,
sfx_count: 2,
caption_coverage_ratio: 0.95,
caption_style: Some("word_highlight".into()),
voiceover_wpm: Some(145.0),
voice_ids: vec!["af_heart".into(), "bm_lewis".into()],
emote_alignment_ok: true,
aspect_ratio: Some("9:16".into()),
..Default::default()
```

- [ ] **Step 8: Run all tests**

```bash
cargo test --workspace --exclude openscript-tauri --lib --bins --tests 2>&1 | tail -20
```

Expected: >=248 tests pass. `rich_manifest_scores_high` must show `production_score >= 70`.

- [ ] **Step 9: Build with zero warnings**

```bash
cargo build --workspace --exclude openscript-tauri 2>&1 | grep -E "^error|^warning|Finished"
```

Expected: `Finished` with zero warning lines.

- [ ] **Step 10: Smoke test**

```bash
cargo build -p openscript-mcp --release --bin mcp-server 2>&1 | tail -5
bash scripts/smoke_test_mcp.sh 2>&1 | tail -10
```

Expected: `84 tools verified`, `hf.classify working correctly`.

- [ ] **Step 11: Commit and push**

```bash
git add crates/openscript-core/src/production_quality.rs
git commit -m "Phase 4: Integrate v4.0 dimensions into evaluate_production_quality (100 pts, kpi 4.0.0)"
git push github main
```

- [ ] **Step 12: Run post-iteration gate**

```bash
bash scripts/post-iteration.sh
```

Expected: `✓ POST-ITERATION GATE PASSED`

---

## Self-Review Checklist

**Spec coverage:**
- ✅ `score_sfx_quality` (6 pts) — Task 1
- ✅ `score_music_quality` (8 pts, renamed+expanded) — Task 2
- ✅ `score_sticker_design` expanded (overlap, off-screen, always-on) — Task 3
- ✅ `score_caption_quality` (6 pts, coverage, CPS, style) — Task 4
- ✅ `score_voiceover_quality` (6 pts) — Task 5
- ✅ `score_audio_mix_quality` (5 pts) — Task 6
- ✅ `score_visual_hierarchy` (5 pts) — Task 7
- ✅ `score_platform_optimization` (5 pts) — Task 7
- ✅ Weight redistribution to exactly 100 pts — Task 8
- ✅ Hard gates (no-SFX→C, CPS>25→C, LUFS→C, clipping→D) — Task 8
- ✅ `kpi_version: "4.0.0"` — Task 8
- ✅ `rich_manifest_scores_high` test updated to pass with new weights — Task 8
- ✅ Each task ends with `git push github main`

**No placeholders:** Every step has exact code. No TBDs. No "similar to Task N".

**Type consistency:**
- `score_sticker_design_with_duration` used in Task 8 aggregate — same name as defined in Task 3.
- `score_music_quality` replaces `score_music_variance` — one call site, updated in Task 2 step 4.
- `score_caption_quality` replaces `score_captions` in the aggregate — wired in Task 8 step 2.
- All new `RenderManifest` fields use `#[serde(default)]` — backward compatible.
