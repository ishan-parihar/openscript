//! Design Token Engine — OKLCH Color, WCAG Contrast, Typography & Spacing.
//!
//! A single-module design system ported from Python (carousel-mcp).
//! Provides:
//!   - sRGB ↔ OKLCH color space conversions (Björn Ottosson's Oklab matrices)
//!   - WCAG 2.x relative luminance & contrast ratio calculations
//!   - 14-token palette derivation from a single primary color with auto-contrast-fixing
//!   - Modular type scale (6 levels) from configurable base size & ratio
//!   - 8pt spacing scale
//!   - 7 curated Google Font pairings
//!   - DesignTokens struct with CSS variable and JSON output
//!
//! Uses stdlib + serde_json only — no external color conversion crates.

// ─── Color Space: sRGB ↔ Linear ─────────────────────────────────────────────

/// Gamma expansion: sRGB [0,1] → linear RGB [0,1].
pub fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Gamma compression: linear RGB [0,1] → sRGB [0,1].
pub fn linear_to_srgb(c: f64) -> f64 {
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

// ─── Color Space: Linear RGB ↔ Oklab ────────────────────────────────────────

// Björn Ottosson's Oklab conversion matrices (2020).
// Linear RGB → LMS (long/medium/short cone responses)
const _LMS_FROM_LINEAR: [[f64; 3]; 3] = [
    [0.4122214708, 0.5363325363, 0.0514459929],
    [0.2119034982, 0.6806995451, 0.1073969566],
    [0.0883024619, 0.2817188376, 0.6299787005],
];

// LMS (after cube root) → OkLab (l, a, b)
const _LAB_FROM_LMS: [[f64; 3]; 3] = [
    [0.2104542553, 0.7936177850, -0.0040720468],
    [1.9779984951, -2.4285922050, 0.4505937099],
    [0.0259040371, 0.7827717662, -0.8086757660],
];

// Inverse: OkLab → LMS (before cube root)
const _LMS_FROM_LAB: [[f64; 3]; 3] = [
    [1.0000000000, 0.3963377774, 0.2158037573],
    [1.0000000000, -0.1055613458, -0.0638541728],
    [1.0000000000, -0.0894841775, -1.2914855480],
];

// Inverse: LMS (after cube root) → Linear RGB
const _LINEAR_FROM_LMS: [[f64; 3]; 3] = [
    [4.0767416621, -3.3077115913, 0.2309699292],
    [-1.2684380046, 2.6097574011, -0.3413193965],
    [-0.0041960863, -0.7034186147, 1.7076147010],
];

/// Multiply a 3×3 matrix by a 3-vector.
fn _matmul3x3(m: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

/// Real cube root (handles negatives).
fn _cbrt(x: f64) -> f64 {
    if x >= 0.0 {
        x.powf(1.0 / 3.0)
    } else {
        -((-x).powf(1.0 / 3.0))
    }
}

/// Convert sRGB [0,1] to OKLCH.
///
/// Returns (L, C, H) where:
///   L ∈ [0, 1]      — lightness
///   C ∈ [0, ~0.4]   — chroma
///   H ∈ [0, 360)    — hue in degrees
pub fn rgb_to_oklch(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    // sRGB → linear
    let lr = srgb_to_linear(r);
    let lg = srgb_to_linear(g);
    let lb = srgb_to_linear(b);

    // Linear RGB → LMS
    let lms = _matmul3x3(&_LMS_FROM_LINEAR, [lr, lg, lb]);

    // Cube root LMS
    let lp = _cbrt(lms[0]);
    let mp = _cbrt(lms[1]);
    let sp = _cbrt(lms[2]);

    // LMS' → OkLab
    let lab = _matmul3x3(&_LAB_FROM_LMS, [lp, mp, sp]);
    let l = lab[0];
    let a = lab[1];
    let b_val = lab[2];

    // OkLab → OKLCH
    let c = (a * a + b_val * b_val).sqrt();
    let mut h = b_val.atan2(a).to_degrees() % 360.0;
    if h < 0.0 {
        h += 360.0;
    }

    (l, c, h)
}

/// Convert OKLCH to sRGB [0,1].
///
/// Clamps final RGB values to [0, 1] to handle out-of-gamut colors.
pub fn oklch_to_rgb(l: f64, c: f64, h: f64) -> (f64, f64, f64) {
    // OKLCH → OkLab
    let h_rad = h.to_radians();
    let a = c * h_rad.cos();
    let b_val = c * h_rad.sin();

    // OkLab → LMS' (before cube root)
    let lms_prime = _matmul3x3(&_LMS_FROM_LAB, [l, a, b_val]);

    // Cube to get LMS
    let lm = lms_prime[0].powi(3);
    let mm = lms_prime[1].powi(3);
    let sm = lms_prime[2].powi(3);

    // LMS → linear RGB
    let linear = _matmul3x3(&_LINEAR_FROM_LMS, [lm, mm, sm]);

    // Linear → sRGB, clamped
    (
        linear_to_srgb(linear[0]).max(0.0).min(1.0),
        linear_to_srgb(linear[1]).max(0.0).min(1.0),
        linear_to_srgb(linear[2]).max(0.0).min(1.0),
    )
}

// ─── Hex Helpers ────────────────────────────────────────────────────────────

/// Normalize any valid 6-digit hex to uppercase with leading #.
pub fn _normalize_hex(hex_color: &str) -> Result<String, String> {
    let hex_color = hex_color.trim();
    let hex_color = if hex_color.starts_with('#') {
        hex_color.to_string()
    } else {
        format!("#{}", hex_color)
    };

    let stripped = hex_color.strip_prefix('#').unwrap_or(&hex_color);
    if stripped.len() != 6 || !stripped.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("Invalid hex color: {}", hex_color));
    }

    Ok(hex_color.to_uppercase())
}

/// Hex string → (R, G, B) each in [0, 1].
pub fn _hex_to_rgb01(hex_color: &str) -> Result<(f64, f64, f64), String> {
    let h = hex_color.trim().strip_prefix('#').unwrap_or(hex_color);
    if h.len() != 6 {
        return Err(format!("Invalid hex color: {}", hex_color));
    }
    let r = u8::from_str_radix(&h[0..2], 16).map_err(|e| e.to_string())? as f64 / 255.0;
    let g = u8::from_str_radix(&h[2..4], 16).map_err(|e| e.to_string())? as f64 / 255.0;
    let b = u8::from_str_radix(&h[4..6], 16).map_err(|e| e.to_string())? as f64 / 255.0;
    Ok((r, g, b))
}

/// (R, G, B) each in [0, 1] → uppercase hex string.
pub fn _rgb01_to_hex(r: f64, g: f64, b: f64) -> String {
    let ri = (r * 255.0).round().max(0.0).min(255.0) as u8;
    let gi = (g * 255.0).round().max(0.0).min(255.0) as u8;
    let bi = (b * 255.0).round().max(0.0).min(255.0) as u8;
    format!("#{:02X}{:02X}{:02X}", ri, gi, bi)
}

/// OKLCH → uppercase hex string (with gamut clamping).
fn _oklch_to_hex(l: f64, c: f64, h: f64) -> String {
    let (r, g, b) = oklch_to_rgb(l, c, h);
    _rgb01_to_hex(r, g, b)
}

// ─── WCAG Contrast ──────────────────────────────────────────────────────────

/// Calculate WCAG 2.x relative luminance for a hex color.
///
/// Per WCAG 2.1 spec:
///   L = 0.2126 * R_linear + 0.7152 * G_linear + 0.0722 * B_linear
pub fn relative_luminance(hex_color: &str) -> f64 {
    let (r, g, b) = _hex_to_rgb01(hex_color).unwrap_or((0.0, 0.0, 0.0));
    let rl = srgb_to_linear(r);
    let gl = srgb_to_linear(g);
    let bl = srgb_to_linear(b);
    0.2126 * rl + 0.7152 * gl + 0.0722 * bl
}

/// Calculate WCAG contrast ratio between two hex colors.
///
/// Returns ratio (lighter / darker) where:
///   21.0 = maximum contrast (white on black)
///    1.0 = identical colors
pub fn contrast_ratio(color1_hex: &str, color2_hex: &str) -> f64 {
    let l1 = relative_luminance(color1_hex);
    let l2 = relative_luminance(color2_hex);
    let lighter = l1.max(l2);
    let darker = l1.min(l2);
    (lighter + 0.05) / (darker + 0.05)
}

/// Check if a foreground/background pair passes WCAG AA.
///
/// AA requirements:
///   - Normal text:   4.5:1 minimum
///   - Large text:    3:1 minimum (>18px or >14px bold)
pub fn passes_wcag_aa(fg_hex: &str, bg_hex: &str, large_text: bool) -> bool {
    let threshold = if large_text { 3.0 } else { 4.5 };
    contrast_ratio(fg_hex, bg_hex) >= threshold
}

// ─── Font Pairings ──────────────────────────────────────────────────────────

/// A curated heading + body font pairing from Google Fonts.
#[derive(Debug, Clone)]
pub struct FontPairing {
    pub heading_font: String,
    pub body_font: String,
    pub google_fonts_url: String,
    pub heading_class: String,
    pub body_class: String,
    pub style_description: String,
}

impl FontPairing {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "heading_font": self.heading_font,
            "body_font": self.body_font,
            "google_fonts_url": self.google_fonts_url,
            "heading_class": self.heading_class,
            "body_class": self.body_class,
            "style_description": self.style_description,
        })
    }
}

struct _FontPairingConfig {
    heading_font: &'static str,
    body_font: &'static str,
    heading_weights: &'static [i32],
    body_weights: &'static [i32],
    style_description: &'static str,
    heading_class: &'static str,
    body_class: &'static str,
}

const _FONT_PAIRINGS: &[(&str, _FontPairingConfig)] = &[
    (
        "editorial",
        _FontPairingConfig {
            heading_font: "Playfair Display",
            body_font: "DM Sans",
            heading_weights: &[300, 600],
            body_weights: &[400, 500, 600],
            style_description: "Editorial / premium feel",
            heading_class: "serif",
            body_class: "sans",
        },
    ),
    (
        "modern",
        _FontPairingConfig {
            heading_font: "Plus Jakarta Sans",
            body_font: "Plus Jakarta Sans",
            heading_weights: &[700],
            body_weights: &[400, 500, 600],
            style_description: "Modern / clean",
            heading_class: "sans",
            body_class: "sans",
        },
    ),
    (
        "warm",
        _FontPairingConfig {
            heading_font: "Lora",
            body_font: "Nunito Sans",
            heading_weights: &[400, 600],
            body_weights: &[400, 500, 600],
            style_description: "Warm / approachable",
            heading_class: "serif",
            body_class: "sans",
        },
    ),
    (
        "technical",
        _FontPairingConfig {
            heading_font: "Space Grotesk",
            body_font: "Space Grotesk",
            heading_weights: &[300, 600],
            body_weights: &[400, 500],
            style_description: "Technical / sharp",
            heading_class: "sans",
            body_class: "sans",
        },
    ),
    (
        "bold",
        _FontPairingConfig {
            heading_font: "Fraunces",
            body_font: "Outfit",
            heading_weights: &[300, 600],
            body_weights: &[400, 500, 600],
            style_description: "Bold / expressive",
            heading_class: "serif",
            body_class: "sans",
        },
    ),
    (
        "classic",
        _FontPairingConfig {
            heading_font: "Libre Baskerville",
            body_font: "Work Sans",
            heading_weights: &[400, 700],
            body_weights: &[400, 500, 600],
            style_description: "Classic / trustworthy",
            heading_class: "serif",
            body_class: "sans",
        },
    ),
    (
        "rounded",
        _FontPairingConfig {
            heading_font: "Bricolage Grotesque",
            body_font: "Bricolage Grotesque",
            heading_weights: &[600],
            body_weights: &[400, 500],
            style_description: "Rounded / friendly",
            heading_class: "sans",
            body_class: "sans",
        },
    ),
];

fn _build_google_fonts_url(cfg: &_FontPairingConfig) -> String {
    let heading_families = format!(
        "{}:wght@{}",
        cfg.heading_font,
        cfg.heading_weights
            .iter()
            .map(|w| w.to_string())
            .collect::<Vec<_>>()
            .join(",")
    );
    let url = if cfg.heading_font != cfg.body_font {
        let body_families = format!(
            "{}:wght@{}",
            cfg.body_font,
            cfg.body_weights
                .iter()
                .map(|w| w.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );
        format!(
            "https://fonts.googleapis.com/css2?family={}&family={}&display=swap",
            heading_families, body_families
        )
    } else {
        format!(
            "https://fonts.googleapis.com/css2?family={}&display=swap",
            heading_families
        )
    };
    url
}

/// Get a heading + body font pairing for a given brand style.
pub fn get_font_pairing(style: &str) -> Result<FontPairing, String> {
    let style = style.trim().to_lowercase();
    let cfg = _FONT_PAIRINGS
        .iter()
        .find(|(name, _)| *name == style)
        .ok_or_else(|| {
            let available = list_font_pairings().join(", ");
            format!("Unknown font style: '{}'. Available: {}", style, available)
        })?;
    let cfg = &cfg.1;

    let google_fonts_url = _build_google_fonts_url(cfg);

    Ok(FontPairing {
        heading_font: cfg.heading_font.to_string(),
        body_font: cfg.body_font.to_string(),
        google_fonts_url,
        heading_class: cfg.heading_class.to_string(),
        body_class: cfg.body_class.to_string(),
        style_description: cfg.style_description.to_string(),
    })
}

/// Return sorted list of available font pairing style names.
pub fn list_font_pairings() -> Vec<&'static str> {
    let mut names: Vec<&str> = _FONT_PAIRINGS.iter().map(|(name, _)| *name).collect();
    names.sort_unstable();
    names
}

// ─── Modular Type Scale ─────────────────────────────────────────────────────

/// Generate a 6-level modular type scale.
///
/// Each level is derived mathematically from base_size and ratio:
///   display  = base × ratio³
///   headline = base × ratio²
///   title    = base × ratio¹
///   body     = base
///   caption  = base / ratio
///   micro    = base / ratio²
pub fn generate_type_scale(base: i32, ratio: f64) -> serde_json::Value {
    let base_f = base as f64;
    serde_json::json!({
        "display": {
            "font_size": (base_f * ratio.powi(3)).round() as i32,
            "line_height": 1.08,
            "letter_spacing": -0.02,
            "font_weight": 600,
        },
        "headline": {
            "font_size": (base_f * ratio.powi(2)).round() as i32,
            "line_height": 1.12,
            "letter_spacing": -0.015,
            "font_weight": 600,
        },
        "title": {
            "font_size": (base_f * ratio).round() as i32,
            "line_height": 1.18,
            "letter_spacing": -0.01,
            "font_weight": 600,
        },
        "body": {
            "font_size": base,
            "line_height": 1.55,
            "letter_spacing": 0.0,
            "font_weight": 400,
        },
        "caption": {
            "font_size": (base_f / ratio).round() as i32,
            "line_height": 1.40,
            "letter_spacing": 0.02,
            "font_weight": 600,
        },
        "micro": {
            "font_size": (base_f / ratio.powi(2)).round() as i32,
            "line_height": 1.35,
            "letter_spacing": 0.04,
            "font_weight": 500,
        },
    })
}

// ─── Spacing Scale ──────────────────────────────────────────────────────────

/// Generate an 8pt spacing scale.
pub fn generate_spacing_scale() -> serde_json::Value {
    let base = 8;
    serde_json::json!({
        "0": 0,
        "1": base * 1,
        "2": base * 2,
        "3": base * 3,
        "4": base * 4,
        "5": base * 5,
        "6": base * 6,
        "7": base * 7,
        "8": base * 8,
        "10": base * 10,
        "12": base * 12,
    })
}

// ─── Palette Derivation ─────────────────────────────────────────────────────

/// Auto-clamp text color OKLCH lightness until WCAG AA passes.
fn _auto_clamp_text(
    base_l: f64,
    c: f64,
    h: f64,
    bg_hex: &str,
    direction: &str,
    step: f64,
    min_l: f64,
    max_l: f64,
    target_ratio: f64,
) -> String {
    let mut current_l = base_l;
    for _ in 0..100 {
        let color_hex = _oklch_to_hex(current_l, c, h);
        let ratio = contrast_ratio(&color_hex, bg_hex);
        if ratio >= target_ratio {
            return color_hex;
        }
        if direction == "darken" {
            current_l -= step;
            if current_l < min_l {
                break;
            }
        } else {
            current_l += step;
            if current_l > max_l {
                break;
            }
        }
    }
    // Return best effort even if target not met
    _oklch_to_hex(current_l, c, h)
}

/// Auto-clamp border color OKLCH lightness until minimum contrast is met.
fn _auto_clamp_border(
    base_l: f64,
    c: f64,
    h: f64,
    bg_hex: &str,
    min_ratio: f64,
    direction: &str,
    step: f64,
    min_l: f64,
    max_l: f64,
) -> String {
    let mut current_l = base_l;
    for _ in 0..100 {
        let color_hex = _oklch_to_hex(current_l, c, h);
        let ratio = contrast_ratio(&color_hex, bg_hex);
        if ratio >= min_ratio {
            return color_hex;
        }
        if direction == "darken" {
            current_l -= step;
            if current_l < min_l {
                break;
            }
        } else {
            current_l += step;
            if current_l > max_l {
                break;
            }
        }
    }
    _oklch_to_hex(current_l, c, h)
}

/// Build a contrast report for all text/background and border/background pairs.
fn _build_contrast_report(
    text_primary: &str,
    text_secondary: &str,
    text_on_dark: &str,
    text_on_dark_secondary: &str,
    surface_light: &str,
    surface_dark: &str,
    border_light: &str,
    border_dark: &str,
) -> serde_json::Value {
    serde_json::json!({
        "text_primary on surface_light": {
            "ratio": (contrast_ratio(text_primary, surface_light) * 100.0).round() / 100.0,
            "passes": passes_wcag_aa(text_primary, surface_light, false),
        },
        "text_secondary on surface_light": {
            "ratio": (contrast_ratio(text_secondary, surface_light) * 100.0).round() / 100.0,
            "passes": passes_wcag_aa(text_secondary, surface_light, false),
        },
        "text_on_dark on surface_dark": {
            "ratio": (contrast_ratio(text_on_dark, surface_dark) * 100.0).round() / 100.0,
            "passes": passes_wcag_aa(text_on_dark, surface_dark, false),
        },
        "text_on_dark_secondary on surface_dark": {
            "ratio": (contrast_ratio(text_on_dark_secondary, surface_dark) * 100.0).round() / 100.0,
            "passes": passes_wcag_aa(text_on_dark_secondary, surface_dark, false),
        },
        "border_light on surface_light": {
            "ratio": (contrast_ratio(border_light, surface_light) * 100.0).round() / 100.0,
            "passes": contrast_ratio(border_light, surface_light) >= 1.5,
        },
        "border_dark on surface_dark": {
            "ratio": (contrast_ratio(border_dark, surface_dark) * 100.0).round() / 100.0,
            "passes": contrast_ratio(border_dark, surface_dark) >= 1.5,
        },
    })
}

/// Design token set — colors, typography, spacing, and validation.
///
/// All color tokens are uppercase hex strings (or CSS gradient string for
/// the `gradient` token). The `contrast_report` documents every
/// text/background pair's WCAG compliance.
#[derive(Debug, Clone)]
pub struct DesignTokens {
    // Colors
    pub primary: String,
    pub primary_light: String,
    pub primary_dark: String,
    pub surface_light: String,
    pub surface_dark: String,
    pub text_primary: String,
    pub text_secondary: String,
    pub text_on_dark: String,
    pub text_on_dark_secondary: String,
    pub border_light: String,
    pub border_dark: String,
    pub accent: String,
    pub gradient: String,
    pub temperature: String,
    // Typography
    pub heading_font: String,
    pub body_font: String,
    pub google_fonts_url: String,
    pub type_scale: serde_json::Value,
    // Spacing
    pub spacing: serde_json::Value,
    // Motion Timing
    pub timing: serde_json::Value,
    // Validation
    pub contrast_report: serde_json::Value,
}

impl DesignTokens {
    /// Return a CSS :root block with all color tokens as custom properties.
    pub fn to_css_variables(&self) -> String {
        let mut lines = Vec::new();
        lines.push(":root {".to_string());

        let color_tokens: &[(&str, &str)] = &[
            ("primary", &self.primary),
            ("primary-light", &self.primary_light),
            ("primary-dark", &self.primary_dark),
            ("surface-light", &self.surface_light),
            ("surface-dark", &self.surface_dark),
            ("text-primary", &self.text_primary),
            ("text-secondary", &self.text_secondary),
            ("text-on-dark", &self.text_on_dark),
            ("text-on-dark-secondary", &self.text_on_dark_secondary),
            ("border-light", &self.border_light),
            ("border-dark", &self.border_dark),
            ("accent", &self.accent),
            ("gradient", &self.gradient),
        ];

        for (name, value) in color_tokens {
            lines.push(format!("  --{}: {};", name, value));
        }

        lines.push("}".to_string());
        lines.join("\n")
    }

    /// Return font pairing info as JSON for MCP output.
    pub fn font_pairing_to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "heading_font": self.heading_font,
            "body_font": self.body_font,
            "google_fonts_url": self.google_fonts_url,
        })
    }

    /// Return a flat JSON object of color + typography tokens for MCP output.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "primary": self.primary,
            "primary_light": self.primary_light,
            "primary_dark": self.primary_dark,
            "surface_light": self.surface_light,
            "surface_dark": self.surface_dark,
            "text_primary": self.text_primary,
            "text_secondary": self.text_secondary,
            "text_on_dark": self.text_on_dark,
            "text_on_dark_secondary": self.text_on_dark_secondary,
            "border_light": self.border_light,
            "border_dark": self.border_dark,
            "accent": self.accent,
            "gradient": self.gradient,
            "temperature": self.temperature,
            "heading_font": self.heading_font,
            "body_font": self.body_font,
            "google_fonts_url": self.google_fonts_url,
            "type_scale": self.type_scale,
            "spacing": self.spacing,
            "timing": self.timing,
            "contrast_report": self.contrast_report,
        })
    }
}

/// Derive a complete 14-token design palette from a single primary color.
///
/// Uses OKLCH color space for perceptually uniform derivations.
/// All text/background pairs are auto-validated against WCAG AA (4.5:1).
pub fn derive_palette(primary_hex: &str, style: &str) -> Result<DesignTokens, String> {
    let primary = _normalize_hex(primary_hex)?;
    // Style is available for future extension; OKLCH derivation is universal

    // Convert primary to OKLCH
    let (pr, pg, pb) = _hex_to_rgb01(&primary)?;
    let (pl, pc, ph) = rgb_to_oklch(pr, pg, pb);

    // 1. primary_light: L+0.15, same C,H (clamped to L<=0.97)
    let primary_light = _oklch_to_hex((pl + 0.15).min(0.97), pc, ph);

    // 2. primary_dark: L-0.15, same C,H (clamped to L>=0.05)
    let primary_dark = _oklch_to_hex((pl - 0.15).max(0.05), pc, ph);

    // 3. surface_light: L=0.97, C=0.01, H=primary_H
    let surface_light = _oklch_to_hex(0.97, 0.01, ph);

    // 4. surface_dark: L=0.08, C=0.02, H=primary_H
    let surface_dark = _oklch_to_hex(0.08, 0.02, ph);

    // 5. text_primary: OKLCH L=0.15, same H, low C=0.01
    let text_primary = _oklch_to_hex(0.15, 0.01, ph);

    // 6. text_secondary: start at L=0.50, auto-clamp if needed
    let text_secondary = _auto_clamp_text(
        0.50,
        0.01,
        ph,
        &surface_light,
        "darken",
        0.01,
        0.05,
        0.97,
        4.5,
    );

    // 7. text_on_dark: OKLCH L=0.95, same H, low C=0.01
    let text_on_dark = _oklch_to_hex(0.95, 0.01, ph);

    // 8. text_on_dark_secondary: start at L=0.70, auto-clamp if needed
    let text_on_dark_secondary = _auto_clamp_text(
        0.70,
        0.02,
        ph,
        &surface_dark,
        "lighten",
        0.01,
        0.05,
        0.97,
        4.5,
    );

    // 9. border_light: contrast >= 1.5:1 vs surface_light
    let border_light = _auto_clamp_border(
        0.85,
        0.015,
        ph,
        &surface_light,
        1.5,
        "darken",
        0.01,
        0.05,
        0.97,
    );

    // 10. border_dark: contrast >= 1.5:1 vs surface_dark
    let border_dark = _auto_clamp_border(
        0.22,
        0.03,
        ph,
        &surface_dark,
        1.5,
        "lighten",
        0.01,
        0.05,
        0.97,
    );

    // 11. accent: same L as primary, H+180
    let accent = _oklch_to_hex(pl, pc, (ph + 180.0) % 360.0);

    // 12. gradient: CSS gradient string
    let gradient = format!(
        "linear-gradient(165deg, {} 0%, {} 50%, {} 100%)",
        primary_dark, primary, primary_light
    );

    // 13. temperature: from primary hue
    let temperature = if ph >= 180.0 { "warm" } else { "cool" }.to_string();

    // Contrast Report
    let contrast_report = _build_contrast_report(
        &text_primary,
        &text_secondary,
        &text_on_dark,
        &text_on_dark_secondary,
        &surface_light,
        &surface_dark,
        &border_light,
        &border_dark,
    );

    // Typography
    let fonts = get_font_pairing(style)?;
    let type_scale = generate_type_scale(28, 1.250);
    let spacing = generate_spacing_scale();
    let timing = generate_motion_timing();

    Ok(DesignTokens {
        primary,
        primary_light,
        primary_dark,
        surface_light,
        surface_dark,
        text_primary,
        text_secondary,
        text_on_dark,
        text_on_dark_secondary,
        border_light,
        border_dark,
        accent,
        gradient,
        temperature,
        heading_font: fonts.heading_font,
        body_font: fonts.body_font,
        google_fonts_url: fonts.google_fonts_url,
        type_scale,
        spacing,
        timing,
        contrast_report,
    })
}

/// Generate motion-native timing presets in frames (at 30fps).
///
/// These are Remotion-native timing values — not milliseconds — so agents
/// use them directly in Sequence `from`/`durationInFrames` props.
pub fn generate_motion_timing() -> serde_json::Value {
    serde_json::json!({
        "speed": {
            "micro": 8,
            "fast": 15,
            "medium": 30,
            "slow": 60,
            "deliberate": 90,
        },
        "stagger": {
            "tight": 4,
            "standard": 8,
            "relaxed": 15,
        },
        "easing": {
            "in_out": "Easing.bezier(0.42, 0, 0.58, 1)",
            "snappy": "Easing.cubic",
            "bounce": "Easing.elastic(1.5)",
            "smooth": "Easing.sin",
            "linear": "Easing.linear",
        },
        "fps": 30,
        "frame_duration_ms": 33.33,
    })
}
