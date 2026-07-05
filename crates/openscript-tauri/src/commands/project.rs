use openscript_core::timeline::Timeline;
use serde_json::{json, Value};
use std::path::PathBuf;
use tauri::State;
use uuid::Uuid;

use crate::state::AppState;

/// Create a new project from a source video.
#[tauri::command]
pub async fn create_project(
    state: State<'_, AppState>,
    source_video: String,
) -> Result<Value, String> {
    let video_path = PathBuf::from(&source_video);
    if !video_path.exists() {
        return Err(format!("Source video not found: {}", source_video));
    }

    let project_id = Uuid::new_v4().to_string();
    let project_name = video_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Untitled")
        .to_string();

    let timeline_dir = PathBuf::from(format!(".openscript/projects/{}", project_id));
    std::fs::create_dir_all(&timeline_dir)
        .map_err(|e| format!("Failed to create project dir: {}", e))?;

    let timeline_path = timeline_dir.join("timeline.json");

    let timeline = Timeline::new(video_path.clone(), "9:16", 30, None);

    let timeline_json = serde_json::to_string_pretty(&timeline)
        .map_err(|e| format!("Failed to serialize timeline: {}", e))?;
    std::fs::write(&timeline_path, &timeline_json)
        .map_err(|e| format!("Failed to write timeline: {}", e))?;

    let project = crate::state::app_state::Project::new(
        project_id.clone(),
        project_name.clone(),
        source_video,
        timeline_path.to_string_lossy().to_string(),
        timeline,
    );

    let mut projects = state.projects.write().map_err(|_| "Lock poisoned")?;
    projects.insert(project_id.clone(), project);
    *state.active_project.write().map_err(|_| "Lock poisoned")? = Some(project_id.clone());

    Ok(json!({
        "project_id": project_id,
        "name": project_name,
        "timeline_path": timeline_path.to_string_lossy(),
    }))
}

/// Load an existing project by ID.
#[tauri::command]
pub async fn load_project(state: State<'_, AppState>, project_id: String) -> Result<Value, String> {
    // Try in-memory first
    {
        let projects = state.projects.read().map_err(|_| "Lock poisoned")?;
        if let Some(project) = projects.get(&project_id) {
            *state.active_project.write().map_err(|_| "Lock poisoned")? = Some(project_id);
            return Ok(json!({
                "project_id": project.id,
                "name": project.name,
                "source_video": project.source_video_path,
                "timeline_path": project.timeline_path,
                "timeline": serde_json::to_value(&project.timeline)
                    .map_err(|e| format!("Failed to serialize timeline: {}", e))?,
            }));
        }
    }

    // Try loading from disk
    let timeline_path = PathBuf::from(format!(".openscript/projects/{}/timeline.json", project_id));
    if !timeline_path.exists() {
        return Err(format!("Project not found: {}", project_id));
    }

    let content = std::fs::read_to_string(&timeline_path)
        .map_err(|e| format!("Failed to read timeline: {}", e))?;
    let timeline: Timeline =
        serde_json::from_str(&content).map_err(|e| format!("Failed to parse timeline: {}", e))?;

    let project = crate::state::app_state::Project::new(
        project_id.clone(),
        "Loaded Project".to_string(),
        timeline.source.to_string_lossy().to_string(),
        timeline_path.to_string_lossy().to_string(),
        timeline,
    );

    let mut projects = state.projects.write().map_err(|_| "Lock poisoned")?;
    projects.insert(project_id.clone(), project);
    *state.active_project.write().map_err(|_| "Lock poisoned")? = Some(project_id.clone());

    Ok(json!({ "project_id": project_id, "loaded": true }))
}

/// List all open projects.
#[tauri::command]
pub async fn list_projects(state: State<'_, AppState>) -> Result<Value, String> {
    let projects = state.projects.read().map_err(|_| "Lock poisoned")?;
    let active = state.active_project.read().map_err(|_| "Lock poisoned")?;

    let list: Vec<Value> = projects
        .values()
        .map(|p| {
            json!({
                "id": p.id,
                "name": p.name,
                "source_video": p.source_video_path,
                "active": active.as_ref() == Some(&p.id),
            })
        })
        .collect();

    Ok(json!(list))
}

/// Save the active project's timeline to disk.
#[tauri::command]
pub async fn save_project(state: State<'_, AppState>) -> Result<Value, String> {
    crate::commands::timeline::save_project_inner(&state).map(|_| json!({ "saved": true }))
}
