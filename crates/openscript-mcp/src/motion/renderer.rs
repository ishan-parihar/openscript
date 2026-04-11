use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::fs;
use tokio::process::Command;

#[derive(Debug)]
pub struct RenderArgs {
    pub tsx_code: String,
    pub output_path: Option<String>,
    pub duration_in_frames: u32,
    pub fps: u32,
}

#[derive(Debug)]
pub struct RenderResult {
    pub output_path: String,
    pub duration_ms: u64,
    pub file_size: u64,
    pub frame_count: u32,
    pub warnings: Vec<String>,
}

pub async fn render_motion(args: RenderArgs) -> Result<RenderResult, String> {
    let remotion_root = find_remotion_root()
        .ok_or_else(|| "Could not find remotion/ directory. Set OPENSCRIPT_ROOT or ensure remotion/ exists in an ancestor directory.".to_string())?;

    let composition_path = remotion_root.join("src/compositions/hot-composition.tsx");

    if let Some(parent) = composition_path.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create composition directory: {e}"))?;
    }

    fs::write(&composition_path, &args.tsx_code)
        .await
        .map_err(|e| format!("Failed to write TSX composition: {e}"))?;

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let output_path = match args.output_path {
        Some(path) => PathBuf::from(path),
        None => {
            let project_root = find_openscript_root(&remotion_root).unwrap_or_else(|| {
                remotion_root
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .to_path_buf()
            });
            let artifacts_dir = project_root.join("artifacts");
            fs::create_dir_all(&artifacts_dir)
                .await
                .map_err(|e| format!("Failed to create artifacts directory: {e}"))?;
            artifacts_dir.join(format!("motion_{timestamp}.mp4"))
        }
    };

    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create output directory: {e}"))?;
    }

    let output_str = output_path
        .to_str()
        .ok_or("Output path contains invalid UTF-8")?
        .to_string();

    let render_output = Command::new("npx")
        .args([
            "remotion",
            "render",
            "HotMotion",
            &output_str,
            &format!("--duration={}", args.duration_in_frames),
            &format!("--fps={}", args.fps),
            "--log-level=error",
        ])
        .current_dir(&remotion_root)
        .output()
        .await
        .map_err(|e| format!("Failed to execute remotion render: {e}"))?;

    if !render_output.status.success() {
        let stderr = String::from_utf8_lossy(&render_output.stderr);
        let stdout = String::from_utf8_lossy(&render_output.stdout);
        let error_detail = if !stderr.is_empty() {
            stderr.to_string()
        } else if !stdout.is_empty() {
            stdout.to_string()
        } else {
            format!("Remotion render exited with status: {}", render_output.status)
        };
        return Err(format!("Remotion render failed: {error_detail}"));
    }

    let metadata = fs::metadata(&output_path)
        .await
        .map_err(|e| format!("Render succeeded but cannot read output file: {e}"))?;

    let mut warnings = Vec::new();

    let stderr_str = String::from_utf8_lossy(&render_output.stderr);
    if !stderr_str.is_empty() {
        let truncated = if stderr_str.len() > 500 {
            format!("{}...", &stderr_str[..500])
        } else {
            stderr_str.to_string()
        };
        warnings.push(format!("Remotion CLI stderr: {truncated}"));
    }

    let stdout_str = String::from_utf8_lossy(&render_output.stdout);
    if !stdout_str.is_empty() && stdout_str.to_lowercase().contains("warning") {
        let truncated = if stdout_str.len() > 500 {
            format!("{}...", &stdout_str[..500])
        } else {
            stdout_str.to_string()
        };
        warnings.push(format!("Remotion CLI stdout warning: {truncated}"));
    }

    let (actual_duration_ms, actual_frame_count) =
        match measure_output_with_ffprobe(&output_str).await {
            Ok((duration_ms, frame_count)) => (duration_ms, frame_count),
            Err(e) => {
                warnings.push(format!("Could not measure output with ffprobe: {e}"));
                let fallback_duration_ms =
                    ((args.duration_in_frames as f64) / (args.fps as f64) * 1000.0) as u64;
                (fallback_duration_ms, args.duration_in_frames)
            }
        };

    match detect_silent_audio(&output_str).await {
        Ok(Some(warning)) => warnings.push(warning),
        Ok(None) => {
            if let Ok(false) = has_audio_stream(&output_str).await {
                warnings.push("Output has no audio track.".to_string());
            }
        }
        Err(e) => warnings.push(e),
    }

    Ok(RenderResult {
        output_path: output_str,
        duration_ms: actual_duration_ms,
        file_size: metadata.len(),
        frame_count: actual_frame_count,
        warnings,
    })
}

async fn measure_output_with_ffprobe(
    output_path: &str,
) -> Result<(u64, u32), String> {
    let ffprobe_output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-show_entries",
            "stream=nb_frames",
            "-of",
            "json",
            output_path,
        ])
        .output()
        .await
        .map_err(|e| format!("Failed to run ffprobe: {e}"))?;

    if !ffprobe_output.status.success() {
        return Err(format!(
            "ffprobe exited with status: {}",
            ffprobe_output.status
        ));
    }

    let json_str = String::from_utf8_lossy(&ffprobe_output.stdout);
    let probe: serde_json::Value = serde_json::from_str(&json_str)
        .map_err(|e| format!("Failed to parse ffprobe JSON: {e}"))?;

    let duration_ms = probe
        .get("format")
        .and_then(|f| f.get("duration"))
        .and_then(|d| d.as_str())
        .and_then(|d| d.parse::<f64>().ok())
        .map(|d| (d * 1000.0) as u64)
        .ok_or_else(|| "Could not extract duration from ffprobe output".to_string())?;

    let frame_count = probe
        .get("streams")
        .and_then(|s| s.as_array())
        .and_then(|streams| streams.first())
        .and_then(|stream| stream.get("nb_frames"))
        .and_then(|nf| {
            // nb_frames can be a string "N/A" or a number
            if let Some(s) = nf.as_str() {
                s.parse::<u32>().ok()
            } else {
                nf.as_u64().map(|n| n as u32)
            }
        })
        .unwrap_or_else(|| ((duration_ms as f64) / 1000.0 * 30.0).round() as u32);

    Ok((duration_ms, frame_count))
}

async fn has_audio_stream(output_path: &str) -> Result<bool, String> {
    let ffprobe_output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a",
            "-show_entries",
            "stream=index",
            "-of",
            "csv=p=0",
            output_path,
        ])
        .output()
        .await
        .map_err(|e| format!("Failed to run ffprobe for audio check: {e}"))?;

    if !ffprobe_output.status.success() {
        return Err(format!(
            "ffprobe audio check exited with status: {}",
            ffprobe_output.status
        ));
    }

    let stdout = String::from_utf8_lossy(&ffprobe_output.stdout);
    Ok(!stdout.trim().is_empty())
}

async fn detect_silent_audio(output_path: &str) -> Result<Option<String>, String> {
    match has_audio_stream(output_path).await {
        Ok(false) => return Ok(None),
        Ok(true) => {}
        Err(e) => return Err(e),
    }

    let ffmpeg_output = Command::new("ffmpeg")
        .args([
            "-i",
            output_path,
            "-af",
            "volumedetect",
            "-f",
            "null",
            "-",
        ])
        .output()
        .await
        .map_err(|e| format!("Could not verify audio quality: {e}"))?;

    // volumedetect writes results to stderr, not stdout
    let stderr = String::from_utf8_lossy(&ffmpeg_output.stderr);

    let max_volume: Option<f64> = stderr
        .lines()
        .find(|line| line.contains("max_volume:"))
        .and_then(|line| {
            line.split(':')
                .nth(1)
                .and_then(|v| v.trim().trim_end_matches(" dB").parse::<f64>().ok())
        });

    match max_volume {
        Some(db) if db < -60.0 => Ok(Some(format!(
            "Output contains silent or near-silent audio track (max_volume: {db} dB). Consider adding <Audio> or <OffthreadAudio> elements to your composition."
        ))),
        Some(_) => Ok(None),
        None => Ok(Some(
            "Output contains silent or near-silent audio track (max_volume: unknown dB). Consider adding <Audio> or <OffthreadAudio> elements to your composition.".to_string(),
        )),
    }
}

fn find_remotion_root() -> Option<PathBuf> {
    if let Ok(root) = std::env::var("OPENSCRIPT_ROOT") {
        let candidate = PathBuf::from(&root).join("remotion");
        if candidate.is_dir() {
            return Some(candidate);
        }
    }

    let mut current = std::env::current_dir().ok()?;
    loop {
        let candidate = current.join("remotion");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if !current.pop() {
            break;
        }
    }
    None
}

fn find_openscript_root(remotion_root: &Path) -> Option<PathBuf> {
    remotion_root.parent().map(Path::to_path_buf)
}
