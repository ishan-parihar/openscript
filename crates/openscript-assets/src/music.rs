use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MusicAsset {
    pub id: String,
    pub path: String,
    pub title: String,
    pub artist: String,
    pub duration_ms: i64,
    pub sample_rate: u32,
    pub channels: u32,
    pub mood: String,
    pub energy: String,
    pub bpm: Option<u32>,
    pub loopability: bool,
    pub intro_friendly: bool,
    pub cta_friendly: bool,
    pub loudness_target_lufs: f64,
    pub tags: Vec<String>,
    pub genre: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loudness_lufs: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_db: Option<f64>,
}

/// Unified index wrapper — matches Python's create_music_index output.
/// Both Rust and Python read/write this format.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MusicIndexWrapper {
    pub version: String,
    pub created_at: String,
    pub music_paths: Vec<String>,
    pub total_assets: usize,
    pub moods: serde_json::Map<String, serde_json::Value>,
    pub energy_levels: serde_json::Map<String, serde_json::Value>,
    pub assets: Vec<MusicAsset>,
    #[serde(default)]
    pub search_aliases: serde_json::Map<String, serde_json::Value>,
}

pub struct MusicIndex {
    assets: Vec<MusicAsset>,
    _index_path: Option<PathBuf>,
    music_paths: Vec<String>,
}

impl MusicIndex {
    pub fn load(path: Option<&str>) -> Result<Self, std::io::Error> {
        match path {
            Some(p) => {
                let data = std::fs::read_to_string(p)?;
                // Try wrapper format first (Python-compatible), then fall back to flat array
                let (assets, paths) =
                    if let Ok(wrapper) = serde_json::from_str::<MusicIndexWrapper>(&data) {
                        (wrapper.assets, wrapper.music_paths)
                    } else {
                        (serde_json::from_str(&data).unwrap_or_default(), vec![])
                    };
                Ok(Self {
                    assets,
                    _index_path: Some(PathBuf::from(p)),
                    music_paths: paths,
                })
            }
            None => Ok(Self {
                assets: Vec::new(),
                _index_path: None,
                music_paths: vec![],
            }),
        }
    }

    pub fn search(
        &self,
        query: &str,
        mood: Option<&str>,
        energy: Option<&str>,
        intro_friendly: Option<bool>,
        cta_friendly: Option<bool>,
        loopable: Option<bool>,
        limit: usize,
    ) -> Vec<&MusicAsset> {
        self.assets
            .iter()
            .filter(|asset| {
                if !query.is_empty() {
                    let q = query.to_lowercase();
                    let title_match = asset.title.to_lowercase().contains(&q);
                    let filename_match = asset
                        .path
                        .rsplit('/')
                        .next()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&q);
                    if !title_match && !filename_match {
                        return false;
                    }
                }
                if let Some(m) = mood {
                    if asset.mood != m {
                        return false;
                    }
                }
                if let Some(e) = energy {
                    if asset.energy != e {
                        return false;
                    }
                }
                if let Some(v) = intro_friendly {
                    if asset.intro_friendly != v {
                        return false;
                    }
                }
                if let Some(v) = cta_friendly {
                    if asset.cta_friendly != v {
                        return false;
                    }
                }
                if let Some(v) = loopable {
                    if asset.loopability != v {
                        return false;
                    }
                }
                true
            })
            .take(limit)
            .collect()
    }

    /// Save in the unified wrapper format (Python-compatible).
    pub fn save(&self, path: &str) -> Result<(), std::io::Error> {
        let mut moods = serde_json::Map::new();
        let mut energy_levels = serde_json::Map::new();
        for asset in &self.assets {
            let mood_key = asset.mood.clone();
            let mood_val = moods.entry(mood_key).or_insert(serde_json::json!(0));
            if let Some(n) = mood_val.as_u64() {
                *mood_val = serde_json::json!(n + 1);
            }

            let energy_key = asset.energy.clone();
            let energy_val = energy_levels
                .entry(energy_key)
                .or_insert(serde_json::json!(0));
            if let Some(n) = energy_val.as_u64() {
                *energy_val = serde_json::json!(n + 1);
            }
        }

        let wrapper = MusicIndexWrapper {
            version: "1.0".to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            music_paths: self.music_paths.clone(),
            total_assets: self.assets.len(),
            moods,
            energy_levels,
            assets: self.assets.clone(),
            search_aliases: serde_json::Map::new(),
        };

        let data = serde_json::to_string_pretty(&wrapper)?;
        std::fs::write(path, data)
    }

    pub fn len(&self) -> usize {
        self.assets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.assets.is_empty()
    }

    pub fn assets(&self) -> &[MusicAsset] {
        &self.assets
    }

    /// Scan multiple directories recursively for audio files and build an index.
    pub fn scan_directories(roots: &[String]) -> Result<Self, std::io::Error> {
        use std::ffi::OsStr;

        let mut assets = Vec::new();
        let mut id_counter = 0;

        fn walk_dir(
            dir: &std::path::Path,
            assets: &mut Vec<MusicAsset>,
            counter: &mut usize,
        ) -> Result<(), std::io::Error> {
            if dir.is_dir() {
                for entry in std::fs::read_dir(dir)? {
                    let entry = entry?;
                    let path = entry.path();
                    if path.is_dir() {
                        walk_dir(&path, assets, counter)?;
                    } else if let Some(ext) = path.extension().and_then(OsStr::to_str) {
                        if matches!(ext, "wav" | "mp3" | "ogg" | "flac" | "aac") {
                            let filename = path
                                .file_stem()
                                .and_then(|s| s.to_str())
                                .unwrap_or("unknown")
                                .to_string();

                            let path_str = path.to_string_lossy().to_lowercase();
                            let duration_ms =
                                crate::probe_duration_ms(&path.to_string_lossy()).unwrap_or(0);

                            let mood = if path_str.contains("happy")
                                || path_str.contains("joy")
                                || path_str.contains("upbeat")
                            {
                                "upbeat"
                            } else if path_str.contains("sad")
                                || path_str.contains("melancholy")
                                || path_str.contains("melancholic")
                            {
                                "melancholic"
                            } else if path_str.contains("calm")
                                || path_str.contains("peaceful")
                                || path_str.contains("ambient")
                            {
                                "ambient"
                            } else if path_str.contains("dark")
                                || path_str.contains("tension")
                                || path_str.contains("dramatic")
                            {
                                "dramatic"
                            } else if path_str.contains("corporate")
                                || path_str.contains("business")
                                || path_str.contains("professional")
                            {
                                "corporate"
                            } else if path_str.contains("electronic")
                                || path_str.contains("synth")
                                || path_str.contains("techno")
                            {
                                "electronic"
                            } else {
                                "neutral"
                            };

                            let energy = if path_str.contains("high")
                                || path_str.contains("fast")
                                || path_str.contains("intense")
                                || path_str.contains("aggressive")
                            {
                                "high"
                            } else if path_str.contains("low")
                                || path_str.contains("slow")
                                || path_str.contains("soft")
                                || path_str.contains("gentle")
                            {
                                "low"
                            } else {
                                "medium"
                            };

                            let loopable = path_str.contains("loop")
                                || path_str.contains("seamless")
                                || path_str.contains("cycle");
                            let intro_friendly = path_str.contains("intro")
                                || path_str.contains("open")
                                || path_str.contains("start");
                            let cta_friendly = path_str.contains("cta")
                                || path_str.contains("call")
                                || (path_str.contains("action")
                                    && (path_str.contains("action_")
                                        || path_str.contains("-action")
                                        || path_str.contains("_action")))
                                || (path_str.contains("end")
                                    && (path_str.contains("ending")
                                        || path_str.contains("_end")
                                        || path_str.contains("-end")))
                                || path_str.contains("outro");

                            let asset = MusicAsset {
                                id: format!("music_{:04}", *counter),
                                path: path.to_string_lossy().to_string(),
                                title: filename.clone(),
                                artist: "Unknown".to_string(),
                                duration_ms,
                                sample_rate: 44100,
                                channels: 2,
                                mood: mood.to_string(),
                                energy: energy.to_string(),
                                bpm: None,
                                loopability: loopable,
                                intro_friendly,
                                cta_friendly,
                                loudness_target_lufs: -14.0,
                                tags: vec![filename.clone()],
                                genre: String::new(),
                                loudness_lufs: None,
                                peak_db: None,
                            };
                            assets.push(asset);
                            *counter += 1;
                        }
                    }
                }
            }
            Ok(())
        }

        for root in roots {
            walk_dir(std::path::Path::new(root), &mut assets, &mut id_counter)?;
        }

        Ok(Self {
            assets,
            _index_path: None,
            music_paths: roots.to_vec(),
        })
    }
}

impl Default for MusicIndex {
    fn default() -> Self {
        Self {
            assets: Vec::new(),
            _index_path: None,
            music_paths: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_music(id: &str, title: &str, mood: &str, energy: &str) -> MusicAsset {
        MusicAsset {
            id: id.into(),
            path: format!("/music/{}.mp3", title),
            title: title.into(),
            artist: "Unknown".into(),
            duration_ms: 180000,
            sample_rate: 44100,
            channels: 2,
            mood: mood.into(),
            energy: energy.into(),
            bpm: None,
            loopability: false,
            intro_friendly: false,
            cta_friendly: false,
            loudness_target_lufs: -14.0,
            tags: vec![],
            genre: "".into(),
            loudness_lufs: None,
            peak_db: None,
        }
    }

    #[test]
    fn test_search_by_mood() {
        let idx = MusicIndex {
            assets: vec![
                make_music("1", "Calm Track", "neutral", "low"),
                make_music("2", "Energetic Beat", "upbeat", "high"),
            ],
            _index_path: None,
            music_paths: vec![],
        };
        let results = idx.search("", Some("neutral"), None, None, None, None, 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "1");
    }

    #[test]
    fn test_search_by_energy() {
        let idx = MusicIndex {
            assets: vec![
                make_music("1", "Calm Track", "neutral", "low"),
                make_music("2", "Energetic Beat", "upbeat", "high"),
            ],
            _index_path: None,
            music_paths: vec![],
        };
        let results = idx.search("", None, Some("high"), None, None, None, 10);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_search_by_intro_friendly() {
        let mut asset = make_music("1", "Intro Song", "neutral", "medium");
        asset.intro_friendly = true;
        let idx = MusicIndex {
            assets: vec![asset, make_music("2", "Main Track", "neutral", "medium")],
            _index_path: None,
            music_paths: vec![],
        };
        let results = idx.search("", None, None, Some(true), None, None, 10);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_search_by_loopable() {
        let mut asset = make_music("1", "Loopable", "ambient", "low");
        asset.loopability = true;
        let idx = MusicIndex {
            assets: vec![asset, make_music("2", "One Shot", "ambient", "low")],
            _index_path: None,
            music_paths: vec![],
        };
        let results = idx.search("", None, None, None, None, Some(true), 10);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_search_query_title() {
        let idx = MusicIndex {
            assets: vec![
                make_music("1", "Summer Vibes", "happy", "medium"),
                make_music("2", "Winter Chill", "calm", "low"),
            ],
            _index_path: None,
            music_paths: vec![],
        };
        let results = idx.search("summer", None, None, None, None, None, 10);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_search_combined_filters() {
        let idx = MusicIndex {
            assets: vec![
                make_music("1", "Track A", "neutral", "low"),
                make_music("2", "Track B", "neutral", "high"),
                make_music("3", "Track C", "upbeat", "low"),
            ],
            _index_path: None,
            music_paths: vec![],
        };
        let results = idx.search("", Some("neutral"), Some("low"), None, None, None, 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "1");
    }

    #[test]
    fn test_load_empty() {
        let idx = MusicIndex::load(None).unwrap();
        assert_eq!(idx.len(), 0);
    }

    #[test]
    fn test_save_and_load() {
        let dir = std::env::temp_dir();
        let path = dir.join("test_music_index.json");
        let idx = MusicIndex {
            assets: vec![make_music("1", "Test", "neutral", "low")],
            _index_path: None,
            music_paths: vec![],
        };
        idx.save(path.to_str().unwrap()).unwrap();
        let loaded = MusicIndex::load(Some(path.to_str().unwrap())).unwrap();
        assert_eq!(loaded.len(), 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_scan_directories_empty() {
        let dir = std::env::temp_dir().join("test_music_empty");
        let _ = std::fs::create_dir_all(&dir);
        let idx = MusicIndex::scan_directories(&[dir.to_string_lossy().to_string()]).unwrap();
        assert_eq!(idx.len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_scan_directories_finds_audio_files() {
        let dir = std::env::temp_dir().join("test_music_scan");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("happy_track.mp3"), &[0u8; 10]).unwrap();
        std::fs::write(dir.join("calm_bg.wav"), &[0u8; 10]).unwrap();
        std::fs::write(dir.join("notes.txt"), &[0u8; 10]).unwrap();

        let idx = MusicIndex::scan_directories(&[dir.to_string_lossy().to_string()]).unwrap();
        assert_eq!(idx.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_scan_directories_multiple_roots() {
        let dir1 = std::env::temp_dir().join("test_music_root1");
        let dir2 = std::env::temp_dir().join("test_music_root2");
        let _ = std::fs::create_dir_all(&dir1);
        let _ = std::fs::create_dir_all(&dir2);
        std::fs::write(dir1.join("track1.mp3"), &[0u8; 10]).unwrap();
        std::fs::write(dir2.join("track2.mp3"), &[0u8; 10]).unwrap();

        let idx = MusicIndex::scan_directories(&[
            dir1.to_string_lossy().to_string(),
            dir2.to_string_lossy().to_string(),
        ])
        .unwrap();
        assert_eq!(idx.len(), 2);
        let _ = std::fs::remove_dir_all(&dir1);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    #[test]
    fn test_scan_directories_mood_from_filename() {
        let dir = std::env::temp_dir().join("test_music_mood");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("happy_summer.mp3"), &[0u8; 10]).unwrap();
        std::fs::write(dir.join("dark_tension.mp3"), &[0u8; 10]).unwrap();

        let idx = MusicIndex::scan_directories(&[dir.to_string_lossy().to_string()]).unwrap();
        assert_eq!(idx.len(), 2);
        let happy = idx.assets().iter().find(|a| a.mood == "upbeat").unwrap();
        let dark = idx.assets().iter().find(|a| a.mood == "dramatic").unwrap();
        assert!(happy.title.contains("happy"));
        assert!(dark.title.contains("dark"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
