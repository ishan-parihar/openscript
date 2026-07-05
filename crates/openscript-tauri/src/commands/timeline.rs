use openscript_core::timeline::Segment;
use openscript_core::types::EditorialRole;
use serde_json::{json, Value};
use tauri::State;

use crate::state::AppState;

fn parse_editorial_role(role: &str) -> Option<EditorialRole> {
    match role {
        "hook" => Some(EditorialRole::Hook),
        "setup" => Some(EditorialRole::Setup),
        "proof" => Some(EditorialRole::Proof),
        "contrast" => Some(EditorialRole::Contrast),
        "payoff" => Some(EditorialRole::Payoff),
        "cta" => Some(EditorialRole::Cta),
        "intro" => Some(EditorialRole::Intro),
        "transition" => Some(EditorialRole::Transition),
        "highlight" => Some(EditorialRole::Highlight),
        "outro" => Some(EditorialRole::Outro),
        _ => None,
    }
}

#[tauri::command]
pub async fn add_segment(
    state: State<'_, AppState>,
    start: f64,
    end: f64,
    caption: String,
    semantic_role: Option<String>,
) -> Result<Value, String> {
    let snapshot_before = state
        .timeline_snapshot()
        .ok_or_else(|| "No active project".to_string())?;

    let new_id = state
        .with_active_project_mut(|project| {
            let timeline = &mut project.timeline;
            let new_id = format!("seg_{:03}", timeline.segments.len() + 1);
            timeline.segments.push(Segment {
                id: new_id.clone(),
                start,
                end,
                caption,
                crossfade_ms: 0,
                semantic_role: semantic_role.as_deref().and_then(parse_editorial_role),
            });
            Ok::<_, String>(new_id)
        })
        .ok_or_else(|| "No active project".to_string())??;

    let snapshot_after = state
        .timeline_snapshot()
        .ok_or_else(|| "No active project".to_string())?;

    state
        .undo_manager
        .write()
        .map_err(|_| "Lock poisoned")?
        .record(
            format!("Add segment: {}", new_id),
            snapshot_before,
            snapshot_after,
        );

    let _ = save_project_inner(&state);

    Ok(json!({ "segment_id": new_id }))
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
                "timeline": project.timeline,
                "segment_count": project.timeline.segments.len(),
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

    let timeline: openscript_core::timeline::Timeline = serde_json::from_value(snapshot)
        .map_err(|e| format!("Failed to restore timeline from undo: {}", e))?;

    state.with_active_project_mut(|project| {
        project.timeline = timeline;
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

    let timeline: openscript_core::timeline::Timeline = serde_json::from_value(snapshot)
        .map_err(|e| format!("Failed to restore timeline from redo: {}", e))?;

    state.with_active_project_mut(|project| {
        project.timeline = timeline;
    });

    let _ = save_project_inner(&state);

    Ok(json!({ "redone": desc }))
}

pub(crate) fn save_project_inner(state: &State<'_, AppState>) -> Result<(), String> {
    let timeline_path = state
        .with_active_project(|p| p.timeline_path.clone())
        .ok_or_else(|| "No active project".to_string())?;
    let timeline_json = state
        .with_active_project(|p| serde_json::to_string_pretty(&p.timeline))
        .ok_or_else(|| "No active project".to_string())?
        .map_err(|e| format!("Failed to serialize: {}", e))?;
    std::fs::write(&timeline_path, &timeline_json).map_err(|e| format!("Failed to save: {}", e))
}

#[tauri::command]
pub async fn remove_segment(
    state: State<'_, AppState>,
    segment_id: String,
) -> Result<Value, String> {
    let snapshot_before = state
        .timeline_snapshot()
        .ok_or_else(|| "No active project".to_string())?;

    let _removed = state
        .with_active_project_mut(|project| {
            let timeline = &mut project.timeline;
            let initial_len = timeline.segments.len();
            timeline.segments.retain(|s| s.id != segment_id);
            if timeline.segments.len() == initial_len {
                Err(format!("Segment not found: {}", segment_id))
            } else {
                Ok(())
            }
        })
        .ok_or_else(|| "No active project".to_string())??;

    let snapshot_after = state
        .timeline_snapshot()
        .ok_or_else(|| "No active project".to_string())?;

    state
        .undo_manager
        .write()
        .map_err(|_| "Lock poisoned")?
        .record(
            format!("Remove segment: {}", segment_id),
            snapshot_before,
            snapshot_after,
        );

    let _ = save_project_inner(&state);

    Ok(json!({ "removed": true, "segment_id": segment_id }))
}

#[tauri::command]
pub async fn update_segment(
    state: State<'_, AppState>,
    segment_id: String,
    start: Option<f64>,
    end: Option<f64>,
    caption: Option<String>,
) -> Result<Value, String> {
    let snapshot_before = state
        .timeline_snapshot()
        .ok_or_else(|| "No active project".to_string())?;

    let _updated = state
        .with_active_project_mut(|project| {
            let timeline = &mut project.timeline;
            match timeline.segments.iter_mut().find(|s| s.id == segment_id) {
                Some(seg) => {
                    if let Some(s) = start {
                        seg.start = s;
                    }
                    if let Some(e) = end {
                        seg.end = e;
                    }
                    if let Some(c) = caption {
                        seg.caption = c;
                    }
                    Ok(())
                }
                None => Err(format!("Segment not found: {}", segment_id)),
            }
        })
        .ok_or_else(|| "No active project".to_string())??;

    let snapshot_after = state
        .timeline_snapshot()
        .ok_or_else(|| "No active project".to_string())?;

    state
        .undo_manager
        .write()
        .map_err(|_| "Lock poisoned")?
        .record(
            format!("Update segment: {}", segment_id),
            snapshot_before,
            snapshot_after,
        );

    let _ = save_project_inner(&state);

    Ok(json!({ "updated": true, "segment_id": segment_id }))
}
