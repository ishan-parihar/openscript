// ---------------------------------------------------------------------------
// tools_broll — B-roll / background / segment handlers (agentic keyword pipeline, stock acquisition)
// Split out of tools.rs (pure-move refactor). `use super::*` grants access
// to tools.rs's helpers, imports, and re-exported sibling handlers.
// ---------------------------------------------------------------------------
use super::*;

pub(crate) async fn handle_broll_suggest(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let edl_path = extract_str(&args, "edl_path")?;
    let _srt_path = default_opt_str(&args, "srt_path");
    let cadence_seconds = default_f64(&args, "cadence_seconds", 2.0);

    let data = std::fs::read_to_string(edl_path)?;
    let timeline: serde_json::Value = serde_json::from_str(&data)?;

    let segments = timeline
        .get("segments")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let cadence_ms = (cadence_seconds * 1000.0) as i64;
    let mut suggestions = Vec::new();
    let mut position_ms = 0i64;

    for seg in &segments {
        let start = seg.get("start").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let end = seg.get("end").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let duration_ms = ((end - start) * 1000.0) as i64;
        // Derive concept from the segment caption instead of hardcoding "b-roll".
        // Use a salient noun/phrase from the caption — skip stopwords and short
        // words that produce garbage Pexels searches ("The", "But", "And").
        let caption = seg
            .get("caption")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let concept = {
            let kws = crate::keywords::extract_salient_keywords(caption, 3);
            if kws.is_empty() {
                "b-roll".to_string()
            } else {
                kws.join(" ")
            }
        };

        if duration_ms > cadence_ms * 2 {
            let mut t = 0i64;
            while t < duration_ms {
                let slot_duration = cadence_ms.min(duration_ms - t);
                suggestions.push(json!({
                    "position_ms": position_ms + t,
                    "duration_ms": slot_duration,
                    "concept": concept,
                }));
                t += cadence_ms;
            }
        }

        position_ms += duration_ms;
    }

    Ok(json!({
        "status": "success",
        "edl_path": edl_path,
        "suggestions": suggestions,
        "count": suggestions.len(),
    }))
}

pub(crate) async fn handle_broll_fetch(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_assets::pexels::PexelsClient;

    // Accept enriched_segments (from broll.keywords) OR concepts/keywords (flat array).
    let enriched_segments: Vec<serde_json::Value> = args.get("enriched_segments")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let max_kw_per_search = args.get("max_keywords_per_search")
        .and_then(|v| v.as_u64())
        .unwrap_or(3) as usize;

    // If enriched_segments provided, extract concepts from their keywords arrays.
    // Each segment's keywords are joined into a single search query for better Pexels results.
    let concepts_from_enriched: Vec<String> = if !enriched_segments.is_empty() {
        enriched_segments.iter().map(|seg| {
            let keywords = seg.get("keywords")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|k| k.as_str().map(String::from)).collect::<Vec<_>>())
                .unwrap_or_default();
            // Filter: keep keywords >= 3 chars (skip single-char noise like "par", "ko").
            // Multi-word phrases with spaces ("city skyline") are great Pexels queries.
            // Single words like "corruption", "protest" are also good — keep them.
            let good_kws: Vec<String> = keywords.into_iter()
                .filter(|k| k.len() >= 3)
                .take(max_kw_per_search)
                .collect();
            if good_kws.is_empty() {
                // Fallback: use first keyword >= 2 chars
                seg.get("keywords")
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|k| k.as_str())
                    .filter(|k| k.len() >= 2)
                    .unwrap_or("video")
                    .to_string()
            } else {
                good_kws.join(" ")
            }
        }).collect()
    } else if args.get("concepts").is_some() {
        extract_arr(&args, "concepts")?
    } else if let Some(s) = args.get("keywords").and_then(|v| v.as_str()) {
        vec![s.to_string()]
    } else if args.get("keywords").is_some() {
        extract_arr(&args, "keywords")?
    } else {
        return Err(ToolError::MissingArg(
            "concepts (or keywords) or enriched_segments".to_string(),
        ));
    };
    if concepts_from_enriched.is_empty() {
        return Err(ToolError::InvalidArg(
            "concepts/keywords must not be empty".into(),
        ));
    }
    let concepts = concepts_from_enriched;
    let asset_dir =
        default_opt_str(&args, "asset_dir").unwrap_or_else(|| "mcp/assets/broll_cache".to_string());
    let orientation = default_str(&args, "orientation", "9:16");
    let quality = default_str(&args, "quality", "sd");
    // Number of DISTINCT clips to download per concept. When the agent has
    // more segments than concepts (e.g. 44 segments / 12 concepts), downloading
    // several distinct clips per concept lets the auto-placer cycle through
    // them so consecutive segments don't reuse the same footage.
    let download_n = args
        .get("download_n")
        .and_then(|v| v.as_u64())
        .map(|n| n.max(1) as usize)
        .unwrap_or(1);
    let download_explicit = args.get("download").and_then(|v| v.as_bool());
    // Auto-enable download when enriched_segments + timeline_path are both
    // provided — auto-placement requires downloaded files on disk.
    let has_enriched = !enriched_segments.is_empty();
    let has_timeline = default_opt_str(&args, "timeline_path").is_some();
    let download = download_explicit.unwrap_or(has_enriched && has_timeline);
    // Local fallback clips used when Pexels returns nothing for a concept
    // (mirrors background.fetch's fallback_pool semantics).
    let fallback_pool: Vec<String> = args
        .get("fallback_pool")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let api_key = pexels_key();
    if api_key.is_empty() {
        // Bug #16 fix: do not hard-fail when PEXELS_API_KEY is missing.
        // Return status:warning with actionable guidance so an agent can
        // fall back to background.fetch (which has its own fallback chain)
        // or supply a fallback_pool.
        if fallback_pool.is_empty() {
            return Ok(json!({
                "status": "warning",
                "message": "PEXELS_API_KEY not set and no fallback_pool provided. Set the key in mcp/assets/.openscript_config.json, or provide a fallback_pool of local clip paths, or use background.fetch which has its own fallback chain.",
                "results": [],
                "total_concepts": concepts.len(),
                "missing_key": true,
            }));
        }
        // No key but caller supplied fallback_pool — return one fallback
        // entry per concept so downstream tools (broll.assign) can still
        // place something on the timeline.
        let results: Vec<serde_json::Value> = concepts
            .iter()
            .enumerate()
            .map(|(i, concept)| {
                let path = fallback_pool[i % fallback_pool.len()].clone();
                json!({
                    "concept": concept,
                    "videos": [],
                    "count": 0,
                    "cached_path": path,
                    "source": "fallback_pool",
                })
            })
            .collect();
        let mut downloaded: Vec<serde_json::Value> = Vec::new();
        for (i, concept) in concepts.iter().enumerate() {
            downloaded.push(json!({
                "concept": concept,
                "path": fallback_pool[i % fallback_pool.len()],
                "source": "fallback_pool",
            }));
        }
        return Ok(json!({
            "status": "warning",
            "message": "PEXELS_API_KEY not set; using fallback_pool only.",
            "results": results,
            "downloaded": downloaded,
            "total_concepts": concepts.len(),
            "missing_key": true,
        }));
    }

    let total = concepts.len();
    report_progress(0.0, total as f64, "Fetching b-roll...")
        .await
        .ok();

    let mut client = PexelsClient::new(&api_key, &asset_dir);
    // Non-repetition: clips already placed on this timeline (or chosen earlier
    // in THIS run) are excluded from candidate selection — the same footage
    // must never appear twice later in the sequence (b-roll-repeat bug).
    let mut used_ids: std::collections::HashSet<i64> = default_opt_str(&args, "timeline_path")
        .and_then(|tl| Timeline::load(&tl).ok())
        .map(|t| used_broll_video_ids(&t))
        .unwrap_or_default();
    let mut all_results = Vec::new();
    let mut downloaded = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    for (i, concept) in concepts.iter().enumerate() {
        report_progress(i as f64, total as f64, &format!("Searching: {}", concept))
            .await
            .ok();

        let videos = client
            .search(concept, &orientation, &quality)
            .await
            .map_err(|e| ToolError::Asset(e.to_string()))?;

        // Download up to `download_n` DISTINCT clips per concept (not just the
        // top hit). Distinct footage per segment is what breaks the "same clip,
        // different zoom/pan" illusion — reuse is only acceptable when the
        // source library is genuinely exhausted, and the verifier flags that.
        let mut cached_paths: Vec<String> = Vec::new();
        // path → real duration (Pexels metadata) for EACH downloaded clip, so
        // auto-place records the duration of the clip actually placed, not the
        // first result's. Without this, probe_broll_gaps compares the segment
        // window against the wrong clip's duration whenever the cursor cycles
        // to a different distinct clip (missed or false gaps).
        let mut path_durations: std::collections::HashMap<String, f64> =
            std::collections::HashMap::new();
        if download {
            let mut seen_ids: std::collections::HashSet<i64> = std::collections::HashSet::new();
            // Skip ids already placed on the timeline / chosen this run.
            let (cands, reused) = fresh_candidates(&videos, &used_ids, download_n);
            if reused > 0 {
                warnings.push(format!(
                    "concept '{}': Pexels returned {} candidate(s), all already used on this timeline — reusing {} distinct clip(s) (library exhausted for this concept). Widen the keywords to get truly unique footage.",
                    concept,
                    videos.len(),
                    reused
                ));
            }
            for v in cands {
                if !seen_ids.insert(v.id) {
                    continue;
                }
                used_ids.insert(v.id);
                let v_duration = v.duration as f64;
                match client.download_best(v, concept).await {
                    Ok(path) => {
                        path_durations.insert(path.clone(), v_duration);
                        cached_paths.push(path.clone());
                        downloaded.push((concept.clone(), path));
                    }
                    Err(e) => {
                        tracing::warn!("[broll.fetch] Download failed for {}: {}", concept, e)
                    }
                }
            }
        }
        let cached_path = cached_paths.first().cloned();

        let video_json: Vec<serde_json::Value> = videos
            .iter()
            .take(3)
            .map(|v| {
                json!({
                    "id": v.id,
                    "width": v.width,
                    "height": v.height,
                    "duration": v.duration,
                    "image": v.image,
                    "url": v.url,
                })
            })
            .collect();

        let mut result = json!({
            "concept": concept,
            "videos": video_json,
            "count": video_json.len(),
        });
        if let Some(path) = &cached_path {
            result["cached_path"] = json!(path);
        }
        if !cached_paths.is_empty() {
            // Distinct clips downloaded for this concept. The auto-placer
            // cycles through them so consecutive segments sharing a concept
            // still get different footage.
            result["cached_paths"] = json!(cached_paths);
        }
        if !path_durations.is_empty() {
            // Per-path durations so auto-place can store the duration of the
            // clip it actually placed (see path_durations in the download
            // loop above).
            let dur_map: serde_json::Map<String, serde_json::Value> = path_durations
                .iter()
                .map(|(p, d)| (p.clone(), json!(d)))
                .collect();
            result["cached_path_durations"] = json!(dur_map);
        }
        // Record the source clip's real duration (from Pexels metadata) so
        // timeline.validate / verify.production can compare it against the
        // segment window without re-probing. Short clips become coverage
        // gaps (broll_gaps) instead of silently looping.
        if let Some(first) = videos.first() {
            result["source_duration_s"] = json!(first.duration);
        }

        // Per-concept fallback: if Pexels returned nothing, try fallback_pool
        // so downstream tools (broll.assign) still have a path to use.
        if video_json.is_empty() && !fallback_pool.is_empty() {
            let fallback_path = fallback_pool[i % fallback_pool.len()].clone();
            result["cached_path"] = json!(&fallback_path);
            result["source"] = json!("fallback_pool");
            warnings.push(format!(
                "concept '{}' returned 0 Pexels results — using fallback_pool entry",
                concept
            ));
            if download {
                downloaded.push((concept.clone(), fallback_path));
            }
        }
        all_results.push(result);
    }

    report_progress(total as f64, total as f64, "B-roll fetch complete")
        .await
        .ok();

    // Status is "warning" if any concept returned 0 videos (mirrors
    // background.fetch's behaviour of warning when falling back).
    let any_empty = all_results
        .iter()
        .any(|r| r.get("count").and_then(|v| v.as_u64()).unwrap_or(0) == 0);
    let status = if any_empty { "warning" } else { "fetched" };

    let mut resp = json!({
        "status": status,
        "results": all_results,
        "total_concepts": concepts.len(),
    });
    if !downloaded.is_empty() {
        resp["downloaded"] = json!(downloaded
            .iter()
            .map(|(c, p)| json!({"concept": c, "path": p}))
            .collect::<Vec<_>>());
    }
    if !warnings.is_empty() {
        resp["warnings"] = json!(warnings);
    }

    // AUTO-PLACE: If timeline_path provided, place each clip on timeline
    let timeline_path = default_opt_str(&args, "timeline_path");
    // Priority: enriched_segments > segments arg > timeline segments
    let placement_segments = if !enriched_segments.is_empty() {
        enriched_segments
    } else {
        args.get("segments")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
    };

    if let Some(ref tl_path) = timeline_path {
        // Load segments from timeline if not provided in args/enriched_segments
        let segments = if !placement_segments.is_empty() {
            placement_segments
        } else if std::path::Path::new(tl_path).exists() {
            // Read segments directly from the timeline JSON
            if let Ok(tl_str) = std::fs::read_to_string(tl_path) {
                if let Ok(tl_val) = serde_json::from_str::<serde_json::Value>(&tl_str) {
                    tl_val.get("segments")
                        .and_then(|s| s.as_array())
                        .cloned()
                        .unwrap_or_default()
                } else { Vec::new() }
            } else { Vec::new() }
        } else { Vec::new() };
        if !segments.is_empty() {
            let mut tl = Timeline::load(tl_path)
                .map_err(|e| ToolError::Io(std::io::Error::other(format!("Failed to load timeline: {}", e))))?;
            // Idempotent re-placement: drop b-roll events previously placed by
            // broll.fetch (they are regenerable) so re-runs — e.g. video.to_video
            // reusing the same timeline file — never stack stale clips on top of
            // fresh ones (the 44-event BROLL_REPEAT false positive: old events
            // kept their broll_{i} asset ids, colliding with the new placement).
            // Non-broll.fetch events (manual b-roll, broll_bg, script.to_timeline
            // background) are untouched.
            let prev_count = tl
                .tracks
                .get_mut(&TrackType::Broll)
                .map(|evs| {
                    let before = evs.len();
                    evs.retain(|e| {
                        !matches!(
                            e.provenance.as_ref().map(|p| p.tool.as_str()),
                            Some("broll.fetch")
                        )
                    });
                    before - evs.len()
                })
                .unwrap_or(0);
            if prev_count > 0 {
                // Prune asset records orphaned by the event clear (ids no
                // longer referenced by any remaining b-roll event).
                let keep_ids: std::collections::HashSet<String> = tl
                    .tracks
                    .get(&TrackType::Broll)
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .map(|e| e.asset_id.clone())
                    .collect();
                tl.assets.broll.retain(|k, _| keep_ids.contains(k));
                tracing::info!(
                    "[broll.fetch] cleared {} previously placed broll.fetch event(s) for idempotent re-place",
                    prev_count
                );
            }
            let mut assigned_count = 0usize;
            // Distribute clips to segments. When there are MORE segments than
            // concepts (the common 44-seg/12-concept case), cycle through each
            // concept's DISTINCT downloaded clips (`cached_paths`) so adjacent
            // segments reuse the same footage only when the pool is exhausted.
            let mut concept_cursor: std::collections::HashMap<usize, usize> =
                std::collections::HashMap::new();
            // V2V alternation (docs/V2V_ALTERNATION_ARCHITECTURE.md): when the
            // timeline is in alternate mode, "source"-role segments are the
            // ORIGINAL video — they must NOT get a b-roll event (an event there
            // would cover the original footage and break the alternation). Only
            // "broll"-role segments are placed. Non-alternate timelines (the
            // default) behave exactly as before (every segment is b-roll).
            let in_alternation = tl.directives.presentation.is_alternate();
            for (i, segment) in segments.iter().enumerate() {
                let seg_id = segment
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if in_alternation && tl.directives.presentation.role_for(&seg_id) == openscript_core::presentation::ROLE_SOURCE {
                    tracing::debug!(
                        "[broll.fetch] skipping source-role segment {} (V2V alternation) — original video shows here",
                        seg_id
                    );
                    continue;
                }
                let result_val = &all_results[i % all_results.len()];
                let concept_idx = i % all_results.len();
                // Advance a per-concept cursor through the distinct clip pool.
                let pool: Vec<String> = result_val
                    .get("cached_paths")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|p| p.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_else(|| {
                        result_val
                            .get("cached_path")
                            .and_then(|v| v.as_str())
                            .filter(|p| !p.is_empty() && *p != "placeholder")
                            .map(|p| vec![p.to_string()])
                            .unwrap_or_default()
                    });
                if pool.is_empty() {
                    continue;
                }
                let cursor = concept_cursor.entry(concept_idx).or_insert(0);
                let cached_path = pool[*cursor % pool.len()].clone();
                *cursor += 1;
                let start_s = segment.get("start_s")
                    .or_else(|| segment.get("start"))
                    .and_then(|v| v.as_f64()).unwrap_or(0.0);
                let end_s = segment.get("end_s")
                    .or_else(|| segment.get("end"))
                    .and_then(|v| v.as_f64()).unwrap_or(start_s + 3.0);
                let concept_str = result_val.get("concept")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let position_ms = (start_s * 1000.0) as i64;
                let duration_ms = ((end_s - start_s) * 1000.0) as i64;
                if duration_ms <= 0 { continue; }let event_id = format!("broll_{}", i);
                    let asset_id = event_id.clone();
                let broll_event = openscript_core::timeline::TimelineEvent {
                    id: event_id.clone(),
                    asset_id: asset_id.clone(),
                    start_ms: position_ms,
                    end_ms: position_ms + duration_ms,
                    offset_ms: 0,
                    gain_db: 0.0,
                    fade_in_ms: 0,
                    fade_out_ms: 0,
                    tags: vec![concept_str.to_string()],
                    provenance: Some(openscript_core::timeline::Provenance {
                        tool: "broll.fetch".to_string(),
                        editorial_role: None,
                        concept: Some(concept_str.to_string()),
                    }),
                    kind: openscript_core::timeline::EventKind::Broll {
                        concept: concept_str.to_string(),
                        source_provider: "pexels".to_string(),
                        transition_style: "cut".to_string(),
                        crop_mode: "center".to_string(),
                        orientation: orientation.clone(),
                        motion_intensity: "medium".to_string(),
                    },
                };
                tl.tracks.entry(openscript_core::types::TrackType::Broll)
                    .or_default()
                    .push(broll_event);
                // Persist the source clip's real duration (from Pexels metadata)
                // so verify.production / timeline.validate can compare it
                // against the segment window without re-probing — short clips
                // become coverage gaps (broll_gaps) instead of silently looping.
                // Use the duration of the clip ACTUALLY placed (per-path map
                // from the download loop), falling back to the result-wide
                // first-video hint only when the placed clip is the first one.
                let mut asset_record = serde_json::json!({
                    "path": cached_path,
                    "concept": concept_str,
                });
                let placed_duration = result_val
                    .get("cached_path_durations")
                    .and_then(|v| v.as_object())
                    .and_then(|m| m.get(&cached_path))
                    .and_then(|v| v.as_f64());
                if let Some(d) = placed_duration {
                    asset_record["source_duration_s"] = json!(d);
                } else if let Some(d) = result_val
                    .get("source_duration_s")
                    .and_then(|v| v.as_f64())
                {
                    asset_record["source_duration_s"] = json!(d);
                }
                tl.assets.broll.insert(asset_id.clone(), asset_record);
                assigned_count += 1;
            }
            tl.updated_at = chrono::Utc::now();
            tl.save(tl_path)
                .map_err(|e| ToolError::Io(std::io::Error::other(format!("Failed to save timeline: {}", e))))?;
            resp["timeline_path"] = json!(tl_path);
            resp["auto_assigned"] = json!(assigned_count);
            if assigned_count > 0 {
                resp["status"] = json!("placed");
            }
        }
    }

    Ok(resp)
}

pub(crate) async fn handle_broll_assign(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let timeline_path = extract_str(&args, "timeline_path")?;
    let concept = extract_str(&args, "concept")?;
    let position_ms = extract_i64(&args, "position_ms")?;
    let duration_ms = extract_i64(&args, "duration_ms")?;
    let asset_path = default_opt_str(&args, "asset_path");
    let transition_style = default_str(&args, "transition_style", "cut");
    let crop_mode = default_str(&args, "crop_mode", "center");
    // Expose fade_in_ms / fade_out_ms as parameters (were hardcoded to 0).
    let fade_in_ms = default_u32(&args, "fade_in_ms", 0);
    let fade_out_ms = default_u32(&args, "fade_out_ms", 0);

    let mut timeline = Timeline::load(timeline_path)?;
    let event_id = format!("broll_{:03}", track_count(&timeline, &TrackType::Broll) + 1);

    let resolved_path = asset_path.unwrap_or_else(|| {
        let cache_dir = "mcp/assets/broll_cache";
        if let Ok(entries) = std::fs::read_dir(cache_dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(&concept.replace(' ', "_")) && name.ends_with(".mp4") {
                    return entry.path().to_string_lossy().to_string();
                }
            }
        }
        // No match found — return empty string so the existence check below catches it
        String::new()
    });

    // If the resolved path doesn't exist on disk, use "placeholder" so the
    // render pipeline skips this event instead of crashing ffmpeg with a
    // glob pattern or non-existent path.
    let (asset_id, asset_registry_path, matched) = if resolved_path.is_empty()
        || resolved_path.contains("placeholder")
        || !std::path::Path::new(&resolved_path).exists()
    {
        ("placeholder".to_string(), "placeholder".to_string(), false)
    } else {
        (resolved_path.clone(), resolved_path.clone(), true)
    };

    let event = openscript_core::timeline::TimelineEvent {
        id: event_id.clone(),
        asset_id: asset_id.clone(),
        start_ms: position_ms,
        end_ms: position_ms + duration_ms,
        offset_ms: 0,
        gain_db: 0.0,
        fade_in_ms,
        fade_out_ms,
        tags: vec![concept.to_string()],
        provenance: Some(openscript_core::timeline::Provenance {
            tool: "broll.assign".into(),
            editorial_role: None,
            concept: Some(concept.to_string()),
        }),
        kind: openscript_core::timeline::EventKind::Broll {
            concept: concept.to_string(),
            source_provider: asset_id.clone(),
            transition_style,
            crop_mode,
            orientation: "9:16".into(),
            motion_intensity: "medium".into(),
        },
    };

    timeline.add_track_event(TrackType::Broll, event);
    timeline.add_asset(
        "broll",
        event_id.clone(),
        json!({"path": asset_registry_path}),
    );
    timeline.save(timeline_path)?;

    // Fix: return status "warning" (not "assigned") when no asset matched,
    // mirroring sfx.assign's pattern. Prior versions returned "assigned" with
    // asset_id:"placeholder", silently losing the agent's intent — the render
    // pipeline drops placeholder events, so the agent never knew the b-roll
    // slot was empty.
    let (status, message) = if matched {
        (
            "assigned",
            format!("B-roll assigned for concept '{}' at {} ms", concept, position_ms),
        )
    } else {
        (
            "warning",
            format!(
                "No b-roll asset found for concept '{}' at {} ms. Placeholder event created — render will skip this event. Use broll.fetch or background.fetch to download a real asset, then re-assign.",
                concept, position_ms
            ),
        )
    };

    Ok(json!({
        "status": status,
        "matched": matched,
        "message": message,
        "event_id": event_id,
        "asset_id": asset_id,
        "asset_path": asset_registry_path,
        "position_ms": position_ms,
        "duration_ms": duration_ms,
        "timeline_path": timeline_path,
    }))
}

pub(crate) async fn handle_timeline_autofill_broll(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let timeline_path = extract_str(&args, "timeline_path")?;
    let cadence_seconds = default_f64(&args, "cadence_seconds", 2.0);
    let _orientation = default_str(&args, "orientation", "9:16");
    let _quality = default_str(&args, "quality", "sd");
    let max_gaps = default_u32(&args, "max_gaps", 20);

    let mut timeline = Timeline::load(timeline_path)?;

    let cadence_ms = (cadence_seconds * 1000.0) as i64;
    let total_ms = timeline.total_duration_ms();
    let mut count = 0;
    let mut position_ms = 0i64;

    report_progress(0.0, max_gaps as f64, "Auto-filling b-roll slots...")
        .await
        .ok();

    while position_ms < total_ms && count < max_gaps as i64 {
        let duration = cadence_ms.min(total_ms - position_ms);
        if duration > 0 {
            let event_id = format!("broll_{:03}", track_count(&timeline, &TrackType::Broll) + 1);
            let concept = timeline
                .segments
                .iter()
                .find(|s| {
                    let seg_start = (s.start * 1000.0) as i64;
                    let seg_end = (s.end * 1000.0) as i64;
                    position_ms >= seg_start && position_ms < seg_end
                })
                .map(|s| {
                    s.caption
                        .split_whitespace()
                        .take(2)
                        .collect::<Vec<_>>()
                        .join("_")
                })
                .unwrap_or_else(|| "general".into());

            let event = openscript_core::timeline::TimelineEvent {
                id: event_id.clone(),
                asset_id: "placeholder".into(),
                start_ms: position_ms,
                end_ms: position_ms + duration,
                offset_ms: 0,
                gain_db: 0.0,
                fade_in_ms: 0,
                fade_out_ms: 0,
                tags: vec![concept.clone()],
                provenance: Some(openscript_core::timeline::Provenance {
                    tool: "timeline.autofill_broll".into(),
                    editorial_role: None,
                    concept: Some(concept.clone()),
                }),
                kind: openscript_core::timeline::EventKind::Broll {
                    concept,
                    source_provider: "placeholder".into(),
                    transition_style: "cut".into(),
                    crop_mode: "center".into(),
                    orientation: "9:16".into(),
                    motion_intensity: "medium".into(),
                },
            };

            timeline.add_track_event(TrackType::Broll, event);
            count += 1;

            // Report progress every 5 slots to avoid spamming
            if count % 5 == 0 || count == max_gaps as i64 {
                report_progress(
                    count as f64,
                    max_gaps as f64,
                    &format!("Filled {} b-roll slots", count),
                )
                .await
                .ok();
            }
        }
        position_ms += cadence_ms;
    }

    timeline.save(timeline_path)?;

    Ok(json!({
        "status": "autofilled",
        "timeline_path": timeline_path,
        "broll_events_added": count,
    }))
}

/// Generate basic keyword suggestions from a caption for Pexels search.
pub(crate) async fn handle_broll_plan(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let timeline_path = extract_str(&args, "timeline_path")?;
    let timeline_str = std::fs::read_to_string(timeline_path)
        .map_err(|e| ToolError::InvalidArg(format!("Failed to read timeline {}: {}", timeline_path, e)))?;
    let timeline: serde_json::Value = serde_json::from_str(&timeline_str)
        .map_err(|e| ToolError::InvalidArg(format!("Failed to parse timeline JSON: {}", e)))?;
    // Try tracks.dialogue.events first, then top-level segments, then default empty.
    // Note: dialogue may be a list (empty) instead of a dict with 'events' — handle both.
    let segments = timeline.get("tracks")
        .and_then(|tracks| tracks.get("dialogue"))
        .and_then(|dialogue| {
            // dialogue may be {"events": [...]} or a plain list [...]
            dialogue.get("events")
                .and_then(|e| e.as_array().cloned())
                .filter(|v| !v.is_empty())
                .or_else(|| dialogue.as_array().cloned().filter(|v| !v.is_empty()))
        })
        .or_else(|| {
            timeline.get("segments")
                .and_then(|s| s.as_array().cloned())
                .filter(|v| !v.is_empty())
        })
        .unwrap_or_default();
    let mut result_segments = Vec::new();
    for (idx, seg) in segments.iter().enumerate() {
    let start_s = seg.get("start_s")
        .or_else(|| seg.get("start_ms"))
        .or_else(|| seg.get("start")).and_then(|v| v.as_f64())
        .map(|v| if v > 1000.0 { v / 1000.0 } else { v })
        .unwrap_or(0.0);
    let end_s = seg.get("end_s")
        .or_else(|| seg.get("end_ms"))
        .or_else(|| seg.get("end")).and_then(|v| v.as_f64())
        .map(|v| if v > 1000.0 { v / 1000.0 } else { v })
        .unwrap_or(start_s + 5.0);
        let caption = seg.get("caption")
            .or_else(|| seg.get("text")).and_then(|v| v.as_str())
            .unwrap_or("");
        let duration_s = end_s - start_s;
        result_segments.push(json!({
            "id": format!("seg_{}", idx),
            "start_s": start_s,
            "end_s": end_s,
            "duration_s": duration_s,
            "caption": caption,
        }));
    }
    Ok(json!({
        "status": "success",
        "segments_count": result_segments.len(),
        "segments": result_segments,
    }))
}

pub(crate) async fn handle_broll_keywords(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    // Extract segments from args
    let segments = args.get("segments")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ToolError::MissingArg("segments".to_string()))?;

    if segments.is_empty() {
        return Ok(json!({
            "status": "warning",
            "message": "No segments provided",
            "segments": [],
        }));
    }

    let video_title = args.get("video_title")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let language = args.get("language")
        .and_then(|v| v.as_str())
        .unwrap_or("hinglish");

    let max_batch_size = args.get("max_batch_size")
        .and_then(|v| v.as_u64())
        .unwrap_or(15).max(1) as usize;
    let _ = max_batch_size; // batching is owned by keywords::draft_scene_keywords

    // Phase 158: covered concepts from the timeline (non-redundant draft pass).
    let covered_concepts: Vec<String> = if let Some(tl_path) = default_opt_str(&args, "timeline_path") {
        match Timeline::load(&tl_path) {
            Ok(tl) => {
                let mut concepts: Vec<String> = Vec::new();
                if let Some(broll) = tl.tracks.get(&TrackType::Broll) {
                    for ev in broll {
                        if let openscript_core::timeline::EventKind::Broll { concept, .. } = &ev.kind {
                            if !concept.is_empty() && !concepts.contains(concept) {
                                concepts.push(concept.clone());
                            }
                        }
                    }
                }
                concepts
            }
            Err(_) => Vec::new(),
        }
    } else {
        Vec::new()
    };

    // Unified draft: one batched LLM call (visual + reactions per segment) with
    // id-echo, missing-id redraft, and the salience fallback — the SAME module
    // used by script.to_video, sticker.keywords, and broll.auto.
    let mut draft_inputs: Vec<crate::keywords::SegmentInput> = Vec::with_capacity(segments.len());
    for (i, seg) in segments.iter().enumerate() {
        let id = seg.get("id")
            .and_then(|v| v.as_str())
            .unwrap_or(&format!("seg_{}", i))
            .to_string();
        let caption = seg.get("caption")
            .or_else(|| seg.get("text"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let start_s = seg.get("start_s")
            .or_else(|| seg.get("start_ms"))
            .or_else(|| seg.get("start"))
            .and_then(|v| v.as_f64())
            .map(|v| if v > 1000.0 { v / 1000.0 } else { v })
            .unwrap_or(0.0);
        let end_s = seg.get("end_s")
            .or_else(|| seg.get("end_ms"))
            .or_else(|| seg.get("end"))
            .and_then(|v| v.as_f64())
            .map(|v| if v > 1000.0 { v / 1000.0 } else { v })
            .unwrap_or(start_s + 3.0);
        draft_inputs.push(crate::keywords::SegmentInput {
            segment_id: id,
            caption,
            language_hint: if language.is_empty() { None } else { Some(language.to_string()) },
            duration_s: (end_s - start_s).max(0.0),
            scene_idx: i,
            total_scenes: segments.len(),
            video_title: video_title.to_string(),
            video_keywords: Vec::new(),
            covered_concepts: covered_concepts.clone(),
        });
    }

    report_progress(30.0, 100.0, "Drafting visual keywords (unified)...").await.ok();
    let drafted = crate::keywords::draft_scene_keywords(&draft_inputs).await;

    let mut last_backend = String::new();
    let mut last_model = String::new();
    for d in &drafted {
        if d.source != crate::keywords::KeywordSource::Heuristic {
            let parts: Vec<&str> = d.backend.split('/').collect();
            if parts.len() == 2 && last_backend.is_empty() {
                last_backend = parts[0].to_string();
                last_model = parts[1].to_string();
            }
        }
    }

    report_progress(90.0, 100.0, "Assembling results...").await.ok();

    // Build the output: enrich each segment with the unified visual keywords.
    let mut enriched_segments = Vec::new();
    for (i, seg) in segments.iter().enumerate() {
        let id = seg.get("id")
            .and_then(|v| v.as_str())
            .unwrap_or(&format!("seg_{}", i))
            .to_string();
        let caption = seg.get("caption")
            .or_else(|| seg.get("text"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let start_s = seg.get("start_s")
            .or_else(|| seg.get("start_ms"))
            .or_else(|| seg.get("start"))
            .and_then(|v| v.as_f64())
            .map(|v| if v > 1000.0 { v / 1000.0 } else { v })
            .unwrap_or(0.0);
        let end_s = seg.get("end_s")
            .or_else(|| seg.get("end_ms"))
            .or_else(|| seg.get("end"))
            .and_then(|v| v.as_f64())
            .map(|v| if v > 1000.0 { v / 1000.0 } else { v })
            .unwrap_or(start_s + 3.0);

        let keywords = drafted
            .get(i)
            .map(|d| d.visual.clone())
            .unwrap_or_default();

        enriched_segments.push(json!({
            "id": id,
            "start_s": start_s,
            "end_s": end_s,
            "duration_s": end_s - start_s,
            "caption": caption,
            "keywords": keywords,
        }));
    }

    report_progress(100.0, 100.0, "Keyword extraction complete.").await.ok();

    Ok(json!({
        "status": "success",
        "backend": last_backend,
        "model": last_model,
        "segments_count": enriched_segments.len(),
        "segments": enriched_segments,
    }))
}

pub(crate) async fn handle_broll_validate_keywords(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    use openscript_assets::pexels::PexelsClient;

    let enriched_segments: Vec<serde_json::Value> = args
        .get("enriched_segments")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if enriched_segments.is_empty() {
        return Err(ToolError::MissingArg(
            "enriched_segments (from broll.keywords)".to_string(),
        ));
    }
    let max_candidates = args
        .get("max_candidates")
        .and_then(|v| v.as_u64())
        .unwrap_or(6)
        .max(2) as usize;
    let max_keywords = args
        .get("max_keywords_per_search")
        .and_then(|v| v.as_u64())
        .unwrap_or(2)
        .max(1) as usize;
    let orientation = default_str(&args, "orientation", "9:16");
    let quality = default_str(&args, "quality", "sd");
    let asset_dir = default_opt_str(&args, "asset_dir")
        .unwrap_or_else(|| "mcp/assets/broll_cache".to_string());
    let language = default_str(&args, "language", "hinglish");

    let api_key = pexels_key();
    if api_key.is_empty() {
        return Ok(json!({
            "status": "warning",
            "message": "PEXELS_API_KEY not set — cannot search candidates for relevance validation. Draft keywords are returned unchanged; set the key or run broll.fetch with a fallback_pool.",
            "validated": false,
            "segments": enriched_segments,
        }));
    }

    let mut client = PexelsClient::new(&api_key, &asset_dir);
    let mut validated: Vec<serde_json::Value> = Vec::new();
    let mut skipped: Vec<serde_json::Value> = Vec::new();
    let mut last_backend = String::new();
    let mut last_model = String::new();

    for (i, seg) in enriched_segments.iter().enumerate() {
        let id = seg
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or(&format!("seg_{}", i))
            .to_string();
        let caption = seg
            .get("caption")
            .or_else(|| seg.get("text"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let start_s = seg
            .get("start_s")
            .or_else(|| seg.get("start"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let end_s = seg
            .get("end_s")
            .or_else(|| seg.get("end"))
            .and_then(|v| v.as_f64())
            .unwrap_or(start_s + 3.0);
        let window_s = (end_s - start_s).max(1.0);
        let draft: Vec<String> = seg
            .get("keywords")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|k| k.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        // If a segment arrived without draft keywords, draft them agentically.
        let (draft, backend, model) = if draft.is_empty() {
            let (kws, b, m) = llm_draft_keywords(&caption, &[], &language).await;
            (kws, b, m)
        } else {
            (draft, String::new(), String::new())
        };
        if !backend.is_empty() {
            last_backend = backend;
        }
        if !model.is_empty() {
            last_model = model;
        }

        // Search Pexels. Multi-word phrase FIRST — Pexels matches joined
        // phrases far better than lone words (the A2V path joins its top
        // keywords into ONE query; lone-word searches returned generic clips).
        // Individual keywords are fallbacks only when the phrase under-delivers.
        let queries: Vec<String> = draft
            .iter()
            .filter(|k| k.len() >= 3)
            .take(max_keywords)
            .cloned()
            .collect();
        if queries.is_empty() {
            skipped.push(json!({"id": id, "reason": "no usable draft keywords"}));
            continue;
        }
        let mut candidates: Vec<openscript_assets::pexels::PexelsVideo> = Vec::new();
        let mut seen: std::collections::HashSet<i64> = std::collections::HashSet::new();
        let joined = if queries.len() >= 2 {
            Some(queries[..2].join(" "))
        } else {
            None
        };
        if let Some(j) = &joined {
            match client.search(j, &orientation, &quality).await {
                Ok(vids) => {
                    for v in vids {
                        if v.duration > 0
                            && seen.insert(v.id)
                            && candidates.len() < max_candidates
                        {
                            candidates.push(v);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("[broll.validate_keywords] search '{}' failed: {}", j, e)
                }
            }
        }
        if candidates.len() < max_candidates {
            for q in &queries {
                if candidates.len() >= max_candidates {
                    break;
                }
                match client.search(q, &orientation, &quality).await {
                    Ok(vids) => {
                        for v in vids {
                            if v.duration > 0
                                && seen.insert(v.id)
                                && candidates.len() < max_candidates
                            {
                                candidates.push(v);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            "[broll.validate_keywords] search '{}' failed: {}",
                            q,
                            e
                        )
                    }
                }
            }
        }
        // Non-looping gate: only candidates that cover the segment window qualify.
        // When NO candidate covers the window, the pool still shows the results
        // but each is tagged `covers_window: false` so the consumer knows a
        // download of it would flag BROLL_GAP (and trigger broll.repair) — the
        // agent must not treat it as a safe pick.
        let covering: Vec<openscript_assets::pexels::PexelsVideo> = candidates_covering_window(
            &candidates,
            window_s,
            0.5,
        )
        .into_iter()
        .cloned()
        .collect();
        let covers_ids: std::collections::HashSet<i64> =
            covering.iter().map(|v| v.id).collect();
        let pool: Vec<openscript_assets::pexels::PexelsVideo> = if covering.is_empty() {
            candidates
        } else {
            covering
        };
        if pool.is_empty() {
            skipped.push(json!({
                "id": id,
                "reason": format!(
                    "no Pexels results for draft keywords [{}]",
                    queries.join(", ")
                )
            }));
            continue;
        }

        // Agent validates the real candidates against the spoken caption.
        let avoid = std::collections::HashSet::new();
        let (best_id, final_kws, relevance, reason, backend, model) =
            llm_validate_candidates(&caption, &draft, &pool, window_s, &avoid).await;
        if !backend.is_empty() {
            last_backend = backend;
        }
        if !model.is_empty() {
            last_model = model;
        }

        let best_video = best_id.and_then(|bid| {
            pool.iter()
                .find(|v| v.id == bid)
                .map(|v| {
                    json!({
                        "id": v.id,
                        "name": pexels_url_slug(&v.url),
                        "duration_s": v.duration,
                        "url": v.url,
                        "covers_window": covers_ids.contains(&v.id),
                    })
                })
        });
        let candidates_json: Vec<serde_json::Value> = pool
            .iter()
            .map(|v| {
                json!({
                    "id": v.id,
                    "name": pexels_url_slug(&v.url),
                    "duration_s": v.duration,
                    "size": format!("{}x{}", v.width, v.height),
                    "url": v.url,
                    "covers_window": covers_ids.contains(&v.id),
                })
            })
            .collect();
        validated.push(json!({
            "id": id,
            "start_s": start_s,
            "end_s": end_s,
            "duration_s": end_s - start_s,
            "caption": caption,
            "draft_keywords": draft,
            "final_keywords": final_kws,
            "best_video": best_video,
            "relevance": relevance,
            "reason": reason,
            "candidates": candidates_json,
        }));
    }

    Ok(json!({
        "status": "validated",
        "backend": last_backend,
        "model": last_model,
        "validated_count": validated.len(),
        "skipped_count": skipped.len(),
        "skipped": skipped,
        "segments": validated,
    }))
}

pub(crate) async fn handle_broll_repair(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_assets::pexels::PexelsClient;

    let timeline_path = extract_str(&args, "timeline_path")?;
    let max_segments = args
        .get("max_segments")
        .and_then(|v| v.as_u64())
        .unwrap_or(10)
        .max(1) as usize;
    let orientation = default_str(&args, "orientation", "9:16");
    let quality = default_str(&args, "quality", "sd");
    let asset_dir = default_opt_str(&args, "asset_dir")
        .unwrap_or_else(|| "mcp/assets/broll_cache".to_string());
    let language = default_str(&args, "language", "hinglish");

    let mut tl = Timeline::load(&timeline_path)
        .map_err(|e| ToolError::Io(std::io::Error::other(format!("Failed to load timeline: {}", e))))?;
    let gaps = probe_broll_gaps(&tl).await;
    if gaps.is_empty() {
        return Ok(json!({
            "status": "ok",
            "message": "No b-roll coverage gaps — every clip covers its segment window.",
            "repaired": 0,
            "remaining_gaps": [],
            "timeline_path": timeline_path,
        }));
    }

    let api_key = pexels_key();
    if api_key.is_empty() {
        return Ok(json!({
            "status": "warning",
            "message": format!(
                "{} b-roll gap(s) exist but PEXELS_API_KEY is not set — cannot repair.",
                gaps.len()
            ),
            "repaired": 0,
            "gaps": gaps,
            "remaining_gaps": gaps,
        }));
    }

    let mut client = PexelsClient::new(&api_key, &asset_dir);
    let used_ids = used_broll_video_ids(&tl);
    let context_text = render_timeline_context_text(&tl);

    let mut decisions: Vec<serde_json::Value> = Vec::new();
    let mut repaired = 0usize;
    let mut last_backend = String::new();
    let mut last_model = String::new();

    for gap in gaps.iter().take(max_segments) {
        let window_s = gap.required_s.max(1.0);
        let caption = find_segment_for_window(&tl, &gap.segment_id)
            .map(|s| s.caption.clone())
            .unwrap_or_default();
        // No matching segment caption? Seed the draft from the existing concept
        // tag instead of burning an LLM call on an empty string.
        if caption.trim().is_empty() && gap.concept.trim().is_empty() {
            decisions.push(json!({
                "segment_id": gap.segment_id,
                "window_s": window_s,
                "status": "unrepairable_this_pass",
                "reason": "no caption or concept available to search for this segment",
            }));
            continue;
        }

        // Existing concepts across the timeline (non-redundant drafts).
        let avoid_concepts: Vec<String> = tl
            .tracks
            .get(&TrackType::Broll)
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|e| match &e.kind {
                openscript_core::timeline::EventKind::Broll { concept, .. } => {
                    Some(concept.clone())
                }
                _ => None,
            })
            .collect();

        // Stage 1: agent drafts fresh keywords from the spoken caption,
        // avoiding concepts already covered elsewhere in the timeline.
        let (draft, backend, model) = if caption.trim().is_empty() {
            // Seed from the existing concept tag (no caption to translate).
            (vec![gap.concept.clone()], "seed".into(), "existing-concept".into())
        } else {
            llm_draft_keywords(&caption, &avoid_concepts, &language).await
        };
        if !backend.is_empty() {
            last_backend = backend;
        }
        if !model.is_empty() {
            last_model = model;
        }

        // Search Pexels — joined phrase first (matches the A2V query style),
        // individual keywords as fallbacks when the phrase under-delivers.
        let queries: Vec<String> = draft
            .iter()
            .filter(|k| k.len() >= 3)
            .take(2)
            .cloned()
            .collect();
        let mut candidates: Vec<openscript_assets::pexels::PexelsVideo> = Vec::new();
        let mut seen: std::collections::HashSet<i64> = std::collections::HashSet::new();
        let joined = if queries.len() >= 2 {
            Some(queries[..2].join(" "))
        } else {
            None
        };
        if let Some(j) = &joined {
            if let Ok(vids) = client.search(j, &orientation, &quality).await {
                for v in vids {
                    if v.duration > 0 && seen.insert(v.id) {
                        candidates.push(v);
                    }
                }
            }
        }
        if candidates.is_empty() {
            for q in &queries {
                match client.search(q, &orientation, &quality).await {
                    Ok(vids) => {
                        for v in vids {
                            if v.duration > 0 && seen.insert(v.id) {
                                candidates.push(v);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("[broll.repair] search '{}' failed: {}", q, e)
                    }
                }
            }
        }

        // Non-looping gate: the window MUST be covered (+0.5s trim slack).
        let covering: Vec<openscript_assets::pexels::PexelsVideo> = candidates_covering_window(
            &candidates,
            window_s,
            0.5,
        )
        .into_iter()
        .cloned()
        .collect();
        if covering.is_empty() {
            decisions.push(json!({
                "segment_id": gap.segment_id,
                "caption": caption,
                "window_s": window_s,
                "draft_keywords": queries,
                "status": "unrepairable_this_pass",
                "reason": format!(
                    "no Pexels candidate covers the {:.1}s window (non-looping gate) — widen keywords or accept the held-frame",
                    window_s
                ),
            }));
            continue;
        }

        // Stage 2: agent validates the covering candidates against the speech.
        let (best_id, final_kws, relevance, reason, backend, model) =
            llm_validate_candidates(&caption, &queries, &covering, window_s, &used_ids).await;
        if !backend.is_empty() {
            last_backend = backend;
        }
        if !model.is_empty() {
            last_model = model;
        }

        // The LLM never sees the used-id blocklist, so its pick must be
        // cross-checked: an already-used clip (or a hallucinated id) falls
        // back to the longest covering UNUSED clip — the non-redundancy rule.
        let chosen = best_id
            .and_then(|bid| covering.iter().find(|v| v.id == bid && !used_ids.contains(&v.id)))
            .or_else(|| {
                covering
                    .iter()
                    .filter(|v| !used_ids.contains(&v.id))
                    .max_by_key(|v| v.duration)
            })
            .unwrap_or_else(|| covering.iter().max_by_key(|v| v.duration).unwrap());
        let concept = final_kws.first().cloned().unwrap_or_else(|| queries.first().cloned().unwrap_or("b-roll".into()));
        match client.download_best(chosen, &concept).await {
            Ok(path) => {
                // Replace the event's asset + the asset record — or CREATE the
                // missing event. Segments whose keyword draft produced nothing
                // have NO b-roll event at all; mutating-only left them
                // permanently uncovered.
                let new_asset_id = format!("broll_gap_{}", gap.segment_id);
                let old_asset_id = tl
                    .tracks
                    .get(&TrackType::Broll)
                    .and_then(|evts| evts.iter().find(|e| e.id == gap.segment_id))
                    .map(|e| e.asset_id.clone());
                // Missing-event windows are sized from the timeline segment
                // (seconds → ms); existing events keep their own timing.
                let seg_window: Option<(i64, i64)> = {
                    let has_evt = tl
                        .tracks
                        .get(&TrackType::Broll)
                        .map(|evts| evts.iter().any(|e| e.id == gap.segment_id))
                        .unwrap_or(false);
                    if has_evt {
                        None
                    } else {
                        find_segment_for_window(&tl, &gap.segment_id).map(|s| {
                            ((s.start * 1000.0) as i64, (s.end * 1000.0) as i64)
                        })
                    }
                };
                let evts = tl.tracks.entry(TrackType::Broll).or_default();
                match evts.iter_mut().find(|e| e.id == gap.segment_id) {
                    Some(evt) => {
                        evt.asset_id = new_asset_id.clone();
                        evt.tags = vec![concept.clone()];
                        if let openscript_core::timeline::EventKind::Broll {
                            concept: c,
                            source_provider: sp,
                            ..
                        } = &mut evt.kind
                        {
                            *c = concept.clone();
                            *sp = "pexels".to_string();
                        }
                        if let Some(prov) = &mut evt.provenance {
                            prov.concept = Some(concept.clone());
                            prov.tool = "broll.repair".to_string();
                        }
                    }
                    None => {
                        if let Some((start_ms, end_ms)) = seg_window {
                            evts.push(openscript_core::timeline::TimelineEvent {
                                id: gap.segment_id.clone(),
                                asset_id: new_asset_id.clone(),
                                start_ms,
                                end_ms,
                                offset_ms: 0,
                                gain_db: 0.0,
                                fade_in_ms: 0,
                                fade_out_ms: 0,
                                tags: vec![concept.clone()],
                                provenance: Some(openscript_core::timeline::Provenance {
                                    tool: "broll.repair".to_string(),
                                    editorial_role: None,
                                    concept: Some(concept.clone()),
                                }),
                                kind: openscript_core::timeline::EventKind::Broll {
                                    concept: concept.clone(),
                                    source_provider: "pexels".to_string(),
                                    transition_style: "cut".to_string(),
                                    crop_mode: "center".to_string(),
                                    orientation: orientation.clone(),
                                    motion_intensity: "medium".to_string(),
                                },
                            });
                        }
                    }
                }
                tl.assets.broll.insert(
                    new_asset_id.clone(),
                    serde_json::json!({
                        "path": path,
                        "concept": concept,
                        "source_duration_s": chosen.duration,
                    }),
                );
                // Drop the stale asset record for the swapped-out clip (the
                // cached file stays on disk — only the registry entry is removed).
                if let Some(old) = old_asset_id {
                    if old != new_asset_id {
                        tl.assets.broll.remove(&old);
                    }
                }
                decisions.push(json!({
                    "segment_id": gap.segment_id,
                    "caption": caption,
                    "window_s": window_s,
                    "draft_keywords": queries,
                    "final_keywords": final_kws,
                    "chosen_video": {
                        "id": chosen.id,
                        "name": pexels_url_slug(&chosen.url),
                        "duration_s": chosen.duration,
                    },
                    "relevance": relevance,
                    "reason": reason,
                    "asset_id": new_asset_id,
                    "path": path,
                    "status": "repaired",
                }));
                repaired += 1;
            }
            Err(e) => {
                decisions.push(json!({
                    "segment_id": gap.segment_id,
                    "caption": caption,
                    "window_s": window_s,
                    "draft_keywords": queries,
                    "status": "download_failed",
                    "reason": e.to_string(),
                }));
            }
        }
    }

    tl.updated_at = chrono::Utc::now();
    tl.save(&timeline_path).map_err(|e| {
        ToolError::Io(std::io::Error::other(format!("Failed to save timeline: {}", e)))
    })?;

    let remaining = probe_broll_gaps(&tl).await;
    let ok = remaining.is_empty();
    Ok(json!({
        "status": if ok { "healed" } else { "partial" },
        "message": if ok {
            "All flagged b-roll gaps repaired — timeline is fully covered.".to_string()
        } else {
            format!(
                "{} gap(s) repaired; {} gap(s) remain (run broll.repair again or widen keywords).",
                repaired,
                remaining.len()
            )
        },
        "backend": last_backend,
        "model": last_model,
        "repaired": repaired,
        "context_used": context_text.lines().count(),
        "decisions": decisions,
        "remaining_gaps": remaining,
        "timeline_path": timeline_path,
    }))
}

pub(crate) async fn handle_broll_auto(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let srt_path = args.get("srt_path").and_then(|v| v.as_str()).map(String::from);
    let audio_path = args.get("audio_path").and_then(|v| v.as_str()).map(String::from);
    let timeline_path_arg = args.get("timeline_path").and_then(|v| v.as_str()).map(String::from);
    // Word-level SRT from transcribe — real per-word alignments so the
    // word-highlight captions stay synced with the voice (caption-sync fix).
    let word_srt_path = default_opt_str(&args, "word_srt_path");

    let min_duration_s = default_f64(&args, "min_duration_s", 2.0);
    let max_duration_s = default_f64(&args, "max_duration_s", 6.0);
    let language = default_str(&args, "language", "hinglish");
    let quality = default_str(&args, "quality", "sd");
    let orientation = default_str(&args, "orientation", "9:16");
    let max_batch_size = default_u32(&args, "max_batch_size", 15);
    let max_candidates = default_u32(&args, "max_candidates", 6);
    let max_keywords_per_search = default_u32(&args, "max_keywords_per_search", 2);
    let max_repair_iterations = default_u32(&args, "max_repair_iterations", 3);
    let run_stickers = default_bool(&args, "stickers", true);
    let run_captions = default_bool(&args, "captions", true);

    // ---- Stage A: resolve timeline + segments ----
    let (timeline_path, segments) = if let Some(tl) = &timeline_path_arg {
        let timeline = Timeline::load(tl)?;
        let segs: Vec<serde_json::Value> = timeline
            .segments
            .iter()
            .map(|s| {
                json!({
                    "id": s.id.clone(),
                    "start_s": s.start,
                    "end_s": s.end,
                    "duration_s": s.end - s.start,
                    "caption": s.caption.clone(),
                })
            })
            .collect();
        (tl.clone(), segs)
    } else {
        let srt = srt_path.clone().ok_or_else(|| {
            ToolError::MissingArg(
                "broll.auto requires srt_path + audio_path (or timeline_path)".into(),
            )
        })?;
        let audio = audio_path.clone().ok_or_else(|| {
            ToolError::MissingArg("broll.auto requires audio_path (or timeline_path)".into())
        })?;

        // 1. segment.analyze — sentence-aware 2-6s segmentation
        report_progress(5.0, 100.0, "1/6 segment.analyze").await.ok();
        let analyzed = handle_segment_analyze(json!({
            "audio_path": audio,
            "srt_path": srt,
            "min_duration_s": min_duration_s,
            "max_duration_s": max_duration_s,
        }))
        .await?;
        let segments: Vec<serde_json::Value> = analyzed
            .get("segments")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        // 2. srt.to_timeline — build the timeline with identical segmentation
        let out_path = args
            .get("output_path")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| {
                let stem = Path::new(&srt)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "broll_auto".to_string());
                format!("artifacts/{}.timeline.json", stem)
            });
        report_progress(20.0, 100.0, "2/6 srt.to_timeline").await.ok();
        let built = handle_srt_to_timeline(json!({
            "srt_path": srt,
            "source_video": audio,
            "output_path": out_path,
            "aspect": orientation,
            "fps": 30,
            "min_duration_s": min_duration_s,
            "max_duration_s": max_duration_s,
        }))
        .await?;
        let tl = built
            .get("timeline_path")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or(out_path);
        (tl, segments)
    };

    let segments_arr = segments.clone();
    if segments_arr.is_empty() {
        return Err(ToolError::InvalidArg(
            "broll.auto: no segments found — check SRT/timeline".into(),
        ));
    }

    // ---- Alternation mode (V2V): plan visual roles + filter to broll ----
    // When alternation.enabled, the visual layer alternates stock b-roll ↔ the
    // ORIGINAL source video per transcript segment (docs/V2V_ALTERNATION_
    // ARCHITECTURE.md). Roles are planned by presentation::plan_alternation and
    // persisted to directives.presentation.visual_roles; ONLY "broll"-role
    // segments enter the keyword → validate → fetch pipeline. "source"-role
    // segments get NO b-roll event, so the renderer shows the original video
    // there. Stickers + captions still run across ALL segments (the A2V
    // pipeline remains intact).
    // ---- Video-level context (V2V equivalent of a script's title +
    // video_keywords): derive topical keywords from the WHOLE transcript so
    // the per-segment draft is anchored to the video's subject instead of
    // hallucinating from noisy ASR fragments (the "cooking turkey oven" for an
    // India-politics video bug). Mirrors script.to_video's effective_video_
    // keywords path. ----
    let video_title_arg = default_opt_str(&args, "video_title");
    let all_captions: Vec<String> = segments
        .iter()
        .filter_map(|s| {
            s.get("caption")
                .or_else(|| s.get("text"))
                .and_then(|v| v.as_str())
                .map(|c| c.to_string())
        })
        .filter(|c| !c.trim().is_empty())
        .collect();
    let title_hint = video_title_arg.clone().or_else(|| {
        Path::new(&timeline_path)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
    });
    let (video_title, video_keywords) = crate::keywords::derive_video_context(
        &all_captions,
        title_hint.as_deref().unwrap_or(""),
        &language,
    )
    .await;
    tracing::info!(
        "[broll.auto] derived video context: title={:?} topic={:?} ({} segment captions)",
        video_title,
        video_keywords.iter().take(6).collect::<Vec<_>>(),
        all_captions.len()
    );

    // Concepts already placed on this timeline — the drafter must AVOID
    // re-suggesting them (non-redundant drafts across re-runs).
    let existing_broll_concepts: Vec<String> = Timeline::load(&timeline_path)
        .ok()
        .map(|tl| {
            tl.tracks
                .get(&TrackType::Broll)
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|e| match &e.kind {
                    openscript_core::timeline::EventKind::Broll { concept, .. } => {
                        Some(concept.clone())
                    }
                    _ => None,
                })
                .filter(|c| !c.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let alternation = args.get("alternation").cloned().unwrap_or_else(|| json!({}));
    let alternation_enabled = alternation
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let alt_pattern = alternation
        .get("pattern")
        .and_then(|v| v.as_str())
        .unwrap_or(openscript_core::presentation::PATTERN_EVERY_OTHER)
        .to_string();
    let alt_every_n = alternation.get("every_n").and_then(|v| v.as_u64()).unwrap_or(2) as u32;
    let alt_ratio = alternation.get("broll_ratio").and_then(|v| v.as_f64());
    // NOTE: re-voice (source_audio 'duck') is EXCLUDED from V2V by decision —
    // the original video's audio is always preserved as-is. See
    // docs/V2V_ALTERNATION_ARCHITECTURE.md §3.6.

    let mut broll_segments = segments.clone();
    let mut alternation_summary: Option<serde_json::Value> = None;
    if alternation_enabled {
        let mut tl = Timeline::load(&timeline_path)?;
        let roles = openscript_core::presentation::plan_alternation(
            &tl.segments,
            &alt_pattern,
            alt_every_n,
            alt_ratio,
        );
        tl.directives.presentation.mode = "alternate".into();
        tl.directives.presentation.visual_roles = roles.clone();
        tl.directives.presentation.pattern = alt_pattern.clone();
        tl.directives.presentation.every_n = alt_every_n;
        // source_audio stays "keep" (re-voice excluded — schema field retained
        // for backward compatibility only).
        tl.updated_at = chrono::Utc::now();
        tl.save(&timeline_path)?;
        let roles_owned = roles;
        broll_segments.retain(|s| {
            let id = s
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            roles_owned.get(&id).map(|r| r == "broll").unwrap_or(true)
        });
        let source_count = segments_arr.len().saturating_sub(broll_segments.len());
        alternation_summary = Some(json!({
            "mode": "alternate",
            "pattern": alt_pattern,
            "every_n": alt_every_n,
            "broll_ratio": alt_ratio,
            "revoice": "excluded",
            "broll_segments": broll_segments.len(),
            "source_segments": source_count,
        }));
        tracing::info!(
            "[broll.auto] V2V alternation: pattern={} every_n={} ratio={:?} → {} b-roll / {} source segment(s)",
            alt_pattern, alt_every_n, alt_ratio, broll_segments.len(), source_count
        );
    }

    // ---- Stage B: draft keywords (agentic, unified) ----
    // ONE draft call emits BOTH visual (stock) and reactions (GIPHY) keywords
    // per segment via the shared keywords module. B-roll consumes `visual`;
    // stickers consume `reactions` (Stage F) — the old path fed validated
    // visual b-roll nouns into the GIPHY sticker search (the sticker-
    // relevance bug). In alternation mode only broll-role segments are drafted
    // (source segments need no stock footage).
    let _ = max_batch_size; // draft batching is owned by keywords (MAX_DRAFT_BATCH)
    let mut drafted: Vec<crate::keywords::SceneKeywords> = Vec::new();
    let mut validated_segments = json!([]);
    let mut auto_assigned = 0u64;
    if !broll_segments.is_empty() {
        report_progress(35.0, 100.0, "3/6 keywords.draft (visual + reactions)").await.ok();
        // Draft for ALL segments (not just broll-role ones) — stickers consume
        // the SAME unified draft across every segment; only the fetch below is
        // filtered to broll-role segments. Video context anchors the draft.
        let draft_inputs: Vec<crate::keywords::SegmentInput> = segments
            .iter()
            .enumerate()
            .map(|(i, s)| crate::keywords::SegmentInput {
                segment_id: s
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&format!("seg_{}", i))
                    .to_string(),
                caption: s
                    .get("caption")
                    .or_else(|| s.get("text"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                language_hint: Some(language.clone()),
                duration_s: (s
                    .get("end_s")
                    .or_else(|| s.get("end"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0)
                    - s.get("start_s")
                        .or_else(|| s.get("start"))
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0))
                .max(0.0),
                scene_idx: i,
                total_scenes: segments.len(),
                video_title: video_title.clone(),
                video_keywords: video_keywords.clone(),
                covered_concepts: existing_broll_concepts.clone(),
            })
            .collect();
        drafted = crate::keywords::draft_scene_keywords(&draft_inputs).await;
        // Index alignment: drafted is ALL segments, broll_segments is the
        // broll-role subset — look keywords up by segment id, never position.
        let drafted_by_id: std::collections::HashMap<String, crate::keywords::SceneKeywords> =
            drafted
                .iter()
                .map(|d| (d.segment_id.clone(), d.clone()))
                .collect();
        let draft_segments: serde_json::Value = json!(
            broll_segments
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let mut seg = s.clone();
                    let id = seg
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if let Some(obj) = seg.as_object_mut() {
                        let visual = drafted_by_id
                            .get(&id)
                            .map(|d| d.visual.clone())
                            .or_else(|| drafted.get(i).map(|d| d.visual.clone()))
                            .unwrap_or_default();
                        obj.insert("keywords".into(), json!(visual));
                    }
                    seg
                })
                .collect::<Vec<serde_json::Value>>()
        );

        // ---- Stage C: relevance validation (agent picks best real video) ----
        report_progress(50.0, 100.0, "4/6 broll.validate_keywords (relevance)").await.ok();
        let validated = handle_broll_validate_keywords(json!({
            "enriched_segments": draft_segments,
            "max_candidates": max_candidates,
            "max_keywords_per_search": max_keywords_per_search,
            "orientation": orientation,
            "quality": quality,
            "language": language,
        }))
        .await?;
        validated_segments = validated.get("segments").cloned().unwrap_or_else(|| json!([]));

        // ---- Stage D: fetch + auto-place ----
        report_progress(65.0, 100.0, "5/6 broll.fetch (download + place)").await.ok();
        let fetch_segments: Vec<serde_json::Value> = validated_segments
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|v| {
                        let mut seg = json!({
                            "id": v.get("id").cloned().unwrap_or_else(|| json!("")),
                            "start_s": v.get("start_s").cloned().unwrap_or_else(|| json!(0)),
                            "end_s": v.get("end_s").cloned().unwrap_or_else(|| json!(0)),
                            "caption": v.get("caption").cloned().unwrap_or_else(|| json!("")),
                        });
                        let kw = v
                            .get("final_keywords")
                            .cloned()
                            .or_else(|| v.get("draft_keywords").cloned())
                            .unwrap_or_else(|| json!([]));
                        seg["keywords"] = kw;
                        seg
                    })
                    .collect()
            })
            .unwrap_or_default();

        let fetched = handle_broll_fetch(json!({
            "enriched_segments": fetch_segments,
            "timeline_path": timeline_path,
            "download_n": 1,
            "quality": quality,
            "orientation": orientation,
        }))
        .await?;
        auto_assigned = fetched.get("auto_assigned").and_then(|v| v.as_u64()).unwrap_or(0);
    }

    // ---- Stage E: validate + repair loop until zero gaps ----
    report_progress(80.0, 100.0, "6/6 timeline.validate + repair loop").await.ok();
    let mut repair_passes = 0u32;
    let mut repaired_total = 0u64;
    let mut remaining_gaps: Vec<serde_json::Value> = Vec::new();
    let mut initial_gaps = 0usize;
    let mut final_valid = false;

    for pass in 0..max_repair_iterations {
        let vres = handle_timeline_validate(json!({"timeline_path": timeline_path})).await?;
        let gaps = vres
            .get("broll_gaps")
            .and_then(|g| g.as_array())
            .cloned()
            .unwrap_or_default();
        if pass == 0 {
            initial_gaps = gaps.len();
        }
        if gaps.is_empty() {
            final_valid = vres.get("valid").and_then(|v| v.as_bool()).unwrap_or(false);
            remaining_gaps = vec![];
            break;
        }
        repair_passes = pass + 1;
        let repair = handle_broll_repair(json!({
            "timeline_path": timeline_path,
            "max_segments": gaps.len(),
            "language": language,
            "quality": quality,
            "orientation": orientation,
        }))
        .await?;
        let repaired_this = repair.get("repaired").and_then(|v| v.as_u64()).unwrap_or(0);
        repaired_total += repaired_this;
        remaining_gaps = repair
            .get("remaining_gaps")
            .and_then(|g| g.as_array())
            .cloned()
            .unwrap_or_default();
        if repaired_this == 0 {
            break; // no progress — avoid infinite loop
        }
    }
    if remaining_gaps.is_empty() {
        let vres = handle_timeline_validate(json!({"timeline_path": timeline_path})).await?;
        final_valid = vres.get("valid").and_then(|v| v.as_bool()).unwrap_or(false);
    }

    // ---- Stage F: optional sticker + caption stages (finalize A2V one-call) ----
    let mut stickers_placed = 0u64;
    let mut captions_ass_path: Option<String> = None;
    let mut sticker_warning: Option<String> = None;

    if run_stickers {
        report_progress(88.0, 100.0, "sticker.auto (agentic GIPHY stickers)").await.ok();
        // Unification (fixed): b-roll and stickers share ONE draft pass but
        // consume DIFFERENT outputs — b-roll uses `visual`, stickers use
        // `reactions` (reaction/meme keywords that GIPHY actually indexes).
        // The old path fed validated visual b-roll nouns into the GIPHY
        // sticker search, which produced irrelevant noun/crowd GIFs.
        // Unification (requested): b-roll AND stickers share ONE keyword source
        // — the same per-segment draft, blended (reactions first for GIPHY +
        // visual subject nouns for context), across ALL segments. The sticker
        // relevance gate (sticker.validate_keywords) still approves/rejects each
        // GIF, so sharing never means flooding.
        let sticker_shared_keywords: Vec<serde_json::Value> = drafted
            .iter()
            .map(|d| {
                let caption = segments
                    .iter()
                    .find(|s| s.get("id").and_then(|v| v.as_str()) == Some(d.segment_id.as_str()))
                    .and_then(|s| s.get("caption").and_then(|v| v.as_str()))
                    .unwrap_or("")
                    .to_string();
                json!({
                    "id": d.segment_id.clone(),
                    "caption": caption,
                    // Emphatic/intent ride along so sticker.validate_keywords
                    // can apply the emotional intent when judging GIFs.
                    "emphatic": d.emphatic,
                    "intent": d.intent.clone().unwrap_or_else(|| "emphasis".to_string()),
                    "sticker_keywords": crate::keywords::blend_sticker_keywords(
                        &d.visual,
                        &d.reactions
                    ),
                })
            })
            .collect();
        // sticker.auto loads the timeline's segments directly (timeline_path
        // branch) and runs shared keywords → GIPHY relevance gate → Stickers track.
        let sticker_res = handle_sticker_auto(json!({
            "timeline_path": timeline_path,
            "language": language,
            "shared_keywords": sticker_shared_keywords,
            // "auto": position cycling + spacing gates (sticker relevance fix)
            "position": "auto",
            "min_gap_s": 2.0,
            "scale": 0.25,
            // Cap sticker volume in the one-call (GIPHY rate limits + render time).
            "max_stickers": segments_arr.len().min(12),
        }))
        .await?;
        stickers_placed = sticker_res.get("stickers_placed").and_then(|v| v.as_u64()).unwrap_or(0);
        if let Some(msg) = sticker_res.get("message").and_then(|v| v.as_str()) {
            if stickers_placed == 0 {
                sticker_warning = Some(msg.to_string());
            }
        }
    }

    if run_captions {
        report_progress(94.0, 100.0, "captions.generate_ass (styled ASS)").await.ok();
        // Pass the explicit SRT when we have one (the timeline's `source` is
        // the audio file, so deriving `audio.srt` from it can miss the
        // transcript). captions.generate_ass falls back to timeline-derived
        // SRT when srt_path is absent.
        let mut cap_args = json!({
            "timeline_path": timeline_path,
            "style": "word_highlight",
            "position": "center",
        });
        if let Some(ref sp) = srt_path {
            if let Some(obj) = cap_args.as_object_mut() {
                obj.insert("srt_path".into(), json!(sp));
            }
        }
        if let Some(ref wsp) = word_srt_path {
            if let Some(obj) = cap_args.as_object_mut() {
                obj.insert("word_srt_path".into(), json!(wsp));
            }
        }
        let cap_res = handle_captions_generate_ass(cap_args).await;
        match cap_res {
            Ok(r) => {
                captions_ass_path = r.get("ass_path").and_then(|v| v.as_str()).map(String::from);
            }
            Err(e) => {
                tracing::warn!("[broll.auto] caption generation failed (non-fatal): {}", e);
            }
        }
    }

    report_progress(100.0, 100.0, "broll.auto complete").await.ok();

    Ok(json!({
        "status": if final_valid { "success" } else { "partial" },
        "message": if final_valid {
            let coverage_note = if alternation_enabled {
                format!(
                    " V2V alternation active: {} b-roll / {} source segment(s) — original video shows on source segments.",
                    broll_segments.len(),
                    segments_arr.len().saturating_sub(broll_segments.len())
                )
            } else {
                String::new()
            };
            format!(
                "A2V b-roll complete: {} segments {} with validated, non-looping clips ({} placed, {} repair pass(es)).{} {}{}",
                segments_arr.len(),
                if alternation_enabled { "covered in alternation" } else { "fully covered" },
                auto_assigned,
                repair_passes,
                if stickers_placed > 0 { format!(" {} sticker(s) placed.", stickers_placed) } else { String::new() },
                if let Some(ref w) = sticker_warning { format!(" Stickers skipped: {}", w) } else { String::new() },
                coverage_note
            )
        } else {
            format!(
                "{} gap(s) remain after {} repair pass(es) — rerun broll.repair with wider keywords.",
                remaining_gaps.len(),
                repair_passes
            )
        },
        "timeline_path": timeline_path,
        "segments_count": segments_arr.len(),
        "auto_assigned": auto_assigned,
        "initial_gaps": initial_gaps,
        "repair_passes": repair_passes,
        "repaired_total": repaired_total,
        "remaining_gaps": remaining_gaps,
        "valid": final_valid,
        "alternation": alternation_summary,
        "stickers_placed": stickers_placed,
        "sticker_warning": sticker_warning,
        "captions_ass_path": captions_ass_path,
        "pipeline": json!({
            "analyze": "segment.analyze",
            "draft": "broll.keywords",
            "validate": "broll.validate_keywords",
            "fetch": "broll.fetch",
            "repair": "broll.repair",
            "stickers": "sticker.auto",
            "captions": "captions.generate_ass",
        }),
    }))
}

/// ONE-CALL V2V orchestrator: turn an EXISTING video into a captioned,
/// music-backed short whose visual layer ALTERNATES stock b-roll ↔ the
/// original footage per transcript segment — [broll → video → broll].
///
/// Pipeline (everything from the A2V pipeline remains, only the visual layer
/// alternates):
///   transcribe (or reuse srt_path) → segment.analyze → srt.to_timeline with
///   source_video = the ORIGINAL video (the renderer's base layer) →
///   broll.auto with alternation.enabled (plans roles, fetches stock ONLY for
///   broll-role segments, stickers + captions across ALL segments) →
///   timeline.render (base = original video).
///
/// The original video's audio is the master clock; voiceover/music/SFX mix
/// above it exactly as in A2V. Returns: timeline_path, output_path, alternation
/// summary, segment/asset counts.
pub(crate) async fn handle_video_to_video(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let video_path = extract_str(&args, "video_path")?;
    if !Path::new(&video_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Source video not found: {}",
            video_path
        )));
    }
    let srt_path = default_opt_str(&args, "srt_path");
    let output_path = default_opt_str(&args, "output_path");
    let crf = default_opt_u32(&args, "crf");
    let aspect = default_str(&args, "aspect", "9:16");
    let fps = default_u32(&args, "fps", 30);
    let min_duration_s = default_f64(&args, "min_duration_s", 2.0);
    let max_duration_s = default_f64(&args, "max_duration_s", 6.0);
    let language = default_str(&args, "language", "hinglish");
    let quality = default_str(&args, "quality", "sd");
    let orientation = default_str(&args, "orientation", "9:16");
    let max_candidates = default_u32(&args, "max_candidates", 6);
    let max_keywords_per_search = default_u32(&args, "max_keywords_per_search", 2);
    let max_repair_iterations = default_u32(&args, "max_repair_iterations", 3);
    let run_stickers = default_bool(&args, "stickers", true);
    let run_captions = default_bool(&args, "captions", true);
    // Alternation defaults to ENABLED for video.to_video — that is the point of
    // this tool. Disable with alternation.enabled=false for full-coverage.
    let mut alternation = args
        .get("alternation")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if alternation.get("enabled").is_none() {
        if let Some(obj) = alternation.as_object_mut() {
            obj.insert("enabled".into(), json!(true));
        }
    }

    // ---- Stage 1: transcribe (or reuse provided SRT) ----
    report_progress(5.0, 100.0, "1/5 transcribe").await.ok();
    let (phrase_srt, word_srt) = if let Some(ref sp) = srt_path {
        (sp.clone(), None)
    } else {
        let out_srt = format!(
            "artifacts/{}.srt",
            Path::new(&video_path)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "v2v".to_string())
        );
        let tx = handle_transcribe(json!({
            "media_path": video_path,
            "output_srt_path": out_srt,
        }))
        .await?;
        (
            tx.get("phrase_srt_path")
                .or_else(|| tx.get("output_srt_path"))
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or(out_srt),
            tx.get("word_srt_path").and_then(|v| v.as_str()).map(String::from),
        )
    };
    if !Path::new(&phrase_srt).exists() {
        return Err(ToolError::NotFound(format!(
            "SRT not found: {}",
            phrase_srt
        )));
    }

    // ---- Stage 2: build the timeline with the ORIGINAL video as source ----
    report_progress(20.0, 100.0, "2/5 srt.to_timeline (source = original video)").await.ok();
    let tl_path = format!(
        "artifacts/{}.timeline.json",
        Path::new(&video_path)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "v2v".to_string())
    );
    let built = handle_srt_to_timeline(json!({
        "srt_path": phrase_srt,
        "source_video": video_path,
        "output_path": tl_path,
        "aspect": aspect,
        "fps": fps,
        "min_duration_s": min_duration_s,
        "max_duration_s": max_duration_s,
    }))
    .await?;
    let timeline_path = built
        .get("timeline_path")
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or(tl_path);

    // ---- Stage 3: alternation b-roll (stock on broll-role segments only) ----
    report_progress(45.0, 100.0, "3/5 broll.auto (V2V alternation)").await.ok();
    let src_title = Path::new(&video_path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "video".to_string());
    let mut auto_args = json!({
        "timeline_path": timeline_path,
        "video_title": src_title,
        "language": language,
        "quality": quality,
        "orientation": orientation,
        "max_candidates": max_candidates,
        "max_keywords_per_search": max_keywords_per_search,
        "max_repair_iterations": max_repair_iterations,
        "stickers": run_stickers,
        "captions": run_captions,
        "alternation": alternation,
    });
    if let Some(ref wsp) = word_srt {
        if let Some(obj) = auto_args.as_object_mut() {
            obj.insert("word_srt_path".into(), json!(wsp));
        }
    }
    let broll = handle_broll_auto(auto_args).await?;
    let alternation_summary = broll.get("alternation").cloned();

    // ---- Stage 4: render (base = original video → source segments show) ----
    report_progress(85.0, 100.0, "4/5 timeline.render").await.ok();
    let render_args = json!({
        "timeline_path": timeline_path,
        "source_video": video_path,
    });
    let mut render_args = render_args;
    if let Some(op) = output_path {
        if let Some(obj) = render_args.as_object_mut() {
            obj.insert("output_path".into(), json!(op));
        }
    }
    if let Some(c) = crf {
        if let Some(obj) = render_args.as_object_mut() {
            obj.insert("crf".into(), json!(c));
        }
    }
    let rendered = handle_timeline_render(render_args).await?;
    let output_path_out = rendered
        .get("output_path")
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_default();

    report_progress(100.0, 100.0, "video.to_video complete").await.ok();

    Ok(json!({
        "status": "rendered",
        "timeline_path": timeline_path,
        "output_path": output_path_out,
        "alternation": alternation_summary,
        "broll": json!({
            "valid": broll.get("valid").cloned().unwrap_or_else(|| json!(false)),
            "auto_assigned": broll.get("auto_assigned").cloned().unwrap_or_else(|| json!(0)),
            "stickers_placed": broll.get("stickers_placed").cloned().unwrap_or_else(|| json!(0)),
            "message": broll.get("message").cloned().unwrap_or_else(|| json!("")),
        }),
        "render": json!({
            "file_size_bytes": rendered.get("file_size_bytes").cloned().unwrap_or_else(|| json!(0)),
            "segments_count": rendered.get("segments_count").cloned().unwrap_or_else(|| json!(0)),
        }),
        "pipeline": json!([
            "transcribe",
            "srt.to_timeline",
            "broll.auto (alternation)",
            "sticker.auto",
            "captions.generate_ass",
            "timeline.render",
        ]),
    }))
}

pub(crate) async fn handle_broll_probe(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let query = extract_str(&args, "query")?;
    let aspect = default_str(&args, "aspect", "9:16");
    let min_duration_s = default_f64(&args, "min_duration_s", 0.0);
    let max_duration_s = default_f64(&args, "max_duration_s", 0.0);
    let per_provider = default_u32(&args, "per_provider", 8) as usize;
    let signal: Vec<String> = args
        .get("signal")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    report_progress(0.0, 100.0, &format!("Probing stock engines for '{}'...", query))
        .await
        .ok();

    let q = crate::stock_pool::StockPoolQuery {
        query: query.to_string(),
        aspect,
        min_duration_s,
        max_duration_s,
        per_provider,
        signal,
    };
    let outcome = crate::stock_pool::search_stock_pool(&q).await;

    let candidates: Vec<serde_json::Value> = outcome
        .candidates
        .iter()
        .map(|c| {
            json!({
                "provider": c.provider,
                "id": c.id,
                "title": c.title,
                "duration_s": c.duration_s,
                "width": c.width,
                "height": c.height,
                "thumbnail_url": c.thumbnail_url,
                "page_url": c.page_url,
                "direct_url": c.direct_url,
                "lexical": c.lexical,
            })
        })
        .collect();

    let per_provider: serde_json::Value = outcome
        .per_provider
        .iter()
        .map(|(p, n)| json!({ "provider": p, "count": n }))
        .collect();

    report_progress(100.0, 100.0, &format!("Found {} ranked candidates", candidates.len()))
        .await
        .ok();

    Ok(json!({
        "status": "searched",
        "query": query,
        "per_provider": per_provider,
        "count": candidates.len(),
        "candidates": candidates,
    }))
}

pub(crate) async fn handle_segment_analyze(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    // Accept both audio_path and video_path for backward compat (agents use video_path)
    let audio_path = extract_str(&args, "audio_path")
        .or_else(|_| extract_str(&args, "video_path"))?;
    let srt_path = args.get("srt_path").and_then(|v| v.as_str()).map(String::from);

    // Step 1: Transcribe or load SRT
    let word_srt_path = if let Some(ref path) = srt_path {
        path.clone()
    } else {
        // Transcribe the audio
        report_progress(0.0, 100.0, "Transcribing audio...").await.ok();
        let out_dir = std::env::temp_dir().join("segment_analyze");
        let _ = std::fs::create_dir_all(&out_dir);
        let out_srt = out_dir.join("transcript.srt").to_string_lossy().to_string();
        let result = transcribe_with_engine(
            audio_path,
            &out_srt,
            openscript_transcribe::transcriber::TranscriptionEngine::HinglishGgml,
            "auto",
            None,
        )
        .await
        .map_err(|e| ToolError::InvalidArg(format!("Transcription failed: {}", e)))?;
        result.word_srt_path
            .unwrap_or(result.phrase_srt_path.unwrap_or(result.output_path))
    };

    // Step 2: Parse SRT entries
    report_progress(30.0, 100.0, "Parsing SRT entries...").await.ok();
    let entries = parse_srt(&word_srt_path)?;

    if entries.is_empty() {
        return Ok(json!({
            "status": "warning",
            "message": "No SRT entries found",
            "segments": [],
        }));
    }

    // Step 3: Group into segments using sentence-aware segmentation with
    // min/max duration enforcement (docs/SEGMENTATION_ARCHITECTURE.md).
    // Replaces the old fixed `SCENE_SIZE=4` chunking, which produced
    // unbounded 10–27s segments and broke mid-sentence. Pause detection
    // (>300ms gaps) groups at sentence boundaries; enforce_segment_bounds
    // then merges segments < min (2.0s) and splits segments > max (6.0s)
    // at the longest internal pause — the short-form retention target.
    report_progress(50.0, 100.0, "Grouping into segments (sentence-aware)...").await.ok();
    let min_dur_s = args.get("min_duration_s").and_then(|v| v.as_f64()).unwrap_or(2.0);
    let max_dur_s = args.get("max_duration_s").and_then(|v| v.as_f64()).unwrap_or(6.0);
    let grouped = openscript_core::srt::group_entries_with_words_max_duration(
        &entries,
        15,   // ~4s at 2.5 words/s
        80,   // ~2 caption lines
        0.3,  // 300ms breath pause boundary
        max_dur_s,
    );
    let bounded = openscript_core::srt::enforce_segment_bounds(grouped, min_dur_s, max_dur_s);
    let mut scenes: Vec<(String, f64, f64)> = bounded
        .into_iter()
        .map(|p| (p.text, p.start, p.end))
        .collect();

    // Clamp scenes at the source media duration. SRT entries can overshoot the
    // audio end (whisper tail hallucination / trailing silence), producing
    // segments past the master clock — the "audio 2:15, video 2:41" black tail.
    // broll.fetch places clips against these scenes, so the clamp here keeps
    // every b-roll window inside the source audio.
    if let Some(src_dur) = probe_source_duration(std::path::Path::new(&audio_path)).await {
        scenes.retain(|(_, start, _)| *start < src_dur);
        for (_, _, end) in scenes.iter_mut() {
            if *end > src_dur + SOURCE_DUR_TOLERANCE_S {
                *end = src_dur;
            }
        }
    }

    // Step 4: For each segment, run stock_signal analysis
    report_progress(60.0, 100.0, "Analyzing segments for b-roll keywords...").await.ok();
    let mut result_segments = Vec::new();
    for (idx, (text, start_s, end_s)) in scenes.iter().enumerate() {
        let duration_s = end_s - start_s;
        // Agent generates English keywords from Hinglish content - no auto-extraction
        result_segments.push(json!({
            "id": format!("seg_{:03}", idx + 1),
            "start_s": start_s,
            "end_s": end_s,
            "duration_s": duration_s,
            "caption": text,
        }));
    }    report_progress(100.0, 100.0, "Analysis complete.").await.ok();

    // Build section_map: maps segment index to its role in the video structure
    // Sections: intro (first 15%), body (middle 70%), outro (last 15%)
    let total_segs = result_segments.len();
    let section_map: Vec<serde_json::Value> = result_segments.iter().enumerate().map(|(i, seg)| {
        let fraction = i as f64 / total_segs.max(1) as f64;
        let section = if fraction < 0.15 {
            "intro"
        } else if fraction > 0.85 {
            "outro"
        } else {
            "body"
        };
        json!({
            "segment_id": seg["id"].clone(),
            "section": section,
            "start_s": seg["start_s"].clone(),
            "end_s": seg["end_s"].clone(),
        })
    }).collect();

    Ok(json!({
        "status": "success",
        "segments_count": result_segments.len(),
        "segments": result_segments,
        "section_map": section_map,
    }))
}

pub(crate) async fn handle_background_fetch(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let query = extract_str(&args, "query")?;
    let duration_s = default_f64(&args, "duration_s", 30.0);
    // SEGMENTATION_ARCHITECTURE min/max clip duration: clips shorter than
    // `min_duration_s` are skipped (alternates are fetched instead of looping);
    // `max_duration_s` caps the upper bound. 0 = fall back to duration_s / no cap.
    let min_duration_s = default_f64(&args, "min_duration_s", 0.0);
    let max_duration_s = default_f64(&args, "max_duration_s", 0.0);
    let aspect = default_str(&args, "aspect", "9:16");
    let scene_text = default_str(&args, "scene_text", "");
    let cache_dir = default_str(&args, "cache_dir", "mcp/assets/background_cache");
    let enable_youtube = default_bool(&args, "enable_youtube", false);
    let fallback_pool: Vec<String> = args
        .get("fallback_pool")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    // Pexels ids already used elsewhere in this run (non-redundancy across calls).
    let used_pexels_ids: std::collections::HashSet<i64> = args
        .get("used_video_ids")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_i64()).collect())
        .unwrap_or_default();

    std::fs::create_dir_all(&cache_dir)?;

    let mut used_stock_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut used_stock_hashes: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut used_pexels: std::collections::HashSet<i64> = used_pexels_ids;

    // Phase 158: scene-text-aware signal — the old path derived the lexical
    // signal from the raw query alone with EMPTY video_keywords, so topic
    // detection collapsed to Lifestyle and ranking lost all scene context.
    // When the caller supplies scene_text, merge its salient keywords with the
    // query tokens (unicode-aware — works for any script system).
    let mut signal = crate::keywords::extract_salient_keywords(&scene_text, 6);
    for t in crate::keywords::extract_salient_keywords(&query, 4) {
        if !signal.contains(&t) {
            signal.push(t);
        }
    }
    if signal.is_empty() {
        signal = vec![query.to_string()];
    }
    let outcome = crate::scene_media::fetch_scene_background(
        crate::scene_media::SceneMediaRequest {
            query: query.to_string(),
            signal_tokens: signal,
            scene_text,
            duration_s,
            min_duration_s,
            max_duration_s,
            aspect: aspect.clone(),
            cache_dir: cache_dir.to_string(),
            // Per-call unique stem: the old fixed "clip" name meant every
            // background.fetch call (or concurrent calls) overwrote the same
            // {cache_dir}/clip.mp4, so timelines referenced whichever call
            // wrote last. Hash the query so each fetch owns its file.
            out_stem: format!("clip_{:x}", md5_hash(query.as_bytes())),
            scene_idx: 0,
            enable_youtube,
            fallback_pool,
            used_video_ids: &mut used_stock_ids,
            used_content_hashes: &mut used_stock_hashes,
            used_pexels_ids: &mut used_pexels,
        },
    )
    .await?;

    let source = outcome.source.clone();
    let provider_id = outcome.provider_id.clone();
    // Probe the produced clip so consumers (broll_gaps / broll.auto) receive the
    // ACTUAL duration and a truthful needs_looping flag — the old background.fetch
    // reported these accurately per provider (regression fix after unification).
    let actual_duration_s = match openscript_ffmpeg::probe::probe(&outcome.clip_path).await {
        Ok(m) if m.duration > 0.0 => m.duration,
        _ => duration_s,
    };
    let mut result = json!({
        "status": if outcome.fell_to_procedural { "warning" } else { "fetched" },
        "clip_path": outcome.clip_path,
        "source": source,
        "source_duration_s": actual_duration_s,
        "start_s": 0.0,
        "duration_s": duration_s,
        "needs_looping": actual_duration_s < duration_s,
        "cached": false,
        "exhausted": outcome.exhausted,
    });
    match source.as_str() {
        "pexels" => {
            if let Some(pid) = provider_id.as_deref() {
                // Preserve the numeric pexels_id contract (was i64 before the
                // scene_media unification).
                result["pexels_id"] = match pid.parse::<i64>() {
                    Ok(n) => json!(n),
                    Err(_) => json!(pid),
                };
            }
        }
        "pixabay" => result["pixabay_id"] = json!(provider_id),
        "youtube" => result["youtube_id"] = json!(provider_id),
        _ => {}
    }
    result["lexical_score"] = json!(outcome.lexical_score);
    result["source_title"] = json!(outcome.source_title);
    if let Some(v) = outcome.vision_score {
        result["vision_score"] = json!(v);
    }
    if let Some(r) = outcome.vision_reason {
        result["vision_reason"] = json!(r);
    }
    if outcome.fell_to_procedural {
        result["warning"] =
            json!("All stock tiers exhausted — procedural fallback used (see exhausted)");
    }
    Ok(result)
}

pub(crate) async fn handle_background_assign(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let script_input = extract_str(&args, "script")?;
    let manifest_path = extract_str(&args, "voiceover_manifest")?;
    let background_pool: Vec<String> = args
        .get("background_pool")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ToolError::MissingArg("background_pool is required".into()))?
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    let output_path = default_str(
        &args,
        "output_path",
        "artifacts/background_assignments.json",
    );

    // Parse script
    let json_str = read_script_input(script_input)?;
    let spec = parse_script(&json_str)
        .map_err(|e| ToolError::InvalidArg(format!("Script parse error: {}", e)))?;

    // Load voiceover manifest
    let manifest_str = std::fs::read_to_string(sanitize_input_path(manifest_path)?)?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest_str)?;

    // Extract scene IDs, speakers, and durations from manifest
    let mut scene_ids = Vec::new();
    let mut scene_speakers = Vec::new();
    let mut scene_durations = Vec::new();

    if let Some(segments) = manifest.get("segments").and_then(|v| v.as_array()) {
        for seg in segments {
            scene_ids.push(
                seg.get("scene_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            );
            scene_speakers.push(
                seg.get("speaker")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            );
            let dur_ms = seg
                .get("duration_ms")
                .and_then(|v| v.as_i64())
                .unwrap_or(3000);
            scene_durations.push(dur_ms as f64 / 1000.0);
        }
    }

    // Phase 5: Add pause_ms from SceneSpec to scene durations (breath beats).
    // This extends each scene's duration by the specified pause, creating
    // natural breathing gaps between scenes without affecting the TTS audio.
    for (i, dur) in scene_durations.iter_mut().enumerate() {
        if let Some(scene) = spec.scenes.get(i) {
            if let Some(pause) = scene.pause_ms {
                if pause > 0 {
                    *dur += pause as f64 / 1000.0;
                }
            }
        }
    }

    // Assign backgrounds
    let clips = assign_backgrounds(
        &scene_ids,
        &scene_speakers,
        &background_pool,
        &spec.background.change_cadence,
        &scene_durations,
    );

    // Write assignments
    if let Some(parent) = Path::new(&output_path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let assignments = json!({
        "clips": clips,
        "cadence": spec.background.change_cadence,
        "pool_size": background_pool.len(),
    });
    std::fs::write(&output_path, serde_json::to_string_pretty(&assignments)?)?;

    Ok(json!({
        "status": "assigned",
        "output_path": output_path,
        "clip_count": clips.len(),
        "cadence": spec.background.change_cadence,
    }))
}

pub(crate) async fn handle_background_search(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let mood_filter = default_opt_str(&args, "mood");
    let energy_filter = default_opt_str(&args, "energy");
    let motion_filter = default_opt_str(&args, "motion_intensity");
    let limit = default_u32(&args, "limit", 10) as usize;

    // Resolve the index path CWD-independently. The round-2 UX audit
    // (GAP #12) found background.search only worked from the repo root
    // because it used a relative path. Now uses resolve_repo_path which
    // tries CWD > OPENSCRIPT_ROOT > CARGO_MANIFEST_DIR.
    let index_path_raw = std::env::var("OPENSCRIPT_BACKGROUNDS_INDEX")
        .unwrap_or_else(|_| "mcp/assets/backgrounds_index.json".to_string());
    let index_path = resolve_repo_path(&index_path_raw);

    if !index_path.exists() {
        return Err(ToolError::NotFound(format!(
            "Backgrounds index not found at {} (resolved from {}). The index is committed at mcp/assets/backgrounds_index.json — if missing, re-clone or restore from git.",
            index_path.display(),
            index_path_raw
        )));
    }

    let index_str = std::fs::read_to_string(&index_path)?;
    let index: serde_json::Value = serde_json::from_str(&index_str)?;

    let entries = index
        .get("entries")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut results: Vec<serde_json::Value> = Vec::new();
    for entry in &entries {
        if let Some(ref mood) = mood_filter {
            let entry_mood = entry.get("mood").and_then(|v| v.as_str()).unwrap_or("");
            if entry_mood != mood {
                continue;
            }
        }
        if let Some(ref energy) = energy_filter {
            let entry_energy = entry.get("energy").and_then(|v| v.as_str()).unwrap_or("");
            if entry_energy != energy {
                continue;
            }
        }
        if let Some(ref motion) = motion_filter {
            let entry_motion = entry
                .get("motion_intensity")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if entry_motion != motion {
                continue;
            }
        }

        // Build the full path so callers can use it directly in fallback_pool.
        // Resolve relative to the index file's parent's parent (so
        // mcp/assets/backgrounds_index.json → mcp/assets/backgrounds/).
        // This makes the returned paths work regardless of the agent's CWD.
        let filename = entry.get("filename").and_then(|v| v.as_str()).unwrap_or("");
        let backgrounds_dir = index_path
            .parent() // mcp/assets/
            .map(|p| p.join("backgrounds"))
            .unwrap_or_else(|| std::path::PathBuf::from("mcp/assets/backgrounds"));
        let full_path = backgrounds_dir.join(filename);
        let full_path_str = full_path.to_string_lossy().to_string();

        let mut result = entry.clone();
        if let Some(obj) = result.as_object_mut() {
            obj.insert("path".into(), json!(full_path_str));
        }
        results.push(result);
    }

    let total = results.len();
    results.truncate(limit);

    Ok(json!({
        "status": "searched",
        "filters": {
            "mood": mood_filter,
            "energy": energy_filter,
            "motion_intensity": motion_filter,
        },
        "total_matches": total,
        "count": results.len(),
        "results": results,
        "index_stats": {
            "total_entries": index.get("total_entries"),
            "mood_counts": index.get("mood_counts"),
        },
    }))
}

