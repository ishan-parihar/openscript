use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SfxAsset {
    pub id: String,
    pub path: String,
    pub category: String,
    pub subcategory: String,
    pub editorial_role: String,
    pub duration_ms: i64,
    pub sample_rate: u32,
    pub channels: u32,
    pub peak_db: f64,
    pub loudness_lufs: f64,
    pub recommended_gain_db: f64,
    pub recommended_use: String,
    pub safe_overlay: bool,
    pub tags: Vec<String>,
    pub filename: String,
}

/// Unified index wrapper — matches Python's create_sfx_index output.
/// Both Rust and Python read/write this format.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SfxIndexWrapper {
    pub version: String,
    pub created_at: String,
    pub sfx_path: String,
    pub total_assets: usize,
    pub categories: serde_json::Map<String, serde_json::Value>,
    pub editorial_roles: serde_json::Map<String, serde_json::Value>,
    pub assets: Vec<SfxAsset>,
    #[serde(default)]
    pub search_aliases: serde_json::Map<String, serde_json::Value>,
}

pub struct SfxIndex {
    assets: Vec<SfxAsset>,
    index_path: Option<PathBuf>,
}

impl SfxIndex {
    pub fn load(path: Option<&str>) -> Result<Self, std::io::Error> {
        match path {
            Some(p) => {
                let data = std::fs::read_to_string(p)?;
                // Try wrapper format first (Python-compatible), then fall back to flat array
                let assets: Vec<SfxAsset> =
                    if let Ok(wrapper) = serde_json::from_str::<SfxIndexWrapper>(&data) {
                        wrapper.assets
                    } else {
                        serde_json::from_str(&data).unwrap_or_default()
                    };
                Ok(Self {
                    assets,
                    index_path: Some(PathBuf::from(p)),
                })
            }
            None => Ok(Self {
                assets: Vec::new(),
                index_path: None,
            }),
        }
    }

    pub fn search(
        &self,
        query: &str,
        editorial_role: Option<&str>,
        category: Option<&str>,
        limit: usize,
    ) -> Vec<&SfxAsset> {
        self.assets
            .iter()
            .filter(|asset| {
                if !query.is_empty()
                    && !asset
                        .filename
                        .to_lowercase()
                        .contains(&query.to_lowercase())
                {
                    return false;
                }
                if let Some(role) = editorial_role {
                    if asset.editorial_role != role {
                        return false;
                    }
                }
                if let Some(cat) = category {
                    if !asset.category.to_lowercase().contains(&cat.to_lowercase()) {
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
        // Build category counts
        let mut categories = serde_json::Map::new();
        let mut editorial_roles = serde_json::Map::new();
        for asset in &self.assets {
            let cat_key = asset.category.clone();
            let cat_val = categories.entry(cat_key).or_insert(serde_json::json!(0));
            if let Some(n) = cat_val.as_u64() {
                *cat_val = serde_json::json!(n + 1);
            }

            let role_key = asset.editorial_role.clone();
            let role_val = editorial_roles
                .entry(role_key)
                .or_insert(serde_json::json!(0));
            if let Some(n) = role_val.as_u64() {
                *role_val = serde_json::json!(n + 1);
            }
        }

        let wrapper = SfxIndexWrapper {
            version: "1.0".to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            sfx_path: self
                .index_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
            total_assets: self.assets.len(),
            categories,
            editorial_roles,
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

    pub fn assets(&self) -> &[SfxAsset] {
        &self.assets
    }

    /// Scan a directory recursively for audio files and build an index.
    pub fn scan_directory(root: &str) -> Result<Self, std::io::Error> {
        use std::ffi::OsStr;

        let mut assets = Vec::new();
        let mut id_counter = 0;

        fn walk_dir(
            dir: &std::path::Path,
            assets: &mut Vec<SfxAsset>,
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

                            let rel_path = path
                                .parent()
                                .map(|p| {
                                    p.strip_prefix(dir)
                                        .map(|p| p.to_string_lossy().to_string())
                                        .unwrap_or_default()
                                })
                                .unwrap_or_default();

                            let path_str = path.to_string_lossy().to_lowercase();
                            let editorial_role = if path_str.contains("intro")
                                || path_str.contains("open")
                            {
                                "intro"
                            } else if path_str.contains("transition") {
                                "transition"
                            } else if path_str.contains("highlight") || path_str.contains("emphas")
                            {
                                "highlight"
                            } else if path_str.contains("cta") || path_str.contains("call") {
                                "cta"
                            } else if path_str.contains("outro") || path_str.contains("close") {
                                "outro"
                            } else if path_str.contains("ambience") || path_str.contains("ambient")
                            {
                                "ambience"
                            } else if path_str.contains("text") {
                                "text"
                            } else if path_str.contains("emotion") {
                                "emotion"
                            } else {
                                "general"
                            };

                            let recommended_use = if path_str.contains("stinger") {
                                "stinger"
                            } else if path_str.contains("riser") {
                                "riser"
                            } else if path_str.contains("loop") {
                                "loop"
                            } else if path_str.contains("bed") {
                                "bed"
                            } else {
                                "single_hit"
                            };

                            let safe_overlay =
                                path_str.contains("safe") || editorial_role == "ambience";

                            let duration_ms =
                                crate::probe_duration_ms(&path.to_string_lossy()).unwrap_or(0);

                            let asset = SfxAsset {
                                id: format!("sfx_{:04}", *counter),
                                path: path.to_string_lossy().to_string(),
                                category: rel_path.clone(),
                                subcategory: filename.clone(),
                                editorial_role: editorial_role.to_string(),
                                duration_ms,
                                sample_rate: 48000,
                                channels: 2,
                                peak_db: 0.0,
                                loudness_lufs: 0.0,
                                recommended_gain_db: -6.0,
                                recommended_use: recommended_use.to_string(),
                                safe_overlay,
                                tags: vec![filename.clone()],
                                filename,
                            };
                            assets.push(asset);
                            *counter += 1;
                        }
                    }
                }
            }
            Ok(())
        }

        walk_dir(std::path::Path::new(root), &mut assets, &mut id_counter)?;

        Ok(Self {
            assets,
            index_path: Some(PathBuf::from(root)),
        })
    }
}

impl Default for SfxIndex {
    fn default() -> Self {
        Self {
            assets: Vec::new(),
            index_path: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_asset(id: &str, filename: &str, role: &str, category: &str) -> SfxAsset {
        SfxAsset {
            id: id.into(),
            path: format!("/sfx/{}.wav", filename),
            category: category.into(),
            subcategory: "sub".into(),
            editorial_role: role.into(),
            duration_ms: 1000,
            sample_rate: 48000,
            channels: 2,
            peak_db: 0.0,
            loudness_lufs: -14.0,
            recommended_gain_db: -6.0,
            recommended_use: "single_hit".into(),
            safe_overlay: false,
            tags: vec![],
            filename: filename.into(),
        }
    }

    #[test]
    fn test_search_by_query() {
        let idx = SfxIndex {
            assets: vec![
                make_asset("1", "cinematic boom", "intro", "INTROS"),
                make_asset("2", "whoosh fast", "transition", "TRANSITIONS"),
                make_asset("3", "cinematic riser", "intro", "INTROS"),
            ],
            index_path: None,
        };
        let results = idx.search("cinematic", None, None, 10);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_search_by_editorial_role() {
        let idx = SfxIndex {
            assets: vec![
                make_asset("1", "boom", "intro", "INTROS"),
                make_asset("2", "whoosh", "transition", "TRANSITIONS"),
            ],
            index_path: None,
        };
        let results = idx.search("", Some("intro"), None, 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "1");
    }

    #[test]
    fn test_search_by_category() {
        let idx = SfxIndex {
            assets: vec![
                make_asset("1", "boom", "intro", "INTROS"),
                make_asset("2", "whoosh", "transition", "TRANSITIONS"),
            ],
            index_path: None,
        };
        let results = idx.search("", None, Some("TRANSITIONS"), 10);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_search_combined() {
        let idx = SfxIndex {
            assets: vec![
                make_asset("1", "cinematic boom", "intro", "INTROS"),
                make_asset("2", "cinematic whoosh", "transition", "TRANSITIONS"),
                make_asset("3", "boom loud", "intro", "INTROS"),
            ],
            index_path: None,
        };
        let results = idx.search("cinematic", Some("intro"), None, 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "1");
    }

    #[test]
    fn test_search_limit() {
        let idx = SfxIndex {
            assets: vec![
                make_asset("1", "boom", "intro", "INTROS"),
                make_asset("2", "boom 2", "intro", "INTROS"),
                make_asset("3", "boom 3", "intro", "INTROS"),
            ],
            index_path: None,
        };
        let results = idx.search("boom", None, None, 2);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_search_case_insensitive() {
        let idx = SfxIndex {
            assets: vec![make_asset("1", "Cinematic BOOM", "intro", "INTROS")],
            index_path: None,
        };
        let results = idx.search("cinematic", None, None, 10);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_load_empty_path() {
        let idx = SfxIndex::load(None).unwrap();
        assert_eq!(idx.len(), 0);
    }

    #[test]
    fn test_save_and_load() {
        let dir = std::env::temp_dir();
        let path = dir.join("test_sfx_index.json");
        let idx = SfxIndex {
            assets: vec![make_asset("1", "test", "intro", "INTROS")],
            index_path: None,
        };
        idx.save(path.to_str().unwrap()).unwrap();
        let loaded = SfxIndex::load(Some(path.to_str().unwrap())).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.assets()[0].id, "1");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_scan_directory_empty() {
        let dir = std::env::temp_dir().join("test_sfx_empty");
        let _ = std::fs::create_dir_all(&dir);
        let idx = SfxIndex::scan_directory(dir.to_str().unwrap()).unwrap();
        assert_eq!(idx.len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_scan_directory_finds_audio_files() {
        let dir = std::env::temp_dir().join("test_sfx_scan");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("boom.wav"), &[0u8; 10]).unwrap();
        std::fs::write(dir.join("whoosh.mp3"), &[0u8; 10]).unwrap();
        std::fs::write(dir.join("readme.txt"), &[0u8; 10]).unwrap();

        let idx = SfxIndex::scan_directory(dir.to_str().unwrap()).unwrap();
        assert_eq!(idx.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_scan_directory_recursive() {
        let dir = std::env::temp_dir().join("test_sfx_recursive");
        let sub = dir.join("transitions");
        let _ = std::fs::create_dir_all(&sub);
        std::fs::write(dir.join("intro_boom.wav"), &[0u8; 10]).unwrap();
        std::fs::write(sub.join("whoosh.wav"), &[0u8; 10]).unwrap();

        let idx = SfxIndex::scan_directory(dir.to_str().unwrap()).unwrap();
        assert_eq!(idx.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_scan_directory_editorial_role_from_path() {
        let dir = std::env::temp_dir().join("test_sfx_role");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("intro_boom.wav"), &[0u8; 10]).unwrap();
        std::fs::write(dir.join("transition_whoosh.wav"), &[0u8; 10]).unwrap();
        std::fs::write(dir.join("generic_sound.wav"), &[0u8; 10]).unwrap();

        let idx = SfxIndex::scan_directory(dir.to_str().unwrap()).unwrap();
        assert_eq!(idx.len(), 3);
        let intro = idx
            .assets()
            .iter()
            .find(|a| a.editorial_role == "intro")
            .unwrap();
        let transition = idx
            .assets()
            .iter()
            .find(|a| a.editorial_role == "transition")
            .unwrap();
        let general = idx
            .assets()
            .iter()
            .find(|a| a.editorial_role == "general")
            .unwrap();
        assert!(intro.filename.contains("intro"));
        assert!(transition.filename.contains("transition"));
        assert!(general.filename.contains("generic"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
