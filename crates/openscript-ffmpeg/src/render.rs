use std::process::Stdio;
use std::sync::atomic::AtomicBool;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::filter_graph::FilterGraphBuilder;
use crate::gpu::GpuConfig;
use crate::FfmpegError;
use openscript_core::timeline::{Segment, Timeline};

/// Parse FFmpeg's `time=HH:MM:SS.ms` stderr output into milliseconds.
///
/// FFmpeg emits progress lines like:
/// `time=00:00:05.23 bitrate=1234.5kbits/s speed=1.23x`
fn parse_ffmpeg_time_to_ms(time_str: &str) -> Option<i64> {
    // Expected format: "HH:MM:SS.ms"
    let parts: Vec<&str> = time_str.split(':').collect();
    if parts.len() != 3 {
        return None;
    }

    let hours: i64 = parts[0].parse().ok()?;
    let minutes: i64 = parts[1].parse().ok()?;

    // Seconds part may include decimal: "05.23"
    let sec_parts: Vec<&str> = parts[2].split('.').collect();
    let seconds: i64 = sec_parts.first()?.parse().ok()?;
    let millis: i64 = if sec_parts.len() > 1 {
        // Pad or truncate to 3 digits for milliseconds
        let frac = sec_parts[1];
        if frac.len() >= 3 {
            frac[..3].parse().ok()?
        } else {
            format!("{:0<3}", frac).parse().ok()?
        }
    } else {
        0
    };

    Some(hours * 3_600_000 + minutes * 60_000 + seconds * 1000 + millis)
}

/// Spawn FFmpeg as a child process, parse stderr for progress, and wait for completion.
///
/// Returns the output path on success. On failure, writes the log and returns the log path.
///
/// If `cancel_token` is `Some`, the function polls the token between stderr reads; if the
/// token becomes `true`, the child process is killed and `FfmpegError::RenderFailed` is
/// returned with a "cancelled by user" message.
async fn spawn_ffmpeg_with_progress(
    mut cmd: Command,
    total_duration_ms: i64,
    log_path: &str,
    cancel_token: Option<&AtomicBool>,
) -> Result<String, FfmpegError> {
    let mut child = cmd
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true) // Prevent orphaned ffmpeg on MCP client disconnect
        .spawn()?;

    let stderr = child.stderr.take().expect("stderr was piped above");
    let mut reader = BufReader::new(stderr);
    let mut line = String::new();
    let mut last_progress: f64 = 0.0;
    let mut full_stderr = Vec::new();

    loop {
        // Check cancellation before each read. If cancelled, kill the child and bail.
        if let Some(token) = cancel_token {
            if token.load(std::sync::atomic::Ordering::SeqCst) {
                // Best-effort kill; ignore the result because the child may have exited already.
                let _ = child.start_kill();
                let _ = child.wait().await;
                std::fs::write(log_path, "Render cancelled by user\n")?;
                return Err(FfmpegError::RenderFailed("cancelled by user".to_string()));
            }
        }

        // tokio::select! races the next stderr line against a 200ms timeout so we can
        // re-check the cancel token. ffmpeg emits progress roughly every 100ms-1s.
        let read_result = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            reader.read_line(&mut line),
        )
        .await;

        match read_result {
            Ok(Ok(0)) => break, // EOF
            Ok(Ok(_n)) => {
                // Collect stderr for error reporting
                full_stderr.extend_from_slice(line.as_bytes());

                // Parse "time=HH:MM:SS.ms" pattern from FFmpeg stderr
                if let Some(rest) = line.strip_prefix("time=") {
                    if let Some(time_str) = rest.split_whitespace().next() {
                        if let Some(current_ms) = parse_ffmpeg_time_to_ms(time_str) {
                            if total_duration_ms > 0 {
                                last_progress = (current_ms as f64 / total_duration_ms as f64
                                    * 100.0)
                                    .min(100.0);
                            }
                        }
                    }
                }
                line.clear();
            }
            Ok(Err(_e)) => break,
            Err(_elapsed) => {
                // Timeout — loop back to check cancellation. Continue reading.
                continue;
            }
        }
    }

    let status = child.wait().await?;
    if !status.success() {
        std::fs::write(
            log_path,
            format!(
                "Exit: {:?}\nProgress: {:.1}%\nStderr: {}",
                status,
                last_progress,
                String::from_utf8_lossy(&full_stderr)
            ),
        )?;
        return Err(FfmpegError::RenderFailed(log_path.to_string()));
    }

    // Progress reached 100% on successful completion
    Ok(String::new())
}

pub struct RenderConfig {
    pub video_path: String,
    pub edl_path: String,
    pub burn_captions: bool,
    pub srt_path: Option<String>,
    pub ass_path: Option<String>,
    pub overlay_mov: Option<String>,
    pub aspect: String,
    pub crf: u32,
    pub fps: u32,
}

pub async fn render(config: RenderConfig) -> Result<String, FfmpegError> {
    render_with_cancel(config, None).await
}

pub async fn render_with_cancel(
    config: RenderConfig,
    cancel_token: Option<&AtomicBool>,
) -> Result<String, FfmpegError> {
    // Load EDL
    let edl_data = std::fs::read_to_string(&config.edl_path)?;
    let edl: serde_json::Value = serde_json::from_str(&edl_data)?;
    let segments: Vec<Segment> = serde_json::from_value(edl["segments"].clone())?;

    if segments.is_empty() {
        return Err(FfmpegError::NoSegments);
    }

    // Build filter graph (handles xfade, subtitles, overlay, loudnorm internally)
    let loudnorm = true;
    let mut builder =
        FilterGraphBuilder::new(segments.clone(), config.fps, &config.aspect, loudnorm);
    if config.burn_captions {
        if let Some(ass) = &config.ass_path {
            builder = builder.with_ass(ass.clone());
        } else if let Some(srt) = &config.srt_path {
            builder = builder.with_srt(srt.clone());
        }
    }
    if let Some(overlay) = &config.overlay_mov {
        builder = builder.with_overlay_mov(overlay.clone());
    }
    let (filter_complex, vout, aout) = builder.build();

    // Output path
    let edl_p = std::path::Path::new(&config.edl_path);
    let out_path = edl_p
        .with_extension("reel.mp4")
        .to_string_lossy()
        .to_string();
    let log_path = edl_p
        .with_extension("render.log")
        .to_string_lossy()
        .to_string();

    // Calculate total duration from segments
    let total_duration_ms: i64 = segments
        .iter()
        .map(|s| ((s.end - s.start) * 1000.0) as i64)
        .sum();

    // Build ffmpeg command — only single input needed (overlay loaded via movie filter)
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y");
    let gpu = GpuConfig::resolve();
    gpu.add_input(&mut cmd);
    cmd.arg("-i").arg(&config.video_path);
    cmd.arg("-filter_complex").arg(&filter_complex);
    cmd.arg("-map").arg(&vout);
    cmd.arg("-map").arg(&aout);
    // Video codec — NVENC (GPU) or libx264 (CPU) per OPENSCRIPT_FFMPEG_GPU
    gpu.add_encoder(&mut cmd, "medium", config.crf, config.fps, true);
    cmd.arg("-c:a").arg("aac");
    cmd.arg("-b:a").arg("160k");
    cmd.arg("-movflags").arg("+faststart");
    // Same shortest-stream cap as render_from_timeline: the audio is the master
    // clock; the video must never run past it (black/silent tail regression).
    cmd.arg("-shortest");
    cmd.arg(&out_path);

    // Execute with progress parsing
    spawn_ffmpeg_with_progress(cmd, total_duration_ms, &log_path, cancel_token).await?;

    Ok(out_path)
}

/// Render a complete timeline with all tracks (b-roll, music, SFX, captions).
/// This is the high-level entry point that bridges the timeline schema to FFmpeg.
pub async fn render_from_timeline(
    timeline: &Timeline,
    source_video: &str,
    output_path: Option<&str>,
    crf: Option<u32>,
) -> Result<String, FfmpegError> {
    render_from_timeline_with_cancel(timeline, source_video, output_path, crf, None).await
}

/// Same as `render_from_timeline` but accepts an optional cancellation token.
/// When the token becomes `true`, the ffmpeg child process is killed and the
/// render returns `FfmpegError::RenderFailed("cancelled by user")`.
pub async fn render_from_timeline_with_cancel(
    timeline: &Timeline,
    source_video: &str,
    output_path: Option<&str>,
    crf: Option<u32>,
    cancel_token: Option<&AtomicBool>,
) -> Result<String, FfmpegError> {
    if timeline.segments.is_empty() {
        return Err(FfmpegError::NoSegments);
    }

    // Build filter graph from timeline — extracts all track events automatically
    let mut builder = FilterGraphBuilder::from_timeline(timeline);

    // Probe the source so we can detect audio-only inputs. Without this
    // detection, an mp3/wav source causes the filter graph to reference
    // `[0:v]` (which doesn't exist) and ffmpeg fails with
    // "Stream specifier ':v' in filtergraph description ... matches no
    // streams". The builder's `with_audio_only(true)` path synthesizes a
    // solid-color video background via `color=` so b-roll + captions still
    // render normally. Probe failures fall back to the legacy path (which
    // works for any video+audio source — only audio-only is the failure mode).
    if let Ok(metrics) = crate::probe::probe(source_video).await {
        if metrics.width.is_none() || metrics.height.is_none() {
            tracing::info!(
                "[render] Source has no video stream — using audio-only filter path with solid-color background"
            );
            builder = builder.with_audio_only(true);

            // MP3 (and other sequential codecs) lack a seek index, so 44
            // `[0:a]atrim` filters force ffmpeg to decode the audio
            // sequentially from time 0 each time — total decode wall time is
            // O(n × duration). Pre-decoding to WAV in a single fast pass
            // gives random access and turns the atrim cost into O(duration)
            // total. The WAV is written next to the source as `.decoded.wav`
            // and reused if already present. ~140s of audio at 44.1kHz mono
            // is ~12 MB — small enough to keep on disk.
            let decoded_wav = format!("{}.decoded.wav", source_video);
            if !std::path::Path::new(&decoded_wav).exists() {
                let status = Command::new("ffmpeg")
                    .args([
                        "-y",
                        "-i",
                        source_video,
                        "-c:a",
                        "pcm_s16le",
                        "-ar",
                        "44100",
                        &decoded_wav,
                    ])
                    .status()
                    .await;
                match status {
                    Ok(s) if s.success() => {
                        tracing::info!(
                            "[render] Pre-decoded audio to {} for fast atrim access",
                            decoded_wav
                        );
                    }
                    Ok(s) => {
                        return Err(FfmpegError::RenderFailed(format!(
                            "pre-decode failed with exit {:?}",
                            s.code()
                        )));
                    }
                    Err(e) => {
                        return Err(FfmpegError::RenderFailed(format!(
                            "pre-decode spawn failed: {}",
                            e
                        )));
                    }
                }
            }
        }
    }

    // Add subtitle/caption sources
    let burn_captions = timeline.effects.burn_captions;
    if burn_captions {
        // Set fonts directory for subtitle rendering (Bebas Neue)
        if let Ok(fonts_dir) = std::env::var("OPENSCRIPT_FONTS_DIR") {
            builder = builder.with_fonts_dir(fonts_dir);
        } else {
            let workspace_fonts = std::env::current_dir()
                .ok()
                .map(|d| d.join("mcp/fonts"))
                .filter(|p| p.exists());
            if let Some(fd) = workspace_fonts {
                builder = builder.with_fonts_dir(fd.to_string_lossy().to_string());
            } else {
                tracing::warn!("No fonts directory found (OPENSCRIPT_FONTS_DIR not set, mcp/fonts not found) — captions with custom fonts will use system default");
            }
        }

        // Check assets for generated caption files
        if let Some(subtitle_asset) = timeline
            .assets
            .captions
            .get("ass")
            .and_then(|v| v.get("path").and_then(|p| p.as_str()))
        {
            if subtitle_asset.ends_with(".ass") {
                builder = builder.with_ass(subtitle_asset.to_string());
            } else if subtitle_asset.ends_with(".srt") {
                builder = builder.with_srt(subtitle_asset.to_string());
            }
        } else if let Some(subtitle_asset) = timeline
            .assets
            .captions
            .get("srt")
            .and_then(|v| v.get("path").and_then(|p| p.as_str()))
        {
            if subtitle_asset.ends_with(".srt") {
                builder = builder.with_srt(subtitle_asset.to_string());
            }
        }

        // Also check for overlay MOV (PupCaps animated captions)
        if let Some(overlay_asset) = timeline
            .assets
            .captions
            .get("overlay_mov")
            .and_then(|v| v.get("path").and_then(|p| p.as_str()))
        {
            if overlay_asset.ends_with(".mov") || overlay_asset.ends_with(".mp4") {
                builder = builder.with_overlay_mov(overlay_asset.to_string());
            }
        }
    }

    // Probe each b-roll source so the filter graph can cap seek_offset at
    // `src_dur - seg_dur`. Without this, a short source (e.g. 7s) with a
    // long overlay (e.g. 12s) gets a seek_offset that exhausts the source
    // mid-segment, and the movie= filter holds the last frame for the
    // remaining seconds (visible as a static image on the rendered video).
    // Probe failures are non-fatal — the builder falls back to the
    // conservative 3-play loop + small seek cap.
    //
    // NOTE: assets are keyed by `asset_id` (e.g. `broll_0`), NOT by the
    // event's `id` (e.g. `broll_001`). Looking up by `evt.id` silently
    // misses every asset, leaving `source_duration_s` unset for all events
    // and defeating the whole probe. (Phase 128 fixed the same bug in
    // FilterGraphBuilder::from_timeline but missed this probe loop.)
    if let Some(broll_track) = timeline.tracks.get(&openscript_core::types::TrackType::Broll) {
        let mut probed = std::collections::HashMap::new();
        for evt in broll_track.iter() {
            if let openscript_core::timeline::EventKind::Broll { .. } = &evt.kind {
                // Robust asset resolution mirroring FilterGraphBuilder::
                // from_timeline — accept both registry conventions:
                // (a) asset_id == registry key, (b) asset_id == file path with
                // the registry keyed by event_id. The old direct-key lookup
                // silently missed broll.assign-style events, so their clips
                // never got duration-capped seek offsets (short-clip held
                // frames on V2V alternated timelines).
                let record = timeline
                    .assets
                    .broll
                    .get(&evt.asset_id)
                    .or_else(|| {
                        timeline.assets.broll.values().find(|v| {
                            v.get("path")
                                .and_then(|p| p.as_str())
                                .map(|p| p == evt.asset_id)
                                .unwrap_or(false)
                        })
                    });
                let path = record
                    .and_then(|v| v.get("path").and_then(|p| p.as_str()))
                    .unwrap_or(&evt.asset_id)
                    .to_string();
                if !path.is_empty() && path != "placeholder" && !probed.contains_key(&path) {
                    match crate::probe::probe(&path).await {
                        Ok(metrics) if metrics.duration > 0.0 => {
                            tracing::debug!(
                                "[render] broll probe ok: {} ({:.2}s)",
                                path,
                                metrics.duration
                            );
                            probed.insert(path.clone(), metrics.duration);
                        }
                        Ok(metrics) => {
                            tracing::debug!(
                                "[render] broll probe duration<=0 ({:.2}s): {}",
                                metrics.duration,
                                path
                            );
                        }
                        Err(e) => {
                            tracing::debug!("[render] broll probe failed for {}: {}", path, e);
                        }
                    }
                }
            }
        }
        if !probed.is_empty() {
            builder = builder.with_broll_durations(probed);
        }
    }

    let (filter_complex, vout, aout) = builder.build();

    // DEBUG: dump filter graph for b-roll investigation
    std::fs::write("/tmp/debug_filter_graph.txt", format!("VOUT={}\nAOUT={}\n\nFILTER_COMPLEX:\n{}", vout, aout, filter_complex)).ok();

    // Determine output path
    let out_path = match output_path {
        Some(p) => p.to_string(),
        None => timeline
            .source
            .with_extension("reel.mp4")
            .to_string_lossy()
            .to_string(),
    };

    // Defense-in-depth: create the output parent dir if missing, so a fresh
    // output_path (e.g. output/videos/v3/foo.mp4) can't fail with a cryptic
    // "No such file or directory" after all prep work is done.
    if let Some(parent) = std::path::Path::new(&out_path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let log_path = std::path::Path::new(&out_path)
        .with_extension("render.log")
        .to_string_lossy()
        .to_string();

    let used_crf = crf.unwrap_or(20);

    // Calculate total duration from timeline segments
    let total_duration_ms: i64 = timeline
        .segments
        .iter()
        .map(|s| ((s.end - s.start) * 1000.0) as i64)
        .sum();

    // Build ffmpeg command. When audio-only pre-decode ran, the decoded WAV
    // (which has random access) is the faster input — atrim cost drops from
    // O(n × duration) to O(duration).
    let decoded_wav = format!("{}.decoded.wav", source_video);
    let ffmpeg_input = if std::path::Path::new(&decoded_wav).exists() {
        decoded_wav.as_str()
    } else {
        source_video
    };
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y");
    let gpu = GpuConfig::resolve();
    gpu.add_input(&mut cmd);
    cmd.arg("-i").arg(ffmpeg_input);
    cmd.arg("-filter_complex").arg(&filter_complex);
    cmd.arg("-map").arg(&vout);
    cmd.arg("-map").arg(&aout);
    // Video codec — NVENC (GPU) or libx264 (CPU) per OPENSCRIPT_FFMPEG_GPU
    gpu.add_encoder(&mut cmd, "medium", used_crf, timeline.target.fps, true);
    cmd.arg("-c:a").arg("aac");
    cmd.arg("-b:a").arg("160k");
    // `-shortest` caps the output at the end of the SHORTEST stream. Without it,
    // an overlay with eof_action=repeat (b-roll, caption overlays) extends the
    // video stream past the source audio end, producing a black/silent tail
    // (regression: A2V renders ran 2:41 for a 2:15 source). The audio is the
    // master clock — video must never outlive it.
    cmd.arg("-shortest");
    cmd.arg(&out_path);

    // Execute with progress parsing
    spawn_ffmpeg_with_progress(cmd, total_duration_ms, &log_path, cancel_token).await?;

    Ok(out_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_config_fields() {
        let config = RenderConfig {
            video_path: "test.mp4".into(),
            edl_path: "test.edl.json".into(),
            burn_captions: true,
            srt_path: None,
            ass_path: None,
            overlay_mov: None,
            aspect: "9:16".into(),
            crf: 20,
            fps: 30,
        };
        assert_eq!(config.video_path, "test.mp4");
        assert_eq!(config.crf, 20);
    }

    #[test]
    fn test_parse_ffmpeg_time_basic() {
        assert_eq!(parse_ffmpeg_time_to_ms("00:00:00.00"), Some(0));
        assert_eq!(parse_ffmpeg_time_to_ms("00:00:01.00"), Some(1000));
        assert_eq!(parse_ffmpeg_time_to_ms("00:00:05.230"), Some(5230));
        assert_eq!(parse_ffmpeg_time_to_ms("00:01:00.00"), Some(60_000));
        assert_eq!(parse_ffmpeg_time_to_ms("00:01:30.500"), Some(90_500));
        assert_eq!(parse_ffmpeg_time_to_ms("01:00:00.00"), Some(3_600_000));
        assert_eq!(parse_ffmpeg_time_to_ms("01:02:03.456"), Some(3_723_456));
    }

    #[test]
    fn test_parse_ffmpeg_time_short_frac() {
        assert_eq!(parse_ffmpeg_time_to_ms("00:00:01.5"), Some(1500));
        assert_eq!(parse_ffmpeg_time_to_ms("00:00:01.23"), Some(1230));
    }

    #[test]
    fn test_parse_ffmpeg_time_no_frac() {
        assert_eq!(parse_ffmpeg_time_to_ms("00:00:05"), Some(5000));
    }

    #[test]
    fn test_parse_ffmpeg_time_invalid() {
        assert_eq!(parse_ffmpeg_time_to_ms(""), None);
        assert_eq!(parse_ffmpeg_time_to_ms("abc"), None);
        assert_eq!(parse_ffmpeg_time_to_ms("00:00"), None);
        assert_eq!(parse_ffmpeg_time_to_ms("00:00:00:00"), None);
        assert_eq!(parse_ffmpeg_time_to_ms("00:xx:00.00"), None);
    }
}
