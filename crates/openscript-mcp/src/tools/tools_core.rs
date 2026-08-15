// ---------------------------------------------------------------------------
// tools_core — Core pipeline + timeline handlers (transcribe, srt, captions, edl, render, reelize, overlay, timeline.*)
// Split out of tools.rs (pure-move refactor). `use super::*` grants access
// to tools.rs's helpers, imports, and re-exported sibling handlers.
// ---------------------------------------------------------------------------
use super::*;

pub(crate) async fn handle_transcribe(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    // Feature gate: transcription is a toggleable subsystem — a config that
    // turns it off gets a clear error instead of a missing-engine failure.
    if !crate::config::feature_transcription("hinglish_ggml") {
        return Err(ToolError::Srt(
            "Transcription (hinglish-ggml) is disabled in the active configuration. Enable \
             features.transcription.hinglish_ggml=true in ~/.openscript/config.json (or set \
             OPENSCRIPT_FEATURE_TRANSCRIPTION_HINGLISH_GGML=1), then run: bash setup.sh"
                .to_string(),
        ));
    }
    let media_path = sanitize_input_path(extract_str(&args, "media_path")?)?
        .to_string_lossy()
        .to_string();
    let output_srt_path = default_opt_str(&args, "output_srt_path").unwrap_or_else(|| {
        let p = Path::new(&media_path);
        let parent = p.parent().unwrap_or(Path::new("."));
        let stem = p
            .file_stem()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        parent
            .join(format!("{}.srt", stem))
            .to_string_lossy()
            .to_string()
    });

    if !Path::new(&media_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Media file not found: {}",
            media_path
        )));
    }

    let language_hint = default_str(&args, "language_hint", "auto");        // All transcription uses HinglishGgml (the sole engine)
        let engine = openscript_transcribe::transcriber::TranscriptionEngine::HinglishGgml;

    report_progress(0.0, 100.0, "Starting transcription...")
        .await
        .ok();

    // Wire progress callback so the MCP client sees real-time transcription progress
    let progress_cb = |pct: f64, msg: &str| {
        let msg_owned = msg.to_string();
        tokio::spawn(async move {
            let _ = report_progress(pct, 100.0, &msg_owned).await;
        });
    };
    let result = transcribe_with_engine(&media_path, &output_srt_path, engine, &language_hint, Some(&progress_cb))
        .await
        .map_err(|e| ToolError::Srt(e.to_string()))?;

    report_progress(100.0, 100.0, "Transcription complete")
        .await
        .ok();

    Ok(json!({
        "status": "transcribed",
        "output_srt_path": result.output_path,
        "entry_count": result.entry_count,
        "word_srt_path": result.word_srt_path,
        "phrase_srt_path": result.phrase_srt_path,
        "engine": format!("{}", result.engine),
    }))
}

// ---------------------------------------------------------------------------
// Handler: captions.generate_ass
// ---------------------------------------------------------------------------
pub(crate) async fn handle_captions_generate_ass(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    // Support auto-loading srt_path from timeline when only timeline_path is provided.
    let srt_path = if let Some(s) = args.get("srt_path").and_then(|v| v.as_str()) {
        s.to_string()
    } else if let Some(tl_path) = args.get("timeline_path").and_then(|v| v.as_str()) {
        // Derive SRT path from timeline's source field
        let tl = Timeline::load(tl_path)?;
        let source = tl.source.to_string_lossy().to_string();
        if source.is_empty() {
            return Err(ToolError::MissingArg(
                "srt_path (or timeline_path with source set)".to_string(),
            ));
        }
        // Replace video extension with .srt
        let path = std::path::Path::new(&source);
        let srt = path.with_extension("srt");
        if !srt.exists() {
            return Err(ToolError::NotFound(format!(
                "SRT not found at {} — derived from timeline source {}",
                srt.display(), source
            )));
        }
        srt.to_string_lossy().to_string()
    } else {
        return Err(ToolError::MissingArg(
            "srt_path or timeline_path".to_string(),
        ));
    };
    // Optional word-level SRT (from transcribe's word_srt_path). When present,
    // parse THAT instead of the phrase SRT so per-word timings are real
    // transcription alignments — the caption-voice sync fix for the A2V path.
    let word_srt_path = args.get("word_srt_path")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let grouped_srt_path = args.get("grouped_srt_path")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let style = default_str(&args, "style", "word_highlight");
    let font = default_str(&args, "font", "Bebas Neue");
    let font_size = default_u32(&args, "font_size", 84);
    let color = default_str(&args, "color", "#ffffff");
    let highlight_color = default_str(&args, "highlight_color", "#00ff88");
    let position = default_str(&args, "position", "center");
    let safe_zone = args.get("safe_zone").and_then(|v| v.as_f64()).unwrap_or(0.85);
    let max_words_per_line = default_u32(&args, "max_words_per_line", 5);
    let width = default_u32(&args, "width", 1080);
    let height = default_u32(&args, "height", 1920);
    let crossfade_ms = args.get("crossfade_ms").and_then(|v| v.as_i64()).map(|v| v as u32);
    let output_path = args.get("output_path")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let spec = CaptionsSpec {
        style: style.to_string(),
        font: font.to_string(),
        font_size,
        color: color.to_string(),
        highlight_color: highlight_color.to_string(),
        position: position.to_string(),
        safe_zone,
        max_words_per_line,
    };

    // Determine output path
    let ass_path = if let Some(p) = output_path {
        p
    } else {
        let parent = Path::new(&srt_path).parent().unwrap_or(Path::new("."));
        parent.join("captions.ass").to_string_lossy().to_string()
    };

    // Caption TEXT and phrase windows ALWAYS come from the phrase transcript
    // (srt_path) — the language-correct captions (e.g. Hinglish). The word SRT
    // (word_srt_path) only contributes REAL per-word timings when its words
    // actually align with the phrase text (same language + content); on a
    // language mismatch or a stale/partial word SRT, per-word timings fall
    // back to char-proportional estimates. This fixes the A2V caption bugs:
    //   1. English captions on Hinglish audio (a foreign word SRT was used as
    //      the entire caption source, replacing the Hinglish phrase text).
    //   2. Captions breaking off mid-video (the word SRT covered only part of
    //      the audio, so the ASS inherited a 60s hole).
    let caption_segments: Vec<CaptionSegment> = {
        // Parse the phrase transcript first — authoritative text + windows.
        let phrase_entries = match openscript_core::srt::parse_srt(&srt_path) {
            Ok(e) if !e.is_empty() => e,
            Ok(_) | Err(_) => {
                // Fallback: try grouped SRT with estimated word timings.
                let fallback_path = grouped_srt_path.as_deref().unwrap_or(&srt_path);
                let (err_note, fallback_entries) = match openscript_core::srt::parse_srt(fallback_path) {
                    Ok(fb) if !fb.is_empty() => {
                        tracing::warn!("Phrase SRT parse failed/empty for captions, using grouped SRT fallback");
                        (String::new(), fb)
                    }
                    Ok(_) | Err(_) => {
                        return Err(ToolError::InvalidArg(format!(
                            "Failed to parse SRT {} (fallback {} also failed)", srt_path, fallback_path
                        )));
                    }
                };
                tracing::warn!("Phrase SRT unavailable for captions: {}", err_note);
                fallback_entries
            }
        };
        // Word-level timings (optional enrichment only).
        let word_entries: Vec<openscript_core::srt::SrtEntry> = word_srt_path
            .as_deref()
            .and_then(|p| openscript_core::srt::parse_srt(p).ok())
            .unwrap_or_default();

        // Crossfade remap (output clock) when segments overlap via xfade.
        let crossfade_s = crossfade_ms.map(|cf| cf as f64 / 1000.0);
        let mut out_offsets: Vec<f64> = Vec::with_capacity(phrase_entries.len());
        let mut accum = 0.0;
        for (i, ph) in phrase_entries.iter().enumerate() {
            out_offsets.push(accum - (i as f64 * crossfade_s.unwrap_or(0.0)));
            accum += ph.end - ph.start;
        }

        phrase_entries
            .iter()
            .enumerate()
            .map(|(i, ph)| {
                let out_start = (out_offsets[i] * 1000.0).round() as i64;
                let out_end = out_start + ((ph.end - ph.start) * 1000.0).round() as i64;
                let words = caption_words_for_phrase(ph, &word_entries, out_start, ph.start);
                CaptionSegment {
                    text: ph.text.clone(),
                    start_ms: out_start,
                    end_ms: out_end,
                    words,
                }
            })
            .collect()
    };

    let ass_content = generate_ass(&caption_segments, &spec, width, height);
    std::fs::write(&ass_path, &ass_content)?;

    let canonical_ass = std::fs::canonicalize(&ass_path)
        .unwrap_or_else(|_| ass_path.clone().into());

    // AUTO-REGISTER: If timeline_path provided, register ASS in timeline.assets.captions
    let captions_timeline_path = default_opt_str(&args, "timeline_path");
    if let Some(ref tl_path) = captions_timeline_path {
        if let Ok(mut tl) = Timeline::load(tl_path) {
            tl.assets.captions.insert("ass".to_string(), serde_json::json!({
                "path": canonical_ass.to_string_lossy().to_string(),
            }));
            // Register caption style in effects so verify.production can detect it
            tl.effects.caption_style = Some(style);
            tl.updated_at = chrono::Utc::now();
            let _ = tl.save(tl_path);
        }
    }

    Ok(json!({
        "status": "success",
        "ass_path": canonical_ass.to_string_lossy().to_string(),
        "segment_count": caption_segments.len(),
    }))
}

pub(crate) async fn handle_srt_read(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let srt_path = sanitize_input_path(extract_str(&args, "srt_path")?)?
        .to_string_lossy()
        .to_string();
    let entries = parse_srt(&srt_path)?;
    let result: Vec<serde_json::Value> = entries
        .iter()
        .map(|e| {
            json!({
                "idx": e.idx,
                "start": e.start,
                "end": e.end,
                "text": e.text,
            })
        })
        .collect();
    Ok(json!({
        "status": "success",
        "srt_path": srt_path,
        "count": result.len(),
        "entries": result,
    }))
}

pub(crate) async fn handle_srt_prepare(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let srt_path = sanitize_input_path(extract_str(&args, "srt_path")?)?
        .to_string_lossy()
        .to_string();
    let max_words = default_u32(&args, "max_words", 10) as usize;
    let max_chars = default_u32(&args, "max_chars", 64) as usize;
    let max_gap = default_f64(&args, "max_gap", 0.6);
    let max_duration_s = default_f64(&args, "max_duration_s", 5.0);

    let entries = parse_srt(&srt_path)?;
    let groups = {
        use openscript_core::srt::group_entries_with_words_max_duration;
        let phrases = group_entries_with_words_max_duration(
            &entries, max_words, max_chars, max_gap, max_duration_s,
        );
        phrases.into_iter().map(|p| (p.text, p.start, p.end)).collect::<Vec<_>>()
    };

    let out_srt_path = {
        let p = Path::new(&srt_path);
        let parent = p.parent().unwrap_or(Path::new("."));
        let stem = p
            .file_stem()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        parent
            .join(format!("{}.grouped.srt", stem))
            .to_string_lossy()
            .to_string()
    };
    let flat: Vec<(String, f64, f64)> = groups
        .iter()
        .map(|(text, start, end)| (text.clone(), *start, *end))
        .collect();
    write_srt(&flat, &out_srt_path)?;

    let result: Vec<serde_json::Value> = groups
        .iter()
        .map(|(text, start, end)| {
            json!({
                "text": text,
                "start": start,
                "end": end,
            })
        })
        .collect();

    Ok(json!({
        "status": "success",
        "output_path": out_srt_path,
        "count": result.len(),
        "groups": result,
    }))
}

pub(crate) async fn handle_srt_apply_edit(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_core::srt::{parse_srt, retime_srt};
    use openscript_ffmpeg::render::{render, RenderConfig};
    use openscript_ffmpeg::subtitles::srt_to_ass;

    let video_path = extract_str(&args, "video_path")?;
    let edited_srt_path = extract_str(&args, "edited_srt_path")?;
    let merge_gap = default_f64(&args, "merge_gap", 0.25);
    let crossfade_ms = default_u32(&args, "crossfade_ms", 80);
    let aspect = default_str(&args, "aspect", "9:16");
    let burn_captions = default_bool(&args, "burn_captions", true);
    let crf = default_u32(&args, "crf", 20);
    let fps = default_u32(&args, "fps", 30);

    report_progress(0.0, 100.0, "Parsing edited SRT...")
        .await
        .ok();

    let edited_entries = parse_srt(edited_srt_path).map_err(|e| ToolError::Srt(e.to_string()))?;

    if edited_entries.is_empty() {
        return Err(ToolError::Srt("Edited SRT has no entries".to_string()));
    }

    // Build EDL segments from edited SRT entries
    let segments: Vec<(f64, f64, String)> = edited_entries
        .iter()
        .map(|e| (e.start, e.end, e.text.clone()))
        .collect();

    // Create EDL v1 JSON
    let edl = json!({
        "source": video_path,
        "target": {"aspect": aspect, "fps": fps},
        "segments": segments.iter().enumerate().map(|(i, (s, e, t))| {
            json!({"id": format!("seg_{:03}", i + 1), "start": s, "end": e, "caption": t, "crossfade_ms": crossfade_ms})
        }).collect::<Vec<_>>(),
        "effects": {"burn_captions": burn_captions, "audio": {"loudnorm": true}},
    });

    // Save EDL alongside the edited SRT
    let edl_path = {
        let p = Path::new(edited_srt_path);
        let parent = p.parent().unwrap_or(Path::new("."));
        let stem = p
            .file_stem()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        parent
            .join(format!("{}.edl.json", stem))
            .to_string_lossy()
            .to_string()
    };
    let edl_json = serde_json::to_string_pretty(&edl).map_err(ToolError::Json)?;
    std::fs::write(&edl_path, edl_json).map_err(ToolError::Io)?;

    // Generate ASS subtitles if burn_captions
    let ass_path = if burn_captions {
        report_progress(20.0, 100.0, "Generating subtitle styles...")
            .await
            .ok();
        let orig_srt = segments.clone();
        let retimed = retime_srt(
            &orig_srt,
            &segments
                .iter()
                .map(|(s, e, _)| (*s, *e))
                .collect::<Vec<_>>(),
            merge_gap,
        );

        let ass_out = Path::new(&edl_path).with_extension("ass");
        let ass_path_str = ass_out.to_string_lossy().into_owned();
        srt_to_ass(&retimed, &ass_path_str, "Default")
            .map_err(|e| ToolError::Ffmpeg(e.to_string()))?;
        Some(ass_path_str)
    } else {
        None
    };

    // Render
    report_progress(40.0, 100.0, "Rendering edited video...")
        .await
        .ok();
    let config = RenderConfig {
        video_path: video_path.to_string(),
        edl_path: edl_path.clone(),
        burn_captions,
        srt_path: Some(edited_srt_path.to_string()),
        ass_path,
        overlay_mov: None,
        aspect,
        crf,
        fps,
    };

    let output_path = render(config)
        .await
        .map_err(|e| ToolError::Ffmpeg(e.to_string()))?;

    report_progress(100.0, 100.0, "Edit applied and rendered")
        .await
        .ok();

    Ok(json!({
        "status": "rendered",
        "output_path": output_path,
        "edl_path": edl_path,
        "segments_count": segments.len(),
        "total_duration_s": segments.iter().map(|(s, e, _)| e - s).sum::<f64>(),
    }))
}

pub(crate) async fn handle_edl_build(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let srt_path = sanitize_input_path(extract_str(&args, "srt_path")?)?
        .to_string_lossy()
        .to_string();
    let strategy = default_str(&args, "strategy", "keep");
    let max_duration = default_opt_f64(&args, "max_duration");
    let crossfade_ms = default_u32(&args, "crossfade_ms", 120);
    let analysis_path = default_opt_str(&args, "analysis_path");
    let aspect = default_str(&args, "aspect", "9:16");

    let entries = parse_srt(&srt_path).map_err(|e| ToolError::Srt(e.to_string()))?;

    let groups = group_entries(&entries, 10, 64, 0.6);

    let analysis = analyze_srt(&groups);

    if let Some(ap) = &analysis_path {
        let analysis_json =
            serde_json::to_string_pretty(&analysis).map_err(ToolError::Json)?;
        std::fs::write(ap, analysis_json).map_err(ToolError::Io)?;
    }

    let segments = build_edl(&analysis, &strategy, max_duration, crossfade_ms);

    let edl = json!({
        "source": "",
        "target": {"aspect": aspect, "fps": 30},
        "segments": segments.iter().enumerate().map(|(i, (s, e, t))| {
            json!({"id": format!("seg_{:03}", i + 1), "start": s, "end": e, "caption": t, "crossfade_ms": crossfade_ms})
        }).collect::<Vec<_>>(),
        "effects": {"burn_captions": true, "audio": {"loudnorm": true}},
    });

    let output_path = {
        let p = Path::new(&srt_path);
        let parent = p.parent().unwrap_or(Path::new("."));
        let stem = p
            .file_stem()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        parent
            .join(format!("{}.edl.json", stem))
            .to_string_lossy()
            .to_string()
    };

    let edl_json = serde_json::to_string_pretty(&edl).map_err(ToolError::Json)?;
    std::fs::write(&output_path, edl_json).map_err(ToolError::Io)?;

    let total_duration: f64 = segments.iter().map(|(s, e, _)| e - s).sum();

    Ok(json!({
        "status": "built",
        "edl_path": output_path,
        "strategy": strategy,
        "segments_count": segments.len(),
        "total_duration_s": total_duration,
        "analysis_count": analysis.len(),
    }))
}

pub(crate) async fn handle_render(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_core::srt::parse_srt;
    use openscript_ffmpeg::render::{render, RenderConfig};
    use openscript_ffmpeg::subtitles::srt_to_ass;

    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?
        .to_string_lossy()
        .to_string();
    let edl_path = sanitize_input_path(extract_str(&args, "edl_path")?)?
        .to_string_lossy()
        .to_string();
    let burn_captions = default_bool(&args, "burn_captions", true);
    let srt_path = default_opt_str(&args, "srt_path");
    let ass_path = default_opt_str(&args, "ass_path");
    let aspect = default_str(&args, "aspect", "9:16");
    let crf = default_u32(&args, "crf", 20);
    let fps = default_u32(&args, "fps", 30);

    report_progress(0.0, 100.0, "Preparing render...")
        .await
        .ok();

    let resolved_ass_path = if burn_captions && ass_path.is_none() {
        if let Some(srt) = &srt_path {
            if Path::new(srt).exists() {
                report_progress(10.0, 100.0, "Converting subtitles...")
                    .await
                    .ok();
                let entries = parse_srt(srt).map_err(|e| ToolError::Srt(e.to_string()))?;
                let flat: Vec<(f64, f64, String)> = entries
                    .iter()
                    .map(|e| (e.start, e.end, e.text.clone()))
                    .collect();
                let ass_out = Path::new(srt).with_extension("ass");
                let ass_path_str = ass_out.to_string_lossy().into_owned();
                srt_to_ass(&flat, &ass_path_str, "Default")
                    .map_err(|e| ToolError::Ffmpeg(e.to_string()))?;
                Some(ass_path_str)
            } else {
                None
            }
        } else {
            None
        }
    } else {
        ass_path
    };

    let config = RenderConfig {
        video_path: video_path.to_string(),
        edl_path: edl_path.to_string(),
        burn_captions,
        srt_path,
        ass_path: resolved_ass_path,
        overlay_mov: None,
        aspect,
        crf,
        fps,
    };

    report_progress(20.0, 100.0, "Rendering video with FFmpeg...")
        .await
        .ok();

    let output_path = render(config)
        .await
        .map_err(|e| ToolError::Ffmpeg(e.to_string()))?;

    report_progress(100.0, 100.0, "Render complete").await.ok();

    Ok(json!({
        "status": "rendered",
        "output_path": output_path,
    }))
}

pub(crate) async fn handle_reelize(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?
        .to_string_lossy()
        .to_string();
    let srt_path = default_opt_str(&args, "srt_path")
        .map(|s| sanitize_input_path(&s).map(|p| p.to_string_lossy().to_string()))
        .transpose()?;
    let preset = default_str(&args, "preset", "Balanced");
    let max_duration = default_opt_f64(&args, "max_duration");
    let aspect = default_str(&args, "aspect", "9:16");
    let burn_captions = default_bool(&args, "burn_captions", true);

    if !Path::new(&video_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Video not found: {}",
            video_path
        )));
    }

    // Step 1: Transcribe (if no SRT provided)
    let resolved_srt_path = if let Some(srt) = srt_path {
        report_progress(5.0, 100.0, "Using existing SRT...")
            .await
            .ok();
        srt.to_string()
    } else {
        report_progress(0.0, 100.0, "Step 1/4: Transcribing audio...")
            .await
            .ok();
        let transcribe_args = json!({
            "media_path": video_path,
        });
        let transcribe_result = handle_transcribe(transcribe_args).await?;
        report_progress(25.0, 100.0, "Transcription complete")
            .await
            .ok();
        transcribe_result
            .get("output_srt_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::Srt("Transcription did not return output path".to_string()))?
            .to_string()
    };

    // Step 2: SRT prepare (group word-per-line)
    report_progress(30.0, 100.0, "Step 2/4: Grouping captions...")
        .await
        .ok();
    let prepare_args = json!({
        "srt_path": resolved_srt_path,
        "max_words": 10,
        "max_chars": 64,
        "max_gap": 0.6,
    });
    let prepare_result = handle_srt_prepare(prepare_args).await?;
    let grouped_srt = prepare_result
        .get("output_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::Srt("SRT prepare did not return output path".to_string()))?
        .to_string();

    // Step 3: EDL build
    report_progress(50.0, 100.0, "Step 3/4: Building edit decision list...")
        .await
        .ok();
    let crossfade_ms = match preset.as_str() {
        "Tight" => 120,
        "Balanced" => 100,
        "Natural" => 60,
        _ => 100,
    };

    let edl_args = json!({
        "srt_path": grouped_srt,
        "strategy": "keep",
        "max_duration": max_duration,
        "crossfade_ms": crossfade_ms,
        "aspect": aspect,
    });
    let edl_result = handle_edl_build(edl_args).await?;
    let edl_path = edl_result
        .get("edl_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::Srt("EDL build did not return path".to_string()))?
        .to_string();

    // Step 4: Render
    report_progress(70.0, 100.0, "Step 4/4: Rendering final video...")
        .await
        .ok();
    let render_args = json!({
        "video_path": video_path,
        "edl_path": edl_path,
        "srt_path": grouped_srt,
        "burn_captions": burn_captions,
        "aspect": aspect,
        "crf": 20,
        "fps": 30,
    });
    let render_result = handle_render(render_args).await?;
    let output_path = render_result
        .get("output_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::Ffmpeg("Render did not return output path".to_string()))?
        .to_string();

    let total_segments = edl_result
        .get("segments_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let total_duration = edl_result
        .get("total_duration_s")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);

    report_progress(100.0, 100.0, "Reel complete!").await.ok();

    Ok(json!({
        "status": "rendered",
        "output_path": output_path,
        "preset": preset,
        "segments_count": total_segments,
        "total_duration_s": total_duration,
        "srt_path": resolved_srt_path,
        "edl_path": edl_path,
    }))
}

pub(crate) async fn handle_overlay_generate(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let srt_path = extract_str(&args, "srt_path")?;
    let _edl_path = extract_str(&args, "edl_path")?;
    let out_path = default_opt_str(&args, "out_path").unwrap_or_else(|| {
        let p = Path::new(&srt_path);
        let stem = p
            .file_stem()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        format!("{}.overlay.mov", stem)
    });
    let width = default_u32(&args, "width", 1080);
    let height = default_u32(&args, "height", 1920);
    let fps = default_u32(&args, "fps", 30);
    let animate = default_bool(&args, "animate", false);
    let style = default_str(&args, "style", "pupcaps_center");
    let timeline_path = default_opt_str(&args, "timeline_path");

    report_progress(0.0, 100.0, "Generating caption overlay...")
        .await
        .ok();

    let pupcaps_path = "third_party/PupCaps/pupcaps";

    let mut cmd = tokio::process::Command::new("node");
    cmd.arg(pupcaps_path)
        .arg("retimed")
        .arg(srt_path)
        .arg("--output")
        .arg(&out_path)
        .arg("--width")
        .arg(width.to_string())
        .arg("--height")
        .arg(height.to_string())
        .arg("--fps")
        .arg(fps.to_string())
        .arg("--style")
        .arg(format!("mcp/styles/{}.css", style))
        .kill_on_drop(true);

    if animate {
        cmd.arg("--animate");
    }

    let out = cmd.output().await;
    match out {
        Ok(o) if o.status.success() => {
            if let Some(tl_path) = &timeline_path {
                if Path::new(tl_path).exists() {
                    if let Ok(mut timeline) = Timeline::load(tl_path) {
                        timeline.add_asset(
                            "captions",
                            "overlay_mov".to_string(),
                            json!({"path": out_path}),
                        );
                        timeline.save(tl_path).ok();
                    }
                }
            }
            report_progress(100.0, 100.0, "Overlay generated")
                .await
                .ok();
            Ok(json!({
                "status": "generated",
                "output_path": out_path,
            }))
        }
        Ok(o) => Err(ToolError::Ffmpeg(format!(
            "overlay.generate failed: {}",
            String::from_utf8_lossy(&o.stderr)
        ))),
        Err(e) => Err(ToolError::Ffmpeg(format!("overlay.generate error: {}", e))),
    }
}

pub(crate) async fn handle_timeline_build(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let source_video = extract_str(&args, "source_video")?;
    let mut aspect = default_str(&args, "aspect", "9:16");
    let mut fps = default_u32(&args, "fps", 30);
    let max_duration = default_opt_u32(&args, "max_duration");

    // Platform presets: override aspect, fps, max_duration based on target platform
    if let Some(platform) = args.get("platform").and_then(|v| v.as_str()) {
        match platform {
            "tiktok" | "reels" | "shorts" => {
                aspect = "9:16".to_string();
                fps = 30;
            }
            "youtube" | "landscape" => {
                aspect = "16:9".to_string();
                fps = 30;
            }
            "instagram" | "square" => {
                aspect = "1:1".to_string();
                fps = 30;
            }
            _ => {} // unknown platform, keep user defaults
        }
    }
    let output_path = default_opt_str(&args, "output_path")
        .unwrap_or_else(|| default_timeline_path(source_video));

    if !Path::new(source_video).exists() {
        return Err(ToolError::NotFound(format!(
            "Source video not found: {}",
            source_video
        )));
    }

    let timeline = Timeline::new(source_video.into(), &aspect, fps, max_duration);
    timeline.save(&output_path)?;

    Ok(json!({
        "status": "created",
        "timeline_path": output_path,
        "source": source_video,
        "aspect": aspect,
        "fps": fps,
    }))
}

pub(crate) async fn handle_timeline_load(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let timeline_path = sanitize_input_path(extract_str(&args, "timeline_path")?)?
        .to_string_lossy()
        .to_string();
    let timeline = Timeline::load(&timeline_path)?;

    Ok(json!({
        "status": "loaded",
        "timeline_path": timeline_path,
        "version": timeline.version,
        "source": timeline.source.to_string_lossy(),
        "segments_count": timeline.segments.len(),
        "tracks": timeline.tracks.keys().map(|k: &openscript_core::types::TrackType| k.to_string()).collect::<Vec<_>>(),
    }))
}

/// Convert SRT entries into timeline segments in one call.
pub(crate) async fn handle_srt_to_timeline(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let srt_path = sanitize_input_path(extract_str(&args, "srt_path")?)?;
    let source_video = default_opt_str(&args, "source_video");
    let output_path = default_opt_str(&args, "output_path");
    let crossfade_ms = default_u32(&args, "crossfade_ms", 80);
    let aspect = default_str(&args, "aspect", "9:16");
    let fps = default_u32(&args, "fps", 30);
    let scene_size = args.get("scene_size").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
    let max_duration_s: Option<f64> = args.get("max_duration_s").and_then(|v| v.as_f64());
    let min_duration_s: Option<f64> = args.get("min_duration_s").and_then(|v| v.as_f64());

    // Parse SRT file
    let entries = parse_srt(&srt_path)
        .map_err(|e| ToolError::Srt(format!("Failed to parse SRT: {}", e)))?;

    if entries.is_empty() {
        return Err(ToolError::Srt("SRT file has no entries".to_string()));
    }

    // Load or create timeline
    let timeline_path_arg = default_opt_str(&args, "timeline_path");

    let source_path = source_video.as_ref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            // Derive source from SRT path — replace .srt with original media extension
            // Try common extensions: .mp4, .mp3, .wav, .mkv, .webm
            let srt_parent = std::path::Path::new(&srt_path).parent().unwrap_or(std::path::Path::new("."));
            let srt_stem = std::path::Path::new(&srt_path).file_stem().unwrap_or_default();
            for ext in &[".mp4", ".mp3", ".wav", ".mkv", ".webm", ".m4a"] {
                let candidate = srt_parent.join(format!("{}{}", srt_stem.to_string_lossy(), ext));
                if candidate.exists() {
                    return candidate;
                }
            }
            // No media file found — leave empty so timeline.render's audio-only
            // detection can derive source from segments or skip source validation.
            std::path::PathBuf::new()
        });

    let mut timeline = if let Some(ref tp) = timeline_path_arg {
        if !tp.is_empty() && std::path::Path::new(tp).exists() {
            Timeline::load(tp).map_err(|e| ToolError::Timeline(e.to_string()))?
        } else {
            Timeline::new(source_path.clone(), &aspect, fps, None)
        }
    } else {
        Timeline::new(source_path.clone(), &aspect, fps, None)
    };

    // Add SRT entries as segments
    let mut segments_count = 0usize;

    if let Some(max_dur) = max_duration_s {
        // === SENTENCE-AWARE MODE: duration-based grouping ===
        // Uses pause detection (>300ms gaps) and duration caps
        let min_dur = min_duration_s.unwrap_or(2.0);
        // Sentence-aware segmentation parameters (per docs/SEGMENTATION_ARCHITECTURE.md):
        // - 15 words ≈ 4s at 2.5 words/s natural speaking pace
        // - 80 chars ≈ 2 lines of captions at standard font size
        // - 300ms gap = natural breath pause boundary (silence between sentences)
        // - max_dur = user-provided cap (e.g., 5.0s for short-form content)
        let grouped = openscript_core::srt::group_entries_with_words_max_duration(
            &entries,
            15,    // max_words: ~4s at 2.5 words/s
            80,    // max_chars: ~2 caption lines
            0.3,   // max_gap: 300ms = breath pause boundary
            max_dur,
        );

        // Convert GroupedPhrase to timeline segments.
        // enforce_segment_bounds merges segments shorter than min_duration_s
        // into their successor AND splits segments longer than max_duration_s
        // at their longest internal pause. (The old inline merge only handled
        // the min side and could produce a merged segment that exceeded max.)
        let bounded = openscript_core::srt::enforce_segment_bounds(grouped, min_dur, max_dur);

        // Add bounded segments to timeline
        for g in bounded {
            if g.end > g.start {
                timeline.add_segment(g.start, g.end, &g.text, crossfade_ms, None);
                segments_count += 1;
            }
        }
    } else if scene_size <= 1 {
        // === LEGACY MODE: one segment per entry ===
        for entry in &entries {
            if entry.end > entry.start {
                timeline.add_segment(entry.start, entry.end, &entry.text, crossfade_ms, None);
                segments_count += 1;
            }
        }
    } else {
        // === LEGACY MODE: fixed chunk grouping ===
        for chunk in entries.chunks(scene_size) {
            let valid: Vec<_> = chunk.iter().filter(|e| e.end > e.start).collect();
            if valid.is_empty() { continue; }
            let start = valid.first().unwrap().start;
            let end = valid.last().unwrap().end;
            let caption = valid.iter().map(|e| e.text.as_str()).collect::<Vec<_>>().join(" ");
            timeline.add_segment(start, end, &caption, crossfade_ms, None);
            segments_count += 1;
        }
    }

    // Clamp segments at the source media duration. SRT entries occasionally
    // overshoot the audio end (whisper tail hallucination, trailing silence in
    // the word SRT). Without this, the last segments extend past the source and
    // the renderer's overlay chain (eof_action=repeat) holds the final b-roll
    // frame past the audio end — the "audio 2:15 but video 2:41" black+silence
    // tail. The source is the master clock; segments must fit inside it.
    // The clamp is best-effort: if the source is missing or unprobeable we
    // leave segments untouched (the render's `-shortest` still caps output).
    if let Some(src_dur) = probe_source_duration(&source_path).await {
        let before = timeline.segments.len();
        let (dropped, clamped) = clamp_segments_to_duration(&mut timeline.segments, src_dur);
        if dropped > 0 || clamped > 0 {
            tracing::warn!(
                "[srt.to_timeline] clamped {} / dropped {} of {} segments to source duration {:.2}s (trailing SRT overshoot truncated)",
                clamped, dropped, before, src_dur
            );
        }
    }

    // Determine output path: explicit output_path > timeline_path > derived from srt_path
    let resolved_output = output_path
        .filter(|s| !s.is_empty())
        .or_else(|| timeline_path_arg.filter(|s| !s.is_empty()))
        .unwrap_or_else(|| srt_path.with_extension("timeline.json").to_string_lossy().to_string());

    // Save timeline
    timeline.save(&resolved_output)
        .map_err(|e| ToolError::Timeline(format!("Failed to save timeline: {}", e)))?;

    // Report the CLAMPED last-segment end as duration_s — the un-clamped SRT
    // tail (whisper hallucination) would mislead an agent into thinking the
    // timeline runs past the source. source_duration_s is the same value but
    // named to make the master clock explicit.
    let duration_s = timeline.segments.last().map(|s| s.end).unwrap_or(0.0);

    Ok(json!({
        "status": "built",
        "timeline_path": resolved_output,
        "segments_count": segments_count,
        "duration_s": duration_s,
        "aspect": aspect,
        "fps": fps,
        "source_duration_s": duration_s,
    }))
}

pub(crate) async fn handle_timeline_validate(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let timeline_path = sanitize_input_path(extract_str(&args, "timeline_path")?)?
        .to_string_lossy()
        .to_string();
    let timeline = Timeline::load(&timeline_path)?;
    let mut errors = timeline.validate();
    // SEGMENTATION_ARCHITECTURE.md §3: every segment must fall within
    // [MIN_SEGMENT_DURATION_S, MAX_SEGMENT_DURATION_S] (2.0s–6.0s) for
    // short-form retention. Long cuts bleed attention; sub-min cuts flicker.
    errors.extend(timeline.validate_segmentation());
    // DURATION: segments must not extend past the source media (the master
    // clock). SRT tail hallucination / trailing silence produces segments past
    // the audio end; the renderer's overlay repeat then holds the last b-roll
    // frame beyond the audio — the "audio 2:15, video 2:41" black+silence tail.
    // The probe is async, so this lives in the MCP layer (not core's sync
    // validate) — mirroring probe_broll_gaps. Best-effort: skip if the source
    // is missing/unprobeable.
    if let Some(src_dur) = probe_source_duration(&timeline.source).await {
        for seg in &timeline.segments {
            if seg.end > src_dur + SOURCE_DUR_TOLERANCE_S {
                errors.push(format!(
                    "DURATION: segment {} ends at {:.1}s but source media is only {:.1}s — segments must fit inside the source (re-run srt.to_timeline/segment.analyze which clamp at the source duration, or trim this segment)",
                    seg.id, seg.end, src_dur
                ));
            }
        }
    }
    // Phase 54: Reject empty timelines
    if timeline.segments.is_empty() {
        errors.push("Timeline has no segments. Call timeline.add_segment to populate it with segments.".to_string());
    }
    // B-roll coverage: flag segments whose assigned clip is shorter than the
    // segment window. The renderer plays clips exactly once (no loop fill),
    // so these gaps render as a held frame — the agent must re-run keyword
    // generation + broll.fetch for a longer clip.
    let broll_gaps = probe_broll_gaps(&timeline).await;
    for g in &broll_gaps {
        errors.push(format!(
            "BROLL_GAP: segment {} needs {:.1}s but clip {} provides {:.1}s (gap {:.1}s) — {}",
            g.segment_id, g.required_s, g.asset_id, g.available_s, g.gap_s, g.action
        ));
    }
    // B-roll non-repetition: the same clip must not appear on 2+ events —
    // identical footage later in the sequence reads as an error (the
    // b-roll-repeat bug where the deterministic fetch path could place the
    // same Pexels clip on two segments). Dedup happens at TWO levels:
    // 1. exact cache path (same file, same slug)
    // 2. Pexels video id embedded in the cache filename — the same clip can
    //    be cached under DIFFERENT query slugs (e.g.
    //    crowd_people_aavaaz_35340082.mp4 vs crowd_people_yah_35340082.mp4),
    //    which is still the SAME footage and must also be flagged.
    let mut seen_clip_paths: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut seen_clip_ids: std::collections::HashMap<i64, String> = std::collections::HashMap::new();
    for ev in timeline.tracks.get(&TrackType::Broll).cloned().unwrap_or_default() {
        if let Some(p) = timeline
            .assets
            .broll
            .get(&ev.asset_id)
            .and_then(|a| a.get("path"))
            .and_then(|v| v.as_str())
        {
            if let Some(prev) = seen_clip_paths.insert(p.to_string(), ev.id.clone()) {
                errors.push(format!(
                    "BROLL_REPEAT: clip {} is used by both {} and {} — same footage must not repeat later in the sequence (re-run broll.fetch / broll.repair for a distinct clip)",
                    p, prev, ev.id
                ));
            } else if let Some(id) = cache_path_video_id(p) {
                // Path is new — the same Pexels video id under a DIFFERENT
                // query slug is still the same footage (e.g.
                // crowd_people_aavaaz_35340082.mp4 vs
                // crowd_people_yah_35340082.mp4). Only the id check runs here
                // so exact-path duplicates emit exactly one (path) error.
                if let Some(prev) = seen_clip_ids.insert(id, ev.id.clone()) {
                    errors.push(format!(
                        "BROLL_REPEAT: Pexels video {} (used by {} and {}) is the same clip cached under different query slugs — same footage must not repeat later in the sequence (re-run broll.fetch / broll.repair for a distinct clip)",
                        id, prev, ev.id
                    ));
                }
            }
        }
    }
    // ---- V2V alternation checks (docs/V2V_ALTERNATION_ARCHITECTURE.md §3.5) ----
    // When the timeline is in alternate mode, the visual layer is DELIBERATELY
    // partial: "source"-role segments show the ORIGINAL video (no b-roll event),
    // "broll"-role segments must be covered. Validate intent + coverage + breadth.
    let presentation = &timeline.directives.presentation;
    let mut broll_role = 0usize;
    let mut source_role = 0usize;
    if presentation.is_alternate() {
        // Intent: every segment needs a role.
        for seg in &timeline.segments {
            match presentation.role_for(&seg.id) {
                openscript_core::presentation::ROLE_BROLL => broll_role += 1,
                openscript_core::presentation::ROLE_SOURCE => source_role += 1,
                _ => {}
            }
            if !presentation.visual_roles.contains_key(&seg.id) {
                errors.push(format!(
                    "PRESENTATION: segment {} has no visual role in an alternate-mode timeline — run timeline.presentation (or broll.auto with alternation) to re-plan roles",
                    seg.id
                ));
            }
        }
        // Coverage: broll-role segments need a b-roll event covering their window
        // (probe_broll_gaps above catches clips shorter than the window; here we
        // catch a broll-role segment with NO event at all).
        let mut broll_event_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        if let Some(evts) = timeline.tracks.get(&TrackType::Broll) {
            for ev in evts {
                if let Some(seg) = find_segment_for_window(&timeline, &ev.id) {
                    broll_event_ids.insert(seg.id.clone());
                }
            }
        }
        for seg in &timeline.segments {
            if presentation.role_for(&seg.id) == openscript_core::presentation::ROLE_BROLL
                && !broll_event_ids.contains(&seg.id)
            {
                errors.push(format!(
                    "PRESENTATION: segment {} is broll-role but has NO b-roll event — run broll.auto/broll.fetch with alternation enabled to cover it (original video would otherwise show where stock is intended)",
                    seg.id
                ));
            }
            // Source-role segments must NOT have a b-roll event — one would cover
            // the original footage and break the alternation.
            if presentation.role_for(&seg.id) == openscript_core::presentation::ROLE_SOURCE
                && broll_event_ids.contains(&seg.id)
            {
                errors.push(format!(
                    "BROLL_ON_SOURCE: segment {} is source-role (original video shows) but has a b-roll event — remove it or re-plan roles so the alternation stays intact",
                    seg.id
                ));
            }
        }
        // Breadth: an alternation with zero source segments is degenerate (that is
        // full coverage = cover mode); one with zero broll segments means the agent
        // accidentally planned all-source (no stock anywhere).
        if source_role == 0 && !timeline.segments.is_empty() {
            errors.push(
                "PRESENTATION: alternate mode has NO source-role segments — this is full coverage; set mode='cover' or adjust the pattern/broll_ratio to actually alternate".to_string(),
            );
        }
        if broll_role == 0 && !timeline.segments.is_empty() {
            errors.push(
                "PRESENTATION: alternate mode has NO broll-role segments — the visual layer would be pure original footage with no stock; adjust the pattern/broll_ratio (e.g. every_other or broll_ratio 0.5)".to_string(),
            );
        }
    }
    let valid = errors.is_empty();

    Ok(json!({
        "status": if valid { "valid" } else { "invalid" },
        "timeline_path": timeline_path,
        "valid": valid,
        "errors": errors,
        "broll_gaps": broll_gaps,
        "presentation": json!({
            "mode": presentation.mode,
            "pattern": presentation.pattern,
            "every_n": presentation.every_n,
            "source_audio": presentation.source_audio,
            "broll_role_segments": broll_role,
            "source_role_segments": source_role,
        }),
    }))
}

/// Inspect or re-plan the V2V presentation directive on a timeline.
///
/// Query mode: `{timeline_path}` → returns mode + per-segment visual roles.
/// Plan mode: `{timeline_path, mode, pattern, every_n, broll_ratio}` → re-plans
/// roles via presentation::plan_alternation and persists them. mode='cover'
/// clears roles (the default full-coverage behaviour).
pub(crate) async fn handle_timeline_presentation(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let timeline_path = sanitize_input_path(extract_str(&args, "timeline_path")?)?
        .to_string_lossy()
        .to_string();
    let mut timeline = Timeline::load(&timeline_path)?;

    // Query mode when no plan params are provided.
    let mode = args.get("mode").and_then(|v| v.as_str());
    let pattern = args.get("pattern").and_then(|v| v.as_str());
    let every_n = args.get("every_n").and_then(|v| v.as_u64()).map(|v| v as u32);
    let broll_ratio = args.get("broll_ratio").and_then(|v| v.as_f64());

    if mode.is_none() && pattern.is_none() && every_n.is_none() && broll_ratio.is_none() {
        let roles: serde_json::Map<String, serde_json::Value> = timeline
            .segments
            .iter()
            .map(|s| {
                (
                    s.id.clone(),
                    json!(timeline.directives.presentation.role_for(&s.id)),
                )
            })
            .collect();
        return Ok(json!({
            "status": "queried",
            "timeline_path": timeline_path,
            "mode": timeline.directives.presentation.mode,
            "pattern": timeline.directives.presentation.pattern,
            "every_n": timeline.directives.presentation.every_n,
            "visual_roles": roles,
        }));
    }

    // Plan mode.
    if let Some(m) = mode {
        if m == "cover" {
            timeline.directives.presentation.mode = "cover".into();
            timeline.directives.presentation.visual_roles.clear();
            timeline.directives.presentation.pattern =
                openscript_core::presentation::PATTERN_EVERY_OTHER.into();
            timeline.directives.presentation.every_n = 2;
        } else if m == "alternate" {
            timeline.directives.presentation.mode = "alternate".into();
            if let Some(p) = pattern {
                timeline.directives.presentation.pattern = p.to_string();
            }
            if let Some(n) = every_n {
                timeline.directives.presentation.every_n = n;
            }
            let roles = openscript_core::presentation::plan_alternation(
                &timeline.segments,
                &timeline.directives.presentation.pattern,
                timeline.directives.presentation.every_n,
                broll_ratio,
            );
            timeline.directives.presentation.visual_roles = roles;
        } else {
            return Err(ToolError::InvalidArg(format!(
                "mode must be 'cover' or 'alternate', got '{}'",
                m
            )));
        }
    } else if pattern.is_some() || every_n.is_some() || broll_ratio.is_some() {
        // Re-plan within the current mode (must be alternate).
        if !timeline.directives.presentation.is_alternate() {
            return Err(ToolError::InvalidArg(
                "cannot re-plan roles while mode='cover' — pass mode='alternate' first".into(),
            ));
        }
        if let Some(p) = pattern {
            timeline.directives.presentation.pattern = p.to_string();
        }
        if let Some(n) = every_n {
            timeline.directives.presentation.every_n = n;
        }
        let roles = openscript_core::presentation::plan_alternation(
            &timeline.segments,
            &timeline.directives.presentation.pattern,
            timeline.directives.presentation.every_n,
            broll_ratio,
        );
        timeline.directives.presentation.visual_roles = roles;
    }

    timeline.updated_at = chrono::Utc::now();
    timeline.save(&timeline_path)?;

    let roles: serde_json::Map<String, serde_json::Value> = timeline
        .segments
        .iter()
        .map(|s| {
            (
                s.id.clone(),
                json!(timeline.directives.presentation.role_for(&s.id)),
            )
        })
        .collect();
    // Re-voice (source_audio 'duck') is EXCLUDED from V2V by decision — the
    // original video's audio is always preserved as-is (genuine output). See
    // docs/V2V_ALTERNATION_ARCHITECTURE.md §3.6 for rationale + revisit path.
    // The schema field is retained for backward compatibility and is "keep".
    let mut resp = serde_json::Map::new();
    resp.insert("status".into(), json!("planned"));
    resp.insert("timeline_path".into(), json!(timeline_path));
    resp.insert("mode".into(), json!(timeline.directives.presentation.mode));
    resp.insert("pattern".into(), json!(timeline.directives.presentation.pattern));
    resp.insert("every_n".into(), json!(timeline.directives.presentation.every_n));
    resp.insert("source_audio".into(), json!("keep"));
    resp.insert("visual_roles".into(), json!(roles));
    Ok(serde_json::Value::Object(resp))
}

pub(crate) async fn handle_timeline_upgrade(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let edl_v1_path = sanitize_input_path(extract_str(&args, "edl_v1_path")?)?
        .to_string_lossy()
        .to_string();
    let output_path = default_opt_str(&args, "output_path");

    let data = std::fs::read_to_string(&edl_v1_path)?;
    let v1: serde_json::Value = serde_json::from_str(&data)?;
    let timeline = Timeline::from_edl_v1(&v1)?;

    let out_path = output_path.unwrap_or_else(|| {
        let p = Path::new(&edl_v1_path);
        let stem = p
            .file_stem()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        format!("{}.timeline.json", stem)
    });

    timeline.save(&out_path)?;

    Ok(json!({
        "status": "upgraded",
        "timeline_path": out_path,
        "segments_count": timeline.segments.len(),
    }))
}

pub(crate) async fn handle_timeline_add_segment(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let timeline_path = extract_str(&args, "timeline_path")?;
    let start = extract_f64(&args, "start")?;
    let end = extract_f64(&args, "end")?;
    let caption = extract_str(&args, "caption")?;
    let crossfade_ms = default_u32(&args, "crossfade_ms", 80);
    let semantic_role = default_opt_str(&args, "semantic_role");

    let mut timeline = Timeline::load(timeline_path)?;
    let segment_id =
        timeline.add_segment(start, end, caption, crossfade_ms, semantic_role.as_deref());
    timeline.save(timeline_path)?;

    Ok(json!({
        "status": "segment_added",
        "segment_id": segment_id,
        "timeline_path": timeline_path,
    }))
}

pub(crate) async fn handle_timeline_add_track_event(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let timeline_path = extract_str(&args, "timeline_path")?;
    let track_type_str = extract_str(&args, "track_type")?;
    let event = args
        .get("event")
        .ok_or_else(|| ToolError::MissingArg("event".to_string()))?
        .clone();

    let track_type: TrackType = track_type_str.parse().map_err(ToolError::Timeline)?;

    let mut timeline = Timeline::load(timeline_path)?;

    let event_obj: openscript_core::timeline::TimelineEvent =
        serde_json::from_value(event.clone()).map_err(ToolError::Json)?;

    timeline.add_track_event(track_type, event_obj);
    timeline.save(timeline_path)?;

    let event_id = event
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    Ok(json!({
        "status": "event_added",
        "event_id": event_id,
        "track_type": track_type_str,
        "timeline_path": timeline_path,
    }))
}

pub(crate) async fn handle_timeline_diff(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let timeline_path_a = extract_str(&args, "timeline_path_a")?;
    let timeline_path_b = extract_str(&args, "timeline_path_b")?;

    let a = Timeline::load(timeline_path_a)?;
    let b = Timeline::load(timeline_path_b)?;

    let duration_a = a.total_duration_ms();
    let duration_b = b.total_duration_ms();
    let duration_change_ms = duration_b - duration_a;

    let seg_ids_a: std::collections::HashSet<&str> =
        a.segments.iter().map(|s| s.id.as_str()).collect();
    let seg_ids_b: std::collections::HashSet<&str> =
        b.segments.iter().map(|s| s.id.as_str()).collect();

    let added: Vec<&str> = {
        let mut v: Vec<&str> = seg_ids_b.difference(&seg_ids_a).copied().collect();
        v.sort();
        v
    };
    let removed: Vec<&str> = {
        let mut v: Vec<&str> = seg_ids_a.difference(&seg_ids_b).copied().collect();
        v.sort();
        v
    };

    let mut modified = Vec::new();
    for seg_a in &a.segments {
        if seg_ids_b.contains(seg_a.id.as_str()) {
            if let Some(seg_b) = b.segments.iter().find(|s| s.id == seg_a.id) {
                if seg_a.start != seg_b.start
                    || seg_a.end != seg_b.end
                    || seg_a.caption != seg_b.caption
                {
                    modified.push(seg_a.id.as_str());
                }
            }
        }
    }
    // P2-4 fix: sort modified segment ids for stable, readable output. Prior
    // versions returned them in arbitrary iteration order.
    modified.sort();

    let track_changes = json!({
        "dialogue": {
            "a": track_count(&a, &TrackType::Dialogue),
            "b": track_count(&b, &TrackType::Dialogue),
        },
        "voiceover": {
            "a": track_count(&a, &TrackType::Voiceover),
            "b": track_count(&b, &TrackType::Voiceover),
        },
        "broll": {
            "a": track_count(&a, &TrackType::Broll),
            "b": track_count(&b, &TrackType::Broll),
        },
        "music": {
            "a": track_count(&a, &TrackType::Music),
            "b": track_count(&b, &TrackType::Music),
        },
        "sfx": {
            "a": track_count(&a, &TrackType::Sfx),
            "b": track_count(&b, &TrackType::Sfx),
        },
    });

    Ok(json!({
        "status": "success",
        "duration_change_ms": duration_change_ms,
        "segments": {
            "added": added,
            "removed": removed,
            "modified": modified,
        },
        "tracks": track_changes,
    }))
}

pub(crate) async fn handle_timeline_preview(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let timeline_path = sanitize_input_path(extract_str(&args, "timeline_path")?)?
        .to_string_lossy()
        .to_string();
    let timeline = Timeline::load(&timeline_path)?;

    let total_duration_ms = timeline.total_duration_ms();
    let segments_info: Vec<serde_json::Value> = timeline
        .segments
        .iter()
        .map(|s| {
            // P2-1 fix: append ellipsis when the caption is truncated, so agents
            // can tell the preview is abbreviated. Prior versions silently cut at
            // 60 chars with no indication.
            let caption_display = if s.caption.chars().count() > 60 {
                format!("{}...", s.caption.chars().take(57).collect::<String>())
            } else {
                s.caption.clone()
            };
            json!({
                "id": s.id,
                "start": s.start,
                "end": s.end,
                "caption": caption_display,
                "crossfade_ms": s.crossfade_ms,
            })
        })
        .collect();

    let tracks_info: serde_json::Map<String, serde_json::Value> = timeline
        .tracks
        .iter()
        .map(|(track, events)| {
            let track = track as &TrackType;
            let events = events as &Vec<openscript_core::timeline::TimelineEvent>;
            (
                track.to_string(),
                json!({
                    "count": events.len(),
                    "total_duration_ms": events.iter().map(|e| e.end_ms - e.start_ms).sum::<i64>(),
                }),
            )
        })
        .collect();

    let mut errors = timeline.validate();
    // Phase 54: Reject empty timelines
    if timeline.segments.is_empty() {
        errors.push("Timeline has no segments. Call timeline.add_segment to populate it.".to_string());
    }
    // Segmentation bounds (SEGMENTATION_ARCHITECTURE.md) — same as validate.
    errors.extend(timeline.validate_segmentation());
    // B-roll coverage gaps (async probe) — same as validate, so preview is the
    // single-call viewer an agent uses to reason about the whole operation.
    let broll_gaps = probe_broll_gaps(&timeline).await;
    for g in &broll_gaps {
        errors.push(format!(
            "BROLL_GAP: segment {} needs {:.1}s but clip {} provides {:.1}s (gap {:.1}s) — {}",
            g.segment_id, g.required_s, g.asset_id, g.available_s, g.gap_s, g.action
        ));
    }
    let render_ready = errors.is_empty() && !timeline.segments.is_empty();

    // Phase 136: the composition layer stack (bottom→top, with per-event
    // concept/asset/timing) + used-clip ids — the timeline-viewer context
    // that lets an agent see the full operational flow in one call.
    let viewer = build_timeline_viewer_context(&timeline);
    let used_ids: Vec<i64> = {
        let mut ids: Vec<i64> = used_broll_video_ids(&timeline).into_iter().collect();
        ids.sort_unstable();
        ids
    };

    Ok(json!({
        "status": "success",
        "timeline_path": timeline_path,
        "version": timeline.version,
        "total_duration_ms": total_duration_ms,
        "segments_count": timeline.segments.len(),
        "segments": segments_info,
        "tracks": tracks_info,
        "composition": viewer,
        "broll_gaps": broll_gaps,
        "used_broll_video_ids": used_ids,
        "render_ready": render_ready,
        "validation_errors": errors,
    }))
}

pub(crate) async fn handle_timeline_render(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_ffmpeg::render::render_from_timeline;

    let timeline_path = extract_str(&args, "timeline_path")?;
    let source_video = default_opt_str(&args, "source_video");
    let output_path = default_opt_str(&args, "output_path");
    let crf = default_opt_u32(&args, "crf");

    if !Path::new(&timeline_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Timeline not found: {}",
            timeline_path
        )));
    }

    let mut timeline = Timeline::load(timeline_path)?;

    // If caller provides source_video, override the timeline's source before validation.
    // This allows rendering when srt.to_timeline didn't set the source field.
    let source_provided = source_video.is_some();
    if let Some(ref sv) = source_video {
        timeline.source = std::path::PathBuf::from(sv);
    }

    let mut errors = timeline.validate();
    // When source_video is provided, ignore 'Source video path is required'
    // since the override above already handled it.
    if source_provided {
        errors.retain(|e| e != "Source video path is required");
    }
    // Also skip overlap validation — tools like sfx.auto_assign and broll.fetch
    // may add track events that create apparent overlaps in segment metadata.
    // The render pipeline handles overlapping segments gracefully.
    errors.retain(|e| !e.contains("overlaps with previous segment"));
    if !errors.is_empty() {
        return Err(ToolError::Timeline(format!(
            "Timeline has {} validation error(s): {:?}",
            errors.len(),
            errors
        )));
    }

    let source = source_video.unwrap_or_else(|| timeline.source.to_string_lossy().to_string());
    if !Path::new(&source).exists() {
        return Err(ToolError::NotFound(format!(
            "Source video not found: {}",
            source
        )));
    }

    // Auto-detect audio-only source and generate a black background video.
    // This enables A2V (audio-to-video) pipeline: audio → timeline → render.
    let source_is_video = {
        let probe = std::process::Command::new("ffprobe")
            .args(["-v", "quiet", "-select_streams", "v:0", "-show_entries", "stream=codec_type", "-of", "csv=p=0", &source])
            .output();
        match probe {
            Ok(o) => o.status.success() && !o.stdout.is_empty(),
            Err(_) => false, // assume audio-only if ffprobe fails (safer)
        }
    };
    let render_source = if !source_is_video {
        // Derive duration from timeline segments instead of hardcoded fallback
        let duration = {
            let d = std::process::Command::new("ffprobe")
                .args(["-v", "quiet", "-show_entries", "format=duration", "-of", "csv=p=0", &source])
                .output();
            match d {
                Ok(o) => String::from_utf8_lossy(&o.stdout)
                    .trim()
                    .parse::<f64>()
                    .ok(),
                Err(_) => None,
            }
        }
        .unwrap_or_else(|| {
            // Fallback: compute from timeline segment boundaries
            timeline.segments.iter()
                .map(|s| s.end)
                .fold(0.0f64, f64::max)
                .max(1.0) // guard against empty segments (avoid 0-second video)
        });

        // Derive dimensions from timeline's aspect ratio + resolve_width/resolve_height
        let w = timeline.target.resolve_width();
        let h = timeline.target.resolve_height();
        let bg_video = {
            let p = std::path::Path::new(&source);
            let stem = p.file_stem().unwrap_or_default().to_string_lossy().to_string();
            let parent = p.parent().unwrap_or(std::path::Path::new("."));
            parent.join(format!("{}.bg.mp4", stem)).to_string_lossy().to_string()
        };
        tracing::info!("[timeline.render] Audio-only source detected ({}x{}). Generating black background: {}", w, h, bg_video);
        report_progress(10.0, 100.0, "Generating black background video from audio...").await.ok();
        let bg_result = std::process::Command::new("ffmpeg")
            .args([
                "-y", "-f", "lavfi",
                "-i", &format!("color=c=black:s={}x{}:d={:.1}:r={}", w, h, duration, timeline.target.fps),
                "-i", &source,
                "-c:v", "libx264", "-tune", "stillimage",
                "-c:a", "aac", "-b:a", "192k",
                "-shortest",
                &bg_video,
            ])
            .output();
        match bg_result {
            Ok(o) if o.status.success() => {
                tracing::info!("[timeline.render] Background video generated: {}", bg_video);
                bg_video
            }
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                let last_lines: Vec<&str> = stderr.lines().rev().take(5).collect();
                return Err(ToolError::Ffmpeg(format!(
                    "Failed to generate background video from audio: {}",
                    last_lines.join("\n")
                )));
            }
            Err(e) => {
                return Err(ToolError::Ffmpeg(format!(
                    "Failed to run ffmpeg for background generation: {}", e
                )));
            }
        }
    } else {
        source.clone()
    };

    let total_tracks = timeline.tracks.values().map(|v| v.len()).sum::<usize>();
    report_progress(
        0.0,
        100.0,
        &format!(
            "Rendering timeline ({} segments, {} track events)...",
            timeline.segments.len(),
            total_tracks
        ),
    )
    .await
    .ok();

    report_progress(20.0, 100.0, "Building filter graph...")
        .await
        .ok();

    // Filter out placeholder b-roll events before rendering to prevent FFmpeg crash
    if let Some(broll_events) = timeline.tracks.get_mut(&TrackType::Broll) {
        let before = broll_events.len();
        broll_events.retain(|e| e.asset_id != "placeholder" && !e.asset_id.is_empty());
        let removed = before - broll_events.len();
        if removed > 0 {
            tracing::warn!(
                "[timeline.render] Filtered {} placeholder b-roll events",
                removed
            );
        }
    }

    let result = render_from_timeline(&timeline, &render_source, output_path.as_deref(), crf).await;

    // Cleanup generated background video regardless of render outcome
    let _cleanup_bg = (!source_is_video).then(|| {
        let _ = std::fs::remove_file(&render_source);
    });

    match result {
        Ok(out_path) => {
            report_progress(100.0, 100.0, "Render complete").await.ok();
            let file_size = std::fs::metadata(&out_path).map(|m| m.len()).unwrap_or(0);
            Ok(json!({
                "status": "rendered",
                "output_path": out_path,
                "file_size_bytes": file_size,
                "segments_count": timeline.segments.len(),
                "overlays_rendered": total_tracks,
            }))
        }
        Err(e) => {
            // P0-2 fix: include the ffmpeg error inline (and a tail of the render
            // log when one exists) so AI agents can self-correct without having
            // to read a separate log file. Prior versions returned only
            // "Render failed, see log: /path/to/render.log" which gave agents
            // no actionable information.
            let err_str = e.to_string();
            let log_excerpt = if let Some(log_path) = err_str
                .strip_prefix("Render failed, see log: ")
                .or_else(|| err_str.strip_prefix("Render failed: "))
            {
                std::fs::read_to_string(log_path).ok().map(|content| {
                    let lines: Vec<&str> = content.lines().collect();
                    let last_20: Vec<&str> = lines.iter().rev().take(20).rev().cloned().collect();
                    last_20.join("\n")
                })
            } else {
                None
            };
            let mut msg = format!("Render failed: {}", err_str);
            if let Some(excerpt) = log_excerpt {
                if !excerpt.is_empty() {
                    msg.push_str("\n\n--- render log (last 20 lines) ---\n");
                    msg.push_str(&excerpt);
                }
            }
            Err(ToolError::Ffmpeg(msg))
        }
    }
}

pub(crate) async fn handle_reelize_brief(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?
        .to_string_lossy()
        .to_string();
    let srt_path_opt = default_opt_str(&args, "srt_path")
        .map(|s| sanitize_input_path(&s).map(|p| p.to_string_lossy().to_string()))
        .transpose()?;

    if !Path::new(&video_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Video not found: {}",
            video_path
        )));
    }

    let resolved_srt_path = if let Some(srt) = srt_path_opt {
        report_progress(5.0, 100.0, "Using existing SRT...")
            .await
            .ok();
        srt
    } else {
        report_progress(0.0, 100.0, "Transcribing audio...")
            .await
            .ok();
        let transcribe_result = handle_transcribe(json!({"media_path": video_path})).await?;
        report_progress(30.0, 100.0, "Transcription complete")
            .await
            .ok();
        transcribe_result
            .get("output_srt_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::Srt("Transcription did not return output path".to_string()))?
            .to_string()
    };

    report_progress(35.0, 100.0, "Grouping caption segments...")
        .await
        .ok();
    let prepare_result = handle_srt_prepare(json!({
        "srt_path": &resolved_srt_path,
        "max_words": 10,
        "max_chars": 64,
        "max_gap": 0.6,
    }))
    .await?;
    let grouped_srt_path = prepare_result
        .get("output_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::Srt("SRT prepare did not return output path".to_string()))?
        .to_string();

    report_progress(50.0, 100.0, "Analyzing segments...")
        .await
        .ok();
    let entries = parse_srt(&grouped_srt_path)?;

    const STOPWORDS: &[&str] = &[
        "the", "a", "an", "is", "are", "was", "were", "be", "been", "being", "have", "has", "had",
        "do", "does", "did", "will", "would", "could", "should", "may", "might", "can", "shall",
        "to", "of", "in", "for", "on", "with", "at", "by", "from", "as", "into", "like", "through",
        "after", "over", "between", "out", "against", "during", "without", "before", "under",
        "around", "among", "that", "this", "these", "those", "it", "its", "i", "me", "my", "we",
        "our", "you", "your", "he", "him", "his", "she", "her", "they", "them", "their", "what",
        "which", "who", "whom", "whose", "where", "when", "why", "how", "not", "no", "nor", "so",
        "but", "and", "or", "if", "then", "than", "too", "very", "just", "about", "up", "some",
    ];

    let extract_keywords = |text: &str, limit: usize| -> Vec<String> {
        text.split_whitespace()
            .filter(|w| {
                let lower = w.to_lowercase();
                let cleaned: String = lower.chars().filter(|c| c.is_alphabetic()).collect();
                !cleaned.is_empty() && !STOPWORDS.contains(&cleaned.as_str())
            })
            .map(|w| {
                let lower = w.to_lowercase();
                lower.chars().filter(|c| c.is_alphabetic()).collect()
            })
            .take(limit)
            .collect()
    };

    let mut segments: Vec<serde_json::Value> = Vec::new();
    let mut total_dialogue_s = 0.0;

    for (i, entry) in entries.iter().enumerate() {
        let duration_s = entry.end - entry.start;
        let word_count = entry.text.split_whitespace().count();
        let wps = if duration_s > 0.0 {
            word_count as f64 / duration_s
        } else {
            0.0
        };

        let keywords = extract_keywords(&entry.text, 5);
        let broll_concepts: Vec<String> = if entry.text.len() < 20 && !entry.text.trim().is_empty()
        {
            let mut concepts = keywords.iter().take(3).cloned().collect::<Vec<_>>();
            concepts.push(entry.text.trim().to_string());
            concepts
        } else {
            keywords.iter().take(3).cloned().collect()
        };

        total_dialogue_s += duration_s;

        segments.push(json!({
            "id": format!("seg_{:03}", i + 1),
            "start_s": entry.start,
            "end_s": entry.end,
            "duration_s": duration_s,
            "text": entry.text,
            "word_count": word_count,
            "words_per_second": (wps * 100.0).round() / 100.0,
            "suggested_broll_concepts": broll_concepts,
            "topic_keywords": keywords,
        }));
    }

    let mut topic_map: std::collections::HashMap<String, (usize, f64)> =
        std::collections::HashMap::new();
    for seg in &segments {
        if let Some(keywords) = seg.get("topic_keywords").and_then(|v| v.as_array()) {
            if let Some(first) = keywords.first().and_then(|v| v.as_str()) {
                let topic = first.to_string();
                let entry = topic_map.entry(topic).or_insert((0, 0.0));
                entry.0 += 1;
                entry.1 += seg
                    .get("duration_s")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
            }
        }
    }

    let topic_summary: Vec<serde_json::Value> = topic_map
        .into_iter()
        .map(|(topic, (count, total_s))| {
            json!({
                "topic": topic,
                "segment_count": count,
                "total_s": (total_s * 100.0).round() / 100.0,
            })
        })
        .collect();

    let source_duration_s = match tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "json",
        ])
        .arg(&video_path)
        .output()
        .await
    {
        Ok(output) => {
            if let Ok(probe) = serde_json::from_slice::<serde_json::Value>(&output.stdout) {
                probe
                    .get("format")
                    .and_then(|f| f.get("duration"))
                    .and_then(|v| v.as_str())
                    .and_then(|v| v.parse::<f64>().ok())
                    .unwrap_or(0.0)
            } else {
                0.0
            }
        }
        Err(e) => {
            tracing::warn!("ffprobe failed for source duration: {}", e);
            0.0
        }
    };

    report_progress(100.0, 100.0, "Brief complete").await.ok();

    Ok(json!({
        "source_path": video_path,
        "source_duration_s": (source_duration_s * 100.0).round() / 100.0,
        "total_segments": segments.len(),
        "total_dialogue_s": (total_dialogue_s * 100.0).round() / 100.0,
        "segments": segments,
        "topic_summary": topic_summary,
    }))
}

pub(crate) async fn handle_reelize_direct(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_ffmpeg::render::render_from_timeline;
    use openscript_ffmpeg::subtitles;

    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?
        .to_string_lossy()
        .to_string();
    let segments_arr = args
        .get("segments")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ToolError::MissingArg("segments".to_string()))?;
    let aspect = default_str(&args, "aspect", "9:16");
    let crossfade_ms = default_u32(&args, "crossfade_ms", 300);
    let fps = default_u32(&args, "fps", 30);
    let crf = default_u32(&args, "crf", 20);
    let output_path = default_opt_str(&args, "output_path");
    let captions_obj = args.get("captions").cloned().unwrap_or(json!({}));
    let caption_style = default_str(&captions_obj, "style", "standard");
    let captions_enabled = default_bool(&captions_obj, "enabled", true);
    let broll_arr = args
        .get("broll")
        .and_then(|v| v.as_array()).cloned()
        .unwrap_or_default();
    let sfx_arr = args
        .get("sfx")
        .and_then(|v| v.as_array()).cloned()
        .unwrap_or_default();
    let music_obj = args.get("music").cloned();
    let voiceover_arr = args
        .get("voiceover")
        .and_then(|v| v.as_array()).cloned()
        .unwrap_or_default();
    let srt_path_opt = default_opt_str(&args, "srt_path")
        .map(|s| sanitize_input_path(&s).map(|p| p.to_string_lossy().to_string()))
        .transpose()?;

    if !Path::new(&video_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Video not found: {}",
            video_path
        )));
    }

    let mut warnings: Vec<String> = Vec::new();

    report_progress(0.0, 100.0, "Transcribing audio...")
        .await
        .ok();
    let resolved_srt_path = if let Some(srt) = srt_path_opt {
        srt
    } else {
        let transcribe_result = handle_transcribe(json!({"media_path": video_path})).await?;
        transcribe_result
            .get("output_srt_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::Srt("Transcription did not return output path".to_string()))?
            .to_string()
    };

    report_progress(15.0, 100.0, "Preparing grouped SRT...")
        .await
        .ok();
    let prepare_result = handle_srt_prepare(json!({
        "srt_path": &resolved_srt_path,
        "max_words": 10,
        "max_chars": 64,
        "max_gap": 0.6,
    }))
    .await?;
    let _grouped_srt_path = prepare_result
        .get("output_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::Srt("SRT prepare did not return output path".to_string()))?
        .to_string();

    report_progress(25.0, 100.0, "Building timeline...")
        .await
        .ok();
    let timeline_path = default_timeline_path(&video_path);
    let mut timeline = Timeline::new(
        std::path::Path::new(&video_path).to_path_buf(),
        &aspect,
        fps,
        None,
    );

    for segment in segments_arr {
        let start = segment.get("start").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let end = segment.get("end").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let caption = segment
            .get("caption")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let seg_crossfade = segment
            .get("crossfade_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(crossfade_ms as u64) as u32;
        let semantic_role = segment.get("id").and_then(|v| v.as_str());

        timeline.add_segment(start, end, caption, seg_crossfade, semantic_role);
    }

    if captions_enabled {
        use openscript_core::srt::parse_srt;

        let word_srt_path = {
            let p = Path::new(&resolved_srt_path);
            let parent = p.parent().unwrap_or(Path::new("."));
            let stem = p
                .file_stem()
                .map(|s| s.to_string_lossy())
                .unwrap_or_default();
            parent
                .join(format!("{}.apex.word.srt", stem))
                .to_string_lossy()
                .to_string()
        };

        let word_entries = parse_srt(&word_srt_path).ok();
        let raw_srt_entries = parse_srt(&resolved_srt_path)
            .map_err(|e| ToolError::Srt(format!("Failed to parse SRT: {}", e)))?;

        let use_concat = segments_arr.len() > 10;
        let mut timeline_segments: Vec<(f64, f64, String)> = Vec::new();
        let mut output_cursor_s: f64 = 0.0;

        for segment in segments_arr {
            let seg_start = segment.get("start").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let seg_end = segment.get("end").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let caption = segment
                .get("caption")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let seg_crossfade_s = segment
                .get("crossfade_ms")
                .and_then(|v| v.as_f64())
                .unwrap_or(crossfade_ms as f64)
                / 1000.0;
            let seg_duration = seg_end - seg_start;

            if let Some(ref words) = word_entries {
                let words_in_range: Vec<_> = words
                    .iter()
                    .filter(|e| {
                        e.start >= seg_start && e.end <= seg_end + 0.05 && !e.text.trim().is_empty()
                    })
                    .collect();

                let mut i = 0;
                while i < words_in_range.len() {
                    let chunk_size = if words_in_range.len() - i >= 5 {
                        3
                    } else if words_in_range.len() - i == 4 {
                        2
                    } else {
                        words_in_range.len() - i
                    };
                    let chunk_start = output_cursor_s + (words_in_range[i].start - seg_start);
                    let chunk_end =
                        output_cursor_s + (words_in_range[i + chunk_size - 1].end - seg_start);
                    let text: Vec<_> = words_in_range[i..i + chunk_size]
                        .iter()
                        .map(|e| e.text.trim().to_string())
                        .collect();
                    timeline_segments.push((chunk_start, chunk_end, text.join(" ")));
                    i += chunk_size;
                }
            } else {
                let srt_in_range: Vec<_> = raw_srt_entries
                    .iter()
                    .filter(|e| {
                        e.start >= seg_start && e.end <= seg_end + 0.05 && !e.text.trim().is_empty()
                    })
                    .collect();

                if !srt_in_range.is_empty() && !caption.is_empty() {
                    let caption_words: Vec<&str> = caption.split_whitespace().collect();
                    let n = srt_in_range.len();
                    for (i, srt_entry) in srt_in_range.iter().enumerate() {
                        let out_start = output_cursor_s + (srt_entry.start - seg_start);
                        let out_end = output_cursor_s + (srt_entry.end - seg_start);
                        let ws = (i * caption_words.len()) / n;
                        let we = ((i + 1) * caption_words.len()) / n;
                        let chunk = caption_words[ws..we].join(" ");
                        if !chunk.is_empty() {
                            timeline_segments.push((out_start, out_end, chunk));
                        }
                    }
                } else if !caption.is_empty() {
                    timeline_segments.push((
                        output_cursor_s,
                        output_cursor_s + seg_duration,
                        caption.to_string(),
                    ));
                }
            }

            if use_concat {
                output_cursor_s += seg_duration;
            } else {
                output_cursor_s += seg_duration - seg_crossfade_s;
                if output_cursor_s < 0.0 {
                    output_cursor_s = 0.0;
                }
            }
        }

        let caption_asset_dir = Path::new(&timeline_path).parent().unwrap_or(Path::new("."));
        let style_name = if caption_style == "kinetic" {
            "KineticViral"
        } else {
            "Standard"
        };

        let ass_path = caption_asset_dir
            .join(format!("captions_{}.ass", style_name.to_lowercase()))
            .to_string_lossy()
            .to_string();

        if caption_style == "kinetic" {
            subtitles::generate_kinetic_captions(
                &timeline_segments,
                &ass_path,
                style_name,
                "&H00FFD700",
            )
            .map_err(|e| ToolError::Srt(e.to_string()))?;
        } else {
            subtitles::srt_to_ass(&timeline_segments, &ass_path, style_name)
                .map_err(|e| ToolError::Srt(e.to_string()))?;
        }

        timeline.add_asset("captions", "ass".to_string(), json!({"path": ass_path}));
    }

    // Save timeline BEFORE calling sub-tools (they load from disk)
    timeline.save(&timeline_path)?;

    report_progress(40.0, 100.0, "Fetching b-roll...")
        .await
        .ok();
    for directive in &broll_arr {
        let concept = directive
            .get("concept")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let overlay_at_s = directive
            .get("overlay_at_s")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let duration_s = directive
            .get("duration_s")
            .and_then(|v| v.as_f64())
            .unwrap_or(3.0);

        let fetch_result = handle_broll_fetch(json!({
            "concepts": [concept],
            "orientation": "9:16",
            "quality": "sd",
            "download": true,
        }))
        .await;

        match fetch_result {
            Ok(result) => {
                let cached_path = result
                    .get("downloaded")
                    .and_then(|d| d.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|v| v.get("path"))
                    .and_then(|v| v.as_str())
                    .map(String::from);

                if let Some(path) = cached_path {
                    let assign_result = handle_broll_assign(json!({
                        "timeline_path": &timeline_path,
                        "concept": concept,
                        "position_ms": (overlay_at_s * 1000.0) as i64,
                        "duration_ms": (duration_s * 1000.0) as i64,
                        "asset_path": path,
                    }))
                    .await;
                    if let Err(e) = assign_result {
                        warnings.push(format!("broll assign failed for '{}': {}", concept, e));
                    }
                } else {
                    warnings.push(format!(
                        "broll fetch found no downloadable asset for '{}'",
                        concept
                    ));
                }
            }
            Err(e) => {
                warnings.push(format!("broll fetch failed for '{}': {}", concept, e));
            }
        }
    }

    report_progress(55.0, 100.0, "Assigning SFX...").await.ok();

    for directive in &sfx_arr {
        let role = directive
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("transition");
        let at_s = directive
            .get("at_s")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        let assign_result = handle_sfx_assign(json!({
            "timeline_path": &timeline_path,
            "editorial_role": role,
            "query": "",
            "position_ms": (at_s * 1000.0) as i64,
        }))
        .await;
        if let Err(e) = assign_result {
            warnings.push(format!("sfx assign failed for role '{}': {}", role, e));
        }
    }

    if let Some(ref music) = music_obj {
        report_progress(65.0, 100.0, "Assigning music...")
            .await
            .ok();
        let mood = default_str(music, "mood", "neutral");
        let energy = default_str(music, "energy", "medium");
        let gain_db = default_f64(music, "gain_db", -12.0);
        let ducking = default_bool(music, "duck_under_dialogue", true);

        // Search for a matching music track, then pass its path
        let music_path = match handle_library_search(json!({
            "mood": mood,
            "energy": energy,
            "limit": 1,
        }))
        .await
        {
            Ok(r) => r
                .get("results")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|first| first.get("path"))
                .and_then(|p| p.as_str())
                .map(|s| s.to_string()),
            Err(_) => None,
        };

        if let Some(path) = music_path {
            let music_result = handle_music_assign(json!({
                "timeline_path": &timeline_path,
                "path": path,
                "mood": mood,
                "energy": energy,
                "gain_db": gain_db,
                "ducking": ducking,
            }))
            .await;
            if let Err(e) = music_result {
                warnings.push(format!("music assign failed: {}", e));
            }
        } else {
            warnings.push("No music track found in index — skipping music assignment".to_string());
        }
    }

    if !voiceover_arr.is_empty() {
        report_progress(75.0, 100.0, "Generating voiceovers...")
            .await
            .ok();
    }
    for directive in &voiceover_arr {
        let text = directive.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let position_s = directive
            .get("position_s")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let profile_id = directive
            .get("voice_profile_id")
            .and_then(|v| v.as_str())
            .unwrap_or("test_narrator");
        let speed = directive
            .get("speed")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0);
        let gain_db = directive
            .get("gain_db")
            .and_then(|v| v.as_f64())
            .unwrap_or(-6.0);

        let vo_result = handle_voiceover_generate(json!({
            "timeline_path": &timeline_path,
            "text": text,
            "voice_profile_id": profile_id,
            "position_ms": (position_s * 1000.0) as i64,
            "speed": speed,
            "gain_db": gain_db,
        }))
        .await;
        if let Err(e) = vo_result {
            warnings.push(format!("voiceover generate failed: {}", e));
        }
    }

    report_progress(85.0, 100.0, "Validating timeline...")
        .await
        .ok();
    let timeline = Timeline::load(&timeline_path)?;
    let validation_errors = timeline.validate();
    if !validation_errors.is_empty() {
        return Err(ToolError::Timeline(format!(
            "Timeline validation failed: {}",
            validation_errors.join("; ")
        )));
    }

    report_progress(90.0, 100.0, "Rendering final video...")
        .await
        .ok();
    let output = render_from_timeline(&timeline, &video_path, output_path.as_deref(), Some(crf))
        .await
        .map_err(|e| ToolError::Ffmpeg(e.to_string()))?;

    let broll_count = track_count(&timeline, &TrackType::Broll);
    let sfx_count = track_count(&timeline, &TrackType::Sfx);
    let music_count = track_count(&timeline, &TrackType::Music);
    let voiceover_count = track_count(&timeline, &TrackType::Voiceover);
    let duration_s = timeline.rendered_duration_ms() as f64 / 1000.0;

    report_progress(100.0, 100.0, "Direct complete").await.ok();

    Ok(json!({
        "status": "rendered",
        "output_path": output,
        "duration_s": (duration_s * 100.0).round() / 100.0,
        "segments_count": timeline.segments.len(),
        "broll_count": broll_count,
        "sfx_count": sfx_count,
        "music_count": music_count,
        "voiceover_count": voiceover_count,
        "timeline_path": timeline_path,
        "warnings": if warnings.is_empty() { serde_json::Value::Null } else { json!(warnings) },
    }))
}

pub(crate) async fn handle_timeline_inspect(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let preview_path = extract_str(&args, "timeline_preview_path")?;
    let layer = extract_str(&args, "layer")?;

    // Read the timeline preview file
    let preview = std::fs::read_to_string(sanitize_input_path(preview_path)?)
        .map_err(|e| ToolError::NotFound(format!("Cannot read timeline preview: {}", e)))?;

    // Also try to read the timeline JSON for full details
    let timeline_dir = std::path::Path::new(preview_path)
        .parent()
        .unwrap_or(std::path::Path::new("."));
    let timeline_json_path = timeline_dir.join("timeline.json");
    let manifest_path = timeline_dir.join("voices").join("manifest.json");

    let mut details = Vec::new();

    match layer {
        "background" => {
            // Read from timeline tracks, not assets (the schema stores events in tracks)
            if let Ok(tl_str) = std::fs::read_to_string(&timeline_json_path) {
                if let Ok(tl) = serde_json::from_str::<serde_json::Value>(&tl_str) {
                    // Try tracks.broll first (EDL v2 schema)
                    if let Some(tracks) = tl.get("tracks").and_then(|t| t.as_object()) {
                        if let Some(broll_events) = tracks.get("broll").and_then(|b| b.as_array()) {
                            for event in broll_events {
                                let id = event.get("id").and_then(|v| v.as_str()).unwrap_or("");
                                let start_ms =
                                    event.get("start_ms").and_then(|v| v.as_i64()).unwrap_or(0);
                                let end_ms =
                                    event.get("end_ms").and_then(|v| v.as_i64()).unwrap_or(0);
                                let asset_id =
                                    event.get("asset_id").and_then(|v| v.as_str()).unwrap_or("");
                                // Look up the actual file path from assets
                                let path = tl
                                    .get("assets")
                                    .and_then(|a| a.get("broll"))
                                    .and_then(|b| b.as_object())
                                    .and_then(|b| b.get(asset_id))
                                    .and_then(|p| p.get("path"))
                                    .and_then(|p| p.as_str())
                                    .unwrap_or(asset_id);
                                details.push(json!({
                                    "id": id,
                                    "start_ms": start_ms,
                                    "end_ms": end_ms,
                                    "path": path,
                                    "exists": std::path::Path::new(path).exists(),
                                }));
                            }
                        }
                    }
                    // Also check assets.broll as fallback
                    if details.is_empty() {
                        if let Some(broll) = tl
                            .get("assets")
                            .and_then(|a| a.get("broll"))
                            .and_then(|b| b.as_object())
                        {
                            for (id, asset) in broll {
                                let path = asset.get("path").and_then(|v| v.as_str()).unwrap_or("");
                                details.push(json!({
                                    "id": id,
                                    "path": path,
                                    "exists": std::path::Path::new(path).exists(),
                                }));
                            }
                        }
                    }
                }
            }
        }
        "voiceover" => {
            if let Ok(m_str) = std::fs::read_to_string(&manifest_path) {
                if let Ok(m) = serde_json::from_str::<serde_json::Value>(&m_str) {
                    if let Some(segments) = m.get("segments").and_then(|v| v.as_array()) {
                        for seg in segments {
                            details.push(json!({
                                "scene_id": seg.get("scene_id"),
                                "speaker": seg.get("speaker"),
                                "text": seg.get("text"),
                                "start_ms": seg.get("start_ms"),
                                "end_ms": seg.get("end_ms"),
                                "duration_ms": seg.get("duration_ms"),
                                "wav_path": seg.get("wav_path"),
                                "word_count": seg.get("words").and_then(|v| v.as_array()).map(|a| a.len()),
                                "backend": seg.get("backend"),
                            }));
                        }
                    }
                }
            }
        }
        "music" => {
            if let Ok(tl_str) = std::fs::read_to_string(&timeline_json_path) {
                if let Ok(tl) = serde_json::from_str::<serde_json::Value>(&tl_str) {
                    // Try tracks.music first (EDL v2 schema)
                    if let Some(tracks) = tl.get("tracks").and_then(|t| t.as_object()) {
                        if let Some(music_events) = tracks.get("music").and_then(|m| m.as_array()) {
                            for event in music_events {
                                let id = event.get("id").and_then(|v| v.as_str()).unwrap_or("");
                                let start_ms =
                                    event.get("start_ms").and_then(|v| v.as_i64()).unwrap_or(0);
                                let end_ms =
                                    event.get("end_ms").and_then(|v| v.as_i64()).unwrap_or(0);
                                let asset_id =
                                    event.get("asset_id").and_then(|v| v.as_str()).unwrap_or("");
                                let path = tl
                                    .get("assets")
                                    .and_then(|a| a.get("music"))
                                    .and_then(|m| m.as_object())
                                    .and_then(|m| m.get(asset_id))
                                    .and_then(|p| p.get("path"))
                                    .and_then(|p| p.as_str())
                                    .unwrap_or(asset_id);
                                details.push(json!({
                                    "id": id,
                                    "start_ms": start_ms,
                                    "end_ms": end_ms,
                                    "path": path,
                                }));
                            }
                        }
                    }
                    // Fallback to assets.music
                    if details.is_empty() {
                        if let Some(music) = tl
                            .get("assets")
                            .and_then(|a| a.get("music"))
                            .and_then(|m| m.as_object())
                        {
                            for (id, asset) in music {
                                details.push(json!({
                                    "id": id,
                                    "path": asset.get("path"),
                                }));
                            }
                        }
                    }
                }
            }
        }
        "captions" => {
            let captions_path = timeline_dir.join("captions.ass");
            if let Ok(content) = std::fs::read_to_string(&captions_path) {
                let dialogue_count = content.matches("Dialogue:").count();
                details.push(json!({
                    "path": captions_path.to_string_lossy(),
                    "dialogue_count": dialogue_count,
                    "size_bytes": content.len(),
                }));
            }
        }
        "stickers" => {
            // Stickers are in the script.to_video response, not stored separately
            details.push(json!({
                "message": "Sticker details are in the script.to_video response. Check the 'sticker_count' and 'timeline_preview' fields.",
            }));
        }
        _ => {
            return Err(ToolError::InvalidArg(format!(
                "Unknown layer: {}. Use: background, voiceover, music, captions, stickers",
                layer
            )));
        }
    }

    Ok(json!({
        "status": "inspected",
        "layer": layer,
        "event_count": details.len(),
        "events": details,
        "preview_excerpt": preview.lines().take(5).collect::<Vec<_>>().join("\n"),
    }))
}

/// Place an image/GIF/PNG overlay on the timeline at a specific position and
/// duration. The overlay is stored as a b-roll track event with a special
/// `overlay` tag, and the render pipeline composites it via FFmpeg's overlay
/// filter. This closes the "search → download → assign" loop for stickers,
/// GIFs, and images.
pub(crate) async fn handle_overlay_assign(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let timeline_path = extract_str(&args, "timeline_path")?;
    let asset_path = extract_str(&args, "asset_path")?;
    let start_ms = extract_i64(&args, "start_ms")?;
    let end_ms = extract_i64(&args, "end_ms")?;
    let position = default_str(&args, "position", "bottom-right");
    let scale = default_f64(&args, "scale", 0.2);
    let fade_in_ms = default_u32(&args, "fade_in_ms", 0);
    let fade_out_ms = default_u32(&args, "fade_out_ms", 0);
    let speaker_name = default_opt_str(&args, "speaker_name");

    // Validate the asset exists
    if !std::path::Path::new(asset_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Overlay asset not found: {}. Use media.download or gif.download to fetch it first.",
            asset_path
        )));
    }

    let mut timeline = Timeline::load(timeline_path)?;
    let event_id = format!("overlay_{:03}", track_count(&timeline, &TrackType::Broll) + 1);

    let duration_ms = end_ms - start_ms;
    let mut tags = vec!["overlay".to_string(), position.to_string()];
    if let Some(ref speaker) = speaker_name {
        tags.push(speaker.clone());
    }

    let event = openscript_core::timeline::TimelineEvent {
        id: event_id.clone(),
        asset_id: asset_path.to_string(),
        start_ms,
        end_ms,
        offset_ms: 0,
        gain_db: 0.0,
        fade_in_ms,
        fade_out_ms,
        tags,
        provenance: Some(openscript_core::timeline::Provenance {
            tool: "overlay.assign".into(),
            editorial_role: None,
            concept: Some(format!("overlay:{}:{}", position, scale)),
        }),
        kind: openscript_core::timeline::EventKind::Broll {
            concept: format!("overlay:{}", position),
            source_provider: asset_path.to_string(),
            transition_style: "overlay".into(),
            crop_mode: "none".into(),
            orientation: "9:16".into(),
            motion_intensity: "static".into(),
        },
    };

    timeline.add_track_event(TrackType::Broll, event);
    timeline.add_asset(
        "broll",
        event_id.clone(),
        json!({
            "path": asset_path,
            "overlay": true,
            "position": position,
            "scale": scale,
        }),
    );
    timeline.save(timeline_path)?;

    Ok(json!({
        "status": "assigned",
        "event_id": event_id,
        "asset_path": asset_path,
        "start_ms": start_ms,
        "end_ms": end_ms,
        "duration_ms": duration_ms,
        "position": position,
        "scale": scale,
        "timeline_path": timeline_path,
    }))
}

/// Compile an EDL v2 timeline JSON into a HyperFrames HTML composition by
/// shelling out to `tsx hyperframes/src/edl_v2_to_html.ts`. The resulting
/// index.html can then be rendered via `hf.render` or `composition.render`.
///
/// This is the bridge between the NLE timeline (EDL v2 JSON) and the
/// HyperFrames motion-graphics render engine. Prior to this tool, the
/// edl_v2_to_html.ts compiler was dead code — never called by any Rust
/// handler. An agent had to run it manually, which broke the programmatic
/// pipeline.
pub(crate) async fn handle_timeline_to_hyperframes(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let timeline_path = extract_str(&args, "timeline_path")?;
    let output_dir = default_str(&args, "output_dir", "artifacts/hf_composition");
    let composition_id = default_opt_str(&args, "composition_id");

    // Validate timeline exists
    if !std::path::Path::new(timeline_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Timeline file not found: {}",
            timeline_path
        )));
    }

    // Create output directory
    std::fs::create_dir_all(&output_dir)
        .map_err(|e| ToolError::Asset(format!("Failed to create output dir: {}", e)))?;

    let index_html_path = format!("{}/index.html", output_dir);

    // Build the tsx command
    let compiler_script = "hyperframes/src/edl_v2_to_html.ts";
    if !std::path::Path::new(compiler_script).exists() {
        return Err(ToolError::NotFound(format!(
            "HyperFrames compiler not found: {}. Ensure the hyperframes/ directory is present.",
            compiler_script
        )));
    }

    let mut cmd = tokio::process::Command::new("npx");
    cmd.arg("tsx")
        .arg(compiler_script)
        .arg("--timeline")
        .arg(timeline_path)
        .arg("--out")
        .arg(&index_html_path);

    if let Some(ref cid) = composition_id {
        cmd.arg("--composition-id").arg(cid);
    }

    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    // Run with a 60s timeout — compilation should be fast
    let child = cmd
        .spawn()
        .map_err(|e| ToolError::Asset(format!("Failed to spawn tsx: {}", e)))?;

    let output = tokio::time::timeout(std::time::Duration::from_secs(60), child.wait_with_output())
        .await
        .map_err(|_| ToolError::Asset("tsx compilation timed out (60s)".to_string()))?
        .map_err(|e| ToolError::Asset(format!("tsx execution failed: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ToolError::Asset(format!(
            "HyperFrames compilation failed: {}",
            stderr.trim()
        )));
    }

    // Verify the output was created
    if !std::path::Path::new(&index_html_path).exists() {
        return Err(ToolError::Asset(format!(
            "Compilation appeared to succeed but no index.html was written to {}",
            index_html_path
        )));
    }

    let file_size = std::fs::metadata(&index_html_path)
        .map(|m| m.len())
        .unwrap_or(0);

    // Extract composition ID from the HTML (data-composition-id attribute)
    let html_content = std::fs::read_to_string(&index_html_path).unwrap_or_default();
    let extracted_cid = html_content
        .find("data-composition-id=\"")
        .and_then(|pos| {
            let start = pos + "data-composition-id=\"".len();
            html_content[start..].find('"').map(|end| &html_content[start..start + end])
        })
        .unwrap_or("unknown")
        .to_string();

    Ok(json!({
        "status": "compiled",
        "project_dir": output_dir,
        "index_html_path": index_html_path,
        "composition_id": extracted_cid,
        "file_size_bytes": file_size,
        "next_step": "Call hf.render or composition.render with project_dir to produce the final MP4",
    }))
}

