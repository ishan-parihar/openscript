use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// One emotional delivery take attached to a voice profile — the "tonality
/// template" building block for voice-cloning presets.
///
/// A single reference WAV captures the profile's neutral timbre; each
/// emotion take is a SEPARATE reference recording of the same speaker
/// delivering the emotion (e.g. an angry reading, a whisper). At synth time
/// a scene's `emote` selects the matching take so every line is attuned to
/// the required tonality instead of being synthesized with the neutral
/// clone timbre.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct EmotionTake {
    /// Reference audio for this emotional delivery (WAV).
    #[serde(default)]
    pub ref_audio: String,
    /// Exact transcript of the emotion reference audio.
    #[serde(default)]
    pub ref_text: String,
    /// Reference-fidelity knob (carried by gepard; retained for compatibility):
    /// higher = timbre clings closer to THIS emotion reference (1.0 = default).
    #[serde(default)]
    pub cfg_scale: Option<f64>,
    /// Optional per-emotion speech speed multiplier.
    #[serde(default)]
    pub speed: Option<f64>,
}

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

    /// Emotion-template map: `{emotion_id -> EmotionTake}`. When a scene
    /// carries an `emote`, synthesis uses the matching take (per-engine:
    /// audio8 = compound `{id}@{emotion}` registered voice) instead of the
    /// neutral base reference.
    #[serde(default)]
    pub emotions: HashMap<String, EmotionTake>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_emotions_template_roundtrip() {
        let mut profile = VoiceProfile {
            id: "ishan".into(),
            provider: "audio8".into(),
            mode: "clone".into(),
            model: String::new(),
            ref_audio: "base.wav".into(),
            ref_text: String::new(),
            language: "English".into(),
            description: None,
            sample_rate: 22050,
            created_at: String::new(),
            emotions: HashMap::new(),
        };
        profile.emotions.insert(
            "angry".into(),
            EmotionTake {
                ref_audio: "angry.wav".into(),
                ref_text: "I am furious!".into(),
                cfg_scale: Some(1.5),
                speed: Some(1.1),
            },
        );
        let json = serde_json::to_string(&profile).unwrap();
        let back: VoiceProfile = serde_json::from_str(&json).unwrap();
        let take = back.emotions.get("angry").unwrap();
        assert_eq!(take.ref_audio, "angry.wav");
        assert_eq!(take.cfg_scale, Some(1.5));
        assert_eq!(take.speed, Some(1.1));
    }

    #[test]
    fn test_emotions_default_to_empty_map() {
        // Old profiles without an emotions key must deserialize fine.
        let json = r#"{"id":"k1","provider":"kokoro","mode":"preset","ref_audio":"","ref_text":"","language":"en"}"#;
        let profile: VoiceProfile = serde_json::from_str(json).unwrap();
        assert!(profile.emotions.is_empty());
    }
}

