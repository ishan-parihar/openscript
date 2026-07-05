#![allow(dead_code)]

use openscript_tts::client::TtsClient;
use openscript_tts::profiles::VoiceProfileRegistry;
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

/// Generate TTS audio using a voice profile.
///
/// `output_path` is optional — if `None`, a default path under
/// `/tmp/openscript-tts-cache/{uuid}.wav` is generated. This fixes a
/// CRITICAL bug where the frontend called `ttsGenerate(text, voice, undefined)`
/// and Rust's required `output_path: String` deserialisation failed, breaking
/// the Generate button.
#[tauri::command]
pub async fn tts_generate(
    state: State<'_, AppState>,
    text: String,
    voice_profile_id: String,
    output_path: Option<String>,
) -> Result<Value, String> {
    let registry = load_registry(&state)?;
    let profile = registry
        .get(&voice_profile_id)
        .ok_or_else(|| format!("Voice profile not found: {}", voice_profile_id))?
        .clone();

    let cache_dir = "/tmp/openscript-tts-cache";
    std::fs::create_dir_all(cache_dir).map_err(|e| format!("Failed to create TTS cache dir: {}", e))?;
    let resolved_output_path = output_path
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| format!("{}/{}.wav", cache_dir, uuid::Uuid::new_v4()));

    let client = TtsClient::new(&state.tts_url, cache_dir);

    let result = client
        .generate(
            &voice_profile_id,
            &text,
            &resolved_output_path,
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
