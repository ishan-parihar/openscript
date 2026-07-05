use openscript_ffmpeg::render::render_from_timeline_with_cancel;
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

/// Map a UI quality label to an ffmpeg CRF value.
///
/// CRF (Constant Rate Factor) for x264: lower = higher quality / larger file.
/// - 18  = visually lossless (high)
/// - 23  = x264 default (standard)
/// - 30  = lower quality, smaller file (preview)
///
/// Prior versions of this function returned inverted values (preview=20,
/// standard=30, high=0) which meant "preview" produced better quality than
/// "standard". This was a CRITICAL bug — the labels lied to the user.
fn quality_to_crf(quality: &str) -> u32 {
    match quality {
        "preview" => 30,
        "high" => 18,
        _ => 23, // "standard" and any unknown label
    }
}

/// Resolve an output path. If the caller did not supply one, generate a default
/// under `$HOME/openscript/renders/{timestamp}.mp4` so the render always has a
/// valid destination. This fixes a CRITICAL bug where the frontend called
/// `renderTimeline({quality})` with `outputPath: undefined`, which dropped the
/// key entirely and made Rust's required `output_path: String` deserialisation
/// fail.
fn resolve_output_path(output_path: Option<String>) -> Result<String, String> {
    if let Some(p) = output_path {
        if !p.is_empty() {
            return Ok(p);
        }
    }
    let home = std::env::var("HOME")
        .map_err(|_| "HOME not set; cannot generate output path".to_string())?;
    let renders_dir = std::path::Path::new(&home)
        .join("openscript")
        .join("renders");
    std::fs::create_dir_all(&renders_dir)
        .map_err(|e| format!("Failed to create renders dir: {}", e))?;
    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%S");
    Ok(renders_dir
        .join(format!("render-{}.mp4", ts))
        .to_string_lossy()
        .to_string())
}

#[tauri::command]
pub async fn reelize_timeline(
    state: State<'_, AppState>,
    video_path: String,
) -> Result<Value, String> {
    let output_path = format!("{}.reel.mp4", video_path);
    render_timeline(state, Some(output_path), "standard".to_string()).await
}

#[tauri::command]
pub async fn render_timeline(
    state: State<'_, AppState>,
    output_path: Option<String>,
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

    let out_path = resolve_output_path(output_path)?;

    RENDER_IN_PROGRESS.store(true, Ordering::SeqCst);
    RENDER_PROGRESS.store(0, Ordering::SeqCst);
    RENDER_CANCELLED.store(false, Ordering::SeqCst);
    *RENDER_LOG.lock().unwrap() = String::new();

    let crf = quality_to_crf(&quality);

    // Pass the cancellation token so cancel_render() can actually kill ffmpeg.
    let result = render_from_timeline_with_cancel(
        &timeline,
        &source_video,
        Some(&out_path),
        Some(crf),
        Some(&RENDER_CANCELLED),
    )
    .await;

    RENDER_IN_PROGRESS.store(false, Ordering::SeqCst);

    match result {
        Ok(path) => {
            let file_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            RENDER_PROGRESS.store(100, Ordering::SeqCst);
            Ok(json!({
                "output_path": path,
                "file_size_bytes": file_size,
                "status": "completed",
            }))
        }
        Err(e) => {
            let error_msg = format!("{}", e);
            *RENDER_LOG.lock().unwrap() = error_msg.clone();
            // If the render was cancelled, report a cancelled status instead of an error.
            if RENDER_CANCELLED.load(Ordering::SeqCst) {
                RENDER_CANCELLED.store(false, Ordering::SeqCst);
                Ok(json!({
                    "output_path": serde_json::Value::Null,
                    "file_size_bytes": 0,
                    "status": "cancelled",
                }))
            } else {
                Err(format!("Render failed: {}", error_msg))
            }
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

/// Request that an in-progress render be cancelled.
///
/// Prior versions of this function only flipped `RENDER_CANCELLED` to true but
/// the render path never checked the flag, so the cancel was a no-op. The
/// render path now polls `RENDER_CANCELLED` via the `cancel_token` parameter
/// passed to `render_from_timeline_with_cancel`; when the flag becomes true,
/// ffmpeg is killed and the render returns.
///
/// We deliberately do NOT reset `RENDER_IN_PROGRESS` here — that is done by the
/// render loop itself when it observes the cancellation and exits. This avoids
/// a race where a second render could start before the first ffmpeg child has
/// been reaped.
#[tauri::command]
pub async fn cancel_render() -> Result<Value, String> {
    if !RENDER_IN_PROGRESS.load(Ordering::SeqCst) {
        return Ok(json!({
            "cancelled": false,
            "reason": "No render in progress",
        }));
    }

    RENDER_CANCELLED.store(true, Ordering::SeqCst);

    Ok(json!({
        "cancelled": true,
        "reason": "User requested cancellation; ffmpeg will be killed at the next progress poll",
    }))
}
