//! From-scratch video creation script schema.
//!
//! A `ScriptSpec` is the single source of truth for AI-agent-driven video
//! creation. It describes speakers, scenes, backgrounds, captions, music,
//! and output — every field is explicit to eliminate ambiguity.
//!
//! The schema is parsed by the `script.parse` MCP tool and consumed by
//! `script.to_timeline` / `script.to_video`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Top-level ScriptSpec
// ---------------------------------------------------------------------------

/// The complete specification for a from-scratch video.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ScriptSpec {
    /// Schema version identifier.
    #[serde(default = "default_schema")]
    pub schema: String,

    /// Human-readable title for the video.
    #[serde(default)]
    pub title: String,

    /// Output metadata (aspect, fps, resolution).
    #[serde(default)]
    pub meta: MetaSpec,

    /// TTS engine configuration.
    #[serde(default)]
    pub tts: TtsSpec,

    /// Speaker definitions (voice, visual preset, position).
    ///
    /// Keys are speaker IDs referenced by scenes. At least one speaker
    /// is required.
    pub speakers: HashMap<String, SpeakerSpec>,

    /// Background video configuration (gameplay, procedural, or static).
    #[serde(default)]
    pub background: BackgroundSpec,

    /// Background music configuration.
    #[serde(default)]
    pub music: Option<MusicSpec>,

    /// Caption styling.
    #[serde(default)]
    pub captions: CaptionsSpec,

    /// Speaker sticker layout on screen.
    #[serde(default = "default_speaker_layout")]
    pub speaker_layout: SpeakerLayout,

    /// Sticker behavior configuration.
    #[serde(default)]
    pub stickers: StickersSpec,

    /// The ordered list of scenes (the script content).
    pub scenes: Vec<SceneSpec>,

    /// Sound effects to place at specific times or triggers.
    #[serde(default)]
    pub sfx: Vec<SfxSpec>,

    /// Render output configuration.
    #[serde(default)]
    pub output: OutputSpec,

    /// When the script was created (auto-set if absent).
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
}

fn default_schema() -> String {
    "openscript-video/v1".to_string()
}

fn default_speaker_layout() -> SpeakerLayout {
    SpeakerLayout::SplitTop
}

// ---------------------------------------------------------------------------
// Meta
// ---------------------------------------------------------------------------

/// Output video metadata.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MetaSpec {
    /// Aspect ratio: "9:16", "16:9", "1:1".
    #[serde(default = "default_aspect")]
    pub aspect: String,

    /// Frames per second: 24, 30, or 60.
    #[serde(default = "default_fps")]
    pub fps: u32,

    /// Output width in pixels.
    #[serde(default = "default_width")]
    pub width: u32,

    /// Output height in pixels.
    #[serde(default = "default_height")]
    pub height: u32,

    /// Resolution label: "720p", "1080p", "4k".
    #[serde(default = "default_resolution")]
    pub resolution: String,
}

fn default_aspect() -> String {
    "9:16".to_string()
}
fn default_fps() -> u32 {
    30
}
fn default_width() -> u32 {
    1080
}
fn default_height() -> u32 {
    1920
}
fn default_resolution() -> String {
    "1080p".to_string()
}

impl Default for MetaSpec {
    fn default() -> Self {
        Self {
            aspect: default_aspect(),
            fps: default_fps(),
            width: default_width(),
            height: default_height(),
            resolution: default_resolution(),
        }
    }
}

// ---------------------------------------------------------------------------
// TTS
// ---------------------------------------------------------------------------

/// TTS engine configuration.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TtsSpec {
    /// Backend: "kokoro" (default, native) or "sidecar" (faster-qwen3-tts).
    #[serde(default = "default_tts_backend")]
    pub backend: String,

    /// Default speech speed multiplier (1.0 = normal).
    #[serde(default = "default_speed")]
    pub default_speed: f64,

    /// Default pitch multiplier (1.0 = normal).
    #[serde(default = "default_pitch")]
    pub default_pitch: f64,
}

fn default_tts_backend() -> String {
    "kokoro".to_string()
}
fn default_speed() -> f64 {
    1.0
}
fn default_pitch() -> f64 {
    1.0
}

impl Default for TtsSpec {
    fn default() -> Self {
        Self {
            backend: default_tts_backend(),
            default_speed: default_speed(),
            default_pitch: default_pitch(),
        }
    }
}

// ---------------------------------------------------------------------------
// Speaker
// ---------------------------------------------------------------------------

/// A speaker definition: voice identity + visual representation.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SpeakerSpec {
    /// Voice profile ID or Kokoro voice (e.g. "kokoro:af_heart").
    /// References a voice profile in the registry.
    pub voice: String,

    /// SVG preset name (e.g. "default_person", "robot", "cat").
    /// Must match a directory in mcp/assets/svg_presets/.
    #[serde(default = "default_preset")]
    pub preset: String,

    /// On-screen position for the speaker's sticker.
    #[serde(default = "default_position")]
    pub position: String,

    /// Sticker scale relative to canvas width (0.0–1.0).
    #[serde(default = "default_scale")]
    pub scale: f64,

    /// Default emote when scene doesn't specify one.
    #[serde(default = "default_emote")]
    pub emote_default: String,
}

fn default_preset() -> String {
    "default_person".to_string()
}
fn default_position() -> String {
    "top-left".to_string()
}
fn default_scale() -> f64 {
    0.25
}
fn default_emote() -> String {
    "neutral".to_string()
}

// ---------------------------------------------------------------------------
// Background
// ---------------------------------------------------------------------------

/// Background video configuration.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BackgroundSpec {
    /// Type: "gameplay" (YouTube auto-download), "procedural" (FFmpeg), "static" (image).
    #[serde(default = "default_bg_type")]
    pub r#type: String,

    /// Source: "youtube" for gameplay type.
    #[serde(default = "default_bg_source")]
    pub source: String,

    /// Search query for YouTube (gameplay type).
    #[serde(default)]
    pub query: String,

    /// Fallback clips if auto-download fails. Paths relative to project root.
    #[serde(default)]
    pub fallback_pool: Vec<String>,

    /// Crop mode: "center", "top", "bottom".
    #[serde(default = "default_crop_mode")]
    pub crop_mode: String,

    /// Whether to loop the background if shorter than the scene.
    #[serde(default = "default_loop")]
    pub loop_: bool,

    /// Background video volume in dB (typically -20 to -30 for subtle gameplay).
    #[serde(default = "default_bg_volume")]
    pub volume_db: f64,

    /// When to change backgrounds: "scene", "speaker", "fixed".
    #[serde(default = "default_change_cadence")]
    pub change_cadence: String,
}

fn default_bg_type() -> String {
    "gameplay".to_string()
}
fn default_bg_source() -> String {
    "youtube".to_string()
}
fn default_crop_mode() -> String {
    "center".to_string()
}
fn default_loop() -> bool {
    true
}
fn default_bg_volume() -> f64 {
    -20.0
}
fn default_change_cadence() -> String {
    "scene".to_string()
}

impl Default for BackgroundSpec {
    fn default() -> Self {
        Self {
            r#type: default_bg_type(),
            source: default_bg_source(),
            query: String::new(),
            fallback_pool: Vec::new(),
            crop_mode: default_crop_mode(),
            loop_: default_loop(),
            volume_db: default_bg_volume(),
            change_cadence: default_change_cadence(),
        }
    }
}

// ---------------------------------------------------------------------------
// Music
// ---------------------------------------------------------------------------

/// Background music configuration.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MusicSpec {
    /// Path to the music file (MP3/WAV).
    pub path: String,

    /// Music volume in dB (typically -15 to -20).
    #[serde(default = "default_music_gain")]
    pub gain_db: f64,

    /// Whether to duck music during speech.
    #[serde(default = "default_ducking")]
    pub ducking: bool,

    /// Ducking depth in dB (how much to lower music during speech).
    #[serde(default = "default_ducking_depth")]
    pub ducking_depth_db: f64,
}

fn default_music_gain() -> f64 {
    -18.0
}
fn default_ducking() -> bool {
    true
}
fn default_ducking_depth() -> f64 {
    12.0
}

// ---------------------------------------------------------------------------
// Captions
// ---------------------------------------------------------------------------

/// Caption styling configuration.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CaptionsSpec {
    /// Style: "word_highlight", "sentence_fade", "karaoke_fill", "subtitle_rail".
    #[serde(default = "default_caption_style")]
    pub style: String,

    /// Font family (e.g. "Bebas Neue", "Inter").
    #[serde(default = "default_font")]
    pub font: String,

    /// Font size in pixels.
    #[serde(default = "default_font_size")]
    pub font_size: u32,

    /// Text color (hex).
    #[serde(default = "default_text_color")]
    pub color: String,

    /// Highlight color for word_highlight / karaoke_fill styles.
    #[serde(default = "default_highlight_color")]
    pub highlight_color: String,

    /// Position: "bottom", "top", "center".
    #[serde(default = "default_caption_position")]
    pub position: String,

    /// Safe zone fraction (0.0–1.0) — captions stay within this fraction of the canvas.
    #[serde(default = "default_safe_zone")]
    pub safe_zone: f64,

    /// Maximum words per line before wrapping.
    #[serde(default = "default_max_words")]
    pub max_words_per_line: u32,
}

fn default_caption_style() -> String {
    "word_highlight".to_string()
}
fn default_font() -> String {
    "Bebas Neue".to_string()
}
fn default_font_size() -> u32 {
    72
}
fn default_text_color() -> String {
    "#ffffff".to_string()
}
fn default_highlight_color() -> String {
    "#00ff88".to_string()
}
fn default_caption_position() -> String {
    "bottom".to_string()
}
fn default_safe_zone() -> f64 {
    0.85
}
fn default_max_words() -> u32 {
    5
}

impl Default for CaptionsSpec {
    fn default() -> Self {
        Self {
            style: default_caption_style(),
            font: default_font(),
            font_size: default_font_size(),
            color: default_text_color(),
            highlight_color: default_highlight_color(),
            position: default_caption_position(),
            safe_zone: default_safe_zone(),
            max_words_per_line: default_max_words(),
        }
    }
}

// ---------------------------------------------------------------------------
// Speaker Layout
// ---------------------------------------------------------------------------

/// Where speaker stickers appear on screen.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SpeakerLayout {
    /// One sticker, centered (solo narration).
    SingleCenter,
    /// Two stickers top-left + top-right (podcast duet).
    SplitTop,
    /// Two stickers left + right (debate style).
    SplitSide,
    /// One large sticker, swaps to current speaker.
    ActiveSpeaker,
    /// Small sticker in corner, gameplay fills frame.
    PipCorner,
}

// ---------------------------------------------------------------------------
// Stickers
// ---------------------------------------------------------------------------

/// Sticker behavior configuration.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StickersSpec {
    /// Whether stickers are enabled.
    #[serde(default = "default_stickers_enabled")]
    pub enabled: bool,

    /// Lip-sync mode: "amplitude", "viseme", "none".
    #[serde(default = "default_lip_sync")]
    pub lip_sync: String,

    /// Whether to animate eye blinks.
    #[serde(default = "default_blink")]
    pub blink: bool,

    /// Whether to apply idle body bob.
    #[serde(default = "default_idle_bob")]
    pub idle_bob: bool,
}

fn default_stickers_enabled() -> bool {
    true
}
fn default_lip_sync() -> String {
    "amplitude".to_string()
}
fn default_blink() -> bool {
    true
}
fn default_idle_bob() -> bool {
    true
}

impl Default for StickersSpec {
    fn default() -> Self {
        Self {
            enabled: default_stickers_enabled(),
            lip_sync: default_lip_sync(),
            blink: default_blink(),
            idle_bob: default_idle_bob(),
        }
    }
}

// ---------------------------------------------------------------------------
// Scene
// ---------------------------------------------------------------------------

/// A single scene in the script (one speaker's line).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SceneSpec {
    /// Unique scene ID (auto-generated if absent).
    #[serde(default)]
    pub id: String,

    /// Speaker ID (must match a key in speakers).
    pub speaker: String,

    /// The spoken text for this scene.
    pub text: String,

    /// Emote for this scene (e.g. "neutral", "happy", "surprised", "thinking").
    /// Falls back to speaker's emote_default if absent.
    #[serde(default)]
    pub emote: Option<String>,

    /// Override background for this scene (preset name or null for auto).
    #[serde(default)]
    pub background: Option<String>,

    /// Override scene duration in ms (null = use TTS duration).
    #[serde(default)]
    pub duration_override_ms: Option<i64>,

    /// Whether the speaker's sticker is visible in this scene.
    #[serde(default = "default_sticker_visible")]
    pub sticker_visible: bool,
}

fn default_sticker_visible() -> bool {
    true
}

// ---------------------------------------------------------------------------
// SFX
// ---------------------------------------------------------------------------

/// A sound effect placement.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SfxSpec {
    /// Absolute time in ms, or null if trigger-based.
    #[serde(default)]
    pub at_ms: Option<i64>,

    /// SFX role: "intro", "transition", "highlight", "outro".
    #[serde(default)]
    pub role: String,

    /// Trigger condition: "scene_change", "speaker_change", or null.
    #[serde(default)]
    pub trigger: Option<String>,
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

/// Render output configuration.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OutputSpec {
    /// Output format: "mp4", "mov", "webm".
    #[serde(default = "default_format")]
    pub format: String,

    /// Video codec: "h264", "h265".
    #[serde(default = "default_codec")]
    pub codec: String,

    /// CRF (constant rate factor) — lower = higher quality. 18–28 typical.
    #[serde(default = "default_crf")]
    pub crf: u32,

    /// FFmpeg preset: "ultrafast" ... "slow".
    /// Note: this uses `default_ffmpeg_preset` (returns "slow"), NOT
    /// `default_preset` (returns "default_person", a SpeakerSpec preset
    /// name). Prior versions incorrectly called `default_preset` here,
    /// which meant `serde_json::from_str::<ScriptSpec>(r#"{"output":{}}"#)`
    /// produced `preset = "default_person"` — a value ffmpeg would reject.
    /// `Default::default()` and serde deserialisation now produce the same
    /// preset ("slow").
    #[serde(default = "default_ffmpeg_preset")]
    pub preset: String,
}

fn default_format() -> String {
    "mp4".to_string()
}
fn default_codec() -> String {
    "h264".to_string()
}
fn default_crf() -> u32 {
    18
}
fn default_ffmpeg_preset() -> String {
    "slow".to_string()
}

impl Default for OutputSpec {
    fn default() -> Self {
        Self {
            format: default_format(),
            codec: default_codec(),
            crf: default_crf(),
            preset: default_ffmpeg_preset(),
        }
    }
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Validation error for a ScriptSpec.
#[derive(Debug, Clone, Serialize)]
pub struct ScriptValidationError {
    pub field: String,
    pub message: String,
}

/// Validate a ScriptSpec. Returns a list of errors (empty = valid).
pub fn validate_script(spec: &ScriptSpec) -> Vec<ScriptValidationError> {
    let mut errors = Vec::new();

    // Must have at least one speaker
    if spec.speakers.is_empty() {
        errors.push(ScriptValidationError {
            field: "speakers".into(),
            message: "At least one speaker is required".into(),
        });
    }

    // Must have at least one scene
    if spec.scenes.is_empty() {
        errors.push(ScriptValidationError {
            field: "scenes".into(),
            message: "At least one scene is required".into(),
        });
    }

    // Every scene's speaker must exist in speakers
    for (i, scene) in spec.scenes.iter().enumerate() {
        if !spec.speakers.contains_key(&scene.speaker) {
            errors.push(ScriptValidationError {
                field: format!("scenes[{}].speaker", i),
                message: format!(
                    "Speaker '{}' not defined in speakers map",
                    scene.speaker
                ),
            });
        }
        if scene.text.trim().is_empty() {
            errors.push(ScriptValidationError {
                field: format!("scenes[{}].text", i),
                message: "Scene text cannot be empty".into(),
            });
        }
        // Auto-assign ID if missing
    }

    // Validate aspect ratio
    let valid_aspects = ["9:16", "16:9", "1:1"];
    if !valid_aspects.contains(&spec.meta.aspect.as_str()) {
        errors.push(ScriptValidationError {
            field: "meta.aspect".into(),
            message: format!(
                "Invalid aspect '{}'. Must be one of: {}",
                spec.meta.aspect,
                valid_aspects.join(", ")
            ),
        });
    }

    // Validate fps
    let valid_fps = [24, 30, 60];
    if !valid_fps.contains(&spec.meta.fps) {
        errors.push(ScriptValidationError {
            field: "meta.fps".into(),
            message: format!(
                "Invalid fps '{}'. Must be one of: {}",
                spec.meta.fps,
                valid_fps.iter().map(|f| f.to_string()).collect::<Vec<_>>().join(", ")
            ),
        });
    }

    // Validate caption style
    let valid_caption_styles = ["word_highlight", "sentence_fade", "karaoke_fill", "subtitle_rail"];
    if !valid_caption_styles.contains(&spec.captions.style.as_str()) {
        errors.push(ScriptValidationError {
            field: "captions.style".into(),
            message: format!(
                "Invalid caption style '{}'. Must be one of: {}",
                spec.captions.style,
                valid_caption_styles.join(", ")
            ),
        });
    }

    // Validate lip_sync mode
    let valid_lip_sync = ["amplitude", "viseme", "none"];
    if !valid_lip_sync.contains(&spec.stickers.lip_sync.as_str()) {
        errors.push(ScriptValidationError {
            field: "stickers.lip_sync".into(),
            message: format!(
                "Invalid lip_sync '{}'. Must be one of: {}",
                spec.stickers.lip_sync,
                valid_lip_sync.join(", ")
            ),
        });
    }

    // Validate background type
    let valid_bg_types = ["gameplay", "procedural", "static"];
    if !valid_bg_types.contains(&spec.background.r#type.as_str()) {
        errors.push(ScriptValidationError {
            field: "background.type".into(),
            message: format!(
                "Invalid background type '{}'. Must be one of: {}",
                spec.background.r#type,
                valid_bg_types.join(", ")
            ),
        });
    }

    // Validate change_cadence
    let valid_cadence = ["scene", "speaker", "fixed"];
    if !valid_cadence.contains(&spec.background.change_cadence.as_str()) {
        errors.push(ScriptValidationError {
            field: "background.change_cadence".into(),
            message: format!(
                "Invalid change_cadence '{}'. Must be one of: {}",
                spec.background.change_cadence,
                valid_cadence.join(", ")
            ),
        });
    }

    // Validate TTS backend
    let valid_tts_backends = ["kokoro", "sidecar"];
    if !valid_tts_backends.contains(&spec.tts.backend.as_str()) {
        errors.push(ScriptValidationError {
            field: "tts.backend".into(),
            message: format!(
                "Invalid TTS backend '{}'. Must be one of: {}",
                spec.tts.backend,
                valid_tts_backends.join(", ")
            ),
        });
    }

    errors
}

/// Parse a JSON string into a ScriptSpec, applying defaults for missing fields.
pub fn parse_script(json: &str) -> Result<ScriptSpec, serde_json::Error> {
    let mut spec: ScriptSpec = serde_json::from_str(json)?;

    // Auto-assign scene IDs if missing
    for (i, scene) in spec.scenes.iter_mut().enumerate() {
        if scene.id.is_empty() {
            scene.id = format!("scene_{:03}", i + 1);
        }
    }

    Ok(spec)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_script() {
        let json = r#"{
            "speakers": {
                "alice": {"voice": "kokoro:af_heart"}
            },
            "scenes": [
                {"speaker": "alice", "text": "Hello world!"}
            ]
        }"#;

        let spec = parse_script(json).unwrap();
        assert_eq!(spec.speakers.len(), 1);
        assert_eq!(spec.scenes.len(), 1);
        assert_eq!(spec.scenes[0].speaker, "alice");
        assert_eq!(spec.scenes[0].text, "Hello world!");
        assert_eq!(spec.scenes[0].id, "scene_001"); // auto-assigned

        // Defaults applied
        assert_eq!(spec.meta.aspect, "9:16");
        assert_eq!(spec.meta.fps, 30);
        assert_eq!(spec.tts.backend, "kokoro");
        assert_eq!(spec.captions.style, "word_highlight");
        assert_eq!(spec.speaker_layout, SpeakerLayout::SplitTop);
        assert!(spec.stickers.enabled);
        assert_eq!(spec.stickers.lip_sync, "amplitude");
    }

    #[test]
    fn test_parse_full_script() {
        let json = r#"{
            "title": "Test Podcast",
            "meta": {"aspect": "16:9", "fps": 24, "width": 1920, "height": 1080},
            "tts": {"backend": "sidecar", "default_speed": 1.2},
            "speakers": {
                "alice": {"voice": "kokoro:af_heart", "preset": "default_person", "position": "top-left"},
                "bob": {"voice": "kokoro:am_michael", "preset": "robot", "position": "top-right"}
            },
            "background": {"type": "procedural", "change_cadence": "speaker"},
            "captions": {"style": "karaoke_fill", "font": "Inter", "font_size": 64},
            "speaker_layout": "split_side",
            "stickers": {"enabled": true, "lip_sync": "viseme"},
            "scenes": [
                {"id": "s1", "speaker": "alice", "text": "Welcome!", "emote": "happy"},
                {"speaker": "bob", "text": "Hi there!", "emote": "neutral"}
            ],
            "sfx": [{"at_ms": 0, "role": "intro"}],
            "output": {"format": "mp4", "crf": 20}
        }"#;

        let spec = parse_script(json).unwrap();
        assert_eq!(spec.title, "Test Podcast");
        assert_eq!(spec.meta.aspect, "16:9");
        assert_eq!(spec.meta.fps, 24);
        assert_eq!(spec.tts.backend, "sidecar");
        assert_eq!(spec.tts.default_speed, 1.2);
        assert_eq!(spec.speakers.len(), 2);
        assert_eq!(spec.captions.style, "karaoke_fill");
        assert_eq!(spec.speaker_layout, SpeakerLayout::SplitSide);
        assert_eq!(spec.stickers.lip_sync, "viseme");
        assert_eq!(spec.scenes.len(), 2);
        assert_eq!(spec.scenes[0].id, "s1"); // preserves explicit ID
        assert_eq!(spec.scenes[1].id, "scene_002"); // auto-assigned
        assert_eq!(spec.sfx.len(), 1);
        assert_eq!(spec.output.crf, 20);
    }

    #[test]
    fn test_validate_valid_script() {
        let spec = parse_script(r#"{
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [{"speaker": "alice", "text": "Hello!"}]
        }"#).unwrap();

        let errors = validate_script(&spec);
        assert!(errors.is_empty(), "Expected no validation errors, got: {:?}", errors);
    }

    #[test]
    fn test_validate_missing_speakers() {
        let spec = parse_script(r#"{
            "speakers": {},
            "scenes": [{"speaker": "alice", "text": "Hello!"}]
        }"#).unwrap();

        let errors = validate_script(&spec);
        assert!(errors.iter().any(|e| e.field == "speakers" && e.message.contains("At least one speaker")));
    }

    #[test]
    fn test_validate_missing_scenes() {
        let spec = parse_script(r#"{
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": []
        }"#).unwrap();

        let errors = validate_script(&spec);
        assert!(errors.iter().any(|e| e.field == "scenes"));
    }

    #[test]
    fn test_validate_undefined_speaker_in_scene() {
        let spec = parse_script(r#"{
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [{"speaker": "bob", "text": "Hello!"}]
        }"#).unwrap();

        let errors = validate_script(&spec);
        assert!(errors.iter().any(|e| e.field == "scenes[0].speaker" && e.message.contains("not defined")));
    }

    #[test]
    fn test_validate_empty_scene_text() {
        let spec = parse_script(r#"{
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [{"speaker": "alice", "text": "  "}]
        }"#).unwrap();

        let errors = validate_script(&spec);
        assert!(errors.iter().any(|e| e.field == "scenes[0].text"));
    }

    #[test]
    fn test_validate_invalid_aspect() {
        let spec = parse_script(r#"{
            "meta": {"aspect": "4:3"},
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [{"speaker": "alice", "text": "Hi"}]
        }"#).unwrap();

        let errors = validate_script(&spec);
        assert!(errors.iter().any(|e| e.field == "meta.aspect"));
    }

    #[test]
    fn test_validate_invalid_fps() {
        let spec = parse_script(r#"{
            "meta": {"fps": 25},
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [{"speaker": "alice", "text": "Hi"}]
        }"#).unwrap();

        let errors = validate_script(&spec);
        assert!(errors.iter().any(|e| e.field == "meta.fps"));
    }

    #[test]
    fn test_validate_invalid_caption_style() {
        let spec = parse_script(r#"{
            "captions": {"style": "fancy"},
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [{"speaker": "alice", "text": "Hi"}]
        }"#).unwrap();

        let errors = validate_script(&spec);
        assert!(errors.iter().any(|e| e.field == "captions.style"));
    }

    #[test]
    fn test_validate_invalid_lip_sync() {
        let spec = parse_script(r#"{
            "stickers": {"lip_sync": "ml-powered"},
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [{"speaker": "alice", "text": "Hi"}]
        }"#).unwrap();

        let errors = validate_script(&spec);
        assert!(errors.iter().any(|e| e.field == "stickers.lip_sync"));
    }

    #[test]
    fn test_validate_invalid_tts_backend() {
        let spec = parse_script(r#"{
            "tts": {"backend": "elevenlabs"},
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [{"speaker": "alice", "text": "Hi"}]
        }"#).unwrap();

        let errors = validate_script(&spec);
        assert!(errors.iter().any(|e| e.field == "tts.backend"));
    }

    #[test]
    fn test_validate_invalid_background_type() {
        let spec = parse_script(r#"{
            "background": {"type": "stock_footage"},
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [{"speaker": "alice", "text": "Hi"}]
        }"#).unwrap();

        let errors = validate_script(&spec);
        assert!(errors.iter().any(|e| e.field == "background.type"));
    }

    #[test]
    fn test_speaker_layout_serialization() {
        // Verify snake_case serialization
        let json = r#""split_top""#;
        let layout: SpeakerLayout = serde_json::from_str(json).unwrap();
        assert_eq!(layout, SpeakerLayout::SplitTop);

        let json = r#""active_speaker""#;
        let layout: SpeakerLayout = serde_json::from_str(json).unwrap();
        assert_eq!(layout, SpeakerLayout::ActiveSpeaker);

        let json = r#""pip_corner""#;
        let layout: SpeakerLayout = serde_json::from_str(json).unwrap();
        assert_eq!(layout, SpeakerLayout::PipCorner);
    }

    #[test]
    fn test_auto_assign_scene_ids() {
        let spec = parse_script(r#"{
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [
                {"speaker": "alice", "text": "First"},
                {"speaker": "alice", "text": "Second"},
                {"id": "custom", "speaker": "alice", "text": "Third"}
            ]
        }"#).unwrap();

        assert_eq!(spec.scenes[0].id, "scene_001");
        assert_eq!(spec.scenes[1].id, "scene_002");
        assert_eq!(spec.scenes[2].id, "custom"); // preserves explicit ID
    }

    #[test]
    fn test_defaults_applied_for_missing_sections() {
        let spec = parse_script(r#"{
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [{"speaker": "alice", "text": "Hi"}]
        }"#).unwrap();

        // All optional sections get defaults
        assert_eq!(spec.meta.width, 1080);
        assert_eq!(spec.meta.height, 1920);
        assert_eq!(spec.background.r#type, "gameplay");
        assert_eq!(spec.background.change_cadence, "scene");
        assert!(spec.music.is_none());
        assert_eq!(spec.captions.font, "Bebas Neue");
        assert_eq!(spec.output.format, "mp4");
    }
}
