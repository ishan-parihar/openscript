#![allow(dead_code)]

use openscript_assets::music::MusicIndex;
use openscript_assets::sfx::SfxIndex;
use openscript_assets::pexels::{PexelsClient, match_concept};

use openscript_core::timeline::{TimelineEvent, EventKind, Provenance};
use openscript_core::types::TrackType;
use serde_json::{json, Value};
use tauri::State;

use crate::commands::timeline::save_project_inner;
use crate::state::AppState;

const PEXELS_CACHE_DIR: &str = "/tmp/openscript-pexels-cache";
const MUSIC_INDEX_FILE: &str = "index.json";
const SFX_INDEX_FILE: &str = "index.json";

fn generate_event_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("evt_{}", ts)
}

#[tauri::command]
pub async fn broll_fetch(
    state: State<'_, AppState>,
    concepts: Vec<String>,
    download: Option<bool>,
) -> Result<Value, String> {
    let api_key = state
        .pexels_api_key
        .clone()
        .ok_or_else(|| "PEXELS_API_KEY not set".to_string())?;

    let should_download = download.unwrap_or(false);
    let mut results = Vec::new();

    for concept in &concepts {
        let mut client = PexelsClient::new(&api_key, PEXELS_CACHE_DIR);

        let alias = match_concept(concept);
        let search_term = alias.as_deref().unwrap_or(concept);

        let videos = client
            .search(search_term, "portrait", "sd")
            .await
            .map_err(|e| format!("Pexels search failed for '{}': {}", concept, e))?;

        let mut video_entries = Vec::new();
        for v in &videos {
            let mut entry = json!({
                "id": v.id,
                "width": v.width,
                "height": v.height,
                "url": v.url,
                "image": v.image,
            });

            if should_download && !v.video_files.is_empty() {
                match client.download_best(v, search_term).await {
                    Ok(cached_path) => {
                        entry["cached_path"] = json!(cached_path);
                    }
                    Err(e) => {
                        eprintln!(
                            "Failed to download video {} for '{}': {}",
                            v.id, concept, e
                        );
                    }
                }
            }

            video_entries.push(entry);
        }

        results.push(json!({
            "concept": concept,
            "matched_concept": alias,
            "videos": video_entries,
        }));
    }

    Ok(json!(results))
}

#[tauri::command]
pub async fn broll_assign(
    state: State<'_, AppState>,
    concept: String,
    position_ms: i64,
    duration_ms: i64,
) -> Result<Value, String> {
    let event_id = generate_event_id();
    let end_ms = position_ms + duration_ms;

    state
        .with_active_project_mut(|project| {
            let event = TimelineEvent {
                id: event_id.clone(),
                asset_id: format!("broll_{}", concept.replace(' ', "_")),
                start_ms: position_ms,
                end_ms,
                offset_ms: 0,
                gain_db: 0.0,
                fade_in_ms: 0,
                fade_out_ms: 0,
                tags: vec![concept.clone()],
                provenance: Some(Provenance {
                    tool: "broll_assign".to_string(),
                    editorial_role: None,
                    concept: Some(concept.clone()),
                }),
                kind: EventKind::Broll {
                    concept: concept.clone(),
                    source_provider: "pexels".to_string(),
                    transition_style: "crossfade".to_string(),
                    crop_mode: "smart".to_string(),
                    orientation: "portrait".to_string(),
                    motion_intensity: "medium".to_string(),
                },
            };

            project
                .timeline
                .tracks
                .entry(TrackType::Broll)
                .or_default()
                .push(event);
        });

    let _ = save_project_inner(&state);

    Ok(json!({
        "event_id": event_id,
        "concept": concept,
        "start_ms": position_ms,
        "end_ms": end_ms,
        "assigned": true,
    }))
}

fn load_music_index(base_path: &std::path::Path) -> Result<MusicIndex, String> {
    let music_dir = base_path.join("Music");
    let index_path = music_dir.join(MUSIC_INDEX_FILE);

    if index_path.exists() {
        MusicIndex::load(Some(
            index_path
                .to_str()
                .ok_or_else(|| "Invalid music index path".to_string())?,
        ))
        .map_err(|e| format!("Failed to load music index: {}", e))
    } else {
        let dir_str = music_dir
            .to_str()
            .ok_or_else(|| "Invalid music directory path".to_string())?
            .to_string();
        MusicIndex::scan_directories(&[dir_str])
            .map_err(|e| format!("Failed to scan music directory: {}", e))
    }
}

fn load_sfx_index(base_path: &std::path::Path) -> Result<SfxIndex, String> {
    let sfx_dir = base_path.join("SFX");
    let index_path = sfx_dir.join(SFX_INDEX_FILE);

    if index_path.exists() {
        SfxIndex::load(Some(
            index_path
                .to_str()
                .ok_or_else(|| "Invalid SFX index path".to_string())?,
        ))
        .map_err(|e| format!("Failed to load SFX index: {}", e))
    } else {
        let dir_str = sfx_dir
            .to_str()
            .ok_or_else(|| "Invalid SFX directory path".to_string())?;
        SfxIndex::scan_directory(dir_str)
            .map_err(|e| format!("Failed to scan SFX directory: {}", e))
    }
}

#[tauri::command]
pub async fn music_search(
    state: State<'_, AppState>,
    mood: Option<String>,
    energy: Option<String>,
) -> Result<Value, String> {
    let index = load_music_index(&state.assets_base_path)?;

    let results = index.search(
        "",
        mood.as_deref(),
        energy.as_deref(),
        None,
        None,
        None,
        50,
    );

    let tracks: Vec<Value> = results
        .iter()
        .map(|a| {
            json!({
                "id": a.id,
                "title": a.title,
                "artist": a.artist,
                "path": a.path,
                "duration_ms": a.duration_ms,
                "mood": a.mood,
                "energy": a.energy,
                "bpm": a.bpm,
                "loopability": a.loopability,
                "intro_friendly": a.intro_friendly,
                "cta_friendly": a.cta_friendly,
                "loudness_target_lufs": a.loudness_target_lufs,
                "tags": a.tags,
                "genre": a.genre,
            })
        })
        .collect();

    Ok(json!({
        "total": tracks.len(),
        "tracks": tracks,
    }))
}

#[tauri::command]
pub async fn music_assign(
    state: State<'_, AppState>,
    mood: String,
    energy: String,
) -> Result<Value, String> {
    let index = load_music_index(&state.assets_base_path)?;

    let first_track = index
        .search("", Some(&mood), Some(&energy), None, None, None, 1)
        .into_iter()
        .next()
        .cloned();

    let asset_id = match &first_track {
        Some(track) => track.id.clone(),
        None => format!("music_{}_{}", mood, energy),
    };

    let duration_ms = first_track.as_ref().map(|t| t.duration_ms).unwrap_or(0);

    let timeline_duration_ms = state
        .with_active_project(|project| {
            let total_seconds: f64 = project
                .timeline
                .segments
                .iter()
                .map(|s| s.end - s.start)
                .sum();
            (total_seconds * 1000.0) as i64
        })
        .ok_or_else(|| "No active project".to_string())?;

    let event_id = generate_event_id();

    state
        .with_active_project_mut(|project| {
            let event = TimelineEvent {
                id: event_id.clone(),
                asset_id: asset_id.clone(),
                start_ms: 0,
                end_ms: if timeline_duration_ms > 0 {
                    timeline_duration_ms
                } else {
                    duration_ms
                },
                offset_ms: 0,
                gain_db: -6.0,
                fade_in_ms: 500,
                fade_out_ms: 500,
                tags: vec![mood.clone(), energy.clone()],
                provenance: Some(Provenance {
                    tool: "music_assign".to_string(),
                    editorial_role: None,
                    concept: None,
                }),
                kind: EventKind::Music {
                    mood: mood.clone(),
                    energy: energy.clone(),
                    bpm: first_track.as_ref().and_then(|t| t.bpm),
                    loopability: first_track
                        .as_ref()
                        .map(|t| t.loopability)
                        .unwrap_or(false),
                    intro_friendly: first_track
                        .as_ref()
                        .map(|t| t.intro_friendly)
                        .unwrap_or(false),
                    cta_friendly: first_track
                        .as_ref()
                        .map(|t| t.cta_friendly)
                        .unwrap_or(false),
                    loudness_target_lufs: first_track
                        .as_ref()
                        .map(|t| t.loudness_target_lufs)
                        .unwrap_or(-14.0),
                    loop_mode: "seamless".to_string(),
                    ducking_policy: "on_voice".to_string(),
                },
            };

            project
                .timeline
                .tracks
                .entry(TrackType::Music)
                .or_default()
                .push(event);
        });

    let _ = save_project_inner(&state);

    Ok(json!({
        "event_id": event_id,
        "asset_id": asset_id,
        "mood": mood,
        "energy": energy,
        "assigned": true,
    }))
}

#[tauri::command]
pub async fn sfx_search(
    state: State<'_, AppState>,
    query: Option<String>,
    editorial_role: Option<String>,
) -> Result<Value, String> {
    let index = load_sfx_index(&state.assets_base_path)?;

    let results = index.search(
        query.as_deref().unwrap_or(""),
        editorial_role.as_deref(),
        None,
        50,
    );

    let sfx_list: Vec<Value> = results
        .iter()
        .map(|a| {
            json!({
                "id": a.id,
                "filename": a.filename,
                "path": a.path,
                "category": a.category,
                "subcategory": a.subcategory,
                "editorial_role": a.editorial_role,
                "duration_ms": a.duration_ms,
                "sample_rate": a.sample_rate,
                "peak_db": a.peak_db,
                "loudness_lufs": a.loudness_lufs,
                "recommended_gain_db": a.recommended_gain_db,
                "recommended_use": a.recommended_use,
                "safe_overlay": a.safe_overlay,
                "tags": a.tags,
            })
        })
        .collect();

    Ok(json!({
        "total": sfx_list.len(),
        "sfx": sfx_list,
    }))
}

#[tauri::command]
pub async fn sfx_assign(
    state: State<'_, AppState>,
    editorial_role: String,
    position_ms: i64,
) -> Result<Value, String> {
    let index = load_sfx_index(&state.assets_base_path)?;

    let first_sfx = index
        .search("", Some(&editorial_role), None, 1)
        .into_iter()
        .next()
        .cloned();

    let (
        asset_id,
        duration_ms,
        category,
        subcategory,
        peak_db,
        loudness_lufs,
        recommended_gain_db,
        recommended_use,
        safe_overlay,
    ) = match &first_sfx {
        Some(sfx) => (
            sfx.id.clone(),
            sfx.duration_ms,
            sfx.category.clone(),
            sfx.subcategory.clone(),
            sfx.peak_db,
            sfx.loudness_lufs,
            sfx.recommended_gain_db,
            sfx.recommended_use.clone(),
            sfx.safe_overlay,
        ),
        None => (
            format!("sfx_{}", editorial_role),
            1000,
            String::new(),
            String::new(),
            0.0,
            0.0,
            -6.0,
            String::new(),
            false,
        ),
    };

    let event_id = generate_event_id();
    let end_ms = position_ms + duration_ms;

    state
        .with_active_project_mut(|project| {
            let event = TimelineEvent {
                id: event_id.clone(),
                asset_id: asset_id.clone(),
                start_ms: position_ms,
                end_ms,
                offset_ms: 0,
                gain_db: recommended_gain_db,
                fade_in_ms: 0,
                fade_out_ms: 50,
                tags: vec![editorial_role.clone()],
                provenance: Some(Provenance {
                    tool: "sfx_assign".to_string(),
                    editorial_role: Some(editorial_role.clone()),
                    concept: None,
                }),
                kind: EventKind::Sfx {
                    editorial_role: editorial_role.clone(),
                    category,
                    subcategory,
                    duration_ms,
                    sample_rate: 48000,
                    peak_db,
                    loudness_lufs,
                    recommended_gain_db,
                    recommended_use,
                    safe_overlay,
                },
            };

            project
                .timeline
                .tracks
                .entry(TrackType::Sfx)
                .or_default()
                .push(event);
        });

    let _ = save_project_inner(&state);

    Ok(json!({
        "event_id": event_id,
        "asset_id": asset_id,
        "editorial_role": editorial_role,
        "start_ms": position_ms,
        "end_ms": end_ms,
        "assigned": true,
    }))
}
