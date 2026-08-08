//! User-curated footage library (asset-development pipeline).
//!
//! This is the WRITE side of the asset surface and is strictly SEPARATE from
//! the video-generation pipeline: the `asset.*` tools write this index, while
//! generation only READS it (Tier 1 in `scene_media::fetch_scene_background`).
//!
//! Storage: media files live in `mcp/assets/user_library/` (gitignored — user
//! footage is not committed), the index lives at `mcp/assets/user_library_index.json`
//! with the same schema conventions as `music_index.json` / `sfx_index.json`.
//!
//! Curation lifecycle: `candidate` → `approved` | `rejected`.
//! Only `approved` assets with `quality_rating >= LIBRARY_QUALITY_FLOOR`
//! (default 3.0) are eligible for generation.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::ToolError;

/// Index JSON path (schema ships in-repo; media itself is gitignored).
pub const LIBRARY_INDEX_PATH: &str = "mcp/assets/user_library_index.json";
/// Media root for the user's curated footage.
pub const LIBRARY_ROOT: &str = "mcp/assets/user_library";
/// Curation status string constants.
pub const STATUS_CANDIDATE: &str = "candidate";
pub const STATUS_APPROVED: &str = "approved";
pub const STATUS_REJECTED: &str = "rejected";

const INDEX_VERSION: u32 = 2;

/// One curated footage entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetEntry {
    pub id: String,
    pub path: String,
    /// Content fingerprint (dedup key across ingests and providers).
    pub content_hash: String,
    /// "user_upload" | "pexels" | "pixabay" | "youtube"
    pub source: String,
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub duration_s: f64,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    #[serde(default)]
    pub aspect: String,
    #[serde(default)]
    pub title: String,
    /// Auto-tagged from filename stem + user refinements.
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub mood: String,
    #[serde(default)]
    pub energy: String,
    #[serde(default)]
    pub motion_intensity: String,
    /// User-classified quality 0–5.
    #[serde(default)]
    pub quality_rating: f64,
    /// Per-keyword relevance 0–1 (user-classified; drives generation search).
    #[serde(default)]
    pub relevance: HashMap<String, f64>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// STATUS_CANDIDATE | STATUS_APPROVED | STATUS_REJECTED.
    #[serde(default = "default_status")]
    pub curation_status: String,
    #[serde(default)]
    pub usage_count: u64,
    #[serde(default)]
    pub last_used_at: Option<String>,
    #[serde(default)]
    pub rated_at: Option<String>,
    #[serde(default)]
    pub created_at: String,
}

fn default_status() -> String {
    STATUS_CANDIDATE.to_string()
}

/// The library index file.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AssetLibrary {
    pub version: u32,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub root: String,
    pub total_assets: usize,
    #[serde(default)]
    pub assets: Vec<AssetEntry>,
    #[serde(default)]
    pub search_aliases: HashMap<String, Vec<String>>,
}

/// Result of an ingest pass.
#[derive(Debug, Default)]
pub struct IngestReport {
    pub indexed: usize,
    pub skipped_duplicates: usize,
    pub errors: Vec<String>,
}

impl Default for AssetEntry {
    fn default() -> Self {
        Self {
            id: String::new(),
            path: String::new(),
            content_hash: String::new(),
            source: "user_upload".to_string(),
            provider_id: None,
            duration_s: 0.0,
            width: 0,
            height: 0,
            aspect: String::new(),
            title: String::new(),
            keywords: Vec::new(),
            mood: String::new(),
            energy: String::new(),
            motion_intensity: String::new(),
            quality_rating: 0.0,
            relevance: HashMap::new(),
            tags: Vec::new(),
            curation_status: default_status(),
            usage_count: 0,
            last_used_at: None,
            rated_at: None,
            created_at: String::new(),
        }
    }
}

impl AssetLibrary {
    /// Load the index; an empty default when the file is absent or corrupt.
    pub fn load() -> Result<Self, ToolError> {
        let raw = std::fs::read_to_string(LIBRARY_INDEX_PATH);
        match raw {
            Ok(s) => serde_json::from_str(&s).map_err(|e| {
                ToolError::Asset(format!("user_library_index.json parse error: {e}"))
            }),
            Err(_) => Ok(Self::default()),
        }
    }

    /// Persist the index (also refreshes the version/root/count fields).
    pub fn save(&self) -> Result<(), ToolError> {
        let mut next = self.clone();
        next.version = INDEX_VERSION;
        next.root = LIBRARY_ROOT.to_string();
        next.total_assets = next.assets.len();
        let parent = Path::new(LIBRARY_INDEX_PATH)
            .parent()
            .unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        let s = serde_json::to_string_pretty(&next)
            .map_err(|e| ToolError::Asset(format!("index serialization error: {e}")))?;
        std::fs::write(LIBRARY_INDEX_PATH, s)?;
        Ok(())
    }

    /// Find an entry by content hash (dedup).
    pub fn by_hash(&self, hash: &str) -> Option<&AssetEntry> {
        self.assets.iter().find(|a| a.content_hash == hash)
    }

    /// Find an entry by id.
    pub fn by_id(&self, id: &str) -> Option<&AssetEntry> {
        self.assets.iter().find(|a| a.id == id)
    }

    /// Insert or replace an entry by id.
    pub fn upsert(&mut self, entry: AssetEntry) {
        if let Some(pos) = self.assets.iter().position(|a| a.id == entry.id) {
            self.assets[pos] = entry;
        } else {
            self.assets.push(entry);
        }
    }

    /// Generation-side search: `approved` assets with quality ≥ floor, ranked by
    /// relevance-to-signal × quality × freshness (least-recently-used wins ties).
    /// Returns at most 5 candidates; the caller picks the first.
    pub fn search(&self, signal: &[String], quality_floor: f64) -> Vec<AssetEntry> {
        let mut scored: Vec<(f64, usize, &AssetEntry)> = Vec::new();
        for (i, a) in self.assets.iter().enumerate() {
            if a.curation_status != STATUS_APPROVED || a.quality_rating < quality_floor {
                continue;
            }
            let rel = self.relevance_to_signal(a, signal);
            let kw_hits = a
                .keywords
                .iter()
                .filter(|k| signal.iter().any(|s| s == *k))
                .count() as f64;
            if rel <= 0.0 && kw_hits == 0.0 {
                continue;
            }
            let freshness = 1.0 / (1.0 + a.usage_count as f64);
            let score = (rel + kw_hits * 0.2) * (a.quality_rating / 5.0) * freshness;
            scored.push((score, i, a));
        }
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        scored
            .into_iter()
            .take(5)
            .map(|(_, _, a)| a.clone())
            .collect()
    }

    /// Mean per-token relevance of the entry against the scene signal tokens.
    fn relevance_to_signal(&self, a: &AssetEntry, signal: &[String]) -> f64 {
        if a.relevance.is_empty() {
            return 0.0;
        }
        let mut sum = 0.0;
        let mut n = 0;
        for tok in signal {
            if let Some(r) = a.relevance.get(tok) {
                sum += r;
                n += 1;
            }
        }
        if n == 0 {
            0.0
        } else {
            sum / n as f64
        }
    }

    /// Mark an asset used by generation (LRU freshness + usage stats).
    pub fn mark_used(&mut self, id: &str) {
        if let Some(a) = self.assets.iter_mut().find(|a| a.id == id) {
            a.usage_count = a.usage_count.saturating_add(1);
            a.last_used_at = Some(now_iso());
        }
    }

    /// Apply a user/agent classification (asset.rate).
    pub fn rate(
        &mut self,
        id: &str,
        relevance: HashMap<String, f64>,
        quality_rating: f64,
        mood: &str,
        energy: &str,
        motion_intensity: &str,
        tags: Vec<String>,
        status: &str,
    ) -> Option<&AssetEntry> {
        let a = self.assets.iter_mut().find(|a| a.id == id)?;
        a.relevance = relevance;
        a.quality_rating = quality_rating;
        if !mood.is_empty() {
            a.mood = mood.to_string();
        }
        if !energy.is_empty() {
            a.energy = energy.to_string();
        }
        if !motion_intensity.is_empty() {
            a.motion_intensity = motion_intensity.to_string();
        }
        a.tags = tags;
        a.curation_status = status.to_string();
        a.rated_at = Some(now_iso());
        Some(a)
    }

    /// Scan a directory (default LIBRARY_ROOT) and index new media files.
    /// Idempotent: content-hash dedup skips already-indexed files.
    pub async fn ingest_dir(&mut self, dir: &str) -> Result<IngestReport, ToolError> {
        let mut report = IngestReport::default();
        if !Path::new(dir).exists() {
            std::fs::create_dir_all(dir)?;
            return Ok(report);
        }
        let mut entries: Vec<_> = std::fs::read_dir(dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| is_media_file(p))
            .collect();
        entries.sort();
        let mut next_id = next_asset_id(&self.assets);

        for path in entries {
            let pstr = path.to_string_lossy().to_string();
            let hash = match crate::tools::file_content_fingerprint(&pstr) {
                Some(h) => h,
                None => {
                    report.errors.push(format!("{pstr}: unreadable"));
                    continue;
                }
            };
            if self.by_hash(&hash).is_some() {
                report.skipped_duplicates += 1;
                continue;
            }
            let meta = match openscript_ffmpeg::probe::probe(&pstr).await {
                Ok(m) => m,
                Err(_) => {
                    report.errors.push(format!("{pstr}: ffprobe failed"));
                    continue;
                }
            };
            let width = meta.width.unwrap_or(0);
            let height = meta.height.unwrap_or(0);
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("untitled");
            let entry = AssetEntry {
                id: next_id.clone(),
                path: pstr.clone(),
                content_hash: hash,
                source: "user_upload".to_string(),
                provider_id: None,
                duration_s: meta.duration,
                width,
                height,
                aspect: aspect_label(width, height),
                title: stem.to_string(),
                keywords: auto_keywords_from_stem(stem),
                created_at: now_iso(),
                ..AssetEntry::default()
            };
            self.upsert(entry);
            report.indexed += 1;
            next_id = bump_id(&next_id);
        }
        Ok(report)
    }

    /// Add an external clip (imported from a provider or copied locally).
    /// Returns the new asset id.
    pub async fn add_external(
        &mut self,
        path: &str,
        source: &str,
        provider_id: Option<String>,
        title: &str,
        keywords: Vec<String>,
    ) -> Result<String, ToolError> {
        let id = next_asset_id(&self.assets);
        let hash = crate::tools::file_content_fingerprint(path).ok_or_else(|| {
            ToolError::Asset(format!("imported file unreadable: {path}"))
        })?;
        if let Some(existing) = self.by_hash(&hash) {
            return Ok(existing.id.clone());
        }
        let meta = openscript_ffmpeg::probe::probe(path).await.map_err(|e| {
            ToolError::Asset(format!("imported file probe failed: {e}"))
        })?;
        let width = meta.width.unwrap_or(0);
        let height = meta.height.unwrap_or(0);
        let entry = AssetEntry {
            id: id.clone(),
            path: path.to_string(),
            content_hash: hash,
            source: source.to_string(),
            provider_id,
            duration_s: meta.duration,
            width,
            height,
            aspect: aspect_label(width, height),
            title: title.to_string(),
            keywords,
            created_at: now_iso(),
            ..AssetEntry::default()
        };
        self.upsert(entry);
        Ok(id)
    }

    /// Remove an entry by id (asset.library cleanup).
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.assets.len();
        self.assets.retain(|a| a.id != id);
        self.assets.len() != before
    }
}

/// True for the media extensions the library indexes.
pub fn is_media_file(p: &Path) -> bool {
    matches!(
        p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("mp4" | "mov" | "webm" | "mkv" | "avi")
    )
}

/// Best-effort aspect label from width/height.
fn aspect_label(width: u32, height: u32) -> String {
    if width == 0 || height == 0 {
        return String::new();
    }
    if (width as f64 / height as f64 - 9.0 / 16.0).abs() < 0.1 {
        "9:16".to_string()
    } else if (width as f64 / height as f64 - 16.0 / 9.0).abs() < 0.1 {
        "16:9".to_string()
    } else if (width as f64 / height as f64 - 1.0).abs() < 0.1 {
        "1:1".to_string()
    } else {
        format!("{width}:{height}")
    }
}

/// Filename-stem → search keywords (`morning_desk_01.mp4` → morning, desk).
pub fn auto_keywords_from_stem(stem: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in stem.split(['_', '-', ' ', '.']) {
        let p = part.trim().to_ascii_lowercase();
        if p.chars().count() >= 3 && !p.chars().all(|c| c.is_ascii_digit()) {
            if !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

/// RFC-3339-ish timestamp for created/rated/used fields.
fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Highest existing numeric suffix + 1 (`ul_0001` after `ul_0000`), stable
/// across removals (unlike `assets.len() + 1`, which can collide after deletes).
fn next_asset_id(assets: &[AssetEntry]) -> String {
    let max = assets
        .iter()
        .filter_map(|a| a.id.strip_prefix("ul_"))
        .filter_map(|s| s.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    format!("ul_{:04}", max + 1)
}

/// Increment a `ul_XXXX` id.
fn bump_id(id: &str) -> String {
    let n = id
        .strip_prefix("ul_")
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    format!("ul_{:04}", n + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, status: &str, quality: f64, kw: &[&str]) -> AssetEntry {
        let mut relevance = HashMap::new();
        for k in kw {
            relevance.insert(k.to_string(), 0.9);
        }
        AssetEntry {
            id: id.to_string(),
            content_hash: id.to_string(),
            curation_status: status.to_string(),
            quality_rating: quality,
            keywords: kw.iter().map(|k| k.to_string()).collect(),
            relevance,
            ..AssetEntry::default()
        }
    }

    #[test]
    fn search_returns_only_approved_above_quality_floor() {
        let mut lib = AssetLibrary::default();
        lib.assets = vec![
            entry("a", STATUS_APPROVED, 4.0, &["morning", "desk"]),
            entry("b", STATUS_CANDIDATE, 5.0, &["morning", "desk"]),
            entry("c", STATUS_APPROVED, 2.0, &["morning", "desk"]),
        ];
        let hits = lib.search(&["morning".to_string(), "desk".to_string()], 3.0);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "a");
    }

    #[test]
    fn search_ranks_relevance_and_freshness() {
        let mut lib = AssetLibrary::default();
        let mut used_high = entry("used", STATUS_APPROVED, 5.0, &["morning", "desk"]);
        used_high.usage_count = 4;
        let fresh = entry("fresh", STATUS_APPROVED, 4.0, &["morning", "desk"]);
        lib.assets = vec![used_high, fresh];
        let hits = lib.search(&["morning".to_string()], 3.0);
        // Freshness penalty outweighs the small quality gap: fresh first.
        assert_eq!(hits[0].id, "fresh");
    }

    #[test]
    fn rate_updates_entry_and_marks_rated_at() {
        let mut lib = AssetLibrary::default();
        lib.assets = vec![entry("x", STATUS_CANDIDATE, 0.0, &["morning"])];
        let mut relevance = HashMap::new();
        relevance.insert("sunrise".to_string(), 0.95);
        let rated = lib.rate(
            "x",
            relevance,
            4.5,
            "calm",
            "low",
            "slow",
            vec!["vertical".to_string()],
            STATUS_APPROVED,
        );
        assert!(rated.is_some());
        let a = &lib.assets[0];
        assert_eq!(a.curation_status, STATUS_APPROVED);
        assert_eq!(a.quality_rating, 4.5);
        assert_eq!(a.relevance.get("sunrise"), Some(&0.95));
        assert!(a.rated_at.is_some());
    }

    #[test]
    fn auto_keywords_strip_digits_and_short_words() {
        let kw = auto_keywords_from_stem("Morning_Desk_01_v2");
        assert!(kw.contains(&"morning".to_string()));
        assert!(kw.contains(&"desk".to_string()));
        assert!(!kw.iter().any(|k| k.chars().all(|c| c.is_ascii_digit())));
    }

    #[test]
    fn upsert_replaces_by_id() {
        let mut lib = AssetLibrary::default();
        let mut a = entry("x", STATUS_CANDIDATE, 0.0, &["morning"]);
        a.title = "first".to_string();
        lib.upsert(a);
        let mut b = entry("x", STATUS_CANDIDATE, 0.0, &["morning"]);
        b.title = "second".to_string();
        lib.upsert(b);
        assert_eq!(lib.assets.len(), 1);
        assert_eq!(lib.assets[0].title, "second");
    }
}
