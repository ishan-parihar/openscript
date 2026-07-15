//! Stock B-roll **signal vs noise** gates for multi-broll selection.
//!
//! Noise sources observed in production:
//! 1. **Query pollution** — dialogue fragments (`Swap one.`, `Phone later.`)
//!    leak into yt-dlp/Pexels searches and pull irrelevant viral clips.
//! 2. **Geometry distortion** — `scale=W:H,crop=W:H` stretches landscape
//!    sources into 9:16 with non-square SAR (looks squashed/stretched).
//! 3. **Uniqueness-only selection** — first unique ID wins even if the title
//!    has zero topical overlap with the scene.
//!
//! Pipeline:
//! ```text
//! scene text + video_keywords
//!   → build_scene_stock_query (sanitize + topic bias + visual anchor)
//!   → candidate search (title + id)
//!   → lexical_relevance gate (title vs signal tokens)
//!   → download + cover-crop (setsar=1, no stretch)
//!   → geometry_gate (pixel size + SAR ≈ 1 + target aspect)
//!   → optional vision_gate (when vision backend available)
//! ```

use serde_json::json;
use std::collections::HashSet;
use std::path::Path;
use std::process::Stdio;

// ---------------------------------------------------------------------------
// Token / query signal extraction
// ---------------------------------------------------------------------------

/// Words that are high-frequency dialogue glue or listicle noise — not visual.
const NOISE_TOKENS: &[&str] = &[
    // listicle / structure
    "swap", "one", "two", "three", "four", "five", "first", "second", "third",
    "later", "before", "after", "then", "next", "step", "tip", "habit", "habits",
    // generic verbs with no visual
    "starts", "start", "started", "make", "makes", "made", "try", "tries",
    "watch", "come", "comes", "back", "get", "gets", "got", "keep", "keeps",
    "open", "opens", "touch", "touches", "write", "writes", "check", "checking",
    // pronouns / fillers already partially stopped elsewhere
    "your", "you", "our", "their", "thing", "things", "whole", "single", "must",
    "exactly", "really", "just", "like", "also", "even", "still", "don", "doesn",
    "isn", "aren", "wasn", "won", "can", "cant", "dont",
    // meta
    "stock", "footage", "cinematic", "video", "clip", "free", "royalty",
];

/// Prefer concrete **shot** nouns when ranking tokens (not broad topic words
/// like "morning"/"routine" which match every lifestyle clip).
const VISUAL_BOOST: &[&str] = &[
    "phone", "lock", "screen", "coffee", "water", "glass", "light", "sunrise",
    "window", "desk", "notebook", "paper", "note", "pen", "music", "headphones",
    "kitchen", "bed", "bedroom", "alarm", "clock", "commute", "outdoor", "sun",
    "breakfast", "yoga", "stretch", "hand", "typing", "laptop",
    "message", "app", "scroll", "smartphone", "mug", "steam",
];

fn is_alpha_token(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphabetic())
}

/// Normalize a free-text blob into lowercased alphabetic tokens.
pub fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_ascii_alphabetic())
        .map(|s| s.to_ascii_lowercase())
        .filter(|s| s.len() >= 3 && is_alpha_token(s))
        .collect()
}

fn is_noise(tok: &str) -> bool {
    NOISE_TOKENS.contains(&tok)
}

/// Extract **visual signal** tokens from scene dialogue (not listicle noise).
///
/// **Scene-first:** concrete shot nouns from the spoken line outrank broad
/// `video_keywords`, so multi-broll queries differ per scene instead of all
/// collapsing to the same topic list.
pub fn signal_tokens_from_scene(scene_text: &str, video_keywords: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out: Vec<String> = Vec::new();

    // 1) Scene tokens (visual-boosted) — per-scene specificity
    let mut scene: Vec<String> = tokenize(scene_text)
        .into_iter()
        .filter(|t| !is_noise(t))
        .collect();
    scene.sort_by_key(|t| {
        if VISUAL_BOOST.contains(&t.as_str()) {
            0
        } else {
            1
        }
    });
    for t in scene {
        if seen.insert(t.clone()) {
            out.push(t);
        }
    }

    // 2) Topic keywords (whole-video context) — fill remaining slots
    for k in video_keywords {
        for t in tokenize(k) {
            if !is_noise(&t) && seen.insert(t.clone()) {
                out.push(t);
            }
        }
    }
    out
}

/// Visual anchors chosen by **matching** scene signal, not fixed rotation alone.
const ANCHOR_BANK: &[(&str, &[&str])] = &[
    (
        "sunrise window natural light bedroom",
        &["light", "sun", "window", "morning", "bed", "bedroom", "alarm", "wake"],
    ),
    (
        "coffee mug steam desk morning",
        &["coffee", "mug", "desk", "steam", "drink", "cup"],
    ),
    (
        "hand writing notebook paper daylight",
        &["write", "paper", "note", "notebook", "pen", "plan", "journal"],
    ),
    (
        "pouring water glass kitchen morning",
        &["water", "glass", "drink", "kitchen", "pour"],
    ),
    (
        "smartphone lock screen hands close up",
        &["phone", "lock", "screen", "scroll", "message", "app", "mobile"],
    ),
    (
        "headphones music listening daylight",
        &["music", "song", "headphones", "audio", "listen"],
    ),
    (
        "commute outdoor daylight walking",
        &["commute", "outdoor", "walk", "street", "outside", "presence"],
    ),
    (
        "yoga stretch morning home light",
        &["yoga", "stretch", "focus", "body", "breath"],
    ),
    (
        "healthy breakfast table natural light",
        &["breakfast", "food", "table", "eat", "meal"],
    ),
];

fn pick_visual_anchor(signal: &[String], scene_idx: usize) -> String {
    let set: HashSet<&str> = signal.iter().map(|s| s.as_str()).collect();
    // Weight concrete visual nouns higher than broad topic words ("morning").
    let mut best: Option<(i32, &str)> = None;
    for (anchor, keys) in ANCHOR_BANK {
        let mut score = 0i32;
        for k in *keys {
            if set.contains(k) {
                score += if VISUAL_BOOST.contains(k) { 4 } else { 1 };
            }
        }
        if score > 0 {
            match best {
                Some((s, _)) if s >= score => {}
                _ => best = Some((score, anchor)),
            }
        }
    }
    if let Some((_, a)) = best {
        return a.to_string();
    }
    // Fall back to rotated bank so multi-scene still diversifies
    ANCHOR_BANK[scene_idx % ANCHOR_BANK.len()].0.to_string()
}

/// Build a clean stock search query: signal tokens + theme + visual anchor + orientation bias.
pub fn build_scene_stock_query(
    scene_text: &str,
    video_keywords: &[String],
    theme: &str,
    aspect: &str,
    scene_idx: usize,
) -> SceneStockQuery {
    let signal = signal_tokens_from_scene(scene_text, video_keywords);
    // Cap query length — long queries confuse ytsearch
    let core: Vec<&str> = signal.iter().map(|s| s.as_str()).take(5).collect();
    let mut parts: Vec<String> = Vec::new();
    if !core.is_empty() {
        parts.push(core.join(" "));
    }
    // Theme mood (avoid double words)
    let theme_l = theme.to_ascii_lowercase();
    if theme_l == "calm" && !parts.iter().any(|p| p.contains("calm")) {
        parts.insert(0, "calm".into());
    } else if theme_l == "energetic" && !parts.iter().any(|p| p.contains("energetic")) {
        // Prefer "lifestyle" over "energetic" for stock search — less sports noise
        parts.push("lifestyle".into());
    }
    let anchor = pick_visual_anchor(&signal, scene_idx);
    parts.push(anchor.clone());
    // Orientation bias for vertical shorts
    if aspect == "9:16" || aspect.is_empty() {
        parts.push("vertical video".into());
    }
    // Always bias to stock-style footage, not vlogs with talking heads
    parts.push("stock footage b-roll".into());

    let query = parts
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    SceneStockQuery {
        query,
        signal_tokens: signal,
        visual_anchor: anchor,
        scene_idx,
    }
}

#[derive(Debug, Clone)]
pub struct SceneStockQuery {
    pub query: String,
    pub signal_tokens: Vec<String>,
    pub visual_anchor: String,
    pub scene_idx: usize,
}

// ---------------------------------------------------------------------------
// Lexical relevance (title / description vs signal)
// ---------------------------------------------------------------------------

/// Jaccard-like overlap + visual boost. Returns 0.0–1.0.
pub fn lexical_relevance(candidate_text: &str, signal: &[String]) -> f64 {
    if signal.is_empty() {
        return 0.5; // unknown — neutral
    }
    let cand: HashSet<String> = tokenize(candidate_text).into_iter().collect();
    if cand.is_empty() {
        return 0.0;
    }
    let mut hits = 0.0;
    let mut weight_sum = 0.0;
    for s in signal {
        let w = if VISUAL_BOOST.contains(&s.as_str()) {
            2.0
        } else {
            1.0
        };
        weight_sum += w;
        if cand.contains(s) {
            hits += w;
        }
    }
    if weight_sum <= 0.0 {
        return 0.0;
    }
    // Soften: partial credit if any visual boost hits
    let raw = hits / weight_sum;
    // Also credit token subset containment
    let signal_set: HashSet<&str> = signal.iter().map(|s| s.as_str()).collect();
    let overlap = cand.iter().filter(|c| signal_set.contains(c.as_str())).count();
    let soft = (overlap as f64 / signal.len().max(1) as f64).min(1.0);
    (0.65 * raw + 0.35 * soft).clamp(0.0, 1.0)
}

/// Minimum lexical score to accept a candidate title (reject pure noise).
pub fn min_lexical_accept() -> f64 {
    std::env::var("OPENSCRIPT_STOCK_MIN_LEXICAL")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.12)
}

// ---------------------------------------------------------------------------
// Geometry: cover-crop without stretch + post-validate
// ---------------------------------------------------------------------------

/// Cover-crop filter: fill target frame, center crop, **force square pixels**.
/// Fixes the stretch bug: old `scale=W:H,crop=W:H` kept landscape SAR and
/// displayed 16:9 content as distorted 9:16.
pub fn cover_crop_filter(width: u32, height: u32) -> String {
    format!(
        "scale={w}:{h}:force_original_aspect_ratio=increase,\
         crop={w}:{h},setsar=1,fps=30,format=yuv420p",
        w = width,
        h = height
    )
}

pub fn cover_crop_filter_for_aspect(aspect: &str) -> String {
    let (w, h) = match aspect {
        "16:9" => (1920, 1080),
        "1:1" => (1080, 1080),
        _ => (1080, 1920),
    };
    cover_crop_filter(w, h)
}

#[derive(Debug, Clone)]
pub struct GeometryReport {
    pub width: u32,
    pub height: u32,
    pub sar_num: u32,
    pub sar_den: u32,
    pub display_aspect: f64,
    pub ok: bool,
    pub reasons: Vec<String>,
}

/// Probe a clip with ffprobe and decide if geometry is clean for the target.
pub fn probe_geometry(path: &str, target_aspect: &str) -> GeometryReport {
    let mut report = GeometryReport {
        width: 0,
        height: 0,
        sar_num: 1,
        sar_den: 1,
        display_aspect: 0.0,
        ok: false,
        reasons: Vec::new(),
    };
    if !Path::new(path).exists() {
        report.reasons.push("file missing".into());
        return report;
    }
    let out = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,sample_aspect_ratio",
            "-of",
            "json",
            path,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    let Ok(o) = out else {
        report.reasons.push("ffprobe failed".into());
        return report;
    };
    let v: serde_json::Value = match serde_json::from_slice(&o.stdout) {
        Ok(v) => v,
        Err(_) => {
            report.reasons.push("ffprobe json parse failed".into());
            return report;
        }
    };
    let stream = v
        .pointer("/streams/0")
        .cloned()
        .unwrap_or(json!({}));
    report.width = stream.get("width").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
    report.height = stream
        .get("height")
        .and_then(|x| x.as_u64())
        .unwrap_or(0) as u32;
    let sar = stream
        .get("sample_aspect_ratio")
        .and_then(|x| x.as_str())
        .unwrap_or("1:1");
    let mut sar_parts = sar.split(':');
    report.sar_num = sar_parts
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1)
        .max(1);
    report.sar_den = sar_parts
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1)
        .max(1);

    if report.width < 480 || report.height < 480 {
        report.reasons.push(format!(
            "resolution too low {}x{}",
            report.width, report.height
        ));
    }

    // SAR must be ~1:1 after our crop (allow tiny rational noise)
    let sar_ratio = report.sar_num as f64 / report.sar_den as f64;
    if (sar_ratio - 1.0).abs() > 0.05 {
        report.reasons.push(format!(
            "non-square SAR {}:{} (display stretch risk)",
            report.sar_num, report.sar_den
        ));
    }

    let pix_aspect = if report.height > 0 {
        (report.width as f64 * sar_ratio) / report.height as f64
    } else {
        0.0
    };
    report.display_aspect = pix_aspect;

    let target = match target_aspect {
        "16:9" => 16.0 / 9.0,
        "1:1" => 1.0,
        _ => 9.0 / 16.0,
    };
    if (pix_aspect - target).abs() > 0.08 {
        report.reasons.push(format!(
            "display aspect {:.3} far from target {:.3}",
            pix_aspect, target
        ));
    }

    report.ok = report.reasons.is_empty();
    report
}

// ---------------------------------------------------------------------------
// Candidate ranking
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct RankedCandidate {
    pub id: String,
    pub title: String,
    pub lexical: f64,
}

/// Titles that are almost always **audio beds**, not visual stock footage.
/// Cold installs without Pexels used to rank "10 hours lofi focus music" as B-roll.
const BROLL_TITLE_DENY: &[&str] = &[
    "lofi", "lo-fi", "lo fi", "focus music", "study music", "sleep music",
    "relaxing music", "chill music", "ambient music", "background music",
    "no copyright music", "ncs", "hours of", "1 hour", "2 hour", "3 hour",
    "10 hour", "12 hour", "24 hour", "playlist", "mix music", "music mix",
    "beats to", "radio", "podcast", "audiobook", "asmr", "white noise",
    "rain sounds", "meditation music", "yoga music", "spa music",
    "copyright free music", "royalty free music only",
];

/// True when a YouTube title is almost certainly music/audio, not B-roll video.
pub fn is_broll_title_denylisted(title: &str) -> bool {
    let t = title.to_ascii_lowercase();
    if BROLL_TITLE_DENY.iter().any(|d| t.contains(d)) {
        return true;
    }
    // "music" without a visual cue → likely an audio stream
    if t.contains("music") {
        let visual = ["footage", "b-roll", "broll", "cinematic", "stock", "timelapse", "time-lapse", "drone", "city", "nature"];
        if !visual.iter().any(|v| t.contains(v)) {
            return true;
        }
    }
    false
}

/// Rank (id, title) pairs by lexical relevance; drop denylist + below threshold.
pub fn rank_and_filter_candidates(
    candidates: &[(String, String)],
    signal: &[String],
    min_score: f64,
) -> Vec<RankedCandidate> {
    let mut ranked: Vec<RankedCandidate> = candidates
        .iter()
        .filter(|(_, title)| !is_broll_title_denylisted(title))
        .map(|(id, title)| RankedCandidate {
            lexical: lexical_relevance(title, signal),
            id: id.clone(),
            title: title.clone(),
        })
        .collect();
    ranked.sort_by(|a, b| b.lexical.total_cmp(&a.lexical));
    // Hard gate: never accept lex≈0 "noise" titles. Empty → call site falls
    // back to procedural rather than shipping irrelevant stock.
    ranked
        .into_iter()
        .filter(|c| c.lexical >= min_score)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_listicle_noise_from_scene() {
        let sig = signal_tokens_from_scene(
            "Swap one. Light and water before the lock screen.",
            &["morning".into(), "phone".into()],
        );
        assert!(sig.contains(&"morning".into()) || sig.contains(&"phone".into()));
        assert!(!sig.iter().any(|t| t == "swap"));
        assert!(!sig.iter().any(|t| t == "one"));
        // visual nouns kept
        assert!(sig.iter().any(|t| t == "water" || t == "light" || t == "lock" || t == "screen" || t == "phone"));
    }

    #[test]
    fn denylists_music_titles_as_broll() {
        assert!(is_broll_title_denylisted("10 Hours Lofi Focus Music for Study"));
        assert!(is_broll_title_denylisted("Relaxing Music Playlist Chill Beats"));
        assert!(!is_broll_title_denylisted("Cinematic city drone stock footage vertical"));
        assert!(!is_broll_title_denylisted("Morning coffee desk typing b-roll"));
    }

    #[test]
    fn rank_filters_denylist_music() {
        let cands = vec![
            ("aaaaaa".into(), "10 hour lofi study music".into()),
            ("bbbbbb".into(), "coffee desk morning stock footage".into()),
        ];
        let ranked = rank_and_filter_candidates(&cands, &["coffee".into(), "desk".into()], 0.01);
        assert!(ranked.iter().all(|c| c.id != "aaaaaa"));
        assert!(ranked.iter().any(|c| c.id == "bbbbbb"));
    }

    #[test]
    fn query_does_not_contain_swap_fragments() {
        let q = build_scene_stock_query(
            "Swap two. One paper note. Write the single must-do.",
            &["morning".into(), "habits".into()],
            "energetic",
            "9:16",
            2,
        );
        let lower = q.query.to_ascii_lowercase();
        assert!(!lower.contains("swap"));
        assert!(lower.contains("morning") || lower.contains("paper") || lower.contains("notebook"));
        assert!(lower.contains("vertical") || lower.contains("stock"));
    }

    #[test]
    fn phone_scene_picks_phone_anchor() {
        let q = build_scene_stock_query(
            "If your morning starts with the phone, your whole day starts reactive.",
            &["morning".into(), "phone".into()],
            "energetic",
            "9:16",
            0,
        );
        assert!(
            q.visual_anchor.contains("smartphone") || q.visual_anchor.contains("phone"),
            "anchor={}",
            q.visual_anchor
        );
    }

    #[test]
    fn lexical_rejects_unrelated_title() {
        let signal = vec!["phone".into(), "morning".into(), "water".into()];
        let good = lexical_relevance("morning phone addiction stock footage vertical", &signal);
        let bad = lexical_relevance("minecraft parkour no copyright gameplay", &signal);
        assert!(good > bad);
        assert!(good >= 0.2);
        assert!(bad < 0.15);
    }

    #[test]
    fn cover_crop_sets_sar() {
        let f = cover_crop_filter(1080, 1920);
        assert!(f.contains("force_original_aspect_ratio=increase"));
        assert!(f.contains("setsar=1"));
        assert!(f.contains("crop=1080:1920"));
    }

    #[test]
    fn rank_prefers_relevant_titles() {
        let cands = vec![
            ("a".into(), "EPIC Minecraft Parkour Hours".into()),
            ("b".into(), "Morning coffee phone free stock footage".into()),
            ("c".into(), "Funny cat compilation".into()),
        ];
        let signal = vec!["morning".into(), "coffee".into(), "phone".into()];
        let ranked = rank_and_filter_candidates(&cands, &signal, 0.12);
        assert_eq!(ranked[0].id, "b");
    }

    #[test]
    fn rank_drops_all_noise_instead_of_accepting_zero() {
        let cands = vec![
            ("a".into(), "Everyone Mocked His Civilian Tech".into()),
            ("b".into(), "Funny cat compilation hours".into()),
        ];
        let signal = vec!["desk".into(), "laptop".into(), "coffee".into()];
        let ranked = rank_and_filter_candidates(&cands, &signal, 0.12);
        assert!(ranked.is_empty(), "expected empty, got {:?}", ranked);
    }

    #[test]
    fn scene_first_signal_differs_per_line() {
        let kw = vec!["desk".into(), "focus".into()];
        let a = signal_tokens_from_scene("Headphones on. One instrumental playlist.", &kw);
        let b = signal_tokens_from_scene("Notebook beside the coffee. Capture thoughts.", &kw);
        assert!(a.iter().any(|t| t == "headphones" || t == "playlist" || t == "instrumental"));
        assert!(b.iter().any(|t| t == "notebook" || t == "coffee"));
        // First tokens should be scene-specific, not only topic keywords
        assert_ne!(a.first(), b.first());
    }
}
