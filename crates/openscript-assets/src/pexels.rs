use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

pub const CONCEPT_ALIASES: &[(&str, &[&str])] = &[
    (
        "technology",
        &[
            "technology",
            "computer",
            "digital",
            "tech",
            "AI",
            "software",
            "coding",
        ],
    ),
    (
        "city",
        &[
            "city",
            "urban",
            "street",
            "buildings",
            "skyline",
            "downtown",
        ],
    ),
    (
        "people",
        &[
            "people",
            "person",
            "human",
            "crowd",
            "team",
            "group",
            "community",
        ],
    ),
    (
        "money",
        &[
            "money",
            "cash",
            "finance",
            "payment",
            "bank",
            "investment",
            "crypto",
        ],
    ),
    (
        "nature",
        &[
            "nature", "forest", "trees", "outdoor", "mountain", "ocean", "river",
        ],
    ),
    (
        "business",
        &[
            "business",
            "office",
            "meeting",
            "work",
            "corporate",
            "conference",
        ],
    ),
    (
        "food",
        &["food", "cooking", "kitchen", "restaurant", "meal", "dining"],
    ),
    (
        "travel",
        &[
            "travel",
            "airport",
            "hotel",
            "vacation",
            "adventure",
            "exploring",
        ],
    ),
    (
        "health",
        &[
            "health", "fitness", "gym", "exercise", "wellness", "medical",
        ],
    ),
    (
        "education",
        &[
            "education",
            "school",
            "learning",
            "study",
            "teaching",
            "classroom",
        ],
    ),
    (
        "emotion",
        &[
            "happy", "sad", "excited", "worried", "joy", "fear", "love", "anger",
        ],
    ),
    (
        "action",
        &[
            "running", "walking", "talking", "working", "exercise", "sports",
        ],
    ),
    (
        "growth",
        &[
            "growth",
            "success",
            "progress",
            "innovation",
            "startup",
            "business growth",
        ],
    ),
    (
        "failure",
        &[
            "failure",
            "crisis",
            "problem",
            "challenge",
            "risk",
            "danger",
        ],
    ),
    (
        "communication",
        &[
            "communication",
            "phone",
            "email",
            "message",
            "social media",
            "networking",
        ],
    ),
    (
        "time",
        &[
            "time", "clock", "deadline", "schedule", "calendar", "planning",
        ],
    ),
    (
        "data",
        &[
            "data",
            "analytics",
            "chart",
            "graph",
            "statistics",
            "report",
        ],
    ),
    (
        "security",
        &[
            "security",
            "privacy",
            "protection",
            "lock",
            "shield",
            "safe",
        ],
    ),
    (
        "creativity",
        &[
            "creativity",
            "art",
            "design",
            "music",
            "painting",
            "drawing",
        ],
    ),
    (
        "home",
        &[
            "home",
            "house",
            "living room",
            "family",
            "interior",
            "furniture",
        ],
    ),
    (
        "car",
        &[
            "car",
            "vehicle",
            "driving",
            "transportation",
            "traffic",
            "road",
        ],
    ),
    (
        "weather",
        &["weather", "rain", "sun", "cloud", "storm", "snow", "wind"],
    ),
    (
        "science",
        &[
            "science",
            "laboratory",
            "experiment",
            "research",
            "discovery",
        ],
    ),
    (
        "shopping",
        &["shopping", "store", "mall", "buy", "purchase", "ecommerce"],
    ),
    (
        "celebration",
        &[
            "celebration",
            "party",
            "birthday",
            "wedding",
            "festival",
            "holiday",
        ],
    ),
];

/// Match text against CONCEPT_ALIASES and return the best matching concept key.
pub fn match_concept(text: &str) -> Option<String> {
    let text_lower = text.to_lowercase();
    let words: Vec<&str> = text_lower.split_whitespace().collect();

    let mut best_match: Option<(String, usize)> = None;

    for (concept_key, aliases) in CONCEPT_ALIASES {
        let mut match_count = 0;
        for alias in *aliases {
            let alias_lower = alias.to_lowercase();
            if alias_lower.contains(' ') {
                if text_lower.contains(&alias_lower) {
                    match_count += 1;
                }
            } else {
                for word in &words {
                    if word == &alias_lower || word.starts_with(&alias_lower) {
                        match_count += 1;
                        break;
                    }
                }
            }
        }
        if match_count > 0 {
            if let Some((_, best_count)) = &best_match {
                if match_count > *best_count {
                    best_match = Some((concept_key.to_string(), match_count));
                }
            } else {
                best_match = Some((concept_key.to_string(), match_count));
            }
        }
    }

    best_match.map(|(key, _)| key)
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PexelsVideo {
    pub id: i64,
    pub width: i64,
    pub height: i64,
    pub url: String,
    pub image: String,
    pub video_files: Vec<PexelsVideoFile>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PexelsVideoFile {
    pub id: i64,
    pub quality: String,
    pub width: i64,
    pub height: i64,
    pub link: String,
    pub size: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum PexelsError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Pexels API error: {0}")]
    Api(String),
    #[error("No API key set")]
    NoApiKey,
}

pub struct PexelsClient {
    http: Client,
    api_key: String,
    cache: HashMap<String, Vec<PexelsVideo>>,
    cache_dir: PathBuf,
}

impl PexelsClient {
    pub fn new(api_key: &str, cache_dir: &str) -> Self {
        Self {
            http: Client::new(),
            api_key: api_key.to_string(),
            cache: HashMap::new(),
            cache_dir: PathBuf::from(cache_dir),
        }
    }

    /// Search for a single b-roll slot using concept aliases.
    /// Tries concept aliases in priority order and returns the first video found.
    pub async fn search_for_slot(
        &mut self,
        concept: &str,
        orientation: &str,
        quality: &str,
    ) -> Result<Option<PexelsVideo>, PexelsError> {
        // Try concept aliases in priority order
        let aliases = CONCEPT_ALIASES
            .iter()
            .find(|(key, _)| *key == concept)
            .map(|(_, aliases)| *aliases);

        if let Some(alias_list) = aliases {
            for alias in alias_list {
                match self.fetch_page(alias, orientation, 1, quality).await {
                    Ok(mut vids) => {
                        if !vids.is_empty() {
                            return Ok(Some(vids.remove(0)));
                        }
                    }
                    Err(_) => continue,
                }
            }
        } else {
            // No alias found, try the concept directly
            match self.fetch_page(concept, orientation, 1, quality).await {
                Ok(mut vids) => {
                    if !vids.is_empty() {
                        return Ok(Some(vids.remove(0)));
                    }
                }
                Err(_) => {}
            }
        }

        Ok(None)
    }

    pub async fn search(
        &mut self,
        concept: &str,
        orientation: &str,
        quality: &str,
    ) -> Result<Vec<PexelsVideo>, PexelsError> {
        if self.api_key.is_empty() {
            return Err(PexelsError::NoApiKey);
        }

        let cache_key = format!("{}|{}", concept, orientation);
        if let Some(cached) = self.cache.get(&cache_key) {
            return Ok(cached.clone());
        }

        let aliases = CONCEPT_ALIASES
            .iter()
            .find(|(key, _)| *key == concept)
            .map(|(_, aliases)| *aliases);

        let videos = if let Some(alias_list) = aliases {
            let mut all_videos: HashMap<i64, PexelsVideo> = HashMap::new();
            for alias in alias_list {
                match self.fetch_page(alias, orientation, 1, quality).await {
                    Ok(mut vids) => {
                        for v in vids.drain(..) {
                            all_videos.entry(v.id).or_insert(v);
                        }
                    }
                    Err(_) => continue,
                }
            }
            all_videos.into_values().collect()
        } else {
            self.fetch_page(concept, orientation, 1, quality).await?
        };

        let cache_key_file = self
            .cache_dir
            .join(format!("{}.json", cache_key.replace(' ', "_")));
        if let Some(parent) = cache_key_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(data) = serde_json::to_string(&videos) {
            let _ = std::fs::write(&cache_key_file, data);
        }

        let results: Vec<PexelsVideo> = videos.into_iter().take(15).collect();
        self.cache.insert(cache_key, results.clone());
        Ok(results)
    }

    pub async fn download(
        &self,
        video_file: &PexelsVideoFile,
        dest: &str,
    ) -> Result<String, PexelsError> {
        let resp = self.http.get(&video_file.link).send().await?;
        let bytes = resp.bytes().await?;
        std::fs::write(dest, &bytes)?;
        Ok(dest.to_string())
    }

    /// Pick the best video file for 9:16 (portrait) output and download to cache.
    /// Returns the cached file path.
    pub async fn download_best(
        &self,
        video: &PexelsVideo,
        concept: &str,
    ) -> Result<String, PexelsError> {
        let _ = std::fs::create_dir_all(&self.cache_dir);

        let best = self.pick_best_video_file(video);
        let Some(best_file) = best else {
            return Err(PexelsError::Api(format!(
                "No suitable video file found for video {}",
                video.id
            )));
        };

        let ext = best_file
            .link
            .rsplit('.')
            .next()
            .unwrap_or("mp4")
            .split('?')
            .next()
            .unwrap_or("mp4");
        let dest = self.cache_dir.join(format!(
            "{}_{}.{}",
            concept.replace(' ', "_"),
            video.id,
            ext
        ));
        let dest_str = dest.to_string_lossy().to_string();

        if dest.exists() {
            return Ok(dest_str);
        }

        self.download(best_file, &dest_str).await
    }

    /// Pick the best video file for 9:16 output: prefer portrait orientation, highest quality.
    fn pick_best_video_file<'a>(&self, video: &'a PexelsVideo) -> Option<&'a PexelsVideoFile> {
        if video.video_files.is_empty() {
            return None;
        }

        video
            .video_files
            .iter()
            .filter(|f| !f.link.is_empty())
            .max_by_key(|f| {
                let is_portrait = f.height > f.width;
                let is_hd = f.height >= 1920 || f.width >= 1920;
                let size_bonus = f.size;
                (is_portrait as i64) * 10_000_000 + (is_hd as i64) * 1_000_000 + size_bonus
            })
    }

    async fn fetch_page(
        &self,
        query: &str,
        orientation: &str,
        page: i64,
        _quality: &str,
    ) -> Result<Vec<PexelsVideo>, PexelsError> {
        // Map aspect-ratio notation to Pexels API orientation values
        let orientation = match orientation {
            "9:16" | "portrait" | "vertical" => "portrait",
            "16:9" | "landscape" | "horizontal" => "landscape",
            "1:1" | "square" => "square",
            _ => "portrait", // default to portrait for vertical video
        };
        let url = format!(
            "https://api.pexels.com/videos/search?query={}&per_page=15&orientation={}&page={}",
            urlencoding::encode(query),
            orientation,
            page
        );

        let resp = self
            .http
            .get(&url)
            .header("Authorization", &self.api_key)
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(PexelsError::Api(format!(
                "API returned status {}",
                resp.status()
            )));
        }

        let body: serde_json::Value = resp.json().await?;
        let videos = body
            .get("videos")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let mut results = Vec::new();
        for v in videos {
            let id = v.get("id").and_then(|x| x.as_i64()).unwrap_or(0);
            let width = v.get("width").and_then(|x| x.as_i64()).unwrap_or(0);
            let height = v.get("height").and_then(|x| x.as_i64()).unwrap_or(0);
            let url = v
                .get("url")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let image = v
                .get("image")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();

            let video_files = v
                .get("video_files")
                .and_then(|x| x.as_array())
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|f| {
                    Some(PexelsVideoFile {
                        id: f.get("id").and_then(|x| x.as_i64()).unwrap_or(0),
                        quality: f
                            .get("quality")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string(),
                        width: f.get("width").and_then(|x| x.as_i64()).unwrap_or(0),
                        height: f.get("height").and_then(|x| x.as_i64()).unwrap_or(0),
                        link: f
                            .get("link")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string(),
                        size: f.get("size").and_then(|x| x.as_i64()).unwrap_or(0),
                    })
                })
                .collect();

            results.push(PexelsVideo {
                id,
                width,
                height,
                url,
                image,
                video_files,
            });
        }

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_creation() {
        let client = PexelsClient::new("test_key", "/tmp/cache");
        assert_eq!(client.api_key, "test_key");
    }

    #[test]
    fn test_no_api_key_error() {
        let mut client = PexelsClient::new("", "/tmp/cache");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(client.search("technology", "portrait", "sd"));
        assert!(matches!(result, Err(PexelsError::NoApiKey)));
    }

    #[test]
    fn test_concept_alias_lookup() {
        let tech_aliases = CONCEPT_ALIASES
            .iter()
            .find(|(k, _)| *k == "technology")
            .map(|(_, v)| *v);
        assert!(tech_aliases.is_some());
        let aliases = tech_aliases.unwrap();
        assert!(aliases.contains(&"computer"));
        assert!(aliases.contains(&"tech"));
    }

    #[test]
    fn test_unknown_concept_has_no_alias() {
        let result = CONCEPT_ALIASES.iter().find(|(k, _)| *k == "unknown");
        assert!(result.is_none());
    }
}
