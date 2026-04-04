use std::fs;
use std::path::Path;

/// A single SRT entry.
#[derive(Debug, Clone)]
pub struct SrtEntry {
    pub idx: usize,
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// Parse an SRT file into entries. Handles both standard and word-per-line formats.
pub fn parse_srt<P: AsRef<Path>>(path: P) -> Result<Vec<SrtEntry>, SrtError> {
    let content = fs::read_to_string(path)?
        .replace("\r\n", "\n")
        .replace("\r", "\n");
    let mut entries = Vec::new();
    for block in content.split("\n\n").filter(|b| !b.trim().is_empty()) {
        let lines: Vec<&str> = block.lines().filter(|l| !l.trim().is_empty()).collect();
        if lines.len() < 2 {
            continue;
        }
        let mut ts_idx = 0;
        if lines[0].trim().parse::<usize>().is_ok() {
            ts_idx = 1;
        }
        if ts_idx >= lines.len() || !lines[ts_idx].contains("-->") {
            continue;
        }
        let parts: Vec<&str> = lines[ts_idx].splitn(2, "-->").collect();
        if parts.len() != 2 {
            continue;
        }
        let start = parse_timestamp(parts[0].trim())?;
        let end = parse_timestamp(parts[1].trim())?;
        let text = lines[ts_idx + 1..]
            .iter()
            .map(|l| l.trim())
            .collect::<Vec<_>>()
            .join(" ");
        entries.push(SrtEntry {
            idx: entries.len() + 1,
            start,
            end,
            text,
        });
    }
    Ok(entries)
}

fn parse_timestamp(ts: &str) -> Result<f64, SrtError> {
    let ts = ts.replace(",", ".");
    let parts: Vec<&str> = ts.split(':').collect();
    if parts.len() != 3 {
        return Err(SrtError::InvalidTimestamp(ts.into()));
    }
    let hh: f64 = parts[0].parse()?;
    let mm: f64 = parts[1].parse()?;
    let ss: f64 = parts[2].parse()?;
    Ok(hh * 3600.0 + mm * 60.0 + ss)
}

/// Group word-per-line SRT entries into phrase/sentence groups.
pub fn group_entries(
    entries: &[SrtEntry],
    max_words: usize,
    max_chars: usize,
    max_gap: f64,
) -> Vec<(String, f64, f64)> {
    let mut groups = Vec::new();
    let mut cur_words: Vec<String> = Vec::new();
    let mut cur_start: Option<f64> = None;
    let mut cur_end: Option<f64> = None;

    for e in entries {
        let w = e.text.trim();
        if w.is_empty() {
            continue;
        }
        if cur_start.is_none() {
            cur_start = Some(e.start);
            cur_end = Some(e.end);
            cur_words = vec![w.to_string()];
            continue;
        }
        let gap = e.start - cur_end.unwrap_or(e.start);
        let next_len = cur_words.join(" ").len() + 1 + w.len();
        let should_break = gap > max_gap || cur_words.len() >= max_words || next_len > max_chars;
        if should_break {
            groups.push((cur_words.join(" "), cur_start.unwrap(), cur_end.unwrap()));
            cur_start = Some(e.start);
            cur_end = Some(e.end);
            cur_words = vec![w.to_string()];
        } else {
            cur_words.push(w.to_string());
            cur_end = Some(e.end);
        }
    }
    if !cur_words.is_empty() {
        groups.push((cur_words.join(" "), cur_start.unwrap(), cur_end.unwrap()));
    }
    groups
}

/// Write grouped entries back to SRT format.
pub fn write_srt<P: AsRef<Path>>(
    groups: &[(String, f64, f64)],
    out_path: P,
) -> Result<(), SrtError> {
    let mut content = String::new();
    for (i, (text, start, end)) in groups.iter().enumerate() {
        content.push_str(&format!(
            "{}\n{} --> {}\n{}\n\n",
            i + 1,
            fmt_ts(*start),
            fmt_ts(*end),
            text
        ));
    }
    fs::write(out_path, content)?;
    Ok(())
}

fn fmt_ts(seconds: f64) -> String {
    let secs = if seconds < 0.0 { 0.0 } else { seconds };
    let ms = ((secs % 1.0) * 1000.0).round() as u32;
    let s_int = secs as u64;
    let hh = s_int / 3600;
    let mm = (s_int % 3600) / 60;
    let ss = s_int % 60;
    format!("{:02}:{:02}:{:02},{:03}", hh, mm, ss, ms)
}

/// Re-time SRT entries to match an EDL segment sequence.
pub fn retime_srt(
    srt_entries: &[(f64, f64, String)],
    edl_segments: &[(f64, f64)],
    gap_merge: f64,
) -> Vec<(f64, f64, String)> {
    let mut out = Vec::new();
    let mut t_out = 0.0;
    for (seg_start, seg_end) in edl_segments {
        let seg_dur = (seg_end - seg_start).max(0.0);
        if seg_dur <= 0.0 {
            continue;
        }
        for (st, et, txt) in srt_entries {
            if *et <= *seg_start || *st >= *seg_end {
                continue;
            }
            let clip_st = st.max(*seg_start);
            let clip_et = et.min(*seg_end);
            if clip_et <= clip_st {
                continue;
            }
            let new_st = t_out + (clip_st - seg_start);
            let new_et = t_out + (clip_et - seg_start);
            out.push((new_st, new_et, txt.clone()));
        }
        t_out += seg_dur;
    }
    if gap_merge > 0.0 && !out.is_empty() {
        let mut merged = Vec::new();
        let mut cur = out[0].clone();
        for (st, et, txt) in out.iter().skip(1) {
            if st - cur.1 <= gap_merge {
                cur.1 = cur.1.max(*et);
                cur.2 = format!("{} {}", cur.2, txt).trim().to_string();
            } else {
                merged.push(cur);
                cur = (*st, *et, txt.clone());
            }
        }
        merged.push(cur);
        return merged;
    }
    out
}

/// Default filler words for English.
const FILLER_WORDS: &[&str] = &[
    "um",
    "uh",
    "uhh",
    "umm",
    "er",
    "ah",
    "like",
    "you know",
    "i mean",
    "so",
    "basically",
    "literally",
    "actually",
    "right",
    "ok",
    "okay",
];

/// Default keyword categories.
const KEYWORD_CATEGORIES: &[(&str, &[&str])] = &[
    (
        "tech",
        &[
            "ai",
            "machine learning",
            "code",
            "software",
            "tech",
            "digital",
            "computer",
        ],
    ),
    (
        "business",
        &[
            "business", "money", "startup", "revenue", "profit", "growth", "market",
        ],
    ),
    (
        "health",
        &[
            "health", "fitness", "diet", "exercise", "wellness", "mental", "stress",
        ],
    ),
    (
        "education",
        &[
            "learn",
            "study",
            "teach",
            "school",
            "education",
            "knowledge",
            "skill",
        ],
    ),
    (
        "lifestyle",
        &[
            "life", "travel", "food", "family", "friend", "love", "happy",
        ],
    ),
];

/// Analysis result for a single SRT entry.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SrtAnalysisEntry {
    pub text: String,
    pub start: f64,
    pub end: f64,
    pub duration: f64,
    pub word_count: usize,
    pub filler_count: usize,
    pub filler_ratio: f64,
    pub keywords_found: Vec<String>,
    pub keywords_score: usize,
    pub keep: bool,
}

/// Analyze SRT entries for filler words and keywords.
pub fn analyze_srt(entries: &[(String, f64, f64)]) -> Vec<SrtAnalysisEntry> {
    let mut results = Vec::new();

    for (text, start, end) in entries {
        let duration = end - start;
        let text_lower = text.to_lowercase();
        let words: Vec<&str> = text_lower.split_whitespace().collect();
        let word_count = words.len();

        let filler_count = words.iter().filter(|w| FILLER_WORDS.contains(w)).count();
        let filler_ratio = if word_count > 0 {
            filler_count as f64 / word_count as f64
        } else {
            0.0
        };

        let mut keywords_found = Vec::new();
        for (_category, kws) in KEYWORD_CATEGORIES {
            for kw in *kws {
                if text_lower.contains(kw) && !keywords_found.contains(&kw.to_string()) {
                    keywords_found.push(kw.to_string());
                }
            }
        }
        let keywords_score = keywords_found.len();

        let keep = filler_ratio < 0.5 || keywords_score > 0;

        results.push(SrtAnalysisEntry {
            text: text.clone(),
            start: *start,
            end: *end,
            duration,
            word_count,
            filler_count,
            filler_ratio,
            keywords_found,
            keywords_score,
            keep,
        });
    }

    results
}

/// Build an EDL (Edit Decision List) from analyzed SRT entries.
///
/// Strategy "keep": Score all entries, sort by score desc (then duration asc),
/// greedily pack up to max_duration.
///
/// Strategy "remove": Keep only entries where keep == true, sequentially until max_duration.
pub fn build_edl(
    analysis: &[SrtAnalysisEntry],
    strategy: &str,
    max_duration: Option<f64>,
    crossfade_ms: u32,
) -> Vec<(f64, f64, String)> {
    let _ = crossfade_ms;
    let max_dur = max_duration.unwrap_or(f64::MAX);
    let mut total_duration = 0.0;
    let mut segments = Vec::new();

    match strategy {
        "keep" => {
            let mut scored: Vec<_> = analysis
                .iter()
                .map(|a| {
                    let score = a.keywords_score as f64 - a.filler_ratio;
                    (score, a.duration, a)
                })
                .collect();
            scored.sort_by(|a, b| {
                b.0.partial_cmp(&a.0)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            });

            for (_, _, entry) in scored {
                if total_duration + entry.duration > max_dur {
                    break;
                }
                segments.push((entry.start, entry.end, entry.text.clone()));
                total_duration += entry.duration;
            }
        }
        "remove" => {
            for entry in analysis {
                if !entry.keep {
                    continue;
                }
                if total_duration + entry.duration > max_dur {
                    break;
                }
                segments.push((entry.start, entry.end, entry.text.clone()));
                total_duration += entry.duration;
            }
        }
        _ => {
            for entry in analysis {
                if total_duration + entry.duration > max_dur {
                    break;
                }
                segments.push((entry.start, entry.end, entry.text.clone()));
                total_duration += entry.duration;
            }
        }
    }

    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_analyze_srt_empty() {
        let entries: Vec<(String, f64, f64)> = vec![];
        let result = analyze_srt(&entries);
        assert!(result.is_empty());
    }

    #[test]
    fn test_analyze_srt_filler_detection() {
        let entries = vec![("um uh hello world".to_string(), 0.0, 2.0)];
        let result = analyze_srt(&entries);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].filler_count, 2);
        assert_eq!(result[0].word_count, 4);
        assert!((result[0].filler_ratio - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_analyze_srt_keyword_detection() {
        let entries = vec![("ai and machine learning code".to_string(), 0.0, 3.0)];
        let result = analyze_srt(&entries);
        assert_eq!(result.len(), 1);
        assert!(result[0].keywords_score >= 2);
        assert!(result[0].keywords_found.contains(&"ai".to_string()));
        assert!(result[0]
            .keywords_found
            .contains(&"machine learning".to_string()));
        assert!(result[0].keep);
    }

    #[test]
    fn test_analyze_srt_keep_logic() {
        let entries = vec![
            ("um uh like basically".to_string(), 0.0, 2.0),
            ("software development is great".to_string(), 2.0, 5.0),
        ];
        let result = analyze_srt(&entries);
        assert!(!result[0].keep);
        assert!(result[1].keep);
    }

    #[test]
    fn test_build_edl_keep_strategy() {
        let analysis = vec![
            SrtAnalysisEntry {
                text: "hello world".to_string(),
                start: 0.0,
                end: 2.0,
                duration: 2.0,
                word_count: 2,
                filler_count: 0,
                filler_ratio: 0.0,
                keywords_found: vec!["ai".to_string()],
                keywords_score: 1,
                keep: true,
            },
            SrtAnalysisEntry {
                text: "um uh".to_string(),
                start: 2.0,
                end: 3.0,
                duration: 1.0,
                word_count: 2,
                filler_count: 2,
                filler_ratio: 1.0,
                keywords_found: vec![],
                keywords_score: 0,
                keep: false,
            },
        ];
        let segments = build_edl(&analysis, "keep", Some(5.0), 120);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].0, 0.0);
    }

    #[test]
    fn test_build_edl_remove_strategy() {
        let analysis = vec![
            SrtAnalysisEntry {
                text: "keep this".to_string(),
                start: 0.0,
                end: 2.0,
                duration: 2.0,
                word_count: 2,
                filler_count: 0,
                filler_ratio: 0.0,
                keywords_found: vec![],
                keywords_score: 0,
                keep: true,
            },
            SrtAnalysisEntry {
                text: "remove this".to_string(),
                start: 2.0,
                end: 4.0,
                duration: 2.0,
                word_count: 2,
                filler_count: 2,
                filler_ratio: 1.0,
                keywords_found: vec![],
                keywords_score: 0,
                keep: false,
            },
            SrtAnalysisEntry {
                text: "also keep".to_string(),
                start: 4.0,
                end: 6.0,
                duration: 2.0,
                word_count: 2,
                filler_count: 0,
                filler_ratio: 0.0,
                keywords_found: vec![],
                keywords_score: 0,
                keep: true,
            },
        ];
        let segments = build_edl(&analysis, "remove", Some(10.0), 120);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].2, "keep this");
        assert_eq!(segments[1].2, "also keep");
    }

    #[test]
    fn test_build_edl_max_duration() {
        let analysis = vec![
            SrtAnalysisEntry {
                text: "a".to_string(),
                start: 0.0,
                end: 3.0,
                duration: 3.0,
                word_count: 1,
                filler_count: 0,
                filler_ratio: 0.0,
                keywords_found: vec!["ai".to_string()],
                keywords_score: 1,
                keep: true,
            },
            SrtAnalysisEntry {
                text: "b".to_string(),
                start: 3.0,
                end: 6.0,
                duration: 3.0,
                word_count: 1,
                filler_count: 0,
                filler_ratio: 0.0,
                keywords_found: vec!["ai".to_string()],
                keywords_score: 1,
                keep: true,
            },
        ];
        let segments = build_edl(&analysis, "keep", Some(4.0), 120);
        assert_eq!(segments.len(), 1);
    }

    #[test]
    fn test_build_edl_default_strategy() {
        let analysis = vec![SrtAnalysisEntry {
            text: "entry one".to_string(),
            start: 0.0,
            end: 1.0,
            duration: 1.0,
            word_count: 2,
            filler_count: 2,
            filler_ratio: 1.0,
            keywords_found: vec![],
            keywords_score: 0,
            keep: false,
        }];
        let segments = build_edl(&analysis, "unknown", None, 120);
        assert_eq!(segments.len(), 1);
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SrtError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid timestamp: {0}")]
    InvalidTimestamp(String),
    #[error("Parse error: {0}")]
    Parse(#[from] std::num::ParseFloatError),
}
