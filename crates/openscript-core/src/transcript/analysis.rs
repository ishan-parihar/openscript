//! Transcript analysis: speaker detection, filler word identification, silence gaps.

use serde::{Deserialize, Serialize};

/// Common filler words/phrases to detect in transcripts.
const FILLER_WORDS: &[&str] = &[
    "um",
    "uh",
    "uhh",
    "umm",
    "er",
    "like",
    "you know",
    "basically",
    "actually",
    "literally",
    "so",
    "right",
    "okay",
    "well",
];

/// A detected filler word in the transcript.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FillerWord {
    pub text: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub segment_id: String,
}

/// Analysis result for a transcript.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptAnalysis {
    pub filler_words: Vec<FillerWord>,
    pub filler_word_count: usize,
    pub total_words: usize,
    pub filler_percentage: f64,
    pub word_count: u64,
    pub estimated_duration_s: f64,
    pub segments_analyzed: usize,
}

/// Detect filler words in caption text.
pub fn detect_filler_words(
    segments: &[(String, u64, u64, String)], // (segment_id, start_ms, end_ms, text)
) -> TranscriptAnalysis {
    let mut filler_words = Vec::new();
    let mut total_words = 0;

    for (segment_id, start_ms, _end_ms, text) in segments {
        let words: Vec<&str> = text.split_whitespace().collect();
        total_words += words.len();

        for word in &words {
            let lowered = word.to_lowercase();
            let lower = lowered.trim_matches(|c: char| !c.is_alphabetic());
            if FILLER_WORDS.contains(&lower) {
                filler_words.push(FillerWord {
                    text: word.to_string(),
                    start_ms: *start_ms,
                    end_ms: *start_ms + 500, // Approximate
                    segment_id: segment_id.clone(),
                });
            }
        }
    }

    let filler_percentage = if total_words > 0 {
        (filler_words.len() as f64 / total_words as f64) * 100.0
    } else {
        0.0
    };

    TranscriptAnalysis {
        filler_word_count: filler_words.len(),
        filler_words,
        total_words,
        filler_percentage,
        word_count: total_words as u64,
        estimated_duration_s: 0.0,
        segments_analyzed: segments.len(),
    }
}

/// Detect potential speaker changes based on text patterns.
/// This is a heuristic — true speaker detection requires audio analysis.
pub fn detect_speaker_changes(segments: &[(String, String)]) -> Vec<(String, String)> {
    segments
        .iter()
        .map(|(id, text)| {
            let trimmed = text.trim();
            if trimmed.starts_with('?') || trimmed.starts_with("Wait") || trimmed.starts_with("Hmm")
            {
                (id.clone(), "Speaker B".to_string())
            } else {
                (id.clone(), "Speaker A".to_string())
            }
        })
        .collect()
}

/// Remove filler words from text, returning cleaned text.
pub fn remove_filler_words(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    words
        .into_iter()
        .filter(|w| {
            let lowered = w.to_lowercase();
            let lower = lowered.trim_matches(|c: char| !c.is_alphabetic());
            !FILLER_WORDS.contains(&lower)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_filler_words() {
        let segments = vec![
            (
                "seg_001".to_string(),
                0,
                5000,
                "um well I think like basically".to_string(),
            ),
            (
                "seg_002".to_string(),
                5000,
                10000,
                "the project is actually going well".to_string(),
            ),
        ];

        let analysis = detect_filler_words(&segments);
        assert_eq!(analysis.filler_word_count, 6); // um, well, like, basically, actually, well
        assert!(analysis.filler_percentage > 0.0);
    }

    #[test]
    fn test_remove_filler_words() {
        let text = "um well I think like basically yes";
        let cleaned = remove_filler_words(text);
        assert_eq!(cleaned, "I think yes");
    }

    #[test]
    fn test_detect_speaker_changes() {
        let segments = vec![
            ("seg_001".to_string(), "Hello world".to_string()),
            ("seg_002".to_string(), "?What do you think".to_string()),
        ];
        let speakers = detect_speaker_changes(&segments);
        assert_eq!(speakers[0].1, "Speaker A");
        assert_eq!(speakers[1].1, "Speaker B");
    }
}
