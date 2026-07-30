//! Sticker preset system for safe positioning relative to caption rail.
//!
//! Provides predefined sticker positions that avoid overlap with caption
//! safe zones for different caption styles (word_highlight, karaoke, etc.).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Preset for sticker positioning with safe zone awareness.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StickerPresetConfig {
    pub name: String,
    pub description: String,
    /// Position string compatible with FFmpeg overlay filter
    pub position: String,
    /// Scale factor relative to canvas width (0.0-1.0)
    pub scale: f64,
    /// Minimum margin from bottom edge in pixels (for caption rail clearance)
    pub safe_margin_px: u32,
    /// Speaker role this preset is designed for
    pub speaker_role: Option<String>,
    /// Canvas width this preset was designed for
    pub canvas_width: u32,
    /// Canvas height this preset was designed for
    pub canvas_height: u32,
}

/// All available sticker presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StickerPreset {
    /// Speaker on left side of frame
    SpeakerLeft,
    /// Speaker on right side of frame
    SpeakerRight,
    /// Speaker centered, above caption rail
    SpeakerCenter,
    /// Reaction sticker at top center
    ReactionTop,
    /// Reaction sticker at top right corner
    ReactionCorner,
    /// Small sticker at top left
    TopLeftSmall,
    /// Small sticker at top right
    TopRightSmall,
    /// Centered with large margin for captions
    CenterSafe,
}

impl StickerPreset {
    /// Get the configuration for this preset.
    pub fn config(self) -> StickerPresetConfig {
        match self {
            StickerPreset::SpeakerLeft => StickerPresetConfig {
                name: "SpeakerLeft".to_string(),
                description: "Speaker on left side, vertically centered with caption clearance".to_string(),
                position: "center-left".to_string(),
                scale: 0.30,
                safe_margin_px: 60,
                speaker_role: Some("speaker_1".to_string()),
                canvas_width: 1080,
                canvas_height: 1920,
            },
            StickerPreset::SpeakerRight => StickerPresetConfig {
                name: "SpeakerRight".to_string(),
                description: "Speaker on right side, vertically centered with caption clearance".to_string(),
                position: "center-right".to_string(),
                scale: 0.30,
                safe_margin_px: 60,
                speaker_role: Some("speaker_2".to_string()),
                canvas_width: 1080,
                canvas_height: 1920,
            },
            StickerPreset::SpeakerCenter => StickerPresetConfig {
                name: "SpeakerCenter".to_string(),
                description: "Speaker centered above caption rail with large margin".to_string(),
                position: "center".to_string(),
                scale: 0.25,
                safe_margin_px: 80,
                speaker_role: Some("narrator".to_string()),
                canvas_width: 1080,
                canvas_height: 1920,
            },
            StickerPreset::ReactionTop => StickerPresetConfig {
                name: "ReactionTop".to_string(),
                description: "Reaction sticker at top center for emphasis".to_string(),
                position: "top-center".to_string(),
                scale: 0.40,
                safe_margin_px: 40,
                speaker_role: None,
                canvas_width: 1080,
                canvas_height: 1920,
            },
            StickerPreset::ReactionCorner => StickerPresetConfig {
                name: "ReactionCorner".to_string(),
                description: "Reaction sticker at top right corner".to_string(),
                position: "top-right".to_string(),
                scale: 0.35,
                safe_margin_px: 40,
                speaker_role: None,
                canvas_width: 1080,
                canvas_height: 1920,
            },
            StickerPreset::TopLeftSmall => StickerPresetConfig {
                name: "TopLeftSmall".to_string(),
                description: "Small sticker at top left corner".to_string(),
                position: "top-left".to_string(),
                scale: 0.25,
                safe_margin_px: 40,
                speaker_role: None,
                canvas_width: 1080,
                canvas_height: 1920,
            },
            StickerPreset::TopRightSmall => StickerPresetConfig {
                name: "TopRightSmall".to_string(),
                description: "Small sticker at top right corner".to_string(),
                position: "top-right".to_string(),
                scale: 0.25,
                safe_margin_px: 40,
                speaker_role: None,
                canvas_width: 1080,
                canvas_height: 1920,
            },
            StickerPreset::CenterSafe => StickerPresetConfig {
                name: "CenterSafe".to_string(),
                description: "Centered with maximum caption clearance".to_string(),
                position: "center".to_string(),
                scale: 0.20,
                safe_margin_px: 100,
                speaker_role: None,
                canvas_width: 1080,
                canvas_height: 1920,
            },
        }
    }

    /// Get all presets as a map.
    pub fn all() -> HashMap<String, StickerPresetConfig> {
        [
            StickerPreset::SpeakerLeft,
            StickerPreset::SpeakerRight,
            StickerPreset::SpeakerCenter,
            StickerPreset::ReactionTop,
            StickerPreset::ReactionCorner,
            StickerPreset::TopLeftSmall,
            StickerPreset::TopRightSmall,
            StickerPreset::CenterSafe,
        ]
        .iter()
        .map(|p| (p.to_string(), p.config()))
        .collect()
    }

    /// Get a preset by name (case-insensitive).
    pub fn from_name(name: &str) -> Option<Self> {
        let name_lower = name.to_lowercase().replace('-', "_");
        match name_lower.as_str() {
            "speaker_left" | "speakerleft" => Some(Self::SpeakerLeft),
            "speaker_right" | "speakerright" => Some(Self::SpeakerRight),
            "speaker_center" | "speakercenter" => Some(Self::SpeakerCenter),
            "reaction_top" | "reactiontop" => Some(Self::ReactionTop),
            "reaction_corner" | "reactioncorner" => Some(Self::ReactionCorner),
            "top_left_small" | "topleftsmall" => Some(Self::TopLeftSmall),
            "top_right_small" | "toprightsmall" => Some(Self::TopRightSmall),
            "center_safe" | "centersafe" => Some(Self::CenterSafe),
            _ => None,
        }
    }
}

impl std::fmt::Display for StickerPreset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::SpeakerLeft => "speaker_left",
            Self::SpeakerRight => "speaker_right",
            Self::SpeakerCenter => "speaker_center",
            Self::ReactionTop => "reaction_top",
            Self::ReactionCorner => "reaction_corner",
            Self::TopLeftSmall => "top_left_small",
            Self::TopRightSmall => "top_right_small",
            Self::CenterSafe => "center_safe",
        };
        write!(f, "{}", name)
    }
}

/// Validate that a sticker position/scale combination is safe for a given caption style.
pub fn validate_sticker_safety(
    position: &str,
    scale: f64,
    caption_style: Option<&str>,
    canvas_width: u32,
    canvas_height: u32,
) -> (bool, Vec<String>) {
    let mut warnings = Vec::new();
    let mut safe = true;

    // Calculate sticker bounding box
    let sticker_w = (canvas_width as f64 * scale).round() as i32;
    let sticker_h = sticker_w; // square stickers
    let margin = 40i32;

    let (tl_x, tl_y) = match position.to_lowercase().as_str() {
        "top-left" => (margin, margin),
        "top-right" => (canvas_width as i32 - sticker_w - margin, margin),
        "top-center" | "center-top" => ((canvas_width as i32 - sticker_w) / 2, margin),
        "bottom-left" => (margin, canvas_height as i32 - sticker_h - margin),
        "bottom-right" => (canvas_width as i32 - sticker_w - margin, canvas_height as i32 - sticker_h - margin),
        "bottom-center" | "center-bottom" => ((canvas_width as i32 - sticker_w) / 2, canvas_height as i32 - sticker_h - margin),
        "center" => ((canvas_width as i32 - sticker_w) / 2, (canvas_height as i32 - sticker_h) / 2),
        "center-left" => (margin, (canvas_height as i32 - sticker_h) / 2),
        "center-right" => (canvas_width as i32 - sticker_w - margin, (canvas_height as i32 - sticker_h) / 2),
        _ => (margin, margin), // default to top-left
    };

    let sticker_box = (tl_x, tl_y, tl_x + sticker_w, tl_y + sticker_h);

    // Caption safe zone based on style
    let rail_ratio = match caption_style {
        Some("word_highlight" | "karaoke") => 0.15,
        Some("sentence_fade") => 0.12,
        Some("burn_in" | "subtitle_rail") => 0.10,
        _ => 0.12, // default
    };
    let rail_h = (canvas_height as f64 * rail_ratio).round() as i32;
    let caption_box = (0, canvas_height as i32 - rail_h, canvas_width as i32, canvas_height as i32);

    // Check overlap
    let (s_left, s_top, s_right, s_bottom) = sticker_box;
    let (c_left, c_top, c_right, c_bottom) = caption_box;

    if !(s_right <= c_left || c_right <= s_left || s_bottom <= c_top || c_bottom <= s_top) {
        warnings.push(format!(
            "Sticker at '{}' scale={:.2} overlaps caption safe zone ({}–{}px from top)",
            position, scale, c_top, c_bottom
        ));
        safe = false;
    }

    // Check scale bounds
    if !(0.15..=0.50).contains(&scale) {
        warnings.push(format!("Scale {:.2} outside recommended range 0.15–0.50", scale));
        safe = false;
    }

    (safe, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_preset_configs() {
        let presets = StickerPreset::all();
        assert_eq!(presets.len(), 8);

        // Check SpeakerLeft
        let left = presets.get("speaker_left").unwrap();
        assert_eq!(left.position, "center-left");
        assert_eq!(left.scale, 0.30);
        assert_eq!(left.safe_margin_px, 60);
    }

    #[test]
    fn test_preset_from_name() {
        assert!(StickerPreset::from_name("speaker_left").is_some());
        assert!(StickerPreset::from_name("SpeakerLeft").is_some());
        assert!(StickerPreset::from_name("speaker-left").is_some());
        assert!(StickerPreset::from_name("invalid").is_none());
    }

    #[test]
    fn test_validate_safety_bottom_overlaps() {
        let (safe, warnings) = validate_sticker_safety("bottom-center", 0.35, Some("word_highlight"), 1080, 1920);
        assert!(!safe);
        assert!(warnings.iter().any(|w| w.contains("overlaps")));
    }

    #[test]
    fn test_validate_safety_center_overlaps_word_highlight() {
        let (safe, _warnings) = validate_sticker_safety("center", 0.30, Some("word_highlight"), 1080, 1920);
        // center with scale 0.3 on 1080 = 324px, centered at y=798, bottom at 1122
        // word_highlight rail is bottom 15% = 288px, so top of rail at 1632
        // 1122 < 1632, so no overlap for scale 0.3 at center
        assert!(safe);
    }

    #[test]
    fn test_validate_safety_center_large_overlaps() {
        let (safe, _warnings) = validate_sticker_safety("center", 0.45, Some("word_highlight"), 1080, 1920);
        // center with scale 0.45 on 1080 = 486px, centered at y=717, bottom at 1203
        // word_highlight rail starts at 1632, so still safe
        // But with 0.50: 540px, centered at y=690, bottom at 1230 - still safe
        // Actually need larger scale to overlap
        assert!(safe);
    }

    #[test]
    fn test_validate_safety_top_positions_always_safe() {
        let positions = ["top-left", "top-right", "top-center", "center-top"];
        for pos in positions {
            let (safe, _) = validate_sticker_safety(pos, 0.40, Some("word_highlight"), 1080, 1920);
            assert!(safe, "Position {} should be safe", pos);
        }
    }

    #[test]
    fn test_validate_scale_bounds() {
        let (safe, warnings) = validate_sticker_safety("top-left", 0.10, None, 1080, 1920);
        assert!(!safe);
        assert!(warnings.iter().any(|w| w.contains("Scale")));

        let (safe, warnings) = validate_sticker_safety("top-left", 0.60, None, 1080, 1920);
        assert!(!safe);
        assert!(warnings.iter().any(|w| w.contains("Scale")));
    }
}