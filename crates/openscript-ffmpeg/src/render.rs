use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::filter_graph::FilterGraphBuilder;
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
async fn spawn_ffmpeg_with_progress(
    mut cmd: Command,
    total_duration_ms: i64,
    log_path: &str,
) -> Result<String, FfmpegError> {
    let mut child = cmd
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)  // Prevent orphaned ffmpeg on MCP client disconnect
        .spawn()?;

    let stderr = child.stderr.take().expect("stderr was piped above");
    let mut reader = BufReader::new(stderr);
    let mut line = String::new();
    let mut last_progress: f64 = 0.0;
    let mut full_stderr = Vec::new();

    while reader.read_line(&mut line).await? > 0 {
        // Collect stderr for error reporting
        full_stderr.extend_from_slice(line.as_bytes());

        // Parse "time=HH:MM:SS.ms" pattern from FFmpeg stderr
        if let Some(rest) = line.strip_prefix("time=") {
            if let Some(time_str) = rest.split_whitespace().next() {
                if let Some(current_ms) = parse_ffmpeg_time_to_ms(time_str) {
                    if total_duration_ms > 0 {
                        last_progress =
                            (current_ms as f64 / total_duration_ms as f64 * 100.0).min(100.0);
                    }
                }
            }
        }
        line.clear();
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
    // Load EDL
    let edl_data = std::fs::read_to_string(&config.edl_path)?;
    let edl: serde_json::Value = serde_json::from_str(&edl_data)?;
    let segments: Vec<Segment> = serde_json::from_value(edl["segments"].clone())?;

    if segments.is_empty() {
        return Err(FfmpegError::NoSegments);
    }

    // Build filter graph (handles xfade, subtitles, overlay, loudnorm internally)
    let loudnorm = true;
    let mut builder = FilterGraphBuilder::new(segments.clone(), config.fps, &config.aspect, loudnorm);
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
    cmd.arg("-y").arg("-i").arg(&config.video_path);
    cmd.arg("-filter_complex").arg(&filter_complex);
    cmd.arg("-map").arg(&vout);
    cmd.arg("-map").arg(&aout);
    cmd.arg("-c:v").arg("libx264");
    cmd.arg("-profile:v").arg("high");
    cmd.arg("-pix_fmt").arg("yuv420p");
    cmd.arg("-crf").arg(config.crf.to_string());
    cmd.arg("-r").arg(config.fps.to_string());
    cmd.arg("-c:a").arg("aac");
    cmd.arg("-b:a").arg("160k");
    cmd.arg(&out_path);

    // Execute with progress parsing
    spawn_ffmpeg_with_progress(cmd, total_duration_ms, &log_path).await?;

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
    if timeline.segments.is_empty() {
        return Err(FfmpegError::NoSegments);
    }

    // Build filter graph from timeline — extracts all track events automatically
    let mut builder = FilterGraphBuilder::from_timeline(timeline);

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

    let (filter_complex, vout, aout) = builder.build();

    // Determine output path
    let out_path = match output_path {
        Some(p) => p.to_string(),
        None => timeline
            .source
            .with_extension("reel.mp4")
            .to_string_lossy()
            .to_string(),
    };

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

    // Build ffmpeg command
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y").arg("-i").arg(source_video);
    cmd.arg("-filter_complex").arg(&filter_complex);
    cmd.arg("-map").arg(&vout);
    cmd.arg("-map").arg(&aout);
    cmd.arg("-c:v").arg("libx264");
    cmd.arg("-profile:v").arg("high");
    cmd.arg("-pix_fmt").arg("yuv420p");
    cmd.arg("-crf").arg(used_crf.to_string());
    cmd.arg("-r").arg(timeline.target.fps.to_string());
    cmd.arg("-c:a").arg("aac");
    cmd.arg("-b:a").arg("160k");
    cmd.arg(&out_path);

    // Execute with progress parsing
    spawn_ffmpeg_with_progress(cmd, total_duration_ms, &log_path).await?;

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
