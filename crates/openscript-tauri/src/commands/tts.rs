#![allow(dead_code)]

use openscript_tts::client::TtsClient;
use openscript_tts::profiles::{VoiceProfile, VoiceProfileRegistry};
use serde_json::{json, Value};
use std::path::PathBuf;
use tauri::State;

use crate::state::AppState;

fn get_registry_path(state: &State<'_, AppState>) -> PathBuf {
    state
        .assets_base_path
        .join("voice_profiles.json")
}

fn load_registry(state: &State<'_, AppState>) -> Result<VoiceProfileRegistry, String> {
    let registry_path = get_registry_path(state);
    VoiceProfileRegistry::new(registry_path.to_str().unwrap_or_default())
        .map_err(|e| format!("Failed to load voice profile registry: {}", e))
}

/// List all registered voice profiles.
#[tauri::command]
pub async fn voice_profile_list(state: State<'_, AppState>) -> Result<Value, String> {
    let registry = load_registry(&state)?;
    let profiles: Vec<Value> = registry
        .list()
        .iter()
        .map(|p| {
            json!({
                "id": p.id,
                "name": p.description.as_deref().unwrap_or(&p.id),
                "language": p.language,
            })
        })
        .collect();

    Ok(json!({
        "profiles": profiles,
        "count": profiles.len(),
    }))
}

/// Add a new voice profile to the registry.
#[tauri::command]
pub async fn voice_profile_add(
    state: State<'_, AppState>,
    name: String,
    language: String,
    audio_file_path: String,
) -> Result<Value, String> {
    let profile_id = format!("voice_{}", name.to_lowercase().replace(' ', "_"));

    if !std::path::Path::new(&audio_file_path).exists() {
        return Err(format!("Reference audio file not found: {}", audio_file_path));
    }

    let ref_text = name.clone();

    let profile = VoiceProfile {
        id: profile_id.clone(),
        provider: "qwen3".to_string(),
        mode: "voice_clone".to_string(),
        model: "faster-qwen3-tts".to_string(),
        ref_audio: audio_file_path,
        ref_text,
        language,
        description: Some(name),
        sample_rate: 22050,
        created_at: chrono::Utc::now().to_rfc3339(),
    };

    let mut registry = load_registry(&state)?;
    registry
        .add(profile)
        .map_err(|e| format!("Failed to save voice profile: {}", e))?;

    Ok(json!({
        "profile_id": profile_id,
    }))
}

/// Remove a voice profile from the registry.
#[tauri::command]
pub async fn voice_profile_remove(
    state: State<'_, AppState>,
    profile_id: String,
) -> Result<Value, String> {
    let mut registry = load_registry(&state)?;
    let removed = registry
        .remove(&profile_id)
        .map_err(|e| format!("Failed to remove voice profile: {}", e))?;

    Ok(json!({
        "removed": removed.is_some(),
    }))
}

/// Generate TTS audio using a voice profile.
#[tauri::command]
pub async fn tts_generate(
    state: State<'_, AppState>,
    text: String,
    voice_profile_id: String,
    output_path: String,
) -> Result<Value, String> {
    let registry = load_registry(&state)?;
    let profile = registry
        .get(&voice_profile_id)
        .ok_or_else(|| format!("Voice profile not found: {}", voice_profile_id))?
        .clone();

    let cache_dir = "/tmp/openscript-tts-cache";
    let client = TtsClient::new(&state.tts_url, cache_dir);

    let result = client
        .generate(
            &voice_profile_id,
            &text,
            &output_path,
            1.0,
            1.0,
            1.0,
            "wav",
            &profile,
        )
        .await
        .map_err(|e| format!("TTS generation failed: {}", e))?;

    Ok(json!({
        "output_path": result.output_path,
        "duration_ms": result.duration_ms,
    }))
}

/// Estimate the duration of TTS output for given text and voice profile.
#[tauri::command]
pub async fn tts_estimate_duration(
    state: State<'_, AppState>,
    text: String,
    voice_profile_id: String,
) -> Result<Value, String> {
    let registry = load_registry(&state)?;
    let profile = registry
        .get(&voice_profile_id)
        .ok_or_else(|| format!("Voice profile not found: {}", voice_profile_id))?;

    let estimated_duration_ms = TtsClient::estimate_duration(&text, 1.0);

    Ok(json!({
        "estimated_duration_ms": estimated_duration_ms,
        "profile_id": profile.id,
    }))
}
