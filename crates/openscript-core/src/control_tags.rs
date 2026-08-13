//! Control-tag handling for script text.
//!
//! TTS engines with inline control tokens (Higgs: `<|emotion:...|>`,
//! `<|style:...|>`, `<|sfx:...|>`, `<|prosody:...|>`) accept those tags
//! inline in the *spoken* text. The tags are consumed by the audio model and
//! must NEVER leak into display surfaces (captions, timeline previews, b-roll
//! keyword derivation). This module provides the single shared stripper so
//! every display path cleans text identically.

/// Remove TTS control tags (everything of the form `<|...|>`) from a text
/// string.
///
/// Control tags are audio-only directives consumed by the synthesis engine —
/// they are not spoken words and must not appear in captions or previews.
/// The pattern `<|...|>` is the canonical inline-control-token form used by
/// Higgs (`<|emotion:...|>`, `<|style:...|>`, `<|sfx:...|>`,
/// `<|prosody:...|>`). Any other bracketed text (`<tag>` without the pipe,
/// or a literal `<` in HTML-ish content) is left untouched.
///
/// Leftover whitespace from removed tags is collapsed: `<|sfx:whoosh|> Hmm`
/// becomes ` Hmm` (leading space trimmed), and `word <|prosody:pause|> word`
/// becomes `word word` (double space collapsed to one).
pub fn strip_control_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(start) = rest.find("<|") {
        // Keep everything before the tag.
        out.push_str(&rest[..start]);
        // Find the closing "|>" of this tag.
        match rest[start + 2..].find("|>") {
            Some(rel) => {
                let end = start + 2 + rel + 2;
                rest = &rest[end..];
            }
            None => {
                // Unclosed tag — keep the remainder verbatim.
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);

    // Collapse whitespace left by removed tags (double spaces, leading space
    // from a leading tag) into single spaces, then trim.
    let mut collapsed = String::with_capacity(out.len());
    let mut prev_ws = false;
    for ch in out.chars() {
        if ch.is_whitespace() {
            if !prev_ws {
                collapsed.push(' ');
            }
            prev_ws = true;
        } else {
            collapsed.push(ch);
            prev_ws = false;
        }
    }
    collapsed.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_single_prosody_tag() {
        let s = "Influence is not about what you say. <|prosody:long_pause|> It is about what people feel.";
        let stripped = strip_control_tags(s);
        assert_eq!(
            stripped,
            "Influence is not about what you say. It is about what people feel."
        );
    }

    #[test]
    fn strips_multiple_tags_inline() {
        let s = "<|emotion:angry|> This is loud. <|prosody:expressive_high|> And then this.";
        assert_eq!(strip_control_tags(s), "This is loud. And then this.");
    }

    #[test]
    fn strips_sfx_tag_touching_word() {
        // `<|sfx:laughter|>Haha` — the tag directly precedes the word with no
        // space; stripping must keep "Haha" (no leading space introduced).
        let s = "<|sfx:laughter|>Haha";
        assert_eq!(strip_control_tags(s), "Haha");
    }

    #[test]
    fn collapses_double_space_from_removed_tag() {
        let s = "word <|prosody:pause|> word";
        assert_eq!(strip_control_tags(s), "word word");
    }

    #[test]
    fn leaves_plain_text_untouched() {
        let s = "Just a normal sentence with <b>html-ish</b> brackets.";
        assert_eq!(strip_control_tags(s), s);
    }

    #[test]
    fn leaves_angle_brackets_without_pipe_untouched() {
        let s = "A < less-than sign is fine, and > is too.";
        assert_eq!(strip_control_tags(s), s);
    }

    #[test]
    fn handles_unclosed_tag_verbatim() {
        // No closing "|>" — keep the remainder as-is rather than dropping it.
        let s = "Start <|unclosed";
        assert_eq!(strip_control_tags(s), "Start <|unclosed");
    }

    #[test]
    fn empty_input() {
        assert_eq!(strip_control_tags(""), "");
    }

    #[test]
    fn strips_real_higgs_scene_vocabulary() {
        // Regression: the exact inline-tag forms used in production scripts
        // (higgs_stress_ctl.json scene lines). Tags are audio-only directives;
        // the caption must read as clean prose with tags fully removed.
        let s = "Influence is not about what you say. <|prosody:long_pause|> It is about what people feel before you even open your mouth.";
        assert_eq!(
            strip_control_tags(s),
            "Influence is not about what you say. It is about what people feel before you even open your mouth."
        );

        let s2 = "So the next time you speak, remember this. <|prosody:long_pause|> The pattern you name becomes real, and the one you ignore keeps its power. <|prosody:expressive_high|> Choose what you point at carefully.";
        assert_eq!(
            strip_control_tags(s2),
            "So the next time you speak, remember this. The pattern you name becomes real, and the one you ignore keeps its power. Choose what you point at carefully."
        );
    }

    #[test]
    fn leaves_single_angle_brackets_for_math_untouched() {
        // `<` / `>` used as comparison or markup must survive (only `<|` is a
        // control tag opener).
        let s = "If x < y and y > z then x < z.";
        assert_eq!(strip_control_tags(s), s);
    }
}
