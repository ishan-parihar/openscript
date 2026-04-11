use openscript_core::timeline::Timeline;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use super::undo::UndoManager;

#[derive(Debug, Clone)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub source_video_path: String,
    pub timeline_path: String,
    pub transcript_path: Option<String>,
    pub timeline: Timeline,
    #[allow(dead_code)]
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub modified_at: chrono::DateTime<chrono::Utc>,
}

impl Project {
    pub fn new(
        id: String,
        name: String,
        source_video: String,
        timeline_path: String,
        timeline: Timeline,
    ) -> Self {
        let now = chrono::Utc::now();
        Self {
            id,
            name,
            source_video_path: source_video,
            timeline_path,
            transcript_path: None,
            timeline,
            created_at: now,
            modified_at: now,
        }
    }
}

pub struct AppState {
    pub projects: Arc<RwLock<HashMap<String, Project>>>,
    pub active_project: Arc<RwLock<Option<String>>>,
    pub undo_manager: Arc<RwLock<UndoManager>>,
    pub assets_base_path: PathBuf,
    pub tts_url: String,
    pub pexels_api_key: Option<String>,
}

impl AppState {
    pub fn new() -> Self {
        let assets_base = std::env::var("OPENSCRIPT_SFX_PATH")
            .ok()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/home/ishanp/Videos/Assets"));
        let tts_url = std::env::var("OPENSCRIPT_TTS_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:17493".to_string());
        let pexels_key = std::env::var("PEXELS_API_KEY").ok();
        Self {
            projects: Arc::new(RwLock::new(HashMap::new())),
            active_project: Arc::new(RwLock::new(None)),
            undo_manager: Arc::new(RwLock::new(UndoManager::new())),
            assets_base_path: assets_base,
            tts_url,
            pexels_api_key: pexels_key,
        }
    }

    pub fn with_active_project<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&Project) -> R,
    {
        let guard = self.active_project.read().ok()?;
        let id = guard.as_ref()?.clone();
        drop(guard);
        let projects = self.projects.read().ok()?;
        let project = projects.get(&id)?;
        Some(f(project))
    }

    pub fn with_active_project_mut<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&mut Project) -> R,
    {
        let guard = self.active_project.read().ok()?;
        let id = guard.as_ref()?.clone();
        drop(guard);
        let mut projects = self.projects.write().ok()?;
        let project = projects.get_mut(&id)?;
        project.modified_at = chrono::Utc::now();
        Some(f(project))
    }

    pub fn timeline_snapshot(&self) -> Option<Value> {
        self.with_active_project_mut(|project| serde_json::to_value(&project.timeline).ok())?
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
