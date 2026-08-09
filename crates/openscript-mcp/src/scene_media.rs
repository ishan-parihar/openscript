//! Unified scene background acquisition (Phase 1 — fallback discipline).
//!
//! ONE chain for every caller (script.to_video, background.fetch):
//!
//!   Tier 1  user_library  → curated approved assets matching the scene signal
//!   Tier 2  pexels        → stock API (requires PEXELS_API_KEY)
//!   Tier 3  pixabay       → film-footage API (requires PIXABAY_API_KEY)
//!   Tier 4  youtube       → yt-dlp, OPT-IN (enable_youtube), vision-gated
//!   Tier 5  fallback_pool → caller-supplied non-procedural local paths
//!   Tier 6  procedural    → NEVER unless tiers 1–5 are all exhausted
//!
//! Every tier attempt is recorded in [`SceneMediaOutcome::exhausted`] so the
//! "why did this scene get a procedural cut" question is answerable at runtime
//! (previously script.to_video's inline chain omitted Pixabay entirely and
//! procedural could be reached while a valid fallback was never tried).
//!
//! YouTube tier policy (user decision): opt-in only, with a stricter lexical
//! bar AND a non-fail-open vision gate — if the vision backend is unavailable
//! the candidate is REJECTED rather than assumed good (its metadata is the
//! riskiest of all providers).

use std::collections::HashSet;

use crate::error::ToolError;

/// Minimum post-prior lexical relevance for a YouTube candidate when opted in.
pub const YT_MIN_LEXICAL: f64 = 0.25;
/// Source prior (multiplier on lexical) for YouTube — encodes "curated stock
/// beats social-platform clips" as an explicit weight.
pub const YT_SOURCE_PRIOR: f64 = 0.7;
/// Minimum user quality rating for a library asset in generation.
pub const LIBRARY_QUALITY_FLOOR: f64 = 3.0;

/// Per-scene acquisition request. The dedup sets are borrowed and mutated so
/// cross-scene non-redundancy is preserved (same clip never repeats).
pub struct SceneMediaRequest<'a> {
    pub query: String,
    pub signal_tokens: Vec<String>,
    pub scene_text: String,
    pub duration_s: f64,
    pub min_duration_s: f64,
    pub max_duration_s: f64,
    pub aspect: String,
    pub cache_dir: String,
    /// Output stem, e.g. "scene_001" or "clip" — files are written as
    /// `{cache_dir}/{stem}.mp4` (tier 4 fan-out appends `_b`).
    pub out_stem: String,
    pub scene_idx: usize,
    pub enable_youtube: bool,
    pub fallback_pool: Vec<String>,
    pub used_video_ids: &'a mut HashSet<String>,
    pub used_content_hashes: &'a mut HashSet<String>,
    pub used_pexels_ids: &'a mut HashSet<i64>,
}

/// Result of a unified acquisition attempt.
pub struct SceneMediaOutcome {
    pub clip_path: String,
    /// "user_library" | "pexels" | "pixabay" | "youtube" | "fallback_pool" | "procedural"
    pub source: String,
    pub provider_id: Option<String>,
    pub content_hash: String,
    pub search_query: String,
    pub lexical_score: f64,
    pub source_title: String,
    pub vision_score: Option<f64>,
    pub vision_reason: Option<String>,
    /// Every tier attempted + why it did not produce the clip.
    pub exhausted: Vec<String>,
    /// True when the last-resort procedural tier was used.
    pub fell_to_procedural: bool,
}

/// Run the full hierarchy for one scene.
pub async fn fetch_scene_background(
    req: SceneMediaRequest<'_>,
) -> Result<SceneMediaOutcome, ToolError> {
    // Destructure so the &mut dedup sets are usable by the tier helpers without
    // borrow-checker fights (they are reborrowed per tier).
    let SceneMediaRequest {
        query,
        signal_tokens,
        scene_text,
        duration_s,
        min_duration_s,
        max_duration_s,
        aspect,
        cache_dir,
        out_stem,
        scene_idx,
        enable_youtube,
        fallback_pool,
        used_video_ids,
        used_content_hashes,
        used_pexels_ids,
    } = req;
    let mut exhausted: Vec<String> = Vec::new();

    // ---- Tier 1: user library (curated beats providers) ----
    let lib = crate::asset_library::AssetLibrary::load().ok();
    let lib_hit = lib
        .as_ref()
        .and_then(|l| l.search(&signal_tokens, LIBRARY_QUALITY_FLOOR).first().cloned());
    if let Some(hit) = lib_hit {
        if let Some(mut l) = lib {
            l.mark_used(&hit.id);
            let _ = l.save();
        }
        tracing::info!(
            "[scene_media] scene {} tier=user_library asset={}",
            scene_idx + 1,
            hit.id
        );
        return Ok(SceneMediaOutcome {
            clip_path: hit.path,
            source: "user_library".to_string(),
            provider_id: None,
            content_hash: hit.content_hash,
            search_query: query,
            lexical_score: 1.0,
            source_title: hit.title,
            vision_score: None,
            vision_reason: None,
            exhausted,
            fell_to_procedural: false,
        });
    }
    exhausted.push("user_library: no approved match above floor".to_string());

    // ---- Tier 2: Pexels (requires key) ----
    if !crate::tools::pexels_key().is_empty() {
        if let Some(outcome) = tier_pexels(
            &query,
            &aspect,
            duration_s,
            min_duration_s,
            max_duration_s,
            &cache_dir,
            &out_stem,
            scene_idx,
            used_content_hashes,
            used_pexels_ids,
            &mut exhausted,
        )
        .await?
        {
            return Ok(outcome);
        }
    } else {
        exhausted.push("pexels: PEXELS_API_KEY unset".to_string());
    }

    // ---- Tier 3: Pixabay (requires key; film footage, not animation) ----
    if !crate::tools::pixabay_key().is_empty() {
        let out = format!("{cache_dir}/{out_stem}.mp4");
        if let Some(fetch) = crate::tools::fetch_pixabay_stock_clip_signal(
            &query,
            &signal_tokens,
            duration_s,
            min_duration_s,
            max_duration_s,
            &aspect,
            &out,
            used_video_ids,
            used_content_hashes,
        )
        .await
        {
            tracing::info!(
                "[scene_media] scene {} tier=pixabay id={} lex={:.2}",
                scene_idx + 1,
                fetch.video_id,
                fetch.lexical_score
            );
            return Ok(SceneMediaOutcome {
                clip_path: fetch.path,
                source: "pixabay".to_string(),
                provider_id: Some(fetch.video_id),
                content_hash: fetch.content_hash,
                search_query: fetch.search_query,
                lexical_score: fetch.lexical_score,
                source_title: fetch.source_title,
                vision_score: fetch.vision_score,
                vision_reason: fetch.vision_reason,
                exhausted,
                fell_to_procedural: false,
            });
        }
        exhausted.push("pixabay: no passing candidate".to_string());
    } else {
        exhausted.push("pixabay: PIXABAY_API_KEY unset — SKIPPED".to_string());
    }

    // ---- Tier 4: YouTube (opt-in, vision-gated, non-fail-open) ----
    if enable_youtube {
        if let Some(outcome) = tier_youtube(
            &query,
            &signal_tokens,
            &scene_text,
            duration_s,
            &aspect,
            &cache_dir,
            &out_stem,
            scene_idx,
            min_duration_s,
            max_duration_s,
            used_video_ids,
            used_content_hashes,
            &mut exhausted,
        )
        .await?
        {
            return Ok(outcome);
        }
    } else {
        exhausted.push("youtube: opted-out (enable_youtube / OPENSCRIPT_YT_FOR_GENERATION)".to_string());
    }

    // ---- Tier 5: fallback pool (caller-supplied non-procedural paths) ----
    for p in &fallback_pool {
        if !crate::tools::is_procedural_media_path(p) && std::path::Path::new(p).exists() {
            tracing::info!(
                "[scene_media] scene {} tier=fallback_pool path={}",
                scene_idx + 1,
                p
            );
            return Ok(SceneMediaOutcome {
                clip_path: p.clone(),
                source: "fallback_pool".to_string(),
                provider_id: None,
                content_hash: crate::tools::file_content_fingerprint(p).unwrap_or_default(),
                search_query: query,
                lexical_score: 0.0,
                source_title: String::new(),
                vision_score: None,
                vision_reason: None,
                exhausted,
                fell_to_procedural: false,
            });
        }
    }
    exhausted.push(if fallback_pool.is_empty() {
        "fallback_pool: empty".to_string()
    } else {
        "fallback_pool: no non-procedural existing path".to_string()
    });

    // ---- Tier 6: procedural (last resort, never silent) ----
    let proc_path = tier_procedural(&cache_dir, scene_idx, &used_content_hashes);
    let proc_path = match proc_path {
        Some(p) => p,
        None => {
            return Err(ToolError::Asset(
                "ALL stock tiers exhausted AND no procedural clip found in mcp/assets/backgrounds — set PEXELS_API_KEY or PIXABAY_API_KEY".to_string(),
            ))
        }
    };
    tracing::warn!(
        "[scene_media] scene {} fell to procedural — exhausted={:?}",
        scene_idx + 1,
        exhausted
    );
    let proc_hash = crate::tools::file_content_fingerprint(&proc_path).unwrap_or_default();
    Ok(SceneMediaOutcome {
        clip_path: proc_path,
        source: "procedural".to_string(),
        provider_id: None,
        content_hash: proc_hash,
        search_query: query,
        lexical_score: 0.0,
        source_title: String::new(),
        vision_score: None,
        vision_reason: None,
        exhausted,
        fell_to_procedural: true,
    })
}

/// Tier 2 — Pexels two-pass strategy: prefer clips that COVER the scene
/// duration (alternates, never looping); only if nothing covers, fall back to
/// the longest short clips (renderer loops as an explicit last resort).
#[allow(clippy::too_many_arguments)]
async fn tier_pexels(
    query: &str,
    aspect: &str,
    duration_s: f64,
    min_duration_s: f64,
    max_duration_s: f64,
    cache_dir: &str,
    out_stem: &str,
    scene_idx: usize,
    used_content_hashes: &mut HashSet<String>,
    used_pexels_ids: &mut HashSet<i64>,
    exhausted: &mut Vec<String>,
) -> Result<Option<SceneMediaOutcome>, ToolError> {
    let orientation = crate::tools::aspect_to_orientation(aspect);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| ToolError::Asset(format!("HTTP client error: {e}")))?;
    let key = crate::tools::pexels_key();

    let needed_dur = duration_s.max(3.0);
    // Honor explicit caller bounds (SEGMENTATION_ARCHITECTURE): the covering
    // pass filters by max_duration_s too; min is the max of the caller floor
    // and the scene length (never accept a clip shorter than the scene).
    let min_filter = needed_dur.max(min_duration_s);
    let mut covering: Vec<(String, i64)> = Vec::new(); // (file url, pexels id)
    let mut shorts: Vec<(i64, String, i64)> = Vec::new(); // (dur, url, id)

    // Pass 1 (pages 1-3): only clips that cover the scene duration.
    for page in 1..=3 {
        let url =
            crate::tools::pexels_search_url(query, orientation, page, min_filter, max_duration_s);
        let Ok(resp) = client.get(&url).header("Authorization", &key).send().await else {
            continue;
        };
        if !resp.status().is_success() {
            continue;
        }
        let Ok(body) = resp.json::<serde_json::Value>().await else {
            continue;
        };
        let Some(videos) = body.get("videos").and_then(|v| v.as_array()) else {
            continue;
        };
        for video in videos {
            let vid_id = video.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
            if vid_id > 0 && used_pexels_ids.contains(&vid_id) {
                continue;
            }
            let vid_dur = video.get("duration").and_then(|v| v.as_i64()).unwrap_or(0);
            let Some(url) = crate::tools::pexels_file_url(video) else {
                continue;
            };
            if (vid_dur as f64) >= needed_dur && covering.len() < 6 {
                covering.push((url, vid_id));
            }
        }
        if !covering.is_empty() {
            break;
        }
    }

    // Pass 2 (fallback): only if no alternate covers — longest short clips.
    if covering.is_empty() {
        for page in 1..=2 {
            let url = crate::tools::pexels_search_url(query, orientation, page, 0.0, 0.0);
            let Ok(resp) = client.get(&url).header("Authorization", &key).send().await else {
                continue;
            };
            if !resp.status().is_success() {
                continue;
            }
            let Ok(body) = resp.json::<serde_json::Value>().await else {
                continue;
            };
            let Some(videos) = body.get("videos").and_then(|v| v.as_array()) else {
                continue;
            };
            for video in videos {
                let vid_id = video.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
                if vid_id > 0 && used_pexels_ids.contains(&vid_id) {
                    continue;
                }
                let vid_dur = video.get("duration").and_then(|v| v.as_i64()).unwrap_or(0);
                if vid_dur < 3 {
                    continue;
                }
                let Some(url) = crate::tools::pexels_file_url(video) else {
                    continue;
                };
                shorts.push((vid_dur, url, vid_id));
            }
        }
        shorts.sort_by(|a, b| b.0.cmp(&a.0));
        shorts.truncate(4);
    }

    let candidates: Vec<(String, i64)> = covering
        .into_iter()
        .chain(shorts.into_iter().map(|(_, u, i)| (u, i)))
        .collect();
    if candidates.is_empty() {
        exhausted.push(format!(
            "pexels: 0 candidates (covering {needed_dur:.1}s) for '{}'",
            crate::tools::truncate_utf8(query, 60)
        ));
        return Ok(None);
    }

    // Resolve once per tier attempt: the NVENC/NVDEC availability probe is
    // OnceLock-cached, so this is an env read + two atomic loads after the
    // first call in the process.
    let gpu = openscript_ffmpeg::gpu::GpuConfig::resolve();
    for (url, vid_id) in candidates {
        let clip_path = format!("{cache_dir}/{out_stem}.mp4");
        let Ok(dl_resp) = client.get(url).send().await else {
            continue;
        };
        if !dl_resp.status().is_success() {
            continue;
        }
        let Ok(bytes) = dl_resp.bytes().await else {
            continue;
        };
        if std::fs::write(&clip_path, &bytes).is_err() {
            continue;
        }
        let crop_filter = crate::stock_signal::cover_crop_filter_for_aspect(aspect);
        let trimmed = format!("{cache_dir}/{out_stem}_trim.mp4");
        let trim_result = crate::tools::build_stock_trim_command(
            &gpu,
            &clip_path,
            &trimmed,
            duration_s,
            None,
            &crop_filter,
        )
        .output()
        .await;
        let chosen = if trim_result.as_ref().map(|o| o.status.success()).unwrap_or(false) {
            trimmed
        } else {
            tracing::warn!(
                "[pexels stock] trim FAILED id={vid_id} — falling back to untrimmed clip. ffmpeg: {}",
                trim_result
                    .as_ref()
                    .map(crate::tools::trim_stderr_tail)
                    .unwrap_or_default()
            );
            clip_path
        };
        // Geometry gate (no stretch).
        let geo = crate::stock_signal::probe_geometry(&chosen, aspect);
        if !geo.ok {
            tracing::warn!("[pexels stock] geometry reject id={vid_id} {:?}", geo.reasons);
            let _ = std::fs::remove_file(&chosen);
            continue;
        }
        // Content-hash dedup: reject if same bytes as a prior scene.
        if let Some(h) = crate::tools::file_content_fingerprint(&chosen) {
            if used_content_hashes.contains(&h) {
                let _ = std::fs::remove_file(&chosen);
                continue;
            }
            used_content_hashes.insert(h.clone());
            used_pexels_ids.insert(vid_id);
            tracing::info!(
                "[scene_media] scene {} tier=pexels id={} dur_needed={:.1}s",
                scene_idx + 1,
                vid_id,
                duration_s
            );
            return Ok(Some(SceneMediaOutcome {
                clip_path: chosen,
                source: "pexels".to_string(),
                provider_id: Some(vid_id.to_string()),
                content_hash: h,
                search_query: query.to_string(),
                lexical_score: 0.5, // Pexels metadata is reliable; no lexical gate
                source_title: format!("pexels_{vid_id}"),
                vision_score: None,
                vision_reason: None,
                exhausted: std::mem::take(exhausted),
                fell_to_procedural: false,
            }));
        }
    }
    exhausted.push("pexels: candidates failed download/geometry/dedup".to_string());
    Ok(None)
}

/// Tier 4 — YouTube (opt-in). Tries the query, then a signal-noun fan-out.
/// Non-fail-open: a candidate is accepted only when its post-prior lexical
/// clears the bar AND the vision frame-gate actually ran (Some score).
#[allow(clippy::too_many_arguments)]
async fn tier_youtube(
    query: &str,
    signal_tokens: &[String],
    scene_text: &str,
    duration_s: f64,
    aspect: &str,
    cache_dir: &str,
    out_stem: &str,
    scene_idx: usize,
    min_duration_s: f64,
    max_duration_s: f64,
    used_video_ids: &mut HashSet<String>,
    used_content_hashes: &mut HashSet<String>,
    exhausted: &mut Vec<String>,
) -> Result<Option<SceneMediaOutcome>, ToolError> {
    let yt_q = if query.to_ascii_lowercase().contains("stock") {
        query.to_string()
    } else {
        format!("{query} stock footage vertical")
    };
    let out = format!("{cache_dir}/{out_stem}.mp4");
    if let Some(fetch) = crate::tools::fetch_youtube_stock_clip_signal(
        &yt_q,
        signal_tokens,
        duration_s,
        aspect,
        &out,
        scene_idx,
        used_video_ids,
        used_content_hashes,
        scene_text,
        min_duration_s,
        max_duration_s,
    )
    .await
    {
        if yt_tier_accepts(fetch.lexical_score, fetch.vision_score) {
            tracing::info!(
                "[scene_media] scene {} tier=youtube id={} lex={:.2} vision={:?}",
                scene_idx + 1,
                fetch.video_id,
                fetch.lexical_score,
                fetch.vision_score
            );
            return Ok(Some(SceneMediaOutcome {
                clip_path: fetch.path,
                source: "youtube".to_string(),
                provider_id: Some(fetch.video_id),
                content_hash: fetch.content_hash,
                search_query: fetch.search_query,
                lexical_score: fetch.lexical_score,
                source_title: fetch.source_title,
                vision_score: fetch.vision_score,
                vision_reason: fetch.vision_reason,
                exhausted: std::mem::take(exhausted),
                fell_to_procedural: false,
            }));
        }
        exhausted.push(format!(
            "youtube: rejected lex={:.2} (prior {:.2}, floor {YT_MIN_LEXICAL}) vision={:?}",
            fetch.lexical_score,
            fetch.lexical_score * YT_SOURCE_PRIOR,
            fetch.vision_score
        ));
        return Ok(None);
    }
    exhausted.push("youtube: search/download failed".to_string());

    // Fan-out: scene nouns only, stock-phrased.
    if !signal_tokens.is_empty() {
        let short_q = signal_tokens.iter().take(4).cloned().collect::<Vec<_>>().join(" ");
        let out2 = format!("{cache_dir}/{out_stem}_b.mp4");
        if let Some(fetch) = crate::tools::fetch_youtube_stock_clip_signal(
            &format!("{short_q} stock footage vertical"),
            signal_tokens,
            duration_s,
            aspect,
            &out2,
            scene_idx,
            used_video_ids,
            used_content_hashes,
            scene_text,
            min_duration_s,
            max_duration_s,
        )
        .await
        {
            if yt_tier_accepts(fetch.lexical_score, fetch.vision_score) {
                tracing::info!(
                    "[scene_media] scene {} tier=youtube(fanout) id={} lex={:.2}",
                    scene_idx + 1,
                    fetch.video_id,
                    fetch.lexical_score
                );
                return Ok(Some(SceneMediaOutcome {
                    clip_path: fetch.path,
                    source: "youtube".to_string(),
                    provider_id: Some(fetch.video_id),
                    content_hash: fetch.content_hash,
                    search_query: fetch.search_query,
                    lexical_score: fetch.lexical_score,
                    source_title: fetch.source_title,
                    vision_score: fetch.vision_score,
                    vision_reason: fetch.vision_reason,
                    exhausted: std::mem::take(exhausted),
                    fell_to_procedural: false,
                }));
            }
            exhausted.push(format!(
                "youtube fan-out: rejected lex={:.2} vision={:?}",
                fetch.lexical_score, fetch.vision_score
            ));
        } else {
            exhausted.push("youtube fan-out: search/download failed".to_string());
        }
    }
    Ok(None)
}

/// Tier 6 — procedural: rotate the pre-generated clips, preferring one whose
/// content hash is unused this run (anti-repeat across scenes).
fn tier_procedural(
    _cache_dir: &str,
    scene_idx: usize,
    used_content_hashes: &HashSet<String>,
) -> Option<String> {
    let mut proc_candidates: Vec<String> = std::fs::read_dir("mcp/assets/backgrounds")
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.starts_with("procedural_") && n.ends_with(".mp4"))
                        .unwrap_or(false)
                })
                .filter_map(|p| p.to_str().map(|s| s.to_string()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    proc_candidates.sort();
    choose_procedural_clip(&proc_candidates, scene_idx, used_content_hashes)
}

/// Pure selection helper (unit-testable): prefer an unused clip, else rotate to
/// maximize repeat distance.
pub fn choose_procedural_clip(
    candidates: &[String],
    scene_idx: usize,
    used_hashes: &HashSet<String>,
) -> Option<String> {
    if candidates.is_empty() {
        return None;
    }
    for c in candidates {
        if let Some(h) = crate::tools::file_content_fingerprint(c) {
            if !used_hashes.contains(&h) {
                return Some(c.clone());
            }
        }
    }
    // All used this run — rotate; the least-recently-used ordering means
    // `scene_idx` modulo length maximizes distance between repeats.
    Some(candidates[scene_idx % candidates.len()].clone())
}

/// YouTube tier acceptance: source-prior-adjusted lexical clears the bar AND
/// the vision gate actually ran (non-fail-open — None means reject).
pub fn yt_tier_accepts(lexical: f64, vision: Option<f64>) -> bool {
    lexical * YT_SOURCE_PRIOR >= YT_MIN_LEXICAL && vision.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yt_tier_rejects_when_vision_gate_did_not_run() {
        // Non-fail-open: None vision → reject even with high lexical.
        assert!(!yt_tier_accepts(0.9, None));
        assert!(yt_tier_accepts(0.9, Some(0.8)));
    }

    #[test]
    fn yt_tier_applies_source_prior() {
        // 0.30 * 0.7 = 0.21 < 0.25 → reject; 0.40 * 0.7 = 0.28 → accept.
        assert!(!yt_tier_accepts(0.30, Some(0.8)));
        assert!(yt_tier_accepts(0.40, Some(0.8)));
    }

    #[test]
    fn choose_procedural_none_when_no_candidates() {
        assert!(choose_procedural_clip(&[], 0, &HashSet::new()).is_none());
    }

    #[test]
    fn choose_procedural_prefers_unused_then_rotates() {
        let cands = vec!["a.mp4".to_string(), "b.mp4".to_string(), "c.mp4".to_string()];
        let mut used = HashSet::new();
        used.insert("hash-b".to_string());
        // Prefer an unused file over rotation.
        let picked = choose_procedural_clip(&cands, 0, &used);
        assert!(picked.is_some());
        assert_ne!(picked.unwrap(), "b.mp4");
        // All used → rotation with max distance.
        let mut all = HashSet::new();
        all.insert("hash-a".to_string());
        all.insert("hash-b".to_string());
        all.insert("hash-c".to_string());
        assert_eq!(
            choose_procedural_clip(&cands, 1, &all).unwrap(),
            "b.mp4"
        );
        assert_eq!(
            choose_procedural_clip(&cands, 4, &all).unwrap(),
            "b.mp4"
        );
    }
}
