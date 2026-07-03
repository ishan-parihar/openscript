use crate::profiles::VoiceProfile;
use base64::{engine::general_purpose::STANDARD, Engine};
use reqwest::Client;
use reqwest::multipart::{Form, Part};
use serde::Deserialize;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Deserialize)]
struct GenerateResponse {
    audio_b64: Option<String>,
    duration_ms: Option<i64>,
    error: Option<String>,
}

/// Qwen3-TTS /generate endpoint uses multipart form data.
#[derive(Serialize, Clone, Default)]
struct TtsGenerateForm {
    #[serde(rename = "text")]
    text: String,
    #[serde(rename = "ref_text")]
    ref_text: String,
    #[serde(rename = "language")]
    language: String,
    #[serde(rename = "mode")]
    mode: String,
    #[serde(rename = "xvec_only")]
    xvec_only: bool,
    #[serde(rename = "non_streaming_mode")]
    non_streaming_mode: bool,
}

pub struct TtsClient {
    http: Client,
    base_url: String,
    cache_dir: PathBuf,
}

impl TtsClient {
    pub fn new(base_url: &str, cache_dir: &str) -> Self {
        Self {
            http: Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .unwrap_or_default(),
            base_url: base_url.to_string(),
            cache_dir: PathBuf::from(cache_dir),
        }
    }

    /// Check if the TTS sidecar server is reachable.
    pub async fn health_check(&self) -> Result<bool, TtsError> {
        // Try /health endpoint first
        let url = format!("{}/health", self.base_url);
        match self.http.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => Ok(true),
            _ => {
                // Fallback: try root endpoint
                match self.http.get(&self.base_url).send().await {
                    Ok(resp) => Ok(resp.status().is_success()),
                    Err(_) => Ok(false),
                }
            }
        }
    }

    pub async fn generate(
        &self,
        voice_profile_id: &str,
        text: &str,
        output_path: &str,
        speed: f64,
        pitch: f64,
        volume: f64,
        _format: &str,
        profile: &VoiceProfile,
    ) -> Result<TtsResult, TtsError> {
        let key = Self::cache_key(voice_profile_id, text, speed, pitch, volume);

        if let Some(cached) = self.get_cached_path(&key, "wav") {
            if cached.exists() {
                std::fs::create_dir_all(Path::new(output_path).parent().unwrap_or(Path::new(".")))?;
                std::fs::copy(&cached, output_path)?;
                let duration_ms = Self::extract_audio_duration(&cached).unwrap_or(0);
                return Ok(TtsResult {
                    output_path: output_path.to_string(),
                    duration_ms,
                    cached: true,
                });
            }
        }

        let form = TtsGenerateForm {
            text: text.to_string(),
            ref_text: profile.ref_text.clone(),
            language: profile.language.to_lowercase(),
            mode: "voice_clone".to_string(),
            xvec_only: true,
            non_streaming_mode: true,
        };

        let ref_bytes = std::fs::read(&profile.ref_audio).map_err(|e| {
            TtsError::Sidecar(format!("Cannot read ref audio {}: {}", profile.ref_audio, e))
        })?;

        let multipart = Form::new()
            .text("text", form.text)
            .text("ref_text", form.ref_text)
            .text("language", form.language)
            .text("mode", form.mode)
            .text("xvec_only", if form.xvec_only { "true" } else { "false" })
            .text("non_streaming_mode", if form.non_streaming_mode { "true" } else { "false" })
            .part(
                "ref_audio",
                Part::bytes(ref_bytes).file_name("ref.wav").mime_str("audio/wav").map_err(|e| {
                    TtsError::Sidecar(format!("Failed to build ref part: {}", e))
                })?,
            );

        let response = self
            .http
            .post(format!("{}/generate", self.base_url))
            .multipart(multipart)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(TtsError::Sidecar(format!(
                "HTTP {}: {}",
                status, body
            )));
        }

        let json_resp: GenerateResponse = response.json().await.map_err(|e| {
            TtsError::Sidecar(format!("Failed to parse TTS response: {}", e))
        })?;

        if let Some(err) = json_resp.error {
            return Err(TtsError::Sidecar(err));
        }

        let audio_b64 = json_resp.audio_b64.ok_or_else(|| {
            TtsError::Sidecar("TTS response missing audio_b64 field".to_string())
        })?;

        let bytes = STANDARD.decode(&audio_b64).map_err(|e| {
            TtsError::Sidecar(format!("Failed to decode base64 audio: {}", e))
        })?;

        let output = PathBuf::from(output_path);
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&output, &bytes)?;

        let cache_path = self.cache_dir.join(format!("{}.wav", key));
        if let Some(parent) = cache_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&cache_path, &bytes)?;

        // Best-effort cache eviction — prevents unbounded growth
        crate::evict_cache_if_needed(&self.cache_dir);

        let duration_ms = json_resp.duration_ms.unwrap_or_else(|| {
            Self::extract_audio_duration(&output).unwrap_or(0)
        });

        Ok(TtsResult {
            output_path: output_path.to_string(),
            duration_ms,
            cached: false,
        })
    }

    /// Extract audio duration in milliseconds from a WAV/MP3 file.
    /// Uses ffprobe to avoid adding audio parsing dependencies.
    fn extract_audio_duration(path: &Path) -> Option<i64> {
        let output = std::process::Command::new("ffprobe")
            .args([
                "-v", "error",
                "-show_entries", "format=duration",
                "-of", "default=noprint_wrappers=1:nokey=1",
                path.to_str()?,
            ])
            .output()
            .ok()?;

        if output.status.success() {
            let dur_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
            dur_str.parse::<f64>().ok().map(|d| (d * 1000.0).round() as i64)
        } else {
            None
        }
    }

    pub fn estimate_duration(text: &str, speed: f64) -> i64 {
        let words = text.split_whitespace().count() as f64;
        let base_rate = 2.5;
        let adjusted = base_rate * speed;
        if adjusted <= 0.0 {
            return 0;
        }
        ((words / adjusted) * 1000.0) as i64
    }

    fn cache_key(voice_id: &str, text: &str, speed: f64, pitch: f64, volume: f64) -> String {
        let input = format!("{}|{}|{}|{}|{}", voice_id, text, speed, pitch, volume);
        let mut hasher = Sha256::new();
        hasher.update(input.as_bytes());
        let result = hasher.finalize();
        hex::encode(result)[..16].to_string()
    }

    fn get_cached_path(&self, key: &str, format: &str) -> Option<PathBuf> {
        let path = self.cache_dir.join(format!("{}.{}", key, format));
        if path.exists() {
            Some(path)
        } else {
            None
        }
    }
}

pub struct TtsResult {
    pub output_path: String,
    pub duration_ms: i64,
    pub cached: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum TtsError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("TTS sidecar error: {0}")]
    Sidecar(String),
}
