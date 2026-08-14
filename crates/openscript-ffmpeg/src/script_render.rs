//! From-scratch video renderer.
//!
//! Renders a video from a script timeline (background + voiceover + music + captions).
//! Unlike `render_from_timeline` (which is designed for NLE editing of existing footage),
//! this function builds a simpler FFmpeg pipeline for from-scratch compositions:
//!
//! 1. Background video (looped to total duration, scaled/cropped to target aspect)
//! 2. Voiceover audio (concatenated WAVs, one per scene)
//! 3. Optional music (with sidechain ducking during voiceover)
//! 4. Caption burn-in (ASS subtitles)
//! 5. Output: H.264 MP4

use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::gpu::GpuConfig;
use crate::FfmpegError;

/// Specification for a from-scratch render.
pub struct ScriptRenderSpec {
    /// Background video path (will be looped if shorter than total duration).
    pub background_path: String,
    /// Voiceover WAV paths in order (one per scene).
    pub voiceover_paths: Vec<String>,
    /// Optional background music path.
    pub music_path: Option<String>,
    /// Music volume (0.0–1.0).
    pub music_volume: f64,
    /// Whether to duck music during voiceover.
    pub ducking: bool,
    /// Ducking depth in dB.
    pub ducking_depth_db: f64,
    /// Caption ASS file path (optional).
    pub captions_path: Option<String>,
    /// Output width.
    pub width: u32,
    /// Output height.
    pub height: u32,
    /// Output FPS.
    pub fps: u32,
    /// Output path.
    pub output_path: String,
    /// CRF (quality, lower = better).
    pub crf: u32,
    /// FFmpeg preset.
    pub preset: String,
    /// Total duration in seconds.
    pub total_duration_s: f64,
}

/// Render a from-scratch video composition.
///
/// Builds an FFmpeg command with:
/// - Input 0: background video (looped)
/// - Input 1+: voiceover WAVs (concatenated via concat demuxer)
/// - Input N: music (optional)
/// - Filter: scale/crop background, burn captions, mix audio with ducking
pub async fn render_from_script(spec: &ScriptRenderSpec) -> Result<String, FfmpegError> {
    if spec.voiceover_paths.is_empty() {
        return Err(FfmpegError::NoSegments);
    }

    // Build a concat list file for voiceover WAVs (use absolute paths)
    let concat_list_path = format!("{}.concat.txt", spec.output_path);
    let concat_content: String = spec
        .voiceover_paths
        .iter()
        .map(|p| {
            let abs = std::fs::canonicalize(p).unwrap_or_else(|_| std::path::PathBuf::from(p));
            format!("file '{}'\n", abs.to_string_lossy().replace('\'', "'\\''"))
        })
        .collect();
    std::fs::write(&concat_list_path, &concat_content)?;

    // Build the FFmpeg command
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y");

    // Resolve GPU acceleration once (NVENC encode + CUDA decode when
    // available, per OPENSCRIPT_FFMPEG_GPU). Filter graph stays CPU.
    let gpu = GpuConfig::resolve();

    // Input 0: background video (looped)
    cmd.arg("-stream_loop").arg("-1"); // loop infinitely
    cmd.arg("-t").arg(spec.total_duration_s.to_string()); // but only read what we need
    gpu.add_input(&mut cmd);
    cmd.arg("-i").arg(&spec.background_path);

    // Input 1: voiceover concatenation
    cmd.arg("-f").arg("concat");
    cmd.arg("-safe").arg("0");
    gpu.add_input(&mut cmd);
    cmd.arg("-i").arg(&concat_list_path);

    // Input 2: music (optional)
    let has_music = spec.music_path.is_some() && spec.ducking;
    if has_music {
        let music_path = spec.music_path.as_ref().unwrap();
        if std::path::Path::new(music_path).exists() {
            cmd.arg("-stream_loop").arg("-1");
            cmd.arg("-t").arg(spec.total_duration_s.to_string());
            gpu.add_input(&mut cmd);
            cmd.arg("-i").arg(music_path);
        }
    }

    // Build filter complex
    let mut filters: Vec<String> = Vec::new();

    // Video: scale background to target size with crop
    filters.push(format!(
        "[0:v]scale={}:{}:force_original_aspect_ratio=increase,crop={}:{},fps={}[vbg]",
        spec.width, spec.height, spec.width, spec.height, spec.fps
    ));

    // Burn captions if provided
    if let Some(ref captions) = spec.captions_path {
        if std::path::Path::new(captions).exists() {
            let escaped = captions.replace('\\', "/").replace('\'', "'\\''");
            filters.push(format!("[vbg]subtitles='{}'[vcap]", escaped));
        } else {
            filters.push("[vbg]copy[vcap]".to_string());
        }
    } else {
        filters.push("[vbg]copy[vcap]".to_string());
    }

    // Audio: voiceover from input 1
    if has_music {
        // Duck music during voiceover using sidechaincompress
        let music_vol = spec.music_volume;
        let threshold = 0.001_f64.powf(1.0 - spec.ducking_depth_db / 20.0);
        filters.push("[1:a]asplit=2[vo_out][vo_sc]".to_string());
        filters.push(format!("[2:a]volume={}[music_vol]", music_vol));
        filters.push(format!(
            "[music_vol][vo_sc]sidechaincompress=threshold={}:ratio=4:attack=50:release=200:makeup=1:level_sc=1[music_ducked]",
            threshold
        ));
        filters.push("[vo_out][music_ducked]amix=inputs=2:duration=first:dropout_transition=2:normalize=0[aout]".to_string());
    } else {
        // Just voiceover
        filters.push("[1:a]anull[aout]".to_string());
    }

    // Loudness normalization — TP=-2.5 for conservative headroom (P0 audio
    // clipping fix). NOTE (audit 2026-08-13, ffmpeg n9.0): `LRA=11`
    // over-attenuated ducked mixes by ~8 dB; loudnorm TP alone is the limit.
    filters.push("[aout]loudnorm=I=-16:TP=-2.5[aout_norm]".to_string());

    let filter_complex = filters.join(";");

    cmd.arg("-filter_complex").arg(&filter_complex);
    cmd.arg("-map").arg("[vcap]");
    cmd.arg("-map").arg("[aout_norm]");

    // Video codec — NVENC (GPU) or libx264 (CPU) per OPENSCRIPT_FFMPEG_GPU
    gpu.add_encoder(&mut cmd, &spec.preset, spec.crf, spec.fps, false);

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

    // Log path
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

    // Write log
    let _ = std::fs::write(&log_path, &full_stderr);

    // Cleanup concat list
    let _ = std::fs::remove_file(&concat_list_path);

    if !output.success() {
        let stderr_str = String::from_utf8_lossy(&full_stderr);
        // VRAM-pressure fail-soft (same contract as render_multilayer):
        // h264_nvenc encoder init OOM is fatal (resident TTS sidecar on a
        // small card); retry ONCE with CPU libx264 via the env-forced CPU
        // re-resolve inside `render_from_script`'s next `GpuConfig::resolve()`.
        if gpu.active() && crate::gpu::nvenc_oom_failure(&stderr_str) {
            tracing::warn!(
                "[render] GPU encode failed (VRAM pressure) — retrying once with CPU libx264"
            );
            std::env::set_var("OPENSCRIPT_FFMPEG_GPU", "cpu");
            // Box the recursive future (E0733: async recursion requires boxing).
            return Box::pin(render_from_script(spec)).await;
        }
        return Err(FfmpegError::RenderFailed(format!(
            "Render failed, see log: {}\nLast 5 lines: {}",
            log_path,
            stderr_str
                .lines()
                .rev()
                .take(5)
                .collect::<Vec<_>>()
                .join("\n")
        )));
    }

    Ok(spec.output_path.clone())
}
