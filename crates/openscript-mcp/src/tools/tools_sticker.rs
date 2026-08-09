// ---------------------------------------------------------------------------
// tools_sticker — Sticker handlers (sticker.*, GIPHY relevance pipeline)
// Split out of tools.rs (pure-move refactor). `use super::*` grants access
// to tools.rs's helpers, imports, and re-exported sibling handlers.
// ---------------------------------------------------------------------------
use super::*;

/// Handler: sticker.presets — list all available sticker positioning presets
pub(crate) async fn handle_sticker_presets(
    _args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let presets = openscript_core::sticker_presets::StickerPreset::all();
    let presets_json: serde_json::Value = serde_json::to_value(&presets)
        .map_err(|e| ToolError::InvalidArg(format!("Failed to serialize presets: {}", e)))?;
    Ok(json!({
        "status": "success",
        "count": presets.len(),
        "presets": presets_json,
        "message": "Use preset name in speaker.preset field of script JSON. Each preset defines position, scale, and caption-safe margin."
    }))
}

pub(crate) async fn handle_sticker_load_preset(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let preset_name = extract_str(&args, "preset_name")?;
    let presets_dir = default_str(&args, "presets_dir", "mcp/assets/svg_presets");

    let preset_dir = format!("{}/{}", presets_dir, preset_name);
    if !Path::new(&preset_dir).exists() {
        return Err(ToolError::NotFound(format!(
            "Preset not found: {} (looked in {})",
            preset_name, preset_dir
        )));
    }

    // Load preset.json
    let preset_json_path = format!("{}/preset.json", preset_dir);
    let preset_json = std::fs::read_to_string(&preset_json_path)?;
    let preset: StickerPreset = serde_json::from_str(&preset_json)
        .map_err(|e| ToolError::InvalidArg(format!("Failed to parse preset.json: {}", e)))?;

    // Load puppet.svg
    let puppet_svg_path = format!("{}/puppet.svg", preset_dir);
    let puppet_svg = std::fs::read_to_string(&puppet_svg_path)?;

    Ok(json!({
        "status": "loaded",
        "preset_name": preset_name,
        "preset": preset,
        "puppet_svg": puppet_svg,
    }))
}

pub(crate) async fn handle_sticker_render(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let wav_path = extract_str(&args, "wav_path")?;
    let preset_name = extract_str(&args, "preset_name")?;
    let position = default_str(&args, "position", "top-left");
    let scale = default_f64(&args, "scale", 0.25);
    let canvas_width = default_u32(&args, "canvas_width", 1080);
    let canvas_height = default_u32(&args, "canvas_height", 1920);
    let fps = default_u32(&args, "fps", 30);
    let output_path = default_str(&args, "output_path", "artifacts/sticker.html");
    let render_to_video = default_bool(&args, "render_to_video", false);

    report_progress(0.0, 100.0, "Loading preset...").await.ok();

    // Load preset
    let presets_dir = "mcp/assets/svg_presets";
    let preset_dir = format!("{}/{}", presets_dir, preset_name);
    if !Path::new(&preset_dir).exists() {
        return Err(ToolError::NotFound(format!(
            "Preset not found: {} (looked in {})",
            preset_name, preset_dir
        )));
    }

    let preset_json = std::fs::read_to_string(format!("{}/preset.json", preset_dir))?;
    let preset: StickerPreset = serde_json::from_str(&preset_json)
        .map_err(|e| ToolError::InvalidArg(format!("Preset parse error: {}", e)))?;

    let puppet_svg = std::fs::read_to_string(format!("{}/puppet.svg", preset_dir))?;

    report_progress(30.0, 100.0, "Extracting amplitude...")
        .await
        .ok();

    // Extract amplitude from WAV
    let amplitude = extract_amplitude(wav_path, fps)
        .map_err(|e| ToolError::InvalidArg(format!("Amplitude extraction failed: {}", e)))?;

    report_progress(60.0, 100.0, "Generating composition...")
        .await
        .ok();

    // Generate sticker HTML composition
    let html = generate_sticker_composition(
        &puppet_svg,
        &preset,
        &amplitude,
        &position,
        scale,
        canvas_width,
        canvas_height,
    );

    // Write output
    if let Some(parent) = Path::new(&output_path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&output_path, html)?;

    // Phase K: Optionally render the HTML to a transparent WebM via hf.render.
    // This produces a video file that multilayer_render can composite as a
    // StickerOverlay. The WebM format preserves alpha transparency.
    let mut video_path: Option<String> = None;
    if render_to_video {
        report_progress(80.0, 100.0, "Rendering sticker to WebM via HyperFrames...")
            .await
            .ok();

        let sticker_dir = std::path::Path::new(&output_path)
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf();
        let webm_path = sticker_dir
            .join(format!(
                "sticker_{}.webm",
                std::path::Path::new(&output_path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("sticker")
            ))
            .to_string_lossy()
            .to_string();

        match crate::hf::handle_hf_render(json!({
            "project_dir": sticker_dir.to_string_lossy().to_string(),
            "output_path": webm_path,
            "quality": "draft",
        }))
        .await
        {
            Ok(_) => {
                video_path = Some(webm_path);
            }
            Err(e) => {
                tracing::warn!("[sticker.render] HF render to WebM failed: {}", e);
                // Non-fatal — the HTML is still usable; the agent can render manually
            }
        }
    }

    report_progress(100.0, 100.0, "Sticker composition generated")
        .await
        .ok();

    Ok(json!({
        "status": "generated",
        "output_path": output_path,
        "video_path": video_path,
        "preset_name": preset_name,
        "position": position,
        "scale": scale,
        "frame_count": amplitude.frames.len(),
        "duration_ms": amplitude.duration_ms,
        "next_step": if video_path.is_some() {
            "Sticker rendered to WebM. Use the video_path as a StickerOverlay in multilayer_render or overlay.assign."
        } else {
            "Sticker HTML generated. Call sticker.render with render_to_video=true to produce a compositable WebM, or use the HTML with hf.render manually."
        },
    }))
}

pub(crate) async fn handle_sticker_keywords(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let segments = args
        .get("segments")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ToolError::MissingArg("segments".to_string()))?;
    if segments.is_empty() {
        return Ok(json!({"status": "warning", "message": "No segments provided", "segments": []}));
    }
    let language = default_str(&args, "language", "hinglish");
    let max_batch_size = default_u32(&args, "max_batch_size", 15).max(1) as usize;
    let _ = max_batch_size; // batching is owned by keywords::draft_scene_keywords

    // Unified draft — one batched LLM call emitting visual + reaction keywords
    // (reactions/intent/emphatic are the sticker subset; visual is ignored here).
    let mut draft_inputs: Vec<crate::keywords::SegmentInput> = Vec::with_capacity(segments.len());
    for (i, seg) in segments.iter().enumerate() {
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
        draft_inputs.push(crate::keywords::SegmentInput {
            segment_id: id,
            caption,
            language_hint: if language.is_empty() { None } else { Some(language.to_string()) },
            duration_s: 0.0,
            scene_idx: i,
            total_scenes: segments.len(),
            video_title: String::new(),
            video_keywords: Vec::new(),
            covered_concepts: Vec::new(),
        });
    }

    report_progress(30.0, 100.0, "Drafting sticker keywords (unified intent pass)...").await.ok();
    let drafted = crate::keywords::draft_scene_keywords(&draft_inputs).await;

    // Enrich segments: reactions/intent/emphatic from the unified draft.
    let mut enriched = Vec::new();
    let mut last_backend = String::new();
    let mut last_model = String::new();
    for (i, seg) in segments.iter().enumerate() {
        let _id = seg
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or(&format!("seg_{}", i))
            .to_string();
        let d = drafted.get(i);
        let reactions: Vec<String> = d.map(|d| d.reactions.clone()).unwrap_or_default();
        let emphatic = d.map(|d| d.emphatic).unwrap_or(false);
        let intent = d
            .and_then(|d| d.intent.clone())
            .unwrap_or_else(|| "emphasis".to_string());
        if let Some(d) = d {
            if d.source != crate::keywords::KeywordSource::Heuristic && last_backend.is_empty() {
                let parts: Vec<&str> = d.backend.split('/').collect();
                if parts.len() == 2 {
                    last_backend = parts[0].to_string();
                    last_model = parts[1].to_string();
                }
            }
        }
        // LLM-down path: the heuristic draft never auto-approves stickers
        // (emphatic=false, reactions=[]) — better no sticker than a wrong one.
        let keywords: Vec<String> = if emphatic { reactions } else { Vec::new() };
        let mut out = seg.clone();
        if let Some(obj) = out.as_object_mut() {
            obj.insert("sticker_keywords".into(), json!(keywords));
            obj.insert("intent".into(), json!(intent));
            obj.insert("emphatic".into(), json!(emphatic));
            if keywords.is_empty() {
                obj.insert("skip_reason".into(), json!("not_emphatic"));
            }
        }
        enriched.push(out);
    }

    Ok(json!({
        "status": "success",
        "segments": enriched,
        "count": enriched.len(),
        "backend": last_backend,
        "model": last_model,
    }))
}

pub(crate) async fn handle_sticker_validate_keywords(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let enriched_segments: Vec<serde_json::Value> = args
        .get("enriched_segments")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if enriched_segments.is_empty() {
        return Err(ToolError::MissingArg(
            "enriched_segments (from sticker.keywords)".to_string(),
        ));
    }
    let max_candidates = default_u32(&args, "max_candidates", 4).max(1) as usize;
    let language = default_str(&args, "language", "hinglish");

    let giphy_api_key = std::env::var("GIPHY_API_KEY").ok();
    if giphy_api_key.is_none() {
        return Ok(json!({
            "status": "warning",
            "message": "GIPHY_API_KEY not set — cannot search candidates for relevance validation. Draft keywords are returned unchanged; set the key or run sticker.auto_assign with an explicit sticker_query.",
            "validated": false,
            "segments": enriched_segments,
        }));
    }
    let giphy_api_key = giphy_api_key.unwrap();
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| ToolError::Asset(format!("HTTP client: {}", e)))?;

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
        let intent = seg
            .get("intent")
            .and_then(|v| v.as_str())
            .unwrap_or("emphasis")
            .to_string();
        let draft: Vec<String> = seg
            .get("sticker_keywords")
            .or_else(|| seg.get("keywords"))
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|k| k.as_str().map(String::from)).collect())
            .unwrap_or_default();

        if draft.is_empty() {
            // Record in BOTH the full segments list (so auto_assign knows this
            // segment was explicitly rejected — no caption-word fallback) and
            // the skipped summary (observability).
            validated.push(json!({
                "id": id,
                "caption": caption,
                "intent": intent,
                "approved": false,
                "skip_reason": "not_emphatic",
                "draft_keywords": [],
            }));
            skipped.push(json!({"id": id, "reason": "not_emphatic"}));
            continue;
        }

        // Search GIPHY with the top draft keywords, dedupe by sticker id.
        let limit = max_candidates.to_string();
        let mut candidates: Vec<serde_json::Value> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for q in draft.iter().take(2) {
            let url = match reqwest::Url::parse_with_params(
                "https://api.giphy.com/v1/stickers/search",
                &[
                    ("api_key", giphy_api_key.as_str()),
                    ("q", q.as_str()),
                    ("limit", limit.as_str()),
                    ("rating", "g"),
                    ("bundle", "sticker_layering"),
                ],
            ) {
                Ok(u) => u,
                Err(_) => continue,
            };
            let resp = match http.get(url).send().await {
                Ok(r) => r,
                Err(_) => continue,
            };
            if !resp.status().is_success() {
                continue;
            }
            let body: serde_json::Value = match resp.json().await {
                Ok(v) => v,
                Err(_) => continue,
            };
            if let Some(data) = body.get("data").and_then(|d| d.as_array()) {
                for item in data {
                    let sid = item.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    if sid.is_empty() || !seen.insert(sid.clone()) {
                        continue;
                    }
                    let url = item
                        .pointer("/images/original/url")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if url.is_empty() {
                        continue;
                    }
                    candidates.push(json!({
                        "id": sid,
                        "title": item.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                        "slug": item.get("slug").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                        "url": url,
                        "preview_url": item.pointer("/images/preview_gif/url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    }));
                    if candidates.len() >= max_candidates {
                        break;
                    }
                }
            }
            if candidates.len() >= max_candidates {
                break;
            }
        }

        if candidates.is_empty() {
            validated.push(json!({
                "id": id,
                "caption": caption,
                "intent": intent,
                "approved": false,
                "skip_reason": "no_giphy_results",
                "draft_keywords": draft,
            }));
            skipped.push(json!({"id": id, "reason": "no_giphy_results"}));
            continue;
        }

        // Agent validates the real candidates against the spoken caption.
        let (best_idx, final_keyword, relevance, reason, backend, model) =
            llm_validate_sticker_candidates(&caption, &intent, &draft, &candidates, &language).await;
        if !backend.is_empty() {
            last_backend = backend;
        }
        if !model.is_empty() {
            last_model = model;
        }

        let best_sticker = best_idx.and_then(|bi| candidates.get(bi)).cloned();
        match best_sticker {
            Some(sticker) => validated.push(json!({
                "id": id,
                "caption": caption,
                "intent": intent,
                // Both spellings emitted so any consumer (keyword search OR
                // direct-pick path) reads the same field it already expects.
                "sticker_keywords": draft,
                "draft_keywords": draft,
                "final_keyword": final_keyword,
                "approved": true,
                "relevance": relevance,
                "reason": reason,
                "best_sticker": sticker,
                "candidates": candidates,
            })),
            None => {
                validated.push(json!({
                    "id": id,
                    "caption": caption,
                    "intent": intent,
                    "approved": false,
                    "skip_reason": "relevance_rejected",
                    "draft_keywords": draft,
                }));
                skipped.push(json!({
                    "id": id,
                    "reason": "relevance_rejected",
                    "caption": caption,
                }));
            }
        }
    }

    // `segments` carries EVERY processed segment (approved + rejected with a
    // skip_reason) so sticker.auto_assign never falls back to caption-word
    // queries for segments the relevance gate already rejected.
    let approved_count = validated
        .iter()
        .filter(|s| s.get("approved").and_then(|v| v.as_bool()).unwrap_or(false))
        .count();
    Ok(json!({
        "status": "validated",
        "backend": last_backend,
        "model": last_model,
        "validated_count": approved_count,
        "processed_count": validated.len(),
        "skipped_count": skipped.len(),
        "skipped": skipped,
        "segments": validated,
    }))
}

pub(crate) async fn handle_sticker_auto(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let timeline_path_arg = args.get("timeline_path").and_then(|v| v.as_str()).map(String::from);
    let position = default_str(&args, "position", "auto");
    let scale = default_f64(&args, "scale", 0.25);
    let max_stickers = default_u32(&args, "max_stickers", 12) as usize;
    let min_gap_s = default_f64(&args, "min_gap_s", 2.0).max(0.0);

    // Stage A: resolve timeline + segments (same pattern as broll.auto)
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
        let srt = args
            .get("srt_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingArg("sticker.auto requires srt_path + audio_path (or timeline_path)".into()))?
            .to_string();
        let audio = args
            .get("audio_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingArg("sticker.auto requires audio_path (or timeline_path)".into()))?
            .to_string();
        report_progress(5.0, 100.0, "1/3 segment.analyze").await.ok();
        let analyzed = handle_segment_analyze(json!({
            "audio_path": audio,
            "srt_path": srt,
            "min_duration_s": 2.0,
            "max_duration_s": 6.0,
        }))
        .await?;
        let segments: Vec<serde_json::Value> = analyzed
            .get("segments")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let out_path = args
            .get("output_path")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| {
                let stem = Path::new(&srt)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "sticker_auto".to_string());
                format!("artifacts/{}.timeline.json", stem)
            });
        report_progress(20.0, 100.0, "2/3 srt.to_timeline").await.ok();
        let built = handle_srt_to_timeline(json!({
            "srt_path": srt,
            "source_video": audio,
            "output_path": out_path,
            "aspect": "9:16",
            "fps": 30,
            "min_duration_s": 2.0,
            "max_duration_s": 6.0,
        }))
        .await?;
        let tl = built
            .get("timeline_path")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or(out_path);
        (tl, segments)
    };

    if segments.is_empty() {
        return Err(ToolError::InvalidArg("sticker.auto: no segments found — check SRT/timeline".into()));
    }

    // Stage B: keyword draft. When the caller already drafted keywords (e.g.
    // broll.auto passes its validated b-roll keywords so ONE keyword source
    // drives both b-roll and stickers — the unification), use them directly and
    // skip the separate LLM sticker-intent pass. Otherwise run the agentic
    // sticker.keywords draft (intent + emphatic).
    let shared_keywords: Vec<serde_json::Value> = args
        .get("shared_keywords")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    report_progress(35.0, 100.0, "2/4 sticker.keywords (agentic intent draft)").await.ok();
    let (mut enriched_segments, backend) = if !shared_keywords.is_empty() {
        (json!(shared_keywords), json!("shared_broll_keywords"))
    } else {
        let drafts = handle_sticker_keywords(json!({
            "segments": segments,
            "language": default_str(&args, "language", "hinglish"),
            "max_batch_size": 15,
        }))
        .await?;
        (
            drafts.get("segments").cloned().unwrap_or_else(|| json!([])),
            drafts.get("backend").cloned().unwrap_or_else(|| json!("")),
        )
    };

    // Stage C: relevance gate — approve only stickers that genuinely match the
    // spoken intent (mirror of broll.validate_keywords). GIPHY/LLM-down ⇒ drafts
    // pass through unchanged; auto_assign's fallbacks + spacing still apply.
    report_progress(55.0, 100.0, "3/4 sticker.validate_keywords (relevance gate)").await.ok();
    let validated = handle_sticker_validate_keywords(json!({
        "enriched_segments": enriched_segments,
        "language": default_str(&args, "language", "hinglish"),
        "max_candidates": 4,
    }))
    .await?;
    if validated.get("status").and_then(|v| v.as_str()) == Some("validated") {
        enriched_segments = validated.get("segments").cloned().unwrap_or_else(|| json!([]));
    }

    // Stage D: search + download + place (approved picks download directly)
    report_progress(70.0, 100.0, "4/4 sticker.auto_assign (GIPHY + place)").await.ok();
    let placed = handle_sticker_auto_assign(json!({
        "timeline_path": timeline_path,
        "enriched_segments": enriched_segments,
        "position": position,
        "scale": scale,
        "max_stickers": max_stickers,
        "min_gap_s": min_gap_s,
    }))
    .await?;

    let stickers_placed = placed.get("events_created").and_then(|v| v.as_u64()).unwrap_or(0);
    let skipped = placed.get("skipped").cloned().unwrap_or_else(|| json!([]));
    let skipped_count = placed.get("skipped_count").and_then(|v| v.as_u64()).unwrap_or_else(|| {
        skipped.as_array().map(|a| a.len() as u64).unwrap_or(0)
    });
    report_progress(100.0, 100.0, "sticker.auto complete").await.ok();

    Ok(json!({
        "status": if stickers_placed > 0 { "success" } else { "warning" },
        "message": format!(
            "Sticker pipeline complete: {} segment(s) analyzed, {} sticker(s) placed, {} skipped (see skipped reasons: intent gate / relevance gate / spacing).",
            segments.len(),
            stickers_placed,
            skipped_count
        ),
        "timeline_path": timeline_path,
        "segments_count": segments.len(),
        "stickers_placed": stickers_placed,
        "skipped": skipped,
        "sticker_keywords_backend": backend,
        "pipeline": json!(["segment.analyze", "sticker.keywords", "sticker.validate_keywords", "sticker.auto_assign"]),
    }))
}

pub(crate) async fn handle_sticker_auto_assign(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let timeline_path = extract_str(&args, "timeline_path")?;
    let sticker_query: Option<String> = args.get("sticker_query").and_then(|v| v.as_str()).map(|s| s.to_string());
    // "auto" (default) cycles anchors for visual variety; an explicit position
    // (e.g. "top-right") anchors every sticker there (manual override).
    let position = default_str(&args, "position", "auto");
    let scale = default_f64(&args, "scale", 0.25);
    let max_stickers = default_u32(&args, "max_stickers", 10) as usize;
    // Minimum seconds between consecutive sticker placements (spacing gate).
    let min_gap_s = default_f64(&args, "min_gap_s", 2.0).max(0.0);
    let enriched_segments: Vec<serde_json::Value> = args
        .get("enriched_segments")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut timeline = Timeline::load(timeline_path)?;
    let segments = timeline.segments.clone();
    if segments.is_empty() {
        return Ok(json!({"status": "warning", "message": "No segments found — cannot auto-assign stickers", "events_created": 0}));
    }

    let giphy_api_key = std::env::var("GIPHY_API_KEY").ok();
    if giphy_api_key.is_none() {
        return Ok(json!({"status": "warning", "message": "GIPHY_API_KEY not set — cannot search for stickers. Set GIPHY_API_KEY env var.", "events_created": 0}));
    }
    let giphy_api_key = giphy_api_key.unwrap();

    // Map segment id → sticker_keywords from sticker.keywords output, if given.
    let mut keyword_by_seg: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    // Map segment id → validated segment (sticker.validate_keywords output).
    // When present, the approved sticker is downloaded DIRECTLY (no re-search)
    // and the relevance gate is respected.
    let mut best_sticker_by_seg: std::collections::HashMap<String, serde_json::Value> = std::collections::HashMap::new();
    // Map segment id → skip_reason from the relevance/intent gate. Segments the
    // gate rejected MUST NOT fall back to caption-word queries.
    let mut skip_reason_by_seg: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for es in &enriched_segments {
        if let Some(id) = es.get("id").and_then(|v| v.as_str()) {
            let kws: Vec<String> = es
                .get("sticker_keywords")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|k| k.as_str().map(String::from)).collect())
                .unwrap_or_default();
            if !kws.is_empty() {
                keyword_by_seg.insert(id.to_string(), kws);
            }
            if let Some(r) = es.get("skip_reason").and_then(|v| v.as_str()) {
                skip_reason_by_seg.insert(id.to_string(), r.to_string());
            }
            // Approved validated picks only — never auto-place an unapproved one.
            let has_best_url = es
                .get("best_sticker")
                .and_then(|v| v.get("url"))
                .and_then(|v| v.as_str())
                .map(|s| !s.is_empty())
                .unwrap_or(false);
            if es.get("approved").and_then(|v| v.as_bool()).unwrap_or(false) && has_best_url {
                best_sticker_by_seg.insert(id.to_string(), es.clone());
            }
        }
    }

    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| ToolError::Asset(format!("HTTP client: {}", e)))?;

    let mut events_created: Vec<serde_json::Value> = Vec::new();
    let mut skipped: Vec<serde_json::Value> = Vec::new();
    let mut current_idx = track_count(&timeline, &TrackType::Stickers);
    let stickers_dir = std::path::PathBuf::from("mcp/assets/stickers");
    let _ = std::fs::create_dir_all(&stickers_dir);

    let mut last_sticker_end_s: Option<f64> = None;
    let mut placed_count = 0usize;
    // No-duplicate-sticker guarantee: GIPHY ids placed earlier in this run are
    // never placed again — two segments with similar keywords often resolve to
    // the SAME top GIPHY sticker (the "same sticker repeats" bug).
    let mut used_sticker_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for seg in segments.iter() {
        if events_created.len() >= max_stickers { break; }

        let seg_id = seg.id.clone();
        // Honored rejection: the intent/relevance gate already decided this
        // segment gets no sticker — never fall back to caption-word queries
        // (that fallback IS the irrelevance bug). An explicit sticker_query is
        // a manual override and bypasses the gate.
        if sticker_query.is_none() {
            if let Some(reason) = skip_reason_by_seg.get(&seg_id) {
                skipped.push(json!({
                    "segment_id": seg.id,
                    "reason": reason.clone(),
                    "query": String::new(),
                }));
                continue;
            }
        }

        // Spacing gate: never place a sticker adjacent to the previous one
        // (min_gap_s between the previous sticker's end and this segment start).
        if !sticker_spacing_allowed(last_sticker_end_s, seg.start, min_gap_s) {
            skipped.push(json!({
                "segment_id": seg.id,
                "reason": "adjacent_spacing",
                "detail": format!(
                    "segment starts {:.1}s after previous sticker's end (min gap {:.1}s)",
                    seg.start - last_sticker_end_s.unwrap_or(0.0),
                    min_gap_s
                ),
            }));
            continue;
        }

        // Validated pick (sticker.validate_keywords) → download DIRECTLY, no
        // re-search; query = final_keyword (provenance + observability).
        let mut query = String::new();
        let mut chosen_url = String::new();
        let mut chosen_title = String::new();
        let mut chosen_sticker_id = String::new();
        if let Some(vs) = best_sticker_by_seg.get(&seg_id) {
            if let Some(bs) = vs.get("best_sticker") {
                // Duplicate guard for validated picks: two segments can approve
                // the same GIPHY sticker — the second is skipped, never re-placed.
                let sticker_giphy_id = bs.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if !sticker_giphy_id.is_empty() && used_sticker_ids.contains(&sticker_giphy_id) {
                    skipped.push(json!({
                        "segment_id": seg.id,
                        "reason": "duplicate_sticker",
                        "detail": format!("GIPHY sticker {} already placed on this timeline", sticker_giphy_id),
                    }));
                    continue;
                }
                chosen_sticker_id = sticker_giphy_id;
                query = vs
                    .get("final_keyword")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .or_else(|| bs.get("title").and_then(|v| v.as_str()))
                    .unwrap_or("")
                    .to_string();
                chosen_url = bs.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();
                chosen_title = bs.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
            }
        }

        if chosen_url.is_empty() {
            // Derive query: per-segment enriched keywords > global override > caption words
            query = if let Some(ref q) = sticker_query {
                q.clone()
            } else {
                keyword_by_seg
                    .get(&seg_id)
                    .and_then(|kws| kws.first().cloned())
                    .or_else(|| {
                        let words: Vec<&str> = seg.caption.split_whitespace().filter(|w: &&str| w.len() > 3).take(3).collect();
                        if words.is_empty() { Some("funny".to_string()) } else { Some(words.join(" ")) }
                    })
                    .unwrap_or_else(|| "funny".to_string())
            };
            let url = reqwest::Url::parse_with_params(
                "https://api.giphy.com/v1/stickers/search",
                &[
                    ("api_key", giphy_api_key.as_str()),
                    ("q", query.as_str()),
                    ("limit", "3"),
                    ("rating", "g"),
                    ("bundle", "sticker_layering"),
                ],
            )
            .map_err(|e| ToolError::InvalidArg(format!("URL parse: {}", e)))?;

            let resp = match http.get(url).send().await {
                Ok(r) => r,
                Err(_) => continue,
            };
            if !resp.status().is_success() { continue; }
            let body: serde_json::Value = match resp.json().await {
                Ok(v) => v,
                Err(_) => continue,
            };
            let data = body.get("data").and_then(|d| d.as_array()).cloned().unwrap_or_default();
            if data.is_empty() {
                skipped.push(json!({"segment_id": seg.id, "query": query, "reason": "no GIPHY results"}));
                continue;
            }

            // Pick the first result whose original URL is downloadable and that
            // has not already been placed (duplicate-sticker guard).
            for item in &data {
                let gid = item.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if !gid.is_empty() && used_sticker_ids.contains(&gid) {
                    continue;
                }
                let u = item.pointer("/images/original/url").and_then(|v| v.as_str()).unwrap_or("");
                if !u.is_empty() {
                    chosen_url = u.to_string();
                    chosen_title = item.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    chosen_sticker_id = gid;
                    break;
                }
            }
            if chosen_url.is_empty() { continue; }
        }

        // Download via the existing gif.download handler (cache-aware)
        let dl = handle_gif_download(json!({
            "url": chosen_url,
        }))
        .await?;
        let asset_path_str = dl
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if asset_path_str.is_empty() || asset_path_str == "placeholder" {
            skipped.push(json!({"segment_id": seg.id, "query": query, "reason": "download failed"}));
            continue;
        }
        if !chosen_sticker_id.is_empty() {
            used_sticker_ids.insert(chosen_sticker_id);
        }

        // Place on the Stickers track. asset_id = event_id (registry key
        // convention used by broll.fetch) so the renderer resolves the path.
        let place_pos = sticker_place_position(&position, placed_count);
        current_idx += 1;
        let event_id = format!("sticker_{:03}", current_idx);
        let start_ms = (seg.start * 1000.0) as i64;
        let end_ms = ((seg.end * 1000.0) as i64).min(start_ms + 5000); // Cap at 5s

        let event = openscript_core::timeline::TimelineEvent {
            id: event_id.clone(),
            asset_id: event_id.clone(),
            start_ms,
            end_ms,
            offset_ms: 0,
            gain_db: 0.0,
            fade_in_ms: 150,
            fade_out_ms: 150,
            tags: vec!["sticker".to_string(), place_pos.clone()],
            provenance: Some(openscript_core::timeline::Provenance {
                tool: "sticker.auto_assign".into(),
                editorial_role: Some("decoration".into()),
                concept: Some(query.clone()),
            }),
            kind: openscript_core::timeline::EventKind::Broll {
                concept: format!("overlay:{}", place_pos),
                source_provider: asset_path_str.clone(),
                transition_style: "overlay".into(),
                crop_mode: "none".into(),
                orientation: "9:16".into(),
                motion_intensity: "static".into(),
            },
        };

        timeline.add_track_event(TrackType::Stickers, event);
        timeline.add_asset("broll", event_id.clone(), json!({
            "path": asset_path_str,
            "position": place_pos,
            "scale": scale,
            "overlay": true,
        }));
        events_created.push(json!({
            "event_id": event_id,
            "position_ms": start_ms,
            "position": place_pos,
            "sticker_path": asset_path_str,
            "query": query,
            "title": chosen_title,
        }));
        last_sticker_end_s = Some(seg.end);
        placed_count += 1;
    }

    timeline.save(timeline_path)?;
    Ok(json!({
        "status": if events_created.is_empty() { "warning" } else { "success" },
        "message": if events_created.is_empty() {
            "No stickers placed — check GIPHY_API_KEY and segment content.".into()
        } else {
            format!("{} sticker(s) placed on the Stickers track.", events_created.len())
        },
        "events_created": events_created.len(),
        "positions": events_created,
        "skipped": skipped,
        "timeline_path": timeline_path,
    }))
}

