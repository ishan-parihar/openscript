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

    /// Topic keywords representing the WHOLE video (3-5 words).
    /// These are prepended to every Pexels/GIPHY search query to bias
    /// results toward the video's topic, not just the individual sentence.
    /// Example: ["brain", "neuroscience", "neurons", "science", "mind"]
    /// If not provided, keywords are auto-extracted from the title.
    /// (Round-13: topic-aware video search upgrade.)
    #[serde(default)]
    pub video_keywords: Vec<String>,

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

    /// Sticker behavior configuration.
    #[serde(default)]
    pub stickers: StickersSpec,

    /// Meme b-roll configuration (GIPHY reaction GIFs per scene).
    /// When enabled, each scene gets a short contextual reaction GIF
    /// that pops in briefly (2-3s) and disappears.
    #[serde(default)]
    pub meme_brolls: MemeBrollSpec,

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
}

fn default_preset() -> String {
    "default_person".to_string()
}
fn default_position() -> String {
    "top-left".to_string()
}
fn default_scale() -> f64 {
    // Raised from 0.25 to 0.35 per round-7 audit: "sticker/gif scaling is
    // always very small and badly compositioned." 0.35 = 35% of canvas width
    // = 378px on 1080px canvas — large enough to be clearly visible.
    0.35
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
    /// Serde alias "loop" accepts the JSON key users naturally write
    /// (Rust reserves `loop` as a keyword, so the field is `loop_`).
    /// Without this alias, `"loop": false` in JSON was silently ignored
    /// and the default (true) was used — a silent-failure UX bug.
    /// (UX audit GAP #3 fix.)
    #[serde(default = "default_loop", alias = "loop")]
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
    // The stock music files in mcp/assets/music/ are normalized to -32 dB mean.
    // To make music audible in the mix, we need to BOOST it, not cut it.
    // +6 dB (linear 2.0) brings the music to ~-26 dB mean, which is audible
    // behind voice without overpowering it.
    // (Round-5 audit: default was -18 dB which made music inaudible.)
    6.0
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
// Meme B-Rolls (GIPHY reaction GIFs per scene)
// ---------------------------------------------------------------------------

/// Meme b-roll configuration. When enabled, each scene gets a short
/// contextual reaction GIF from GIPHY that pops in briefly (2-3s) and
/// then disappears — like TikTok reaction videos.
///
/// Unlike stickers (which persist for the whole speaker segment as a
/// speaker identifier), meme b-rolls are **brief**, **emotional**, and
/// **dynamic** (pop-in + fade-out animation).
///
/// Set `"meme_brolls": {"enabled": true}` in the script to activate.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MemeBrollSpec {
    /// Whether meme b-rolls are enabled.
    #[serde(default = "default_meme_enabled")]
    pub enabled: bool,

    /// Position on screen: "center", "center-bottom", "center-top".
    #[serde(default = "default_meme_position")]
    pub position: String,

    /// Scale as fraction of canvas width (0.35 = 35%).
    #[serde(default = "default_meme_scale")]
    pub scale: f64,

    /// How long each meme plays in seconds.
    #[serde(default = "default_meme_duration")]
    pub duration_s: f64,

    /// Delay after scene start before meme appears (seconds).
    #[serde(default = "default_meme_offset")]
    pub offset_s: f64,

    /// Query strategy: "translate" (GIPHY translate, 1 best match) or
    /// "search" (GIPHY search, pick from results).
    #[serde(default = "default_meme_strategy")]
    pub query_strategy: String,
}

fn default_meme_enabled() -> bool { false }
fn default_meme_position() -> String { "center-bottom".to_string() }
fn default_meme_scale() -> f64 { 0.35 }
fn default_meme_duration() -> f64 { 2.5 }
fn default_meme_offset() -> f64 { 0.3 }
fn default_meme_strategy() -> String { "translate".to_string() }

impl Default for MemeBrollSpec {
    fn default() -> Self {
        Self {
            enabled: default_meme_enabled(),
            position: default_meme_position(),
            scale: default_meme_scale(),
            duration_s: default_meme_duration(),
            offset_s: default_meme_offset(),
            query_strategy: default_meme_strategy(),
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
    #[serde(default)]
    pub emote: Option<String>,

    /// Override background for this scene (preset name or null for auto).
    #[serde(default)]
    pub background: Option<String>,

    /// Override scene duration in ms (null = use TTS duration).
    #[serde(default)]
    pub duration_override_ms: Option<i64>,
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

    /// Render engine: "ffmpeg" (default, multilayer FFmpeg render) or
    /// "hyperframes" (HTML+GSAP motion graphics via hf.render).
    /// When "hyperframes", script.to_video will:
    ///   1. Build the timeline via script.to_timeline
    ///   2. Compile it to HF HTML via timeline.to_hyperframes
    ///   3. Render via hf.render
    /// This gives agents programmatic control over the render engine,
    /// connecting HyperFrames to the golden trajectory.
    #[serde(default = "default_render_engine")]
    pub render_engine: String,

    /// Theme preset: "neutral" (default), "calm", "energetic".
    /// When set to a non-neutral value, applies correlated defaults to
    /// captions + stickers so the video has a consistent emotional tone
    /// without the agent hand-tuning each field.
    ///
    /// "calm": warm-gold highlight (#E8B86D), cream text (#F5F0E8).
    ///   Captions stay word_highlight (word-level sync with speaker's voice
    ///   is the default for ALL content types). Stickers stay enabled.
    ///   For healing/meditation/therapy content.
    ///
    /// "energetic": neon-green highlight (#00ff88), white text,
    ///   word_highlight style, stickers enabled. For gaming/edu-short
    ///   content. This matches the historical defaults.
    ///
    /// "neutral": no override — use the explicit fields as-is.
    ///
    /// Individual field values (e.g. captions.highlight_color) always
    /// override the theme preset. The theme only sets defaults for fields
    /// the agent did not explicitly set.
    /// (UX audit GAP #4 fix — defaults were tuned for energetic/meme
    /// content and fought healing topics. Round-3 audit refined: word-sync
    /// captions and stickers are universal defaults, only colors change.)
    #[serde(default = "default_theme")]
    pub theme: String,
}

fn default_render_engine() -> String {
    "ffmpeg".to_string()
}

fn default_theme() -> String {
    "neutral".to_string()
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
            render_engine: default_render_engine(),
            theme: default_theme(),
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
                message: format!("Speaker '{}' not defined in speakers map", scene.speaker),
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
                valid_fps
                    .iter()
                    .map(|f| f.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        });
    }

    // Validate caption style
    let valid_caption_styles = [
        "word_highlight",
        "sentence_fade",
        "karaoke_fill",
        "subtitle_rail",
    ];
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

    // Validate render_engine
    let valid_engines = ["ffmpeg", "hyperframes"];
    if !valid_engines.contains(&spec.output.render_engine.as_str()) {
        errors.push(ScriptValidationError {
            field: "output.render_engine".into(),
            message: format!(
                "Invalid render_engine '{}'. Must be one of: {}",
                spec.output.render_engine,
                valid_engines.join(", ")
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

    // Auto-extract video_keywords from title if not provided.
    // (Round-13: topic-aware video search upgrade.)
    if spec.video_keywords.is_empty() && !spec.title.is_empty() {
        spec.video_keywords = extract_topic_keywords(&spec.title);
    }

    // Apply theme preset.
    apply_theme(&mut spec);

    Ok(spec)
}

/// Extract topic keywords from a video title.
/// Takes the most significant words (non-stopwords, length > 3),
/// limited to 5 keywords. These represent the WHOLE video topic.
/// Example: "3 Surprising Facts About the Human Brain" → ["surprising", "facts", "human", "brain"]
fn extract_topic_keywords(title: &str) -> Vec<String> {
    let stop_words = [
        "the", "a", "an", "is", "are", "was", "were", "be", "about", "of", "to", "in",
        "on", "at", "by", "with", "and", "or", "for", "from", "how", "why", "what",
        "when", "where", "who", "your", "you", "can", "do", "does", "did", "will",
        "would", "could", "should", "3", "5", "10", "top", "best", "most",
    ];

    title
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
        .filter(|w| !w.is_empty() && w.len() > 3 && !stop_words.contains(&w.as_str()))
        .take(5)
        .collect()
}

/// Apply the theme preset from `output.theme` to captions + stickers.
///
/// A theme only overrides fields that still have their hardcoded default
/// values — if the user explicitly set a field, the theme respects it.
/// This is detected by comparing the current value to the default function.
fn apply_theme(spec: &mut ScriptSpec) {
    match spec.output.theme.as_str() {
        "calm" => {
            // Captions: warm-gold highlight, cream text. Keep word_highlight
            // as the style — word-level sync with the speaker's voice is the
            // expected default for ALL content types, including healing.
            // (Round-3 UX audit: user reported "caption-words does not follow
            // the speaker's voice" — sentence_fade removed that sync. Fix:
            // keep word_highlight, only change the colors.)
            if spec.captions.highlight_color == default_highlight_color() {
                spec.captions.highlight_color = "#E8B86D".to_string(); // warm gold
            }
            if spec.captions.color == default_text_color() {
                spec.captions.color = "#F5F0E8".to_string(); // cream
            }
            // Note: stickers stay enabled. GIPHY stickers can find calming
            // imagery. Agents who want zero stickers set stickers.enabled:false
            // explicitly. (Round-3 UX audit: sticker absence was noted as a
            // quality gap.)
        }
        "energetic" => {
            // Energetic = the historical defaults (neon green, word_highlight,
            // stickers on). No override needed — the defaults ARE the energetic
            // theme. This branch exists for explicitness.
        }
        "neutral" | _ => {
            // No override — use fields as-is.
        }
    }
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
        assert_eq!(spec.stickers.lip_sync, "viseme");
        assert_eq!(spec.scenes.len(), 2);
        assert_eq!(spec.scenes[0].id, "s1"); // preserves explicit ID
        assert_eq!(spec.scenes[1].id, "scene_002"); // auto-assigned
        assert_eq!(spec.sfx.len(), 1);
        assert_eq!(spec.output.crf, 20);
    }

    #[test]
    fn test_validate_valid_script() {
        let spec = parse_script(
            r#"{
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [{"speaker": "alice", "text": "Hello!"}]
        }"#,
        )
        .unwrap();

        let errors = validate_script(&spec);
        assert!(
            errors.is_empty(),
            "Expected no validation errors, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_validate_missing_speakers() {
        let spec = parse_script(
            r#"{
            "speakers": {},
            "scenes": [{"speaker": "alice", "text": "Hello!"}]
        }"#,
        )
        .unwrap();

        let errors = validate_script(&spec);
        assert!(errors
            .iter()
            .any(|e| e.field == "speakers" && e.message.contains("At least one speaker")));
    }

    #[test]
    fn test_validate_missing_scenes() {
        let spec = parse_script(
            r#"{
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": []
        }"#,
        )
        .unwrap();

        let errors = validate_script(&spec);
        assert!(errors.iter().any(|e| e.field == "scenes"));
    }

    #[test]
    fn test_validate_undefined_speaker_in_scene() {
        let spec = parse_script(
            r#"{
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [{"speaker": "bob", "text": "Hello!"}]
        }"#,
        )
        .unwrap();

        let errors = validate_script(&spec);
        assert!(errors
            .iter()
            .any(|e| e.field == "scenes[0].speaker" && e.message.contains("not defined")));
    }

    #[test]
    fn test_validate_empty_scene_text() {
        let spec = parse_script(
            r#"{
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [{"speaker": "alice", "text": "  "}]
        }"#,
        )
        .unwrap();

        let errors = validate_script(&spec);
        assert!(errors.iter().any(|e| e.field == "scenes[0].text"));
    }

    #[test]
    fn test_validate_invalid_aspect() {
        let spec = parse_script(
            r#"{
            "meta": {"aspect": "4:3"},
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [{"speaker": "alice", "text": "Hi"}]
        }"#,
        )
        .unwrap();

        let errors = validate_script(&spec);
        assert!(errors.iter().any(|e| e.field == "meta.aspect"));
    }

    #[test]
    fn test_validate_invalid_fps() {
        let spec = parse_script(
            r#"{
            "meta": {"fps": 25},
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [{"speaker": "alice", "text": "Hi"}]
        }"#,
        )
        .unwrap();

        let errors = validate_script(&spec);
        assert!(errors.iter().any(|e| e.field == "meta.fps"));
    }

    #[test]
    fn test_validate_invalid_caption_style() {
        let spec = parse_script(
            r#"{
            "captions": {"style": "fancy"},
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [{"speaker": "alice", "text": "Hi"}]
        }"#,
        )
        .unwrap();

        let errors = validate_script(&spec);
        assert!(errors.iter().any(|e| e.field == "captions.style"));
    }

    #[test]
    fn test_validate_invalid_lip_sync() {
        let spec = parse_script(
            r#"{
            "stickers": {"lip_sync": "ml-powered"},
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [{"speaker": "alice", "text": "Hi"}]
        }"#,
        )
        .unwrap();

        let errors = validate_script(&spec);
        assert!(errors.iter().any(|e| e.field == "stickers.lip_sync"));
    }

    #[test]
    fn test_validate_invalid_tts_backend() {
        let spec = parse_script(
            r#"{
            "tts": {"backend": "elevenlabs"},
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [{"speaker": "alice", "text": "Hi"}]
        }"#,
        )
        .unwrap();

        let errors = validate_script(&spec);
        assert!(errors.iter().any(|e| e.field == "tts.backend"));
    }

    #[test]
    fn test_validate_invalid_background_type() {
        let spec = parse_script(
            r#"{
            "background": {"type": "stock_footage"},
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [{"speaker": "alice", "text": "Hi"}]
        }"#,
        )
        .unwrap();

        let errors = validate_script(&spec);
        assert!(errors.iter().any(|e| e.field == "background.type"));
    }

    #[test]
    fn test_auto_assign_scene_ids() {
        let spec = parse_script(
            r#"{
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [
                {"speaker": "alice", "text": "First"},
                {"speaker": "alice", "text": "Second"},
                {"id": "custom", "speaker": "alice", "text": "Third"}
            ]
        }"#,
        )
        .unwrap();

        assert_eq!(spec.scenes[0].id, "scene_001");
        assert_eq!(spec.scenes[1].id, "scene_002");
        assert_eq!(spec.scenes[2].id, "custom"); // preserves explicit ID
    }

    #[test]
    fn test_defaults_applied_for_missing_sections() {
        let spec = parse_script(
            r#"{
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [{"speaker": "alice", "text": "Hi"}]
        }"#,
        )
        .unwrap();

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

    /// Verify that the JSON key "loop" (what users naturally write) is
    /// accepted as an alias for the Rust field `loop_` (Rust keyword
    /// collision). Without the #[serde(alias = "loop")] attribute,
    /// `"loop": false` was silently ignored and the default (true) was
    /// used — a silent-failure UX bug found by the fresh-agent audit.
    /// (UX audit GAP #3 regression test.)
    #[test]
    fn test_loop_alias_accepted_in_json() {
        let json = r#"{
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [{"speaker": "alice", "text": "Hi"}],
            "background": {"type": "procedural", "loop": false}
        }"#;
        let spec = parse_script(json).unwrap();
        assert_eq!(
            spec.background.loop_, false,
            "JSON key 'loop' must be accepted as alias for Rust field 'loop_'"
        );

        // Also verify the underscore form still works (backward compat).
        let json2 = r#"{
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [{"speaker": "alice", "text": "Hi"}],
            "background": {"type": "procedural", "loop_": false}
        }"#;
        let spec2 = parse_script(json2).unwrap();
        assert_eq!(spec2.background.loop_, false);
    }

    /// Verify that theme:"calm" applies warm-gold highlight, cream text,
    /// sentence_fade style, and disables stickers — without the agent
    /// hand-tuning each field. (UX audit GAP #4 regression test.)
    /// Round-3 update: word_highlight + stickers are now universal defaults.
    #[test]
    fn test_theme_calm_applies_healing_defaults() {
        let json = r#"{
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [{"speaker": "alice", "text": "Breathe in, breathe out."}],
            "output": {"theme": "calm"}
        }"#;
        let spec = parse_script(json).unwrap();
        assert_eq!(spec.output.theme, "calm");
        assert_eq!(spec.captions.highlight_color, "#E8B86D", "calm theme should set warm-gold highlight");
        assert_eq!(spec.captions.color, "#F5F0E8", "calm theme should set cream text");
        assert_eq!(spec.captions.style, "word_highlight", "calm theme should keep word_highlight (word-sync is universal default)");
        assert!(spec.stickers.enabled, "calm theme should keep stickers enabled");
    }

    /// Verify that theme:"calm" does NOT override fields the user explicitly set.
    #[test]
    fn test_theme_calm_respects_explicit_overrides() {
        let json = r##"{
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [{"speaker": "alice", "text": "Breathe."}],
            "captions": {"highlight_color": "#FF0000"},
            "output": {"theme": "calm"}
        }"##;
        let spec = parse_script(json).unwrap();
        assert_eq!(spec.captions.highlight_color, "#FF0000", "user's explicit red should win over calm theme gold");
        // But fields the user didn't set should still get the calm theme default
        assert_eq!(spec.captions.color, "#F5F0E8", "calm theme cream text should apply (user didn't set color)");
        assert_eq!(spec.captions.style, "word_highlight", "word_highlight is the universal default, not overridden by theme");
    }

    /// Verify that theme:"neutral" (the default) applies no overrides.
    #[test]
    fn test_theme_neutral_is_no_op() {
        let json = r#"{
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [{"speaker": "alice", "text": "Hi"}],
            "output": {"theme": "neutral"}
        }"#;
        let spec = parse_script(json).unwrap();
        assert_eq!(spec.captions.highlight_color, "#00ff88", "neutral theme should keep default neon green");
        assert_eq!(spec.captions.style, "word_highlight", "neutral theme should keep default word_highlight");
        assert!(spec.stickers.enabled, "neutral theme should keep stickers enabled");
    }

    /// Verify that omitting theme entirely defaults to "neutral".
    #[test]
    fn test_theme_defaults_to_neutral() {
        let json = r#"{
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [{"speaker": "alice", "text": "Hi"}]
        }"#;
        let spec = parse_script(json).unwrap();
        assert_eq!(spec.output.theme, "neutral");
    }
