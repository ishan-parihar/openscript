use openscript_ffmpeg::render::render_from_timeline;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::LazyLock;
use tauri::State;

use crate::state::AppState;

static RENDER_IN_PROGRESS: AtomicBool = AtomicBool::new(false);
static RENDER_PROGRESS: AtomicU8 = AtomicU8::new(0);
static RENDER_CANCELLED: AtomicBool = AtomicBool::new(false);

static RENDER_LOG: LazyLock<std::sync::Mutex<String>> =
    LazyLock::new(|| std::sync::Mutex::new(String::new()));

fn quality_to_crf(quality: &str) -> u32 {
    match quality {
        "preview" => 20,
        "high" => 0,
        _ => 30,
    }
}

#[tauri::command]
pub async fn reelize_timeline(
    state: State<'_, AppState>,
    video_path: String,
) -> Result<Value, String> {
    render_timeline(state, format!("{}.reel.mp4", video_path), "standard".to_string()).await
}

#[tauri::command]
pub async fn render_timeline(
    state: State<'_, AppState>,
    output_path: String,
    quality: String,
) -> Result<Value, String> {
    if RENDER_IN_PROGRESS.load(Ordering::SeqCst) {
        return Err("A render is already in progress".to_string());
    }

    let timeline = state
        .with_active_project(|project| project.timeline.clone())
        .ok_or_else(|| "No active project".to_string())?;

    let source_video = state
        .with_active_project(|project| project.source_video_path.clone())
        .ok_or_else(|| "No active project".to_string())?;

    RENDER_IN_PROGRESS.store(true, Ordering::SeqCst);
    RENDER_PROGRESS.store(0, Ordering::SeqCst);
    RENDER_CANCELLED.store(false, Ordering::SeqCst);
    *RENDER_LOG.lock().unwrap() = String::new();

    let crf = quality_to_crf(&quality);

    let result = render_from_timeline(&timeline, &source_video, Some(&output_path), Some(crf)).await;

    RENDER_IN_PROGRESS.store(false, Ordering::SeqCst);
    RENDER_PROGRESS.store(100, Ordering::SeqCst);

    match result {
        Ok(path) => {
            let file_size = std::fs::metadata(&path)
                .map(|m| m.len())
                .unwrap_or(0);

            Ok(json!({
                "output_path": path,
                "file_size_bytes": file_size,
                "status": "completed",
            }))
        }
        Err(e) => {
            let error_msg = format!("{}", e);
            *RENDER_LOG.lock().unwrap() = error_msg.clone();
            Err(format!("Render failed: {}", error_msg))
        }
    }
}

#[tauri::command]
pub async fn get_render_progress() -> Result<Value, String> {
    let in_progress = RENDER_IN_PROGRESS.load(Ordering::SeqCst);
    let progress = RENDER_PROGRESS.load(Ordering::SeqCst);
    let cancelled = RENDER_CANCELLED.load(Ordering::SeqCst);
    let log = RENDER_LOG.lock().unwrap().clone();

    let status = if cancelled {
        "cancelled"
    } else if in_progress {
        "rendering"
    } else if progress >= 100 {
        "completed"
    } else {
        "idle"
    };

    Ok(json!({
        "in_progress": in_progress,
        "progress": progress,
        "status": status,
        "log": if log.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(log) },
    }))
}

#[tauri::command]
pub async fn cancel_render() -> Result<Value, String> {
    if !RENDER_IN_PROGRESS.load(Ordering::SeqCst) {
        return Ok(json!({
            "cancelled": false,
            "reason": "No render in progress",
        }));
    }

    RENDER_CANCELLED.store(true, Ordering::SeqCst);
    RENDER_IN_PROGRESS.store(false, Ordering::SeqCst);

    Ok(json!({
        "cancelled": true,
        "reason": "User requested cancellation",
    }))
}
