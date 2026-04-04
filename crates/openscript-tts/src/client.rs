use crate::profiles::VoiceProfile;
use reqwest::Client;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Serialize)]
struct TtsRequest<'a> {
    model: &'a str,
    input: &'a str,
    voice: &'a str,
    response_format: &'a str,
    speed: f64,
    reference_audio: &'a str,
    reference_text: &'a str,
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
        let url = format!("{}/health", self.base_url);
        match self.http.get(&url).send().await {
            Ok(resp) => Ok(resp.status().is_success()),
            Err(_) => {
                // Fallback: try root endpoint
                match self.http.get(&self.base_url).send().await {
                    Ok(_) => Ok(true),
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
        format: &str,
        profile: &VoiceProfile,
    ) -> Result<TtsResult, TtsError> {
        let key = Self::cache_key(voice_profile_id, text, speed, pitch, volume);

        if let Some(cached) = self.get_cached_path(&key, format) {
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

        let req = TtsRequest {
            model: &profile.model,
            input: text,
            voice: voice_profile_id,
            response_format: format,
            speed,
            reference_audio: &profile.ref_audio,
            reference_text: &profile.ref_text,
        };

        let response = self
            .http
            .post(format!("{}/v1/audio/speech", self.base_url))
            .json(&req)
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

        let bytes = response.bytes().await?;

        let output = PathBuf::from(output_path);
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&output, &bytes)?;

        // Cache the output
        let cache_path = self.cache_dir.join(format!("{}.{}", key, format));
        if let Some(parent) = cache_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&cache_path, &bytes)?;

        // Extract duration from the written audio file
        let duration_ms = Self::extract_audio_duration(&output).unwrap_or(0);

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
            dur_str.parse::<f64>().ok().map(|d| (d * 1000.0) as i64)
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
