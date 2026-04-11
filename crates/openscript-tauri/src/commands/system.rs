use serde_json::{json, Value};
use tauri::State;
use crate::state::AppState;

#[tauri::command]
pub async fn system_capabilities(state: State<'_, AppState>) -> Result<Value, String> {
    // Check voicebox
    let voicebox_available = {
        let client = reqwest::Client::new();
        match client
            .get(format!("{}/health", state.tts_url))
            .timeout(std::time::Duration::from_secs(2))
            .send()
            .await
        {
            Ok(resp) => {
                if let Ok(body) = resp.json::<serde_json::Value>().await {
                    let model_loaded = body
                        .get("model_loaded")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    json!({ "available": true, "model_loaded": model_loaded, "url": state.tts_url })
                } else {
                    json!({ "available": false, "reason": "Voicebox responded but returned invalid JSON" })
                }
            }
            Err(_) => json!({ "available": false, "reason": format!("Cannot reach voicebox at {}", state.tts_url) }),
        }
    };

    let pexels_available = json!({
        "available": state.pexels_api_key.is_some(),
        "reason": if state.pexels_api_key.is_none() {
            "PEXELS_API_KEY not set"
        } else {
            "Ready"
        },
    });

    let sfx_path = state.assets_base_path.join("SFX");
    let sfx_count = if sfx_path.exists() {
        std::fs::read_dir(&sfx_path).map(|d| d.count()).unwrap_or(0)
    } else {
        0
    };

    let music_path = state.assets_base_path.join("Music");
    let music_count = if music_path.exists() {
        std::fs::read_dir(&music_path).map(|d| d.count()).unwrap_or(0)
    } else {
        0
    };

    let transcription_available = json!({ "available": true, "engine": "apex" });

    let ffmpeg_available = {
        let output = std::process::Command::new("ffmpeg").arg("-version").output();
        match output {
            Ok(o) if o.status.success() => json!({ "available": true }),
            _ => json!({ "available": false, "reason": "ffmpeg not found in PATH" }),
        }
    };

    Ok(json!({
        "voicebox": voicebox_available,
        "pexels": pexels_available,
        "sfx_library": { "available": sfx_count > 0, "indexed_count": sfx_count },
        "music_library": { "available": music_count > 0, "indexed_count": music_count },
        "transcription": transcription_available,
        "ffmpeg": ffmpeg_available,
    }))
}
