use openscript_core::timeline::Segment;
use serde_json::{json, Value};
use tauri::State;

use crate::state::AppState;

/// Split a segment at a given timestamp (relative to source video, in seconds).
/// Creates two segments: original start → split point, split point → original end.
#[tauri::command]
pub async fn split_segment(
    state: State<'_, AppState>,
    segment_id: String,
    split_time_s: f64,
) -> Result<Value, String> {
    let snapshot_before = state
        .timeline_snapshot()
        .ok_or_else(|| "No active project".to_string())?;

    let split_result = state
        .with_active_project_mut(|project| {
            let timeline = &mut project.timeline;
            let seg_idx = timeline.segments.iter().position(|s| s.id == segment_id);
            match seg_idx {
                Some(idx) => {
                    let seg = &timeline.segments[idx];
                    if split_time_s <= seg.start || split_time_s >= seg.end {
                        return Err(format!(
                            "Split time {:.2}s is outside segment range [{:.2}s - {:.2}s]",
                            split_time_s, seg.start, seg.end
                        ));
                    }

                    let new_id = format!("seg_{:03}", timeline.segments.len() + 1);
                    let original_caption = seg.caption.clone();
                    let original_role = seg.semantic_role.clone();
                    let original_crossfade = seg.crossfade_ms;
                    let original_end = seg.end;

                    // Modify original: start -> split
                    timeline.segments[idx].end = split_time_s;

                    // Create new segment: split -> end
                    let new_seg = Segment {
                        id: new_id.clone(),
                        start: split_time_s,
                        end: original_end,
                        caption: original_caption,
                        crossfade_ms: original_crossfade,
                        semantic_role: original_role,
                    };
                    timeline.segments.insert(idx + 1, new_seg);

                    Ok(new_id)
                }
                None => Err(format!("Segment not found: {}", segment_id)),
            }
        })
        .ok_or_else(|| "No active project".to_string())??;

    let snapshot_after = state
        .timeline_snapshot()
        .ok_or_else(|| "No active project".to_string())?;

    // Record for undo
    state
        .undo_manager
        .write()
        .map_err(|_| "Lock poisoned")?
        .record(
            format!("Split segment: {}", segment_id),
            snapshot_before,
            snapshot_after,
        );

    // Auto-save
    let _ = save_project_inner(&state);

    Ok(json!({ "segment_id": split_result, "split": true }))
}

/// Get the active timeline as JSON.
#[tauri::command]
pub async fn get_timeline(state: State<'_, AppState>) -> Result<Value, String> {
    state
        .with_active_project(|project| {
            json!({
                "project_id": project.id,
                "name": project.name,
                "source_video": project.source_video_path,
                "segment_count": project.timeline.segments.len(),
            })
        })
        .ok_or_else(|| "No active project".to_string())
}

/// Get timeline preview summary.
#[tauri::command]
pub async fn timeline_preview(state: State<'_, AppState>) -> Result<Value, String> {
    state
        .with_active_project(|project| {
            let total_duration_s: f64 = project
                .timeline
                .segments
                .iter()
                .map(|s| s.end - s.start)
                .sum();

            json!({
                "total_duration_s": total_duration_s,
                "segment_count": project.timeline.segments.len(),
                "render_ready": !project.timeline.segments.is_empty(),
            })
        })
        .ok_or_else(|| "No active project".to_string())
}

/// Undo the last operation.
#[tauri::command]
pub async fn undo(state: State<'_, AppState>) -> Result<Value, String> {
    let (desc, snapshot) = state
        .undo_manager
        .write()
        .map_err(|_| "Lock poisoned")?
        .undo()
        .ok_or_else(|| "Nothing to undo".to_string())?;

    state.with_active_project_mut(|project| {
        project.timeline = serde_json::from_value(snapshot.clone())
            .unwrap_or_else(|_| project.timeline.clone());
    });

    let _ = save_project_inner(&state);

    Ok(json!({ "undone": desc }))
}

/// Redo the last undone operation.
#[tauri::command]
pub async fn redo(state: State<'_, AppState>) -> Result<Value, String> {
    let (desc, snapshot) = state
        .undo_manager
        .write()
        .map_err(|_| "Lock poisoned")?
        .redo()
        .ok_or_else(|| "Nothing to redo".to_string())?;

    state.with_active_project_mut(|project| {
        project.timeline = serde_json::from_value(snapshot.clone())
            .unwrap_or_else(|_| project.timeline.clone());
    });

    let _ = save_project_inner(&state);

    Ok(json!({ "redone": desc }))
}

fn save_project_inner(state: &State<'_, AppState>) -> Result<(), String> {
    let timeline_path = state
        .with_active_project(|p| p.timeline_path.clone())
        .ok_or_else(|| "No active project".to_string())?;
    let timeline_json = state
        .with_active_project(|p| serde_json::to_string_pretty(&p.timeline))
        .ok_or_else(|| "No active project".to_string())?
        .map_err(|e| format!("Failed to serialize: {}", e))?;
    std::fs::write(&timeline_path, &timeline_json).map_err(|e| format!("Failed to save: {}", e))
}
