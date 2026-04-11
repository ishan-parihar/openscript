use serde_json::{json, Value};
use std::path::Path;
use tauri::State;

use crate::state::AppState;

#[tauri::command]
pub async fn reelize_timeline(
    _state: State<'_, AppState>,
    video_path: String,
) -> Result<Value, String> {
    if !Path::new(&video_path).exists() {
        return Err(format!("Video not found: {}", video_path));
    }

    Ok(json!({
        "output_path": "",
        "file_size_bytes": 0,
        "timeline_path": "",
        "segments_count": 0,
        "tracks_rendered": 0,
        "status": "stub",
        "message": "Full render pipeline not yet implemented. Transcribe, build timeline, then call this again.",
    }))
}
