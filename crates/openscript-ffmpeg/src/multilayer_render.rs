//! Multi-layer from-scratch video renderer.
//!
//! Renders a video with proper temporal layering:
//! - Layer 0: Background video (per-scene clips, concatenated)
//! - Layer 1: Sticker overlays (PNG/GIF at positions)
//! - Layer 2: Captions (ASS subtitles burned in)
//! - Audio 1: Voiceover (concatenated WAVs)
//! - Audio 2: Music (with sidechain ducking)
//!
//! This replaces the single-background `render_from_script` with a proper
//! multi-layer composition engine.

use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::FfmpegError;

/// A background clip with its time range.
#[derive(Debug, Clone)]
pub struct BackgroundClip {
    pub path: String,
    pub duration_s: f64,
    pub looped: bool,
}

/// A sticker overlay with its time range and position.
#[derive(Debug, Clone)]
pub struct StickerOverlay {
    pub path: String,
    pub start_s: f64,
    pub end_s: f64,
    pub position: String, // "top-left", "bottom-right", "center", etc.
    pub scale: f64,       // 0.0-1.0 relative to canvas width
}

/// Multi-layer render specification.
pub struct MultiLayerRenderSpec {
    /// Per-scene background clips (will be concatenated)
    pub backgrounds: Vec<BackgroundClip>,
    /// Voiceover WAV paths in order
    pub voiceover_paths: Vec<String>,
    /// Optional sticker overlays
    pub stickers: Vec<StickerOverlay>,
    /// Optional background music path
    pub music_path: Option<String>,
    /// Music volume (0.0-1.0)
    pub music_volume: f64,
    /// Whether to duck music during voiceover
    pub ducking: bool,
    /// Ducking depth in dB
    pub ducking_depth_db: f64,
    /// Caption ASS file path
    pub captions_path: Option<String>,
    /// Output width
    pub width: u32,
    /// Output height
    pub height: u32,
    /// Output FPS
    pub fps: u32,
    /// Output path
    pub output_path: String,
    /// CRF (quality)
    pub crf: u32,
    /// FFmpeg preset
    pub preset: String,
    /// Total duration in seconds
    pub total_duration_s: f64,
}

/// Parse a position string to (x, y) coordinates.
fn parse_position(position: &str, canvas_w: u32, canvas_h: u32, sticker_w: u32, sticker_h: u32) -> (i32, i32) {
    let margin = 40i32;
    match position {
        "top-left" => (margin, margin),
        "top-right" => (canvas_w as i32 - sticker_w as i32 - margin, margin),
        "top-center" => ((canvas_w as i32 - sticker_w as i32) / 2, margin),
        "bottom-left" => (margin, canvas_h as i32 - sticker_h as i32 - margin),
        "bottom-right" => (canvas_w as i32 - sticker_w as i32 - margin, canvas_h as i32 - sticker_h as i32 - margin),
        "bottom-center" => ((canvas_w as i32 - sticker_w as i32) / 2, canvas_h as i32 - sticker_h as i32 - margin),
        "center" => ((canvas_w as i32 - sticker_w as i32) / 2, (canvas_h as i32 - sticker_h as i32) / 2),
        _ => (margin, margin),
    }
}

/// Render a multi-layer video composition.
///
/// FFmpeg filter graph structure:
/// ```text
/// [bg1][bg2][bg3]concat=n=3[vcat]        — concat backgrounds
/// [vcat]scale,crop[vbg]                  — fit to canvas
/// [vbg]subtitles=captions.ass[vcap]      — burn captions
/// [sticker1]scale[st1]                   — scale sticker
/// [vcap][st1]overlay=x:y:enable=between(t,s,e)[vout]  — overlay sticker
/// [vo1][vo2]concat=n=2[audio_vo]         — concat voiceovers
/// [music]volume[mvol]                    — music volume
/// [audio_vo][mvol]sidechaincompress[amix] — duck + mix
/// ```
pub async fn render_multilayer(spec: &MultiLayerRenderSpec) -> Result<String, FfmpegError> {
    if spec.backgrounds.is_empty() {
        return Err(FfmpegError::NoSegments);
    }

    // Build voiceover concat list
    let concat_list_path = format!("{}.concat.txt", spec.output_path);
    let concat_content: String = spec
        .voiceover_paths
        .iter()
        .map(|p| {
            let abs = std::fs::canonicalize(p)
                .unwrap_or_else(|_| std::path::PathBuf::from(p));
            format!("file '{}'\n", abs.to_string_lossy().replace('\'', "'\\''"))
        })
        .collect();
    std::fs::write(&concat_list_path, &concat_content)?;

    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y");

    // Inputs: one per background clip
    let bg_count = spec.backgrounds.len();
    for bg in &spec.backgrounds {
        if bg.looped {
            cmd.arg("-stream_loop").arg("-1");
        }
        cmd.arg("-t").arg(bg.duration_s.to_string());
        cmd.arg("-i").arg(&bg.path);
    }

    // Input: voiceover concat
    let vo_input_idx = bg_count;
    cmd.arg("-f").arg("concat");
    cmd.arg("-safe").arg("0");
    cmd.arg("-i").arg(&concat_list_path);

    // Input: music
    let music_input_idx = vo_input_idx + 1;
    let has_music = spec.music_path.is_some() && std::path::Path::new(spec.music_path.as_ref().unwrap()).exists();
    if has_music {
        cmd.arg("-stream_loop").arg("-1");
        cmd.arg("-t").arg(spec.total_duration_s.to_string());
        cmd.arg("-i").arg(spec.music_path.as_ref().unwrap());
    }

    // Inputs: sticker overlays
    let mut sticker_input_idx = music_input_idx + 1;
    let mut sticker_inputs = Vec::new();
    for sticker in &spec.stickers {
        if std::path::Path::new(&sticker.path).exists() {
            cmd.arg("-i").arg(&sticker.path);
            sticker_inputs.push((sticker_input_idx, sticker));
            sticker_input_idx += 1;
        }
    }

    // Build filter complex
    let mut filters: Vec<String> = Vec::new();

    // 1. Concat background clips
    if bg_count == 1 {
        // Single background — just scale
        filters.push(format!(
            "[0:v]scale={}:{}:force_original_aspect_ratio=increase,crop={}:{},fps={},trim=duration={}[vbg]",
            spec.width, spec.height, spec.width, spec.height, spec.fps, spec.total_duration_s
        ));
    } else {
        // Multiple backgrounds — concat then scale
        let concat_inputs: String = (0..bg_count)
            .map(|i| format!("[{}:v]", i))
            .collect::<String>();
        let bg_durations: Vec<String> = spec.backgrounds.iter()
            .map(|bg| format!("trim=duration={}", bg.duration_s))
            .collect();

        // Trim each background to its scene duration
        for (i, dur_filter) in bg_durations.iter().enumerate() {
            filters.push(format!("[{}:v]{},setpts=PTS-STARTPTS[bg{}]", i, dur_filter, i));
        }

        // Concat all trimmed backgrounds
        let concat_labels: String = (0..bg_count)
            .map(|i| format!("[bg{}]", i))
            .collect::<String>();
        filters.push(format!(
            "{}concat=n={}[vcat]",
            concat_labels, bg_count
        ));

        // Scale the concatenated video
        filters.push(format!(
            "[vcat]scale={}:{}:force_original_aspect_ratio=increase,crop={}:{},fps={}[vbg]",
            spec.width, spec.height, spec.width, spec.height, spec.fps
        ));
    }

    // 2. Burn captions
    let current_video_label = if let Some(ref captions) = spec.captions_path {
        if std::path::Path::new(captions).exists() {
            let escaped = captions.replace('\\', "/").replace('\'', "'\\''");
            filters.push(format!("[vbg]subtitles='{}'[vcap]", escaped));
            "[vcap]"
        } else {
            "[vbg]"
        }
    } else {
        "[vbg]"
    };

    // 3. Overlay stickers
    let mut video_label = current_video_label.to_string();
    for (idx, (input_idx, sticker)) in sticker_inputs.iter().enumerate() {
        let sticker_size = (spec.width as f64 * sticker.scale) as u32;
        let (x, y) = parse_position(&sticker.position, spec.width, spec.height, sticker_size, sticker_size);

        let st_label = format!("[st{}]", idx);
        let out_label = if idx == sticker_inputs.len() - 1 {
            "[vout]".to_string()
        } else {
            format!("[vst{}]", idx)
        };

        // Scale sticker
        filters.push(format!(
            "[{}:v]scale={}:{}[st{}]",
            input_idx, sticker_size, sticker_size, idx
        ));

        // Overlay with time-based enable
        filters.push(format!(
            "{}{}overlay={}:{}:enable='between(t,{},{})'{}",
            video_label, st_label, x, y, sticker.start_s, sticker.end_s, out_label
        ));

        video_label = out_label;
    }

    // If no stickers, rename vcap to vout
    if sticker_inputs.is_empty() {
        filters.push(format!("{}copy[vout]", current_video_label));
    }

    // 4. Audio: voiceover
    if has_music {
        // Duck music during voiceover
        let threshold = 0.001_f64.powf(1.0 - spec.ducking_depth_db / 20.0);
        filters.push(format!(
            "[{}:a]asplit=2[vo_out][vo_sc]",
            vo_input_idx
        ));
        filters.push(format!(
            "[{}:a]volume={}[music_vol]",
            music_input_idx, spec.music_volume
        ));
        filters.push(format!(
            "[music_vol][vo_sc]sidechaincompress=threshold={}:ratio=4:attack=50:release=200:makeup=1:level_sc=1[music_ducked]",
            threshold
        ));
        filters.push(format!(
            "[vo_out][music_ducked]amix=inputs=2:duration=first:dropout_transition=2:normalize=0[aout_raw]"
        ));
    } else {
        filters.push(format!("[{}:a]anull[aout_raw]", vo_input_idx));
    }

    // Loudness normalization
    filters.push("[aout_raw]loudnorm=I=-16:TP=-1.5:LRA=11[aout]".to_string());

    let filter_complex = filters.join(";");

    cmd.arg("-filter_complex").arg(&filter_complex);
    cmd.arg("-map").arg("[vout]");
    cmd.arg("-map").arg("[aout]");

    // Video codec
    cmd.arg("-c:v").arg("libx264");
    cmd.arg("-preset").arg(&spec.preset);
    cmd.arg("-crf").arg(spec.crf.to_string());
    cmd.arg("-pix_fmt").arg("yuv420p");
    cmd.arg("-r").arg(spec.fps.to_string());

    // Audio codec
    cmd.arg("-c:a").arg("aac");
    cmd.arg("-b:a").arg("160k");

    // Duration
    cmd.arg("-t").arg(spec.total_duration_s.to_string());

    // Output
    cmd.arg(&spec.output_path);

    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let log_path = format!("{}.render.log", spec.output_path);

    let mut child = cmd.spawn().map_err(|e| {
        let _ = std::fs::remove_file(&concat_list_path);
        FfmpegError::RenderFailed(format!("Failed to spawn ffmpeg: {}", e))
    })?;

    let stderr = child.stderr.take().expect("stderr was piped");
    let mut reader = BufReader::new(stderr);
    let mut line = String::new();
    let mut full_stderr = Vec::new();

    while reader.read_line(&mut line).await.unwrap_or(0) > 0 {
        full_stderr.extend_from_slice(line.as_bytes());
        line.clear();
    }

    let output = child.wait().await.map_err(|e| {
        let _ = std::fs::remove_file(&concat_list_path);
        FfmpegError::RenderFailed(format!("Failed to wait for ffmpeg: {}", e))
    })?;

    let _ = std::fs::write(&log_path, &full_stderr);
    let _ = std::fs::remove_file(&concat_list_path);

    if !output.success() {
        let stderr_str = String::from_utf8_lossy(&full_stderr);
        return Err(FfmpegError::RenderFailed(format!(
            "Render failed, see log: {}\nLast 5 lines: {}",
            log_path,
            stderr_str.lines().rev().take(5).collect::<Vec<_>>().join("\n")
        )));
    }

    Ok(spec.output_path.clone())
}
