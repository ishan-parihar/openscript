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

/// Ensure segments carry REAL word-level timings for word-based styles
/// (word_highlight, karaoke_fill).
///
/// When the source transcript is phrase-level (no per-word timestamps — e.g.
/// the A2V hinglish-ggml SRT), the SRT parser + grouping produces synthetic
/// "words" that contain the ENTIRE phrase (one per SRT cue, or several when
/// `group_entries_with_words` merges cues up to max_words). Feeding those to
/// `word_highlight` wraps large chunks — or the whole line — in the highlight
/// color, rendering the captions as a dim green block that is effectively
/// invisible on footage (the A2V caption-invisibility bug). Splitting any
/// whitespace-bearing synthetic word into even-spaced estimates restores the
/// intended white text with only the current word highlighted. Genuine
/// single-word captions ("Wow!") are left untouched.
pub fn normalize_word_timings(segments: &mut [CaptionSegment]) {
    for seg in segments.iter_mut() {
        let has_embedded_whitespace = seg
            .words
            .iter()
            .any(|w| w.word.trim().contains(char::is_whitespace));
        if !has_embedded_whitespace {
            continue;
        }
        // Rebuild ALL words from the segment text with even-spaced estimates
        // across the segment window (the synthetic words carry no real timing).
        seg.words = estimate_word_timings(&seg.text, seg.start_ms, seg.end_ms);
    }
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

    // Position → ASS Alignment + vertical margin. Alignment follows the
    // num-pad layout: 2 = bottom-center, 5 = middle-center, 8 = top-center.
    // Bottom is the shorts convention — captions sit in the lower safe zone,
    // clear of the subject. This FIXES the caption-position bug: `spec.position`
    // was previously parsed but never honored, so every style hardcoded
    // Alignment=5 (mid-screen) and captions rendered over the subject.
    let (alignment, margin_v) = match spec.position.as_str() {
        "top" => (8u32, canvas_height / 20),
        "center" => (5u32, canvas_height / 6),
        // "bottom" (and anything unknown) → bottom-center safe zone
        _ => (2u32, canvas_height / 20),
    };
    // Default style — bold, with outline and shadow.
    // Use the spec's caption color (not hardcoded white) so the calm
    // theme's cream text (#F5F0E8) flows through to the ASS renderer.
    // (Round-2 UX audit GAP #13 fix — caption color was drifting to white.)
    out.push_str(&format!(
        "Style: Default,{},{},{},&H000000FF,&H00000000,&H80000000,1,0,0,0,100,100,0,0,1,5,2,{},80,80,{},1\n",
        spec.font,
        spec.font_size,
        primary_color,
        alignment,
        margin_v,
    ));

    // Highlight style — same position, highlight color, bold
    out.push_str(&format!(
        "Style: Highlight,{},{},{},&H000000FF,&H00000000,&H80000000,1,0,0,0,100,100,0,0,1,5,2,{},80,80,{},1\n",
        spec.font,
        spec.font_size,
        highlight_color,
        alignment,
        margin_v,
    ));

    out.push('\n');

    // Events
    out.push_str("[Events]\n");
    out.push_str(
        "Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n",
    );

    // Phrase-level transcripts must be split into real word timings BEFORE
    // word-based styles render, otherwise the whole line is highlighted.
    let mut segments = segments.to_vec();
    normalize_word_timings(&mut segments);

    match spec.style.as_str() {
        "word_highlight" => {
            generate_word_highlight(&mut out, &segments, spec, primary_color, highlight_color)
        }
        "sentence_fade" => generate_sentence_fade(&mut out, &segments, spec),
        "karaoke_fill" => {
            generate_karaoke_fill(&mut out, &segments, spec, primary_color, highlight_color)
        }
        "subtitle_rail" => {
            generate_subtitle_rail(&mut out, &segments, spec, canvas_width, canvas_height)
        }
        _ => generate_word_highlight(&mut out, &segments, spec, primary_color, highlight_color),
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
/// SENTENCE SEPARATION: Each segment is split into sentences on punctuation.
/// Each sentence is displayed independently — the next sentence does NOT
/// appear until the current one is fully spoken.
fn generate_word_highlight(
    out: &mut String,
    segments: &[CaptionSegment],
    spec: &CaptionsSpec,
    primary_color: String,
    highlight_color: String,
) {
    for seg in segments {
        // Split words into sentences based on punctuation (., !, ?)
        let sentences = split_words_into_sentences(&seg.words);
        let max_per_line = spec.max_words_per_line as usize;

        for sentence in &sentences {
            if sentence.is_empty() {
                continue;
            }

            // If the sentence fits in one line, show it as a single line
            if sentence.len() <= max_per_line {
                // For each word in the sentence, show the full sentence with
                // the current word highlighted
                for (i, word) in sentence.iter().enumerate() {
                    let text =
                        build_highlighted_line(sentence, i, &primary_color, &highlight_color);
                    out.push_str(&format!(
                        "Dialogue: 1,{},{},Default,,0,0,0,,{}\n",
                        ass_time(word.start_ms),
                        ass_time(word.end_ms),
                        text
                    ));
                }
            } else {
                // Sentence is longer than max_per_line — split into chunks
                for chunk in sentence.chunks(max_per_line) {
                    for (i, word) in chunk.iter().enumerate() {
                        let text =
                            build_highlighted_line(chunk, i, &primary_color, &highlight_color);
                        out.push_str(&format!(
                            "Dialogue: 1,{},{},Default,,0,0,0,,{}\n",
                            ass_time(word.start_ms),
                            ass_time(word.end_ms),
                            text
                        ));
                    }
                }
            }
        }

        // If no words at all, show the full text as a single event
        if seg.words.is_empty() && !seg.text.is_empty() {
            let text = ass_escape(&seg.text);
            out.push_str(&format!(
                "Dialogue: 1,{},{},Default,,0,0,0,,{{\\fad(100,100)}}{}\n",
                ass_time(seg.start_ms),
                ass_time(seg.end_ms),
                text
            ));
        }
    }
}

/// Split a list of word timings into sentences based on punctuation.
/// A sentence ends when a word contains ., !, or ? at the end.
fn split_words_into_sentences(words: &[WordTiming]) -> Vec<Vec<&WordTiming>> {
    let mut sentences = Vec::new();
    let mut current = Vec::new();

    for word in words {
        current.push(word);
        // Check if this word ends a sentence
        let trimmed = word.word.trim();
        if trimmed.ends_with('.') || trimmed.ends_with('!') || trimmed.ends_with('?') {
            sentences.push(current.clone());
            current.clear();
        }
    }

    // Don't forget the last partial sentence
    if !current.is_empty() {
        sentences.push(current);
    }

    sentences
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
                format!(
                    "{{\\fad(80,80)\\c{}\\fscx110\\fscy110}}{}{{\\c{}\\fscx100\\fscy100}}",
                    highlight_color, escaped_word, primary_color
                )
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
            text.push_str(&format!(
                "{{\\k{}}}{}",
                dur_cs.max(1),
                ass_escape(&word.word)
            ));
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
        assert!(
            !ass.contains("\\{\\c"),
            "ASS override tags must not be escaped"
        );
        assert!(
            ass.contains("{\\c"),
            "Should contain valid ASS color override tags"
        );
        // Default (bottom) position → bottom-center alignment (2) in the
        // num-pad layout, with a safe-zone bottom margin (1920/20 = 96).
        // Regression test for the caption-position bug: position was parsed
        // but ignored, and Alignment was hardcoded to 5 (mid-screen).
        assert!(
            ass.contains("Style: Default,Bebas Neue,72,&H00ffffff,&H000000FF,&H00000000,&H80000000,1,0,0,0,100,100,0,0,1,5,2,2,80,80,96,1"),
            "Default style should use Alignment=2 (bottom-center) with 96px margin, got: {}",
            ass.lines()
                .find(|l| l.starts_with("Style: Default"))
                .unwrap_or("")
        );
    }

    #[test]
    fn test_generate_ass_position_mapping() {
        // position must be honored: bottom → Alignment 2, center → 5, top → 8.
        let cases = [
            ("bottom", "1,5,2,2,80,80,96,1"),
            ("center", "1,5,2,5,80,80,320,1"),
            ("top", "1,5,2,8,80,80,96,1"),
        ];
        for (position, style_suffix) in cases {
            let mut spec = test_spec();
            spec.position = position.to_string();
            let segments = vec![CaptionSegment {
                text: "hello world".to_string(),
                start_ms: 0,
                end_ms: 2000,
                words: estimate_word_timings("hello world", 0, 2000),
            }];
            let ass = generate_ass(&segments, &spec, 1080, 1920);
            let default_style = ass
                .lines()
                .find(|l| l.starts_with("Style: Default"))
                .unwrap_or("");
            assert!(
                default_style.ends_with(style_suffix),
                "position '{}': expected style suffix '{}', got: {}",
                position,
                style_suffix,
                default_style
            );
        }
    }

    #[test]
    fn test_normalize_word_timings_splits_phrase_level() {
        // Phrase-level SRT: one synthetic "word" holds the whole phrase.
        let mut segments = vec![CaptionSegment {
            text: "hello world test".to_string(),
            start_ms: 0,
            end_ms: 3000,
            words: vec![WordTiming {
                word: "hello world test".to_string(),
                start_ms: 0,
                end_ms: 3000,
            }],
        }];
        normalize_word_timings(&mut segments);
        assert_eq!(segments[0].words.len(), 3, "phrase should split into words");
        assert_eq!(segments[0].words[0].word, "hello");
        assert_eq!(segments[0].words[1].word, "world");
        // Timings stay within the segment window.
        assert!(segments[0].words[0].start_ms >= 0);
        assert!(segments[0].words[2].end_ms <= 3000);
    }

    #[test]
    fn test_normalize_word_timings_splits_merged_phrases() {
        // group_entries_with_words can merge several phrase-level cues into
        // one segment: 2+ synthetic "words", each holding multi-word text.
        let mut segments = vec![CaptionSegment {
            text: "hello world test again".to_string(),
            start_ms: 0,
            end_ms: 4000,
            words: vec![
                WordTiming {
                    word: "hello world".to_string(),
                    start_ms: 0,
                    end_ms: 2000,
                },
                WordTiming {
                    word: "test again".to_string(),
                    start_ms: 2000,
                    end_ms: 4000,
                },
            ],
        }];
        normalize_word_timings(&mut segments);
        assert_eq!(segments[0].words.len(), 4, "merged phrases must split into words");
        assert_eq!(segments[0].words[0].word, "hello");
        assert_eq!(segments[0].words[3].word, "again");
    }

    #[test]
    fn test_normalize_word_timings_leaves_single_word() {
        let mut segments = vec![CaptionSegment {
            text: "Wow!".to_string(),
            start_ms: 0,
            end_ms: 1000,
            words: vec![WordTiming {
                word: "Wow!".to_string(),
                start_ms: 0,
                end_ms: 1000,
            }],
        }];
        normalize_word_timings(&mut segments);
        assert_eq!(segments[0].words.len(), 1, "single word stays untouched");
    }

    #[test]
    fn test_word_highlight_phrase_level_renders_white_text() {
        // A phrase-level segment must render white text with only the current
        // word highlighted — the regression behind the invisible captions.
        let mut spec = test_spec();
        spec.style = "word_highlight".to_string();
        let segments = vec![CaptionSegment {
            text: "hello world test".to_string(),
            start_ms: 0,
            end_ms: 3000,
            words: vec![WordTiming {
                word: "hello world test".to_string(),
                start_ms: 0,
                end_ms: 3000,
            }],
        }];
        let ass = generate_ass(&segments, &spec, 1080, 1920);
        // First word highlighted, then an immediate white reset before the
        // remaining words — NOT a green wrap around the whole line.
        assert!(
            ass.contains("hello{\\c&H00ffffff\\fscx100\\fscy100} world test"),
            "plain words must follow the white reset right after the highlighted word"
        );
        // The highlight color must never wrap the entire line: no dialogue
        // whose text ends with the white reset after ALL words are green.
        for line in ass.lines().filter(|l| l.starts_with("Dialogue:")) {
            let after = line.split(",,,").nth(1).unwrap_or("");
            assert!(
                !after.starts_with("{\\fad(80,80)\\c&H0088ff00") || after.contains("{\\c&H00ffffff"),
                "green highlight must always be closed with a white reset: {}",
                line
            );
        }
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
        assert!(
            line.contains("&H0088ff00"),
            "Should contain highlight color"
        );
        assert!(line.contains("two"), "Should contain highlighted word");
        assert!(line.contains("one"), "Should contain non-highlighted word");
        assert!(
            line.contains("three"),
            "Should contain non-highlighted word"
        );
        // Should contain scale override for the highlighted word
        assert!(
            line.contains("fscx110"),
            "Should contain scale-up for highlighted word"
        );
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
        assert!(
            dialogue_count >= 4,
            "Expected at least 4 dialogue events, got {}",
            dialogue_count
        );
    }
}
