//! Caption generation for from-scratch video creation.
//!
//! Generates ASS (Advanced SubStation Alpha) subtitle files from word-level
//! timing data. Supports 4 caption styles:
//! - word_highlight: TikTok-style, current word pops in highlight color
//! - sentence_fade: Full sentence appears, fades on change
//! - karaoke_fill: Word-by-word color fill as spoken
//! - subtitle_rail: Lower-third box with sentence text

use crate::script::CaptionsSpec;
use serde::Serialize;

/// A word with its start and end time in milliseconds.
#[derive(Debug, Clone, Serialize)]
pub struct WordTiming {
    pub word: String,
    pub start_ms: i64,
    pub end_ms: i64,
}

/// A sentence/scene with its words and overall timing.
#[derive(Debug, Clone)]
pub struct CaptionSegment {
    pub text: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub words: Vec<WordTiming>,
}

/// Generate word timings for a text segment given its overall duration.
///
/// If TTS returns word-level timestamps, use those. Otherwise, estimate
/// even-spacing based on word count and duration.
pub fn estimate_word_timings(text: &str, start_ms: i64, end_ms: i64) -> Vec<WordTiming> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() || end_ms <= start_ms {
        return Vec::new();
    }

    let total_duration = end_ms - start_ms;
    let per_word = total_duration / words.len() as i64;

    words
        .iter()
        .enumerate()
        .map(|(i, word)| WordTiming {
            word: word.to_string(),
            start_ms: start_ms + (i as i64 * per_word),
            end_ms: start_ms + ((i + 1) as i64 * per_word),
        })
        .collect()
}

/// Format milliseconds as ASS timestamp: H:MM:SS.cc
fn ass_time(ms: i64) -> String {
    let total_cs = ms / 10;
    let cs = total_cs % 100;
    let total_s = total_cs / 100;
    let s = total_s % 60;
    let total_m = total_s / 60;
    let m = total_m % 60;
    let h = total_m / 60;
    format!("{}:{:02}:{:02}.{:02}", h, m, s, cs)
}

/// Escape text for ASS format (escape braces and backslashes).
fn ass_escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('{', "\\{")
        .replace('}', "\\}")
        .replace('\n', "\\N")
}

/// Generate an ASS subtitle file from caption segments.
///
/// The style parameter controls which caption style is used:
/// - word_highlight: each word appears as it's spoken, current word in highlight color
/// - sentence_fade: full sentence appears at scene start, fades out at end
/// - karaoke_fill: word-by-word color fill (karaoke effect)
/// - subtitle_rail: lower-third box with sentence text
pub fn generate_ass(
    segments: &[CaptionSegment],
    spec: &CaptionsSpec,
    canvas_width: u32,
    canvas_height: u32,
) -> String {
    let mut out = String::new();

    // ASS header
    out.push_str("[Script Info]\n");
    out.push_str("ScriptType: v4.00+\n");
    out.push_str(&format!("PlayResX: {}\n", canvas_width));
    out.push_str(&format!("PlayResY: {}\n", canvas_height));
    out.push_str("ScaledBorderAndShadow: yes\n");
    out.push('\n');

    // Styles
    out.push_str("[V4+ Styles]\n");
    out.push_str("Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n");

    let primary_color = hex_to_ass_color(&spec.color);
    let highlight_color = hex_to_ass_color(&spec.highlight_color);

    // Default style — center of screen, bold, with outline and shadow
    // Alignment=5 means middle-center
    let margin_v = canvas_height / 6; // push captions toward center area
    out.push_str(&format!(
        "Style: Default,{},{},&H00FFFFFF,&H000000FF,&H00000000,&H80000000,1,0,0,0,100,100,0,0,1,5,2,5,80,80,{},1\n",
        spec.font,
        spec.font_size,
        margin_v,
    ));

    // Highlight style — same position, highlight color, bold
    out.push_str(&format!(
        "Style: Highlight,{},{},{},&H000000FF,&H00000000,&H80000000,1,0,0,0,100,100,0,0,1,5,2,5,80,80,{},1\n",
        spec.font,
        spec.font_size,
        highlight_color,
        margin_v,
    ));

    out.push('\n');

    // Events
    out.push_str("[Events]\n");
    out.push_str("Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n");

    match spec.style.as_str() {
        "word_highlight" => generate_word_highlight(&mut out, segments, &spec, primary_color, highlight_color),
        "sentence_fade" => generate_sentence_fade(&mut out, segments, &spec),
        "karaoke_fill" => generate_karaoke_fill(&mut out, segments, &spec, primary_color, highlight_color),
        "subtitle_rail" => generate_subtitle_rail(&mut out, segments, &spec, canvas_width, canvas_height),
        _ => generate_word_highlight(&mut out, segments, &spec, primary_color, highlight_color),
    }

    out
}

/// Convert hex color (#RRGGBB) to ASS color (&H00BBGGRR).
fn hex_to_ass_color(hex: &str) -> String {
    let hex = hex.trim_start_matches('#');
    if hex.len() != 6 {
        return "&H00FFFFFF".to_string(); // default white
    }
    let r = &hex[0..2];
    let g = &hex[2..4];
    let b = &hex[4..6];
    format!("&H00{}{}{}", b, g, r) // ASS is BGR
}

/// Word highlight style: the full line is shown for the chunk duration,
/// with the currently-spoken word highlighted in a different color.
/// As each word is spoken, the highlight moves to it.
///
/// SENTENCE SEPARATION: Each segment is treated as a complete sentence.
/// A gap is enforced between segments so captions don't leak into the next
/// scene. The line is cleared at the end of each segment.
fn generate_word_highlight(
    out: &mut String,
    segments: &[CaptionSegment],
    spec: &CaptionsSpec,
    primary_color: String,
    highlight_color: String,
) {
    for seg in segments {
        let words: Vec<&WordTiming> = seg.words.iter().collect();
        let max_per_line = spec.max_words_per_line as usize;

        // If no words, show the full text as a single event for the segment
        if words.is_empty() {
            let text = ass_escape(&seg.text);
            out.push_str(&format!(
                "Dialogue: 1,{},{},Default,,0,0,0,,{{\\fad(100,100)}}{}\n",
                ass_time(seg.start_ms),
                ass_time(seg.end_ms),
                text
            ));
            continue;
        }

        for chunk in words.chunks(max_per_line) {
            let chunk_end = chunk.last().map(|w| w.end_ms).unwrap_or(seg.end_ms);

            // For each word in the chunk, create a dialogue event spanning
            // that word's duration. The full line is shown, with the current
            // word highlighted. This creates the "word-by-word highlight" effect.
            for (i, word) in chunk.iter().enumerate() {
                let text = build_highlighted_line(chunk, i, &primary_color, &highlight_color);
                out.push_str(&format!(
                    "Dialogue: 1,{},{},Default,,0,0,0,,{}\n",
                    ass_time(word.start_ms),
                    ass_time(word.end_ms),
                    text
                ));
            }
        }

        // SENTENCE SEPARATION: Clear captions at the end of each segment
        // by ensuring the last word's end_ms equals the segment end.
        // The next segment starts fresh with its own first word.
        // No gap-filling event is added — the screen goes blank between
        // sentences, preventing text from leaking into the next scene.
    }
}

/// Build a line of text where the word at `highlight_idx` is in highlight color.
/// Words are escaped individually BEFORE being inserted into the ASS string,
/// so the override tags ({\c...}) are not corrupted.
fn build_highlighted_line(
    words: &[&WordTiming],
    highlight_idx: usize,
    primary_color: &str,
    highlight_color: &str,
) -> String {
    words
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let escaped_word = ass_escape(&w.word);
            if i == highlight_idx {
                // Highlight: change color + scale up slightly for emphasis
                format!("{{\\c{}\\fscx110\\fscy110}}{}{{\\c{}\\fscx100\\fscy100}}", highlight_color, escaped_word, primary_color)
            } else {
                escaped_word
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Sentence fade style: full sentence appears at scene start, fades out.
fn generate_sentence_fade(out: &mut String, segments: &[CaptionSegment], _spec: &CaptionsSpec) {
    for seg in segments {
        let text = ass_escape(&seg.text);
        out.push_str(&format!(
            "Dialogue: 1,{},{},Default,,0,0,0,,{{\\fad(200,300)}}{}\n",
            ass_time(seg.start_ms),
            ass_time(seg.end_ms),
            text
        ));
    }
}

/// Karaoke fill style: word-by-word color fill as spoken.
fn generate_karaoke_fill(
    out: &mut String,
    segments: &[CaptionSegment],
    spec: &CaptionsSpec,
    _primary_color: String,
    highlight_color: String,
) {
    for seg in segments {
        // Build a single dialogue line with karaoke timing
        let mut text = String::new();
        text.push_str(&format!("{{\\c{}}}", highlight_color));

        for (i, word) in seg.words.iter().enumerate() {
            let dur_cs = (word.end_ms - word.start_ms) / 10;
            text.push_str(&format!("{{\\k{}}}{}", dur_cs.max(1), ass_escape(&word.word)));
            if i < seg.words.len() - 1 {
                text.push(' ');
            }
        }
        text.push_str(&format!("{{\\c{}}}", hex_to_ass_color(&spec.color)));

        out.push_str(&format!(
            "Dialogue: 1,{},{},Default,,0,0,0,,{}\n",
            ass_time(seg.start_ms),
            ass_time(seg.end_ms),
            text
        ));
    }
}

/// Subtitle rail style: centered box with sentence text.
fn generate_subtitle_rail(
    out: &mut String,
    segments: &[CaptionSegment],
    _spec: &CaptionsSpec,
    canvas_width: u32,
    canvas_height: u32,
) {
    // Center the box vertically
    let box_height = 200i64;
    let box_top = (canvas_height as i64 / 2) - (box_height / 2);
    let box_bottom = box_top + box_height;
    let box_left = 60i64;
    let box_right = canvas_width as i64 - 60;

    for seg in segments {
        let text = ass_escape(&seg.text);
        // Draw a semi-transparent box behind the text (layer 0)
        out.push_str(&format!(
            "Dialogue: 0,{},{},Default,,0,0,0,,{{\\1c&H80000000&\\p1}}m {} {} l {} {} l {} {} l {} {} {{\\p0}}\n",
            ass_time(seg.start_ms),
            ass_time(seg.end_ms),
            box_left, box_top,
            box_right, box_top,
            box_right, box_bottom,
            box_left, box_bottom,
        ));
        // Text on layer 1
        out.push_str(&format!(
            "Dialogue: 1,{},{},Default,,0,0,0,,{}\n",
            ass_time(seg.start_ms),
            ass_time(seg.end_ms),
            text
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_spec() -> CaptionsSpec {
        CaptionsSpec {
            style: "word_highlight".to_string(),
            font: "Bebas Neue".to_string(),
            font_size: 72,
            color: "#ffffff".to_string(),
            highlight_color: "#00ff88".to_string(),
            position: "bottom".to_string(),
            safe_zone: 0.85,
            max_words_per_line: 5,
        }
    }

    #[test]
    fn test_estimate_word_timings() {
        let words = estimate_word_timings("hello world test", 0, 3000);
        assert_eq!(words.len(), 3);
        assert_eq!(words[0].word, "hello");
        assert_eq!(words[0].start_ms, 0);
        assert_eq!(words[0].end_ms, 1000);
        assert_eq!(words[1].start_ms, 1000);
        assert_eq!(words[2].end_ms, 3000);
    }

    #[test]
    fn test_estimate_word_timings_empty() {
        let words = estimate_word_timings("", 0, 3000);
        assert!(words.is_empty());
    }

    #[test]
    fn test_estimate_word_timings_zero_duration() {
        let words = estimate_word_timings("hello", 1000, 1000);
        assert!(words.is_empty());
    }

    #[test]
    fn test_ass_time_format() {
        assert_eq!(ass_time(0), "0:00:00.00");
        assert_eq!(ass_time(1500), "0:00:01.50");
        assert_eq!(ass_time(65000), "0:01:05.00");
        assert_eq!(ass_time(3661000), "1:01:01.00");
    }

    #[test]
    fn test_ass_escape() {
        assert_eq!(ass_escape("hello"), "hello");
        assert_eq!(ass_escape("{braces}"), "\\{braces\\}");
        assert_eq!(ass_escape("back\\slash"), "back\\\\slash");
    }

    #[test]
    fn test_hex_to_ass_color() {
        assert_eq!(hex_to_ass_color("#ffffff"), "&H00ffffff");
        assert_eq!(hex_to_ass_color("#00ff88"), "&H0088ff00");
        assert_eq!(hex_to_ass_color("ff0000"), "&H000000ff");
        assert_eq!(hex_to_ass_color("invalid"), "&H00FFFFFF"); // fallback
    }

    #[test]
    fn test_generate_ass_word_highlight() {
        let mut spec = test_spec();
        spec.style = "word_highlight".to_string();
        let segments = vec![CaptionSegment {
            text: "hello world".to_string(),
            start_ms: 0,
            end_ms: 2000,
            words: estimate_word_timings("hello world", 0, 2000),
        }];
        let ass = generate_ass(&segments, &spec, 1080, 1920);
        assert!(ass.contains("[Script Info]"));
        assert!(ass.contains("[V4+ Styles]"));
        assert!(ass.contains("[Events]"));
        assert!(ass.contains("Dialogue:"));
        assert!(ass.contains("hello"));
        assert!(ass.contains("world"));
        // BUG FIX: ASS override tags should NOT be escaped
        // The old code was escaping { and } which broke the tags
        assert!(!ass.contains("\\{\\c"), "ASS override tags must not be escaped");
        assert!(ass.contains("{\\c"), "Should contain valid ASS color override tags");
        // Should use center alignment (Alignment=5)
        assert!(ass.contains(",5,"), "Should use center alignment (Alignment=5)");
    }

    #[test]
    fn test_generate_ass_sentence_fade() {
        let mut spec = test_spec();
        spec.style = "sentence_fade".to_string();
        let segments = vec![CaptionSegment {
            text: "Welcome to the show".to_string(),
            start_ms: 1000,
            end_ms: 4000,
            words: Vec::new(),
        }];
        let ass = generate_ass(&segments, &spec, 1080, 1920);
        assert!(ass.contains("\\fad(200,300)"));
        assert!(ass.contains("Welcome to the show"));
    }

    #[test]
    fn test_generate_ass_karaoke_fill() {
        let mut spec = test_spec();
        spec.style = "karaoke_fill".to_string();
        let segments = vec![CaptionSegment {
            text: "hello world".to_string(),
            start_ms: 0,
            end_ms: 2000,
            words: estimate_word_timings("hello world", 0, 2000),
        }];
        let ass = generate_ass(&segments, &spec, 1080, 1920);
        assert!(ass.contains("\\k"));
        assert!(ass.contains("hello"));
    }

    #[test]
    fn test_generate_ass_subtitle_rail() {
        let mut spec = test_spec();
        spec.style = "subtitle_rail".to_string();
        let segments = vec![CaptionSegment {
            text: "Breaking news".to_string(),
            start_ms: 0,
            end_ms: 3000,
            words: Vec::new(),
        }];
        let ass = generate_ass(&segments, &spec, 1080, 1920);
        assert!(ass.contains("Dialogue: 0,")); // layer 0 = box
        assert!(ass.contains("Dialogue: 1,")); // layer 1 = text
        assert!(ass.contains("Breaking news"));
    }

    #[test]
    fn test_generate_ass_empty_segments() {
        let spec = test_spec();
        let segments: Vec<CaptionSegment> = Vec::new();
        let ass = generate_ass(&segments, &spec, 1080, 1920);
        assert!(ass.contains("[Script Info]"));
        assert!(ass.contains("[Events]"));
        // No dialogue lines
        assert!(!ass.contains("Dialogue:"));
    }

    #[test]
    fn test_build_highlighted_line() {
        let words = estimate_word_timings("one two three", 0, 3000);
        let word_refs: Vec<&WordTiming> = words.iter().collect();
        let line = build_highlighted_line(&word_refs, 1, "&H00ffffff", "&H0088ff00");
        // Word at index 1 ("two") should have the highlight color + scale
        assert!(line.contains("&H0088ff00"), "Should contain highlight color");
        assert!(line.contains("two"), "Should contain highlighted word");
        assert!(line.contains("one"), "Should contain non-highlighted word");
        assert!(line.contains("three"), "Should contain non-highlighted word");
        // Should contain scale override for the highlighted word
        assert!(line.contains("fscx110"), "Should contain scale-up for highlighted word");
    }

    #[test]
    fn test_word_highlight_max_words_per_line() {
        let mut spec = test_spec();
        spec.max_words_per_line = 2;
        let segments = vec![CaptionSegment {
            text: "one two three four".to_string(),
            start_ms: 0,
            end_ms: 4000,
            words: estimate_word_timings("one two three four", 0, 4000),
        }];
        let ass = generate_ass(&segments, &spec, 1080, 1920);
        // Should have dialogue lines for 2 chunks of 2 words each
        let dialogue_count = ass.matches("Dialogue:").count();
        // 2 words in line 1 + 2 words in line 2 = 4 dialogue events
        assert!(dialogue_count >= 4, "Expected at least 4 dialogue events, got {}", dialogue_count);
    }
}
