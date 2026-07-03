//! SVG sticker puppet rendering for from-scratch video creation.
//!
//! Generates HyperFrames HTML compositions that animate an SVG puppet's
//! mouth based on WAV amplitude data. The mouth scaleY is driven by
//! per-frame RMS amplitude, producing lip-sync without any ML models.
//!
//! The sticker is rendered as a transparent overlay (WebM with alpha)
//! that is composited over the background by FFmpeg.

use crate::amplitude::AmplitudeTrack;
use serde::{Deserialize, Serialize};

/// Configuration for a sticker preset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StickerPreset {
    pub name: String,
    pub description: String,
    pub mouth_element_id: String,
    pub left_eye_id: String,
    pub right_eye_id: String,
    pub head_id: String,
    pub body_id: String,
    pub mouth_shapes: std::collections::HashMap<String, String>,
    pub emotes: std::collections::HashMap<String, String>,
    pub blink_rate_ms: u64,
    pub blink_duration_ms: u64,
    pub idle_bob_amplitude_px: f64,
    pub idle_bob_period_ms: u64,
    pub mouth_scale_range: [f64; 2],
    pub default_colors: std::collections::HashMap<String, String>,
}

/// Generate a HyperFrames HTML composition for an animated sticker.
///
/// The composition:
/// - Loads the SVG puppet inline
/// - Creates a GSAP timeline that animates the mouth scaleY per frame
/// - Adds eye blink animations at the preset's blink rate
/// - Adds idle body bob animation
/// - The timeline is paused and registered for HF's seek-driven runtime
pub fn generate_sticker_composition(
    puppet_svg: &str,
    preset: &StickerPreset,
    amplitude: &AmplitudeTrack,
    position: &str,
    scale: f64,
    canvas_width: u32,
    canvas_height: u32,
) -> String {
    // Calculate position
    let (pos_x, pos_y) = parse_position(position, canvas_width, canvas_height, scale);

    // Calculate sticker dimensions (400x400 SVG scaled)
    let sticker_size = (canvas_width as f64 * scale) as u32;
    let scale_factor = sticker_size as f64 / 400.0;

    // Build GSAP timeline tweens for mouth animation
    let mut mouth_tweens = String::new();
    let [min_scale, max_scale] = preset.mouth_scale_range;
    let scale_range = max_scale - min_scale;

    for (frame_idx, &amp) in amplitude.frames.iter().enumerate() {
        let time_s = frame_idx as f64 / amplitude.fps as f64;
        // Map amplitude to scaleY: 0.0 → min_scale, 1.0 → max_scale
        let scale_y = min_scale + (amp as f64) * scale_range;
        mouth_tweens.push_str(&format!(
            "  tl.to('#{}', {{ scaleY: {:.4}, duration: 0.001, ease: 'none' }}, {:.4});\n",
            preset.mouth_element_id, scale_y, time_s
        ));
    }

    // Build blink animation
    let mut blink_tweens = String::new();
    if amplitude.duration_ms > 0 {
        let mut blink_time = 0.0;
        while blink_time < amplitude.duration_ms as f64 / 1000.0 {
            let blink_end = blink_time + preset.blink_duration_ms as f64 / 1000.0;
            // Close eyes
            blink_tweens.push_str(&format!(
                "  tl.to('#{}, #{}', {{ scaleY: 0.1, duration: {:.3}, ease: 'power2.in' }}, {:.3});\n",
                preset.left_eye_id, preset.right_eye_id,
                preset.blink_duration_ms as f64 / 2000.0,
                blink_time
            ));
            // Open eyes
            blink_tweens.push_str(&format!(
                "  tl.to('#{}, #{}', {{ scaleY: 1, duration: {:.3}, ease: 'power2.out' }}, {:.3});\n",
                preset.left_eye_id, preset.right_eye_id,
                preset.blink_duration_ms as f64 / 2000.0,
                blink_end
            ));
            blink_time += preset.blink_rate_ms as f64 / 1000.0;
        }
    }

    // Build idle bob animation
    let bob_tweens = if preset.idle_bob_amplitude_px > 0.0 && amplitude.duration_ms > 0 {
        let total_s = amplitude.duration_ms as f64 / 1000.0;
        let period_s = preset.idle_bob_period_ms as f64 / 1000.0;
        let cycles = (total_s / period_s).ceil() as usize;
        let mut tweens = String::new();
        for i in 0..cycles {
            let t = i as f64 * period_s;
            let y = preset.idle_bob_amplitude_px;
            tweens.push_str(&format!(
                "  tl.to('#puppet', {{ y: '+={:.1}', duration: {:.3}, ease: 'sine.inOut' }}, {:.3});\n",
                y, period_s / 2.0, t
            ));
            tweens.push_str(&format!(
                "  tl.to('#puppet', {{ y: '-={:.1}', duration: {:.3}, ease: 'sine.inOut' }}, {:.3});\n",
                y, period_s / 2.0, t + period_s / 2.0
            ));
        }
        tweens
    } else {
        String::new()
    };

    let duration_s = amplitude.duration_ms as f64 / 1000.0;

    format!(
r#"<!DOCTYPE html>
<html lang="en"
  data-composition-id="sticker"
  data-start="0"
  data-duration="{duration_s}"
  data-fps="{fps}"
  data-width="{canvas_width}"
  data-height="{canvas_height}"
>
<head>
  <meta charset="utf-8" />
  <title>Sticker Overlay</title>
  <style>
    * {{ margin: 0; padding: 0; box-sizing: border-box; }}
    html, body {{ overflow: hidden; background: transparent; }}
    #stage {{
      position: relative;
      width: {canvas_width}px;
      height: {canvas_height}px;
      background: transparent;
      overflow: hidden;
    }}
    #sticker-container {{
      position: absolute;
      left: {pos_x}px;
      top: {pos_y}px;
      width: {sticker_size}px;
      height: {sticker_size}px;
      transform: scale({scale_factor});
      transform-origin: top left;
    }}
    #puppet {{
      width: 400px;
      height: 400px;
    }}
    #{mouth_id} {{
      transform-origin: center;
      transform: scaleY(0.1);
    }}
    #{left_eye}, #{right_eye} {{
      transform-origin: center;
    }}
  </style>
</head>
<body>
  <div id="stage">
    <div id="sticker-container">
      <div id="puppet">
        {puppet_svg}
      </div>
    </div>
  </div>

  <script src="https://cdnjs.cloudflare.com/ajax/libs/gsap/3.12.5/gsap.min.js"></script>
  <script>
    const tl = gsap.timeline({{ paused: true }});
{mouth_tweens}
{blink_tweens}
{bob_tweens}

    window.__timelines = window.__timelines || {{}};
    window.__timelines["sticker"] = tl;
  </script>
</body>
</html>"#,
        duration_s = duration_s,
        fps = amplitude.fps,
        canvas_width = canvas_width,
        canvas_height = canvas_height,
        pos_x = pos_x,
        pos_y = pos_y,
        sticker_size = 400,  // unscaled — the container handles scaling
        scale_factor = scale_factor,
        mouth_id = preset.mouth_element_id,
        left_eye = preset.left_eye_id,
        right_eye = preset.right_eye_id,
        puppet_svg = puppet_svg,
        mouth_tweens = mouth_tweens,
        blink_tweens = blink_tweens,
        bob_tweens = bob_tweens,
    )
}

/// Parse a position string (e.g. "top-left", "bottom-right") into (x, y) coordinates.
fn parse_position(position: &str, canvas_width: u32, canvas_height: u32, scale: f64) -> (f64, f64) {
    let sticker_size = canvas_width as f64 * scale;
    let margin = 40.0; // 40px margin from edge

    match position {
        "top-left" => (margin, margin),
        "top-right" => (canvas_width as f64 - sticker_size - margin, margin),
        "bottom-left" => (margin, canvas_height as f64 - sticker_size - margin),
        "bottom-right" => (canvas_width as f64 - sticker_size - margin, canvas_height as f64 - sticker_size - margin),
        "top-center" => ((canvas_width as f64 - sticker_size) / 2.0, margin),
        "bottom-center" => ((canvas_width as f64 - sticker_size) / 2.0, canvas_height as f64 - sticker_size - margin),
        "center" => ((canvas_width as f64 - sticker_size) / 2.0, (canvas_height as f64 - sticker_size) / 2.0),
        _ => (margin, margin), // default to top-left
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_preset() -> StickerPreset {
        StickerPreset {
            name: "default_person".to_string(),
            description: "Test".to_string(),
            mouth_element_id: "mouth".to_string(),
            left_eye_id: "left-eye".to_string(),
            right_eye_id: "right-eye".to_string(),
            head_id: "head".to_string(),
            body_id: "body".to_string(),
            mouth_shapes: std::collections::HashMap::new(),
            emotes: std::collections::HashMap::new(),
            blink_rate_ms: 4000,
            blink_duration_ms: 150,
            idle_bob_amplitude_px: 5.0,
            idle_bob_period_ms: 2000,
            mouth_scale_range: [0.1, 1.0],
            default_colors: std::collections::HashMap::new(),
        }
    }

    fn test_amplitude() -> AmplitudeTrack {
        AmplitudeTrack {
            frames: vec![0.0, 0.5, 1.0, 0.5, 0.0],
            fps: 30,
            duration_ms: 167, // ~5 frames at 30fps
        }
    }

    #[test]
    fn test_generate_sticker_composition() {
        let svg = r#"<svg viewBox="0 0 400 400"><circle id="head" cx="200" cy="180" r="120"/></svg>"#;
        let preset = test_preset();
        let amp = test_amplitude();

        let html = generate_sticker_composition(svg, &preset, &amp, "top-left", 0.25, 1080, 1920);

        assert!(html.contains("data-composition-id=\"sticker\""));
        assert!(html.contains("data-fps=\"30\""));
        assert!(html.contains("gsap.timeline"));
        assert!(html.contains("#mouth"));
        assert!(html.contains("scaleY"));
        // top-left position → x=40, y=40
        assert!(html.contains("left: 40px"));
        assert!(html.contains("top: 40px"));
    }

    #[test]
    fn test_parse_position_top_left() {
        let (x, y) = parse_position("top-left", 1080, 1920, 0.25);
        assert_eq!(x, 40.0);
        assert_eq!(y, 40.0);
    }

    #[test]
    fn test_parse_position_top_right() {
        let (x, y) = parse_position("top-right", 1080, 1920, 0.25);
        // sticker_size = 1080 * 0.25 = 270
        // x = 1080 - 270 - 40 = 770
        assert_eq!(x, 770.0);
        assert_eq!(y, 40.0);
    }

    #[test]
    fn test_parse_position_bottom_right() {
        let (x, y) = parse_position("bottom-right", 1080, 1920, 0.25);
        assert_eq!(x, 770.0);
        assert_eq!(y, 1610.0); // 1920 - 270 - 40
    }

    #[test]
    fn test_parse_position_center() {
        let (x, y) = parse_position("center", 1080, 1920, 0.25);
        // sticker_size = 270
        // x = (1080 - 270) / 2 = 405
        // y = (1920 - 270) / 2 = 825
        assert_eq!(x, 405.0);
        assert_eq!(y, 825.0);
    }

    #[test]
    fn test_parse_position_invalid_defaults_top_left() {
        let (x, y) = parse_position("invalid", 1080, 1920, 0.25);
        assert_eq!(x, 40.0);
        assert_eq!(y, 40.0);
    }

    #[test]
    fn test_mouth_tweens_generated() {
        let svg = "<svg></svg>";
        let preset = test_preset();
        let amp = test_amplitude();

        let html = generate_sticker_composition(svg, &preset, &amp, "center", 0.25, 1080, 1920);

        // Should have 5 mouth tweens (one per amplitude frame)
        let mouth_tween_count = html.matches("#mouth").count();
        assert!(mouth_tween_count >= 5, "Expected at least 5 mouth references, got {}", mouth_tween_count);
    }

    #[test]
    fn test_blink_tweens_generated() {
        let svg = "<svg></svg>";
        let preset = test_preset();
        let amp = AmplitudeTrack {
            frames: vec![0.0; 120], // 4 seconds at 30fps
            fps: 30,
            duration_ms: 4000,
        };

        let html = generate_sticker_composition(svg, &preset, &amp, "center", 0.25, 1080, 1920);

        // Should have blink animations (4s / 4s blink rate = 1 blink = 2 tweens)
        assert!(html.contains("scaleY: 0.1"), "Should have blink close tween");
    }
}
