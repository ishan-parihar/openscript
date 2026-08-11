// ---------------------------------------------------------------------------
// tools_verify — Quality verification handlers (verify.*, director.run)
// Split out of tools.rs (pure-move refactor). `use super::*` grants access
// to tools.rs's helpers, imports, and re-exported sibling handlers.
// ---------------------------------------------------------------------------
use super::*;

/// Measure a WAV file's integrated loudness (LUFS) via ffmpeg loudnorm's
/// JSON print — shared probe_audio_lufs helper (checks stdout+stderr,
/// anchored parse). Returns None when the file is missing/unreadable — the
/// caller decides how to treat a gap.
async fn measure_scene_lufs(path: &str) -> Option<f64> {
    probe_audio_lufs(path).await
}

/// Resolve the per-scene voiceover WAVs from either an explicit `scene_wavs`
/// array or a script.generate_voices manifest (`voiceover_manifest` with
/// `segments[].wav_path`). Returns (paths, scene_names) in order.
fn resolve_scene_wavs(args: &serde_json::Value) -> (Vec<String>, Vec<String>) {
    // Explicit array wins.
    if let Some(arr) = args.get("scene_wavs").and_then(|v| v.as_array()) {
        let paths: Vec<String> = arr
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
        if !paths.is_empty() {
            let names: Vec<String> = paths
                .iter()
                .map(|p| {
                    Path::new(p)
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| "scene".to_string())
                })
                .collect();
            return (paths, names);
        }
    }
    // Fall back to a generate_voices manifest.
    if let Some(mp) = args.get("voiceover_manifest").and_then(|v| v.as_str()) {
        if let Ok(raw) = std::fs::read_to_string(mp) {
            if let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&raw) {
                if let Some(segs) = manifest.get("segments").and_then(|v| v.as_array()) {
                    let paths: Vec<String> = segs
                        .iter()
                        .filter_map(|s| s.get("wav_path").and_then(|v| v.as_str()).map(String::from))
                        .collect();
                    let names: Vec<String> = segs
                        .iter()
                        .filter_map(|s| s.get("scene_id").and_then(|v| v.as_str()).map(String::from))
                        .collect();
                    if !paths.is_empty() {
                        return (paths, names);
                    }
                }
            }
        }
    }
    (Vec::new(), Vec::new())
}

pub(crate) async fn handle_verify_audio(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?
        .to_string_lossy()
        .to_string();
    let expected_has_voice = default_bool(&args, "expected_has_voice", true);
    let max_silence_seconds = default_f64(&args, "max_silence_seconds", 3.0);

    if !Path::new(&video_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Video not found: {}",
            video_path
        )));
    }

    let output = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=codec_name,sample_rate,channels,duration",
            "-of",
            "json",
            &video_path,
        ])
        .output()
        .await
        .map_err(|e| ToolError::Ffmpeg(format!("ffprobe failed: {}", e)))?;

    if !output.status.success() {
        return Err(ToolError::Ffmpeg(format!(
            "ffprobe failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    let probe: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(ToolError::Json)?;

    let streams = probe
        .get("streams")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let has_audio = !streams.is_empty();

    if !has_audio {
        return Ok(json!({
            "status": "warning",
            "issues": ["No audio stream detected — voice/music/SFX are missing"],
            "rms_lufs": null,
            "peak_db": null,
            "silence_segments": [],
            "has_dialogue": false,
            "quality_score": 0,
        }));
    }

    let vol_output = tokio::process::Command::new("ffmpeg")
        .args(["-i", &video_path, "-af", "volumedetect", "-f", "null", "-"])
        .output()
        .await
        .map_err(|e| ToolError::Ffmpeg(format!("volumedetect failed: {}", e)))?;

    if !vol_output.status.success() {
        return Err(ToolError::Ffmpeg(format!(
            "volumedetect failed: {}",
            String::from_utf8_lossy(&vol_output.stderr)
        )));
    }

    let stderr = String::from_utf8_lossy(&vol_output.stderr);
    let mean_volume = stderr
        .lines()
        .find(|l| l.contains("mean_volume"))
        .and_then(|l| l.split(": ").nth(1))
        .and_then(|v| v.trim_end_matches(" dB").parse::<f64>().ok());
    let max_volume = stderr
        .lines()
        .find(|l| l.contains("max_volume"))
        .and_then(|l| l.split(": ").nth(1))
        .and_then(|v| v.trim_end_matches(" dB").parse::<f64>().ok());

    let silence_output = tokio::process::Command::new("ffmpeg")
        .args([
            "-i",
            &video_path,
            "-af",
            &format!("silencedetect=noise=-30dB:d={}", max_silence_seconds),
            "-f",
            "null",
            "-",
        ])
        .output()
        .await
        .map_err(|e| ToolError::Ffmpeg(format!("silencedetect failed: {}", e)))?;

    if !silence_output.status.success() {
        return Err(ToolError::Ffmpeg(format!(
            "silencedetect failed: {}",
            String::from_utf8_lossy(&silence_output.stderr)
        )));
    }

    let silence_stderr = String::from_utf8_lossy(&silence_output.stderr);
    let mut silence_segments: Vec<serde_json::Value> = Vec::new();
    let mut current_start: Option<f64> = None;
    for line in silence_stderr.lines() {
        if line.contains("silence_start:") {
            if let Some(val) = line.split(": ").nth(1).and_then(|v| v.parse::<f64>().ok()) {
                current_start = Some(val);
            }
        } else if line.contains("silence_end:") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if let (Some(start), Some(end)) = (
                current_start,
                parts.get(1).and_then(|v| v.parse::<f64>().ok()),
            ) {
                silence_segments.push(json!({
                    "start": start,
                    "end": end,
                    "duration": end - start,
                }));
                current_start = None;
            }
        }
    }

    // --- Per-scene loudness-variance KPI (Phase 169d) ---
    // When the caller supplies the per-scene voiceover WAVs (explicit
    // `scene_wavs` array or a generate_voices `voiceover_manifest`), measure
    // each scene's integrated LUFS and flag a >6 dB spread. This locks the
    // production-grade invariant that every scene sits at uniform loudness:
    // pre-fix emotion takes came out 10-20 dB quieter than the base voice,
    // leaving lines effectively muted under the music bed ("second speaker
    // inaudible" bug). The TTS sidecars now normalize at the source; this
    // KPI makes the regression loud again instead of shipping silently.
    let (scene_wavs, scene_names) = resolve_scene_wavs(&args);
    let mut per_scene_lufs: Vec<serde_json::Value> = Vec::new();
    let mut lufs_vals: Vec<f64> = Vec::new();
    for (i, wav) in scene_wavs.iter().enumerate() {
        let lufs = if Path::new(wav).exists() {
            measure_scene_lufs(wav).await
        } else {
            None
        };
        if let Some(l) = lufs {
            lufs_vals.push(l);
        }
        per_scene_lufs.push(json!({
            "scene": scene_names.get(i).cloned().unwrap_or_else(|| format!("scene_{}", i + 1)),
            "path": wav,
            "lufs": lufs,
        }));
    }
    let loudness_spread_db: Option<f64> = if lufs_vals.len() >= 2 {
        let min = lufs_vals.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = lufs_vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        Some(max - min)
    } else {
        None
    };
    // null when nothing was measured (a consumer must not read true as
    // "verified" for an empty scene list OR when every measurement failed).
    let loudness_variance_ok: Option<bool> = loudness_spread_db.map(|s| s <= 6.0);

    let rms = mean_volume.unwrap_or(-99.0);
    let peak = max_volume.unwrap_or(-99.0);
    let has_good_level = (-30.0..=-12.0).contains(&rms);
    let has_no_clipping = peak <= 0.0;
    let no_long_silence = silence_segments.is_empty();

    let quality_score = if expected_has_voice {
        let mut score = 0;
        if has_audio {
            score += 25;
        }
        if has_good_level {
            score += 25;
        }
        if has_no_clipping {
            score += 25;
        }
        if no_long_silence {
            score += 25;
        }
        // Loudness-variance KPI: >6 dB spread costs 20 pts (warning); a
        // >12 dB spread (the pre-fix mute-range) costs another 15.
        if let Some(spread) = loudness_spread_db {
            if spread > 6.0 {
                score -= 20;
            }
            if spread > 12.0 {
                score -= 15;
            }
        }
        score
    } else {
        if has_audio {
            50
        } else {
            100
        }
    };

    let mut issues: Vec<String> = Vec::new();
    if !has_audio {
        issues.push("No audio stream".into());
    }
    if !has_good_level && has_audio {
        issues.push(format!(
            "Audio level unhealthy: RMS {} dB (expected -30 to -12 dB)",
            rms
        ));
    }
    if !has_no_clipping {
        issues.push(format!("Audio clipping detected: peak {} dB", peak));
    }
    if !no_long_silence {
        issues.push(format!(
            "{} silence gaps detected (>{})",
            silence_segments.len(),
            max_silence_seconds
        ));
    }
    if let Some(spread) = loudness_spread_db {
        if spread > 6.0 {
            issues.push(format!(
                "Per-scene loudness variance {:.1} dB exceeds 6 dB — quiet scenes get buried under the music bed. Re-generate voices (TTS sidecars normalize each scene to -16 LUFS; emotion takes must be designed via the fixed voicedesign sidecar).",
                spread
            ));
        }
    }

    Ok(json!({
        "status": if quality_score >= 75 { "pass" } else if quality_score >= 50 { "warning" } else { "fail" },
        "rms_lufs": rms,
        "peak_db": peak,
        "silence_segments": silence_segments,
        "has_dialogue": has_audio && has_good_level,
        "quality_score": quality_score,
        "issues": issues,
        "audio_codec": streams.first().and_then(|s| s.get("codec_name")).and_then(|v| v.as_str()).unwrap_or("unknown"),
        "sample_rate": streams.first().and_then(|s| s.get("sample_rate")).and_then(|v| v.as_str()).unwrap_or("unknown"),
        "loudness": json!({
            "scene_count": scene_wavs.len(),
            "per_scene_lufs": per_scene_lufs,
            "spread_db": loudness_spread_db,
            "variance_ok": loudness_variance_ok,
            "threshold_db": 6.0,
        }),
    }))
}

pub(crate) async fn handle_verify_captions(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?
        .to_string_lossy()
        .to_string();
    let srt_path = sanitize_input_path(extract_str(&args, "srt_path")?)?
        .to_string_lossy()
        .to_string();
    let min_caption_duration_ms = default_i64(&args, "min_caption_duration_ms", 300);
    let max_caption_duration_ms = default_i64(&args, "max_caption_duration_ms", 5000);

    if !Path::new(&video_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Video not found: {}",
            video_path
        )));
    }
    if !Path::new(&srt_path).exists() {
        return Err(ToolError::NotFound(format!("Caption file not found: {}", srt_path)));
    }

    let probe_output = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "json",
            &video_path,
        ])
        .output()
        .await
        .map_err(|e| ToolError::Ffmpeg(format!("ffprobe failed: {}", e)))?;

    if !probe_output.status.success() {
        return Err(ToolError::Ffmpeg(format!(
            "ffprobe failed: {}",
            String::from_utf8_lossy(&probe_output.stderr)
        )));
    }

    let probe: serde_json::Value =
        serde_json::from_slice(&probe_output.stdout).map_err(ToolError::Json)?;
    let video_duration_s: f64 = probe
        .get("format")
        .and_then(|f| f.get("duration"))
        .and_then(|v| v.as_str())
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(0.0);
    let video_duration_ms = (video_duration_s * 1000.0) as i64;

    // Auto-detect caption format: ASS (.ass) or SRT (.srt).
    // script.to_video emits ASS; verify.captions previously only accepted
    // SRT, which meant the verify step was unusable after a script.to_video
    // render. Now we accept both and normalize to the same entry format.
    // (UX audit round-2 GAP #10 fix.)
    let is_ass = srt_path.ends_with(".ass");
    let entries: Vec<openscript_core::srt::SrtEntry> = if is_ass {
        parse_ass_captions(&srt_path)?
    } else {
        openscript_core::srt::parse_srt(&srt_path)
            .map_err(|e| ToolError::Srt(e.to_string()))?
    };

    if entries.is_empty() {
        return Ok(json!({
            "status": "fail",
            "issues": ["SRT file has no entries"],
            "caption_count": 0,
            "coverage_percent": 0.0,
            "gaps": [],
            "overlaps": [],
            "avg_caption_duration_ms": 0,
            "readability_score": 0,
        }));
    }

    let mut total_caption_ms: i64 = 0;
    let mut gaps: Vec<serde_json::Value> = Vec::new();
    let mut overlaps: Vec<serde_json::Value> = Vec::new();
    let mut too_fast: Vec<serde_json::Value> = Vec::new();
    let mut too_slow: Vec<serde_json::Value> = Vec::new();

    for (i, entry) in entries.iter().enumerate() {
        let start_ms = (entry.start * 1000.0) as i64;
        let end_ms = (entry.end * 1000.0) as i64;
        let duration_ms = end_ms - start_ms;
        total_caption_ms += duration_ms;

        if duration_ms < min_caption_duration_ms {
            too_fast.push(json!({"idx": entry.idx, "duration_ms": duration_ms, "text": entry.text.chars().take(40).collect::<String>()}));
        }
        if duration_ms > max_caption_duration_ms {
            too_slow.push(json!({"idx": entry.idx, "duration_ms": duration_ms, "text": entry.text.chars().take(40).collect::<String>()}));
        }

        if i > 0 {
            let prev_end = (entries[i - 1].end * 1000.0) as i64;
            let gap_ms = start_ms - prev_end;
            if gap_ms > 2000 {
                gaps.push(json!({"after_idx": entries[i-1].idx, "before_idx": entry.idx, "gap_ms": gap_ms}));
            }
        }

        if i > 0 {
            let prev_end = (entries[i - 1].end * 1000.0) as i64;
            let prev_start = (entries[i - 1].start * 1000.0) as i64;
            if start_ms < prev_end && start_ms > prev_start {
                overlaps.push(json!({"idx_a": entries[i-1].idx, "idx_b": entry.idx, "overlap_ms": prev_end - start_ms}));
            }
        }
    }

    let avg_duration = if !entries.is_empty() {
        total_caption_ms / entries.len() as i64
    } else {
        0
    };
    let coverage = if video_duration_ms > 0 {
        (total_caption_ms as f64 / video_duration_ms as f64) * 100.0
    } else {
        0.0
    };

    let mut issues: Vec<String> = Vec::new();
    if !gaps.is_empty() {
        issues.push(format!("{} caption gaps > 2s", gaps.len()));
    }
    if !overlaps.is_empty() {
        issues.push(format!("{} caption overlaps", overlaps.len()));
    }
    if !too_fast.is_empty() {
        issues.push(format!(
            "{} captions too fast (<{}ms)",
            too_fast.len(),
            min_caption_duration_ms
        ));
    }
    if !too_slow.is_empty() {
        issues.push(format!(
            "{} captions too slow (>{})",
            too_slow.len(),
            max_caption_duration_ms
        ));
    }

    let mut score = 100;
    score -= (gaps.len() as i32) * 10;
    score -= (overlaps.len() as i32) * 15;
    score -= (too_fast.len() as i32) * 5;
    score -= (too_slow.len() as i32) * 5;
    let score = score.max(0).min(100);

    Ok(json!({
        "status": if score >= 80 { "pass" } else if score >= 50 { "warning" } else { "fail" },
        "caption_count": entries.len(),
        "coverage_percent": (coverage * 10.0).round() / 10.0,
        "video_duration_ms": video_duration_ms,
        "total_caption_ms": total_caption_ms,
        "avg_caption_duration_ms": avg_duration,
        "gaps": gaps,
        "overlaps": overlaps,
        "too_fast": too_fast,
        "too_slow": too_slow,
        "readability_score": score,
        "issues": issues,
    }))
}

pub(crate) async fn handle_verify_render(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?
        .to_string_lossy()
        .to_string();
    let timeline_path = sanitize_input_path(extract_str(&args, "timeline_path")?)?
        .to_string_lossy()
        .to_string();
    let expected_aspect = default_str(&args, "expected_aspect", "9:16");
    let duration_tolerance_ms = default_i64(&args, "duration_tolerance_ms", 2000);

    if !Path::new(&video_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Video not found: {}",
            video_path
        )));
    }
    if !Path::new(&timeline_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Timeline not found: {}",
            timeline_path
        )));
    }

    let probe_output = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,r_frame_rate,duration",
            "-show_entries",
            "format=duration,size",
            "-of",
            "json",
            &video_path,
        ])
        .output()
        .await
        .map_err(|e| ToolError::Ffmpeg(format!("ffprobe failed: {}", e)))?;

    if !probe_output.status.success() {
        return Err(ToolError::Ffmpeg(format!(
            "ffprobe failed: {}",
            String::from_utf8_lossy(&probe_output.stderr)
        )));
    }

    let probe: serde_json::Value =
        serde_json::from_slice(&probe_output.stdout).map_err(ToolError::Json)?;

    let streams = probe
        .get("streams")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let format_info = probe.get("format").cloned().unwrap_or(json!({}));

    let width = streams
        .first()
        .and_then(|s| s.get("width"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let height = streams
        .first()
        .and_then(|s| s.get("height"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let file_size = std::fs::metadata(video_path).map(|m| m.len()).unwrap_or(0);

    let actual_duration_s: f64 = format_info
        .get("duration")
        .and_then(|v| v.as_str())
        .and_then(|v| v.parse::<f64>().ok())
        .or_else(|| {
            streams
                .first()
                .and_then(|s| s.get("duration"))
                .and_then(|v| v.as_str())
                .and_then(|v| v.parse::<f64>().ok())
        })
        .unwrap_or(0.0);
    let actual_duration_ms = (actual_duration_s * 1000.0) as i64;

    let timeline = Timeline::load(timeline_path).map_err(|e| ToolError::Timeline(e.to_string()))?;
    let expected_duration_ms = timeline.rendered_duration_ms();
    let segment_count = timeline.segments.len();

    let duration_delta = (actual_duration_ms - expected_duration_ms).abs();
    let duration_match = duration_delta <= duration_tolerance_ms;

    let expected_ratio: f64 = match expected_aspect.as_str() {
        "9:16" => 9.0 / 16.0,
        "16:9" => 16.0 / 9.0,
        "1:1" => 1.0,
        "4:5" => 4.0 / 5.0,
        _ => 9.0 / 16.0,
    };
    let actual_ratio = if height > 0 {
        width as f64 / height as f64
    } else {
        0.0
    };
    let aspect_match = (actual_ratio - expected_ratio).abs() < 0.05;

    let tracks_present: serde_json::Map<String, serde_json::Value> = timeline
        .tracks
        .iter()
        .map(|(track, events)| {
            let track = track as &TrackType;
            let events = events as &Vec<openscript_core::timeline::TimelineEvent>;
            (
                track.to_string(),
                json!({"count": events.len(), "rendered": !events.is_empty()}),
            )
        })
        .collect();

    let total_tracks = timeline.tracks.values().filter(|v| !v.is_empty()).count();
    let has_audio = total_tracks > 1;

    let mut issues: Vec<String> = Vec::new();
    if !duration_match {
        issues.push(format!(
            "Duration mismatch: expected {}ms, got {}ms (delta: {}ms)",
            expected_duration_ms, actual_duration_ms, duration_delta
        ));
    }
    if !aspect_match {
        issues.push(format!(
            "Aspect ratio mismatch: expected {}, got {}x{} (ratio: {:.3})",
            expected_aspect, width, height, actual_ratio
        ));
    }
    if file_size == 0 {
        issues.push("File size is 0 bytes — render may have failed".into());
    }
    if width == 0 || height == 0 {
        issues.push("Could not determine video resolution".into());
    }

    let mut score = 100;
    if !duration_match {
        score -= 30;
    }
    if !aspect_match {
        score -= 25;
    }
    if file_size == 0 {
        score -= 45;
    }
    if !has_audio && total_tracks > 1 {
        score -= 15;
    }
    let score = score.max(0).min(100);

    Ok(json!({
        "status": if score >= 80 { "pass" } else if score >= 50 { "warning" } else { "fail" },
        "duration_match": duration_match,
        "expected_duration_ms": expected_duration_ms,
        "actual_duration_ms": actual_duration_ms,
        "duration_delta_ms": duration_delta,
        "segment_count": segment_count,
        "resolution": format!("{}x{}", width, height),
        "aspect_match": aspect_match,
        "expected_aspect": expected_aspect,
        "file_size_bytes": file_size,
        "tracks_present": tracks_present,
        "has_audio_stream": has_audio,
        "issues": issues,
        "overall_score": score,
        "note": "Technical integrity only. Call verify.production for stock/music/sticker beauty KPIs.",
    }))
}

pub(crate) async fn handle_verify_production(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_core::production_quality::{
        evaluate_production_quality, grade_rank, BackgroundLayerInfo, MemeLayerInfo, MusicLayerInfo,
        RenderManifest, StickerLayerInfo, verify_layer_order,
    };

    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?
        .to_string_lossy()
        .to_string();
    let timeline_path = sanitize_input_path(extract_str(&args, "timeline_path")?)?
        .to_string_lossy()
        .to_string();
    let captions_path = default_opt_str(&args, "captions_path");
    let sticker_count = default_u32(&args, "sticker_count", 0) as usize;
    let meme_count = default_u32(&args, "meme_count", 0) as usize;
    let min_grade = default_str(&args, "min_grade", "B");
    let music_path_arg = default_opt_str(&args, "music_path");
    let manifest_path = default_opt_str(&args, "render_manifest_path");

    if !Path::new(&video_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Video not found: {}",
            video_path
        )));
    }
    if !Path::new(&timeline_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Timeline not found: {}",
            timeline_path
        )));
    }

    let timeline = Timeline::load(&timeline_path).map_err(|e| ToolError::Timeline(e.to_string()))?;
    let (has_dialogue, rms_ok) = probe_dialogue_rms(&video_path).await;

    // Probe actual audio metrics from the rendered video
    let (measured_lufs, measured_peak_dbfs, measured_ducking_depth_db, measured_music_gain_db) =
        probe_audio_metrics(&video_path).await;

    // Probe b-roll motion: fraction of frames with non-zero motion and
    // longest static run. Feeds the broll_motion dimension so static
    // b-roll (from source-exhaustion bug) surfaces as a hard fail.
    let (motion_ratio, longest_static_run_s) = probe_broll_motion(&video_path).await;

    // Prefer authoritative render_manifest.json from script.to_video
    let mut manifest = if let Some(ref mp) = manifest_path {
        if Path::new(mp).exists() {
            let raw = std::fs::read_to_string(mp)?;
            serde_json::from_str::<RenderManifest>(&raw).map_err(ToolError::Json)?
        } else {
            RenderManifest::default()
        }
    } else {
        // Co-located default path next to timeline
        let sibling = Path::new(&timeline_path)
            .parent()
            .map(|p| p.join("render_manifest.json"))
            .unwrap_or_else(|| Path::new("render_manifest.json").to_path_buf());
        if sibling.exists() {
            let raw = std::fs::read_to_string(&sibling)?;
            serde_json::from_str::<RenderManifest>(&raw).unwrap_or_default()
        } else {
            RenderManifest::default()
        }
    };

    // Map Stickers-track events into the manifest so stickers placed by
    // sticker.auto / sticker.auto_assign are scored (not reported absent).
    if manifest.stickers.is_empty() {
        manifest.stickers = stickers_from_timeline(&timeline);
    }

    // Merge explicit overrides / legacy args into manifest
    if manifest.backgrounds.is_empty() {
        let bg_paths: Vec<String> = args
            .get("background_sources")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if !bg_paths.is_empty() {
            let n = bg_paths.len().max(1);
            let slice = (timeline.rendered_duration_ms() / n as i64).max(1);
            manifest.backgrounds = bg_paths
                .into_iter()
                .enumerate()
                .map(|(i, p)| BackgroundLayerInfo {
                    path: p,
                    start_ms: i as i64 * slice,
                    end_ms: (i as i64 + 1) * slice,
                    source_hint: None,
                    content_hash: None,
                    video_id: None,
                    search_query: None,
                    lexical_score: None,
                    source_title: None,
                    vision_score: None,
                    vision_reason: None,
                })
                .collect();
        }
    }
    if manifest.stickers.is_empty() && sticker_count > 0 {
        manifest.stickers = (0..sticker_count)
            .map(|i| StickerLayerInfo {
                path: format!("sticker_{}", i),
                start_ms: 0,
                end_ms: 1000,
                position: "top-left".into(),
                scale: 0.35,
            })
            .collect();
    }
    if manifest.memes.is_empty() && meme_count > 0 {
        manifest.memes = (0..meme_count)
            .map(|i| MemeLayerInfo {
                path: format!("meme_{}", i),
                start_ms: 1000 + i as i64 * 500,
                end_ms: 3000 + i as i64 * 500,
            })
            .collect();
    }
    if manifest.music.is_none() {
        if let Some(p) = music_path_arg {
            manifest.music = Some(MusicLayerInfo {
                path: p,
                gain_db: 0.0,
                ducking: true,
                mood: None,
                energy: None,
             tags: vec![], selection_query: None, source: None, });
        }
    }
    if manifest.captions_path.is_none() {
        manifest.captions_path = captions_path;
    }
    if manifest.duration_ms <= 0 {
        manifest.duration_ms = timeline.rendered_duration_ms();
    }
    manifest.has_dialogue = has_dialogue;
    // Set video_keywords from agent if provided
    if manifest.video_keywords.is_empty() {
        if let Some(kw_arr) = args.get("video_keywords").and_then(|v| v.as_array()) {
            manifest.video_keywords = kw_arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
        }
    }
    // Set caption_style: prefer agent arg, fallback to timeline.effects.caption_style
    if manifest.caption_style.is_none() {
        manifest.caption_style = args.get("caption_style")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| timeline.effects.caption_style.clone());
    }
    // Set voiceover_count based on dialogue detection
    // If the video has dialogue (original audio), count it as voiceover
    if manifest.voiceover_count == 0 && has_dialogue {
        manifest.voiceover_count = 1;
    }
    manifest.rms_ok = rms_ok;

    // Update manifest with measured audio metrics (override planned values with reality)
    if let Some(l) = measured_lufs {
        manifest.measured_lufs = Some(l);
        manifest.lufs = Some(l);
    }
    if let Some(p) = measured_peak_dbfs {
        manifest.measured_peak_dbfs = Some(p);
        manifest.peak_dbfs = Some(p);
    }
    if let Some(d) = measured_ducking_depth_db {
        manifest.measured_ducking_depth_db = Some(d);
        manifest.ducking_depth_db = Some(d);
    }
    if let Some(g) = measured_music_gain_db {
        manifest.measured_music_gain_db = Some(g);
        if manifest.music.is_some() {
            manifest.music.as_mut().unwrap().gain_db = g;
        }
    }
    // Update manifest with measured b-roll motion (catches source-exhaustion
    // bug — static frames after seek_offset lands past source end).
    if let Some(r) = motion_ratio {
        manifest.broll_motion_ratio = Some(r);
    }
    if let Some(s) = longest_static_run_s {
        manifest.longest_static_run_s = Some(s);
    }

    // Per-clip b-roll motion analysis: detect static frames at the
    // individual clip intersection level, not just globally.
    let mut per_clip_motion: Vec<serde_json::Value> = Vec::new();
    if let Some(broll_track) = timeline.tracks.get(&TrackType::Broll) {
        let clip_ranges: Vec<(f64, f64)> = broll_track
            .iter()
            .filter(|ev| !ev.asset_id.is_empty() && ev.asset_id != "placeholder")
            .map(|ev| (ev.start_ms as f64 / 1000.0, ev.end_ms as f64 / 1000.0))
            .collect();
        if !clip_ranges.is_empty() {
            let clip_results = probe_broll_motion_per_clip(&video_path, &clip_ranges).await;
            let static_clips: Vec<usize> = clip_results
                .iter()
                .filter(|(_, ratio, _)| ratio.map_or(false, |r| r < 0.30))
                .map(|(idx, _, _)| *idx)
                .collect();
            for (idx, ratio, run_s) in &clip_results {
                per_clip_motion.push(json!({
                    "clip_index": idx,
                    "motion_ratio": ratio.map(|r| (r * 1000.0).round() / 1000.0),
                    "longest_static_run_s": run_s.map(|s| (s * 100.0).round() / 100.0),
                    "static": ratio.map_or(true, |r| r < 0.30),
                }));
            }
            if !static_clips.is_empty() {
                tracing::warn!(
                    "PER-CLIP STATIC DETECTED: {} clip(s) with < 30% motion: {:?}",
                    static_clips.len(),
                    static_clips
                );
            }
        }
    }

    // Override global broll_motion metrics with per-clip frame-hash data
    // when available — frame-hash detection is more accurate than scene
    // scores for gradual zoompan motion.
    if !per_clip_motion.is_empty() {
        let valid_ratios: Vec<f64> = per_clip_motion.iter()
            .filter_map(|c| c.get("motion_ratio").and_then(|r| r.as_f64()))
            .collect();
        let valid_runs: Vec<f64> = per_clip_motion.iter()
            .filter_map(|c| c.get("longest_static_run_s").and_then(|r| r.as_f64()))
            .collect();
        if !valid_ratios.is_empty() {
            let avg_ratio = valid_ratios.iter().sum::<f64>() / valid_ratios.len() as f64;
            manifest.broll_motion_ratio = Some(avg_ratio);
        }
        if !valid_runs.is_empty() {
            let max_run = valid_runs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            manifest.longest_static_run_s = Some(max_run);
        }
    }

    // Probe b-roll coverage: clip duration vs segment window. The renderer
    // plays clips exactly once (Phase A — no loop fill), so any segment
    // whose clip is shorter than its window leaves a visible gap. Surfacing
    // these as errors is the loop-closure signal: the agent re-runs keyword
    // generation + broll.fetch for a longer clip.
    let broll_gaps = probe_broll_gaps(&timeline).await;
    if !broll_gaps.is_empty() {
        manifest.broll_gaps = broll_gaps.clone();
    }

    let report = evaluate_production_quality(&timeline, &manifest);
    let meets_min = grade_rank(&report.grade) >= grade_rank(&min_grade);

    // Verify layer composition order
    let layer_report = verify_layer_order(&manifest);

    // Post-generation COMPOSITION AUDIT — which layers are present, in which
    // z-order, with counts and ranges. This is the meta-cognitive layer the
    // agent needs to reason about its own render (and to hand to a human or a
    // follow-up iteration): a render whose composition is missing captions or
    // music is immediately diagnosable from this block alone.
    let composition = build_composition_audit(&timeline, &manifest);

    // Optional vision re-score of background clips (local Qwen → OpenRouter free).
    let vision_rescore = default_bool(&args, "vision_rescore", false);
    let mut vision_scores: Vec<serde_json::Value> = Vec::new();
    if vision_rescore {
        let keywords = manifest.video_keywords.clone();
        let scene_fallback = timeline
            .segments
            .iter()
            .map(|s| s.caption.as_str())
            .filter(|c| !c.is_empty())
            .take(4)
            .collect::<Vec<_>>()
            .join(" ");
        for (i, bg) in manifest.backgrounds.iter().take(8).enumerate() {
            if bg.path.is_empty() || bg.path == "placeholder" || !Path::new(&bg.path).exists() {
                vision_scores.push(json!({
                    "index": i,
                    "path": bg.path,
                    "status": "skipped",
                    "reason": "missing or placeholder path",
                }));
                continue;
            }
            let scene_text = if scene_fallback.is_empty() {
                bg.search_query.clone().unwrap_or_else(|| "video scene".into())
            } else {
                scene_fallback.clone()
            };
            match crate::llm::score_clip_relevance(
                &bg.path,
                &scene_text,
                &keywords,
                bg.search_query.as_deref(),
            )
            .await
            {
                Ok(v) => vision_scores.push(v),
                Err(e) => vision_scores.push(json!({
                    "index": i,
                    "path": bg.path,
                    "status": "error",
                    "error": e.to_string(),
                })),
            }
        }
    }

    let status = if !report.hard_fails.is_empty() {
        "fail"
    } else if meets_min {
        "pass"
    } else if report.production_score >= 40 {
        "warning"
    } else {
        "fail"
    };
    // Coverage-gap directives join the agent's next_actions so the audit
    // loop knows exactly which segments need a longer clip.
    let mut next_actions = report.next_actions.clone();
    for g in &manifest.broll_gaps {
        next_actions.push(g.action.clone());
    }
    Ok(json!({
        "status": status,
        "production_score": report.production_score,
        "grade": report.grade,
        "min_grade": min_grade,
        "meets_min_grade": meets_min && report.hard_fails.is_empty(),
        "hard_fails": report.hard_fails,
        "dimensions": report.dimensions,
        "next_actions": next_actions,
        "broll_gaps": manifest.broll_gaps,
        "cuts_per_second": report.cuts_per_second,
        "video_source_mix": report.video_source_mix,
        "timeline_editor": report.timeline_editor,
        "layer_order": layer_report,
        "composition": composition,
        "per_clip_motion": per_clip_motion,
        "kpi_version": report.kpi_version,
        "kpi_note": "verify.render is technical-only. Production v3 hard-fails majority procedural, missing visual hooks, and parade music on calm/focus. Use real stock + topic-tagged music.",
        "vision_rescore": vision_rescore,
        "vision_scores": vision_scores,
    }))
}

/// Content-format playbook (director.format). Returns the playbook for a
/// format type — or the full list when type is missing/'list'. This is the
/// MCP harness surface that shapes HOW agents author scripts (speaker
/// blueprint with gender alternation, scene structure, pacing, reactions).
pub(crate) async fn handle_director_format(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let r#type = default_str(&args, "type", "list");
    if r#type == "list" || r#type.is_empty() {
        return Ok(json!({
            "status": "success",
            "formats": crate::content_formats::format_list(),
            "note": "Call director.format {type: '<format>', topic: '<topic>'} for the full playbook (speaker blueprint, scene structure, worked example).",
        }));
    }
    if !crate::content_formats::is_valid_format(&r#type) {
        return Err(ToolError::InvalidArg(format!(
            "Unknown format '{}'. Must be one of: {}",
            r#type,
            crate::content_formats::FORMAT_TYPES.join(", ")
        )));
    }
    let topic = default_str(&args, "topic", "");
    let playbook = crate::content_formats::playbook(&r#type, &topic);
    Ok(json!({
        "status": "success",
        "playbook": playbook,
        "next_steps": playbook.get("next_steps").cloned().unwrap_or(json!(null)),
    }))
}

/// ONE-SHOT director: preflight → parse → to_video → verify.production.
pub(crate) async fn handle_director_run(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let mut script = extract_str(&args, "script")?.to_string();
    let output_path = default_str(&args, "output_path", "artifacts/director_out.mp4");
    let output_dir = default_str(&args, "output_dir", "artifacts/director_run");
    let min_grade = default_str(&args, "min_grade", "B");

    // Optional content-format injection: when the caller declares a format
    // and the script lacks a format block, inject the format's correlated
    // defaults so parse applies the pacing/reaction/music guidance.
    if let Some(fmt) = default_opt_str(&args, "format") {
        if crate::content_formats::is_valid_format(&fmt) {
            let defaults = crate::content_formats::playbook(&fmt, "")
                .get("defaults")
                .cloned()
                .unwrap_or_else(|| json!({ "type": fmt }));
            let raw = if script.trim_start().starts_with('{') {
                script.clone()
            } else {
                std::fs::read_to_string(&script).unwrap_or_else(|_| script.clone())
            };
            if let Ok(mut root) = serde_json::from_str::<serde_json::Value>(&raw) {
                if let Some(obj) = root.as_object_mut() {
                    if !obj.contains_key("format") {
                        obj.insert("format".into(), defaults);
                        if let Ok(serialized) = serde_json::to_string(&root) {
                            script = serialized;
                        }
                    }
                }
            }
        }
    }
    let _ = std::fs::create_dir_all(&output_dir);

    // Preflight
    let pexels = !pexels_key().is_empty();
    let ytdlp = std::process::Command::new("yt-dlp")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !pexels && !ytdlp {
        return Err(ToolError::Asset(
            "director.run preflight failed: need PEXELS_API_KEY or yt-dlp for stock B-roll"
                .into(),
        ));
    }
    let mut preflight_warnings: Vec<String> = Vec::new();
    if !pexels {
        preflight_warnings.push(
            "PEXELS_API_KEY unset — YouTube-only multi-broll (weaker relevance). Set api_keys.pexels in ~/.openscript/config.json"
                .into(),
        );
    }
    let lib = resolve_repo_path("mcp/assets/music_library_index.json");
    if !lib.exists() {
        preflight_warnings.push(
            "music_library_index.json missing — run library.build for tagged music".into(),
        );
    }

    let parse = handle_script_parse(json!({"script": script})).await?;
    if parse.get("status").and_then(|s| s.as_str()) == Some("error")
        || parse
            .get("errors")
            .and_then(|e| e.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(false)
    {
        return Ok(json!({
            "status": "error",
            "phase": "parse",
            "parse": parse,
            "preflight_warnings": preflight_warnings,
        }));
    }

    let to_video = handle_script_to_video(json!({
        "script": script,
        "output_path": output_path,
        "output_dir": output_dir,
    }))
    .await?;

    let video = to_video
        .get("output_path")
        .and_then(|v| v.as_str())
        .unwrap_or(&output_path)
        .to_string();
    let timeline = to_video
        .get("timeline_path")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let manifest = to_video
        .get("render_manifest_path")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let mut verify_args = json!({
        "video_path": video,
        "timeline_path": timeline,
        "min_grade": min_grade,
    });
    if let Some(m) = manifest {
        verify_args["render_manifest_path"] = json!(m);
    }
    let production = if !timeline.is_empty() && Path::new(&video).exists() {
        handle_verify_production(verify_args).await.ok()
    } else {
        None
    };

    Ok(json!({
        "status": to_video.get("status").cloned().unwrap_or(json!("unknown")),
        "preflight_warnings": preflight_warnings,
        "parse": parse,
        "to_video": to_video,
        "verify_production": production,
        "output_path": video,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------------
    // Per-scene loudness-variance KPI tests (Phase 169d)
    // ---------------------------------------------------------------------

    #[test]
    fn resolve_scene_wavs_accepts_explicit_array() {
        let args = json!({
            "video_path": "out.mp4",
            "scene_wavs": ["/a/scene_1.wav", "/b/scene_2.wav"],
        });
        let (paths, names) = resolve_scene_wavs(&args);
        assert_eq!(paths, vec!["/a/scene_1.wav", "/b/scene_2.wav"]);
        assert_eq!(names, vec!["scene_1", "scene_2"]);
    }

    #[test]
    fn resolve_scene_wavs_parses_manifest() {
        let dir = std::env::temp_dir().join("os_verify_manifest_test");
        std::fs::create_dir_all(&dir).ok();
        let mp = dir.join("manifest.json");
        std::fs::write(
            &mp,
            json!({
                "segments": [
                    {"scene_id": "s1", "wav_path": "/v/s1.wav"},
                    {"scene_id": "s2", "wav_path": "/v/s2.wav"},
                ]
            })
            .to_string(),
        )
        .unwrap();
        let args = json!({"video_path": "out.mp4", "voiceover_manifest": mp.to_string_lossy()});
        let (paths, names) = resolve_scene_wavs(&args);
        assert_eq!(paths, vec!["/v/s1.wav", "/v/s2.wav"]);
        assert_eq!(names, vec!["s1", "s2"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolve_scene_wavs_empty_when_no_input() {
        let (paths, names) = resolve_scene_wavs(&json!({}));
        assert!(paths.is_empty() && names.is_empty());
    }
}

