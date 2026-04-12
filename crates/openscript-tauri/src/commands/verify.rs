#![allow(dead_code)]

use serde_json::{json, Value};
use std::process::Command;

/// Verify audio file properties using ffprobe.
#[tauri::command]
pub async fn verify_audio(file_path: String) -> Result<Value, String> {
    if !std::path::Path::new(&file_path).exists() {
        return Ok(json!({
            "passed": false,
            "issues": ["File not found"],
            "codec": null,
            "sample_rate": null,
            "channels": null,
            "duration_s": null,
        }));
    }

    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_streams",
            "-show_format",
            &file_path,
        ])
        .output()
        .map_err(|e| format!("Failed to run ffprobe: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Ok(json!({
            "passed": false,
            "issues": [format!("ffprobe failed: {}", stderr)],
            "codec": null,
            "sample_rate": null,
            "channels": null,
            "duration_s": null,
        }));
    }

    let probe: Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Failed to parse ffprobe output: {}", e))?;

    let streams = probe
        .get("streams")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let audio_stream = streams
        .iter()
        .find(|s| s.get("codec_type").and_then(|v| v.as_str()) == Some("audio"));

    let mut issues: Vec<String> = Vec::new();

    let codec = audio_stream
        .and_then(|s| s.get("codec_name").and_then(|v| v.as_str()))
        .map(String::from);

    let sample_rate = audio_stream
        .and_then(|s| s.get("sample_rate").and_then(|v| v.as_str()))
        .and_then(|v| v.parse::<u32>().ok());

    let channels = audio_stream
        .and_then(|s| s.get("channels").and_then(|v| v.as_u64()))
        .map(|v| v as u32);

    let duration_s = probe
        .get("format")
        .and_then(|f| f.get("duration").and_then(|v| v.as_f64()));

    if audio_stream.is_none() {
        issues.push("No audio stream found".to_string());
    }

    if let Some(sr) = sample_rate {
        if sr < 16000 {
            issues.push(format!("Sample rate too low: {} Hz (min 16000)", sr));
        }
    }

    if let Some(ch) = channels {
        if ch > 2 {
            issues.push(format!("Too many channels: {} (max 2 for stereo)", ch));
        }
    }

    if let Some(dur) = duration_s {
        if dur < 0.1 {
            issues.push(format!("Audio too short: {:.2}s", dur));
        }
    }

    let passed = issues.is_empty();

    Ok(json!({
        "passed": passed,
        "issues": issues,
        "codec": codec,
        "sample_rate": sample_rate,
        "channels": channels,
        "duration_s": duration_s,
    }))
}

/// Verify caption/subtitle streams in a media file using ffprobe.
#[tauri::command]
pub async fn verify_captions(file_path: String) -> Result<Value, String> {
    if !std::path::Path::new(&file_path).exists() {
        return Ok(json!({
            "passed": false,
            "issues": ["File not found"],
            "has_captions": false,
            "codec": null,
        }));
    }

    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_streams",
            &file_path,
        ])
        .output()
        .map_err(|e| format!("Failed to run ffprobe: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Ok(json!({
            "passed": false,
            "issues": [format!("ffprobe failed: {}", stderr)],
            "has_captions": false,
            "codec": null,
        }));
    }

    let probe: Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Failed to parse ffprobe output: {}", e))?;

    let streams = probe
        .get("streams")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let subtitle_stream = streams.iter().find(|s| {
        let codec_type = s
            .get("codec_type")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        codec_type == "subtitle" || codec_type == "subp"
    });

    let mut issues: Vec<String> = Vec::new();

    let has_captions = subtitle_stream.is_some();

    let codec = subtitle_stream
        .and_then(|s| s.get("codec_name").and_then(|v| v.as_str()))
        .map(String::from);

    if !has_captions {
        issues.push("No caption/subtitle stream found".to_string());
    }

    let passed = has_captions;

    Ok(json!({
        "passed": passed,
        "issues": issues,
        "has_captions": has_captions,
        "codec": codec,
    }))
}
