//! Production Quality Model — architecture-level KPIs for AI-directed shorts.
//!
//! `verify.render` is **technical integrity** only. This module scores whether the
//! timeline/render actually uses the editor like a director:
//! video source quality, visual hooks, cut pacing, music quality, sfx quality,
//! sticker design, caption quality, voiceover quality, audio mix quality,
//! section composition, visual hierarchy, platform optimization, and track utilization.
//!
//! Weights sum to 100. Grade bands:
//!   A 85–100 · B 70–84 · C 55–69 · D 40–54 · F <40
//!
//! v4.0 (2026-07-20):
//! - sfx_quality (6 pts) — SFX punctuation & variety
//! - music_quality (8 pts, expanded from music_variance)
//! - caption_quality (6 pts, expanded from captions)
//! - voiceover_quality (6 pts, new)
//! - audio_mix_quality (5 pts, new) — LUFS, peak, ducking
//! - visual_hierarchy (5 pts, new)
//! - platform_optimization (5 pts, new)
//! - hard gates: no-SFX->C, CPS>25->C, LUFS out-of-range->C, clipping->D

use crate::timeline::{
    EventKind, MAX_SEGMENT_DURATION_S, MIN_SEGMENT_DURATION_S, Timeline,
};
use crate::types::TrackType;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Verify layer order in the render pipeline.
/// Expected order (bottom to top): Background → Meme → Captions → Stickers
/// Returns a report with any violations.
pub fn verify_layer_order(manifest: &RenderManifest) -> LayerOrderReport {
    let mut findings = Vec::new();
    let mut hard_fails = Vec::new();

    // The render pipeline in multilayer_render.rs builds the filter graph in this order:
    // 1. Background concat [vbg]
    // 2. Meme b-roll overlays on background [vmb*]
    // 3. Captions burned on top [vcap]
    // 4. Sticker overlays on top [vst* → vout]
    //
    // This is the CORRECT order: Background → Meme → Captions → Stickers
    // We validate that the manifest has the expected layers in the right z-order.

    // Check: if there are stickers, captions should be present (or at least configured)
    if !manifest.stickers.is_empty() && manifest.captions_path.is_none() {
        findings.push("WARNING: stickers present but no captions configured — stickers will be topmost layer".into());
    }

    // Check: meme clips should be configured if present in manifest
    // (they are validated in the render pipeline)

    // Hard fail: if stickers exist but caption style is "word_highlight" or "karaoke"
    // and stickers are at bottom positions, they WILL overlap
    for st in &manifest.stickers {
        let pos = st.position.to_lowercase();
        if (pos.contains("bottom") || pos == "center") && manifest.caption_style.as_deref() == Some("word_highlight") {
            hard_fails.push(format!(
                "HARD: sticker '{}' at position '{}' will overlap word_highlight caption rail",
                st.path, st.position
            ));
        }
    }

    LayerOrderReport {
        expected_order: vec![
            "Background (vbg)".into(),
            "Meme b-roll (vmb)".into(),
            "Captions (vcap)".into(),
            "Stickers (vst)".into(),
        ],
        actual_order: vec![
            "Background".into(),
            if !manifest.memes.is_empty() { "Meme".into() } else { "Meme (none)".into() },
            if manifest.captions_path.is_some() { "Captions".into() } else { "Captions (none)".into() },
            if !manifest.stickers.is_empty() { "Stickers".into() } else { "Stickers (none)".into() },
        ],
        findings,
        hard_fails,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerOrderReport {
    pub expected_order: Vec<String>,
    pub actual_order: Vec<String>,
    pub findings: Vec<String>,
    pub hard_fails: Vec<String>,
}

/// Calculate the sticker bounding box in canvas pixel coordinates.
/// Returns (left, top, right, bottom).
fn sticker_bbox(sticker: &StickerLayerInfo, canvas_w: u32, canvas_h: u32) -> (i32, i32, i32, i32) {
    let sticker_size = (canvas_w as f64 * sticker.scale).round() as i32;
    let margin = 40i32;
    let cw = canvas_w as i32;
    let ch = canvas_h as i32;

    let (tl_x, tl_y) = match sticker.position.to_lowercase().as_str() {
        "top-left" => (margin, margin),
        "top-right" => (cw - sticker_size - margin, margin),
        "top-center" | "center-top" => ((cw - sticker_size) / 2, margin),
        "bottom-left" => (margin, ch - sticker_size - margin),
        "bottom-right" => (cw - sticker_size - margin, ch - sticker_size - margin),
        "bottom-center" | "center-bottom" => ((cw - sticker_size) / 2, ch - sticker_size - margin),
        "center" => ((cw - sticker_size) / 2, (ch - sticker_size) / 2),
        _ => (margin, margin), // default to top-left
    };

    (tl_x, tl_y, tl_x + sticker_size, tl_y + sticker_size)
}

/// Check if two axis-aligned rectangles overlap.
fn boxes_overlap(a: (i32, i32, i32, i32), b: (i32, i32, i32, i32)) -> bool {
    !(a.2 <= b.0 || b.2 <= a.0 || a.3 <= b.1 || b.3 <= a.1)
}

/// Calculate the caption safe zone (bottom area reserved for captions) based on style and canvas height.
/// Returns (top_y, bottom_y) in pixels, where top_y is the start of the safe zone from top.
fn caption_safe_zone(canvas_h: u32, style: Option<&str>) -> (i32, i32) {
    let h = canvas_h as i32;
    // Caption styles and their approximate bottom rail heights
    let rail_ratio = match style {
        Some("word_highlight" | "karaoke") => 0.15, // ~288px on 1920
        Some("sentence_fade") => 0.12,
        Some("burn_in" | "subtitle_rail") => 0.10,
        _ => 0.12, // default
    };
    let rail_h = (h as f64 * rail_ratio).round() as i32;
    (h - rail_h, h)
}

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
    /// Lexical relevance of stock title vs scene signal (0–1), when ranked.
    #[serde(default)]
    pub lexical_score: Option<f64>,
    /// Provider title / description snippet used at accept time.
    #[serde(default)]
    pub source_title: Option<String>,
}

/// Music bed metadata for variance scoring.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
    /// Selection tags (focus, chill, parade, …) for topic-fit scoring.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Query used to select this bed (library / yt / pixabay).
    #[serde(default)]
    pub selection_query: Option<String>,
    /// Provider: library | pixabay | youtube | stock | unknown
    #[serde(default)]
    pub source: Option<String>,
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

/// A b-roll coverage gap: the assigned clip is shorter than its segment
/// window. Previously the renderer silently looped such clips to fill the
/// window (the "videos are looping" bug). Per docs/SEGMENTATION_UPGRADE_PLAN.md
/// Phase B, the validator now surfaces these as actionable errors so the
/// agent re-runs keyword generation + broll.fetch for a longer clip.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BrollGap {
    /// Timeline event id of the b-roll placement (e.g. "broll_001").
    pub segment_id: String,
    /// Visual concept the clip was fetched for (from the event kind).
    pub concept: String,
    /// Asset id the clip is registered under (e.g. "broll_0").
    pub asset_id: String,
    /// Local path of the fetched clip.
    pub asset_path: String,
    /// Segment window duration in seconds.
    pub required_s: f64,
    /// Actual source clip duration in seconds (probed via ffprobe).
    pub available_s: f64,
    /// `required_s - available_s` — how many seconds are uncovered.
    pub gap_s: f64,
    /// Directive for the agent loop, e.g. "re-run broll.keywords + broll.fetch…".
    pub action: String,
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
    /// Script theme (calm / energetic / …) for music topic fit.
    #[serde(default)]
    pub theme: Option<String>,
    /// Count of auto-assigned SFX events (hook punctuation).
    #[serde(default)]
    pub sfx_count: usize,
    /// Caption style: "word_highlight", "sentence_fade", "karaoke", "burn_in".
    #[serde(default)]
    pub caption_style: Option<String>,
    /// Fraction of voiceover duration covered by captions (0.0–1.0).
    #[serde(default)]
    pub caption_coverage_ratio: f64,
    /// Average words per caption line.
    #[serde(default)]
    pub caption_words_per_line: Option<f64>,
    /// Average characters per second (reading speed).
    #[serde(default)]
    pub caption_chars_per_second: Option<f64>,
    /// Average TTS pace in words per minute (ideal 130–160).
    #[serde(default)]
    pub voiceover_wpm: Option<f64>,
    /// Voice IDs used per speaker slot.
    #[serde(default)]
    pub voice_ids: Vec<String>,
    /// True when TTS emote tags align to content sentiment.
    #[serde(default)]
    pub emote_alignment_ok: bool,
    /// Integrated loudness in LUFS (EBU R128). Target: -16 ± 2.
    #[serde(default)]
    pub lufs: Option<f64>,
    /// True peak level in dBFS. Must be < -1.
    #[serde(default)]
    pub peak_dbfs: Option<f64>,
    /// Measured music ducking depth in dB during speech. Target: >=10 dB.
    #[serde(default)]
    pub ducking_depth_db: Option<f64>,
    /// Output aspect ratio (e.g. "9:16", "16:9", "1:1").
    #[serde(default)]
    pub aspect_ratio: Option<String>,
    /// Measured integrated loudness (LUFS, EBU R128) from rendered video.
    #[serde(default)]
    pub measured_lufs: Option<f64>,
    /// Measured true peak level in dBFS from rendered video.
    #[serde(default)]
    pub measured_peak_dbfs: Option<f64>,
    /// Measured music ducking depth in dB during speech segments.
    #[serde(default)]
    pub measured_ducking_depth_db: Option<f64>,
    /// Measured music gain in dB from rendered video.
    #[serde(default)]
    pub measured_music_gain_db: Option<f64>,
    /// Fraction of frames with non-zero motion (scene_score > 0.001),
    /// measured from the rendered video via ffmpeg `metadata=print`.
    /// 0.0 = entirely static, 1.0 = motion in every frame.
    /// `None` = not probed (verifier was not run on rendered output).
    /// Targets: >= 0.50 for videos with b-roll (otherwise perceived as
    /// static images during the static stretches — the bug fixed in
    /// Phase 129 where short Pexels sources were held as last-frame
    /// after `seek_offset` exhausted the source mid-segment).
    #[serde(default)]
    pub broll_motion_ratio: Option<f64>,
    /// Longest consecutive static run (in seconds) measured from the
    /// rendered video. A run is a sequence of consecutive frames whose
    /// `scene_score < 0.001`. Targets: <= 1.5s for videos with b-roll
    /// — a 9-12s static stretch is the signature of the source-exhaustion
    /// bug.
    #[serde(default)]
    pub longest_static_run_s: Option<f64>,
    /// B-roll coverage gaps: segments whose assigned clip is shorter than
    /// the segment window. Populated by the verifier probing each asset
    /// duration; feeds `score_broll_motion` hard-fail findings so the
    /// agent knows exactly which segments need a longer clip.
    #[serde(default)]
    pub broll_gaps: Vec<BrollGap>,
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
    let p = path.replace('\\', "/");
    // Committed stock catalog is synthetic sine stubs (all OpenScript Stock).
    if p.contains("mcp/assets/music/") {
        return true;
    }
    std::fs::metadata(path)
        .map(|m| m.len() == 481_114)
        .unwrap_or(false)
}

/// Moods/themes that should never pick hype/parade/trailer beds.
pub fn is_calm_focus_context(theme: Option<&str>, video_keywords: &[String]) -> bool {
    let mut blob = theme.unwrap_or("").to_ascii_lowercase();
    for k in video_keywords {
        blob.push(' ');
        blob.push_str(&k.to_ascii_lowercase());
    }
    ["calm", "focus", "desk", "study", "lofi", "ambient", "meditation", "sleep", "chill"]
        .iter()
        .any(|t| blob.contains(t))
}

/// Title/path/tags that clash with calm/focus shorts (parade music, etc.).
pub fn music_hits_denylist(path: &str, mood: Option<&str>, tags: &[String], selection_query: Option<&str>) -> bool {
    let blob = format!(
        "{} {} {} {}",
        path.to_ascii_lowercase(),
        mood.unwrap_or("").to_ascii_lowercase(),
        tags.join(" ").to_ascii_lowercase(),
        selection_query.unwrap_or("").to_ascii_lowercase()
    );
    const DENY: &[&str] = &[
        "parade", "march", "military", "trailer", "epic war", "sport hype",
        "stadium", "anthem", "circus", "carnival", "polka",
    ];
    DENY.iter().any(|d| blob.contains(d))
}

/// Fraction of backgrounds that are procedural synthetic (0–1).
pub fn procedural_ratio(bgs: &[BackgroundLayerInfo]) -> f64 {
    if bgs.is_empty() {
        return 1.0;
    }
    let proc = bgs
        .iter()
        .filter(|b| {
            matches!(
                classify_video_source(&b.path, b.source_hint.as_deref()),
                VideoSourceClass::ProceduralSynthetic
            )
        })
        .count();
    proc as f64 / bgs.len() as f64
}

pub fn production_grade(score: i32) -> &'static str {
    // v4.1: dimensions now sum to 108 (added broll_motion at 8 pts).
    // Grade thresholds rescaled proportionally: 85/100 -> 92/108, etc.
    match score {
        92..=108 => "A",
        76..=91 => "B",
        59..=75 => "C",
        43..=58 => "D",
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

/// Weight 12 — video source quality mix (Pexels > YT > local > unknown > procedural).
fn score_video_source(bgs: &[BackgroundLayerInfo]) -> DimensionScore {
    let n = bgs.len().max(1);
    let mut tier_sum = 0.0;
    let mut findings = Vec::new();
    let mut mix_counts: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    let mut procedural_n = 0usize;
    for b in bgs {
        let cls = classify_video_source(&b.path, b.source_hint.as_deref());
        tier_sum += cls.quality_tier();
        if matches!(cls, VideoSourceClass::ProceduralSynthetic) {
            procedural_n += 1;
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
    let mut score = (avg * 10.0).round() as i32;
    let ratio = procedural_n as f64 / n as f64;
    if procedural_n == n && n > 0 {
        findings.push(
            "HARD: all backgrounds are synthetic procedural — not production stock".into(),
        );
        score = 0;
    } else if ratio >= 0.5 && n >= 2 {
        findings.push(format!(
            "HARD: majority procedural backgrounds ({}/{} = {:.0}%) — no production visual bed",
            procedural_n,
            n,
            ratio * 100.0
        ));
        // Cap source score hard — majority synthetic is not Grade-B material
        score = score.min(2);
    }
    DimensionScore {
        id: "video_source_quality".into(),
        label: "Video source quality".into(),
        score,
        max: 10,
        detail: serde_json::json!({
            "clip_count": bgs.len(),
            "procedural_count": procedural_n,
            "procedural_ratio": (ratio * 1000.0).round() / 1000.0,
            "average_tier": (avg * 1000.0).round() / 1000.0,
            "source_mix": mix_counts,
        }),
        findings,
    }
}

/// Weight 10 — visual hooks: real stock presence (not procedural) especially in the open.
fn score_visual_hooks(bgs: &[BackgroundLayerInfo], duration_ms: i64) -> DimensionScore {
    let mut findings = Vec::new();
    if bgs.is_empty() {
        findings.push("HARD: no visual hooks — zero background clips".into());
        return DimensionScore {
            id: "visual_hooks".into(),
            label: "Visual hooks (real stock / open)".into(),
            score: 0,
            max: 10,
            detail: serde_json::json!({}),
            findings,
        };
    }
    let real: Vec<&BackgroundLayerInfo> = bgs
        .iter()
        .filter(|b| {
            !matches!(
                classify_video_source(&b.path, b.source_hint.as_deref()),
                VideoSourceClass::ProceduralSynthetic
            )
        })
        .collect();
    let real_n = real.len();
    let real_ratio = real_n as f64 / bgs.len() as f64;

    // Hook window: first 3 seconds need a real stock cut if possible
    let hook_has_real = bgs.iter().any(|b| {
        b.start_ms < 3000
            && !matches!(
                classify_video_source(&b.path, b.source_hint.as_deref()),
                VideoSourceClass::ProceduralSynthetic
            )
            && (b.end_ms - b.start_ms) >= 1200
    });

    let score = if real_n == 0 {
        findings.push(
            "HARD: no visual hooks — all backgrounds procedural/synthetic gradients".into(),
        );
        0
    } else {
        let mut s = (real_ratio * 6.0).round() as i32; // up to 6 for coverage
        if hook_has_real {
            s += 2;
        } else {
            findings.push("opening 3s lacks real stock visual hook".into());
            s = s.saturating_sub(1);
        }
        // Bonus if any clip has lexical score evidence
        let lex_ok = real
            .iter()
            .filter(|b| b.lexical_score.unwrap_or(0.0) >= 0.12)
            .count();
        if lex_ok > 0 {
            s = (s + 1).min(8);
        }
        s
    };
    let mut score = score;
    if duration_ms > 8000 && real_n < 2 && bgs.len() >= 3 {
        findings.push("multi-scene short with <2 real stock clips — weak visual variety".into());
        score = score.min(3);
    }
    DimensionScore {
        id: "visual_hooks".into(),
        label: "Visual hooks (real stock / open)".into(),
        score: score.clamp(0, 8),
        max: 8,
        detail: serde_json::json!({
            "real_stock_count": real_n,
            "total_clips": bgs.len(),
            "real_ratio": (real_ratio * 1000.0).round() / 1000.0,
            "hook_has_real_stock": hook_has_real,
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

/// Weight 12 — **visual repetitiveness** (content-hash / video-id uniqueness).
/// Path-only uniqueness is insufficient: same YT video can be saved to different paths.
fn score_visual_repetition(bgs: &[BackgroundLayerInfo]) -> DimensionScore {
    let mut findings = Vec::new();
    let n = bgs.len();
    if n == 0 {
        return DimensionScore {
            id: "visual_repetition".into(),
            label: "Visual variance / anti-repeat".into(),
            score: 0,
            max: 12,
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

    let mut score = (uniqueness * 8.0).round() as i32;
    if max_run >= 3 && n >= 3 {
        findings.push(format!(
            "REPETITION: same visual identity runs for {} consecutive cuts — looks like one clip looping",
            max_run
        ));
        score = (score - 4).max(0);
    } else if max_run >= 2 && n >= 4 {
        findings.push(format!(
            "back-to-back repeat of same visual for {} cuts",
            max_run
        ));
        score = (score - 2).max(0);
    }
    if dominant_share > 0.5 && n >= 3 {
        findings.push(format!(
            "REPETITION: one source used in {:.0}% of scenes — lack of context-relevant variance",
            dominant_share * 100.0
        ));
        score = (score - 3).max(0);
    }
    if uniqueness < 0.5 && n >= 3 {
        findings.push(format!(
            "unique visual identities only {:.0}% — {} distinct clips reused across {} cuts (want ≥80% for multi-scene shorts)",
            uniqueness * 100.0, unique.len(), n
        ));
    }
    // Actionable anti-repeat directive: when the same clip pool is stretched
    // over far more cuts than there are distinct clips, tell the agent to
    // re-fetch MORE distinct footage (broll.fetch download_n) rather than
    // accept re-styled reuse. This is the loop-closure signal for the
    // "same clip, different zoom/pan" failure mode.
    if unique.len() < n && n >= 3 {
        findings.push(format!(
            "ANTI-REPEAT: {n} cuts draw from only {} distinct clip(s) (ratio {:.0}%). Re-run broll.fetch with download_n >= {} (or more concepts) so each segment gets its own footage — same clip re-styled with different zoom/pan is not new content.",
            unique.len(), uniqueness * 100.0, ((n as f64 / unique.len().max(1) as f64).ceil() as usize).max(2)
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
        score: score.min(8),
        max: 8,
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

/// Weight 8 — b-roll motion: fraction of frames with non-zero motion
/// and longest static run in the rendered video. Catches the
/// source-exhaustion bug where `movie= ... :si=0` holds the last frame
/// of a short Pexels source for the remainder of the overlay window
/// (typically 8–13s of static image per b-roll clip).
///
/// Inputs are pre-computed by the caller via ffmpeg `metadata=print`
/// (one ffmpeg pass, no per-frame PSNR). When `broll_motion_ratio` is
/// `None` (verifier never probed the rendered output, or video has no
/// b-roll), this dimension returns `score=max` so it never punishes a
/// non-b-roll video.
fn score_broll_motion(
    broll_motion_ratio: Option<f64>,
    longest_static_run_s: Option<f64>,
    has_broll: bool,
    broll_gaps: &[BrollGap],
) -> DimensionScore {
    let mut findings = Vec::new();
    // Coverage gaps (clip shorter than segment window) are always a hard
    // fail regardless of motion probing: the renderer no longer loops to
    // fill them, so the tail of the window holds the last frame. The agent
    // must re-run keyword generation for a longer clip.
    for g in broll_gaps {
        findings.push(format!(
            "COVERAGE HARD: segment {} needs {:.1}s but clip {} provides {:.1}s (gap {:.1}s) — {}",
            g.segment_id, g.required_s, g.asset_id, g.available_s, g.gap_s, g.action
        ));
    }
    // If the caller didn't probe motion, return neutral — don't punish
    // a video that wasn't verified.
    let (Some(ratio), Some(longest_run)) = (broll_motion_ratio, longest_static_run_s) else {
        let score = 8i32.saturating_sub(2 * broll_gaps.len() as i32).max(0);
        return DimensionScore {
            id: "broll_motion".into(),
            label: "B-roll motion / anti-static".into(),
            score,
            max: 8,
            detail: serde_json::json!({
                "probed": false,
                "reason": "no motion probe on rendered output",
                "broll_gap_count": broll_gaps.len(),
            }),
            findings,
        };
    };
    // Pure-dialogue or static-only videos (no b-roll in the manifest)
    // should also pass with full score, unless coverage gaps exist (gaps
    // imply b-roll is present but too short).
    if !has_broll && broll_gaps.is_empty() {
        return DimensionScore {
            id: "broll_motion".into(),
            label: "B-roll motion / anti-static".into(),
            score: 8,
            max: 8,
            detail: serde_json::json!({
                "probed": true,
                "has_broll": false,
                "motion_ratio": ratio,
                "longest_static_run_s": longest_run,
            }),
            findings,
        };
    }
    // Score formula: linearly reward high motion ratio, linearly
    // penalize long static runs.
    //
    //   ratio_pts  = clamp01(ratio / 0.50) * 4          // up to 4 pts
    //   run_pts    = clamp01((3.0 - longest_run) / 1.5) * 4  // up to 4 pts
    //
    // ratio_pts reaches max when >= 50% of frames have non-zero motion.
    // run_pts reaches max when longest static run <= 1.5s. The signature
    // of the source-exhaustion bug is a static run of 8–13s, which
    // collapses run_pts to 0.
    let ratio_pts = ((ratio / 0.50).clamp(0.0, 1.0) * 4.0) as i32;
    let run_pts = (((3.0 - longest_run) / 1.5).clamp(0.0, 1.0) * 4.0) as i32;
    // Each coverage gap subtracts 2 points (a segment whose clip ended early
    // holds the last frame — a visible quality break the agent must fix).
    let score = ((ratio_pts + run_pts).min(8))
        .saturating_sub(2 * broll_gaps.len() as i32)
        .max(0);
    if ratio < 0.30 {
        findings.push(format!(
            "MOTION HARD: only {}% of frames have non-zero motion (target >= 50%)",
            (ratio * 100.0).round() as i32
        ));
    }
    if longest_run > 3.0 {
        findings.push(format!(
            "STATIC HARD: longest static run {}s (target <= 1.5s) — likely source exhaustion",
            (longest_run * 10.0).round() as i32 / 10
        ));
    }
    DimensionScore {
        id: "broll_motion".into(),
        label: "B-roll motion / anti-static".into(),
        score,
        max: 8,
        detail: serde_json::json!({
            "probed": true,
            "has_broll": has_broll,
            "motion_ratio": (ratio * 1000.0).round() / 1000.0,
            "longest_static_run_s": (longest_run * 100.0).round() / 100.0,
            "ratio_pts": ratio_pts,
            "run_pts": run_pts,
            "broll_gap_count": broll_gaps.len(),
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
        8
    } else if avg >= 0.08 {
        6
    } else if avg >= 0.04 {
        4
    } else if !kw.is_empty() {
        findings.push(
            "search queries weakly aligned with video_keywords / scene text — diversify per-scene queries"
                .into(),
        );
        3
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
        max: 8,
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
        5
    } else if (0.08..0.12).contains(&cps) || (0.55..0.75).contains(&cps) {
        findings.push(format!(
            "cuts_per_second={:.2} slightly outside ideal 0.12–0.55 band",
            cps
        ));
        3
    } else if cps < 0.08 {
        findings.push(format!(
            "cuts_per_second={:.2} too static (want ≥0.12)",
            cps
        ));
        2
    } else {
        findings.push(format!(
            "cuts_per_second={:.2} too rapid (want ≤0.55)",
            cps
        ));
        2
    };

    (
        DimensionScore {
            id: "cuts_pacing".into(),
            label: "Cuts / visual pacing".into(),
            score,
            max: 5,
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

/// SEGMENTATION_ARCHITECTURE.md §3 enforcement dimension: scores how well the
/// timeline's segment durations respect the short-form bounds
/// (MIN_SEGMENT_DURATION_S=2.0s … MAX_SEGMENT_DURATION_S=6.0s). Long cuts
/// bleed viewer attention; sub-min cuts flicker. Full marks when every
/// segment is inside [2.0s, 6.0s] and the mean sits near the 4.0s target.
pub fn score_segmentation_pacing(timeline: &Timeline) -> DimensionScore {
    let mut findings = Vec::new();
    let segs = &timeline.segments;
    if segs.is_empty() {
        return DimensionScore {
            id: "segmentation_pacing".into(),
            label: "Segment duration pacing".into(),
            score: 0,
            max: 8,
            detail: serde_json::json!({}),
            findings: vec!["no segments — segmentation missing".into()],
        };
    }

    let mut over_max = 0usize;
    let mut under_min = 0usize;
    let mut sum_s = 0.0f64;
    let mut max_dur = 0.0f64;
    for seg in segs {
        let d = seg.end - seg.start;
        sum_s += d;
        max_dur = max_dur.max(d);
        if d > MAX_SEGMENT_DURATION_S + 1e-9 {
            over_max += 1;
        } else if d < MIN_SEGMENT_DURATION_S - 1e-9 {
            under_min += 1;
        }
    }
    let mean_s = sum_s / segs.len() as f64;

    // Base score: 8 pts, minus 2 per out-of-bounds segment (capped at 0), and
    // minus 2 when the mean drifts >1s from the 4.0s target.
    let mut score = 8i32 - 2 * (over_max + under_min) as i32;
    if !(3.0..=5.0).contains(&mean_s) {
        score -= 2;
    }
    let score = score.max(0);

    if over_max > 0 {
        findings.push(format!(
            "SEGMENTATION: {over_max} segment(s) exceed the {:.0}s short-form maximum (longest {:.1}s) — long cuts bleed attention; split at the longest internal pause",
            MAX_SEGMENT_DURATION_S, max_dur
        ));
    }
    if under_min > 0 {
        findings.push(format!(
            "SEGMENTATION: {under_min} segment(s) below the {:.0}s minimum — flicker risk; merge with adjacent",
            MIN_SEGMENT_DURATION_S
        ));
    }
    if over_max == 0 && under_min == 0 && !(3.0..=5.0).contains(&mean_s) {
        findings.push(format!(
            "segments within bounds but mean {:.1}s drifts from the 4.0s short-form target",
            mean_s
        ));
    }
    if over_max == 0 && under_min == 0 && score == 8 {
        findings.push(format!(
            "all {} segments within [{:.0}s, {:.0}s], mean {:.1}s — ideal short-form pacing",
            segs.len(), MIN_SEGMENT_DURATION_S, MAX_SEGMENT_DURATION_S, mean_s
        ));
    }

    DimensionScore {
        id: "segmentation_pacing".into(),
        label: "Segment duration pacing".into(),
        score,
        max: 8,
        detail: serde_json::json!({
            "segment_count": segs.len(),
            "mean_duration_s": (mean_s * 100.0).round() / 100.0,
            "longest_duration_s": (max_dur * 100.0).round() / 100.0,
            "over_max_count": over_max,
            "under_min_count": under_min,
            "min_duration_s": MIN_SEGMENT_DURATION_S,
            "max_duration_s": MAX_SEGMENT_DURATION_S,
            "target_duration_s": 4.0,
        }),
        findings,
    }
}

/// Weight 8 — music bed quality: presence, non-synthetic, topic fit,
/// ducking, gain compliance, mood/energy tags, source provider.
fn score_music_quality(
    music: Option<&MusicLayerInfo>,
    theme: Option<&str>,
    video_keywords: &[String],
) -> DimensionScore {
    let mut findings = Vec::new();
    let calm_ctx = is_calm_focus_context(theme, video_keywords);
    let score = match music {
        None => {
            findings.push("HARD: no background music bed".into());
            0
        }
        Some(m) if is_synthetic_music_file(&m.path) => {
            findings.push(
                "HARD: music is synthetic sine-wave placeholder (mcp/assets/music stock)".into(),
            );
            0
        }
        Some(m) if !Path::new(&m.path).exists() => {
            findings.push(format!("HARD: music path missing on disk: {}", m.path));
            0
        }
        Some(m)
            if calm_ctx
                && music_hits_denylist(
                    &m.path,
                    m.mood.as_deref(),
                    &m.tags,
                    m.selection_query.as_deref(),
                ) =>
        {
            findings.push(
                "HARD: music topic mismatch — parade/march/hype bed on calm/focus content".into(),
            );
            0
        }
        Some(m) => {
            let mut s = 3; // base for real, present, non-synthetic music

            if m.ducking {
                s += 1;
            } else {
                findings.push("music ducking disabled — will fight dialogue during speech".into());
            }

            // Gain sweet spot: -18 to -6 dB
            if (-18.0..=-6.0).contains(&m.gain_db) {
                s += 1;
            } else if m.gain_db > 0.0 {
                findings.push(format!(
                    "music gain_db={:.1} is boosted above unity — louder than voice; use -8 to -14 dB",
                    m.gain_db
                ));
            } else if m.gain_db < -24.0 {
                findings.push(format!(
                    "music gain_db={:.1} may be inaudible; target -12 to -8 dB",
                    m.gain_db
                ));
            }

            if m.mood.as_ref().map(|x| !x.is_empty()).unwrap_or(false) {
                s += 1;
            } else {
                findings.push("music.mood not tagged — library.search uses mood for curation".into());
            }

            if m.energy.as_ref().map(|x| !x.is_empty()).unwrap_or(false)
                || !m.tags.is_empty()
            {
                s += 1;
            } else {
                findings.push("music.energy and tags both empty — reduces topic-fit scoring".into());
            }

            if m.source.as_ref().map(|s| s != "unknown" && !s.is_empty()).unwrap_or(false) {
                s += 1;
            }

            s.min(8)
        }
    };
    DimensionScore {
        id: "music_quality".into(),
        label: "BG music quality & topic fit".into(),
        score,
        max: 8,
        detail: serde_json::json!({ "music": music, "calm_focus_context": calm_ctx }),
        findings,
    }
}

/// Weight 6 — SFX punctuation presence, variety, and gain compliance.
#[allow(dead_code)]
fn score_sfx_quality(sfx_count: usize, timeline: &Timeline) -> DimensionScore {
    let mut findings = Vec::new();

    let sfx_events: Vec<_> = timeline
        .tracks
        .get(&TrackType::Sfx)
        .cloned()
        .unwrap_or_default();

    if sfx_count == 0 && sfx_events.is_empty() {
        findings.push(
            "HARD: no SFX at any transition — add whoosh/pop/riser via sfx.assign".into(),
        );
        return DimensionScore {
            id: "sfx_quality".into(),
            label: "SFX punctuation & variety".into(),
            score: 0,
            max: 6,
            detail: serde_json::json!({ "sfx_count": 0, "unique_assets": 0 }),
            findings,
        };
    }

    let mut s = 2; // base for having any SFX

    let unique_sfx: HashSet<_> = sfx_events.iter().map(|e| e.asset_id.as_str()).collect();
    let unique_count = unique_sfx.len().max(sfx_count.min(1));

    if unique_count >= 3 {
        s += 2;
    } else if unique_count >= 2 {
        s += 1;
    } else {
        findings.push(format!(
            "repetitive sfx: only {} unique asset(s) — rotate through >=3 different sounds",
            unique_count
        ));
    }

    let mut asset_counts: HashMap<&str, usize> = HashMap::new();
    for e in &sfx_events {
        *asset_counts.entry(e.asset_id.as_str()).or_insert(0) += 1;
    }
    let max_repeat = asset_counts.values().copied().max().unwrap_or(0);
    if max_repeat > 2 {
        findings.push(format!(
            "sfx asset repeated {}x — a real editor rotates different SFX per transition",
            max_repeat
        ));
        s = (s - 1).max(0);
    }

    let mut gain_violations = 0usize;
    for e in &sfx_events {
        if let EventKind::Sfx { recommended_gain_db, .. } = &e.kind {
            if *recommended_gain_db > -3.0 || *recommended_gain_db < -20.0 {
                gain_violations += 1;
            }
        }
    }
    if gain_violations > 0 {
        findings.push(format!(
            "{} sfx event(s) with gain outside -20 to -3 dB — risk of clipping or inaudibility",
            gain_violations
        ));
    } else if !sfx_events.is_empty() {
        s += 1;
    }

    let coverage = (sfx_count.max(sfx_events.len()) >= 2) as i32;
    s += coverage;

    DimensionScore {
        id: "sfx_quality".into(),
        label: "SFX punctuation & variety".into(),
        score: s.min(6),
        max: 6,
        detail: serde_json::json!({
            "sfx_count": sfx_count,
            "timeline_sfx_events": sfx_events.len(),
            "unique_assets": unique_count,
            "max_repeat": max_repeat,
            "gain_violations": gain_violations,
        }),
        findings,
    }
}

/// Weight 8 — sticker design (full analysis with duration context).
fn score_sticker_design_with_duration(
    stickers: &[StickerLayerInfo],
    duration_ms: i64,
    caption_style: Option<&str>,
) -> DimensionScore {
    let mut findings = Vec::new();
    if stickers.is_empty() {
        findings.push("no stickers/GIFs composited".into());
        return DimensionScore {
            id: "sticker_design".into(),
            label: "Sticker design principles".into(),
            score: 0,
            max: 8,
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
    let mut off_screen = 0usize;

    for st in stickers {
        if (0.20..=0.45).contains(&st.scale) {
            scale_ok += 1;
        } else {
            findings.push(format!("sticker scale={:.2} outside design band 0.20-0.45", st.scale));
        }
        let pos_lower = st.position.to_lowercase();
        if pos_lower.is_empty() || pos_lower == "off" || pos_lower == "hidden" || pos_lower == "none" {
            off_screen += 1;
            findings.push(format!(
                "sticker position '{}' is off-screen or undefined — set a valid position",
                st.position
            ));
        } else if pos_lower.contains("bottom") {
            pos_risk += 1;
            findings.push(format!("sticker position '{}' may collide with caption rail", st.position));
        }
        if st.path.ends_with(".gif") || st.path.ends_with(".webp") {
            animated += 1;
        }
    }

    if scale_ok * 2 >= stickers.len() { s += 2; }
    if pos_risk == 0 && off_screen == 0 {
        s += 2;
    } else if off_screen > 0 {
        s = (s - 1).max(0);
    }
    if animated > 0 {
        s += 1;
    } else {
        findings.push("no animated GIF stickers — static PNG only reduces visual energy".into());
    }

    // Spatial overlap check: sticker vs caption safe zone
    // Use a standard 1080x1920 canvas for the check
    const CANVAS_W: u32 = 1080;
    const CANVAS_H: u32 = 1920;
    let caption_zone = caption_safe_zone(CANVAS_H, caption_style);
    for st in stickers {
        let sticker_box = sticker_bbox(st, CANVAS_W, CANVAS_H);
        let caption_box = (0, caption_zone.0, CANVAS_W as i32, caption_zone.1);
        if boxes_overlap(sticker_box, caption_box) {
            findings.push(format!(
                "HARD: sticker '{}' at position '{}' scale={:.2} overlaps caption safe zone ({}–{}px from top)",
                st.path.split('/').next_back().unwrap_or(&st.path),
                st.position,
                st.scale,
                caption_zone.0,
                caption_zone.1
            ));
            s = (s - 3).max(0);
        }
    }

    // Temporal overlap check
    let mut overlap_pairs = 0usize;
    for i in 0..stickers.len() {
        for j in (i + 1)..stickers.len() {
            let a = &stickers[i];
            let b = &stickers[j];
            let overlap = a.end_ms.min(b.end_ms) - a.start_ms.max(b.start_ms);
            if overlap > 500 {
                overlap_pairs += 1;
            }
        }
    }
    if overlap_pairs > 0 {
        findings.push(format!(
            "{} sticker pair(s) overlap >500ms simultaneously — competing for attention",
            overlap_pairs
        ));
        s = (s - 1).max(0);
    }

    // Always-on: sticker spanning >=90% of video
    if duration_ms > 0 {
        for st in stickers {
            let span = st.end_ms - st.start_ms;
            if span as f64 >= duration_ms as f64 * 0.90 {
                findings.push(format!(
                    "sticker '{}' is always-on ({:.0}% of video) — dynamic placement increases engagement",
                    st.path.split('/').next_back().unwrap_or(&st.path),
                    span as f64 / duration_ms as f64 * 100.0
                ));
                break;
            }
        }
    }

    DimensionScore {
        id: "sticker_design".into(),
        label: "Sticker design principles".into(),
        score: s.min(8),
        max: 8,
        detail: serde_json::json!({
            "sticker_count": stickers.len(),
            "unique_assets": unique.len(),
            "animated_count": animated,
            "scale_ok_count": scale_ok,
            "bottom_position_risk": pos_risk,
            "off_screen_count": off_screen,
            "overlap_pairs": overlap_pairs,
            "design_band_scale": [0.20, 0.45],
        }),
        findings,
    }
}

/// Backward-compat wrapper — no duration = no always-on check.
#[allow(dead_code)]
fn score_sticker_design(stickers: &[StickerLayerInfo]) -> DimensionScore {
    score_sticker_design_with_duration(stickers, 0, None)
}

/// Weight 8 — section composition.
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
            max: 8,
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
        score: s.min(8),
        max: 8,
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

/// Weight 8 — speech (legacy v3.0, replaced by score_voiceover_quality in v4.0).
#[allow(dead_code)]
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

/// Weight 6 — caption quality: presence, style, coverage, readability.
fn score_caption_quality(
    captions_path: Option<&str>,
    coverage_ratio: f64,
    style: Option<&str>,
    chars_per_second: Option<f64>,
    words_per_line: Option<f64>,
) -> DimensionScore {
    let mut findings = Vec::new();

    let present = captions_path
        .map(|p| !p.is_empty() && Path::new(p).exists())
        .unwrap_or(false);

    if !present {
        findings.push("HARD: captions file absent — word-highlight captions required for retention and accessibility".into());
        return DimensionScore {
            id: "caption_quality".into(),
            label: "Caption quality & coverage".into(),
            score: 0,
            max: 6,
            detail: serde_json::json!({ "present": false }),
            findings,
        };
    }

    let mut s = 1; // base for file existing

    // Coverage
    if coverage_ratio >= 0.90 {
        s += 2;
    } else if coverage_ratio >= 0.70 {
        s += 1;
        findings.push(format!(
            "caption coverage {:.0}% — target >=90% of speech duration",
            coverage_ratio * 100.0
        ));
    } else if coverage_ratio > 0.0 {
        findings.push(format!(
            "caption coverage {:.0}% is low — many speech segments uncaptioned",
            coverage_ratio * 100.0
        ));
    }

    // Reading speed
    if let Some(cps) = chars_per_second {
        if cps > 25.0 {
            findings.push(format!(
                "caption CPS={:.1} exceeds 25 — unreadable at normal viewing speed; target <=20 CPS",
                cps
            ));
            s = (s - 1).max(0);
        } else if cps <= 20.0 {
            s += 1;
        }
    }

    // Words per line
    if let Some(wpl) = words_per_line {
        if wpl > 5.0 {
            findings.push(format!(
                "caption avg {:.1} words/line — prefer <=4 words for short-form readability",
                wpl
            ));
        } else {
            s += 1;
        }
    }

    // Style
    match style {
        Some(st) if st == "word_highlight" || st == "karaoke" => { s += 1; }
        Some(st) if !st.is_empty() => { /* acceptable */ }
        _ => {
            findings.push("caption_style not set — prefer word_highlight for engagement".into());
        }
    }

    DimensionScore {
        id: "caption_quality".into(),
        label: "Caption quality & coverage".into(),
        score: s.min(6),
        max: 6,
        detail: serde_json::json!({
            "present": true,
            "captions_path": captions_path,
            "coverage_ratio": coverage_ratio,
            "style": style,
            "chars_per_second": chars_per_second,
            "words_per_line": words_per_line,
        }),
        findings,
    }
}

#[allow(dead_code)]
fn score_captions(captions_path: Option<&str>) -> DimensionScore {
    let cov = if captions_path.is_some() { 1.0 } else { 0.0 };
    score_caption_quality(captions_path, cov, None, None, None)
}

/// Weight 6 — voiceover quality: presence, WPM pacing, voice consistency, emote alignment.
#[allow(dead_code)]
fn score_voiceover_quality(
    has_dialogue: bool,
    voiceover_count: usize,
    wpm: Option<f64>,
    voice_ids: &[String],
    emote_alignment_ok: bool,
) -> DimensionScore {
    let mut findings = Vec::new();

    if !has_dialogue || voiceover_count == 0 {
        findings.push("no voiceover detected — add TTS via script.generate_voices".into());
        return DimensionScore {
            id: "voiceover_quality".into(),
            label: "Voiceover quality & pacing".into(),
            score: 0,
            max: 6,
            detail: serde_json::json!({ "has_dialogue": has_dialogue }),
            findings,
        };
    }

    let mut s = 2; // base for having voiceovers

    if let Some(w) = wpm {
        if (130.0..=160.0).contains(&w) {
            s += 2;
        } else if (110.0..=180.0).contains(&w) {
            s += 1;
            findings.push(format!("voiceover WPM={:.0} slightly outside ideal 130-160 band", w));
        } else if w > 180.0 {
            findings.push(format!(
                "voiceover WPM={:.0} too fast (>180) — listeners can't keep up; target 130-160", w
            ));
        } else {
            findings.push(format!(
                "voiceover WPM={:.0} too slow (<110) — loses audience; target 130-160", w
            ));
        }
    }

    let unique_voices: HashSet<_> = voice_ids.iter().collect();
    if voice_ids.is_empty() {
        findings.push("voice_ids not reported — cannot verify voice consistency".into());
    } else if unique_voices.len() < voice_ids.len() {
        findings.push("duplicate voice IDs across speakers — each speaker should have a unique voice".into());
    } else {
        s += 1;
    }

    if emote_alignment_ok {
        s += 1;
    } else {
        findings.push("emote tags not aligned to content — use generate_voices with emote hints for natural prosody".into());
    }

    DimensionScore {
        id: "voiceover_quality".into(),
        label: "Voiceover quality & pacing".into(),
        score: s.min(6),
        max: 6,
        detail: serde_json::json!({
            "has_dialogue": has_dialogue,
            "voiceover_count": voiceover_count,
            "wpm": wpm,
            "unique_voices": unique_voices.len(),
            "emote_alignment_ok": emote_alignment_ok,
        }),
        findings,
    }
}

/// Weight 5 — audio mix quality: LUFS compliance, clipping, ducking depth, gain balance.
/// Note: This function uses MEASURED values from the rendered video.
/// If measured values are not available (None), it will not hard-fail but will note the absence.
#[allow(dead_code)]
fn score_audio_mix_quality(
    measured_lufs: Option<f64>,
    measured_peak_dbfs: Option<f64>,
    measured_ducking_depth_db: Option<f64>,
    measured_music_gain_db: Option<f64>,
    has_dialogue: bool,
) -> DimensionScore {
    let mut findings = Vec::new();
    let mut s = 0i32;

    // Use measured values if available, fall back to manifest values
    let music_gain_db = measured_music_gain_db.unwrap_or(-12.0);
    let lufs = measured_lufs;
    let peak_dbfs = measured_peak_dbfs;
    let ducking_depth_db = measured_ducking_depth_db;

    // Music gain compliance
    if (-18.0..=-6.0).contains(&music_gain_db) {
        s += 1;
    } else {
        findings.push(format!("music gain_db={:.1} outside -18 to -6 dB safe band", music_gain_db));
    }

    // Peak level
    match peak_dbfs {
        Some(pk) if pk > -1.0 => {
            findings.push(format!(
                "HARD: audio clipping detected (peak={:.1} dBFS > -1 dBFS) — will distort on platforms",
                pk
            ));
        }
        Some(_) => { s += 1; }
        None => {
            findings.push("peak_dbfs not measured — run verify.render to check clipping".into());
        }
    }

    // LUFS compliance: -18 to -14 is the sweet spot
    match lufs {
        Some(l) if l > -14.0 => {
            findings.push(format!(
                "HARD: LUFS={:.1} exceeds -14 — too loud; normalize to -16 +/- 2",
                l
            ));
        }
        Some(l) if l < -18.0 => {
            findings.push(format!("LUFS={:.1} too quiet; target -16 +/- 2", l));
        }
        Some(_) => { s += 2; }
        None => {
            findings.push("lufs not measured — add loudnorm filter or run EBU R128 analysis".into());
        }
    }

    // Ducking effectiveness
    if has_dialogue {
        match ducking_depth_db {
            Some(d) if d >= 10.0 => { s += 1; }
            Some(d) if d >= 6.0 => {
                findings.push(format!("ducking depth {:.1} dB — target >=10 dB for clear speech", d));
            }
            Some(d) => {
                findings.push(format!(
                    "HARD: ducking depth {:.1} dB insufficient — music may mask speech (need >=6 dB)", d
                ));
                s = 0; // Hard fail
            }
            None => {
                findings.push("ducking_depth_db not measured — verify sidechain ducking is active".into());
            }
        }
    } else {
        s += 1; // no speech = no ducking needed
    }

    // Hard gate: no audio stream but music/SFX configured
    // Only trigger if we have SOME audio measurement AND they indicate silence
    // but the manifest says there should be audio
    let has_audio_measurement = measured_lufs.is_some() || measured_peak_dbfs.is_some() || measured_ducking_depth_db.is_some();
    let audio_appears_silent = measured_peak_dbfs.map(|p| p < -60.0).unwrap_or(false)
        && measured_lufs.map(|l| l < -40.0).unwrap_or(false);
    if has_audio_measurement && audio_appears_silent && (music_gain_db != 0.0 || has_dialogue) {
        findings.push("HARD: audio stream appears silent but music/SFX/dialogue configured".into());
        s = 0;
    }

    DimensionScore {
        id: "audio_mix_quality".into(),
        label: "Audio mix quality (LUFS, peak, ducking)".into(),
        score: s.min(5),
        max: 5,
        detail: serde_json::json!({
            "lufs": lufs,
            "peak_dbfs": peak_dbfs,
            "ducking_depth_db": ducking_depth_db,
            "music_gain_db": music_gain_db,
            "has_dialogue": has_dialogue,
        }),
        findings,
    }
}

/// Weight 5 — visual hierarchy: layered elements with clear focal points.
#[allow(dead_code)]
fn score_visual_hierarchy(
    stickers: &[StickerLayerInfo],
    memes: &[MemeLayerInfo],
    sections: &[SectionInfo],
    captions_present: bool,
) -> DimensionScore {
    let mut findings = Vec::new();
    let mut s = 0i32;

    let has_title_card = sections.iter().any(|sec| {
        sec.title_text.as_ref().map(|t| !t.is_empty()).unwrap_or(false)
    });
    if has_title_card { s += 1; }
    else { findings.push("no title cards — add title_text to hook/payoff sections".into()); }

    if !memes.is_empty() { s += 1; }
    else { findings.push("no reaction meme cuts — memes create motion hierarchy above static stickers".into()); }

    if !stickers.is_empty() { s += 1; }
    else { findings.push("no stickers — mid-level motion layer missing from visual hierarchy".into()); }

    if captions_present { s += 1; }
    else { findings.push("no captions — text anchor layer missing from visual hierarchy".into()); }

    let hook_has_visual = sections.iter().any(|sec| {
        matches!(sec.role, SectionRole::Hook) && sec.start_ms < 3000
    }) && (!stickers.is_empty() || has_title_card);
    if hook_has_visual { s += 1; }
    else { findings.push("hook lacks immediate visual element (sticker or title card in first 3s)".into()); }

    DimensionScore {
        id: "visual_hierarchy".into(),
        label: "Visual hierarchy (layers & focus)".into(),
        score: s.min(5),
        max: 5,
        detail: serde_json::json!({
            "has_title_card": has_title_card,
            "has_memes": !memes.is_empty(),
            "has_stickers": !stickers.is_empty(),
            "captions_present": captions_present,
            "hook_has_visual": hook_has_visual,
        }),
        findings,
    }
}

/// Weight 5 — platform optimization: aspect ratio, duration sweet spot.
#[allow(dead_code)]
fn score_platform_optimization(duration_ms: i64, aspect_ratio: Option<&str>) -> DimensionScore {
    let mut findings = Vec::new();
    let mut s = 0i32;

    match aspect_ratio {
        Some("9:16") => { s += 2; }
        Some("1:1") => {
            s += 1;
            findings.push("1:1 aspect — 9:16 vertical preferred for Shorts/Reels/TikTok".into());
        }
        Some(ar) => {
            findings.push(format!("aspect ratio '{}' not optimal — use 9:16", ar));
        }
        None => {
            s += 1; // assume correct if not reported
            findings.push("aspect_ratio not set in manifest — verify render config".into());
        }
    }

    let duration_s = duration_ms / 1000;
    if (15..=60).contains(&duration_s) {
        s += 2;
    } else if (60..=90).contains(&duration_s) {
        s += 1;
        findings.push(format!("duration {}s acceptable; 15-60s maximizes algorithm boost", duration_s));
    } else if duration_s < 15 {
        findings.push(format!("duration {}s too short — platform minimum ~15s", duration_s));
    } else {
        findings.push(format!("duration {}s exceeds 90s — keep short-form under 90s for retention", duration_s));
    }

    s += 1; // first-frame quality: award by default

    DimensionScore {
        id: "platform_optimization".into(),
        label: "Platform optimization (ratio, duration)".into(),
        score: s.min(5),
        max: 5,
        detail: serde_json::json!({
            "aspect_ratio": aspect_ratio,
            "duration_s": duration_s,
            "sweet_spot_s": [15, 60],
        }),
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
    } else if let Some(sfx_events) = timeline.tracks.get(&TrackType::Sfx) {
        // Detect SFX repetition: same asset_id used at multiple transitions
        // sounds amateur — a real editor rotates through different SFX.
        let mut asset_counts = std::collections::HashMap::new();
        for e in sfx_events {
            *asset_counts.entry(e.asset_id.clone()).or_insert(0) += 1;
        }
        let repeated: Vec<_> = asset_counts.iter().filter(|(_, &c)| c > 1).collect();
        if !repeated.is_empty() {
            let detail: Vec<_> = repeated
                .iter()
                .map(|(id, c)| format!("'{}' used {}x", id.split('/').next_back().unwrap_or(id), c))
                .collect();
            findings.push(format!("repetitive sfx: {}", detail.join(", ")));
        }
        // Penalize if fewer than 2 unique SFX assets across >4s video.
        let unique_sfx: std::collections::HashSet<_> = sfx_events.iter().map(|e| &e.asset_id).collect();
        if unique_sfx.len() < 2 && timeline.rendered_duration_ms() > 4000 {
            findings.push(format!(
                "only {} unique sfx asset(s) across {}s video — add variety",
                unique_sfx.len(),
                timeline.rendered_duration_ms() / 1000
            ));
        }
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
    // Penalize repetitive SFX: using the same sound at every transition
    // scores lower than rotating through different assets.
    if let Some(sfx_events) = timeline.tracks.get(&TrackType::Sfx) {
        let unique_sfx: std::collections::HashSet<_> = sfx_events.iter().map(|e| &e.asset_id).collect();
        if sfx_events.len() >= 3 && unique_sfx.len() <= 1 {
            findings.push("sfx repetitive — same sound at every transition, rotate assets".into());
            util = (util - 5).max(0);
        }
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
                    lexical_score: None,
                    source_title: None,
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
                tags: vec![],
                selection_query: None,
                source: None,
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

    let theme = manifest.theme.as_deref();
    let captions_present = captions_path
        .as_deref()
        .map(|p| !p.is_empty() && Path::new(p).exists())
        .unwrap_or(false);

    let d_source  = score_video_source(&backgrounds);
    let d_hooks   = score_visual_hooks(&backgrounds, duration_ms);
    let d_repeat  = score_visual_repetition(&backgrounds);
    let d_context = score_context_relevance(&backgrounds, &sections, &manifest.video_keywords);
    let (d_cuts, cps) = score_cuts_pacing(&backgrounds, duration_ms);
    let d_music   = score_music_quality(music.as_ref(), theme, &manifest.video_keywords);
    let d_sfx     = score_sfx_quality(manifest.sfx_count, timeline);
    let d_sticker = score_sticker_design_with_duration(&manifest.stickers, duration_ms, manifest.caption_style.as_deref());
    let d_cap     = score_caption_quality(
        captions_path.as_deref(),
        manifest.caption_coverage_ratio,
        manifest.caption_style.as_deref(),
        manifest.caption_chars_per_second,
        manifest.caption_words_per_line,
    );
    let d_vo      = score_voiceover_quality(
        manifest.has_dialogue,
        manifest.voiceover_count,
        manifest.voiceover_wpm,
        &manifest.voice_ids,
        manifest.emote_alignment_ok,
    );
    let d_audio   = score_audio_mix_quality(
        manifest.measured_lufs,
        manifest.measured_peak_dbfs,
        manifest.measured_ducking_depth_db,
        manifest.measured_music_gain_db,
        manifest.has_dialogue,
    );
    let d_section = score_section_composition(&sections, &manifest.memes);
    let d_hier    = score_visual_hierarchy(
        &manifest.stickers,
        &manifest.memes,
        &sections,
        captions_present,
    );
    let d_plat    = score_platform_optimization(duration_ms, manifest.aspect_ratio.as_deref());
    let d_seg     = score_segmentation_pacing(timeline);
    let timeline_editor = score_timeline_editor(timeline);

    // Verify layer composition order
    let layer_report = verify_layer_order(manifest);
    // Add layer order hard fails to overall hard fails
    let layer_hard_fails = layer_report.hard_fails.clone();
    let _layer_findings = layer_report.findings.clone();

    // Scale utilization 0-100 → 0-4 pts (down from 0-8 in v3)
    let editor_score = ((timeline_editor.utilization_score as f64) * 0.04).round() as i32;
    let d_editor = DimensionScore {
        id: "timeline_editor".into(),
        label: "Timeline editor efficacious use".into(),
        score: editor_score.min(4),
        max: 4,
        detail: serde_json::to_value(&timeline_editor).unwrap_or(serde_json::json!({})),
        findings: timeline_editor.findings.clone(),
    };

    // Compute broll_motion dimension using the manifest's pre-probed
    // motion ratio + longest static run. When the manifest was not
    // motion-probed (caller did not run probe_broll_motion), this
    // dimension returns score=max so it never punishes.
    let d_motion = score_broll_motion(
        manifest.broll_motion_ratio,
        manifest.longest_static_run_s,
        !backgrounds.is_empty(),
        &manifest.broll_gaps,
    );

    // v4.2: 10+8+8+8+5+8+6+8+6+6+5+8+5+5+4+8+8 = 116
    let dimensions = vec![
        d_source, d_hooks, d_repeat, d_context, d_cuts,
        d_music, d_sfx, d_sticker, d_cap, d_vo,
        d_audio, d_section, d_hier, d_plat, d_editor,
        d_motion, d_seg,
    ];

    let mut production_score: i32 =
        dimensions.iter().map(|d| d.score).sum::<i32>().clamp(0, 116);

    let mut hard_fails = Vec::new();
    let mut next_actions = Vec::new();
    for d in &dimensions {
        for f in &d.findings {
            if d.score == 0
                && matches!(
                    d.id.as_str(),
                    "video_source_quality"
                        | "visual_hooks"
                        | "visual_repetition"
                        | "music_quality"
                        | "sfx_quality"
                        | "speech_audio"
                        | "caption_quality"
                )
            {
                hard_fails.push(format!("{}: {}", d.id, f));
            }
            // Always surface REPETITION / HARD flags
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
                d.findings
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "see detail".into())
            ));
        }
    }

    // Add layer order hard fails
    for f in layer_hard_fails {
        if !hard_fails.contains(&f) {
            hard_fails.push(f);
        }
    }

    // v4.0 grade caps for new hard gates
    let sfx_hard = dimensions.iter()
        .find(|d| d.id == "sfx_quality")
        .map(|d| d.score == 0 && d.findings.iter().any(|f| f.contains("HARD")))
        .unwrap_or(false);
    let lufs_hard = dimensions.iter()
        .find(|d| d.id == "audio_mix_quality")
        .map(|d| d.findings.iter().any(|f| f.contains("LUFS") && f.contains("HARD")))
        .unwrap_or(false);
    let clip_hard = dimensions.iter()
        .find(|d| d.id == "audio_mix_quality")
        .map(|d| d.findings.iter().any(|f| f.contains("clipping") && f.contains("HARD")))
        .unwrap_or(false);
    let cap_cps_hard = dimensions.iter()
        .find(|d| d.id == "caption_quality")
        .map(|d| d.findings.iter().any(|f| f.contains("CPS") && f.contains("unreadable")))
        .unwrap_or(false);
    let ducking_hard = dimensions.iter()
        .find(|d| d.id == "audio_mix_quality")
        .map(|d| d.findings.iter().any(|f| f.contains("ducking depth") && f.contains("HARD")))
        .unwrap_or(false);
    let no_audio_hard = dimensions.iter()
        .find(|d| d.id == "audio_mix_quality")
        .map(|d| d.findings.iter().any(|f| f.contains("no audio stream") && f.contains("HARD")))
        .unwrap_or(false);

    if sfx_hard && production_score > 69 {
        production_score = 69;
        hard_fails.push("SFX hard gate: no SFX -> grade capped C".into());
    }
    if lufs_hard && production_score > 69 {
        production_score = 69;
        hard_fails.push("LUFS hard gate: loudness out of -14 to -18 range -> grade capped C".into());
    }
    if cap_cps_hard && production_score > 69 {
        production_score = 69;
        hard_fails.push("Caption hard gate: CPS > 25 (unreadable) -> grade capped C".into());
    }
    if clip_hard && production_score > 54 {
        production_score = 54;
        hard_fails.push("Clipping hard gate: peak > -1 dBFS -> grade capped D".into());
    }
    if ducking_hard && production_score > 54 {
        production_score = 54;
        hard_fails.push("Ducking hard gate: ducking depth < 6 dB -> grade capped D".into());
    }
    if no_audio_hard && production_score > 54 {
        production_score = 54;
        hard_fails.push("No audio hard gate: no audio stream but music/SFX configured -> grade capped D".into());
    }

    // Cap grade when hard fails present (fail closed on synthetic majority)
    let mut grade = production_grade(production_score).to_string();
    if !hard_fails.is_empty() {
        if production_score > 54 {
            production_score = 54;
        }
        grade = production_grade(production_score).to_string();
        if grade_rank(&grade) > grade_rank("D") {
            grade = "D".into();
            production_score = production_score.min(54);
        }
        next_actions.insert(
            0,
            "HARD FAIL: re-render with real stock + topic music (see hard_fails)".into(),
        );
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
        grade,
        dimensions,
        hard_fails,
        next_actions,
        timeline_editor,
        cuts_per_second: (cps * 1000.0).round() / 1000.0,
        video_source_mix: serde_json::Value::Object(mix),
        kpi_version: "4.2.0".into(),
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
                 lexical_score: None, source_title: None, },
                BackgroundLayerInfo {
                    path: "mcp/assets/backgrounds/procedural_02.mp4".into(),
                    start_ms: 8000,
                    end_ms: 16000,
                    source_hint: None,
                    content_hash: Some("proc2".into()),
                    video_id: None,
                    search_query: None,
                 lexical_score: None, source_title: None, },
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
        use crate::timeline::{EventKind, TimelineEvent};
        use crate::types::TrackType;
        let mut tl = empty_timeline();
        for (i, asset_id) in ["whoosh_a", "pop_b", "riser_c"].iter().enumerate() {
            tl.tracks.entry(TrackType::Sfx).or_default().push(TimelineEvent {
                id: format!("sfx_{}", i),
                asset_id: asset_id.to_string(),
                start_ms: (i as i64) * 6000,
                end_ms: (i as i64) * 6000 + 400,
                offset_ms: 0,
                gain_db: 0.0,
                fade_in_ms: 0,
                fade_out_ms: 0,
                tags: vec![],
                provenance: None,
                kind: EventKind::Sfx {
                    editorial_role: "transition".to_string(),
                    category: String::new(),
                    subcategory: String::new(),
                    duration_ms: 400,
                    sample_rate: 44100,
                    peak_db: -10.0,
                    loudness_lufs: -18.0,
                    recommended_gain_db: -10.0,
                    recommended_use: String::new(),
                    safe_overlay: true,
                },
            });
        }
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
                 lexical_score: None, source_title: None, })
                .collect(),
            stickers: vec![StickerLayerInfo {
                path: "mcp/assets/stickers/giphy_alice.gif".into(),
                start_ms: 0,
                end_ms: 5000,
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
                gain_db: -12.0,
                ducking: true,
                mood: Some("calm".into()),
                energy: Some("low".into()),
                tags: vec!["lofi".into()],
                selection_query: Some("lofi chill".into()),
                source: Some("library".into()),
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
            theme: Some("calm".into()),
            sfx_count: 3,
            caption_coverage_ratio: 0.95,
            caption_style: Some("word_highlight".into()),
            voiceover_wpm: Some(145.0),
            voice_ids: vec!["af_heart".into(), "bm_lewis".into()],
            emote_alignment_ok: true,
            aspect_ratio: Some("9:16".into()),
            ..Default::default()
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
             lexical_score: None, source_title: None, })
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
    fn segmentation_pacing_penalizes_long_and_short_cuts() {
        let mut tl = empty_timeline();
        // 12s cut — way over the 6s short-form max
        tl.segments.push(crate::timeline::Segment {
            id: "seg_001".into(),
            start: 0.0,
            end: 12.0,
            caption: "long".into(),
            crossfade_ms: 0,
            semantic_role: None,
        });
        // 0.5s cut — under the 2s min
        tl.segments.push(crate::timeline::Segment {
            id: "seg_002".into(),
            start: 12.0,
            end: 12.5,
            caption: "flicker".into(),
            crossfade_ms: 0,
            semantic_role: None,
        });
        let d = score_segmentation_pacing(&tl);
        assert!(d.score < 8, "long cuts must be penalized, got {}", d.score);
        assert!(
            d.findings.iter().any(|f| f.contains("SEGMENTATION") && f.contains("exceed")),
            "must flag the long cut: {:?}",
            d.findings
        );
        assert!(
            d.findings.iter().any(|f| f.contains("SEGMENTATION") && f.contains("below")),
            "must flag the short cut: {:?}",
            d.findings
        );
    }

    #[test]
    fn segmentation_pacing_full_marks_in_bounds() {
        let mut tl = empty_timeline();
        for (i, (s, e)) in [(0.0, 3.5), (3.5, 7.5), (7.5, 12.0)].iter().enumerate() {
            tl.segments.push(crate::timeline::Segment {
                id: format!("seg_{:03}", i + 1),
                start: *s,
                end: *e,
                caption: "ok".into(),
                crossfade_ms: 0,
                semantic_role: None,
            });
        }
        let d = score_segmentation_pacing(&tl);
        assert_eq!(d.score, 8, "in-bounds segments with 4s-ish mean get full marks, got {}", d.score);
        assert_eq!(d.max, 8);
    }

    #[test]
    fn anti_repeat_directive_mentions_refetch() {
        // 5 cuts drawn from only 2 distinct clips — the "same clip, different
        // zoom/pan" failure mode the agent must break by re-fetching distinct
        // footage (broll.fetch download_n).
        let bgs: Vec<BackgroundLayerInfo> = (0..5)
            .map(|i| BackgroundLayerInfo {
                path: format!("cache/clip_{}.mp4", i % 2),
                start_ms: i * 3000,
                end_ms: (i + 1) * 3000,
                source_hint: Some("pexels".into()),
                content_hash: Some(format!("hash_{}", i % 2)),
                video_id: Some(format!("vid{}", i % 2)),
                search_query: Some("query".into()),
                lexical_score: None,
                source_title: None,
            })
            .collect();
        let d = score_visual_repetition(&bgs);
        assert!(
            d.findings.iter().any(|f| f.contains("ANTI-REPEAT") && f.contains("download_n")),
            "must emit actionable ANTI-REPEAT with download_n directive: {:?}",
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
             lexical_score: None, source_title: None, })
            .collect();
        let d = score_visual_repetition(&bgs);
        assert!(d.score >= 8, "unique hashes should be high, got {}", d.score);
    }

    #[test]
    fn majority_procedural_hard_fails() {
        let tl = empty_timeline();
        let manifest = RenderManifest {
            duration_ms: 20000,
            backgrounds: vec![
                BackgroundLayerInfo {
                    path: "cache/yt_real.mp4".into(),
                    start_ms: 0,
                    end_ms: 4000,
                    source_hint: Some("youtube".into()),
                    content_hash: Some("real1".into()),
                    video_id: Some("vid1".into()),
                    search_query: Some("desk laptop".into()),
                    lexical_score: Some(0.4),
                    source_title: Some("Workspace laptop coffee".into()),
                },
                BackgroundLayerInfo {
                    path: "mcp/assets/backgrounds/procedural_01.mp4".into(),
                    start_ms: 4000,
                    end_ms: 8000,
                    content_hash: Some("p1".into()),
                    ..Default::default()
                },
                BackgroundLayerInfo {
                    path: "mcp/assets/backgrounds/procedural_02.mp4".into(),
                    start_ms: 8000,
                    end_ms: 12000,
                    content_hash: Some("p2".into()),
                    ..Default::default()
                },
                BackgroundLayerInfo {
                    path: "mcp/assets/backgrounds/procedural_03.mp4".into(),
                    start_ms: 12000,
                    end_ms: 16000,
                    content_hash: Some("p3".into()),
                    ..Default::default()
                },
                BackgroundLayerInfo {
                    path: "mcp/assets/backgrounds/procedural_04.mp4".into(),
                    start_ms: 16000,
                    end_ms: 20000,
                    content_hash: Some("p4".into()),
                    ..Default::default()
                },
            ],
            has_dialogue: true,
            rms_ok: true,
            captions_path: Some("captions.ass".into()),
            video_keywords: vec!["desk".into(), "focus".into()],
            theme: Some("calm".into()),
            ..Default::default()
        };
        let report = evaluate_production_quality(&tl, &manifest);
        assert!(
            !report.hard_fails.is_empty(),
            "majority procedural must hard-fail: {:?}",
            report.hard_fails
        );
        assert!(
            report.hard_fails.iter().any(|f| f.contains("majority procedural")
                || f.contains("HARD:")),
            "{:?}",
            report.hard_fails
        );
        assert!(
            grade_rank(&report.grade) <= grade_rank("D"),
            "grade capped D, got {}",
            report.grade
        );
    }

    #[test]
    fn parade_music_on_focus_hard_fails() {
        let tl = empty_timeline();
        let music = std::env::temp_dir().join("parade_bed.mp3");
        std::fs::write(&music, vec![3u8; 12000]).unwrap();
        let caps = std::env::temp_dir().join("pq_caps2.ass");
        std::fs::write(&caps, b"[Script Info]\n").unwrap();
        let manifest = RenderManifest {
            duration_ms: 10000,
            backgrounds: vec![BackgroundLayerInfo {
                path: "cache/yt_1.mp4".into(),
                start_ms: 0,
                end_ms: 10000,
                source_hint: Some("youtube".into()),
                content_hash: Some("h1".into()),
                search_query: Some("desk focus".into()),
                ..Default::default()
            }],
            music: Some(MusicLayerInfo {
                path: music.to_string_lossy().into(),
                gain_db: -10.0,
                ducking: true,
                mood: Some("energetic".into()),
                tags: vec!["parade".into(), "march".into()],
                selection_query: Some("upbeat parade march".into()),
                source: Some("youtube".into()),
                energy: Some("high".into()),
            }),
            has_dialogue: true,
            rms_ok: true,
            captions_path: Some(caps.to_string_lossy().into()),
            video_keywords: vec!["desk".into(), "focus".into()],
            theme: Some("calm".into()),
            ..Default::default()
        };
        let report = evaluate_production_quality(&tl, &manifest);
        assert!(
            report.hard_fails.iter().any(|f| f.contains("music") || f.contains("parade") || f.contains("mismatch")),
            "parade on focus must hard-fail: {:?}",
            report.hard_fails
        );
    }

    #[test]
    fn sfx_quality_no_sfx_scores_zero() {
        let tl = empty_timeline();
        let d = score_sfx_quality(0, &tl);
        assert_eq!(d.score, 0);
        assert_eq!(d.max, 6);
        assert!(d.findings.iter().any(|f| f.contains("sfx") || f.contains("SFX")));
    }

    #[test]
    fn sfx_quality_present_unique_scores_high() {
        use crate::timeline::{EventKind, TimelineEvent};
        use crate::types::TrackType;
        let mut tl = empty_timeline();
        for (i, asset_id) in ["whoosh_a", "pop_b"].iter().enumerate() {
            tl.tracks.entry(TrackType::Sfx).or_default().push(TimelineEvent {
                id: format!("sfx_{}", i),
                asset_id: asset_id.to_string(),
                start_ms: (i as i64) * 4000,
                end_ms: (i as i64) * 4000 + 500,
                offset_ms: 0,
                gain_db: 0.0,
                fade_in_ms: 0,
                fade_out_ms: 0,
                tags: vec![],
                provenance: None,
                kind: EventKind::Sfx {
                    editorial_role: "transition".to_string(),
                    category: String::new(),
                    subcategory: String::new(),
                    duration_ms: 400,
                    sample_rate: 44100,
                    peak_db: -10.0,
                    loudness_lufs: -18.0,
                    recommended_gain_db: -10.0,
                    recommended_use: String::new(),
                    safe_overlay: true,
                },
            });
        }
        let d = score_sfx_quality(2, &tl);
        assert!(d.score >= 4, "two unique SFX assets should score >=4, got {}", d.score);
    }

    #[test]
    fn sfx_quality_repeated_asset_penalized() {
        use crate::timeline::{EventKind, TimelineEvent};
        use crate::types::TrackType;
        let mut tl = empty_timeline();
        for i in 0..4 {
            tl.tracks.entry(TrackType::Sfx).or_default().push(TimelineEvent {
                id: format!("sfx_{}", i),
                asset_id: "whoosh_a".to_string(),
                start_ms: (i as i64) * 3000,
                end_ms: (i as i64) * 3000 + 400,
                offset_ms: 0,
                gain_db: 0.0,
                fade_in_ms: 0,
                fade_out_ms: 0,
                tags: vec![],
                provenance: None,
                kind: EventKind::Sfx {
                    editorial_role: "transition".to_string(),
                    category: String::new(),
                    subcategory: String::new(),
                    duration_ms: 400,
                    sample_rate: 44100,
                    peak_db: -10.0,
                    loudness_lufs: -18.0,
                    recommended_gain_db: -10.0,
                    recommended_use: String::new(),
                    safe_overlay: true,
                },
            });
        }
        let d = score_sfx_quality(4, &tl);
        assert!(d.findings.iter().any(|f| f.contains("repetitive") || f.contains("repeat")));
        assert!(d.score <= 4, "repetitive sfx should score <=4, got {}", d.score);
    }

    #[test]
    fn music_quality_no_music_scores_zero() {
        let d = score_music_quality(None, None, &[]);
        assert_eq!(d.id, "music_quality");
        assert_eq!(d.score, 0);
        assert_eq!(d.max, 8);
        assert!(d.findings.iter().any(|f| f.contains("HARD")));
    }

    #[test]
    fn music_quality_gain_too_loud_penalized() {
        std::fs::write("/tmp/test_music_loud.mp3", vec![4u8; 9000]).unwrap();
        let m = MusicLayerInfo {
            path: "/tmp/test_music_loud.mp3".to_string(),
            gain_db: 2.0,
            ducking: true,
            mood: Some("upbeat".into()),
            energy: Some("high".into()),
            tags: vec!["pop".into()],
            selection_query: Some("morning".into()),
            source: Some("pixabay".into()),
        };
        let d = score_music_quality(Some(&m), None, &[]);
        std::fs::remove_file("/tmp/test_music_loud.mp3").ok();
        assert!(d.findings.iter().any(|f| f.contains("gain") || f.contains("loud") || f.contains("unity")));
    }

    #[test]
    fn music_quality_sweet_spot_scores_high() {
        std::fs::write("/tmp/test_music_sweet.mp3", vec![5u8; 9000]).unwrap();
        let m = MusicLayerInfo {
            path: "/tmp/test_music_sweet.mp3".to_string(),
            gain_db: -12.0,
            ducking: true,
            mood: Some("calm".into()),
            energy: Some("low".into()),
            tags: vec!["lofi".into()],
            selection_query: Some("lofi chill".into()),
            source: Some("library".into()),
        };
        let d = score_music_quality(Some(&m), None, &[]);
        std::fs::remove_file("/tmp/test_music_sweet.mp3").ok();
        assert!(d.score >= 6, "sweet spot music should score >=6/8, got {}", d.score);
    }

    #[test]
    fn sticker_overlap_flagged() {
        let stickers = vec![
            StickerLayerInfo {
                path: "a.gif".into(), start_ms: 0, end_ms: 8000,
                position: "top-left".into(), scale: 0.35,
            },
            StickerLayerInfo {
                path: "b.gif".into(), start_ms: 3000, end_ms: 11000,
                position: "top-right".into(), scale: 0.30,
            },
        ];
        let d = score_sticker_design_with_duration(&stickers, 11000, None);
        assert!(
            d.findings.iter().any(|f| f.contains("overlap") || f.contains("compete")),
            "overlapping stickers should be flagged: {:?}", d.findings
        );
    }

    #[test]
    fn sticker_empty_position_flagged() {
        let stickers = vec![StickerLayerInfo {
            path: "a.gif".into(), start_ms: 0, end_ms: 5000,
            position: "".into(), scale: 0.30,
        }];
        let d = score_sticker_design_with_duration(&stickers, 5000, None);
        assert!(
            d.findings.iter().any(|f| f.contains("position") || f.contains("off-screen") || f.contains("undefined")),
            "empty position should be flagged: {:?}", d.findings
        );
    }

    #[test]
    fn sticker_always_on_flagged() {
        let stickers = vec![StickerLayerInfo {
            path: "a.gif".into(), start_ms: 0, end_ms: 20000,
            position: "top-left".into(), scale: 0.30,
        }];
        let d = score_sticker_design_with_duration(&stickers, 20000, None);
        assert!(
            d.findings.iter().any(|f| f.contains("always") || f.contains("whole video") || f.contains("100%")),
            "always-on sticker should be flagged: {:?}", d.findings
        );
    }

    #[test]
    fn caption_quality_missing_scores_zero() {
        let d = score_caption_quality(None, 0.0, None, None, None);
        assert_eq!(d.id, "caption_quality");
        assert_eq!(d.score, 0);
        assert!(d.findings.iter().any(|f| f.contains("absent") || f.contains("missing")));
    }

    #[test]
    fn caption_quality_low_coverage_penalized() {
        let path = "/tmp/cap_low_cov.ass";
        std::fs::write(path, b"[Script Info]\n").unwrap();
        let d = score_caption_quality(Some(path), 0.30, None, None, None);
        std::fs::remove_file(path).ok();
        assert!(d.findings.iter().any(|f| f.contains("coverage")));
        assert!(d.score <= 4, "30% coverage should score <=4, got {}", d.score);
    }

    #[test]
    fn caption_quality_fast_cps_penalized() {
        let path = "/tmp/cap_fast.ass";
        std::fs::write(path, b"[Script Info]\n").unwrap();
        let d = score_caption_quality(Some(path), 0.95, None, Some(30.0), None);
        std::fs::remove_file(path).ok();
        assert!(
            d.findings.iter().any(|f| f.contains("fast") || f.contains("CPS") || f.contains("unreadable")),
            "fast CPS should be flagged: {:?}", d.findings
        );
    }

    #[test]
    fn caption_quality_full_marks() {
        let path = "/tmp/cap_full.ass";
        std::fs::write(path, b"[Script Info]\n").unwrap();
        let d = score_caption_quality(
            Some(path), 0.95, Some("word_highlight"), Some(12.0), Some(2.5),
        );
        std::fs::remove_file(path).ok();
        assert!(d.score >= 5, "full marks caption should score >=5/6, got {}", d.score);
    }

    #[test]
    fn voiceover_quality_no_dialogue_scores_zero() {
        let d = score_voiceover_quality(false, 0, None, &[], false);
        assert_eq!(d.id, "voiceover_quality");
        assert_eq!(d.score, 0);
        assert_eq!(d.max, 6);
    }

    #[test]
    fn voiceover_quality_ideal_wpm_scores_high() {
        let d = score_voiceover_quality(
            true, 3, Some(145.0),
            &["af_heart".to_string(), "bm_lewis".to_string()],
            true,
        );
        assert!(d.score >= 5, "ideal voiceover should score >=5/6, got {}", d.score);
    }

    #[test]
    fn voiceover_quality_too_fast_penalized() {
        let d = score_voiceover_quality(
            true, 2, Some(220.0),
            &["af_heart".to_string()],
            false,
        );
        assert!(
            d.findings.iter().any(|f| f.contains("fast") || f.contains("WPM") || f.contains("wpm")),
            "fast WPM should be flagged: {:?}", d.findings
        );
        assert!(d.score <= 4, "fast WPM should score <=4, got {}", d.score);
    }

    #[test]
    fn audio_mix_no_data_partial_score() {
        let d = score_audio_mix_quality(None, None, None, None, true);
        assert_eq!(d.id, "audio_mix_quality");
        assert_eq!(d.max, 5);
        assert!(d.score <= 3);
    }

    #[test]
    fn audio_mix_clipping_penalized() {
        let d = score_audio_mix_quality(Some(-14.0), Some(0.5), Some(12.0), Some(-12.0), true);
        assert!(
            d.findings.iter().any(|f| f.contains("clip") || f.contains("peak")),
            "clipping should be flagged: {:?}", d.findings
        );
    }

    #[test]
    fn audio_mix_lufs_out_of_range_penalized() {
        let d = score_audio_mix_quality(Some(-6.0), Some(-2.0), Some(10.0), Some(-12.0), true);
        assert!(
            d.findings.iter().any(|f| f.contains("LUFS") || f.contains("loudness")),
            "LUFS violation should be flagged: {:?}", d.findings
        );
    }

    #[test]
    fn audio_mix_ideal_scores_high() {
        let d = score_audio_mix_quality(Some(-16.0), Some(-3.0), Some(14.0), Some(-12.0), true);
        assert!(d.score >= 4, "ideal audio mix should score >=4/5, got {}", d.score);
    }

    #[test]
    fn visual_hierarchy_no_elements_low_score() {
        let d = score_visual_hierarchy(&[], &[], &[], false);
        assert_eq!(d.id, "visual_hierarchy");
        assert_eq!(d.max, 5);
        assert!(d.score <= 1);
    }

    #[test]
    fn visual_hierarchy_full_stack_scores_high() {
        let stickers = vec![StickerLayerInfo {
            path: "a.gif".into(), start_ms: 0, end_ms: 5000,
            position: "top-left".into(), scale: 0.30,
        }];
        let memes = vec![MemeLayerInfo { path: "m.mp4".into(), start_ms: 5000, end_ms: 8000 }];
        let sections = vec![SectionInfo {
            role: SectionRole::Hook, start_ms: 0, end_ms: 5000,
            text: "Hook".into(), title_text: Some("BIG TITLE".into()),
        }];
        let d = score_visual_hierarchy(&stickers, &memes, &sections, true);
        assert!(d.score >= 4, "full stack should score >=4/5, got {}", d.score);
    }

    #[test]
    fn platform_opt_vertical_short_scores_high() {
        let d = score_platform_optimization(30000, Some("9:16"));
        assert_eq!(d.id, "platform_optimization");
        assert_eq!(d.max, 5);
        assert!(d.score >= 4, "30s vertical should score >=4/5, got {}", d.score);
    }

    #[test]
    fn platform_opt_landscape_too_long_penalized() {
        let d = score_platform_optimization(180000, Some("16:9"));
        assert!(d.findings.iter().any(|f| f.contains("9:16") || f.contains("aspect")));
        assert!(d.findings.iter().any(|f| f.contains("duration") || f.contains("long") || f.contains("90s")));
        assert!(d.score <= 2, "landscape+too-long should score <=2, got {}", d.score);
    }

    #[test]
    fn broll_motion_unprobed_scores_max() {
        // No probed data + no b-roll — should return score=max so it never
        // punishes a video that wasn't motion-verified.
        let d = score_broll_motion(None, None, false, &[]);
        assert_eq!(d.id, "broll_motion");
        assert_eq!(d.score, 8);
        assert_eq!(d.max, 8);
    }

    #[test]
    fn broll_motion_coverage_gap_is_hard_fail() {
        // A segment whose clip is shorter than the window must emit an
        // actionable COVERAGE HARD finding (the loop-closure signal), and
        // the score must drop even when motion probing looks healthy.
        let gap = BrollGap {
            segment_id: "broll_012".into(),
            concept: "city skyline".into(),
            asset_id: "broll_4".into(),
            asset_path: "/cache/x.mp4".into(),
            required_s: 4.0,
            available_s: 2.1,
            gap_s: 1.9,
            action: "re-run broll.keywords + broll.fetch for segment broll_012 — need clip >= 4.0s".into(),
        };
        let d = score_broll_motion(Some(0.80), Some(0.5), true, &[gap]);
        assert!(
            d.findings.iter().any(|f| f.contains("COVERAGE HARD") && f.contains("broll_012")),
            "gap must produce actionable finding: {:?}",
            d.findings
        );
        assert!(
            d.findings.iter().any(|f| f.contains("re-run broll.keywords")),
            "finding must include the re-run directive: {:?}",
            d.findings
        );
        assert!(d.score < 8, "coverage gap must reduce score, got {}", d.score);
    }

    #[test]
    fn broll_motion_healthy_clip_scores_high() {
        // 70% of frames have motion, longest static run is 0.5s — both
        // signals are well within healthy range.
        let d = score_broll_motion(Some(0.70), Some(0.5), true, &[]);
        assert_eq!(d.score, 8, "70% motion + 0.5s static run should score 8/8, got {} ({:?})", d.score, d.findings);
        assert!(d.findings.is_empty(), "healthy clip should have no findings, got {:?}", d.findings);
    }

    #[test]
    fn broll_motion_source_exhaustion_hard_fails() {
        // 20% of frames have motion + 9s longest static run — the exact
        // signature of the source-exhaustion bug (Phase 129 fix).
        let d = score_broll_motion(Some(0.20), Some(9.0), true, &[]);
        assert!(d.score <= 2, "20% motion + 9s static run should hard-fail, got {}", d.score);
        assert!(d.findings.iter().any(|f| f.contains("MOTION HARD")), "expected MOTION HARD finding, got {:?}", d.findings);
        assert!(d.findings.iter().any(|f| f.contains("STATIC HARD")), "expected STATIC HARD finding, got {:?}", d.findings);
    }

    #[test]
    fn broll_motion_no_broll_passes_even_when_static() {
        // Pure-dialogue video (no b-roll in manifest) with poor motion
        // should still pass — the dimension is gated on has_broll.
        let d = score_broll_motion(Some(0.10), Some(8.0), false, &[]);
        assert_eq!(d.score, 8);
    }
}







