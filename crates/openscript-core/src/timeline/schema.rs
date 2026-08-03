use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::types::{EditorialRole, TrackType};

/// The top-level timeline object. This is serialized to/from JSON on disk.
/// Every MCP tool that mutates state loads → mutates → saves this struct.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Timeline {
    pub version: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub source: PathBuf,
    pub target: RenderTarget,
    pub segments: Vec<Segment>,
    pub tracks: TrackMap,
    pub directives: Directives,
    pub assets: AssetRegistry,
    pub effects: Effects,
    /// When true, renders in raw mode: strips zoompan and looping from b-roll.
    /// Used for segmentation-correctness audits per SEGMENTATION_ARCHITECTURE.md.
    #[serde(default)]
    pub raw_render: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RenderTarget {
    pub aspect: String,
    pub fps: u32,
    pub max_duration: Option<u32>,
    /// Output width in pixels. If absent, derived from `aspect`:
    /// `9:16` → 1080, `16:9` → 1920, `1:1` → 1080.
    #[serde(default)]
    pub width: Option<u32>,
    /// Output height in pixels. If absent, derived from `aspect`:
    /// `9:16` → 1920, `16:9` → 1080, `1:1` → 1080.
    #[serde(default)]
    pub height: Option<u32>,
}

impl RenderTarget {
    /// Resolve the output width, falling back to the aspect-ratio default
    /// when `width` is `None`. Used by the ffmpeg filter graph builder so
    /// renders honour the timeline's resolution instead of a hardcoded
    /// 1080×1920.
    pub fn resolve_width(&self) -> u32 {
        self.width.unwrap_or(match self.aspect.as_str() {
            "16:9" => 1920,
            "1:1" => 1080,
            _ => 1080, // "9:16" and any unknown → portrait default
        })
    }

    /// Resolve the output height, falling back to the aspect-ratio default
    /// when `height` is `None`.
    pub fn resolve_height(&self) -> u32 {
        self.height.unwrap_or(match self.aspect.as_str() {
            "16:9" => 1080,
            "1:1" => 1080,
            _ => 1920, // "9:16" and any unknown → portrait default
        })
    }
}

/// A single cut segment from the source video.
/// The `start`/`end` are in SECONDS of the original source video.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Segment {
    pub id: String,
    pub start: f64,
    pub end: f64,
    pub caption: String,
    pub crossfade_ms: u32,
    pub semantic_role: Option<EditorialRole>,
}

/// Map of track type → ordered list of events on that track.
pub type TrackMap = HashMap<TrackType, Vec<TimelineEvent>>;

/// Base event placed on any track. Uses serde flatten for kind-specific fields.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TimelineEvent {
    pub id: String,
    pub asset_id: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub offset_ms: i64,
    pub gain_db: f64,
    pub fade_in_ms: u32,
    pub fade_out_ms: u32,
    pub tags: Vec<String>,
    pub provenance: Option<Provenance>,
    /// Kind-specific fields flattened into the event.
    /// Uses `event_type` as discriminator. Defaults to Dialogue for
    /// backward-compatibility with Python-saved timelines that omit it.
    #[serde(flatten, default)]
    pub kind: EventKind,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Provenance {
    pub tool: String,
    pub editorial_role: Option<String>,
    pub concept: Option<String>,
}

/// Per-kind event data. The `#[serde(flatten)]` on TimelineEvent merges these fields.
/// Defaults to Dialogue for backward-compatibility with Python timelines.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "event_type", rename_all = "snake_case")]
#[derive(Default)]
pub enum EventKind {
    #[default]
    Dialogue,
    Voiceover {
        #[serde(default)]
        voice_profile_id: String,
        #[serde(default)]
        text: String,
        #[serde(default)]
        estimated_duration_ms: i64,
    },
    Caption {
        #[serde(default)]
        text: String,
        #[serde(default)]
        style: String,
        #[serde(default)]
        word_timings: Vec<WordTiming>,
    },
    Broll {
        #[serde(default)]
        concept: String,
        #[serde(default)]
        source_provider: String,
        #[serde(default)]
        transition_style: String,
        #[serde(default)]
        crop_mode: String,
        #[serde(default)]
        orientation: String,
        #[serde(default)]
        motion_intensity: String,
    },
    Music {
        #[serde(default)]
        mood: String,
        #[serde(default)]
        energy: String,
        #[serde(default)]
        bpm: Option<u32>,
        #[serde(default)]
        loopability: bool,
        #[serde(default)]
        intro_friendly: bool,
        #[serde(default)]
        cta_friendly: bool,
        #[serde(default)]
        loudness_target_lufs: f64,
        #[serde(default)]
        loop_mode: String,
        #[serde(default)]
        ducking_policy: String,
    },
    Sfx {
        #[serde(default)]
        editorial_role: String,
        #[serde(default)]
        category: String,
        #[serde(default)]
        subcategory: String,
        #[serde(default)]
        duration_ms: i64,
        #[serde(default)]
        sample_rate: u32,
        #[serde(default)]
        peak_db: f64,
        #[serde(default)]
        loudness_lufs: f64,
        #[serde(default)]
        recommended_gain_db: f64,
        #[serde(default)]
        recommended_use: String,
        #[serde(default)]
        safe_overlay: bool,
    },
}


#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct WordTiming {
    pub word: String,
    pub start_ms: i64,
    pub end_ms: i64,
}

/// Timeline-level directives (mixing, rendering, transitions).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Directives {
    pub ducking: Vec<DuckingDirective>,
    pub transitions: Vec<TransitionDirective>,
    pub mix: MixConfig,
    pub render_backend: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DuckingDirective {
    pub when: String,
    pub target_track: String,
    pub reduction_db: f64,
    pub attack_ms: u32,
    pub release_ms: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TransitionDirective {
    pub from_segment: usize,
    pub to_segment: usize,
    pub style: String,
    pub duration_ms: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MixConfig {
    pub master_gain_db: f64,
    pub limiter_threshold_db: f64,
    pub normalize_to_lufs: f64,
}

/// Registry of all assets referenced by track events.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AssetRegistry {
    pub voices: HashMap<String, serde_json::Value>,
    pub broll: HashMap<String, serde_json::Value>,
    pub music: HashMap<String, serde_json::Value>,
    pub sfx: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub captions: HashMap<String, serde_json::Value>,
}

/// Visual/audio effects applied during render.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Effects {
    pub burn_captions: bool,
    pub audio: AudioEffects,
    /// Caption style used (e.g., "word_highlight", "standard", "kinetic")
    #[serde(default)]
    pub caption_style: Option<String>,
}

fn default_true() -> bool { true }

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AudioEffects {
    #[serde(default = "default_true")]
    pub loudnorm: bool,
}
