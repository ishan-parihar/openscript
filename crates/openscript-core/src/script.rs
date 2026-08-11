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
use std::collections::{HashMap, HashSet};

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

    /// Source language of the script text. Routes the caption word-timing
    /// alignment engine: "en" (default) → Parakeet TDT (fast, English-only);
    /// "hi" / "hinglish" / "hi-IN" → Whisper (multilingual, language hint
    /// `hi`) — Parakeet's English ASR drifts on Hinglish audio and collapses
    /// captions to even-spacing estimates. Accepted values follow the
    /// transcribe `language_hint` vocabulary.
    #[serde(default = "default_language")]
    pub language: String,

    /// Speaker definitions (voice, visual preset, position).
    ///
    /// Keys are speaker IDs referenced by scenes. At least one speaker
    /// is required.
    ///
    /// Accepts BOTH formats:
    /// - Map (canonical): {"narrator": {"voice": "kokoro:af_heart"}}
    /// - Array (agent-friendly): [{"id": "narrator", "voice": "kokoro:af_heart"}]
    ///   (UX audit GAP #2 fix: agents naturally write arrays.)
    ///   Array format is normalized in parse_script() before deserialization.
    pub speakers: HashMap<String, SpeakerSpec>,

    /// Background video configuration (gameplay, procedural, or static).
    ///
    /// Accepts BOTH formats:
    /// - Object (canonical): {"type": "procedural", "change_cadence": "scene"}
    /// - String (agent-friendly): "procedural" (shorthand for {"type": <value>})
    ///   (UX audit GAP: agents wrote bare strings like "procedural".)
    ///   String format is normalized in parse_script() before deserialization.
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

    /// Content-format configuration: correlated defaults + a scene-structure
    /// playbook (presentation | podcast | dialogue | comedy_sketch | romcom |
    /// meme_reel | documentary | how_to) plus the speaker alternation strategy.
    /// Agent-friendly shorthand "format": "podcast" normalizes at parse time.
    #[serde(default)]
    pub format: ContentFormatSpec,

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
    /// Backend engine: "kokoro" (presets, default), "audio8" (zero-shot
    /// clone, ONNX INT4), "gepard" (high-quality native-English clone),
    /// "voicedesign" (Qwen3-TTS-1.7B-VoiceDesign ONNX int4 — DIRECT
    /// natural-language-instruction synthesis, per-line emotion/tonality, NO
    /// cloning), "higgs" (Higgs Audio v3 4B ONNX GenAI int4 — 100+ languages,
    /// zero-shot voice cloning + inline emotion/prosody/style/sfx control
    /// tags), "sidecar" (faster-qwen3-tts). This is the ENGINE SELECTION
    /// for voice generation. Note: a speaker whose `voice` references a
    /// registered profile (e.g. "air_analyst") routes by the profile's own
    /// provider field, which always wins — a character voice registered as a
    /// `voicedesign` profile synthesizes with Qwen3 VoiceDesign even if this
    /// field says "kokoro". This backend matters when a speaker's voice is the
    /// literal string "default": it then selects the fallback built-in
    /// (kokoro → kokoro:af_heart) or errors for engines that require a
    /// configured voice profile. Bare preset names ("af_heart") always
    /// normalize to kokoro: regardless of this field.
    #[serde(default = "default_tts_backend")]
    pub backend: String,

    /// Default voice profile id (e.g. "ishan_gepard", "ishan", "kokoro:af_heart").
    /// When set, a speaker whose voice is the literal string "default"
    /// resolves to this profile. Lets one script-level setting drive the
    /// cloned voice for every speaker.
    #[serde(default)]
    pub voice: Option<String>,

    /// Default speech speed multiplier (1.0 = normal).
    #[serde(default = "default_speed")]
    pub default_speed: f64,

    /// Default pitch multiplier (1.0 = normal).
    #[serde(default = "default_pitch")]
    pub default_pitch: f64,

    /// Default sampling temperature for clone engines (None = engine default:
    /// 0.7 for gepard/audio8 — expressive but stable). Higher = more prosodic
    /// variation/inflection; lower = flatter, more robotic. Production-grade
    /// voices sit 0.6-0.8; the old flat 0.3 default produced "robotic, no
    /// emotional nuance" clones (audit finding).
    #[serde(default)]
    pub default_temperature: Option<f64>,

    /// Default top-k for clone engines (None = engine default).
    #[serde(default)]
    pub default_top_k: Option<u32>,

    /// Default cfg_scale for gepard (reference-fidelity; higher clings closer
    /// to the reference recording). None = 1.0.
    #[serde(default)]
    pub default_cfg_scale: Option<f64>,
}

fn default_tts_backend() -> String {
    "kokoro".to_string()
}
fn default_language() -> String {
    "en".to_string()
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
            voice: None,
            default_speed: default_speed(),
            default_pitch: default_pitch(),
            default_temperature: None,
            default_top_k: None,
            default_cfg_scale: None,
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

    /// Speaker gender: "male" | "female" | "nonbinary" | "auto" (default).
    /// "auto" is inferred at parse time from the Kokoro voice id prefix
    /// (af_/am_/bf_/bm_/...) or a free-text personality hint, else "unknown".
    /// Drives the format.alternation="male_female" strategy and the
    /// content-format speaker blueprints (male/female alternation).
    #[serde(default = "default_gender")]
    pub gender: String,
}

fn default_gender() -> String {
    "auto".to_string()
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
// Content Format
// ---------------------------------------------------------------------------

/// Content-format configuration: correlated defaults + a scene-structure
/// playbook that shapes HOW a script is authored (speaker count, alternation,
/// pacing, reactions) without changing the render pipeline.
///
/// `type` is the format kind; `alternation` drives the male/female speaker
/// alternation strategy. All fields are correlated DEFAULTS — anything the
/// agent set explicitly always wins (same philosophy as `apply_theme`).
///
/// Agent-friendly shorthand is accepted at parse time: `"format": "podcast"`
/// normalizes to `{"type": "podcast"}`.
///
/// Music moods understood by the library/music pipeline. Formats and scripts
/// should only reference these — validate_script rejects anything else.
pub const VALID_MUSIC_MOODS: &[&str] = &[
    "neutral", "calm", "energetic", "dark", "uplifting", "upbeat", "dramatic", "sad",
];

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ContentFormatSpec {
    /// Format kind: "presentation" (default), "podcast", "dialogue",
    /// "comedy_sketch", "romcom", "meme_reel", "documentary", "how_to",
    /// "listicle", "storytime", "debate", "newsflash", "review".
    #[serde(default = "default_format_type")]
    pub r#type: String,

    /// Speaker alternation strategy:
    /// - "none" (default): no constraint; scenes may repeat speakers freely.
    /// - "male_female": requires ≥2 distinct speaker genders; the agent
    ///   authors scenes alternating male/female voices for engagement.
    /// - "auto": alternates available genders where possible; never errors.
    #[serde(default = "default_alternation")]
    pub alternation: String,

    /// Expected speaker count range (0 = no constraint). Validated at parse.
    #[serde(default)]
    pub min_speakers: u32,
    #[serde(default)]
    pub max_speakers: u32,

    /// Expected scene count range (0 = no constraint). Validated at parse.
    #[serde(default)]
    pub min_scenes: u32,
    #[serde(default)]
    pub max_scenes: u32,

    /// Correlated default speech speed (0.0 = no override).
    #[serde(default)]
    pub default_speed: f64,

    /// Correlated default synthesis temperature (None = engine default).
    #[serde(default)]
    pub default_temperature: Option<f64>,

    /// Correlated default for GIPHY reaction meme pop-ins (meme_brolls).
    #[serde(default)]
    pub reaction_memes: bool,

    /// Correlated sticker behavior: "character" (speaker-identifier stickers,
    /// default), "reaction" (reaction-driven stickers), "none".
    #[serde(default = "default_sticker_mode")]
    pub sticker_mode: String,

    /// Correlated music mood hint (auto-select). Stays within the library
    /// vocabulary: neutral | calm | energetic.
    #[serde(default)]
    pub music_mood: Option<String>,
}

fn default_format_type() -> String {
    "presentation".to_string()
}
fn default_alternation() -> String {
    "none".to_string()
}
fn default_sticker_mode() -> String {
    "character".to_string()
}

impl Default for ContentFormatSpec {
    fn default() -> Self {
        Self {
            r#type: default_format_type(),
            alternation: default_alternation(),
            min_speakers: 0,
            max_speakers: 0,
            min_scenes: 0,
            max_scenes: 0,
            default_speed: 0.0,
            default_temperature: None,
            reaction_memes: false,
            sticker_mode: default_sticker_mode(),
            music_mood: None,
        }
    }
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

    /// Opt-in YouTube footage for GENERATION (default false). When false the
    /// acquisition chain stops at Pexels → Pixabay → fallback_pool and never
    /// reaches YouTube (social-platform metadata is clickbait; curated stock
    /// beats it). YouTube stays always-on for asset-development workflows
    /// (asset.probe / broll.probe) regardless of this flag.
    #[serde(default)]
    pub enable_youtube: bool,
}

fn default_bg_type() -> String {
    "procedural".to_string()
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
            enable_youtube: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Music
// ---------------------------------------------------------------------------

/// Background music configuration.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MusicSpec {
    /// Path to the music file (MP3/WAV). Optional — if omitted, auto-select from library by mood.
    #[serde(default)]
    pub path: Option<String>,

    /// Music volume in dB (typically -15 to -20).
    #[serde(default = "default_music_gain")]
    pub gain_db: f64,

    /// Whether to duck music during speech.
    #[serde(default = "default_ducking")]
    pub ducking: bool,

    /// Ducking depth in dB (how much to lower music during speech).
    #[serde(default = "default_ducking_depth")]
    pub ducking_depth_db: f64,

    /// Music mood hint (e.g. "neutral", "calm", "energetic") for
    /// auto-selection from the library. Set by the content format's
    /// `music_mood` when the agent did not pick a track.
    #[serde(default)]
    pub mood: Option<String>,
}

impl Default for MusicSpec {
    fn default() -> Self {
        Self {
            path: None,
            gain_db: default_music_gain(),
            ducking: default_ducking(),
            ducking_depth_db: default_ducking_depth(),
            mood: None,
        }
    }
}

fn default_music_gain() -> f64 {
    // Library tracks (YouTube-scraped) are normalized to ~-16 dB LUFS.
    // Using -10 dB puts music at ~-26 dB mean — audible behind voice
    // without overpowering it. The old +6 dB default boosted library
    // tracks above unity, triggering production quality HARD failures.
    // (Run-9 fresh-agent audit: gain_db=6.0 flagged as "boosted above unity").
    -10.0
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

    /// Position: "center" (default), "bottom", "top".
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
    // Center is the product default: captions sit mid-screen, clear of the
    // subject and of bottom safe-zone UI. Override per-call with "bottom"
    // (shorts lower-third) or "top" when the composition calls for it.
    "center".to_string()
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
    /// When the speaker's voice profile carries an emotion-template map
    /// (voice.profile.add with `emotions`), this selects the matching
    /// emotional delivery take at synthesis time — the line is spoken in
    /// that emotion's tonality, not the neutral clone timbre. Also feeds
    /// sticker/GIPHY reaction search. Free-form string; the emotion-take
    /// lookup falls back to the base voice when no take matches.
    #[serde(default)]
    pub emote: Option<String>,

    /// Natural-language delivery direction for this line, e.g. "low gravelly
    /// whisper, slow deliberate pace". Refines emote-take selection; engines
    /// that accept instruction (VoiceDesign) receive it verbatim.
    #[serde(default)]
    pub tone: Option<String>,

    /// RAW control-tag passthrough for engines with inline control tokens
    /// (Higgs Audio v3: emotion/style/sfx/prosody tags, 43 total). Prepended
    /// verbatim to the line before synthesis — e.g.
    /// `"<|prosody:pause|> mid, <|sfx:laughter|>Haha"` for inline effects the
    /// structured `emote`/`speed`/`pitch` fields don't express. Only the
    /// engine's recognized tags are valid; anything else gets read aloud.
    /// Ignored by engines without a control-tag channel.
    #[serde(default)]
    pub control_tags: Option<String>,

    /// Per-scene speech speed multiplier (overrides tts.default_speed).
    #[serde(default)]
    pub speed: Option<f64>,

    /// Per-scene pitch multiplier (overrides tts.default_pitch).
    #[serde(default)]
    pub pitch: Option<f64>,

    /// Per-scene sampling temperature override (overrides tts.default_temperature).
    /// Lets a single line be more/less expressive than the rest of the video.
    #[serde(default)]
    pub temperature: Option<f64>,

    /// Override background for this scene (preset name or null for auto).
    #[serde(default)]
    pub background: Option<String>,

    /// Override scene duration in ms (null = use TTS duration).
    /// (UX audit GAP #5 fix: agents wrote `duration_seconds` which was
    /// silently ignored. We accept it via a separate field and convert.)
    #[serde(default)]
    pub duration_override_ms: Option<i64>,

    /// Override scene duration in seconds (null = use TTS duration).
    /// Agents naturally think in seconds, not milliseconds. If both
    /// duration_seconds and duration_override_ms are set, duration_override_ms wins.
    #[serde(default)]
    pub duration_seconds: Option<f64>,

    /// Optional pause in ms after this scene's voiceover (breath beat).
    /// When present, adds silence after the audio to create natural pacing.
    #[serde(default)]
    pub pause_ms: Option<i64>,

    /// Per-scene stock footage search query override.
    /// When present, this query is used directly for Pexels/stock search
    /// instead of the auto-generated query from scene text + video_keywords.
    /// Gives agents explicit control over what footage each scene gets.
    /// (UX audit GAP #1 fix: agents had zero control over per-scene footage.)
    #[serde(default)]
    pub stock_query: Option<String>,
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
    /// 1. Build the timeline via script.to_timeline
    ///    2. Compile it to HF HTML via timeline.to_hyperframes
    ///    3. Render via hf.render
    ///    This gives agents programmatic control over the render engine,
    ///    connecting HyperFrames to the golden trajectory.
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
    let valid_tts_backends = ["kokoro", "sidecar", "audio8", "gepard", "voicedesign", "higgs"];
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

    // Validate content-format declaration.
    let valid_formats = [
        "presentation",
        "podcast",
        "dialogue",
        "comedy_sketch",
        "romcom",
        "meme_reel",
        "documentary",
        "how_to",
        "listicle",
        "storytime",
        "debate",
        "newsflash",
        "review",
    ];
    if !valid_formats.contains(&spec.format.r#type.as_str()) {
        errors.push(ScriptValidationError {
            field: "format.type".into(),
            message: format!(
                "Invalid format type '{}'. Must be one of: {}",
                spec.format.r#type,
                valid_formats.join(", ")
            ),
        });
    }
    let valid_alternations = ["none", "male_female", "auto"];
    if !valid_alternations.contains(&spec.format.alternation.as_str()) {
        errors.push(ScriptValidationError {
            field: "format.alternation".into(),
            message: format!(
                "Invalid alternation '{}'. Must be one of: {}",
                spec.format.alternation,
                valid_alternations.join(", ")
            ),
        });
    }
    // Music mood must be one of the moods the library/music pipeline knows.
    // Check BOTH the format-level hint and the applied music block (a
    // hand-written `music: {mood: "bogus"}` must not pass silently).
    let mood_candidates = [
        spec.format.music_mood.as_deref(),
        spec.music.as_ref().and_then(|m| m.mood.as_deref()),
    ];
    for mood in mood_candidates.into_iter().flatten() {
        if !VALID_MUSIC_MOODS.contains(&mood) {
            errors.push(ScriptValidationError {
                field: "music.mood".into(),
                message: format!(
                    "Unknown music mood '{}'. Must be one of: {}",
                    mood,
                    VALID_MUSIC_MOODS.join(", ")
                ),
            });
        }
    }
    // Format speaker/scene count constraints.
    let speaker_count = spec.speakers.len();
    if spec.format.min_speakers > 0 && speaker_count < spec.format.min_speakers as usize {
        errors.push(ScriptValidationError {
            field: "format.min_speakers".into(),
            message: format!(
                "Format '{}' needs at least {} speakers (found {})",
                spec.format.r#type, spec.format.min_speakers, speaker_count
            ),
        });
    }
    if spec.format.max_speakers > 0 && speaker_count > spec.format.max_speakers as usize {
        errors.push(ScriptValidationError {
            field: "format.max_speakers".into(),
            message: format!(
                "Format '{}' allows at most {} speakers (found {})",
                spec.format.r#type, spec.format.max_speakers, speaker_count
            ),
        });
    }
    let scene_count = spec.scenes.len();
    if spec.format.min_scenes > 0 && scene_count < spec.format.min_scenes as usize {
        errors.push(ScriptValidationError {
            field: "format.min_scenes".into(),
            message: format!(
                "Format '{}' needs at least {} scenes (found {})",
                spec.format.r#type, spec.format.min_scenes, scene_count
            ),
        });
    }
    if spec.format.max_scenes > 0 && scene_count > spec.format.max_scenes as usize {
        errors.push(ScriptValidationError {
            field: "format.max_scenes".into(),
            message: format!(
                "Format '{}' allows at most {} scenes (found {})",
                spec.format.r#type, spec.format.max_scenes, scene_count
            ),
        });
    }
    // Alternation requires ≥2 distinct genders when explicitly requested.
    if spec.format.alternation == "male_female" {
        let genders: HashSet<&str> =
            spec.speakers.values().map(|s| s.gender.as_str()).collect();
        let distinct = genders.iter().filter(|g| **g != "unknown").count();
        if distinct < 2 {
            errors.push(ScriptValidationError {
                field: "format.alternation".into(),
                message: format!(
                    "format.alternation='male_female' needs speakers of two distinct genders (male + female). Declare speaker 'gender' or use a counterpart voice (e.g. voice.design a female host). Found genders: {:?}",
                    genders
                ),
            });
        }
    }

    errors
}

/// Parse a JSON string into a ScriptSpec, applying defaults for missing fields.
///
/// Normalizes agent-friendly shorthand formats before deserialization:
/// - speakers array → map (UX audit GAP #2)
/// - background string → object (UX audit GAP)
/// - duration_seconds → duration_override_ms (UX audit GAP #5)
pub fn parse_script(json: &str) -> Result<ScriptSpec, serde_json::Error> {
    // Normalize agent-friendly shorthand formats before deserialization.
    // Agents write arrays for speakers, bare strings for background, and
    // duration_seconds instead of duration_override_ms.
    let mut root: serde_json::Value = serde_json::from_str(json)?;

    if let Some(obj) = root.as_object_mut() {
        // --- Normalize speakers: array → map ---
        // Agents write: [{"id": "narrator", "voice": "kokoro:af_heart"}]
        // Schema expects: {"narrator": {"voice": "kokoro:af_heart"}}
        if let Some(speakers_val) = obj.get("speakers") {
            if let Some(arr) = speakers_val.as_array() {
                let mut map = serde_json::Map::new();
                for (i, entry) in arr.iter().enumerate() {
                    if let Some(entry_obj) = entry.as_object() {
                        let id = entry_obj
                            .get("id")
                            .or_else(|| entry_obj.get("profile_id"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| format!("speaker_{}", i + 1));
                        let mut spec_obj = entry_obj.clone();
                        spec_obj.remove("id");
                        spec_obj.remove("profile_id");
                        map.insert(id, serde_json::Value::Object(spec_obj));
                    }
                }
                obj.insert("speakers".into(), serde_json::Value::Object(map));
            }
        }

        // --- Normalize background: string → object ---
        // Agents write: "procedural"
        // Schema expects: {"type": "procedural"}
        if let Some(bg_val) = obj.get("background") {
            if let Some(s) = bg_val.as_str() {
                let mut bg_obj = serde_json::Map::new();
                bg_obj.insert("type".into(), serde_json::Value::String(s.to_string()));
                obj.insert("background".into(), serde_json::Value::Object(bg_obj));
            }
        }

        // --- Normalize format: string → object ---
        // Agents write: "format": "podcast"
        // Schema expects: {"type": "podcast"}
        if let Some(fmt_val) = obj.get("format") {
            if let Some(s) = fmt_val.as_str() {
                let mut fmt_obj = serde_json::Map::new();
                fmt_obj.insert("type".into(), serde_json::Value::String(s.to_string()));
                obj.insert("format".into(), serde_json::Value::Object(fmt_obj));
            }
        }

        // --- Normalize per-scene background: object → string ---
        // Agents write per-scene: {"type": "gameplay", "stock_query": "octopus", "orientation": "9:16"}
        // SceneSpec.background expects: Option<String> (just the type or preset name).
        // Extract the type as the background string and promote stock_query to scene level.
        if let Some(scenes) = obj.get_mut("scenes") {
            if let Some(scenes_arr) = scenes.as_array_mut() {
                for scene in scenes_arr.iter_mut() {
                    if let Some(scene_obj) = scene.as_object_mut() {
                        // Check if background is a JSON object (not a string)
                        let is_object_bg = scene_obj
                            .get("background")
                            .and_then(|bg| bg.as_object())
                            .is_some();

                        if is_object_bg {
                            // Extract the background object immutably first
                            let (bg_type, stock_query) = {
                                let bg_val = scene_obj.get("background").unwrap();
                                let bg_obj = bg_val.as_object().unwrap();
                                let bg_type = bg_obj
                                    .get("type")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string());
                                let stock_query = bg_obj.get("stock_query").cloned();
                                (bg_type, stock_query)
                            };

                            // Now mutate: set background string
                            if let Some(t) = bg_type {
                                scene_obj
                                    .insert("background".into(), serde_json::Value::String(t));
                            }
                            // Promote stock_query to scene level if not already present
                            if let Some(sq) = stock_query {
                                if !scene_obj.contains_key("stock_query") {
                                    scene_obj.insert("stock_query".into(), sq);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Track explicitly-set fields so correlated format defaults never clobber
    // an explicit agent choice — e.g. "meme_brolls": {"enabled": false} must
    // beat a format's reaction_memes=true even though the value equals the
    // default (value-based checks cannot tell the difference; presence-based
    // ones can). Sub-keys are tracked for the blocks apply_format touches so
    // setting tts.backend alone does NOT suppress the format's pacing defaults.
    let mut explicitly_set: HashSet<String> = HashSet::new();
    if let Some(obj) = root.as_object() {
        for key in obj.keys() {
            explicitly_set.insert(key.clone());
            for (block, fields) in [
                ("tts", &["default_speed", "default_temperature"][..]),
                ("meme_brolls", &["enabled"][..]),
                ("stickers", &["enabled"][..]),
                ("music", &["mood"][..]),
            ] {
                if key == block {
                    if let Some(bo) = obj.get(block).and_then(|v| v.as_object()) {
                        for f in fields {
                            if bo.contains_key(*f) {
                                explicitly_set.insert(format!("{}.{}", block, f));
                            }
                        }
                    }
                }
            }
        }
    }

    let mut spec: ScriptSpec = serde_json::from_value(root)?;

    // Infer speaker genders declared "auto" (Kokoro voice id prefix; the MCP
    // layer enriches voicedesign profiles with their registered gender).
    for speaker in spec.speakers.values_mut() {
        if speaker.gender.is_empty() || speaker.gender == "auto" {
            speaker.gender = infer_gender(&speaker.voice, "");
        }
    }

    // Apply content-format correlated defaults (pacing / reactions / music).
    // Runs BEFORE apply_theme so the theme's caption colors still win.
    apply_format(&mut spec, &explicitly_set);

    // Auto-assign scene IDs if missing
    for (i, scene) in spec.scenes.iter_mut().enumerate() {
        if scene.id.is_empty() {
            scene.id = format!("scene_{:03}", i + 1);
        }
    }

    // Convert duration_seconds → duration_override_ms for each scene.
    // Agents naturally think in seconds, not milliseconds. If both are
    // set, duration_override_ms wins. (UX audit GAP #5 fix.)
    for scene in spec.scenes.iter_mut() {
        if scene.duration_override_ms.is_none() {
            if let Some(secs) = scene.duration_seconds {
                scene.duration_override_ms = Some((secs * 1000.0) as i64);
            }
        }
    }

    // Auto-extract video_keywords from title if not provided.
    // (Round-13: topic-aware video search upgrade.)
    if spec.video_keywords.is_empty() && !spec.title.is_empty() {
        spec.video_keywords = extract_topic_keywords(&spec.title);
    }

    // Auto-upgrade background type: if the caller set type="static" but
    // provided video_keywords, silently upgrade to "procedural" so stock
    // backgrounds are fetched instead of procedural gradients. The caller
    // likely wrote "static" by mistake — type="static" is an explicit opt-out
    // of stock footage, which contradicts providing video search keywords.
    // (Director v5 trial: cold-agent wrote type="static", bypassing all
    // Pexels fetches, producing 0/12 video_source_quality.)
    if spec.background.r#type == "static" && !spec.video_keywords.is_empty() {
        spec.background.r#type = "procedural".to_string();
    }

    // Apply theme preset.
    apply_theme(&mut spec);

    Ok(spec)
}

/// Infer a speaker's gender from a Kokoro voice id prefix or free-text
/// description. Kokoro voice ids encode gender in the locale prefix
/// (e.g. "af_" = American female, "am_" = American male, "bm_" = British
/// male — `<language><gender>_<name>`). Free-text heuristics look for explicit
/// gendered tokens in personality or instruct strings. Returns "male",
/// "female", or "unknown".
pub fn infer_gender(voice: &str, free_text: &str) -> String {
    // 1. Kokoro voice id prefix: "kokoro:af_heart" or "af_heart" or
    //    registry-id form "kokoro_af_heart".
    let bare = match voice.rsplit(':').next() {
        Some(v) => v,
        None => voice,
    };
    let bare = bare.strip_prefix("kokoro_").unwrap_or(bare);
    if let Some(locale) = bare.split('_').next() {
        if locale.len() >= 2 {
            let gender_char = locale.as_bytes()[locale.len() - 1];
            let language = &locale[..locale.len() - 1];
            if !language.is_empty() && language.chars().all(|c| c.is_ascii_alphabetic()) {
                match gender_char {
                    b'f' | b'F' => return "female".to_string(),
                    b'm' | b'M' => return "male".to_string(),
                    _ => {}
                }
            }
        }
    }

    // 2. Free-text token heuristics (explicit gendered vocabulary only).
    let female_tokens = [
        "female", "woman", "women", "girl", "she", "her", "lady", "queen",
        "mother", "feminine", "mrs", "madam", "sister", "daughter", "herself",
    ];
    let male_tokens = [
        "male", "man", "men", "boy", "he", "him", "his", "sir", "king",
        "father", "masculine", "mr", "brother", "son", "uncle", "himself",
    ];
    let text = free_text.to_lowercase();
    let mut female_hits = 0usize;
    let mut male_hits = 0usize;
    for token in text.split(|c: char| !c.is_alphanumeric()) {
        if token.is_empty() {
            continue;
        }
        if female_tokens.contains(&token) {
            female_hits += 1;
        } else if male_tokens.contains(&token) {
            male_hits += 1;
        }
    }
    if female_hits > male_hits {
        "female".to_string()
    } else if male_hits > female_hits {
        "male".to_string()
    } else {
        "unknown".to_string()
    }
}

/// Apply the content-format correlated defaults from `spec.format` to the
/// rest of the spec — but ONLY for fields the agent did not explicitly set
/// (presence-based: `explicitly_set` holds the top-level keys the agent wrote;
/// a value that merely equals a default is still an explicit choice).
fn apply_format(spec: &mut ScriptSpec, explicitly_set: &HashSet<String>) {
    let f = &spec.format;
    // Pacing: only when the specific tts field was not set by the agent
    // (setting tts.backend alone must not suppress the format's pacing).
    if f.default_speed > 0.0
        && spec.tts.default_speed == 1.0
        && !explicitly_set.contains("tts.default_speed")
    {
        spec.tts.default_speed = f.default_speed;
    }
    if let Some(t) = f.default_temperature {
        if spec.tts.default_temperature.is_none() && !explicitly_set.contains("tts.default_temperature")
        {
            spec.tts.default_temperature = Some(t);
        }
    }
    // Reaction memes (GIPHY pop-ins) — never override an explicit choice.
    if f.reaction_memes
        && !spec.meme_brolls.enabled
        && !explicitly_set.contains("meme_brolls.enabled")
    {
        spec.meme_brolls.enabled = true;
    }
    // Sticker behavior.
    match f.sticker_mode.as_str() {
        "none" if !explicitly_set.contains("stickers.enabled") => {
            spec.stickers.enabled = false
        }
        "reaction" => {
            // Stickers stay enabled — the sticker.auto pipeline is already
            // reaction-driven (intent + emphatic keywords).
        }
        _ => {}
    }
    // Music mood (auto-select hint). Fill a missing mood on an explicit music
    // block, or create one when the agent supplied no music at all. A mood the
    // agent set explicitly is never overwritten.
    if let Some(mood) = &f.music_mood {
        match spec.music.as_mut() {
            Some(m) => {
                if m.mood.is_none() {
                    m.mood = Some(mood.clone());
                }
            }
            None => {
                let mut m = MusicSpec::default();
                m.mood = Some(mood.clone());
                spec.music = Some(m);
            }
        }
    }
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
        _ => {
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

    /// The voice-design engine is a first-class backend: scripts must be able
    /// to declare `tts.backend = "voicedesign"` so character voices synthesize
    /// directly with the Qwen3 VoiceDesign model (no cloning).
    #[test]
    fn test_validate_voicedesign_backend_is_valid() {
        let spec = parse_script(
            r#"{
            "tts": {"backend": "voicedesign"},
            "speakers": {"alice": {"voice": "air_analyst"}},
            "scenes": [{"speaker": "alice", "text": "Hi"}]
        }"#,
        )
        .unwrap();

        assert_eq!(spec.tts.backend, "voicedesign");
        let errors = validate_script(&spec);
        assert!(
            errors.iter().all(|e| e.field != "tts.backend"),
            "voicedesign must be a valid TTS backend, got: {:?}",
            errors
        );
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
        assert_eq!(spec.background.r#type, "procedural");
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
        assert!(
            !spec.background.loop_,
            "JSON key 'loop' must be accepted as alias for Rust field 'loop_'"
        );

        // Also verify the underscore form still works (backward compat).
        let json2 = r#"{
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [{"speaker": "alice", "text": "Hi"}],
            "background": {"type": "procedural", "loop_": false}
        }"#;
        let spec2 = parse_script(json2).unwrap();
        assert!(!spec2.background.loop_);
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

    /// Per-scene performance direction: emote (free-form), tone, speed, pitch
    /// must parse and survive round-trip — the line-level tonality plumbing.
    #[test]
    fn test_scene_performance_direction_fields() {
        let json = r#"{
            "speakers": {"narrator": {"voice": "ishan"}},
            "scenes": [
                {"speaker": "narrator", "text": "I am furious.", "emote": "angry", "tone": "low growl, teeth clenched", "speed": 1.3, "pitch": 1.1},
                {"speaker": "narrator", "text": "Calm now.", "emote": "whisper"}
            ]
        }"#;
        let spec = parse_script(json).unwrap();
        assert_eq!(spec.scenes[0].emote.as_deref(), Some("angry"));
        assert_eq!(
            spec.scenes[0].tone.as_deref(),
            Some("low growl, teeth clenched")
        );
        assert_eq!(spec.scenes[0].speed, Some(1.3));
        assert_eq!(spec.scenes[0].pitch, Some(1.1));
        // Defaults: no tone/speed/pitch → None.
        assert_eq!(spec.scenes[1].tone, None);
        assert_eq!(spec.scenes[1].speed, None);
        assert_eq!(spec.scenes[1].pitch, None);
        assert_eq!(spec.scenes[1].emote.as_deref(), Some("whisper"));
    }

    /// Verify tts.voice parses and survives round-trip, and that a speaker
    /// voice of "default" is preserved verbatim (resolution to a profile
    /// happens in the MCP layer where config is visible).
    #[test]
    fn test_tts_voice_field() {
        let json = r#"{
            "tts": {"backend": "gepard", "voice": "ishan_gepard"},
            "speakers": {"narrator": {"voice": "default"}},
            "scenes": [{"speaker": "narrator", "text": "Hi"}]
        }"#;
        let spec = parse_script(json).unwrap();
        assert_eq!(spec.tts.backend, "gepard");
        assert_eq!(spec.tts.voice.as_deref(), Some("ishan_gepard"));
        assert_eq!(spec.speakers["narrator"].voice, "default");
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

    // ========================================================================
    // Regression tests: UX audit GAP fixes (Phase 21)
    // ========================================================================

    /// Speakers array format (agent-friendly).
    #[test]
    fn test_parse_speakers_array_format() {
        let json = r#"{
            "speakers": [
                {"id": "narrator", "voice": "kokoro:af_heart"},
                {"id": "backup", "voice": "kokoro:am_michael"}
            ],
            "scenes": [{"speaker": "narrator", "text": "Hello!"}]
        }"#;
        let spec = parse_script(json).unwrap();
        assert_eq!(spec.speakers.len(), 2);
        assert!(spec.speakers.contains_key("narrator"));
        assert!(spec.speakers.contains_key("backup"));
        assert_eq!(spec.speakers["narrator"].voice, "kokoro:af_heart");
    }

    /// Background string shorthand (agent-friendly).
    #[test]
    fn test_parse_background_string_shorthand() {
        let json = r#"{
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [{"speaker": "alice", "text": "Hi"}],
            "background": "procedural"
        }"#;
        let spec = parse_script(json).unwrap();
        assert_eq!(spec.background.r#type, "procedural");
    }

    /// Duration seconds converted to ms.
    #[test]
    fn test_duration_seconds_converted_to_ms() {
        let json = r#"{
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [
                {"speaker": "alice", "text": "First.", "duration_seconds": 5.0},
                {"speaker": "alice", "text": "Second.", "duration_seconds": 10.5}
            ]
        }"#;
        let spec = parse_script(json).unwrap();
        assert_eq!(spec.scenes[0].duration_override_ms, Some(5000));
        assert_eq!(spec.scenes[1].duration_override_ms, Some(10500));
    }

    /// Duration override ms takes precedence.
    #[test]
    fn test_duration_override_ms_wins() {
        let json = r#"{
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [
                {"speaker": "alice", "text": "Hi.", "duration_override_ms": 3000, "duration_seconds": 10.0}
            ]
        }"#;
        let spec = parse_script(json).unwrap();
        assert_eq!(spec.scenes[0].duration_override_ms, Some(3000));
    }

    /// Per-scene stock_query preserved.
    #[test]
    fn test_scene_stock_query_preserved() {
        let json = r#"{
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [
                {"speaker": "alice", "text": "Coffee rocks.", "stock_query": "coffee beans roasting"},
                {"speaker": "alice", "text": "Tea is great."}
            ]
        }"#;
        let spec = parse_script(json).unwrap();
        assert_eq!(spec.scenes[0].stock_query.as_deref(), Some("coffee beans roasting"));
        assert!(spec.scenes[1].stock_query.is_none());
    }

    /// Per-scene background object normalization (agent-friendly format).
    /// Agents write: {"type": "gameplay", "stock_query": "octopus", "orientation": "9:16"}
    /// SceneSpec.background expects: Option<String> (just the type).
    /// The stock_query should be promoted to scene level.
    #[test]
    fn test_parse_scene_background_object_normalization() {
        let json = r#"{
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [
                {
                    "speaker": "alice", "text": "Deep ocean.",
                    "background": {"type": "gameplay", "stock_query": "octopus underwater", "orientation": "9:16"}
                },
                {
                    "speaker": "alice", "text": "Normal scene.",
                    "background": "procedural"
                },
                {
                    "speaker": "alice", "text": "No background."
                }
            ]
        }"#;
        let spec = parse_script(json).unwrap();
        // Scene 1: background object → string, stock_query promoted
        assert_eq!(spec.scenes[0].background.as_deref(), Some("gameplay"));
        assert_eq!(spec.scenes[0].stock_query.as_deref(), Some("octopus underwater"));
        // Scene 2: background string stays as-is
        assert_eq!(spec.scenes[1].background.as_deref(), Some("procedural"));
        assert!(spec.scenes[1].stock_query.is_none());
        // Scene 3: no background at all
        assert!(spec.scenes[2].background.is_none());
    }

    /// Per-scene background object does NOT overwrite existing stock_query.
    #[test]
    fn test_scene_background_object_preserves_existing_stock_query() {
        let json = r#"{
            "speakers": {"alice": {"voice": "kokoro:af_heart"}},
            "scenes": [
                {
                    "speaker": "alice", "text": "Coffee.",
                    "stock_query": "coffee beans",
                    "background": {"type": "gameplay", "stock_query": "different query"}
                }
            ]
        }"#;
        let spec = parse_script(json).unwrap();
        // Existing stock_query should NOT be overwritten by background object's stock_query
        assert_eq!(spec.scenes[0].stock_query.as_deref(), Some("coffee beans"));
        assert_eq!(spec.scenes[0].background.as_deref(), Some("gameplay"));
    }

    // ========================================================================
    // Content-format + gender alternation tests (Phase 176)
    // ========================================================================

    /// Agent-friendly "format": "podcast" shorthand normalizes to a full spec.
    #[test]
    fn test_format_shorthand_normalized() {
        let json = r#"{
            "format": "podcast",
            "speakers": {
                "host": {"voice": "kokoro:am_michael", "gender": "male"},
                "guest": {"voice": "kokoro:af_heart", "gender": "female"}
            },
            "scenes": [
                {"speaker": "host", "text": "Welcome."},
                {"speaker": "guest", "text": "Thanks!"}
            ]
        }"#;
        let spec = parse_script(json).unwrap();
        assert_eq!(spec.format.r#type, "podcast");
        assert_eq!(spec.format.alternation, "none");
        assert_eq!(spec.format.sticker_mode, "character");
    }

    /// Full format object with alternation parses and validates cleanly.
    #[test]
    fn test_format_object_alternation_valid() {
        let json = r#"{
            "format": {"type": "podcast", "alternation": "male_female"},
            "speakers": {
                "host": {"voice": "kokoro:am_michael", "gender": "male"},
                "guest": {"voice": "kokoro:af_heart", "gender": "female"}
            },
            "scenes": [
                {"speaker": "host", "text": "Welcome."},
                {"speaker": "guest", "text": "Thanks!"}
            ]
        }"#;
        let spec = parse_script(json).unwrap();
        assert_eq!(spec.format.r#type, "podcast");
        assert_eq!(spec.format.alternation, "male_female");
        let errors = validate_script(&spec);
        assert!(errors.is_empty(), "got: {:?}", errors);
    }

    /// Correlated defaults are applied when the agent didn't set them.
    #[test]
    fn test_format_correlated_defaults_applied() {
        let json = r#"{
            "format": {
                "type": "meme_reel",
                "default_speed": 1.1,
                "default_temperature": 0.85,
                "reaction_memes": true,
                "sticker_mode": "reaction",
                "music_mood": "energetic"
            },
            "speakers": {"narrator": {"voice": "kokoro:af_heart"}},
            "scenes": [{"speaker": "narrator", "text": "Short punchline."}]
        }"#;
        let spec = parse_script(json).unwrap();
        assert_eq!(spec.tts.default_speed, 1.1);
        assert_eq!(spec.tts.default_temperature, Some(0.85));
        assert!(spec.meme_brolls.enabled, "reaction_memes should enable meme brolls");
        assert!(spec.stickers.enabled, "reaction sticker mode keeps stickers on");
        let music = spec.music.expect("music_mood should create a music spec");
        assert_eq!(music.mood.as_deref(), Some("energetic"));
    }

    /// Explicit agent fields always beat format correlated defaults.
    #[test]
    fn test_format_explicit_fields_win() {
        let json = r#"{
            "tts": {"default_speed": 1.3},
            "meme_brolls": {"enabled": false},
            "format": {"type": "meme_reel", "default_speed": 1.1, "reaction_memes": true},
            "speakers": {"narrator": {"voice": "kokoro:af_heart"}},
            "scenes": [{"speaker": "narrator", "text": "Hi."}]
        }"#;
        let spec = parse_script(json).unwrap();
        assert_eq!(spec.tts.default_speed, 1.3, "explicit tts speed must win");
        assert!(
            !spec.meme_brolls.enabled,
            "explicit meme_brolls.enabled=false must win"
        );
    }

    /// Kokoro prefix + free-text gender inference.
    #[test]
    fn test_gender_inference_kokoro_prefix() {
        assert_eq!(infer_gender("kokoro:af_heart", ""), "female");
        assert_eq!(infer_gender("af_heart", ""), "female");
        assert_eq!(infer_gender("kokoro_af_bella", ""), "female");
        assert_eq!(infer_gender("kokoro:am_michael", ""), "male");
        assert_eq!(infer_gender("kokoro:bm_george", ""), "male");
        assert_eq!(infer_gender("air_analyst", ""), "unknown");
        assert_eq!(
            infer_gender("ishan", "warm and friendly female voice"),
            "female"
        );
        assert_eq!(infer_gender("ishan", "deep male narrator"), "male");
        // "her" inside a word must NOT count as a female signal.
        assert_eq!(infer_gender("x", "heritage"), "unknown");
    }

    /// Speakers with gender "auto" get inference applied at parse time.
    #[test]
    fn test_speaker_gender_auto_inferred_at_parse() {
        let json = r#"{
            "speakers": {
                "a": {"voice": "kokoro:af_heart"},
                "b": {"voice": "kokoro:am_michael"}
            },
            "scenes": [
                {"speaker": "a", "text": "Hi"},
                {"speaker": "b", "text": "Hi"}
            ]
        }"#;
        let spec = parse_script(json).unwrap();
        assert_eq!(spec.speakers["a"].gender, "female");
        assert_eq!(spec.speakers["b"].gender, "male");
    }

    /// Explicit speaker gender is preserved verbatim.
    #[test]
    fn test_speaker_gender_explicit_preserved() {
        let json = r#"{
            "speakers": {
                "lead": {"voice": "air_analyst", "gender": "nonbinary"}
            },
            "scenes": [{"speaker": "lead", "text": "Hi"}]
        }"#;
        let spec = parse_script(json).unwrap();
        assert_eq!(spec.speakers["lead"].gender, "nonbinary");
    }

    /// male_female alternation errors when all speakers share one gender.
    #[test]
    fn test_alternation_male_female_validates() {
        // Two distinct genders → valid.
        let ok_json = r#"{
            "format": {"type": "podcast", "alternation": "male_female"},
            "speakers": {
                "host": {"voice": "kokoro:am_michael", "gender": "male"},
                "guest": {"voice": "kokoro:af_heart", "gender": "female"}
            },
            "scenes": [
                {"speaker": "host", "text": "Welcome."},
                {"speaker": "guest", "text": "Thanks."}
            ]
        }"#;
        let spec = parse_script(ok_json).unwrap();
        let errors = validate_script(&spec);
        assert!(
            errors.iter().all(|e| e.field != "format.alternation"),
            "got: {:?}",
            errors
        );

        // One gender → error.
        let bad_json = r#"{
            "format": {"type": "podcast", "alternation": "male_female"},
            "speakers": {
                "host": {"voice": "kokoro:am_michael", "gender": "male"},
                "cohost": {"voice": "kokoro:am_george", "gender": "male"}
            },
            "scenes": [
                {"speaker": "host", "text": "Welcome."},
                {"speaker": "cohost", "text": "Thanks."}
            ]
        }"#;
        let spec = parse_script(bad_json).unwrap();
        let errors = validate_script(&spec);
        assert!(
            errors.iter().any(|e| e.field == "format.alternation"),
            "got: {:?}",
            errors
        );
    }

    /// Format speaker-count constraints are validated.
    #[test]
    fn test_format_speaker_count_validated() {
        let json = r#"{
            "format": {"type": "podcast", "min_speakers": 2, "max_speakers": 4},
            "speakers": {"only": {"voice": "kokoro:af_heart"}},
            "scenes": [{"speaker": "only", "text": "Hi"}]
        }"#;
        let spec = parse_script(json).unwrap();
        let errors = validate_script(&spec);
        assert!(
            errors.iter().any(|e| e.field == "format.min_speakers"),
            "got: {:?}",
            errors
        );
    }

    /// Unknown format types are rejected.
    #[test]
    fn test_invalid_format_type_rejected() {
        let json = r#"{
            "format": {"type": "fireside"},
            "speakers": {"narrator": {"voice": "kokoro:af_heart"}},
            "scenes": [{"speaker": "narrator", "text": "Hi"}]
        }"#;
        let spec = parse_script(json).unwrap();
        let errors = validate_script(&spec);
        assert!(
            errors.iter().any(|e| e.field == "format.type"),
            "got: {:?}",
            errors
        );
    }

    /// No format key = presentation, no constraints (backward compatible).
    #[test]
    fn test_default_format_is_presentation() {
        let json = r#"{
            "speakers": {"narrator": {"voice": "kokoro:af_heart"}},
            "scenes": [{"speaker": "narrator", "text": "Hi"}]
        }"#;
        let spec = parse_script(json).unwrap();
        assert_eq!(spec.format.r#type, "presentation");
        assert_eq!(spec.format.alternation, "none");
        assert_eq!(spec.format.sticker_mode, "character");
        let errors = validate_script(&spec);
        assert!(errors.is_empty(), "got: {:?}", errors);
    }
