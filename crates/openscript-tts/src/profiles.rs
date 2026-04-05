use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct VoiceProfile {
    pub id: String,
    pub provider: String,
    pub mode: String,
    pub model: String,
    pub ref_audio: String,
    pub ref_text: String,
    pub language: String,
    pub description: Option<String>,
    pub sample_rate: u32,
    pub created_at: String,
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
