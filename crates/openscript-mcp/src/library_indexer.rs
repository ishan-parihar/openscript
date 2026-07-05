//! Music/SFX library indexer — Rust port of `mcp/scripts/music_library_indexer.py`.
//!
//! Closes the C2 audit gap: `library.build` previously required Python at
//! runtime (shelled out to `music_library_indexer.py --build`). This module
//! reimplements the `--build` path in pure Rust, shelling out to `yt-dlp`
//! (the same CLI the Python script uses) and building the JSON index with
//! `serde_json`. No Python dependency.
//!
//! The `--search` and `--download` modes of the Python script were already
//! dead code — Rust has its own `library.search` and `library.download` MCP
//! tool handlers that do not call the Python script.

use serde_json::{json, Value};
use std::path::Path;
use tokio::process::Command;

/// Source channel definitions (mirrors MUSIC_SOURCES + SFX_SOURCES in the
/// Python script).
const MUSIC_SOURCES: &[(&str, &str)] = &[
    ("NoCopyrightSounds", "https://www.youtube.com/@NoCopyrightSounds"),
    ("AudioLibrary", "https://www.youtube.com/channel/UCQsBfyc5eOobgCzeY8bBzFg"),
    ("BreakingCopyright", "https://www.youtube.com/@BreakingCopyright"),
    ("VlogNoCopyrightMusic", "https://www.youtube.com/@VlogNoCopyrightMusic"),
    ("MixtureOfficial", "https://www.youtube.com/channel/UCkRrhwhJ2Ia_ZlkTQ4XFWJA"),
];

const SFX_SOURCES: &[(&str, &str)] = &[
    ("SoundLibrary1", "https://www.youtube.com/@SoundLibrary1"),
    ("YouTubeSoundEffects", "https://www.youtube.com/@youtubesoundeffects2692/videos"),
];

/// Sanitise a video title into a filesystem-safe filename (mirrors the Python
/// `sanitize_filename` function). Consecutive non-alphanumeric chars are
/// collapsed to a single underscore; leading/trailing underscores are trimmed.
fn sanitize_filename(title: &str) -> String {
    let mut result = String::with_capacity(title.len());
    let mut prev_was_underscore = false;
    for c in title.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            if c == '_' {
                if !prev_was_underscore {
                    result.push('_');
                }
                prev_was_underscore = true;
            } else {
                result.push(c);
                prev_was_underscore = false;
            }
        } else {
            if !prev_was_underscore {
                result.push('_');
            }
            prev_was_underscore = true;
        }
    }
    result.chars().take(80).collect::<String>().trim_matches('_').to_string()
}

/// Extract search tags from a video title (mirrors the Python `extract_tags`
/// function). Removes bracketed/parenthesised content, common suffixes, and
/// short stopwords; appends the source name as a tag.
fn extract_tags(title: &str, source_name: &str) -> Vec<String> {
    let mut cleaned = title.to_string();
    // Remove [bracketed] and (parenthesised) content
    cleaned = strip_between(&cleaned, '[', ']');
    cleaned = strip_between(&cleaned, '(', ')');
    // Remove common suffixes (case-insensitive)
    for suffix in &["Official", "Lyric Video", "Music Video"] {
        let lower_suffix = suffix.to_lowercase();
        if let Some(idx) = cleaned.to_lowercase().find(&lower_suffix) {
            cleaned.truncate(idx);
        }
    }

    let stopwords = [
        "the", "and", "for", "with", "your", "you", "are", "but", "not", "all",
        "can", "has", "this", "that",
    ];
    let mut tags: Vec<String> = cleaned
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
        .filter(|w| w.len() > 2 && !stopwords.contains(&w.as_str()))
        .collect();

    if !source_name.is_empty() {
        tags.push(source_name.to_lowercase());
    }
    tags
}

/// Remove all content between `open` and `close` chars (inclusive). Simple
/// non-nested version — sufficient for video titles.
fn strip_between(s: &str, open: char, close: char) -> String {
    let mut result = String::with_capacity(s.len());
    let mut depth = 0;
    for c in s.chars() {
        if c == open {
            depth += 1;
        } else if c == close && depth > 0 {
            depth -= 1;
        } else if depth == 0 {
            result.push(c);
        }
    }
    result
}

/// Scrape video titles and IDs from a YouTube channel using `yt-dlp
/// --flat-playlist --dump-json`. Mirrors the Python `scrape_youtube_channel`
/// function. Returns a vec of JSON entry objects.
async fn scrape_youtube_channel(
    channel_url: &str,
    source_name: &str,
    media_type: &str,
    max_videos: u32,
) -> Vec<Value> {
    tracing::info!("[library_indexer] Scraping {} ({})", source_name, channel_url);

    let result = Command::new("yt-dlp")
        .arg("--flat-playlist")
        .arg("--dump-json")
        .arg("--no-warnings")
        .arg("--playlist-end")
        .arg(max_videos.to_string())
        .arg(channel_url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await;

    let output = match result {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            tracing::warn!("[library_indexer] yt-dlp failed for {}: {}", source_name, stderr.trim());
            return Vec::new();
        }
        Err(e) => {
            tracing::warn!("[library_indexer] Failed to spawn yt-dlp for {}: {}", source_name, e);
            return Vec::new();
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let skip_words = ["interview", "announcement", "q&a", "faq", "subscribe", "follow me"];
    let mut entries = Vec::new();

    for line in stdout.lines() {
        if line.is_empty() {
            continue;
        }
        let entry: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let title = entry.get("title").and_then(|v| v.as_str()).unwrap_or("");
        let video_id = entry.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let duration = entry.get("duration").and_then(|v| v.as_f64()).unwrap_or(0.0);

        if title.is_empty() || video_id.is_empty() {
            continue;
        }

        // Skip non-music content
        let lower_title = title.to_lowercase();
        if skip_words.iter().any(|w| lower_title.contains(w)) {
            continue;
        }

        let filename = sanitize_filename(title);
        let tags = extract_tags(title, source_name);
        let license = if lower_title.contains("creative commons") {
            "youtube-creative-commons"
        } else {
            "no-copyright"
        };

        entries.push(json!({
            "filename": format!("{}.mp3", filename),
            "title": title,
            "tags": tags,
            "download_url": format!("https://www.youtube.com/watch?v={}", video_id),
            "video_id": video_id,
            "source": source_name,
            "source_type": "youtube",
            "media_type": media_type,
            "duration_s": duration,
            "license": license,
        }));
    }

    tracing::info!("[library_indexer]   Found {} entries from {}", entries.len(), source_name);
    entries
}

/// Build the complete music/SFX library index and write it to
/// `mcp/assets/music_library_index.json`. This is the Rust equivalent of the
/// Python `build_index()` function.
///
/// Returns the index as a `Value` (also written to disk).
pub async fn build_index(output_path: &str) -> Result<Value, String> {
    let mut all_entries: Vec<Value> = Vec::new();
    let mut seen_titles: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Scrape YouTube music channels
    for (name, url) in MUSIC_SOURCES {
        let entries = scrape_youtube_channel(url, name, "music", 50).await;
        for entry in entries {
            let title_key = entry
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_lowercase()
                .trim()
                .to_string();
            if !title_key.is_empty() && seen_titles.insert(title_key) {
                all_entries.push(entry);
            }
        }
    }

    // Scrape YouTube SFX channels
    for (name, url) in SFX_SOURCES {
        let entries = scrape_youtube_channel(url, name, "sfx", 50).await;
        for entry in entries {
            let title_key = entry
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_lowercase()
                .trim()
                .to_string();
            if !title_key.is_empty() && seen_titles.insert(title_key) {
                all_entries.push(entry);
            }
        }
    }

    // Add local stock music to the index
    let local_music_dir = Path::new("mcp/assets/music");
    if local_music_dir.exists() {
        if let Ok(rd) = std::fs::read_dir(local_music_dir) {
            for entry in rd.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("mp3") {
                    let stem = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .replace('_', " ");
                    let tags = extract_tags(&stem, "OpenScript Stock");
                    all_entries.push(json!({
                        "filename": path.file_name().and_then(|s| s.to_str()).unwrap_or("unknown.mp3"),
                        "title": stem,
                        "tags": tags,
                        "download_url": path.to_string_lossy(),
                        "video_id": "",
                        "source": "OpenScript Stock",
                        "source_type": "local",
                        "media_type": "music",
                        "duration_s": 30,
                        "license": "openscript-stock",
                    }));
                }
            }
        }
    }

    let music_count = all_entries
        .iter()
        .filter(|e| e.get("media_type").and_then(|v| v.as_str()) == Some("music"))
        .count();
    let sfx_count = all_entries
        .iter()
        .filter(|e| e.get("media_type").and_then(|v| v.as_str()) == Some("sfx"))
        .count();

    let sources: Vec<String> = MUSIC_SOURCES
        .iter()
        .chain(SFX_SOURCES.iter())
        .map(|(name, _)| name.to_string())
        .collect();

    let index = json!({
        "total_entries": all_entries.len(),
        "music_count": music_count,
        "sfx_count": sfx_count,
        "sources": sources,
        "entries": all_entries,
    });

    // Write to disk
    if let Some(parent) = Path::new(output_path).parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create index dir: {}", e))?;
    }
    let json_str = serde_json::to_string_pretty(&index)
        .map_err(|e| format!("Failed to serialise index: {}", e))?;
    std::fs::write(output_path, json_str)
        .map_err(|e| format!("Failed to write index: {}", e))?;

    tracing::info!(
        "[library_indexer] Index built: {} entries ({} music, {} SFX) → {}",
        all_entries.len(),
        music_count,
        sfx_count,
        output_path
    );

    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_filename_basic() {
        assert_eq!(sanitize_filename("Hello World!"), "Hello_World");
        assert_eq!(sanitize_filename("Song (Official Video)"), "Song_Official_Video");
        assert_eq!(sanitize_filename("Track-01.mp3"), "Track-01_mp3");
    }

    #[test]
    fn test_sanitize_filename_truncates() {
        let long = "a".repeat(100);
        let result = sanitize_filename(&long);
        assert!(result.len() <= 80);
    }

    #[test]
    fn test_extract_tags_basic() {
        let tags = extract_tags("Epic Battle [Official Music Video]", "NoCopyrightSounds");
        assert!(tags.contains(&"epic".to_string()));
        assert!(tags.contains(&"battle".to_string()));
        assert!(tags.contains(&"nocopyrightsounds".to_string()));
        // "Official" should be stripped, "Music Video" should be stripped
        assert!(!tags.iter().any(|t| t == "official"));
    }

    #[test]
    fn test_extract_tags_filters_stopwords() {
        let tags = extract_tags("The Best Song For You", "");
        assert!(!tags.iter().any(|t| t == "the"));
        assert!(!tags.iter().any(|t| t == "for"));
        assert!(!tags.iter().any(|t| t == "you"));
        assert!(tags.contains(&"best".to_string()));
        assert!(tags.contains(&"song".to_string()));
    }

    #[test]
    fn test_strip_between() {
        assert_eq!(strip_between("Hello [World]!", '[', ']'), "Hello !");
        assert_eq!(strip_between("Hello (World)!", '(', ')'), "Hello !");
        assert_eq!(strip_between("No brackets here", '[', ']'), "No brackets here");
        // Nested brackets: the nested implementation strips from the outer '['
        // to the outer ']', removing all inner content.
        assert_eq!(strip_between("Nested [a [b] c]", '[', ']'), "Nested ");
    }
}
