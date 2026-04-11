#![allow(dead_code)] // Some helpers used by future tasks

use openscript_core::srt;
use openscript_core::transcript::analysis::{detect_filler_words, remove_filler_words};
use openscript_core::timeline::Timeline;
use openscript_core::timeline::Segment;
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
    let entries = srt::parse_srt(&srt_path)
        .map_err(|e| format!("Failed to parse SRT: {}", e))?;

    let segments: Vec<Value> = entries.iter().map(|e| {
        json!({
            "index": e.idx,
            "start": e.start,
            "end": e.end,
            "text": e.text,
        })
    }).collect();

    Ok(json!({ "count": segments.len(), "segments": segments }))
}

/// Group word-level SRT into phrase-level for readable editing.
#[tauri::command]
pub async fn prepare_transcript(
    word_srt_path: String,
    max_words: Option<usize>,
    max_chars: Option<usize>,
) -> Result<Value, String> {
    let entries = srt::parse_srt(&word_srt_path)
        .map_err(|e| format!("Failed to parse word SRT: {}", e))?;

    let groups = srt::group_entries(&entries, max_words.unwrap_or(10), max_chars.unwrap_or(64), 0.6);

    let segments: Vec<Value> = groups.iter().map(|(text, start, end)| {
        json!({
            "start": start,
            "end": end,
            "text": text,
        })
    }).collect();

    Ok(json!({ "count": segments.len(), "segments": segments }))
}

/// Analyze transcript for filler words.
#[tauri::command]
pub async fn analyze_transcript(srt_path: String) -> Result<Value, String> {
    let entries = srt::parse_srt(&srt_path)
        .map_err(|e| format!("Failed to parse SRT: {}", e))?;

    let segments: Vec<_> = entries.iter()
        .map(|e| (e.idx.to_string(), (e.start * 1000.0) as u64, (e.end * 1000.0) as u64, e.text.clone()))
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
#[tauri::command]
pub async fn apply_transcript_edit(
    _state: State<'_, AppState>,
    video_path: String,
    edited_segments: Vec<Value>,
    output_path: String,
) -> Result<Value, String> {
    // Validate source video exists
    if !PathBuf::from(&video_path).exists() {
        return Err(format!("Source video not found: {}", video_path));
    }

    // Build segments from edited data
    let segments: Vec<Segment> = edited_segments.iter().enumerate().filter_map(|(i, seg)| {
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
    }).collect();

    if segments.is_empty() {
        return Err("No valid segments provided".to_string());
    }

    // Build a minimal timeline
    let mut timeline = Timeline::new(
        PathBuf::from(&video_path),
        "9:16",
        30,
        None,
    );
    timeline.segments = segments;

    // Render using FFmpeg
    let output = render_from_timeline(
        &timeline,
        &video_path,
        Some(&output_path),
        Some(20),
    ).await.map_err(|e| format!("Render failed: {}", e))?;

    Ok(json!({
        "output_path": output,
        "segments_count": timeline.segments.len(),
    }))
}
