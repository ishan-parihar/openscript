use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct VoiceProfile {
    /// Profile ID. Accepts "id" or legacy "profile_id" key.
    #[serde(default, alias = "profile_id")]
    pub id: String,
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub ref_audio: String,
    #[serde(default)]
    pub ref_text: String,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,
    #[serde(default = "default_created_at")]
    pub created_at: String,
}

fn default_provider() -> String {
    "faster-qwen3-tts".to_string()
}
fn default_mode() -> String {
    "clone".to_string()
}
fn default_language() -> String {
    "English".to_string()
}
fn default_sample_rate() -> u32 {
    24000
}
fn default_created_at() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub struct VoiceProfileRegistry {
    profiles: HashMap<String, VoiceProfile>,
    registry_path: PathBuf,
}

impl VoiceProfileRegistry {
    pub fn new(registry_path: &str) -> Result<Self, std::io::Error> {
        let path = PathBuf::from(registry_path);
        let profiles = if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            match serde_json::from_str(&content) {
                Ok(profiles) => profiles,
                Err(e) => {
                    eprintln!(
                        "[WARN] Failed to parse voice profile registry at {}: {}",
                        path.display(),
                        e
                    );
                    HashMap::new()
                }
            }
        } else {
            HashMap::new()
        };
        Ok(Self {
            profiles,
            registry_path: path,
        })
    }

    pub fn list(&self) -> Vec<&VoiceProfile> {
        self.profiles.values().collect()
    }

    pub fn add(&mut self, profile: VoiceProfile) -> Result<(), std::io::Error> {
        self.profiles.insert(profile.id.clone(), profile);
        self.save()
    }

    pub fn remove(&mut self, profile_id: &str) -> Result<Option<VoiceProfile>, std::io::Error> {
        let removed = self.profiles.remove(profile_id);
        if removed.is_some() {
            self.save()?;
        }
        Ok(removed)
    }

    pub fn get(&self, profile_id: &str) -> Option<&VoiceProfile> {
        self.profiles.get(profile_id)
    }

    pub fn save(&self) -> Result<(), std::io::Error> {
        if let Some(parent) = self.registry_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(&self.profiles)?;
        std::fs::write(&self.registry_path, content)
    }
}
