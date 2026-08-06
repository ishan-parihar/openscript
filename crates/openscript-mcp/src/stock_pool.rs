//! Unified stock candidate pool — normalize **Pexels / Pixabay / YouTube**
//! results into ONE `StockCandidate` model, dedup across engines, and rank
//! with the shared `stock_signal` lexical gate.
//!
//! Prior to this module each engine had its own response shape
//! (`PexelsVideo` vs yt-dlp JSON vs Pixabay hits) so ranking, dedup and
//! duration-coverage logic could not be shared. `search_stock_pool` fixes
//! that: every candidate carries `{ provider, id, title, duration_s, width,
//! height, thumbnail, page_url, direct_url }` and a `lexical` relevance
//! score computed by `stock_signal::lexical_relevance` against the same
//! signal tokens used everywhere else in the pipeline.
//!
//! Provider priority for dedup ties: **Pexels → Pixabay → YouTube** (insertion
//! order — first-seen wins on cross-provider title collisions).

use serde_json::Value;
use std::collections::HashSet;
use std::process::Stdio;

/// One normalized stock candidate, regardless of which engine produced it.
#[derive(Debug, Clone)]
pub struct StockCandidate {
    /// "pexels" | "pixabay" | "youtube"
    pub provider: &'static str,
    /// Engine-specific video id.
    pub id: String,
    /// Human-readable title (Pexels: derived from URL slug).
    pub title: String,
    pub duration_s: f64,
    pub width: u32,
    pub height: u32,
    pub thumbnail_url: String,
    /// Human page URL (Pexels/Pixabay video page or YouTube watch URL).
    pub page_url: String,
    /// Direct downloadable file URL, when known (Pexels file / Pixabay video).
    pub direct_url: Option<String>,
    /// `stock_signal::lexical_relevance` score (filled by ranking).
    pub lexical: f64,
}

/// Parameters for a pool search.
pub struct StockPoolQuery {
    pub query: String,
    /// "9:16" | "16:9" | "1:1"
    pub aspect: String,
    /// 0 = no floor; clips shorter than this are filtered out.
    pub min_duration_s: f64,
    /// 0 = no cap.
    pub max_duration_s: f64,
    /// Max candidates to keep per provider before dedup/rank.
    pub per_provider: usize,
    /// Lexical bias tokens; empty derives from the query.
    pub signal: Vec<String>,
}

/// Result of a pool search: ranked candidates + per-provider counts.
pub struct StockPoolOutcome {
    pub candidates: Vec<StockCandidate>,
    pub per_provider: Vec<(String, usize)>,
}

// ---------------------------------------------------------------------------
// Normalization — one pure function per provider (unit-testable, no I/O)
// ---------------------------------------------------------------------------

/// Pexels video JSON → candidate. Pexels has no title field, so we derive one
/// from the page URL slug (`/video/people-walking-on-a-street-12345/` →
/// "people walking on a street") which makes lexical ranking meaningful.
pub fn pexels_video_to_candidate(v: &Value) -> Option<StockCandidate> {
    let id = v.get("id")?.as_i64()?;
    let page_url = v.get("url")?.as_str()?.to_string();
    let title = slug_to_title(&page_url);
    let duration_s = v.get("duration").and_then(|x| x.as_f64()).unwrap_or(0.0);
    let width = v.get("width").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
    let height = v.get("height").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
    let thumbnail_url = v.get("image").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let direct_url = crate::tools::pexels_file_url(v);
    Some(StockCandidate {
        provider: "pexels",
        id: id.to_string(),
        title,
        duration_s,
        width,
        height,
        thumbnail_url,
        page_url,
        direct_url,
        lexical: 0.0,
    })
}

/// Pixabay video API hit → candidate. Tags act as the title; the `medium`
/// video file (or `large` fallback) is the direct URL.
pub fn pixabay_hit_to_candidate(h: &Value) -> Option<StockCandidate> {
    let id = h.get("id")?.as_u64()?;
    let title = h.get("tags").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let duration_s = h.get("duration").and_then(|x| x.as_f64()).unwrap_or(0.0);
    let thumbnail_url = h.get("previewURL").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let videos = h.get("videos")?;
    let file = videos
        .get("medium")
        .or_else(|| videos.get("large"))
        .or_else(|| videos.get("small"))?;
    let direct_url = file.get("url").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let (width, height) = (
        file.get("width").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
        file.get("height").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
    );
    Some(StockCandidate {
        provider: "pixabay",
        id: id.to_string(),
        title,
        duration_s,
        width,
        height,
        thumbnail_url,
        page_url: format!("https://pixabay.com/videos/{}", id),
        direct_url: Some(direct_url),
        lexical: 0.0,
    })
}

/// yt-dlp `--flat-playlist --dump-json` entry → candidate.
pub fn youtube_entry_to_candidate(e: &Value) -> Option<StockCandidate> {
    let id = e.get("id").and_then(|x| x.as_str())?;
    if id.is_empty() {
        return None;
    }
    let title = e.get("title").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let duration_s = e.get("duration").and_then(|x| x.as_f64()).unwrap_or(0.0);
    let thumbnail_url = e
        .get("thumbnail")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", id));
    Some(StockCandidate {
        provider: "youtube",
        id: id.to_string(),
        title,
        duration_s,
        width: 0,
        height: 0,
        thumbnail_url,
        page_url: format!("https://www.youtube.com/watch?v={}", id),
        direct_url: None,
        lexical: 0.0,
    })
}

/// Derive a readable title from a Pexels page URL slug.
/// Strips the trailing video ID (`people-walking-on-a-street-12345` →
/// "people walking on a street") so the ID never pollutes lexical scoring.
fn slug_to_title(page_url: &str) -> String {
    let slug = page_url.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    let parts: Vec<&str> = slug
        .split('-')
        .filter(|part| !part.is_empty() && part.chars().all(|c| c.is_alphanumeric()))
        .collect();
    // Drop trailing all-digit tokens (the Pexels video id) from the end.
    let mut end = parts.len();
    while end > 0 && parts[end - 1].chars().all(|c| c.is_ascii_digit()) {
        end -= 1;
    }
    parts[..end].join(" ")
}

// ---------------------------------------------------------------------------
// Cross-engine dedup + shared stock_signal ranking (pure, unit-testable)
// ---------------------------------------------------------------------------

/// Lowercase alphanumeric token join — the cross-provider title key.
/// "Free City Street Footage (No Copyright)" and "free city street footage no
/// copyright" collide; distinct clips with different titles do not.
fn title_key(title: &str) -> String {
    title
        .to_ascii_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Dedup (provider:id, then cross-provider title) + denylist + lexical rank.
///
/// Order is preserved by provider priority: callers insert Pexels first, so
/// on a title collision across engines the first-seen (higher priority)
/// candidate wins. Candidates whose title is below `min_lex` (or is a
/// denylisted audio bed) are dropped — same gate as `rank_and_filter_candidates`.
pub fn dedup_and_rank(
    mut pool: Vec<StockCandidate>,
    signal: &[String],
    min_lex: f64,
) -> Vec<StockCandidate> {
    // 1. Provider:id dedup.
    let mut seen_ids: HashSet<(String, String)> = HashSet::new();
    pool.retain(|c| seen_ids.insert((c.provider.to_string(), c.id.clone())));

    // 2. Cross-provider title dedup (first-seen wins = provider priority).
    let mut seen_titles: HashSet<String> = HashSet::new();
    pool.retain(|c| {
        let key = title_key(&c.title);
        if key.is_empty() {
            true // no title → keep (YouTube sometimes lacks titles on flat search)
        } else {
            seen_titles.insert(key)
        }
    });

    // 3. Denylist audio beds (10-hour lofi, NCS playlists…) — never B-roll.
    pool.retain(|c| !crate::stock_signal::is_broll_title_denylisted(&c.title));

    // 4. Lexical score + threshold.
    for c in &mut pool {
        c.lexical = crate::stock_signal::lexical_relevance(&c.title, signal);
    }
    pool.sort_by(|a, b| b.lexical.total_cmp(&a.lexical));
    pool.into_iter().filter(|c| c.lexical >= min_lex).collect()
}

// ---------------------------------------------------------------------------
// Pool search (network)
// ---------------------------------------------------------------------------

/// True when a candidate satisfies the query's min/max duration bounds.
/// Duration `0.0` means *unknown* (e.g. flat-playlist YouTube entries without
/// a duration field) — unknown passes the gate rather than being dropped as
/// "too short".
fn passes_duration_gate(c: &StockCandidate, q: &StockPoolQuery) -> bool {
    if c.duration_s <= 0.0 {
        return true;
    }
    if q.min_duration_s > 0.0 && c.duration_s < q.min_duration_s {
        return false;
    }
    if q.max_duration_s > 0.0 && c.duration_s > q.max_duration_s {
        return false;
    }
    true
}

/// Search every enabled engine, normalize into `StockCandidate`, dedup across
/// engines, and rank with the shared `stock_signal` gate.
pub async fn search_stock_pool(q: &StockPoolQuery) -> StockPoolOutcome {
    let mut pool: Vec<StockCandidate> = Vec::new();

    // One shared client (explicit UA — Pexels blocks default clients, see
    // pexels.rs; Cloudflare 1010 edge).
    let client = reqwest::Client::builder()
        .user_agent("OpenScript/1.0 (+https://github.com/ishan-parihar/openscript)")
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    // 1. Pexels — primary provider (needs key).
    if !crate::tools::pexels_key().is_empty() {
        let orientation = crate::tools::aspect_to_orientation(&q.aspect);
        'pexels: for page in 1..=3i64 {
            let url = crate::tools::pexels_search_url(
                &q.query,
                orientation,
                page,
                q.min_duration_s,
                q.max_duration_s,
            );
            let Ok(resp) = client
                .get(&url)
                .header("Authorization", crate::tools::pexels_key())
                .send()
                .await
            else {
                continue;
            };
            if !resp.status().is_success() {
                continue;
            }
            let Ok(body) = resp.json::<Value>().await else { continue };
            let Some(videos) = body.get("videos").and_then(|v| v.as_array()) else {
                continue;
            };
            for v in videos {
                if let Some(c) = pexels_video_to_candidate(v) {
                    if passes_duration_gate(&c, q) {
                        pool.push(c);
                    }
                }
            }
            if pool.iter().filter(|c| c.provider == "pexels").count() >= q.per_provider {
                break 'pexels;
            }
        }
    }

    // 2. Pixabay — film footage only (NOT `video_type=animation`: the audit
    // flagged that stock.fetch's animation pin returns motion graphics, which
    // are useless as B-roll). Needs PIXABAY_API_KEY.
    if !crate::tools::pixabay_key().is_empty() {
        let url = format!(
            "https://pixabay.com/api/videos/?key={}&q={}&per_page={}&video_type=film",
            crate::tools::pixabay_key(),
            urlencoding::encode(&q.query),
            q.per_provider
        );
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().is_success() {
                if let Ok(body) = resp.json::<Value>().await {
                    if let Some(hits) = body.get("hits").and_then(|v| v.as_array()) {
                        for h in hits {
                            if let Some(c) = pixabay_hit_to_candidate(h) {
                                if passes_duration_gate(&c, q) {
                                    pool.push(c);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // 3. YouTube — no key needed; yt-dlp flat search with durations.
    // Same duration gate as the other engines (0.0 = unknown passes).
    let yt_entries = yt_search_entries(&q.query, q.per_provider * 2).await;
    for e in yt_entries {
        if let Some(c) = youtube_entry_to_candidate(&e) {
            if passes_duration_gate(&c, q) {
                pool.push(c);
            }
        }
    }

    // Shared ranking gate.
    let signal = if q.signal.is_empty() {
        crate::stock_signal::signal_tokens_from_scene(&q.query, &[])
    } else {
        q.signal.clone()
    };
    let min_lex = crate::stock_signal::min_lexical_accept();
    let candidates = dedup_and_rank(pool, &signal, min_lex);

    let mut counts: Vec<(String, usize)> = Vec::new();
    for p in ["pexels", "pixabay", "youtube"] {
        let n = candidates.iter().filter(|c| c.provider == p).count();
        counts.push((p.to_string(), n));
    }
    StockPoolOutcome {
        candidates,
        per_provider: counts,
    }
}

/// yt-dlp flat search returning `(id, title, duration_s)` entries.
async fn yt_search_entries(query: &str, limit: usize) -> Vec<Value> {
    let out = tokio::process::Command::new("yt-dlp")
        .args([
            "--flat-playlist",
            "--dump-json",
            "--no-warnings",
            "--quiet",
            &format!("ytsearch{}:{}", limit, query),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await;
    let Ok(output) = out else { return Vec::new() };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cand(provider: &'static str, id: &str, title: &str, dur: f64) -> StockCandidate {
        StockCandidate {
            provider,
            id: id.to_string(),
            title: title.to_string(),
            duration_s: dur,
            width: 1080,
            height: 1920,
            thumbnail_url: String::new(),
            page_url: String::new(),
            direct_url: None,
            lexical: 0.0,
        }
    }

    #[test]
    fn pexels_slug_becomes_title() {
        let v = json!({
            "id": 12345,
            "width": 2160,
            "height": 3840,
            "url": "https://www.pexels.com/video/people-walking-on-a-street-12345/",
            "image": "https://images.pexels.com/videos/12345/thumb.jpg",
            "duration": 12,
            "video_files": [
                {"id": 1, "width": 720, "height": 1280, "link": "https://videos.pexels.com/video-files/12345/720.mp4", "size": 1000}
            ]
        });
        let c = pexels_video_to_candidate(&v).expect("candidate");
        assert_eq!(c.provider, "pexels");
        assert_eq!(c.id, "12345");
        assert_eq!(c.title, "people walking on a street");
        assert_eq!(c.duration_s, 12.0);
        assert!(c.direct_url.as_deref().unwrap().contains("720.mp4"));
    }

    #[test]
    fn pixabay_hit_uses_tags_and_medium_file() {
        let h = json!({
            "id": 99,
            "tags": "city traffic night",
            "duration": 8,
            "previewURL": "https://cdn.pixabay.com/thumb.jpg",
            "videos": {
                "medium": {"url": "https://cdn.pixabay.com/video/medium.mp4", "width": 1280, "height": 720},
                "small": {"url": "https://cdn.pixabay.com/video/small.mp4", "width": 640, "height": 360}
            }
        });
        let c = pixabay_hit_to_candidate(&h).expect("candidate");
        assert_eq!(c.provider, "pixabay");
        assert_eq!(c.title, "city traffic night");
        assert!(c.direct_url.as_deref().unwrap().contains("medium.mp4"));
        assert_eq!(c.width, 1280);
    }

    #[test]
    fn youtube_entry_maps_fields() {
        let e = json!({
            "id": "abc123XYZ",
            "title": "Free Stock Footage - People Walking",
            "duration": 117.0,
            "thumbnail": "https://i.ytimg.com/vi/abc123XYZ/hqdefault.jpg"
        });
        let c = youtube_entry_to_candidate(&e).expect("candidate");
        assert_eq!(c.provider, "youtube");
        assert_eq!(c.duration_s, 117.0);
        assert!(c.page_url.contains("abc123XYZ"));
    }

    #[test]
    fn cross_provider_title_dedup_keeps_first() {
        let pool = vec![
            cand("pexels", "1", "Free City Street Footage (No Copyright)", 9.0),
            cand("youtube", "yt1", "free city street footage no copyright", 117.0),
        ];
        let ranked = dedup_and_rank(pool, &["city".into(), "street".into()], 0.0);
        assert_eq!(ranked.len(), 1, "cross-provider dup must collapse to one");
        assert_eq!(ranked[0].provider, "pexels", "Pexels wins the tie (insertion order)");
    }

    #[test]
    fn same_provider_same_id_dedups() {
        let pool = vec![
            cand("pexels", "7", "courtroom gavel", 5.0),
            cand("pexels", "7", "courtroom gavel", 5.0),
        ];
        let ranked = dedup_and_rank(pool, &["court".into()], 0.0);
        assert_eq!(ranked.len(), 1);
    }

    #[test]
    fn denylist_drops_music_beds() {
        let pool = vec![
            cand("youtube", "a", "10 Hours Lofi Focus Music for Study", 36000.0),
            cand("youtube", "b", "Cinematic city drone stock footage vertical", 30.0),
        ];
        let ranked = dedup_and_rank(pool, &["city".into(), "drone".into()], 0.0);
        assert!(ranked.iter().all(|c| c.id != "a"));
        assert!(ranked.iter().any(|c| c.id == "b"));
    }

    #[test]
    fn ranking_prefers_relevant_titles() {
        let pool = vec![
            cand("youtube", "a", "Minecraft parkour no copyright gameplay", 500.0),
            cand("pexels", "b", "morning coffee desk phone stock", 12.0),
        ];
        let signal = vec!["morning".into(), "coffee".into(), "phone".into()];
        let ranked = dedup_and_rank(pool, &signal, 0.0);
        assert_eq!(ranked[0].id, "b", "coffee/phone candidate must rank first");
    }

    #[test]
    fn duration_gate_applies_to_all_providers_unknown_passes() {
        let q = StockPoolQuery {
            query: "test".into(),
            aspect: "9:16".into(),
            min_duration_s: 8.0,
            max_duration_s: 0.0,
            per_provider: 5,
            signal: vec![],
        };
        let long = cand("youtube", "a", "long clip", 80.0);
        let short = cand("youtube", "b", "short clip", 3.0);
        let unknown = cand("youtube", "c", "unknown duration", 0.0);
        assert!(passes_duration_gate(&long, &q));
        assert!(!passes_duration_gate(&short, &q), "3s clip must be filtered by min 8s");
        assert!(passes_duration_gate(&unknown, &q), "unknown 0.0 duration must pass, not drop");
    }

    #[test]
    fn threshold_drops_irrelevant() {
        let pool = vec![cand("youtube", "x", "Funny cat compilation", 60.0)];
        let ranked = dedup_and_rank(pool, &["courtroom".into(), "justice".into()], 0.2);
        assert!(ranked.is_empty(), "no lexical overlap → must be dropped at 0.2 gate");
    }
}
