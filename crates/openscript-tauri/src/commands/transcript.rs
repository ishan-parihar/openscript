#![allow(dead_code)] // Some helpers used by future tasks

use openscript_core::srt;
use openscript_core::timeline::Segment;
use openscript_core::timeline::Timeline;
use openscript_core::transcript::analysis::{detect_filler_words, remove_filler_words};
use openscript_ffmpeg::render::render_from_timeline;
use openscript_transcribe::transcriber::transcribe;
use serde_json::{json, Value};
use std::path::PathBuf;
use tauri::State;

use crate::state::AppState;

/// Transcribe a source video using Apex.
#[tauri::command]
pub async fn transcribe_video(
    state: State<'_, AppState>,
    video_path: String,
    output_srt_path: String,
) -> Result<Value, String> {
    let result = transcribe(&video_path, &output_srt_path)
        .await
        .map_err(|e| format!("Transcription failed: {}", e))?;

    // Store transcript path in active project
    state.with_active_project_mut(|project| {
        project.transcript_path = Some(result.output_path.clone());
    });

    Ok(json!({
        "output_srt_path": result.output_path,
        "entry_count": result.entry_count,
        "word_srt_path": result.word_srt_path,
        "phrase_srt_path": result.phrase_srt_path,
    }))
}

/// Read and parse an SRT file into segments.
#[tauri::command]
pub async fn read_transcript(srt_path: String) -> Result<Value, String> {
    let entries = srt::parse_srt(&srt_path).map_err(|e| format!("Failed to parse SRT: {}", e))?;

    let segments: Vec<Value> = entries
        .iter()
        .map(|e| {
            json!({
                "index": e.idx,
                "start": e.start,
                "end": e.end,
                "text": e.text,
            })
        })
        .collect();

    Ok(json!({ "count": segments.len(), "segments": segments }))
}

/// Group word-level SRT into phrase-level for readable editing.
#[tauri::command]
pub async fn prepare_transcript(
    word_srt_path: String,
    max_words: Option<usize>,
    max_chars: Option<usize>,
) -> Result<Value, String> {
    let entries =
        srt::parse_srt(&word_srt_path).map_err(|e| format!("Failed to parse word SRT: {}", e))?;

    let groups = srt::group_entries(
        &entries,
        max_words.unwrap_or(10),
        max_chars.unwrap_or(64),
        0.6,
    );

    let segments: Vec<Value> = groups
        .iter()
        .map(|(text, start, end)| {
            json!({
                "start": start,
                "end": end,
                "text": text,
            })
        })
        .collect();

    Ok(json!({ "count": segments.len(), "segments": segments }))
}

/// Analyze transcript for filler words.
#[tauri::command]
pub async fn analyze_transcript(srt_path: String) -> Result<Value, String> {
    let entries = srt::parse_srt(&srt_path).map_err(|e| format!("Failed to parse SRT: {}", e))?;

    let segments: Vec<_> = entries
        .iter()
        .map(|e| {
            (
                e.idx.to_string(),
                (e.start * 1000.0) as u64,
                (e.end * 1000.0) as u64,
                e.text.clone(),
            )
        })
        .collect();

    let analysis = detect_filler_words(&segments);

    Ok(json!({
        "filler_word_count": analysis.filler_words.len(),
        "total_words": analysis.total_words,
        "filler_percentage": analysis.filler_percentage,
        "filler_words": analysis.filler_words,
        "segments_analyzed": analysis.segments_analyzed,
    }))
}

/// Remove filler words from the transcript, returning cleaned text.
#[tauri::command]
pub async fn remove_filler_words_from_text(text: String) -> Result<Value, String> {
    let cleaned = remove_filler_words(&text);
    Ok(json!({ "original": text, "cleaned": cleaned }))
}

/// Apply edited SRT to video: build timeline and render.
///
/// `aspect` and `fps` were previously hardcoded to "9:16" and 30 — that
/// broke renders of landscape source footage (16:9) and footage with a
/// non-30 fps cadence (audit bug #21). Both now have sensible defaults
/// but accept caller overrides.
#[tauri::command]
pub async fn apply_transcript_edit(
    _state: State<'_, AppState>,
    video_path: String,
    edited_segments: Vec<Value>,
    output_path: String,
    aspect: Option<String>,
    fps: Option<u32>,
) -> Result<Value, String> {
    // Validate source video exists
    if !PathBuf::from(&video_path).exists() {
        return Err(format!("Source video not found: {}", video_path));
    }

    // Build segments from edited data
    let segments: Vec<Segment> = edited_segments
        .iter()
        .enumerate()
        .filter_map(|(i, seg)| {
            let start = seg["start"].as_f64()?;
            let end = seg["end"].as_f64()?;
            let caption = seg["text"].as_str()?.to_string();
            Some(Segment {
                id: format!("seg_{:03}", i + 1),
                start,
                end,
                caption,
                crossfade_ms: 120,
                semantic_role: None,
            })
        })
        .collect();

    if segments.is_empty() {
        return Err("No valid segments provided".to_string());
    }

    // Resolve aspect ratio: explicit param > "16:9" if the source is landscape
    // (width > height) > "9:16" (the historical default for vertical video).
    // We probe ffprobe-like metadata by asking openscript-ffmpeg for the
    // source video's width/height; if that fails, fall back to "9:16".
    let resolved_aspect = aspect.unwrap_or_else(|| {
        // Cheap heuristic: open the file's first packet to get width/height.
        // If probing fails, assume vertical (the prior default).
        match probe_video_dimensions(&video_path) {
            Some((w, h)) if w > h => "16:9".to_string(),
            _ => "9:16".to_string(),
        }
    });
    let resolved_fps = fps.unwrap_or(30);

    // Build a minimal timeline
    let mut timeline = Timeline::new(
        PathBuf::from(&video_path),
        &resolved_aspect,
        resolved_fps,
        None,
    );
    timeline.segments = segments;

    // Render using FFmpeg (no cancel token for transcript-edit renders)
    let output = render_from_timeline(&timeline, &video_path, Some(&output_path), Some(20))
        .await
        .map_err(|e| format!("Render failed: {}", e))?;

    Ok(json!({
        "output_path": output,
        "segments_count": timeline.segments.len(),
        "aspect": resolved_aspect,
        "fps": resolved_fps,
    }))
}

/// Probe a video file's dimensions (width, height) using ffprobe.
/// Returns None if ffprobe is unavailable or the file cannot be probed;
/// callers fall back to the historical default in that case.
fn probe_video_dimensions(video_path: &str) -> Option<(u32, u32)> {
    let output = std::process::Command::new("ffprobe")
        .args([
            "-v", "error",
            "-select_streams", "v:0",
            "-show_entries", "stream=width,height",
            "-of", "csv=p=0",
            video_path,
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut parts = stdout.trim().split(',');
    let w: u32 = parts.next()?.parse().ok()?;
    let h: u32 = parts.next()?.parse().ok()?;
    Some((w, h))
}
