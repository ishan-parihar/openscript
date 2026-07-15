//! Production Quality Model — architecture-level KPIs for AI-directed shorts.
//!
//! `verify.render` is **technical integrity** only. This module scores whether the
//! timeline/render actually uses the editor like a director:
//! video source quality, cut pacing, music variance, sticker design principles,
//! section-level title/text/meme placement, and track utilization.
//!
//! Weights sum to 100. Grade bands:
//!   A 85–100 · B 70–84 · C 55–69 · D 40–54 · F <40
//!
//! v2.1 adds **visual_repetition** (content-hash uniqueness) and
//! **context_relevance** (search query vs scene text / video keywords).

use crate::timeline::{EventKind, Timeline};
use crate::types::TrackType;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Provenance of a visual clip used as background / meme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoSourceClass {
    Pexels,
    Youtube,
    Giphy,
    LocalStock,
    ProceduralSynthetic,
    Unknown,
}

impl VideoSourceClass {
    /// Quality tier 0.0–1.0 used for weighted scoring.
    pub fn quality_tier(self) -> f64 {
        match self {
            VideoSourceClass::Pexels => 1.0,
            VideoSourceClass::Giphy => 0.9,
            VideoSourceClass::Youtube => 0.85,
            VideoSourceClass::LocalStock => 0.8,
            VideoSourceClass::Unknown => 0.4,
            VideoSourceClass::ProceduralSynthetic => 0.0,
        }
    }
}

/// Role of a narrative section in a short-form arc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionRole {
    Hook,
    Body,
    Payoff,
    Cta,
}

/// One composited sticker/GIF overlay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StickerLayerInfo {
    pub path: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub position: String,
    pub scale: f64,
}

/// One full-screen meme / reaction cut.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemeLayerInfo {
    pub path: String,
    pub start_ms: i64,
    pub end_ms: i64,
}

/// One background clip on the visual bed.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BackgroundLayerInfo {
    pub path: String,
    pub start_ms: i64,
    pub end_ms: i64,
    #[serde(default)]
    pub source_hint: Option<String>,
    /// Content fingerprint (hash+size) for true visual uniqueness — paths alone
    /// can differ while bytes are identical (same YT video downloaded twice).
    #[serde(default)]
    pub content_hash: Option<String>,
    /// Provider video id (YouTube id / pexels_N) when known.
    #[serde(default)]
    pub video_id: Option<String>,
    /// Search query used to fetch this clip (for context-relevance scoring).
    #[serde(default)]
    pub search_query: Option<String>,
}

/// Music bed metadata for variance scoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MusicLayerInfo {
    pub path: String,
    #[serde(default)]
    pub gain_db: f64,
    #[serde(default)]
    pub ducking: bool,
    #[serde(default)]
    pub mood: Option<String>,
    #[serde(default)]
    pub energy: Option<String>,
}

/// Narrative section (scene beat) for section-composition scoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectionInfo {
    pub role: SectionRole,
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    /// Explicit on-screen title/card text if the director provided one.
    #[serde(default)]
    pub title_text: Option<String>,
}

/// Full render-side truth for quality scoring.
/// Timeline alone is insufficient: multi-broll/stickers/memes may live only here.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RenderManifest {
    pub duration_ms: i64,
    #[serde(default)]
    pub backgrounds: Vec<BackgroundLayerInfo>,
    #[serde(default)]
    pub stickers: Vec<StickerLayerInfo>,
    #[serde(default)]
    pub memes: Vec<MemeLayerInfo>,
    #[serde(default)]
    pub music: Option<MusicLayerInfo>,
    #[serde(default)]
    pub captions_path: Option<String>,
    #[serde(default)]
    pub voiceover_count: usize,
    #[serde(default)]
    pub sections: Vec<SectionInfo>,
    #[serde(default)]
    pub has_dialogue: bool,
    #[serde(default)]
    pub rms_ok: bool,
    /// Topic keywords for the whole video (context relevance).
    #[serde(default)]
    pub video_keywords: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DimensionScore {
    pub id: String,
    pub label: String,
    pub score: i32,
    pub max: i32,
    pub detail: serde_json::Value,
    #[serde(default)]
    pub findings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProductionQualityReport {
    pub production_score: i32,
    pub grade: String,
    pub dimensions: Vec<DimensionScore>,
    pub hard_fails: Vec<String>,
    pub next_actions: Vec<String>,
    pub timeline_editor: TimelineEditorReport,
    pub cuts_per_second: f64,
    pub video_source_mix: serde_json::Value,
    pub kpi_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineEditorReport {
    /// 0–100: how completely the timeline/editor model is utilized.
    pub utilization_score: i32,
    pub tracks_with_events: Vec<String>,
    pub empty_tracks: Vec<String>,
    pub event_counts: serde_json::Value,
    pub unique_visual_assets: usize,
    pub background_gaps_ms: i64,
    pub background_overlaps_ms: i64,
    pub findings: Vec<String>,
}

// ---------------------------------------------------------------------------
// Classification helpers
// ---------------------------------------------------------------------------

pub fn classify_video_source(path: &str, source_hint: Option<&str>) -> VideoSourceClass {
    let p = path.to_lowercase();
    let hint = source_hint.unwrap_or("").to_lowercase();
    if p.contains("procedural") || p.contains("_procedural") {
        return VideoSourceClass::ProceduralSynthetic;
    }
    if hint.contains("pexels") || p.contains("pexels") {
        return VideoSourceClass::Pexels;
    }
    if hint.contains("giphy") || p.contains("giphy") || p.contains("meme_cache") {
        return VideoSourceClass::Giphy;
    }
    if hint.contains("youtube")
        || p.contains("yt_")
        || p.contains("_yt.mp4")
        || p.contains("youtube")
        || p.contains("background_cache")
    {
        return VideoSourceClass::Youtube;
    }
    if p.contains("stock") || p.contains("library") || p.contains("pixabay") {
        return VideoSourceClass::LocalStock;
    }
    if !hint.is_empty() {
        return VideoSourceClass::LocalStock;
    }
    VideoSourceClass::Unknown
}

pub fn is_synthetic_music_file(path: &str) -> bool {
    if !path.contains("mcp/assets/music/") && !path.contains("mcp\\assets\\music\\") {
        return false;
    }
    std::fs::metadata(path)
        .map(|m| m.len() == 481_114)
        .unwrap_or(false)
}

pub fn production_grade(score: i32) -> &'static str {
    match score {
        85..=100 => "A",
        70..=84 => "B",
        55..=69 => "C",
        40..=54 => "D",
        _ => "F",
    }
}

pub fn grade_rank(g: &str) -> i32 {
    match g {
        "A" => 5,
        "B" => 4,
        "C" => 3,
        "D" => 2,
        _ => 1,
    }
}

// ---------------------------------------------------------------------------
// Dimension scorers (weights documented inline)
// ---------------------------------------------------------------------------

/// Weight 14 — video source quality mix (Pexels > YT > local > unknown > procedural).
fn score_video_source(bgs: &[BackgroundLayerInfo]) -> DimensionScore {
    let n = bgs.len().max(1);
    let mut tier_sum = 0.0;
    let mut findings = Vec::new();
    let mut mix_counts: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    for b in bgs {
        let cls = classify_video_source(&b.path, b.source_hint.as_deref());
        tier_sum += cls.quality_tier();
        if matches!(cls, VideoSourceClass::ProceduralSynthetic) {
            findings.push(format!("procedural synthetic: {}", b.path));
        }
        let key = match cls {
            VideoSourceClass::Pexels => "pexels",
            VideoSourceClass::Youtube => "youtube",
            VideoSourceClass::Giphy => "giphy",
            VideoSourceClass::LocalStock => "local_stock",
            VideoSourceClass::ProceduralSynthetic => "procedural_synthetic",
            VideoSourceClass::Unknown => "unknown",
        };
        let c = mix_counts.get(key).and_then(|v| v.as_u64()).unwrap_or(0) + 1;
        mix_counts.insert(key.to_string(), serde_json::json!(c));
    }
    let avg = tier_sum / n as f64;
    let score = (avg * 14.0).round() as i32;
    if score == 0 {
        findings.push("all backgrounds are synthetic procedural — not production stock".into());
    }
    DimensionScore {
        id: "video_source_quality".into(),
        label: "Video source quality".into(),
        score,
        max: 14,
        detail: serde_json::json!({
            "clip_count": bgs.len(),
            "average_tier": (avg * 1000.0).round() / 1000.0,
            "source_mix": mix_counts,
        }),
        findings,
    }
}

/// Content identity for a background: prefers hash, then video_id, then path.
fn visual_identity(b: &BackgroundLayerInfo) -> String {
    if let Some(ref h) = b.content_hash {
        if !h.is_empty() {
            return format!("hash:{}", h);
        }
    }
    if let Some(ref id) = b.video_id {
        if !id.is_empty() {
            return format!("id:{}", id);
        }
    }
    format!("path:{}", b.path)
}

/// Weight 16 — **visual repetitiveness** (content-hash / video-id uniqueness).
/// Path-only uniqueness is insufficient: same YT video can be saved to different paths.
fn score_visual_repetition(bgs: &[BackgroundLayerInfo]) -> DimensionScore {
    let mut findings = Vec::new();
    let n = bgs.len();
    if n == 0 {
        return DimensionScore {
            id: "visual_repetition".into(),
            label: "Visual variance / anti-repeat".into(),
            score: 0,
            max: 16,
            detail: serde_json::json!({}),
            findings: vec!["no backgrounds".into()],
        };
    }

    let identities: Vec<String> = bgs.iter().map(visual_identity).collect();
    let unique: HashSet<_> = identities.iter().cloned().collect();
    let uniqueness = unique.len() as f64 / n as f64;

    // Max consecutive run of the same identity (e.g. night-phone for 4 scenes)
    let mut max_run = 1usize;
    let mut run = 1usize;
    for w in identities.windows(2) {
        if w[0] == w[1] {
            run += 1;
            max_run = max_run.max(run);
        } else {
            run = 1;
        }
    }

    // Frequency of most-common identity
    let mut freq: HashMap<&str, usize> = HashMap::new();
    for id in &identities {
        *freq.entry(id.as_str()).or_insert(0) += 1;
    }
    let max_freq = freq.values().copied().max().unwrap_or(1);
    let dominant_share = max_freq as f64 / n as f64;

    let mut score = (uniqueness * 16.0).round() as i32;
    if max_run >= 3 && n >= 3 {
        findings.push(format!(
            "REPETITION: same visual identity runs for {} consecutive cuts — looks like one clip looping",
            max_run
        ));
        score = (score - 8).max(0);
    } else if max_run >= 2 && n >= 4 {
        findings.push(format!(
            "back-to-back repeat of same visual for {} cuts",
            max_run
        ));
        score = (score - 3).max(0);
    }
    if dominant_share > 0.5 && n >= 3 {
        findings.push(format!(
            "REPETITION: one source used in {:.0}% of scenes — lack of context-relevant variance",
            dominant_share * 100.0
        ));
        score = (score - 4).max(0);
    }
    if uniqueness < 0.5 && n >= 3 {
        findings.push(format!(
            "unique visual identities only {:.0}% (want ≥80% for multi-scene shorts)",
            uniqueness * 100.0
        ));
    }
    if unique.len() == 1 && n > 1 {
        score = 0;
        findings.push(
            "HARD: every cut is the same source video (different paths do not count as variance)"
                .into(),
        );
    }

    DimensionScore {
        id: "visual_repetition".into(),
        label: "Visual variance / anti-repeat".into(),
        score: score.min(16),
        max: 16,
        detail: serde_json::json!({
            "unique_identities": unique.len(),
            "total_clips": n,
            "uniqueness_ratio": (uniqueness * 1000.0).round() / 1000.0,
            "max_consecutive_same": max_run,
            "dominant_share": (dominant_share * 1000.0).round() / 1000.0,
            "identities": identities,
        }),
        findings,
    }
}

/// Weight 12 — context relevance: search query / keywords vs section text.
fn score_context_relevance(
    bgs: &[BackgroundLayerInfo],
    sections: &[SectionInfo],
    video_keywords: &[String],
) -> DimensionScore {
    let mut findings = Vec::new();
    if bgs.is_empty() {
        return DimensionScore {
            id: "context_relevance".into(),
            label: "Context-relevant visual variance".into(),
            score: 0,
            max: 12,
            detail: serde_json::json!({}),
            findings: vec!["no backgrounds to score".into()],
        };
    }

    fn tokens(s: &str) -> HashSet<String> {
        let stop: HashSet<&str> = [
            "the", "a", "an", "and", "or", "to", "of", "in", "on", "for", "with", "is", "are",
            "be", "this", "that", "your", "you", "from", "stock", "footage", "cinematic",
            "vertical", "broll", "b-roll", "calm", "unique", "scene",
        ]
        .into_iter()
        .collect();
        s.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| t.len() > 2 && !stop.contains(t))
            .map(|t| t.to_string())
            .collect()
    }
    fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
        if a.is_empty() || b.is_empty() {
            return 0.0;
        }
        let inter = a.intersection(b).count() as f64;
        let union = a.union(b).count() as f64;
        if union == 0.0 {
            0.0
        } else {
            inter / union
        }
    }

    let kw: HashSet<String> = video_keywords
        .iter()
        .flat_map(|k| tokens(k))
        .collect();

    let mut scores = Vec::new();
    for (i, b) in bgs.iter().enumerate() {
        let q = b.search_query.clone().unwrap_or_default();
        let q_tok = tokens(&q);
        let section_text = sections
            .get(i)
            .map(|s| s.text.as_str())
            .unwrap_or("");
        let s_tok = tokens(section_text);
        // Relevance = max(query∩section, query∩keywords, section∩keywords)
        let r1 = jaccard(&q_tok, &s_tok);
        let r2 = jaccard(&q_tok, &kw);
        let r3 = jaccard(&s_tok, &kw);
        let r = r1.max(r2).max(r3);
        scores.push(r);
        if r < 0.05 && !section_text.is_empty() {
            findings.push(format!(
                "scene {} visuals poorly matched to text ('{}…') — query='{}'",
                i + 1,
                section_text.chars().take(40).collect::<String>(),
                q.chars().take(50).collect::<String>()
            ));
        }
    }
    let avg = if scores.is_empty() {
        0.0
    } else {
        scores.iter().sum::<f64>() / scores.len() as f64
    };
    // Low avg still gets partial credit if keywords present in queries
    let score = if avg >= 0.15 {
        12
    } else if avg >= 0.08 {
        9
    } else if avg >= 0.04 {
        6
    } else if !kw.is_empty() {
        findings.push(
            "search queries weakly aligned with video_keywords / scene text — diversify per-scene queries"
                .into(),
        );
        4
    } else {
        findings.push(
            "no video_keywords and weak query/text overlap — set video_keywords for topic-aware stock"
                .into(),
        );
        2
    };

    DimensionScore {
        id: "context_relevance".into(),
        label: "Context-relevant visual variance".into(),
        score,
        max: 12,
        detail: serde_json::json!({
            "avg_jaccard": (avg * 1000.0).round() / 1000.0,
            "per_scene": scores.iter().map(|v| (v * 1000.0).round() / 1000.0).collect::<Vec<_>>(),
            "video_keywords": video_keywords,
        }),
        findings,
    }
}

/// Weight 8 — cuts per second band (path transitions).
/// Ideal short-form ~0.12–0.55 cuts/s.
fn score_cuts_pacing(bgs: &[BackgroundLayerInfo], duration_ms: i64) -> (DimensionScore, f64) {
    let duration_s = (duration_ms as f64 / 1000.0).max(0.001);
    let mut cuts = 0usize;
    let mut unique_path = HashSet::new();
    for (i, b) in bgs.iter().enumerate() {
        unique_path.insert(b.path.clone());
        if i > 0 && visual_identity(&bgs[i - 1]) != visual_identity(b) {
            cuts += 1;
        }
    }
    let visual_changes = cuts.max(unique_path.len().saturating_sub(1));
    let cps = visual_changes as f64 / duration_s;

    let mut findings = Vec::new();
    let score = if bgs.is_empty() {
        findings.push("no background cuts — empty visual bed".into());
        0
    } else if unique_path.len() == 1 {
        findings.push("single background path for whole video".into());
        2
    } else if (0.12..=0.55).contains(&cps) {
        8
    } else if (0.08..0.12).contains(&cps) || (0.55..0.75).contains(&cps) {
        findings.push(format!(
            "cuts_per_second={:.2} slightly outside ideal 0.12–0.55 band",
            cps
        ));
        5
    } else if cps < 0.08 {
        findings.push(format!(
            "cuts_per_second={:.2} too static (want ≥0.12)",
            cps
        ));
        3
    } else {
        findings.push(format!(
            "cuts_per_second={:.2} too rapid (want ≤0.55)",
            cps
        ));
        3
    };

    (
        DimensionScore {
            id: "cuts_pacing".into(),
            label: "Cuts / visual pacing".into(),
            score,
            max: 8,
            detail: serde_json::json!({
                "cuts_per_second": (cps * 1000.0).round() / 1000.0,
                "visual_changes": visual_changes,
                "ideal_band": [0.12, 0.55],
            }),
            findings,
        },
        cps,
    )
}

/// Weight 10 — music bed presence + ducking + mood/energy tags + non-synthetic.
fn score_music_variance(music: Option<&MusicLayerInfo>) -> DimensionScore {
    let mut findings = Vec::new();
    let score = match music {
        None => {
            findings.push("no background music bed".into());
            0
        }
        Some(m) if is_synthetic_music_file(&m.path) => {
            findings.push("music is synthetic sine-wave placeholder".into());
            0
        }
        Some(m) if !Path::new(&m.path).exists() => {
            findings.push(format!("music path missing on disk: {}", m.path));
            0
        }
        Some(m) => {
            let mut s = 5;
            if m.ducking {
                s += 2;
            } else {
                findings.push("music ducking disabled — risk of fighting dialogue".into());
            }
            if m.mood.as_ref().map(|x| !x.is_empty()).unwrap_or(false) {
                s += 1;
            } else {
                findings.push("music.mood not tagged".into());
            }
            if m.energy.as_ref().map(|x| !x.is_empty()).unwrap_or(false) {
                s += 1;
            }
            if m.gain_db > -24.0 {
                s += 1;
            } else {
                findings.push(format!("music gain_db={:.1} may be inaudible", m.gain_db));
            }
            s.min(10)
        }
    };
    DimensionScore {
        id: "music_variance".into(),
        label: "BG music presence & variance metadata".into(),
        score,
        max: 10,
        detail: serde_json::json!({ "music": music }),
        findings,
    }
}

/// Weight 10 — sticker design principles.
fn score_sticker_design(stickers: &[StickerLayerInfo]) -> DimensionScore {
    let mut findings = Vec::new();
    if stickers.is_empty() {
        findings.push("no stickers/GIFs composited".into());
        return DimensionScore {
            id: "sticker_design".into(),
            label: "Sticker design principles".into(),
            score: 0,
            max: 10,
            detail: serde_json::json!({ "sticker_count": 0 }),
            findings,
        };
    }

    let mut s = 3;
    let unique: HashSet<_> = stickers.iter().map(|x| x.path.as_str()).collect();
    if unique.len() > 1 || stickers.len() == 1 {
        s += 2;
    } else {
        findings.push("same sticker asset repeated for all speakers — weak identity".into());
    }

    let mut scale_ok = 0;
    let mut pos_risk = 0;
    let mut animated = 0;
    for st in stickers {
        if (0.20..=0.45).contains(&st.scale) {
            scale_ok += 1;
        } else {
            findings.push(format!(
                "sticker scale={:.2} outside design band 0.20–0.45",
                st.scale
            ));
        }
        if st.position.to_lowercase().contains("bottom") {
            pos_risk += 1;
            findings.push(format!(
                "sticker position '{}' may collide with caption rail",
                st.position
            ));
        }
        if st.path.ends_with(".gif") || st.path.ends_with(".webp") {
            animated += 1;
        }
    }
    if scale_ok * 2 >= stickers.len() {
        s += 2;
    }
    if pos_risk == 0 {
        s += 2;
    }
    if animated > 0 {
        s += 1;
    } else {
        findings.push("no animated GIF stickers — static PNG only".into());
    }

    DimensionScore {
        id: "sticker_design".into(),
        label: "Sticker design principles".into(),
        score: s.min(10),
        max: 10,
        detail: serde_json::json!({
            "sticker_count": stickers.len(),
            "unique_assets": unique.len(),
            "animated_count": animated,
            "scale_ok_count": scale_ok,
            "bottom_position_risk": pos_risk,
            "design_band_scale": [0.20, 0.45],
        }),
        findings,
    }
}

/// Weight 10 — section composition.
fn score_section_composition(
    sections: &[SectionInfo],
    memes: &[MemeLayerInfo],
) -> DimensionScore {
    let mut findings = Vec::new();
    if sections.is_empty() {
        findings.push("no section map — cannot validate hook/body/cta structure".into());
        return DimensionScore {
            id: "section_composition".into(),
            label: "Section title/text/meme placement".into(),
            score: 2,
            max: 10,
            detail: serde_json::json!({ "sections": 0 }),
            findings,
        };
    }

    let mut s = 0;
    let with_text = sections.iter().filter(|sec| !sec.text.trim().is_empty()).count();
    s += ((with_text as f64 / sections.len() as f64) * 3.0).round() as i32;

    let has_hook = sections.iter().any(|sec| matches!(sec.role, SectionRole::Hook));
    if has_hook {
        s += 2;
    } else {
        findings.push("no explicit Hook section".into());
    }
    let has_cta = sections.iter().any(|sec| matches!(sec.role, SectionRole::Cta));
    if has_cta {
        s += 2;
    } else {
        findings.push("no CTA section".into());
    }

    let titled = sections
        .iter()
        .filter(|sec| sec.title_text.as_ref().map(|t| !t.is_empty()).unwrap_or(false))
        .count();
    if titled > 0 {
        s += 2;
    } else {
        findings.push("no on-screen title/cards in any section".into());
    }

    let body_with_meme = sections
        .iter()
        .filter(|sec| {
            matches!(sec.role, SectionRole::Body | SectionRole::Payoff)
                && memes
                    .iter()
                    .any(|m| m.start_ms < sec.end_ms && m.end_ms > sec.start_ms)
        })
        .count();
    if !memes.is_empty() {
        s += 1;
        if body_with_meme == 0 {
            findings.push("memes present but none land in body/payoff sections".into());
        }
    } else {
        findings.push("no meme/reaction cuts — enable meme_brolls for punch".into());
    }

    DimensionScore {
        id: "section_composition".into(),
        label: "Section title/text/meme placement".into(),
        score: s.min(10),
        max: 10,
        detail: serde_json::json!({
            "section_count": sections.len(),
            "with_text": with_text,
            "with_title_card": titled,
            "has_hook": has_hook,
            "has_cta": has_cta,
            "meme_count": memes.len(),
            "body_sections_with_meme": body_with_meme,
        }),
        findings,
    }
}

/// Weight 8 — speech.
fn score_speech(has_dialogue: bool, rms_ok: bool) -> DimensionScore {
    let mut findings = Vec::new();
    let score = if has_dialogue && rms_ok {
        8
    } else if has_dialogue {
        findings.push("dialogue present but loudness outside ideal band".into());
        5
    } else {
        findings.push("no dialogue detected".into());
        0
    };
    DimensionScore {
        id: "speech_audio".into(),
        label: "Speech / dialogue".into(),
        score,
        max: 8,
        detail: serde_json::json!({ "has_dialogue": has_dialogue, "rms_ok": rms_ok }),
        findings,
    }
}

/// Weight 8 — captions present.
fn score_captions(captions_path: Option<&str>) -> DimensionScore {
    let present = captions_path
        .map(|p| !p.is_empty() && Path::new(p).exists())
        .unwrap_or(false);
    let mut findings = Vec::new();
    if !present {
        findings.push("captions file missing".into());
    }
    DimensionScore {
        id: "captions".into(),
        label: "Captions".into(),
        score: if present { 8 } else { 0 },
        max: 8,
        detail: serde_json::json!({ "captions_path": captions_path, "present": present }),
        findings,
    }
}

/// Weight 12 — efficacious use of the multi-track timeline editor.
pub fn score_timeline_editor(timeline: &Timeline) -> TimelineEditorReport {
    let mut findings = Vec::new();
    let mut with_events = Vec::new();
    let mut empty = Vec::new();
    let mut counts = serde_json::Map::new();

    let all_tracks = [
        TrackType::Dialogue,
        TrackType::Voiceover,
        TrackType::Captions,
        TrackType::Broll,
        TrackType::Music,
        TrackType::Sfx,
    ];
    for tt in &all_tracks {
        let n = timeline.tracks.get(tt).map(|v| v.len()).unwrap_or(0);
        counts.insert(tt.to_string(), serde_json::json!(n));
        if n > 0 {
            with_events.push(tt.to_string());
        } else {
            empty.push(tt.to_string());
        }
    }

    // Unique visual assets from broll map
    let unique_visual = timeline.assets.broll.len();

    // Gaps / overlaps on broll track
    let mut gap_ms = 0i64;
    let mut overlap_ms = 0i64;
    if let Some(events) = timeline.tracks.get(&TrackType::Broll) {
        let mut sorted = events.clone();
        sorted.sort_by_key(|e| e.start_ms);
        for w in sorted.windows(2) {
            let a = &w[0];
            let b = &w[1];
            if b.start_ms > a.end_ms {
                gap_ms += b.start_ms - a.end_ms;
            } else if b.start_ms < a.end_ms {
                overlap_ms += a.end_ms - b.start_ms;
            }
        }
        if events.len() <= 1 && timeline.rendered_duration_ms() > 5000 {
            findings.push(
                "broll track has ≤1 event while duration >5s — multi-scene stock not reflected in timeline editor"
                    .into(),
            );
        }
    } else {
        findings.push("no broll track events".into());
    }

    if !timeline.tracks.get(&TrackType::Music).map(|v| !v.is_empty()).unwrap_or(false) {
        findings.push("music track empty in timeline editor".into());
    }
    if !timeline.tracks.get(&TrackType::Sfx).map(|v| !v.is_empty()).unwrap_or(false) {
        findings.push("sfx track empty — no punctuation whooshes/hits".into());
    }
    if !timeline.tracks.get(&TrackType::Captions).map(|v| !v.is_empty()).unwrap_or(false)
        && timeline.assets.captions.is_empty()
    {
        findings.push("captions track empty and no captions assets".into());
    }

    // Utilization 0–100, later scaled into max-4 contribution
    let critical = ["voiceover", "broll", "music", "captions"];
    let critical_hit = critical
        .iter()
        .filter(|t| {
            with_events.iter().any(|e| e == *t)
                || (**t == "captions" && !timeline.assets.captions.is_empty())
        })
        .count();
    let mut util = (critical_hit as i32 * 20).min(80);
    if unique_visual >= 3 {
        util += 10;
    } else if unique_visual >= 2 {
        util += 5;
    }
    if timeline.tracks.get(&TrackType::Sfx).map(|v| !v.is_empty()).unwrap_or(false) {
        util += 10;
    }
    if gap_ms > 500 {
        findings.push(format!("background gaps totaling {}ms on broll track", gap_ms));
        util = (util - 10).max(0);
    }
    if overlap_ms > 1500 {
        findings.push(format!(
            "background overlaps totaling {}ms — check change_cadence",
            overlap_ms
        ));
    }

    TimelineEditorReport {
        utilization_score: util.min(100),
        tracks_with_events: with_events,
        empty_tracks: empty,
        event_counts: serde_json::Value::Object(counts),
        unique_visual_assets: unique_visual,
        background_gaps_ms: gap_ms,
        background_overlaps_ms: overlap_ms,
        findings,
    }
}

// ---------------------------------------------------------------------------
// Aggregate
// ---------------------------------------------------------------------------

/// Build default sections from voiceover events when script sections omitted.
pub fn sections_from_timeline(timeline: &Timeline) -> Vec<SectionInfo> {
    let mut vos: Vec<_> = timeline
        .tracks
        .get(&TrackType::Voiceover)
        .cloned()
        .unwrap_or_default();
    vos.sort_by_key(|e| e.start_ms);
    let n = vos.len();
    vos.into_iter()
        .enumerate()
        .map(|(i, e)| {
            let role = if i == 0 {
                SectionRole::Hook
            } else if i + 1 == n {
                SectionRole::Cta
            } else if i + 2 >= n {
                SectionRole::Payoff
            } else {
                SectionRole::Body
            };
            let text = match &e.kind {
                EventKind::Voiceover { text, .. } => text.clone(),
                _ => String::new(),
            };
            SectionInfo {
                role,
                start_ms: e.start_ms,
                end_ms: e.end_ms,
                text,
                title_text: None,
            }
        })
        .collect()
}

/// Infer background layers from timeline when manifest lacks them.
pub fn backgrounds_from_timeline(timeline: &Timeline) -> Vec<BackgroundLayerInfo> {
    let mut out = Vec::new();
    if let Some(events) = timeline.tracks.get(&TrackType::Broll) {
        for e in events {
            let path = timeline
                .assets
                .broll
                .get(&e.asset_id)
                .and_then(|v| v.get("path"))
                .and_then(|p| p.as_str())
                .unwrap_or("")
                .to_string();
            let source_hint = match &e.kind {
                EventKind::Broll {
                    source_provider, ..
                } => Some(source_provider.clone()),
                _ => None,
            };
            if !path.is_empty() {
                out.push(BackgroundLayerInfo {
                    path,
                    start_ms: e.start_ms,
                    end_ms: e.end_ms,
                    source_hint,
                    content_hash: None,
                    video_id: None,
                    search_query: None,
                });
            }
        }
    }
    out
}

pub fn evaluate_production_quality(
    timeline: &Timeline,
    manifest: &RenderManifest,
) -> ProductionQualityReport {
    let duration_ms = if manifest.duration_ms > 0 {
        manifest.duration_ms
    } else {
        timeline.rendered_duration_ms()
    };

    let backgrounds = if !manifest.backgrounds.is_empty() {
        manifest.backgrounds.clone()
    } else {
        backgrounds_from_timeline(timeline)
    };

    let sections = if !manifest.sections.is_empty() {
        manifest.sections.clone()
    } else {
        sections_from_timeline(timeline)
    };

    let music = manifest.music.clone().or_else(|| {
        timeline.assets.music.values().next().and_then(|v| {
            let path = v.get("path")?.as_str()?.to_string();
            Some(MusicLayerInfo {
                path,
                gain_db: 0.0,
                ducking: true,
                mood: None,
                energy: None,
            })
        })
    });

    let captions_path = manifest.captions_path.clone().or_else(|| {
        timeline
            .assets
            .captions
            .get("ass")
            .and_then(|a| a.get("path"))
            .and_then(|p| p.as_str())
            .map(String::from)
            .or_else(|| {
                timeline
                    .assets
                    .captions
                    .get("path")
                    .and_then(|p| p.as_str())
                    .map(String::from)
            })
    });

    let d_source = score_video_source(&backgrounds);
    let d_repeat = score_visual_repetition(&backgrounds);
    let d_context =
        score_context_relevance(&backgrounds, &sections, &manifest.video_keywords);
    let (d_cuts, cps) = score_cuts_pacing(&backgrounds, duration_ms);
    let d_music = score_music_variance(music.as_ref());
    let d_sticker = score_sticker_design(&manifest.stickers);
    let d_section = score_section_composition(&sections, &manifest.memes);
    let d_speech = score_speech(manifest.has_dialogue, manifest.rms_ok);
    let d_cap = score_captions(captions_path.as_deref());
    let timeline_editor = score_timeline_editor(timeline);

    // max 4 from editor utilization
    let editor_score = ((timeline_editor.utilization_score as f64) * 0.04).round() as i32;
    let d_editor = DimensionScore {
        id: "timeline_editor".into(),
        label: "Timeline editor efficacious use".into(),
        score: editor_score.min(4),
        max: 4,
        detail: serde_json::to_value(&timeline_editor).unwrap_or(serde_json::json!({})),
        findings: timeline_editor.findings.clone(),
    };

    // Weights: 14+16+12+8+10+10+10+8+8+4 = 100
    let dimensions = vec![
        d_source,
        d_repeat,
        d_context,
        d_cuts,
        d_music,
        d_sticker,
        d_section,
        d_speech,
        d_cap,
        d_editor,
    ];

    let production_score: i32 = dimensions.iter().map(|d| d.score).sum::<i32>().clamp(0, 100);

    let mut hard_fails = Vec::new();
    let mut next_actions = Vec::new();
    for d in &dimensions {
        for f in &d.findings {
            if d.score == 0
                && matches!(
                    d.id.as_str(),
                    "video_source_quality"
                        | "visual_repetition"
                        | "music_variance"
                        | "speech_audio"
                        | "captions"
                )
            {
                hard_fails.push(format!("{}: {}", d.id, f));
            }
            // Always surface REPETITION flags as hard fails when present
            if f.contains("REPETITION") || f.contains("HARD:") {
                let msg = format!("{}: {}", d.id, f);
                if !hard_fails.contains(&msg) {
                    hard_fails.push(msg);
                }
            }
        }
        if d.score * 2 < d.max {
            next_actions.push(format!(
                "Improve {}: score {}/{} — {}",
                d.id,
                d.score,
                d.max,
                d.findings.first().cloned().unwrap_or_else(|| "see detail".into())
            ));
        }
    }

    // Source mix summary
    let mut mix = serde_json::Map::new();
    for b in &backgrounds {
        let cls = classify_video_source(&b.path, b.source_hint.as_deref());
        let key = format!("{:?}", cls);
        let c = mix.get(&key).and_then(|v| v.as_u64()).unwrap_or(0) + 1;
        mix.insert(key, serde_json::json!(c));
    }

    ProductionQualityReport {
        production_score,
        grade: production_grade(production_score).to_string(),
        dimensions,
        hard_fails,
        next_actions,
        timeline_editor,
        cuts_per_second: (cps * 1000.0).round() / 1000.0,
        video_source_mix: serde_json::Value::Object(mix),
        kpi_version: "2.1.0".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::Timeline;
    use std::path::PathBuf;

    fn empty_timeline() -> Timeline {
        Timeline::new(PathBuf::from("source.mp4"), "9:16", 30, None)
    }

    #[test]
    fn classify_procedural_and_youtube() {
        assert_eq!(
            classify_video_source("mcp/assets/backgrounds/procedural_01.mp4", None),
            VideoSourceClass::ProceduralSynthetic
        );
        assert_eq!(
            classify_video_source("mcp/assets/background_cache/scene_001_yt.mp4", None),
            VideoSourceClass::Youtube
        );
        assert_eq!(
            classify_video_source("cache/x.mp4", Some("pexels")),
            VideoSourceClass::Pexels
        );
    }

    #[test]
    fn procedural_only_is_low_grade() {
        let tl = empty_timeline();
        let manifest = RenderManifest {
            duration_ms: 16000,
            backgrounds: vec![
                BackgroundLayerInfo {
                    path: "mcp/assets/backgrounds/procedural_01.mp4".into(),
                    start_ms: 0,
                    end_ms: 8000,
                    source_hint: None,
                    content_hash: Some("proc1".into()),
                    video_id: None,
                    search_query: None,
                },
                BackgroundLayerInfo {
                    path: "mcp/assets/backgrounds/procedural_02.mp4".into(),
                    start_ms: 8000,
                    end_ms: 16000,
                    source_hint: None,
                    content_hash: Some("proc2".into()),
                    video_id: None,
                    search_query: None,
                },
            ],
            has_dialogue: true,
            rms_ok: true,
            captions_path: Some("captions.ass".into()),
            ..Default::default()
        };
        // captions path may not exist → captions 0
        let report = evaluate_production_quality(&tl, &manifest);
        assert!(
            report.production_score < 55,
            "procedural-only should be <C, got {}",
            report.production_score
        );
        assert!(matches!(report.grade.as_str(), "D" | "F"));
    }

    #[test]
    fn rich_manifest_scores_high() {
        let tl = empty_timeline();
        let music = std::env::temp_dir().join("pq_music.mp3");
        std::fs::write(&music, vec![2u8; 9000]).unwrap();
        let caps = std::env::temp_dir().join("pq_caps.ass");
        std::fs::write(&caps, b"[Script Info]\n").unwrap();

        let manifest = RenderManifest {
            duration_ms: 20000,
            backgrounds: (0..5)
                .map(|i| BackgroundLayerInfo {
                    path: format!("mcp/assets/background_cache/scene_{:03}_yt.mp4", i + 1),
                    start_ms: i * 4000,
                    end_ms: (i + 1) * 4000,
                    source_hint: Some("youtube".into()),
                    content_hash: Some(format!("unique_hash_{}", i)),
                    video_id: Some(format!("vid{}", i)),
                    search_query: Some(format!("morning habit {}", i)),
                })
                .collect(),
            stickers: vec![StickerLayerInfo {
                path: "mcp/assets/stickers/giphy_alice.gif".into(),
                start_ms: 0,
                end_ms: 20000,
                position: "top-left".into(),
                scale: 0.35,
            }],
            memes: vec![MemeLayerInfo {
                path: "meme.mp4".into(),
                start_ms: 5000,
                end_ms: 7500,
            }],
            music: Some(MusicLayerInfo {
                path: music.to_string_lossy().to_string(),
                gain_db: 0.0,
                ducking: true,
                mood: Some("calm".into()),
                energy: Some("low".into()),
            }),
            captions_path: Some(caps.to_string_lossy().to_string()),
            voiceover_count: 5,
            sections: vec![
                SectionInfo {
                    role: SectionRole::Hook,
                    start_ms: 0,
                    end_ms: 4000,
                    text: "Hook text morning".into(),
                    title_text: Some("3 HABITS".into()),
                },
                SectionInfo {
                    role: SectionRole::Body,
                    start_ms: 4000,
                    end_ms: 12000,
                    text: "Body morning habit".into(),
                    title_text: None,
                },
                SectionInfo {
                    role: SectionRole::Cta,
                    start_ms: 12000,
                    end_ms: 20000,
                    text: "Start tomorrow".into(),
                    title_text: None,
                },
            ],
            has_dialogue: true,
            rms_ok: true,
            video_keywords: vec!["morning".into(), "habit".into()],
        };
        let report = evaluate_production_quality(&tl, &manifest);
        let _ = std::fs::remove_file(&music);
        let _ = std::fs::remove_file(&caps);
        assert!(
            report.production_score >= 70,
            "rich stack should be ≥B, got {} {:?}",
            report.production_score,
            report.dimensions.iter().map(|d| (d.id.clone(), d.score)).collect::<Vec<_>>()
        );
        assert!(report.cuts_per_second > 0.1);
    }

    #[test]
    fn sticker_bottom_position_flagged() {
        let d = score_sticker_design(&[StickerLayerInfo {
            path: "x.png".into(),
            start_ms: 0,
            end_ms: 1000,
            position: "bottom-left".into(),
            scale: 0.35,
        }]);
        assert!(d.findings.iter().any(|f| f.contains("caption")));
    }

    #[test]
    fn same_content_hash_across_paths_flags_repetition() {
        let bgs: Vec<BackgroundLayerInfo> = (0..5)
            .map(|i| BackgroundLayerInfo {
                path: format!("cache/scene_{:03}.mp4", i),
                start_ms: i * 3000,
                end_ms: (i + 1) * 3000,
                source_hint: Some("youtube".into()),
                content_hash: Some("deadbeef_111".into()), // SAME hash — the bug
                video_id: Some("abc123".into()),
                search_query: Some("morning routine".into()),
            })
            .collect();
        let d = score_visual_repetition(&bgs);
        assert_eq!(d.score, 0, "identical content must score 0, got {}", d.score);
        assert!(
            d.findings.iter().any(|f| f.contains("REPETITION") || f.contains("HARD")),
            "must flag repetition: {:?}",
            d.findings
        );
    }

    #[test]
    fn unique_hashes_score_high() {
        let bgs: Vec<BackgroundLayerInfo> = (0..5)
            .map(|i| BackgroundLayerInfo {
                path: format!("cache/scene_{:03}.mp4", i),
                start_ms: i * 3000,
                end_ms: (i + 1) * 3000,
                source_hint: Some("youtube".into()),
                content_hash: Some(format!("hash_{}", i)),
                video_id: Some(format!("vid{}", i)),
                search_query: Some(format!("query {}", i)),
            })
            .collect();
        let d = score_visual_repetition(&bgs);
        assert!(d.score >= 14, "unique hashes should be high, got {}", d.score);
    }
}
