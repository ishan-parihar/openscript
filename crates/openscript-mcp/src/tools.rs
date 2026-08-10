pub(crate) use openscript_core::amplitude::extract_amplitude;
pub(crate) use openscript_core::background::assign_backgrounds;
use openscript_core::captions::{estimate_word_timings, generate_ass, WordTiming};
pub(crate) use openscript_core::captions::{CaptionSegment};
pub(crate) use openscript_core::script::{parse_script, validate_script, CaptionsSpec};
pub(crate) use openscript_core::srt::{analyze_srt, build_edl, group_entries, parse_srt, write_srt};
pub(crate) use openscript_core::sticker::{generate_sticker_composition, StickerPreset};
use openscript_core::timeline::Timeline;
use openscript_core::types::TrackType;
pub(crate) use openscript_transcribe::transcriber::transcribe_with_engine;
use openscript_ffmpeg::gpu::GpuConfig;
use serde_json::json;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use crate::error::ToolError;
pub(crate) use crate::server::report_progress;

// ---------------------------------------------------------------------------
// Domain handler modules (split from this file — pure-move refactor).
// Each module `use super::*` so it sees this file's private helpers/imports;
// handlers are re-exported here so route_tool / tests / crate::tools::
// callers resolve unchanged.
// ---------------------------------------------------------------------------
mod tools_core;
mod tools_audio;
mod tools_broll;
mod tools_verify;
mod tools_media;
mod tools_script;
mod tools_sticker;
mod tools_system;
mod tools_asset;
mod tools_character;

pub(crate) use tools_core::*;
pub(crate) use tools_audio::*;
pub(crate) use tools_broll::*;
pub(crate) use tools_verify::*;
pub(crate) use tools_media::*;
pub(crate) use tools_script::*;
pub(crate) use tools_sticker::*;
pub(crate) use tools_system::*;
pub(crate) use tools_asset::*;
pub(crate) use tools_character::*;

// ---------------------------------------------------------------------------
// Tool definitions: 103 static in this array + 6 dynamic hf.* = 109 total
// (43 original + 5 hf.* + 1 composition.render + 6 script.* + 2 background.* + 2 sticker.* + 2 script.to_* + 1 stock.fetch + 1 youtube.download + 1 youtube.search + 1 stock.search + 1 media.search + 1 gif.search + 1 timeline.inspect + 3 library.* + 2 auto_assign.* + broll.keywords/broll.validate_keywords/broll.repair/broll.auto/broll.probe + sticker.keywords/sticker.validate_keywords/sticker.auto + asset.* + voice.design + character.*)
// ---------------------------------------------------------------------------

/// Resolve the fonts directory for ASS subtitle rendering.
/// Checks OPENSCRIPT_FONTS_DIR env var, then falls back to $CWD/mcp/fonts.
fn resolve_fonts_dir() -> Option<String> {
    let fonts = std::env::var("OPENSCRIPT_FONTS_DIR")
        .unwrap_or_else(|_| {
            std::env::current_dir()
                .map(|d| d.join("mcp/fonts").to_string_lossy().to_string())
                .unwrap_or_default()
        });
    if std::path::Path::new(&fonts).exists() {
        Some(fonts)
    } else {
        None
    }
}

/// Truncate a string to at most `max` bytes at a UTF-8 char boundary.
/// Safe for logging titles with multibyte chars (em-dashes, curly apostrophes)
/// that would otherwise panic a naive `&s[..n]` slice.
pub(crate) fn truncate_utf8(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut idx = max;
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    &s[..idx]
}

pub fn tool_definitions() -> serde_json::Value {
    let mut tools = json!([
        // ===================================================================
        // GROUP 1: CORE PIPELINE — Transcribe, caption, and render
        // ===================================================================
        {
            "name": "transcribe",
            "description": "Convert spoken audio to word-level SRT subtitles. Uses HinglishGgml engine (whisper.cpp + Hindi2Hinglish-Apex-GGML) — produces native Latin-script Hinglish output from Hindi audio. No LLM post-processing needed. ALWAYS call this first on any raw video — it produces the SRT that every other tool depends on. Returns: output_srt_path, entry_count, phrase_srt_path, word_srt_path.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "media_path": {"type": "string", "description": "Path to video or audio file to transcribe"},
                    "output_srt_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Optional output SRT path. Auto-generated if omitted."},
                    "language_hint": {"type": "string", "default": "auto", "description": "Language hint: 'auto' (detect), 'hi-IN' (Hindi → Hinglish), 'en-US' (English), 'hinglish'"},
                    "engine": {"type": "string", "default": "hinglish-ggml", "description": "Reserved. Only hinglish-ggml (whisper.cpp + Hindi2Hinglish-Apex-GGML) is supported."}
                },
                "required": ["media_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "srt.read",
            "description": "Parse an SRT file and return all entries with timestamps and text. Use to inspect transcription quality before building edits, or to read an edited SRT for applying changes. Returns: count + entries array with idx, start, end, text.",
            "inputSchema": {
                "type": "object",
                "properties": {"srt_path": {"type": "string", "description": "Path to the SRT file"}},
                "required": ["srt_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "srt.prepare",
            "description": "Convert word-level SRT into phrase-level SRT by grouping words into readable caption segments (max ~10 words, ~64 chars). This is the step between raw transcription and editing — it creates human-readable caption chunks that the EDL uses for segment timing. Returns: output_path with grouped SRT, count of groups.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "srt_path": {"type": "string", "description": "Path to word-level SRT (from transcribe)"},
                    "max_words": {"type": "integer", "default": 10, "description": "Max words per caption segment"},
                    "max_chars": {"type": "integer", "default": 64, "description": "Max characters per caption segment"},
                    "max_gap": {"type": "number", "default": 0.6, "description": "Max gap in seconds between words to keep them in same group"},
                    "max_duration_s": {"type": "number", "default": 5.0, "description": "Max duration in seconds per caption group. Prevents slow captions >5s at end of video."}
                },
                "required": ["srt_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "captions.generate_ass",
            "description": "Generate ASS subtitle file from word-level SRT with per-word timing. Optionally remaps timestamps to account for xfade crossfade overlaps between concatenated segments. Use this to create kinetic-style captions (word_highlight, standard, etc.) for any timeline. Returns: ass_path, segment_count.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "srt_path": {"type": "string", "description": "Path to word-level SRT file (from transcribe)"},
                    "word_srt_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Optional word-level SRT from transcribe (word_srt_path). When provided, per-word timings are REAL transcription alignments instead of estimates from the phrase SRT — keeps the word-highlight synced with the voice (caption-sync fix)."},
                    "style": {"type": "string", "default": "word_highlight", "description": "Caption style: 'word_highlight' (TikTok per-word pop-up), 'standard' (full-sentence), 'kinetic' (animated word-by-word)"},
                    "font": {"type": "string", "default": "Bebas Neue", "description": "Font name for captions"},
                    "font_size": {"type": "integer", "default": 84, "description": "Font size in pixels"},
                    "color": {"type": "string", "default": "#ffffff", "description": "Primary caption color (hex)"},
                    "highlight_color": {"type": "string", "default": "#00ff88", "description": "Word highlight color for word_highlight style (hex)"},
                    "position": {"type": "string", "default": "center", "description": "Caption position: 'center' (default), 'bottom' (shorts safe zone), or 'top'"},
                    "safe_zone": {"type": "number", "default": 0.85, "description": "Vertical safe zone (0.0-1.0) for caption placement"},
                    "max_words_per_line": {"type": "integer", "default": 5, "description": "Max words per displayed line"},
                    "width": {"type": "integer", "default": 1080, "description": "Video width in pixels"},
                    "height": {"type": "integer", "default": 1920, "description": "Video height in pixels"},
                    "crossfade_ms": {"anyOf": [{"type": "integer"}, {"type": "null"}], "description": "Crossfade duration in ms. When set, remaps SRT timestamps from source-time to output-time to account for xfade overlaps between segments."},
                    "grouped_srt_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Optional grouped SRT path for fallback when word-level SRT parsing fails. When set, uses this file with estimated word timings as a fallback."},
                    "output_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Output ASS file path (auto-generated if omitted)"},
                    "timeline_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "When provided, registers the generated ASS file in timeline.assets.captions so timeline.render can find it automatically. No manual registration needed."}
                },
                "required": ["srt_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "srt.apply_edit",
            "description": "Apply user-edited SRT as a keep-list: build an EDL from the edited segments and render the result. Use this when a human has manually removed/edited SRT lines — it treats the edited SRT as the definitive edit, keeping only those segments and burning in updated captions. Returns: output_path, segments_count, total_duration_s.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "video_path": {"type": "string", "description": "Source video file"},
                    "edited_srt_path": {"type": "string", "description": "Path to the user-edited SRT file (acts as keep-list)"},
                    "merge_gap": {"type": "number", "default": 0.25, "description": "Gap in seconds to merge between segments"},
                    "crossfade_ms": {"type": "integer", "default": 80, "description": "Audio crossfade between segments in milliseconds"},
                    "aspect": {"type": "string", "default": "9:16", "description": "Output aspect ratio"},
                    "burn_captions": {"type": "boolean", "default": true, "description": "Burn captions into video output"},
                    "crf": {"type": "integer", "default": 20, "description": "Video quality (lower = higher quality, 18-28 range)"},
                    "fps": {"type": "integer", "default": 30, "description": "Output framerate"}
                },
                "required": ["video_path", "edited_srt_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "edl.build",
            "description": "Build an Edit Decision List from SRT data. Analyzes caption timing, groups words, and produces a JSON EDL with segment start/end times. Use when you need precise control over which parts of the source video make the cut. Strategy 'keep' retains speaking segments; 'remove' creates gaps. Returns: edl_path, segments_count, total_duration_s.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "srt_path": {"type": "string", "description": "Path to SRT file"},
                    "strategy": {"type": "string", "enum": ["keep", "remove"], "default": "keep", "description": "'keep' retains speaking parts, 'remove' creates silence gaps"},
                    "max_duration": {"anyOf": [{"type": "number"}, {"type": "null"}], "description": "Cap total output duration in seconds"},
                    "crossfade_ms": {"type": "integer", "default": 120, "description": "Audio crossfade between segments in ms"},
                    "analysis_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Optional path to save SRT analysis JSON"},
                    "aspect": {"type": "string", "default": "9:16", "description": "Target aspect ratio"}
                },
                "required": ["srt_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "render",
            "description": "Render a video from an EDL with burned-in captions. Use for quick single-step renders when you already have an EDL and SRT. For full multi-track renders (b-roll, music, SFX), use timeline.render instead. Returns: output_path.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "video_path": {"type": "string", "description": "Source video file"},
                    "edl_path": {"type": "string", "description": "Path to EDL JSON file"},
                    "burn_captions": {"type": "boolean", "default": true, "description": "Burn captions into output"},
                    "srt_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "SRT file for caption burn-in"},
                    "ass_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "ASS file for styled caption burn-in (overrides SRT)"},
                    "aspect": {"type": "string", "default": "9:16", "description": "Output aspect ratio"},
                    "crf": {"type": "integer", "default": 20, "description": "Video quality (18-28 range)"},
                    "fps": {"type": "integer", "default": 30, "description": "Output framerate"}
                },
                "required": ["video_path", "edl_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "reelize",
            "description": "One-call reel creation: transcribe → group captions → build EDL → render with burned-in captions. Use for quick turnaround when you don't need b-roll, music, or SFX. For full production reels with all tracks, use timeline.render with atomic tools instead. Returns: output_path, segments_count, total_duration_s, preset used.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "video_path": {"type": "string", "description": "Raw source video"},
                    "srt_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Pre-existing SRT (skip transcription)"},
                    "preset": {"type": "string", "default": "Balanced", "description": "Editing pace: Tight (fast cuts), Balanced (moderate), Natural (relaxed)"},
                    "max_duration": {"anyOf": [{"type": "number"}, {"type": "null"}], "description": "Maximum output duration in seconds"},
                    "aspect": {"type": "string", "default": "9:16", "description": "Output aspect ratio"},
                    "burn_captions": {"type": "boolean", "default": true, "description": "Burn captions into output"}
                },
                "required": ["video_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "reelize.brief",
            "description": "Analyze video footage and return a structured brief for AI-directed editorial decisions. Transcribes, segments, and provides context about ALL footage — transcripts, timing, word counts, suggested b-roll concepts per segment, topic clusters. Use as the FIRST step before directing a video edit. This tool provides CONTEXT only — it makes NO editorial decisions. The AI agent reads the brief and decides which segments to keep, what b-roll to add, etc. Returns: source_duration_s, total_segments, total_dialogue_s, segments (with id, start_s, end_s, duration_s, text, word_count, words_per_second, suggested_broll_concepts, topic_keywords), topic_summary.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "video_path": {"type": "string", "description": "Source video file to analyze"},
                    "srt_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Pre-existing SRT file (skip transcription)"}
                },
                "required": ["video_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "reelize.direct",
            "description": "Execute AI-directed video production. The AI agent provides structured creative instructions (which segments to use, b-roll placement, SFX timing, music, voiceover script, caption style) and this tool builds the timeline, fetches assets, and renders the final reel. This tool EXECUTES — it makes NO creative decisions. Use AFTER reelize.brief, once the AI agent has analyzed the footage and made editorial choices. Returns: output_path, duration, segment counts, asset counts, warnings.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "video_path": {"type": "string", "description": "Source video file"},
                    "brief_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Path to reelize.brief output JSON (optional, for reference)"},
                    "srt_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Pre-existing SRT file (skip transcription)"},
                    "segments": {"type": "array", "items": {"type": "object", "properties": {"id": {"type": "string"}, "start": {"type": "number"}, "end": {"type": "number"}, "caption": {"type": "string"}, "crossfade_ms": {"type": "integer"}}}, "description": "Segments to include in the edit, in order. Each has start/end times from source and caption text."},
                    "aspect": {"type": "string", "default": "9:16", "description": "Output aspect ratio"},
                    "crossfade_ms": {"type": "integer", "default": 300, "description": "Audio crossfade between segments in ms"},
                    "fps": {"type": "integer", "default": 30, "description": "Output framerate"},
                    "broll": {"type": "array", "items": {"type": "object", "properties": {"concept": {"type": "string"}, "overlay_at_s": {"type": "number"}, "duration_s": {"type": "number"}, "style": {"type": "string"}}}, "description": "B-roll overlays: concept keyword, where to place it, how long, and style (full_cutaway or picture_in_picture)"},
                    "sfx": {"type": "array", "items": {"type": "object", "properties": {"role": {"type": "string"}, "at_s": {"type": "number"}}}, "description": "Sound effects: editorial role (whoosh, pop, hit, riser) and placement time in seconds"},
                    "music": {"anyOf": [{"type": "object", "properties": {"mood": {"type": "string"}, "energy": {"type": "string"}, "gain_db": {"type": "number"}, "duck_under_dialogue": {"type": "boolean"}}}, {"type": "null"}], "description": "Background music: mood, energy, volume level, and whether to duck under dialogue"},
                    "voiceover": {"type": "array", "items": {"type": "object", "properties": {"text": {"type": "string"}, "position_s": {"type": "number"}, "voice_profile_id": {"type": "string"}, "speed": {"type": "number"}, "gain_db": {"type": "number"}}}, "description": "TTS voiceover events: script text, placement, and voice profile"},
                    "captions": {"type": "object", "properties": {"enabled": {"type": "boolean", "default": true}, "style": {"type": "string", "enum": ["standard", "kinetic"], "default": "standard"}, "position": {"type": "string", "enum": ["center", "bottom"], "default": "center"}}, "description": "Caption style: standard (full-sentence ASS) or kinetic (word-by-word viral style); position defaults to center screen"},
                    "output_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Output video path (auto-generated if omitted)"},
                    "crf": {"type": "integer", "default": 20, "description": "Video quality (18-28 range)"}
                },
                "required": ["video_path", "segments"],
                "additionalProperties": false
            }
        },
        {
            "name": "overlay.generate",
            "description": "Generate an animated caption overlay MOV using PupCaps. Produces a transparent-background video with styled captions that can be composited over the main render. Use when you want animated, styled captions instead of simple burned-in text. Returns: output_path of the overlay MOV.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "srt_path": {"type": "string", "description": "Word-level SRT file"},
                    "edl_path": {"type": "string", "description": "EDL JSON for timeline retiming"},
                    "timeline_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Timeline path to persist overlay asset"},
                    "out_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Output overlay MOV path"},
                    "width": {"type": "integer", "default": 1080, "description": "Overlay width in pixels"},
                    "height": {"type": "integer", "default": 1920, "description": "Overlay height in pixels"},
                    "fps": {"type": "integer", "default": 30, "description": "Overlay framerate"},
                    "animate": {"type": "boolean", "default": false, "description": "Enable word-by-word animation"},
                    "style": {"type": "string", "default": "pupcaps_center", "description": "Caption style preset"}
                },
                "required": ["srt_path", "edl_path"],
                "additionalProperties": false
            }
        },
        // ===================================================================
        // GROUP 2: TIMELINE V2 — Multi-track editorial timeline
        // ===================================================================
        {
            "name": "timeline.build",
            "description": "Create a fresh multi-track EDL v2 timeline from a source video. This is the foundation for all editorial work — it creates a JSON timeline with empty tracks (dialogue, broll, music, sfx, voiceover, captions) that you populate with other tools. Use this as the FIRST step before any multi-track editing. Returns: timeline_path, aspect, fps.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "source_video": {"type": "string", "description": "Path to source video file"},
                    "aspect": {"type": "string", "default": "9:16", "description": "Target aspect ratio"},
                    "fps": {"type": "integer", "default": 30, "description": "Target framerate"},
                    "max_duration": {"anyOf": [{"type": "integer"}, {"type": "null"}], "description": "Maximum timeline duration in seconds"},
                    "output_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Custom timeline JSON path (auto-generated if omitted)"},
                    "platform": {"type": "string", "description": "Target platform preset: 'tiktok'/'reels'/'shorts' (9:16, 30fps), 'youtube' (16:9, 30fps), 'instagram'/'square' (1:1, 30fps). Overrides aspect and fps."}
                },
                "required": ["source_video"],
                "additionalProperties": false
            }
        },
        {
            "name": "timeline.load",
            "description": "Load an existing timeline JSON and return its structure. Use to inspect a timeline before modifying it — reveals segments, track counts, and version. Always load before making edits. Returns: version, source, segments_count, track names.",
            "inputSchema": {
                "type": "object",
                "properties": {"timeline_path": {"type": "string", "description": "Path to timeline JSON file"}},
                "required": ["timeline_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "timeline.validate",
            "description": "Check a timeline for structural errors — missing segments, invalid track events, timing conflicts. ALWAYS call this before timeline.render to catch issues early. Returns: valid (boolean), errors array.",
            "inputSchema": {
                "type": "object",
                "properties": {"timeline_path": {"type": "string", "description": "Path to timeline JSON file"}},
                "required": ["timeline_path"],
                "additionalProperties": false
            }
        },
                {
            "name": "srt.to_timeline",
            "description": "Convert an SRT file into a timeline with segments in one call. Reads all SRT entries, creates a timeline, and adds each entry as a segment with start/end times and caption text. This is the ONE-CALL replacement for calling timeline.add_segment N times. Use max_duration_s for sentence-aware segmentation (pause detection + duration caps, ideal for short-form b-roll pacing). Returns: timeline_path, segments_count, duration_s.",
            "inputSchema": {
                "type": "object",
                "properties": {
            "srt_path": {"type": "string", "description": "Path to SRT file (from transcribe or srt.prepare)"},
            "source_video": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Source video/audio path. Sets the timeline's source for rendering and validation. If omitted, uses the SRT filename stem."},                    "timeline_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Optional existing timeline to add segments to. If omitted, creates a new timeline."},
                    "output_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Explicit output path for the timeline JSON. Overrides timeline_path for the save location. Auto-generated from srt_path if omitted."},
            "crossfade_ms": {"type": "integer", "default": 80, "description": "Audio crossfade between segments in ms"},
            "aspect": {"type": "string", "default": "9:16", "description": "Target aspect ratio for new timelines"},
            "fps": {"type": "integer", "default": 30, "description": "Target framerate for new timelines"},                    "scene_size": {"type": "integer", "default": 1, "description": "Group N consecutive SRT entries into one segment (e.g., scene_size=4 groups 4 entries per segment). Set to 1 for one-segment-per-entry. Controls segmentation granularity for b-roll placement. Overridden by max_duration_s when set."},
                    "max_duration_s": {"anyOf": [{"type": "number"}, {"type": "null"}], "default": null, "description": "Maximum duration per segment in seconds. When set, groups SRT entries by pause detection (>300ms gaps) and splits segments exceeding this duration. Produces 2-6s segments ideal for short-form b-roll pacing. Overrides scene_size."},
                    "min_duration_s": {"anyOf": [{"type": "number"}, {"type": "null"}], "default": null, "description": "Minimum duration per segment in seconds. Segments shorter than this are merged with adjacent segments. Only effective when max_duration_s is set."}
                },
                "required": ["srt_path"],
                "additionalProperties": false
            }
        },
{
            "name": "timeline.upgrade",
            "description": "Convert a legacy EDL v1 JSON into the modern EDL v2 timeline format. Use when working with old renders that need multi-track capabilities. Returns: timeline_path, segments_count.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "edl_v1_path": {"type": "string", "description": "Path to legacy EDL v1 JSON"},
                    "output_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Output timeline path (auto-generated if omitted)"}
                },
                "required": ["edl_v1_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "timeline.add_segment",
            "description": "Add a video segment to the timeline's main track. Each segment represents a continuous clip from the source video with start/end timestamps and a caption. Use to manually curate which parts of the source make it into the final edit. Returns: segment_id, timeline_path.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_path": {"type": "string", "description": "Path to timeline JSON"},
                    "start": {"type": "number", "description": "Start time in seconds from source video"},
                    "end": {"type": "number", "description": "End time in seconds from source video"},
                    "caption": {"type": "string", "description": "Caption text for this segment (used for burn-in)"},
                    "crossfade_ms": {"type": "integer", "default": 80, "description": "Audio crossfade at segment boundaries in ms"},
                    "semantic_role": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Optional role tag (e.g., 'hook', 'body', 'cta')"}
                },
                "required": ["timeline_path", "start", "end", "caption"],
                "additionalProperties": false
            }
        },
        {
            "name": "timeline.add_track_event",
            "description": "Add an event to any timeline track (dialogue, voiceover, captions, broll, music, sfx). This is the low-level tool for populating tracks — use the specialized tools (broll.assign, music.assign, sfx.assign, voiceover.generate) when possible, but use this for custom event structures. Returns: event_id, track_type, timeline_path.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_path": {"type": "string", "description": "Path to timeline JSON"},
                    "track_type": {"type": "string", "enum": ["dialogue", "voiceover", "captions", "broll", "music", "sfx", "stickers"], "description": "Which track to add the event to"},
                    "event": {"type": "object", "description": "Event object with id, asset_id, start_ms, end_ms, gain_db, kind fields"}
                },
                "required": ["timeline_path", "track_type", "event"],
                "additionalProperties": false
            }
        },
        {
            "name": "voice.profile.add",
            "description": "Register a voice profile for TTS generation. Captures a reference voice from audio + text for cloning or synthesis. Create profiles BEFORE generating voiceovers or commentary. Use unique profile_id per voice. Returns: profile_id.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "profile_id": {"type": "string", "description": "Unique identifier for this voice profile"},
                    "ref_audio": {"type": "string", "description": "Path to reference audio file (clean speech sample)"},
                    "ref_text": {"type": "string", "description": "Transcript of the reference audio"},
                    "provider": {"type": "string", "default": "faster-qwen3-tts", "description": "TTS provider engine: 'gepard' (high-quality native-English zero-shot cloning — Gepard 1.0 Qwen3.5 AR + NeMo NanoCodec, 22.05kHz, requires .venv-gepard via scripts/setup_gepard.sh), 'audio8' (default for cloned voices — Audio8 TTS zero-shot cloning, registers ref_audio + ref_text), 'voicedesign' (Qwen3 VoiceDesign — DIRECT NL-instruction synthesis, no cloning; profiles from voice.design / character.create carry the persona in description), 'kokoro' (preset voices), 'faster-qwen3-tts' (voicebox HTTP sidecar)"},
                    "mode": {"type": "string", "default": "clone", "description": "Voice mode: 'clone' for voice cloning, 'preset' for built-in voices"},
                    "model": {"type": "string", "default": "Qwen/Qwen3-TTS-12Hz-0.6B-Base", "description": "TTS model identifier"},
                    "language": {"type": "string", "default": "English", "description": "Voice language"},
                    "description": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Human-readable description of this voice"},
                    "emotions": {"anyOf": [{"type": "object", "additionalProperties": {"type": "object", "properties": {"ref_audio": {"type": "string", "description": "Reference WAV of this speaker delivering the emotion"}, "ref_text": {"type": "string", "description": "Exact transcript of the emotion reference audio"}, "cfg_scale": {"anyOf": [{"type": "number"}, {"type": "null"}], "description": "Gepard reference-fidelity knob for this take (higher = clings closer to this emotion reference)"}}, "required": ["ref_audio", "ref_text"]}}, {"type": "null"}], "description": "Emotion-template map: {emotion_id: {ref_audio, ref_text, cfg_scale?}}. Each entry is a SEPARATE reference recording of the same speaker delivering that emotion. Scene 'emote' / tts.generate 'emotion' then selects the matching take so every line is attuned to the required tonality. gepard takes are used via per-request ref override; audio8 takes are auto-registered as {profile_id}@{emotion} compound voices."}
                },
                "required": ["profile_id", "ref_audio", "ref_text"],
                "additionalProperties": false
            }
        },
        {
            "name": "voice.profile.list",
            "description": "List all registered voice profiles. Use to see available voices before generating voiceovers. Returns: profiles array with profile_id, provider, language, and count.",
            "inputSchema": {"type": "object", "properties": {}}
        },
        {
            "name": "voice.profile.remove",
            "description": "Delete a voice profile by ID. Use to clean up unused or test profiles. Returns: profile_id removed.",
            "inputSchema": {
                "type": "object",
                "properties": {"profile_id": {"type": "string", "description": "ID of the voice profile to remove"}},
                "required": ["profile_id"],
                "additionalProperties": false
            }
        },
        {
            "name": "voice.design",
            "description": "Design a NOVEL character voice from a natural-language description (Qwen3-TTS-1.7B-VoiceDesign, ONNX int4, 24kHz) — no reference audio needed. Describe a persona (e.g. 'grumpy detective, low gravelly voice') and give a sample line; get back a WAV of a brand-new voice matching the description. Optionally auto-register the designed voice as a reusable voicedesign profile via profile_id — scene lines then synthesize DIRECTLY with the Qwen3 VoiceDesign model (per-line emotion/tone instruct, no cloning). Use for comic/custom-character content where each character needs a distinct voice. Returns: output_path, duration_ms, sample_rate, and profile_id when registered.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "instruct": {"type": "string", "description": "Natural-language voice description, e.g. 'Speak in a warm and friendly female voice' or 'grumpy old detective, low gravelly voice, slight rasp'."},
                    "text": {"type": "string", "description": "Sample line the designed voice should speak, e.g. 'Give every small business the voice of a big one.'"},
                    "output_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Output WAV path (default: artifacts/voices/designed_<timestamp>.wav)"},
                    "profile_id": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Optional. When set, auto-register the designed voice as a clone profile with this id (provider=gepard) so it can be reused via tts.generate / script speakers (voice 'default' + tts.voice). Requires the gepard engine."},
                    "language": {"type": "string", "default": "english", "description": "Language: english, chinese, japanese, korean, german, french, russian, portuguese, spanish, italian"},
                    "seed": {"anyOf": [{"type": "integer"}, {"type": "null"}], "description": "Optional sampling seed for reproducible designs."},
                    "max_tokens": {"type": "integer", "default": 2048, "description": "Max codec frames to generate."},
                    "temperature": {"type": "number", "default": 0.9, "description": "Sampling temperature."},
                    "top_k": {"type": "integer", "default": 50, "description": "Top-k sampling."}
                },
                "required": ["instruct", "text"],
                "additionalProperties": false
            }
        },
        {
            "name": "character.create",
            "description": "PART 1 of the character-first voice-design workflow: define a character (schema + properties: name, role, personality, language) and design its BASE voice. When 'voice' is given, uses that existing profile; otherwise designs the base voice from personality + sample_text via VoiceDesign (Qwen3-TTS-1.7B-VoiceDesign ONNX int4) and registers it as a voicedesign profile — scene lines synthesize DIRECTLY on the Qwen3 model with the character's personality + per-scene emotion instruct (no cloning). Characters are persisted in .openscript/characters.json. THEN design emotional takes with character.design_emotion and write the transcript referencing the character. Returns: character schema + voice_profile_id.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "character_id": {"type": "string", "description": "Unique character id (also the base voice profile id)"},
                    "name": {"type": "string", "default": "character_id", "description": "Character display name"},
                    "role": {"type": "string", "default": "character", "description": "Character role (protagonist, narrator, villain, sidekick, ...)"},
                    "personality": {"type": "string", "description": "Natural-language voice/persona description, e.g. 'grumpy old detective, low gravelly voice, slight rasp, slow deliberate pace'"},
                    "sample_text": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "A line the base voice should speak (required unless 'voice' is given)"},
                    "language": {"type": "string", "default": "english", "description": "Language: english, chinese, japanese, korean, german, french, russian, portuguese, spanish, italian"},
                    "voice": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Optional existing voice profile id to use as the base (skips VoiceDesign)"},
                    "seed": {"anyOf": [{"type": "integer"}, {"type": "null"}], "description": "Optional sampling seed for reproducible base-voice design."}
                },
                "required": ["character_id", "personality"],
                "additionalProperties": false
            }
        },
        {
            "name": "character.design_emotion",
            "description": "Design ONE emotional delivery take for a character (the character's voice-design emotional range). Runs VoiceDesign with the character's personality + the emotion, writes the take WAV as a design artifact, and attaches its instruct to BOTH the character schema AND the character's base voice profile emotions map. After this, any scene with emote='<emotion>' on this character synthesizes with that emotional delivery directly on the Qwen3 VoiceDesign model (per-line tonality, no cloning). Returns: ref_audio + how to trigger the take.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "character_id": {"type": "string", "description": "Character id (from character.create)"},
                    "emotion": {"type": "string", "description": "Emotion-take id, e.g. 'angry', 'whisper', 'excited', 'sad', 'whisper'"},
                    "sample_text": {"type": "string", "description": "A line the character speaks in this emotional delivery"},
                    "instruct": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Optional natural-language description of this emotional delivery (default: personality + '<emotion> delivery')"},
                    "language": {"type": "string", "default": "english", "description": "Language."},
                    "seed": {"anyOf": [{"type": "integer"}, {"type": "null"}], "description": "Optional sampling seed for reproducibility."},
                    "max_tokens": {"type": "integer", "default": 2048, "description": "Max codec frames."},
                    "temperature": {"type": "number", "default": 0.9, "description": "Sampling temperature."},
                    "top_k": {"type": "integer", "default": 50, "description": "Top-k sampling."}
                },
                "required": ["character_id", "emotion", "sample_text"],
                "additionalProperties": false
            }
        },
        {
            "name": "character.list",
            "description": "List all defined characters with their designed emotional takes (from .openscript/characters.json). Use to see what characters exist and which emotions each has before writing a script. Returns: characters array with character_id, name, role, voice, language, emotions.",
            "inputSchema": {"type": "object", "properties": {}}
        },
        {
            "name": "character.remove",
            "description": "Delete a character: its schema entry AND its base voice profile (including emotion takes). WAV artifacts are left on disk (regenerable). Returns: character_id removed.",
            "inputSchema": {
                "type": "object",
                "properties": {"character_id": {"type": "string", "description": "Character id to remove"}},
                "required": ["character_id"],
                "additionalProperties": false
            }
        },
        {
            "name": "tts.generate",
            "description": "Generate speech audio from text using a registered voice profile. Use for producing narration, explanations, or any scripted audio. Routes by provider: 'gepard' (high-quality native-English voice clone, 22.05kHz, Apache-2.0 — best fidelity for English narration; FIRST gepard synth downloads the model ~2.5GB and can take minutes — a cold start, not a hang), 'audio8' (zero-shot voice clone, ONNX INT4 — default for cloned voices), 'voicedesign' (Qwen3-TTS-1.7B-VoiceDesign ONNX int4 — DIRECT NL-instruction synthesis with per-line emotion/tone, no cloning; profiles from voice.design / character.create), 'kokoro' (presets), 'faster-qwen3-tts' (requires OPENSCRIPT_TTS_URL sidecar). Pass an 'emotion' to select the profile's emotion-take (tonality template) when one is registered — e.g. a clone profile with an 'angry' take speaks that line angry instead of neutral. Returns: output_path, duration_ms, cached flag, backend.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "voice_profile_id": {"type": "string", "description": "ID of the voice profile to use"},
                    "text": {"type": "string", "description": "Text to synthesize"},
                    "output_path": {"type": "string", "description": "Output audio file path (WAV/MP3)"},
                    "emotion": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Emotion-take id registered on the profile (e.g. 'angry', 'whisper', 'excited'). When the profile has an emotions template (voice.profile.add with emotions), synthesizes with that emotional delivery's reference instead of the neutral base voice. Falls back to the base voice when no take matches."},
                    "tone": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Natural-language delivery direction, e.g. 'low gravelly whisper, deliberate'. Recorded as a diagnostic and reserved for engines that gain an instruction channel; the emotion-take mechanism carries tonality today."},
                    "speed": {"type": "number", "default": 1.0, "description": "Playback speed multiplier (1.0 = normal; applied post-synthesis for gepard/audio8 clone engines)"},
                    "pitch": {"type": "number", "default": 1.0, "description": "Pitch multiplier (applied post-synthesis for gepard/audio8 clone engines)"},
                    "volume": {"type": "number", "default": 1.0, "description": "Volume multiplier"},
                    "format": {"type": "string", "default": "wav", "description": "Output audio format"},
                    "temperature": {"anyOf": [{"type": "number"}, {"type": "null"}], "description": "Sampling temperature (clone engines). Higher = more prosodic variation/inflection; lower = flatter. Default 0.7 (expressive but stable). 0.3+ is the robotic/flat zone."},
                    "top_k": {"anyOf": [{"type": "integer"}, {"type": "null"}], "description": "Top-k sampling for clone engines (None = engine default)."},
                    "top_p": {"anyOf": [{"type": "number"}, {"type": "null"}], "description": "Top-p nucleus sampling (audio8; None = engine default 0.9)."},
                    "cfg_scale": {"anyOf": [{"type": "number"}, {"type": "null"}], "description": "Gepard reference-fidelity knob (higher = clings closer to the reference recording; 1.0 default). Explicit value wins over the emotion take's cfg_scale."}
                },
                "required": ["voice_profile_id", "text", "output_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "tts.estimate_duration",
            "description": "Estimate how long TTS audio will be for a given text. Use BEFORE placing voiceovers on the timeline so you know the duration to reserve. Rule of thumb: ~2.5 words per second at normal speed. Returns: word_count, estimated_duration_ms.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": {"type": "string", "description": "Text to estimate duration for"},
                    "speed": {"type": "number", "default": 1.0, "description": "Speed multiplier"}
                },
                "required": ["text"],
                "additionalProperties": false
            }
        },
        {
            "name": "sfx.index",
            "description": "Scan the SFX library directory and build a searchable index JSON. Run once when SFX library changes. The index enables sfx.search and sfx.assign. Default path: $HOME/Videos/Assets/SFX (override with OPENSCRIPT_SFX_PATH env var). Returns: output_path, count of indexed files.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sfx_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "SFX directory to scan (default: $OPENSCRIPT_SFX_PATH)"},
                    "output_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Output index JSON path (default: mcp/assets/sfx_index.json)"}
                }
            }
        },
        {
            "name": "sfx.search",
            "description": "Search the SFX index by keyword, editorial role, or category. Editorial roles describe the PURPOSE of a sound: 'intro' (hook/opening), 'transition' (scene change), 'highlight' (emphasis), 'outro' (closing). Use to find the right sound effect before assigning it. Returns: results array with id, filename, path, category, editorial_role, duration_ms, recommended_gain_db.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "default": "", "description": "Keyword search (e.g., 'whoosh', 'click', 'boom')"},
                    "editorial_role": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Editorial purpose: 'intro', 'transition', 'highlight', 'outro', 'hook'"},
                    "category": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Sound category (e.g., 'whoosh', 'impact', 'ambient')"},
                    "limit": {"type": "integer", "default": 10, "description": "Max results to return"}
                }
            }
        },
        {
            "name": "sfx.assign",
            "description": "Assign a sound effect to a specific position on the timeline's SFX track. Use editorial_role to select the RIGHT sound for the RIGHT moment: 'hook' at 0ms (grab attention), 'transition' between segments (smooth cuts), 'highlight' at key moments (emphasize points). Searches SFX index automatically by role. Returns: event_id, position_ms, asset_path.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_path": {"type": "string", "description": "Path to timeline JSON"},
                    "editorial_role": {"type": "string", "description": "Purpose of this SFX: 'hook' (opening grab), 'transition' (between segments), 'highlight' (emphasis), 'outro' (closing)"},
                    "query": {"type": "string", "default": "", "description": "Additional keyword filter (e.g., 'whoosh', 'subtle')"},
                    "position_ms": {"type": "integer", "default": 0, "description": "Timeline position in milliseconds"},
                    "gain_db": {"type": "number", "default": -10.0, "description": "Volume adjustment in decibels"}
                },
                "required": ["timeline_path", "editorial_role"],
                "additionalProperties": false
            }
        },
        {
            "name": "sfx.auto_assign",
            "description": "Auto-place SFX at ALL segment boundaries in ONE CALL: hook SFX at the start, transition SFX between each segment, and outro SFX at the end. Reads the timeline, finds segment boundaries, and places appropriate SFX automatically. ONE-CALL replacement for calling sfx.assign N+2 times. Returns: events_created count, positions, timeline_path.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_path": {"type": "string", "description": "Path to timeline JSON with populated segments"},
                    "gain_db": {"type": "number", "default": -10.0, "description": "Volume for all placed SFX in dB"},
                    "skip_hook": {"type": "boolean", "default": false, "description": "Skip the opening hook SFX"},
                    "skip_outro": {"type": "boolean", "default": false, "description": "Skip the closing outro SFX"}
                },
                "required": ["timeline_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "music.index",
            "description": "Scan music directories and build a searchable index JSON. Run once when adding new music files. Default path: $HOME/Videos/Assets/Music (override with OPENSCRIPT_MUSIC_PATH env var). Returns: output_path, count of indexed files.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "music_paths": {"anyOf": [{"type": "array", "items": {"type": "string"}}, {"type": "null"}], "description": "Directories to scan for music files"},
                    "output_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Output index JSON path"}
                }
            }
        },
        {
            "name": "music.search",
            "description": "Search local music index by mood, energy, genre, or keyword tags. Returns matching tracks with local file paths ready for music.assign. Use this to find background music from the indexed library ($HOME/Videos/Assets/Music). Returns: tracks array with id, title, path, mood, energy, duration_ms.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Free-text search over title, tags, genre"},
                    "mood": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Filter by mood (neutral, upbeat, dark, epic, etc.)"},
                    "energy": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Filter by energy level (low, medium, high)"},
                    "limit": {"type": "integer", "default": 10, "description": "Max results to return"},
                    "index_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Override music index path (default: mcp/assets/music_index.json)"}
                },
                "additionalProperties": false
            }
        },
        {
            "name": "music.assign",
            "description": "Assign background music to the timeline's music track. Requires a music file path — use library.search first to find tracks, then pass the path here. Automatically spans the full timeline duration, applies ducking (lowers music during dialogue/voiceover), and sets gain. Use after building segments — the music provides emotional context beneath the spoken content. Default: -12dB with auto-ducking enabled. Accepts both local file paths and URLs (auto-downloads if URL). Returns: event_id, start_ms, end_ms, asset_path.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_path": {"type": "string", "description": "Path to timeline JSON"},
                    "path": {"type": "string", "description": "Path to the music audio file (MP3/WAV) or URL. Use library.search to find tracks and get their path."},
                    "mood": {"type": "string", "default": "neutral", "description": "Emotional mood matching content tone"},
                    "energy": {"type": "string", "default": "medium", "description": "Intensity level"},
                    "start_ms": {"type": "integer", "default": 0, "description": "Music start position on timeline"},
                    "end_ms": {"anyOf": [{"type": "integer"}, {"type": "null"}], "description": "Music end position (auto = end of timeline)"},
                    "gain_db": {"type": "number", "default": -12.0, "description": "Background music volume in dB (lower = quieter behind voice)"},
                    "ducking": {"type": "boolean", "default": true, "description": "Automatically lower music during dialogue/voiceover sections"}
                },
                "required": ["timeline_path", "path"],
                "additionalProperties": false
            }
        },
        {
            "name": "broll.suggest",
            "description": "Analyze an EDL and suggest b-roll insertion points based on segment duration and cadence. Identifies gaps in the dialogue where visual overlays would enhance engagement. Returns: suggestions array with position_ms, duration_ms, concept for each slot.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "edl_path": {"type": "string", "description": "Path to EDL JSON"},
                    "srt_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Optional SRT for concept extraction"},
                    "cadence_seconds": {"type": "number", "default": 2.0, "description": "How often to suggest b-roll slots"}
                },
                "required": ["edl_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "broll.fetch",
            "description": "Search Pexels for b-roll videos matching given concepts or keywords. Set download=true to download videos. download_n controls how many DISTINCT clips are downloaded per concept (default 1) — when you have more segments than concepts, use download_n >= ceil(segments/concepts) so the auto-placer cycles distinct footage and segments never reuse the same clip (the \"same clip, different zoom/pan\" anti-pattern). When timeline_path and segments are provided, automatically places each downloaded clip on the timeline at the correct position/duration — no manual broll.assign needed. Returns: results with concept, videos, cached_path, cached_paths, source_duration_s. When auto-placed, returns timeline_path and assigned count.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "concepts": {"type": "array", "items": {"type": "string"}, "description": "Visual concepts to search for (e.g., ['city skyline', 'technology', 'nature'])"},
                    "keywords": {"anyOf": [{"type": "string"}, {"type": "array", "items": {"type": "string"}}], "description": "Alias for concepts. Accepts a single keyword string or an array of strings."},
                    "asset_dir": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Cache directory for downloaded videos"},
                    "orientation": {"type": "string", "default": "9:16", "description": "Video orientation: '9:16' (vertical), '16:9' (horizontal)"},
                    "quality": {"type": "string", "default": "sd", "description": "Video quality: 'sd', 'hd', '4k'"},
                    "download": {"type": "boolean", "default": true, "description": "Actually download the top result to cache"},
                    "fallback_pool": {"type": "array", "items": {"type": "string"}, "description": "Local video file paths used when Pexels returns 0 results for a concept (or when PEXELS_API_KEY is missing)."},
                    "timeline_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "When provided, automatically place each fetched clip on the timeline's broll track at the matching segment position. Requires segments or enriched_segments parameter."},
                    "segments": {"type": "array", "items": {"type": "object", "properties": {"id": {"type": "string"}, "start_s": {"type": "number"}, "end_s": {"type": "number"}, "start": {"type": "number"}, "end": {"type": "number"}, "caption": {"type": "string"}}}, "description": "Segment data from segment.analyze or broll.plan. Used with timeline_path for auto-placement. Each concept is matched to a segment by index (concept[0] → segment[0], etc.)."},
                    "enriched_segments": {"type": "array", "items": {"type": "object", "properties": {"id": {"type": "string"}, "start_s": {"type": "number"}, "end_s": {"type": "number"}, "start": {"type": "number"}, "end": {"type": "number"}, "caption": {"type": "string"}, "keywords": {"type": "array", "items": {"type": "string"}}}}, "description": "Enriched segments from broll.keywords output (each with a keywords array). When provided with timeline_path, broll.fetch searches Pexels using the best keywords per segment and auto-places clips. This is the PREFERRED input over concepts+segments."},
                    "max_keywords_per_search": {"type": "integer", "default": 3, "description": "Max keywords to join into a single Pexels search query per segment. More keywords = broader results."},
                    "download_n": {"type": "integer", "default": 1, "description": "Number of DISTINCT clips to download per concept. Set >1 when segments outnumber concepts so each segment gets its own footage (breaks clip reuse)."}
                },
                "required": [],
                "additionalProperties": false
            }
        },
        {
            "name": "broll.assign",
            "description": "Place a b-roll video clip onto the timeline's b-roll track at a specific position. The b-roll overlays the main video segment — use it to illustrate concepts, add visual variety, or cover jump cuts. Use after broll.fetch (with download=true) to get the asset_path. Returns: event_id, asset_id, asset_path, position_ms, duration_ms.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_path": {"type": "string", "description": "Path to timeline JSON"},
                    "concept": {"type": "string", "description": "Concept this b-roll illustrates (e.g., 'technology', 'business')"},
                    "position_ms": {"type": "integer", "description": "Timeline position in ms to start b-roll overlay"},
                    "duration_ms": {"type": "integer", "description": "How long the b-roll overlay lasts in ms"},
                    "asset_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Path to b-roll video file (from broll.fetch download)"},
                    "transition_style": {"type": "string", "default": "cut", "description": "Transition into b-roll: 'cut', 'fade', 'slide'"},
                    "crop_mode": {"type": "string", "default": "center", "description": "How to crop b-roll to target aspect: 'center', 'smart', 'fit'"}
                },
                "required": ["timeline_path", "concept", "position_ms", "duration_ms"],
                "additionalProperties": false
            }
        },
        {
            "name": "broll.plan",
            "description": "Analyze timeline segments and return structured JSON with timestamps, captions, and timing for each segment. The agent reads this data, generates English visual keywords using its LLM capabilities (translating Hinglish if needed), then calls broll.fetch with those agent-generated keywords. Returns: segments array with id, start_s, end_s, caption, duration_s. Use BEFORE broll.fetch to get segment data for keyword generation.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_path": {"type": "string", "description": "Path to timeline JSON with populated segments"}
                },
                "required": ["timeline_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "broll.keywords",
            "description": "Extract English visual search keywords from transcript segments using an LLM. Translates Hinglish/Hindi captions into stock-footage-friendly English keywords. Takes segments from broll.plan or segment.analyze output and returns each segment mapped to 2-3 search keywords optimized for Pexels/Pixabay. This is the AUTOMATED replacement for manual agent keyword generation. Returns: segments array with id, start_s, end_s, caption, keywords (array of strings). Use BEFORE broll.fetch to get high-quality search terms.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "segments": {"type": "array", "items": {"type": "object", "properties": {"id": {"type": "string"}, "start_s": {"type": "number"}, "end_s": {"type": "number"}, "caption": {"type": "string"}}}, "description": "Segments array from broll.plan or segment.analyze. Each segment's caption is translated to English keywords."},
                    "video_title": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Optional video title for context — helps the LLM understand the overall topic and generate more relevant keywords."},
                    "language": {"type": "string", "default": "hinglish", "description": "Source language of captions: 'hinglish', 'hindi', 'english', 'mixed'. Helps the LLM choose the right translation strategy."},
                    "max_batch_size": {"type": "integer", "default": 15, "description": "Max segments per LLM call. Prevents context overflow on small local models. Default 15 works for 4B-param models."},
                    "timeline_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Optional timeline path. When provided, the already-covered b-roll concepts are passed to the agent so drafts AVOID repeating them (non-redundant single-shot keyword pass)."}
                },
                "required": ["segments"],
                "additionalProperties": false
            }
        },
        {
            "name": "broll.validate_keywords",
            "description": "STAGE 2 of the agentic b-roll keyword pipeline (run AFTER broll.keywords). Searches Pexels with each segment's draft keywords, presents the REAL candidate videos (name/slug, duration, resolution) to the agent, and the agent validates each candidate against the spoken caption — returning final_keywords + the best video id per segment. This is the relevance-validation/alignment stage: drafts that Pexels can't serve (or that return irrelevant footage) are corrected here BEFORE any download. Non-looping: candidates shorter than the segment window are filtered out. Returns: segments with final_keywords, best_video, relevance, reason, candidates; skipped entries for segments with no searchable keywords.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "enriched_segments": {"type": "array", "items": {"type": "object", "properties": {"id": {"type": "string"}, "start_s": {"type": "number"}, "end_s": {"type": "number"}, "caption": {"type": "string"}, "keywords": {"type": "array", "items": {"type": "string"}}}}, "description": "Output of broll.keywords: segments each with a keywords array. Each segment's keywords are searched on Pexels and the candidates are validated by the agent."},
                    "max_candidates": {"type": "integer", "default": 6, "description": "Max candidate videos to present to the validation agent per segment."},
                    "max_keywords_per_search": {"type": "integer", "default": 2, "description": "Max draft keywords to search per segment."},
                    "orientation": {"type": "string", "default": "9:16", "description": "Video orientation: '9:16', '16:9', '1:1'."},
                    "quality": {"type": "string", "default": "sd", "description": "Video quality: 'sd', 'hd', '4k'."},
                    "asset_dir": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Pexels cache dir (default mcp/assets/broll_cache)."},
                    "language": {"type": "string", "default": "hinglish", "description": "Source language of captions: 'hinglish', 'hindi', 'english', 'mixed'."}
                },
                "required": ["enriched_segments"],
                "additionalProperties": false
            }
        },
        {
            "name": "broll.repair",
            "description": "Gap-triggered re-pipeline that heals BROLL_GAP coverage errors. Loads the timeline, probes which b-roll clips are shorter than their segment window, and for each gap re-runs the FULL agentic loop with the entire timeline as context (layer stack, all segments, already-covered concepts, already-used clips, gap timestamps): agent drafts fresh keywords → Pexels search → agent validates candidates → download → replace the event + asset. Non-looping (clip must cover the window + slack) and non-redundant (already-used Pexels ids excluded). Call after timeline.validate reports BROLL_GAP errors, then re-validate. Returns: repaired count, per-segment decisions, remaining_gaps.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_path": {"type": "string", "description": "Path to the timeline JSON with BROLL_GAP errors."},
                    "max_segments": {"type": "integer", "default": 10, "description": "Max gap segments to repair per pass. Re-run the tool to heal more."},
                    "orientation": {"type": "string", "default": "9:16", "description": "Video orientation: '9:16', '16:9', '1:1'."},
                    "quality": {"type": "string", "default": "sd", "description": "Video quality: 'sd', 'hd', '4k'."},
                    "asset_dir": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Pexels cache dir (default mcp/assets/broll_cache)."},
                    "language": {"type": "string", "default": "hinglish", "description": "Source language of captions: 'hinglish', 'hindi', 'english', 'mixed'."}
                },
                "required": ["timeline_path"],
                "additionalProperties": false
            }
        },
        // ===================================================================
        // GROUP 2b: SEGMENT ANALYSIS — Transcript-to-keyword pipeline
        // ===================================================================
        {
            "name": "segment.analyze",
            "description": "Analyze a transcript or audio file and return structured segments with captions and timing. Uses sentence-aware segmentation (pause >300ms detection) with min/max duration enforcement per docs/SEGMENTATION_ARCHITECTURE.md: segments shorter than min_duration_s (default 2.0s) are merged, segments longer than max_duration_s (default 6.0s) are split at the longest internal pause — ideal for short-form b-roll pacing. This is a PURE ANALYSIS tool — it does NOT fetch any broll or render any video. The agent reads the returned segments, generates English visual keywords from Hinglish content using its LLM capabilities, then calls broll.fetch with those agent-generated keywords. Returns: segments array with id, start_s, end_s, duration_s, caption.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "audio_path": {"type": "string", "description": "Path to audio/video file to analyze"},
                    "srt_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Pre-existing SRT (skip transcription)"},
                    "min_duration_s": {"anyOf": [{"type": "number"}, {"type": "null"}], "default": 2.0, "description": "Minimum segment duration in seconds. Shorter segments are merged with their successor."},
                    "max_duration_s": {"anyOf": [{"type": "number"}, {"type": "null"}], "default": 6.0, "description": "Maximum segment duration in seconds. Longer segments are split at the longest internal pause."}
                },
                "required": ["audio_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "broll.auto",
            "description": "ONE-CALL A2V b-roll orchestrator: runs the full agentic b-roll pipeline end-to-end and loops until zero gaps remain. Pipeline: segment.analyze (sentence-aware 2-6s) → broll.keywords (agentic draft) → broll.validate_keywords (agent validates real Pexels candidates vs the spoken caption) → srt.to_timeline → broll.fetch (download + auto-place) → timeline.validate → broll.repair loop (re-drafts keywords for any BROLL_GAP with full timeline context) until no gaps remain or max_repair_iterations is hit. Feed it an SRT + audio and get back a fully covered timeline ready for timeline.render. Returns: timeline_path, segments_count, auto_assigned, initial_gaps, repair_passes, repaired_total, remaining_gaps, valid.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "srt_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "SRT transcript (required unless timeline_path is given)."},
                    "word_srt_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Optional word-level SRT from transcribe (word_srt_path). When provided, captions use REAL per-word alignments (caption-voice sync) instead of estimates from the phrase SRT."},
                    "audio_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Source audio/video (required unless timeline_path is given)."},
                    "timeline_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Existing timeline to fill (skips analyze/build)."},
                    "output_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Where to save the produced timeline (default derived from srt_path)."},
                    "min_duration_s": {"type": "number", "default": 2.0, "description": "Minimum segment duration (SEGMENTATION_ARCHITECTURE)."},
                    "max_duration_s": {"type": "number", "default": 6.0, "description": "Maximum segment duration (short-form retention cap)."},
                    "language": {"type": "string", "default": "hinglish", "description": "Source language of captions."},
                    "quality": {"type": "string", "default": "sd", "description": "Pexels quality: sd/hd/4k."},
                    "orientation": {"type": "string", "default": "9:16", "description": "Video orientation."},
                    "max_batch_size": {"type": "integer", "default": 15, "description": "Segments per keyword-draft LLM call."},
                    "max_candidates": {"type": "integer", "default": 6, "description": "Candidates per segment shown to the validation agent."},
                    "max_keywords_per_search": {"type": "integer", "default": 2, "description": "Draft keywords per Pexels search."},
                    "max_repair_iterations": {"type": "integer", "default": 3, "description": "Max repair-loop passes (stops early if a pass repairs 0 gaps)."},
                    "stickers": {"type": "boolean", "default": true, "description": "After b-roll coverage, also run the agentic sticker pipeline (sticker.keywords → GIPHY → Stickers track). Finalizes the A2V one-call."},
                    "captions": {"type": "boolean", "default": true, "description": "After b-roll coverage, generate styled ASS captions and register them in timeline.assets.captions. Finalizes the A2V one-call."}
                },
                "required": [],
                "additionalProperties": false
            }
        },
        {
            "name": "broll.probe",
            "description": "Search ALL stock engines (Pexels → Pixabay → YouTube) for a keyword and return a NORMALIZED, DEDUPLICATED, RANKED candidate pool — one StockCandidate model across engines with the shared stock_signal lexical gate. Use BEFORE broll.fetch/broll.validate_keywords to see what footage actually exists for a scene keyword: per-provider counts plus candidates with provider, id, title, duration_s, width, height, thumbnail, page_url, direct_url, and lexical relevance score. Pixabay is only searched when PIXABAY_API_KEY is set (film footage, not animation). YouTube is always searched via yt-dlp. Cross-engine dedup: the same clip title found on multiple providers collapses to one candidate (Pexels wins ties). Returns: status, query, per_provider counts, count, candidates[].",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search keyword(s) for stock footage, e.g. 'crowd protest rally' or 'morning coffee desk'."},
                    "aspect": {"type": "string", "default": "9:16", "description": "Video orientation: 9:16 / 16:9 / 1:1."},
                    "min_duration_s": {"type": "number", "default": 0, "description": "Only keep candidates at least this long (0 = no floor)."},
                    "max_duration_s": {"type": "number", "default": 0, "description": "Cap candidates at this duration (0 = no cap)."},
                    "per_provider": {"type": "integer", "default": 8, "description": "Max candidates to fetch per engine before dedup/rank."},
                    "signal": {"type": "array", "items": {"type": "string"}, "description": "Optional lexical bias tokens (e.g. from broll.keywords). Empty derives signal tokens from the query."}
                },
                "required": ["query"],
                "additionalProperties": false
            }
        },
        // ===================================================================
        // ===================================================================
        // GROUP 2c: ASSET DEVELOPMENT — user-curated footage library
        // (asset-development pipeline; separate from the generation pipeline)
        // ===================================================================
        {
            "name": "asset.library.status",
            "description": "Asset-development pipeline: library health summary. Returns schema version, media root, total assets, and counts by source (user_upload/pexels/pixabay/youtube) and curation status (candidate/approved/rejected). Use to see what the user's footage library contains before asset.search. Returns: status, version, root, total_assets, by_source, by_status.",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false}
        },
        {
            "name": "asset.ingest",
            "description": "Asset-development pipeline: scan a directory (default mcp/assets/user_library) and index new footage — ffprobe metadata, content-hash fingerprint, auto-keywords from filename. Idempotent (hash dedup skips already-indexed files). Runs BEFORE curation: the indexed entries are 'candidate' until asset.rate approves them. Returns: status, dir, indexed, skipped_duplicates, errors, total_assets.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "dir": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Directory to scan (default mcp/assets/user_library)"}
                },
                "additionalProperties": false
            }
        },
        {
            "name": "asset.probe",
            "description": "Asset-development pipeline: build a CURATION POOL — search Pexels + Pixabay + YouTube for N candidate clips matching keywords and return thumbnails + metadata WITHOUT downloading. The user/agent classifies each candidate (relevance, quality) via asset.rate, then asset.import downloads the winners into the local library. YouTube is always searched here (acquisition engine), independent of the generation opt-in flag. Returns: status, query, per_provider counts, count, candidates[] with thumbnail_url, duration_s, provider, id, direct_url.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search keywords for footage, e.g. 'morning desk coffee'"},
                    "aspect": {"type": "string", "default": "9:16", "description": "Orientation: 9:16 / 16:9 / 1:1"},
                    "min_duration_s": {"type": "number", "default": 0, "description": "Only keep candidates at least this long (0 = no floor)"},
                    "max_duration_s": {"type": "number", "default": 0, "description": "Cap candidates at this duration (0 = no cap)"},
                    "per_provider": {"type": "integer", "default": 8, "description": "Max candidates per provider before dedup/rank"},
                    "signal": {"type": "array", "items": {"type": "string"}, "description": "Optional lexical bias tokens; empty derives from the query"}
                },
                "required": ["query"],
                "additionalProperties": false
            }
        },
        {
            "name": "asset.rate",
            "description": "Asset-development pipeline: classify an asset (from asset.ingest or asset.import) — relevance 0-1 per keyword, quality 0-5, mood/energy/motion tags, and curation status (approved/rejected/candidate). Only approved assets with quality_rating >= 3.0 are eligible for the generation pipeline (Tier 1). Persists to mcp/assets/user_library_index.json. Returns: status, asset_id, curation_status, quality_rating.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "Asset id from asset.library.status / asset.ingest / asset.import"},
                    "relevance": {"type": "object", "additionalProperties": {"type": "number"}, "description": "Per-keyword relevance 0-1, e.g. {\"morning\": 0.9, \"desk\": 0.85}"},
                    "quality_rating": {"type": "number", "default": 0, "description": "Quality 0-5 (user-classified)"},
                    "mood": {"type": "string", "default": "", "description": "calm / energetic / neutral / dark / uplifting"},
                    "energy": {"type": "string", "default": "", "description": "low / medium / high"},
                    "motion_intensity": {"type": "string", "default": "", "description": "slow / medium / fast"},
                    "tags": {"type": "array", "items": {"type": "string"}, "description": "Free tags, e.g. vertical, clean, no_people"},
                    "status": {"type": "string", "default": "candidate", "description": "approved / rejected / candidate"}
                },
                "required": ["id"],
                "additionalProperties": false
            }
        },
        {
            "name": "asset.import",
            "description": "Asset-development pipeline: download a probed external candidate (YouTube via yt-dlp, direct file URL for Pexels/Pixabay) or copy a local file into mcp/assets/user_library/ and index it as a 'candidate'. Use AFTER asset.probe + asset.rate approved a clip. Returns: status, asset_id, path, total_assets.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Direct file URL (Pexels/Pixabay) or YouTube watch URL"},
                    "path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Local file path to copy into the library"},
                    "title": {"type": "string", "default": "", "description": "Human-readable title"},
                    "source": {"type": "string", "default": "user_upload", "description": "user_upload / pexels / pixabay / youtube"},
                    "provider_id": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Provider video id, e.g. pexels_4521"},
                    "keywords": {"type": "array", "items": {"type": "string"}, "description": "Search keywords for this clip"}
                },
                "additionalProperties": false
            }
        },
        {
            "name": "asset.search",
            "description": "Asset-development pipeline: search the curated library by keywords — returns only approved assets with quality_rating >= quality_floor (default 3.0), ranked by relevance-to-keywords × quality × freshness (least-recently-used first). This is the consumption side the generation pipeline uses as its Tier 1 footage source. Returns: status, count, assets[] with id, path, title, keywords, mood, quality_rating, duration_s, aspect, relevance, usage_count.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "keywords": {"type": "string", "description": "Search keywords, e.g. 'morning desk coffee'"},
                    "quality_floor": {"type": "number", "default": 3.0, "description": "Minimum quality_rating to return (default 3.0)"}
                },
                "required": ["keywords"],
                "additionalProperties": false
            }
        },
        // GROUP 3: VOICEOVER & TTS — Commentary, narration, and voice production
        // ===================================================================
        {
            "name": "voiceover.generate",
            "description": "Generate TTS voiceover and add it to the timeline's voiceover track at a specific position. USE CASES: (1) INTRO — hook the viewer with a spoken opening before the main content starts; (2) TRANSITIONS — narrate between segments to guide the viewer; (3) OUTRO — close with a call-to-action or summary; (4) COMMENTARY — add explanatory narration over b-roll sections. Generates WAV audio, places it on the voiceover track, and applies ducking to background music. Returns: output_path, duration_ms, event_id.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_path": {"type": "string", "description": "Path to timeline JSON"},
                    "text": {"type": "string", "description": "Script text for the voiceover"},
                    "voice_profile_id": {"type": "string", "description": "ID of the voice profile to speak with"},
                    "position_ms": {"type": "integer", "default": 0, "description": "Timeline position in ms to start voiceover (0 = beginning)"},
                    "speed": {"type": "number", "default": 1.0, "description": "Speech speed multiplier"},
                    "gain_db": {"type": "number", "default": -6.0, "description": "Voiceover volume in dB relative to other tracks"}
                },
                "required": ["timeline_path", "text", "voice_profile_id"],
                "additionalProperties": false
            }
        },
        {
            "name": "tts.commentary",
            "description": "Generate multiple TTS voiceover commentaries at strategic timeline positions in ONE CALL. commentary_type='all' creates: (1) INTRO at position 0 — welcomes viewers; (2) TRANSITIONS before each segment — guides viewers between topics; (3) OUTRO at end — thanks viewers and gives CTA. Use commentary_type='intro', 'transitions', or 'outro' for individual pieces. MUCH faster than calling voiceover.generate multiple times. Returns: voiceovers_generated (event IDs), positions, count.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_path": {"type": "string", "description": "Path to timeline JSON"},
                    "voice_profile_id": {"type": "string", "description": "ID of the voice profile to use"},
                    "commentary_type": {"type": "string", "enum": ["intro", "transitions", "outro", "all"], "default": "all", "description": "Which commentaries to generate: 'intro' (opening), 'transitions' (between segments), 'outro' (closing), 'all' (complete set)"},
                    "intro_text": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Custom intro script (auto-generated if omitted)"},
                    "outro_text": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Custom outro script (auto-generated if omitted)"},
                    "speed": {"type": "number", "default": 1.0, "description": "Speech speed multiplier"}
                },
                "required": ["timeline_path", "voice_profile_id", "commentary_type"],
                "additionalProperties": false
            }
        },
        // ===================================================================
        // GROUP 4: AGENT UX — Inspection, comparison, and preview tools
        // ===================================================================
        {
            "name": "timeline.diff",
            "description": "Compare two versions of a timeline and report what changed — added/removed/modified segments, track count changes, duration delta. Use before/after edits to understand impact, or to verify an edit produced the expected changes. Returns: duration_change_ms, segments (added, removed, modified), track changes.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_path_a": {"type": "string", "description": "Path to first timeline version"},
                    "timeline_path_b": {"type": "string", "description": "Path to second timeline version"}
                },
                "required": ["timeline_path_a", "timeline_path_b"],
                "additionalProperties": false
            }
        },
        {
            "name": "timeline.preview",
            "description": "Generate a readable summary of a timeline — segments with captions, track counts per type, total duration, validation status, and render readiness. ALWAYS call this before timeline.render to verify the timeline looks correct. Returns: total_duration_ms, segments, tracks (with counts), render_ready (boolean), validation_errors.",
            "inputSchema": {
                "type": "object",
                "properties": {"timeline_path": {"type": "string", "description": "Path to timeline JSON"}},
                "required": ["timeline_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "tts.preview",
            "description": "Preview TTS output for a given text and voice profile — estimates duration and shows profile info WITHOUT generating audio. Use to plan timeline placement before committing to voiceover.generate. Returns: voice_profile info, word_count, estimated_duration_ms.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "voice_profile_id": {"type": "string", "description": "ID of the voice profile"},
                    "text": {"type": "string", "description": "Text to preview"},
                    "speed": {"type": "number", "default": 1.0, "description": "Speed multiplier"}
                },
                "required": ["voice_profile_id", "text"],
                "additionalProperties": false
            }
        },
        {
            "name": "music.ducking.plan",
            "description": "Analyze the timeline's dialogue and voiceover tracks to generate a ducking plan — where and how much to lower background music. Returns start_ms, end_ms, reduction_db for each ducking event. Use to understand how music will behave during speech sections before rendering. Returns: ducking_events array with timing and reduction values.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_path": {"type": "string", "description": "Path to timeline JSON"},
                    "reduction_db": {"type": "number", "default": 10.0, "description": "How many dB to reduce music during speech"}
                },
                "required": ["timeline_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "timeline.autofill_broll",
            "description": "Auto-fill b-roll slots across the timeline based on segment cadence. Creates placeholder b-roll events at regular intervals (cadence_seconds) using concept keywords extracted from nearby segment captions. FASTER than manual broll.fetch but LESS contextually accurate — use for quick drafts, then refine with agent-generated keywords via broll.fetch for final. Returns: broll_events_added count.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_path": {"type": "string", "description": "Path to timeline JSON"},
                    "cadence_seconds": {"type": "number", "default": 2.0, "description": "Interval between b-roll slots"},
                    "orientation": {"type": "string", "default": "9:16", "description": "Expected b-roll orientation"},
                    "quality": {"type": "string", "default": "sd", "description": "Expected b-roll quality"},
                    "max_gaps": {"type": "integer", "default": 20, "description": "Maximum b-roll slots to create"}
                },
                "required": ["timeline_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "timeline.render",
            "description": "Render a complete multi-track timeline to a final video. This is the PRODUCTION render — it processes ALL tracks: b-roll overlays, background music with ducking, SFX hits, voiceover narration, and burned-in captions (static ASS or animated via PupCaps overlay). ALWAYS run timeline.validate first. Returns: output_path, file_size_bytes, segments_count, overlays_rendered.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_path": {"type": "string", "description": "Path to validated timeline JSON"},
                    "source_video": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Override source video path (uses timeline source if omitted)"},
                    "output_path": {"anyOf": [{"type": "string"}, ], "description": "Custom output path (auto-generated if omitted)"},
                    "crf": {"type": "integer", "default": 20, "description": "Video quality (18-28, lower=better)"}
                },
                "required": ["timeline_path"],
                "additionalProperties": false
            }
        },
        // ===================================================================
        // GROUP 5: ORCHESTRATION — Single-call end-to-end pipelines
        // ===================================================================
// ===================================================================
        // GROUP 6: VERIFICATION — Render quality assurance
        // ===================================================================
        {
            "name": "verify.audio",
            "description": "Analyze the audio track of a rendered video for quality issues. Checks: (1) RMS loudness — is the overall volume in acceptable range? (2) Dialogue presence — is there spoken content or just music? (3) Silence detection — are there unexpected gaps? (4) Peak levels — is there clipping? (5) Per-scene loudness variance — when scene voiceover WAVs are supplied (scene_wavs array or voiceover_manifest), flags a >6 dB LUFS spread between scenes (quiet scenes get buried under the music bed). Use AFTER rendering to verify the voice is audible and music isn't drowning out dialogue. Returns: rms_lufs, peak_db, silence_segments, has_dialogue (boolean), quality_score (0-100), loudness (per-scene variance KPI).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "video_path": {"type": "string", "description": "Path to the rendered video to analyze"},
                    "expected_has_voice": {"type": "boolean", "default": true, "description": "Whether the video is expected to contain spoken voice"},
                    "max_silence_seconds": {"type": "number", "default": 3.0, "description": "Threshold for flagging unexpected silence gaps"},
                    "scene_wavs": {"type": "array", "items": {"type": "string"}, "description": "Optional per-scene voiceover WAV paths (e.g. artifacts/renders/air/voices/scene_*_narrator.wav). When provided, measures each scene's integrated LUFS and reports the spread — a >6 dB spread is flagged as an issue and costs score."},
                    "voiceover_manifest": {"type": "string", "description": "Optional path to a script.generate_voices manifest.json (segments[].wav_path). Alternative to scene_wavs for supplying per-scene voiceover files."}
                },
                "required": ["video_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "verify.captions",
            "description": "Verify caption synchronization in a rendered video. Compares caption timing from the source SRT/ASS against the actual video duration to check: (1) Coverage — do captions span the full speaking duration? (2) Gaps — are there sections without captions that should have them? (3) Overlap — do any captions overlap incorrectly? (4) Duration — are individual captions readable (not too fast)? Use AFTER rendering to ensure captions are properly burned in and timed. Auto-detects caption format: .ass files (from script.to_video) and .srt files are both accepted. Returns: caption_count, coverage_percent, gaps, overlaps, avg_caption_duration_ms, readability_score (0-100).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "video_path": {"type": "string", "description": "Path to the rendered video"},
                    "srt_path": {"type": "string", "description": "Source SRT or ASS caption file. script.to_video produces captions.ass in the output_dir; pass that path here."},
                    "min_caption_duration_ms": {"type": "integer", "default": 300, "description": "Minimum readable caption duration in ms"},
                    "max_caption_duration_ms": {"type": "integer", "default": 5000, "description": "Maximum caption duration before flagging"}
                },
                "required": ["video_path", "srt_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "verify.render",
            "description": "TECHNICAL integrity check only (duration/aspect/file size). Does NOT measure production beauty. For stock footage, stickers, music quality use verify.production. Returns: overall_score (0-100 technical).",

            "inputSchema": {
                "type": "object",
                "properties": {
                    "video_path": {"type": "string", "description": "Path to the rendered output video"},
                    "timeline_path": {"type": "string", "description": "Path to the source timeline JSON"},
                    "expected_aspect": {"type": "string", "default": "9:16", "description": "Expected output aspect ratio"},
                    "duration_tolerance_ms": {"type": "integer", "default": 2000, "description": "Acceptable duration deviation in ms"}
                },
                "required": ["video_path", "timeline_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "llm.complete",
            "description": "Run a text LLM completion through the director cascade configured in ~/.openscript/config.json: OpenCode zen (default) → OpenRouter free models. Returns: text, backend, model.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "prompt": {"type": "string", "description": "User prompt"},
                    "system": {"type": "string", "default": "You are a helpful video director assistant.", "description": "System prompt"},
                    "backend": {"type": "string", "default": "auto", "description": "Force backend: auto | opencode | openrouter"}
                },
                "required": ["prompt"],
                "additionalProperties": false
            }
        },
        {
            "name": "system.config.get",
            "description": "Return the effective OpenScript configuration (redacted secrets) from ~/.openscript/config.json with env overrides applied. Use to verify LLM models, base URLs, and which API keys are set.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": [],
                "additionalProperties": false
            }
        },
        {
            "name": "system.config.set",
            "description": "Merge keys into ~/.openscript/config.json (mode 0600). Supports nested paths via object: {api_keys:{openrouter:'…', opencode:'…'}, llm:{opencode_model:'mimo-v2.5-free'}}. Does not echo secrets back. Returns redacted config view.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "patch": {"type": "object", "description": "Partial config object to deep-merge into user config"}
                },
                "required": ["patch"],
                "additionalProperties": false
            }
        },
        {
            "name": "director.run",
            "description": "ONE-SHOT director: system preflight + script.parse + script.to_video + verify.production. Returns video path, production grade, hard_fails, next_actions. Prefer this for cold agents. Fails closed when majority procedural or music topic mismatch.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "script": {"type": "string", "description": "Script JSON string or path to .json"},
                    "output_path": {"type": "string", "default": "artifacts/director_out.mp4"},
                    "output_dir": {"type": "string", "default": "artifacts/director_run"},
                    "min_grade": {"type": "string", "default": "B"}
                },
                "required": ["script"],
                "additionalProperties": false
            }
        },
        {
            "name": "vision.analyze_clip",
            "description": "Extract a frame from a video clip and describe it with the vision cascade (OpenRouter multimodal free models when OPENROUTER_API_KEY is set; local Qwen text fallback). Use to judge morning/night, indoor/outdoor, phone UI, etc. Returns structured description + backend.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "video_path": {"type": "string", "description": "Path to video clip"},
                    "at_s": {"type": "number", "description": "Optional timestamp seconds (default ~40% into clip)"},
                    "prompt": {"type": "string", "description": "Optional analysis question"}
                },
                "required": ["video_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "vision.score_clip",
            "description": "Vision+LLM relevance score of a stock clip vs scene dialogue and video_keywords. Uses OpenCode zen when possible and OpenRouter free multimodal fallbacks. Returns relevance 0–1, time_of_day, match, reason. Wire into multi-broll QA and verify.production context_relevance.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "video_path": {"type": "string"},
                    "scene_text": {"type": "string"},
                    "video_keywords": {"type": "array", "items": {"type": "string"}},
                    "search_query": {"type": "string"}
                },
                "required": ["video_path", "scene_text"],
                "additionalProperties": false
            }
        },
        {
            "name": "verify.production",
            "description": "PRODUCTION-QUALITY KPI gate (v2.1) baked into architecture. Scores efficacious director use of the timeline/render stack including visual_repetition (content-hash) and context_relevance. Prefer render_manifest_path from script.to_video. Optional vision_rescore=true re-scores clips with vision.analyze cascade. verify.render=100 is NOT production quality.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "video_path": {"type": "string", "description": "Rendered MP4 path"},
                    "timeline_path": {"type": "string", "description": "Timeline JSON from script.to_video"},
                    "render_manifest_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Preferred: render_manifest.json written by script.to_video (authoritative multi-broll/sticker/meme truth)"},
                    "captions_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Optional ASS/SRT path"},
                    "sticker_count": {"type": "integer", "default": 0, "description": "Fallback sticker count if no manifest"},
                    "meme_count": {"type": "integer", "default": 0, "description": "Fallback meme count if no manifest"},
                    "background_sources": {"type": "array", "items": {"type": "string"}, "description": "Fallback background paths if no manifest"},
                    "music_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Music path used at render if any"},
                    "min_grade": {"type": "string", "default": "B", "description": "Minimum acceptable grade (A/B/C/D/F)."},
                    "vision_rescore": {"type": "boolean", "default": false, "description": "If true, re-score each background clip with vision.score_clip (OpenCode zen → OpenRouter free multimodal). Adds vision_scores to the response."},
                    "video_keywords": {"type": "array", "items": {"type": "string"}, "description": "Agent-generated keywords describing the video content (e.g., ['corruption', 'protest', 'freedom']). Used by context_relevance scoring to verify b-roll matches the topic."},
                    "caption_style": {"type": "string", "description": "Caption style used: 'word_highlight', 'standard', 'kinetic', 'karaoke'. Detected from ASS file if not provided."}
                },
                "required": ["video_path", "timeline_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "script.schema",
            "description": "Return the full JSON schema for script.parse as a JSON object. Use this to discover what fields are available, their types, defaults, and valid values. Returns: the JSON Schema with examples and field descriptions for ScriptSpec, SceneSpec, SpeakerSpec, BackgroundSpec, and all nested types. Call this BEFORE script.parse to understand the correct format.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": [],
                "additionalProperties": false
            }
        },
        {
            "name": "script.parse",
            "description": "Parse and validate a from-scratch video creation script (JSON). The script is the single source of truth for AI-agent-driven video creation — it describes speakers, scenes, backgrounds, captions, music, and output. Returns the parsed ScriptSpec with defaults applied, plus validation errors (if any). Use BEFORE script.to_timeline / script.to_video to catch schema issues early. See openscript-core/src/script.rs for the full schema. Kokoro is the default TTS backend. IMPORTANT: background.type must be one of: 'gameplay' (Pexels stock footage, requires PEXELS_API_KEY), 'procedural' (generated motion backgrounds), or 'static' (solid color/gradient). 'stock' is NOT a valid value — using it will cause errors.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "script": {"type": "string", "description": "The script JSON string (or path to a .json file). If it starts with '{', parsed as inline JSON; otherwise treated as a file path."},
                    "validate_only": {"type": "boolean", "default": false, "description": "If true, only validate without returning the full parsed spec (lighter response)"}
                },
                "required": ["script"],
                "additionalProperties": false
            }
        },
        {
            "name": "script.generate_voices",
            "description": "Generate TTS voice audio for each scene in a script. Calls the TTS backend (Kokoro default, sidecar fallback) per scene, producing a WAV file per scene. Returns voiceover paths + durations + estimated word timings for caption sync. Use AFTER script.parse and BEFORE script.build_captions. Progress reported per scene.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "script": {"type": "string", "description": "The script JSON string or path to .json file"},
                    "output_dir": {"type": "string", "default": "artifacts/voices", "description": "Directory for generated WAV files"}
                },
                "required": ["script"],
                "additionalProperties": false
            }
        },
        {
            "name": "script.build_captions",
            "description": "Generate an ASS subtitle file from voiceover segments with word timings. Supports 4 caption styles: word_highlight (TikTok-style), sentence_fade, karaoke_fill, subtitle_rail. Uses the caption style from the script spec. Returns the ASS file path.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "script": {"type": "string", "description": "The script JSON string or path to .json file"},
                    "voiceover_manifest": {"type": "string", "description": "Path to the manifest JSON from script.generate_voices (contains segments with word timings)"},
                    "output_path": {"type": "string", "default": "artifacts/captions.ass", "description": "Output ASS file path"}
                },
                "required": ["script", "voiceover_manifest"],
                "additionalProperties": false
            }
        },
        {
            "name": "background.fetch",
            "description": "Fetch a background video clip. Searches Pexels API FIRST (stock footage, requires PEXELS_API_KEY), then YouTube via yt-dlp as fallback. Downloads, extracts a random clip of desired duration, crops to target aspect ratio. For multi-broll: call once per scene with different queries to get topic-relevant backgrounds. Non-redundant: pass used_video_ids (Pexels ids already used in this run) and the same stock clip is never re-fetched under a different query. Vision-aware: pass scene_text (the scene's dialogue) to (a) seed the vision relevance gate that rejects clips whose actual frames don't match the scene, and (b) scope the cache key so a different scene never reuses another scene's cached clip. YouTube candidates are ranked by lexical relevance x duration preference (lecture penalty) and thumbnail/frame vision gates. Returns: clip_path, pexels_id, source (pexels/youtube/fallback/procedural), duration_s, lexical_score, vision_score, vision_reason.",
            "inputSchema": {
                "type": "object",
                "properties": {
            "query": {"type": "string", "description": "YouTube search query (e.g. 'minecraft parkour no copyright')"},
            "duration_s": {"type": "number", "default": 30.0, "description": "Desired clip duration in seconds"},
            "min_duration_s": {"anyOf": [{"type": "number"}, {"type": "null"}], "default": 0, "description": "Minimum clip duration in seconds (SEGMENTATION_ARCHITECTURE min clip duration). Clips shorter than this are skipped so they never need looping — alternates are fetched instead. 0 = default to duration_s."},
            "max_duration_s": {"anyOf": [{"type": "number"}, {"type": "null"}], "default": 0, "description": "Maximum clip duration in seconds (SEGMENTATION_ARCHITECTURE max clip duration). 0 = no cap."},
            "aspect": {"type": "string", "default": "9:16", "description": "Target aspect ratio: 9:16, 16:9, 1:1"},
                    "cache_dir": {"type": "string", "default": "mcp/assets/background_cache", "description": "Cache directory for downloaded videos"},
                    "fallback_pool": {"type": "array", "items": {"type": "string"}, "description": "Local fallback clips if YouTube download fails"},
                    "used_video_ids": {"type": "array", "items": {"type": "integer"}, "description": "Pexels video ids already used in this run/timeline — these clips are skipped so the same footage never repeats under a different query. Returned pexels_id values from prior calls should be accumulated here."},
                    "scene_text": {"type": "string", "description": "The scene's dialogue/text this clip will illustrate. Used (a) to seed the vision relevance gate that rejects clips whose actual frames don't match the scene, and (b) to scope the cache key so a different scene never reuses another scene's cached clip."}
                },
                "required": ["query"],
                "additionalProperties": false
            }
        },
        {
            "name": "background.assign",
            "description": "Assign background clips to script scenes based on change_cadence (scene/speaker/fixed). Takes a voiceover manifest (from script.generate_voices) and a background pool, returns assignments. Use AFTER script.generate_voices and background.fetch.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "script": {"type": "string", "description": "The script JSON string or path to .json file"},
                    "voiceover_manifest": {"type": "string", "description": "Path to manifest JSON from script.generate_voices"},
                    "background_pool": {"type": "array", "items": {"type": "string"}, "description": "List of background video file paths to assign"},
                    "output_path": {"type": "string", "default": "artifacts/background_assignments.json", "description": "Output assignments JSON path"}
                },
                "required": ["script", "voiceover_manifest", "background_pool"],
                "additionalProperties": false
            }
        },
        {
            "name": "background.search",
            "description": "Search the procedural background clip index by mood/energy/motion_intensity. Returns matching clip paths from mcp/assets/backgrounds/. Use this to build a curated fallback_pool for script.to_video when you want a specific emotional tone (e.g. mood:calm for healing content, mood:energetic for gaming recaps). Without this, script.to_video with type:procedural grabs ALL .mp4s in the folder — which mixes calming clips with neon tunnels. Returns: clip paths + metadata.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "mood": {"type": "string", "enum": ["calm", "energetic", "neutral", "dark", "uplifting"], "description": "Emotional tone filter"},
                    "energy": {"type": "string", "enum": ["low", "medium", "high"], "description": "Energy level filter"},
                    "motion_intensity": {"type": "string", "enum": ["slow", "medium", "fast"], "description": "Motion intensity filter"},
                    "limit": {"type": "integer", "default": 10, "description": "Max results"}
                },
                "required": [],
                "additionalProperties": false
            }
        },
        {
            "name": "sticker.presets",
            "description": "List all available sticker positioning presets with their safe-zone configurations. Each preset defines position, scale, and caption clearance for different use cases (speaker left/right/center, reactions, corners). Returns: presets map with name, description, position, scale, safe_margin_px, and speaker_role.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": [],
                "additionalProperties": false
            }
        },
        {
            "name": "sticker.load_preset",
            "description": "Load an SVG sticker preset by name. Presets are directories in mcp/assets/svg_presets/ containing puppet.svg, preset.json, mouth shapes, and emotes. Built-in presets: default_person, robot, cat. Returns the preset config + puppet SVG content.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "preset_name": {"type": "string", "description": "Preset name (e.g. 'default_person', 'robot', 'cat')"},
                    "presets_dir": {"type": "string", "default": "mcp/assets/svg_presets", "description": "Directory containing preset folders"}
                },
                "required": ["preset_name"],
                "additionalProperties": false
            }
        },
        {
            "name": "sticker.render",
            "description": "Render an animated sticker overlay (HyperFrames HTML composition) for a speaker's voiceover. Extracts per-frame amplitude from the WAV, generates GSAP timeline that animates the SVG puppet's mouth scaleY in sync with audio. Produces an HTML file. When render_to_video=true, also renders the HTML to a transparent WebM via hf.render — the WebM can be used directly as a StickerOverlay in multilayer_render or overlay.assign. Use AFTER script.generate_voices and sticker.load_preset.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "wav_path": {"type": "string", "description": "Path to the speaker's voiceover WAV file"},
                    "preset_name": {"type": "string", "description": "SVG preset name (e.g. 'default_person')"},
                    "position": {"type": "string", "default": "top-left", "description": "On-screen position: top-left, top-right, bottom-left, bottom-right, top-center, bottom-center, center"},
                    "scale": {"type": "number", "default": 0.25, "description": "Sticker scale relative to canvas width (0.0-1.0)"},
                    "canvas_width": {"type": "integer", "default": 1080},
                    "canvas_height": {"type": "integer", "default": 1920},
                    "fps": {"type": "integer", "default": 30},
                    "output_path": {"type": "string", "default": "artifacts/sticker.html", "description": "Output HTML composition path"},
                    "render_to_video": {"type": "boolean", "default": false, "description": "When true, also render the HTML to a transparent WebM via hf.render. Returns video_path in the response. Slower (~30s per sticker) but produces a compositable video file."}
                },
                "required": ["wav_path", "preset_name"],
                "additionalProperties": false
            }
        },
        {
            "name": "script.to_timeline",
            "description": "Orchestrate the full from-scratch video creation pipeline into an EDL v2 timeline. Calls script.generate_voices (TTS per scene), script.build_captions (ASS from word timings), background.fetch + background.assign (YouTube gameplay), sticker.render (animated SVG per scene), and assembles everything into a timeline JSON ready for timeline.render. This is the orchestrator — use script.to_video for the one-call pipeline. Returns timeline_path + manifest paths + asset summary.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "script": {"type": "string", "description": "The script JSON string or path to .json file"},
                    "output_dir": {"type": "string", "default": "artifacts", "description": "Directory for generated assets (voices, captions, stickers, timeline)"},
                    "skip_background": {"type": "boolean", "default": false, "description": "Skip background fetching (use fallback pool only)"},
                    "skip_stickers": {"type": "boolean", "default": false, "description": "Skip sticker rendering (no animated overlays)"},
                    "voiceover_manifest_path": {"type": "string", "description": "Optional: path to a pre-existing voiceover manifest JSON. When provided, skips TTS generation and uses the supplied manifest. Manifest format: {total_duration_ms, segments: [{scene_id, speaker, text, start_ms, end_ms, duration_ms, wav_path, words: [{word, start_ms, end_ms}]}]}"}
                },
                "required": ["script"],
                "additionalProperties": false
            }
        },
        {
            "name": "script.to_video",
            "description": "ONE-CALL from-scratch video creation: script JSON → MP4. THE GOLDEN TRAJECTORY — use this for all video creation. Automatically handles: Kokoro TTS per scene, Parakeet force-alignment for caption sync, multi-broll Pexels stock footage per scene, GIPHY sticker overlays, background music with ducking, word-highlight captions, and FFmpeg render. Returns: output_path, file_size, timeline_preview (token-efficient tree view of all layers), timeline_issues, warnings. PREVIOUS STEP: script.parse (validate first). NEXT STEP: verify.render (quality check).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "script": {"type": "string", "description": "The script JSON string or path to .json file"},
                    "output_path": {"type": "string", "default": "output.mp4", "description": "Output MP4 path"},
                    "output_dir": {"type": "string", "default": "artifacts", "description": "Directory for intermediate assets"},
                    "skip_background": {"type": "boolean", "default": false, "description": "Skip background fetching"},
                    "skip_stickers": {"type": "boolean", "default": false, "description": "Skip sticker rendering"},
                    "preview_mode": {"type": "boolean", "default": false, "description": "If true, use draft quality for faster iteration"},
                    "voiceover_manifest_path": {"type": "string", "description": "Optional: path to a pre-existing voiceover manifest JSON. When provided, skips TTS generation (script.generate_voices) and uses the supplied manifest instead. Use this when you have pre-recorded WAV files and want to bypass TTS. Manifest format: {total_duration_ms, segments: [{scene_id, speaker, text, start_ms, end_ms, duration_ms, wav_path, words: [{word, start_ms, end_ms}]}]}"}
                },
                "required": ["script"],
                "additionalProperties": false
            }
        },
        {
            "name": "stock.fetch",
            "description": "Download stock music or videos from Pixabay API. Requires PIXABAY_API_KEY env var. Falls back to local stock library if API key not set. For music: downloads MP3 tracks by mood/genre query. For video: downloads footage clips — `video_type` defaults to 'film' (real footage; set 'animation' only if you explicitly want motion graphics). Returns downloaded file paths. Use for sourcing royalty-free background music and video footage.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "type": {"type": "string", "enum": ["music", "video"], "description": "Media type to fetch"},
                    "query": {"type": "string", "description": "Search query (e.g. 'lofi chill' for music, 'minecraft gameplay' for video)"},
                    "limit": {"type": "integer", "default": 5, "description": "Max results to download"},
                    "output_dir": {"type": "string", "default": "mcp/assets/stock_cache", "description": "Directory for downloaded files"},
                    "video_type": {"type": "string", "enum": ["film", "animation"], "default": "film", "description": "For video: 'film' = real footage (b-roll), 'animation' = motion graphics. Ignored for music."}
                },
                "required": ["type", "query"],
                "additionalProperties": false
            }
        },
        {
            "name": "youtube.download",
            "description": "Download a YouTube video clip for use as background footage. Accepts a direct YouTube URL or search query. If start_s is specified, uses --download-sections to download ONLY that time range (avoids downloading entire 10-hour videos). If start_s is omitted, downloads the full video and extracts a random clip. Crops to target aspect ratio. Use youtube.search first to find the video URL and duration, then specify start_s to clip a specific range.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "YouTube URL or search query (e.g. 'https://youtube.com/watch?v=...' or 'minecraft parkour no copyright')"},
                    "duration_s": {"type": "number", "default": 30.0, "description": "Clip duration in seconds"},
                    "start_s": {"type": "number", "description": "Start time in seconds for range download. If specified, downloads ONLY this range using --download-sections (much faster for long videos). If omitted, downloads full video and extracts random clip."},
                    "aspect": {"type": "string", "default": "9:16", "description": "Target aspect ratio: 9:16, 16:9, 1:1"},
                    "cache_dir": {"type": "string", "default": "mcp/assets/background_cache", "description": "Cache directory"},
                    "use_cookies": {"type": "boolean", "default": true, "description": "Try browser cookies to avoid YouTube bot detection"}
                },
                "required": ["query"],
                "additionalProperties": false
            }
        },
        {
            "name": "youtube.search",
            "description": "Search YouTube for videos WITHOUT downloading. Returns video titles, URLs, durations, and view counts so agents can browse and pick the best video before downloading via youtube.download. Uses yt-dlp's search functionality. Requires yt-dlp installed.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search query (e.g. 'minecraft parkour no copyright gameplay')"},
                    "limit": {"type": "integer", "default": 10, "description": "Max results to return"}
                },
                "required": ["query"],
                "additionalProperties": false
            }
        },
        {
            "name": "stock.search",
            "description": "Search Pixabay for stock music or videos WITHOUT downloading. Returns titles, durations, thumbnails, and URLs so agents can browse before downloading via stock.fetch. Requires PIXABAY_API_KEY env var. Falls back to local stock library listing if no API key. Video searches default to `video_type` 'film' (real footage).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "type": {"type": "string", "enum": ["music", "video"], "description": "Media type to search"},
                    "query": {"type": "string", "description": "Search query"},
                    "limit": {"type": "integer", "default": 10, "description": "Max results"},
                    "video_type": {"type": "string", "enum": ["film", "animation"], "default": "film", "description": "For video: 'film' = real footage (b-roll), 'animation' = motion graphics. Ignored for music."}
                },
                "required": ["type", "query"],
                "additionalProperties": false
            }
        },
        {
            "name": "media.search",
            "description": "Search for PNG images for use as sticker overlays. Uses Pexels Image API (requires PEXELS_API_KEY) and Openverse (free, no key). Returns image URLs, dimensions, and license info. To use a result: download the URL (via curl/wget/reqwest) to a local path, then place it on the timeline via timeline.add_track_event with track_type='broll' and the local path as asset_id. NOTE: media.download does not exist yet — download manually. Use for finding transparent PNGs of people, objects, logos, etc. for video sticker overlays.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search query (e.g. 'person talking', 'businessman portrait', 'cartoon character')"},
                    "limit": {"type": "integer", "default": 10, "description": "Max results"},
                    "source": {"type": "string", "enum": ["pexels", "openverse", "auto"], "default": "auto", "description": "Image source: pexels (requires API key), openverse (free), or auto (try pexels first)"}
                },
                "required": ["query"],
                "additionalProperties": false
            }
        },
        {
            "name": "gif.search",
            "description": "Search for animated GIF stickers (transparent background) from GIPHY. Returns GIF URLs, dimensions, and preview URLs. To use a result: download the URL to a local .gif file, then pass the path to script.to_video (which auto-downloads GIPHY stickers per speaker) or use multilayer_render's sticker overlay. NOTE: gif.download does not exist yet — download manually. GIPHY stickers are transparent GIFs ideal for video overlays. Requires GIPHY_API_KEY env var (get free at https://developers.giphy.com). Falls back to Pexels video search if no GIPHY key.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search query (e.g. 'person talking', 'thumbs up', 'applause')"},
                    "limit": {"type": "integer", "default": 10, "description": "Max results"},
                    "rating": {"type": "string", "enum": ["g", "pg", "pg-13", "r"], "default": "g", "description": "Content rating filter"}
                },
                "required": ["query"],
                "additionalProperties": false
            }
        },
        {
            "name": "media.download",
            "description": "Download an image from a URL (from media.search results) to a local file. Returns the local path. Use after media.search to get a local PNG/JPG that can be placed on the timeline via overlay.assign. Caches downloads in mcp/assets/image_cache/ to avoid re-downloading.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": {"type": "string", "description": "Image URL from media.search results"},
                    "output_path": {"type": "string", "description": "Optional output path. Auto-generated if omitted (mcp/assets/image_cache/<hash>.<ext>)."}
                },
                "required": ["url"],
                "additionalProperties": false
            }
        },
        {
            "name": "gif.download",
            "description": "Download a GIF from a URL (from gif.search results) to a local file. Returns the local path. Use after gif.search to get a local .gif that can be placed on the timeline via overlay.assign or used as a sticker in script.to_video. Caches downloads in mcp/assets/stickers/ to avoid re-downloading.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": {"type": "string", "description": "GIF URL from gif.search results"},
                    "output_path": {"type": "string", "description": "Optional output path. Auto-generated if omitted (mcp/assets/stickers/<hash>.gif)."}
                },
                "required": ["url"],
                "additionalProperties": false
            }
        },
        {
            "name": "timeline.inspect",
            "description": "Deep-dive inspection of a specific layer in the timeline. Returns ALL events on that layer with full details (start_ms, end_ms, asset path, metadata). Use AFTER script.to_video to inspect a specific layer for restructuring. Layers: background, voiceover, music, captions, stickers. For a quick overview use the timeline_preview field from script.to_video instead.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_preview_path": {"type": "string", "description": "Path to the timeline_preview.txt file from script.to_video"},
                    "layer": {"type": "string", "enum": ["background", "voiceover", "music", "captions", "stickers"], "description": "Which layer to inspect in detail"}
                },
                "required": ["timeline_preview_path", "layer"],
                "additionalProperties": false
            }
        },
        {
            "name": "overlay.assign",
            "description": "Place an image/GIF/PNG overlay on the timeline at a specific position and duration. Use after media.download or gif.download to place the local file as a sticker/overlay on the video. The overlay is composited via FFmpeg's overlay filter during render. Returns event_id. Supports position (top-left/top-right/bottom-left/bottom-right/center), scale (0.0-1.0 of canvas width), fade_in_ms, fade_out_ms.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_path": {"type": "string", "description": "Path to the timeline JSON file"},
                    "asset_path": {"type": "string", "description": "Local path to the image/GIF/PNG file (from media.download or gif.download)"},
                    "start_ms": {"type": "integer", "description": "Position in milliseconds where the overlay appears"},
                    "end_ms": {"type": "integer", "description": "Position in milliseconds where the overlay disappears"},
                    "position": {"type": "string", "enum": ["top-left", "top-right", "bottom-left", "bottom-right", "center"], "default": "bottom-right", "description": "Screen position of the overlay"},
                    "scale": {"type": "number", "default": 0.2, "description": "Scale factor relative to canvas width (0.0-1.0, default 0.2 = 20% of width)"},
                    "fade_in_ms": {"type": "integer", "default": 0, "description": "Fade-in duration in milliseconds"},
                    "fade_out_ms": {"type": "integer", "default": 0, "description": "Fade-out duration in milliseconds"},
                    "speaker_name": {"type": "string", "description": "Optional: speaker name this overlay is associated with (for provenance tracking)"}
                },
                "required": ["timeline_path", "asset_path", "start_ms", "end_ms"],
                "additionalProperties": false
            }
        },
        {
            "name": "sticker.keywords",
            "description": "STAGE 1 of the agentic sticker pipeline (parallel to broll.keywords): extract GIPHY sticker search keywords from transcript segments using an LLM. Translates Hinglish/Hindi captions into short reaction/meme/emotion keywords that GIPHY sticker search understands (e.g. 'mind blown', 'facepalm', 'celebration', 'sad'). Each segment maps to 2-3 keywords. Use BEFORE sticker.auto_assign so each segment gets the IDEAL sticker instead of naive caption words. Returns: segments with sticker_keywords (array of strings).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "segments": {"type": "array", "items": {"type": "object", "properties": {"id": {"type": "string"}, "start_s": {"type": "number"}, "end_s": {"type": "number"}, "caption": {"type": "string"}}}, "description": "Segments array from segment.analyze or broll.plan. Each segment's caption is translated to GIPHY sticker keywords."},
                    "language": {"type": "string", "default": "hinglish", "description": "Source language of captions: 'hinglish', 'hindi', 'english', 'mixed'."},
                    "max_batch_size": {"type": "integer", "default": 15, "description": "Max segments per LLM call."}
                },
                "required": ["segments"],
                "additionalProperties": false
            }
        },
        {
            "name": "sticker.validate_keywords",
            "description": "STAGE 2 of the agentic sticker pipeline (parallel to broll.validate_keywords): relevance gate between sticker.keywords and placement. Takes sticker.keywords output, searches GIPHY for REAL candidate stickers per segment, and an LLM approves the best match against the spoken caption's intent/emotion. Segments with no emphatic keywords, no GIPHY results, or no approved match are skipped (better no sticker than an irrelevant one). Use BEFORE sticker.auto_assign so only genuinely relevant stickers reach the timeline. Returns: segments with approved, best_sticker (id/title/url), final_keyword, relevance, reason, candidates + skipped with reasons.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "enriched_segments": {"type": "array", "items": {"type": "object"}, "description": "Output of sticker.keywords: segments each with id, caption, intent, emphatic, sticker_keywords."},
                    "max_candidates": {"type": "integer", "default": 4, "description": "Max GIPHY candidates to validate per segment."},
                    "language": {"type": "string", "default": "hinglish", "description": "Source language of captions."}
                },
                "required": ["enriched_segments"],
                "additionalProperties": false
            }
        },
        {
            "name": "sticker.auto_assign",
            "description": "Auto-place stickers/GIFs at segment positions in ONE CALL. Uses enriched_segments from sticker.validate_keywords (approved picks download DIRECTLY — no re-search) or sticker.keywords when provided; otherwise derives keywords from segment captions. Enforces a spacing gate (min_gap_s between placements) and position cycling ('auto' alternates top-right/bottom-right/center-left/bottom-left; an explicit position anchors all). Searches GIPHY, downloads, and places each sticker on the dedicated Stickers track (scale relative to canvas width) — the renderer composites them as positioned PiP overlays on top of the b-roll. ONE-CALL replacement for gif.search + gif.download + overlay.assign N times. Requires GIPHY_API_KEY. Returns: events_created count, positions, skipped (with reasons), timeline_path.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_path": {"type": "string", "description": "Path to timeline JSON with populated segments"},
                    "enriched_segments": {"anyOf": [{"type": "array", "items": {"type": "object"}}, {"type": "null"}], "description": "Output of sticker.validate_keywords (approved picks download directly) or sticker.keywords (keywords drive the search). Each segment: id, caption, sticker_keywords / approved + best_sticker."},
                    "sticker_query": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Override query for all stickers (e.g., 'funny', 'celebration'). If omitted, uses per-segment keywords."},
                    "position": {"type": "string", "default": "auto", "description": "Anchor position: 'auto' (default) cycles top-right/bottom-right/center-left/bottom-left for visual variety; explicit (top-right/top-left/bottom-right/bottom-left/center etc.) anchors every sticker there."},
                    "scale": {"type": "number", "default": 0.25, "description": "Sticker scale relative to canvas width (0.0-1.0)"},
                    "max_stickers": {"type": "integer", "default": 10, "description": "Maximum stickers to place"},
                    "min_gap_s": {"type": "number", "default": 2.0, "description": "Min seconds between consecutive sticker placements (spacing gate, prevents sticker spam)"}
                },
                "required": ["timeline_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "sticker.auto",
            "description": "ONE-CALL agentic sticker pipeline (parallel to broll.auto): segment.analyze → sticker.keywords (agentic intent+emphatic draft) → sticker.validate_keywords (GIPHY relevance gate — only genuinely relevant stickers approved) → download → place on the Stickers track with spacing + position cycling. Feed it an SRT + audio (or an existing timeline) and get back a timeline whose stickers layer is populated with RELEVANT, non-spammy stickers. Returns: timeline_path, segments_count, stickers_placed, skipped (with reasons), sticker_keywords_backend.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Existing timeline with segments to decorate (skips analyze)."},
                    "srt_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "SRT transcript (required unless timeline_path is given)."},
                    "audio_path": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Source audio/video (required unless timeline_path is given)."},
                    "language": {"type": "string", "default": "hinglish", "description": "Source language of captions."},
                    "position": {"type": "string", "default": "auto", "description": "Sticker anchor: 'auto' (default) cycles top-right/bottom-right/center-left/bottom-left; explicit anchors every sticker there."},
                    "scale": {"type": "number", "default": 0.25, "description": "Sticker scale relative to canvas width."},
                    "max_stickers": {"type": "integer", "default": 12, "description": "Maximum stickers to place."},
                    "min_gap_s": {"type": "number", "default": 2.0, "description": "Min seconds between consecutive sticker placements (spacing gate)."}
                },
                "required": [],
                "additionalProperties": false
            }
        },
        {
            "name": "timeline.to_hyperframes",
            "description": "Compile an EDL v2 timeline JSON into a HyperFrames HTML composition. Wraps the edl_v2_to_html.ts compiler — produces an index.html with GSAP timeline animations, video layers, and b-roll crossfades. After this, call composition.render or hf.render to produce the final MP4. This is the bridge between the NLE timeline and the HyperFrames motion-graphics render engine.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeline_path": {"type": "string", "description": "Path to the EDL v2 timeline JSON file"},
                    "output_dir": {"type": "string", "default": "artifacts/hf_composition", "description": "Directory to write the HF composition (index.html will be created here)"},
                    "composition_id": {"type": "string", "description": "Optional composition ID (default: auto-generated from timeline name)"}
                },
                "required": ["timeline_path"],
                "additionalProperties": false
            }
        },
        {
            "name": "voices.list",
            "description": "List all available TTS voices. Returns registered voice profiles from voices.json (named profiles with descriptions) plus the full list of Kokoro preset voice IDs (e.g. af_heart, am_michael, bf_emma) that can be used directly with script.generate_voices or tts.generate without registration. Use this to discover available voices before generating TTS.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "language": {"type": "string", "description": "Optional: filter by language code (e.g. 'en', 'es', 'fr', 'hi', 'it', 'ja', 'pt', 'zh')"}
                },
                "additionalProperties": false
            }
        },
        {
            "name": "library.search",
            "description": "Search the de-duplicated music/SFX library index. Index contains 500+ entries from NoCopyrightSounds, AudioLibrary, BreakingCopyright, VlogNoCopyrightMusic, MixtureOfficial, SoundLibrary1, and local stock. Each entry has filename, title, tags, download_url, source, duration_s, license. Use library.download to fetch the audio file on demand. Use library.build to rebuild the index from YouTube channels. Supports filtering by source channel, license, duration range, and tag.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search query (e.g. 'epic cinematic', 'lofi chill', 'explosion', 'whoosh')"},
                    "type": {"type": "string", "enum": ["music", "sfx"], "description": "Filter by media type: music or sfx"},
                    "source": {"type": "string", "description": "Filter by source channel (e.g. 'NoCopyrightSounds', 'AudioLibrary', 'BreakingCopyright', 'VlogNoCopyrightMusic', 'MixtureOfficial', 'SoundLibrary1')"},
                    "license": {"type": "string", "description": "Filter by license (e.g. 'no-copyright', 'creative-commons')"},
                    "min_duration_s": {"type": "number", "description": "Minimum duration in seconds (inclusive)"},
                    "max_duration_s": {"type": "number", "description": "Maximum duration in seconds (inclusive)"},
                    "tag": {"type": "string", "description": "Filter by tag (substring match against entry's tags array, case-insensitive)"},
                    "mood": {"type": "string", "enum": ["calm", "energetic", "upbeat", "dramatic", "dark", "sad", "neutral"], "description": "Filter by mood (derived from genre/title analysis)"},
                    "energy": {"type": "string", "enum": ["low", "medium", "high"], "description": "Filter by energy level"},
                    "limit": {"type": "integer", "default": 10, "description": "Max results"}
                },
                "required": ["query"],
                "additionalProperties": false
            }
        },
        {
            "name": "library.download",
            "description": "Download a music/SFX file from the library index on demand. Uses yt-dlp to extract audio as MP3 from YouTube sources. Caches downloaded files for reuse. Use library.search first to find the filename, then library.download to fetch it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "filename": {"type": "string", "description": "Filename from library.search results (e.g. 'Epic_Cinematic_Action_by_Infraction.mp3')"},
                    "output_dir": {"type": "string", "default": "mcp/assets/music_cache", "description": "Directory for downloaded files"}
                },
                "required": ["filename"],
                "additionalProperties": false
            }
        },
        {
            "name": "library.build",
            "description": "Rebuild the music/SFX library index by scraping YouTube channels (NoCopyrightSounds, AudioLibrary, BreakingCopyright, VlogNoCopyrightMusic, MixtureOfficial, SoundLibrary1). Run once at setup or when you want to refresh the library. Takes ~2 minutes. Returns index stats.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }
        },
        // ===================================================================
        // META-TOOLS — discovery and capability introspection
        // ===================================================================
        {
            "name": "system.capabilities",
            "description": "Check which OpenScript subsystems are available BEFORE calling other tools. Returns availability status for: voicebox/TTS, Pexels API, GIPHY API, Pixabay API, SFX library, music library, transcription engine, Kokoro TTS, HyperFrames. Use this first when you're unsure which features are wired — avoids opaque failures from tools whose backing service is missing. Example: if voicebox.available is false, skip tts.generate and use Kokoro (script.generate_voices) instead. No arguments.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }
        },
        {
            "name": "system.doctor",
            "description": "Cold-start production readiness report. Checks ffmpeg/yt-dlp, API keys (Pexels/GIPHY), portable music_production pack, music_library_index, SFX pack, Kokoro models. Returns ready_for_production (bool), checklist, and next_actions. Prefer this over system.capabilities when deciding whether director.run will ship real stock. No arguments.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }
        },
        {
            "name": "help.tool",
            "description": "Find relevant MCP tools for a natural-language task description. Returns ranked tool suggestions with name, relevance score (0.0-1.0), and a short description. Example queries: 'add voiceover to a timeline', 'download background music', 'burn captions into video', 'transcribe Hindi audio'. Use this when you know WHAT you want to do but not WHICH tool to call. Returns up to 8 suggestions.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Natural-language description of the task you want to accomplish"},
                    "limit": {"type": "integer", "description": "Max suggestions to return (default 8, max 20)", "default": 8}
                },
                "required": ["query"],
                "additionalProperties": false
            }
        }
    ]);

    // Append HyperFrames tools (hf.*)
    if let Some(arr) = tools.as_array_mut() {
        arr.extend(crate::hf::tool_definitions());
    }

    tools
}

// ---------------------------------------------------------------------------
// Tool routing
// ---------------------------------------------------------------------------

pub fn route_tool(
    name: &str,
    args: serde_json::Value,
) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, ToolError>> + Send + '_>> {
    let name_owned = name.to_string();
    match name_owned.as_str() {
        "transcribe" => Box::pin(handle_transcribe(args)),
        "srt.read" => Box::pin(handle_srt_read(args)),
        "srt.prepare" => Box::pin(handle_srt_prepare(args)),
        "captions.generate_ass" => Box::pin(handle_captions_generate_ass(args)),
        "srt.apply_edit" => Box::pin(handle_srt_apply_edit(args)),
        "edl.build" => Box::pin(handle_edl_build(args)),
        "render" => Box::pin(handle_render(args)),
        "reelize" => Box::pin(handle_reelize(args)),
        "reelize.brief" => Box::pin(handle_reelize_brief(args)),
        "reelize.direct" => Box::pin(handle_reelize_direct(args)),
        "overlay.generate" => Box::pin(handle_overlay_generate(args)),
        "timeline.build" => Box::pin(handle_timeline_build(args)),
        "timeline.load" => Box::pin(handle_timeline_load(args)),
        "timeline.validate" => Box::pin(handle_timeline_validate(args)),
        "srt.to_timeline" => Box::pin(handle_srt_to_timeline(args)),
        "timeline.upgrade" => Box::pin(handle_timeline_upgrade(args)),
        "timeline.add_segment" => Box::pin(handle_timeline_add_segment(args)),
        "timeline.add_track_event" => Box::pin(handle_timeline_add_track_event(args)),
        "voice.profile.add" => Box::pin(handle_voice_profile_add(args)),
        "voice.profile.list" => Box::pin(handle_voice_profile_list(args)),
        "voice.profile.remove" => Box::pin(handle_voice_profile_remove(args)),
        "voice.design" => Box::pin(handle_voice_design(args)),
        "character.create" => Box::pin(handle_character_create(args)),
        "character.design_emotion" => Box::pin(handle_character_design_emotion(args)),
        "character.list" => Box::pin(handle_character_list(args)),
        "character.remove" => Box::pin(handle_character_remove(args)),
        "tts.generate" => Box::pin(handle_tts_generate(args)),
        "tts.estimate_duration" => Box::pin(handle_tts_estimate_duration(args)),
        "sfx.index" => Box::pin(handle_sfx_index(args)),
        "sfx.search" => Box::pin(handle_sfx_search(args)),
        "sfx.assign" => Box::pin(handle_sfx_assign(args)),
        "sfx.auto_assign" => Box::pin(handle_sfx_auto_assign(args)),
        "music.index" => Box::pin(handle_music_index(args)),
        "music.search" => Box::pin(handle_music_search(args)),
        "music.assign" => Box::pin(handle_music_assign(args)),
        "broll.suggest" => Box::pin(handle_broll_suggest(args)),
        "broll.fetch" => Box::pin(handle_broll_fetch(args)),
        "broll.assign" => Box::pin(handle_broll_assign(args)),        "broll.plan" => Box::pin(handle_broll_plan(args)),
        "broll.keywords" => Box::pin(handle_broll_keywords(args)),
        "broll.validate_keywords" => Box::pin(handle_broll_validate_keywords(args)),
        "broll.repair" => Box::pin(handle_broll_repair(args)),
        "broll.auto" => Box::pin(handle_broll_auto(args)),
        "broll.probe" => Box::pin(handle_broll_probe(args)),
        "asset.library.status" => Box::pin(handle_asset_library_status(args)),
        "asset.ingest" => Box::pin(handle_asset_ingest(args)),
        "asset.probe" => Box::pin(handle_asset_probe(args)),
        "asset.rate" => Box::pin(handle_asset_rate(args)),
        "asset.import" => Box::pin(handle_asset_import(args)),
        "asset.search" => Box::pin(handle_asset_search(args)),
        "segment.analyze" => Box::pin(handle_segment_analyze(args)),
        "voiceover.generate" => Box::pin(handle_voiceover_generate(args)),
        "tts.commentary" => Box::pin(handle_tts_commentary(args)),
        "timeline.diff" => Box::pin(handle_timeline_diff(args)),
        "timeline.preview" => Box::pin(handle_timeline_preview(args)),
        "tts.preview" => Box::pin(handle_tts_preview(args)),
        "music.ducking.plan" => Box::pin(handle_music_ducking_plan(args)),
        "timeline.autofill_broll" => Box::pin(handle_timeline_autofill_broll(args)),
        "timeline.render" => Box::pin(handle_timeline_render(args)),        "verify.audio" => Box::pin(handle_verify_audio(args)),
        "verify.captions" => Box::pin(handle_verify_captions(args)),
        "verify.render" => Box::pin(handle_verify_render(args)),
        "verify.production" => Box::pin(handle_verify_production(args)),
        "llm.complete" => Box::pin(handle_llm_complete(args)),
        "vision.analyze_clip" => Box::pin(handle_vision_analyze_clip(args)),
        "vision.score_clip" => Box::pin(handle_vision_score_clip(args)),
        "system.config.get" => Box::pin(handle_system_config_get(args)),
        "system.config.set" => Box::pin(handle_system_config_set(args)),
        "director.run" => Box::pin(handle_director_run(args)),
        // HyperFrames tools
        "hf.lint" => Box::pin(async move {
            crate::hf::handle_hf_lint(args)
                .await
                .map_err(|e| ToolError::Hf(e.to_string()))
        }),
        "hf.validate" => Box::pin(async move {
            crate::hf::handle_hf_validate(args)
                .await
                .map_err(|e| ToolError::Hf(e.to_string()))
        }),
        "hf.snapshot" => Box::pin(async move {
            crate::hf::handle_hf_snapshot(args)
                .await
                .map_err(|e| ToolError::Hf(e.to_string()))
        }),
        "hf.render" => Box::pin(async move {
            crate::hf::handle_hf_render(args)
                .await
                .map_err(|e| ToolError::Hf(e.to_string()))
        }),
        "hf.classify" => Box::pin(async move {
            crate::hf::handle_hf_classify(args)
                .await
                .map_err(|e| ToolError::Hf(e.to_string()))
        }),
        "composition.render" => Box::pin(async move {
            crate::hf::handle_composition_render(args)
                .await
                .map_err(|e| ToolError::Hf(e.to_string()))
        }),
        "script.schema" => Box::pin(handle_script_schema(args)),
        "script.parse" => Box::pin(handle_script_parse(args)),
        "script.generate_voices" => Box::pin(handle_script_generate_voices(args)),
        "script.build_captions" => Box::pin(handle_script_build_captions(args)),
        "background.fetch" => Box::pin(handle_background_fetch(args)),
        "background.assign" => Box::pin(handle_background_assign(args)),
        "background.search" => Box::pin(handle_background_search(args)),            "sticker.presets" => Box::pin(handle_sticker_presets(args)),
            "sticker.load_preset" => Box::pin(handle_sticker_load_preset(args)),
        "sticker.render" => Box::pin(handle_sticker_render(args)),
        "script.to_timeline" => Box::pin(handle_script_to_timeline(args)),
        "script.to_video" => Box::pin(handle_script_to_video(args)),
        "stock.fetch" => Box::pin(handle_stock_fetch(args)),
        "youtube.download" => Box::pin(handle_youtube_download(args)),
        "youtube.search" => Box::pin(handle_youtube_search(args)),
        "stock.search" => Box::pin(handle_stock_search(args)),
        "media.search" => Box::pin(handle_media_search(args)),
        "media.download" => Box::pin(handle_media_download(args)),
        "gif.search" => Box::pin(handle_gif_search(args)),
        "gif.download" => Box::pin(handle_gif_download(args)),
        "overlay.assign" => Box::pin(handle_overlay_assign(args)),
        "sticker.keywords" => Box::pin(handle_sticker_keywords(args)),
        "sticker.validate_keywords" => Box::pin(handle_sticker_validate_keywords(args)),
        "sticker.auto" => Box::pin(handle_sticker_auto(args)),
        "sticker.auto_assign" => Box::pin(handle_sticker_auto_assign(args)),
        "timeline.to_hyperframes" => Box::pin(handle_timeline_to_hyperframes(args)),
        "voices.list" => Box::pin(handle_voices_list(args)),
        "timeline.inspect" => Box::pin(handle_timeline_inspect(args)),
        "library.search" => Box::pin(handle_library_search(args)),
        "library.download" => Box::pin(handle_library_download(args)),
        "library.build" => Box::pin(handle_library_build(args)),
        // Meta-tools (P1-2 + Rec 3.1 from prior audit)
        "system.capabilities" => Box::pin(handle_system_capabilities(args)),
        "system.doctor" => Box::pin(handle_system_doctor(args)),
        "help.tool" => Box::pin(handle_help_tool(args)),
                _ => Box::pin(async move { Err(ToolError::UnknownTool(name_owned.clone())) }),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub(crate) fn extract_str<'a>(args: &'a serde_json::Value, key: &str) -> Result<&'a str, ToolError> {
    args.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::MissingArg(key.to_string()))
}

fn extract_f64(args: &serde_json::Value, key: &str) -> Result<f64, ToolError> {
    args.get(key)
        .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|n| n as f64)))
        .ok_or_else(|| ToolError::MissingArg(key.to_string()))
}

fn extract_i64(args: &serde_json::Value, key: &str) -> Result<i64, ToolError> {
    args.get(key)
        .and_then(|v| v.as_i64())
        .ok_or_else(|| ToolError::MissingArg(key.to_string()))
}

fn extract_arr(args: &serde_json::Value, key: &str) -> Result<Vec<String>, ToolError> {
    args.get(key)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .ok_or_else(|| ToolError::MissingArg(key.to_string()))
}

pub(crate) fn default_str(args: &serde_json::Value, key: &str, default: &str) -> String {
    args.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or(default)
        .to_string()
}

pub(crate) fn default_f64(args: &serde_json::Value, key: &str, default: f64) -> f64 {
    args.get(key).and_then(|v| v.as_f64()).unwrap_or(default)
}

fn default_u32(args: &serde_json::Value, key: &str, default: u32) -> u32 {
    args.get(key)
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .unwrap_or(default)
}

fn default_i64(args: &serde_json::Value, key: &str, default: i64) -> i64 {
    args.get(key).and_then(|v| v.as_i64()).unwrap_or(default)
}

pub(crate) fn default_bool(args: &serde_json::Value, key: &str, default: bool) -> bool {
    args.get(key).and_then(|v| v.as_bool()).unwrap_or(default)
}

/// Optional boolean: `None` when the key is absent so callers can omit filters.
/// Explicit `true`/`false` are preserved. Used by music NLE tools so default
/// omission does not mean "only tracks with flag=false".
#[allow(dead_code)] // only used in tests after music.search deprecation
fn default_opt_bool(args: &serde_json::Value, key: &str) -> Option<bool> {
    args.get(key).and_then(|v| v.as_bool())
}

/// Extract a meaningful b-roll concept from a caption string.
/// Skips stopwords and short words to avoid garbage Pexels searches like
/// "The" or "But". Falls back to the first 2 significant words.


fn default_opt_str(args: &serde_json::Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from)
}

fn default_opt_i64(args: &serde_json::Value, key: &str) -> Option<i64> {
    args.get(key).and_then(|v| v.as_i64())
}

fn default_opt_u32(args: &serde_json::Value, key: &str) -> Option<u32> {
    args.get(key).and_then(|v| v.as_u64()).map(|v| v as u32)
}

fn default_opt_f64(args: &serde_json::Value, key: &str) -> Option<f64> {
    args.get(key).and_then(|v| v.as_f64())
}

fn track_count(timeline: &Timeline, track_type: &TrackType) -> usize {
    timeline
        .tracks
        .get(track_type)
        .map(|v: &Vec<openscript_core::timeline::TimelineEvent>| v.len())
        .unwrap_or(0)
}

/// Read an API key via the unified config cascade:
/// env → `~/.openscript/config.json` → legacy `mcp/assets/.openscript_config.json`.
fn get_api_key(config_name: &str, _env_name: &str) -> String {
    // Map legacy flat names + nested kinds onto config::resolve_api_key
    let kind = match config_name {
        "pexels_api_key" | "pexels" => "pexels",
        "giphy_api_key" | "giphy" => "giphy",
        "pixabay_api_key" | "pixabay" => "pixabay",
        "openrouter_api_key" | "openrouter" | "openrouter_key" => "openrouter",
        other => other,
    };
    crate::config::resolve_api_key(kind)
}

/// Convenience: get Pexels API key (config file or env var)
pub(crate) fn pexels_key() -> String {
    get_api_key("pexels_api_key", "PEXELS_API_KEY")
}

/// Convenience: get GIPHY API key (config file or env var)
fn giphy_key() -> String {
    get_api_key("giphy_api_key", "GIPHY_API_KEY")
}

/// Convenience: get Pixabay API key (config file or env var)
pub(crate) fn pixabay_key() -> String {
    get_api_key("pixabay_api_key", "PIXABAY_API_KEY")
}

/// Convert an aspect-ratio string ("9:16", "16:9", "1:1") to a (width, height)
/// pixel tuple. Used by every tool that needs to crop/scale stock footage to
/// the target aspect. Prior versions had 7+ duplicate match blocks for this;
/// consolidated into one helper.
fn aspect_to_crop_dims(aspect: &str) -> (u32, u32) {
    match aspect {
        "9:16" => (1080, 1920), // vertical
        "16:9" => (1920, 1080), // horizontal
        "1:1" => (1080, 1080),  // square
        _ => (1080, 1920),      // unknown → portrait default
    }
}

/// Convert an aspect-ratio string to a Pexels orientation keyword.
pub(crate) fn aspect_to_orientation(aspect: &str) -> &'static str {
    match aspect {
        "9:16" => "portrait",
        "16:9" => "landscape",
        "1:1" => "square",
        _ => "portrait",
    }
}

/// Build a Pexels video search URL with optional duration filters.
///
/// Implements the SEGMENTATION_ARCHITECTURE.md clip-duration matching: when
/// `min_duration_s > 0` the API only returns clips at least that long, so a
/// scene's clip never needs looping — the caller keeps fetching ALTERNATE
/// stock videos (via `page`) for the same keywords until one covers the
/// required duration. `max_duration_s > 0` caps the upper bound.
pub(crate) fn pexels_search_url(
    query: &str,
    orientation: &str,
    page: i64,
    min_duration_s: f64,
    max_duration_s: f64,
) -> String {
    let mut url = format!(
        "https://api.pexels.com/videos/search?query={}&per_page=15&orientation={}&page={}",
        urlencoding::encode(query),
        orientation,
        page
    );
    // floor (not ceil) for the API filter: a clip that EXACTLY covers the
    // scene (e.g. 11.9s for an 11.89s scene) must stay in the candidate pool;
    // the caller's strict float check `(vid_dur as f64) >= needed` decides
    // coverage, so the API filter only narrows, never excludes a valid cover.
    if min_duration_s > 0.0 {
        url.push_str(&format!(
            "&min_duration={}",
            min_duration_s.floor() as i64
        ));
    }
    if max_duration_s > 0.0 {
        url.push_str(&format!(
            "&max_duration={}",
            max_duration_s.floor() as i64
        ));
    }
    url
}

/// Find the best download URL (720-1920px width file) for a Pexels video JSON
/// object, or None when no suitable file exists. Shared by the script.to_video
/// pass 1 (covering) and pass 2 (loop fallback) selection loops.
pub(crate) fn pexels_file_url(video: &serde_json::Value) -> Option<String> {
    video
        .get("video_files")
        .and_then(|v| v.as_array())
        .and_then(|files| {
            files.iter().find(|f| {
                let width = f.get("width").and_then(|v| v.as_u64()).unwrap_or(0);
                let u = f.get("link").and_then(|v| v.as_str()).unwrap_or("");
                (720..=1920).contains(&width) && !u.is_empty()
            })
        })
        .and_then(|f| f.get("link").and_then(|v| v.as_str()))
        .map(|s| s.to_string())
}

/// Build an ffmpeg `-vf` filter that **cover-crops** to the target aspect
/// without stretching, and forces square pixels (`setsar=1`).
///
/// Prior `scale=W:H,crop=W:H` stretched landscape stock into 9:16 frames with
/// non-square SAR (display aspect stayed ~16:9) — visible distortion.
/// Delegates to `stock_signal::cover_crop_filter_for_aspect`.
fn crop_filter_for_aspect(aspect: &str) -> String {
    crate::stock_signal::cover_crop_filter_for_aspect(aspect)
}

/// Build the ffmpeg command that cover-crops + re-encodes a downloaded stock
/// clip into the render-ready trim (Pexels / YouTube / Pixabay tiers).
///
/// GPU-aware: when `OPENSCRIPT_FFMPEG_GPU` resolves to NVENC/NVDEC (default
/// `auto`), the decode is CUDA-accelerated and the intermediate re-encode uses
/// `h264_nvenc`. This is where the expensive aspect-ratio upscale actually
/// happens — measured ~1.45x faster than `libx264 -preset fast` on the dev
/// box, and it makes the render-stage `scale` a pass-through (why GPU filter
/// graphs buy nothing downstream). Single-frame thumbnail extraction is
/// deliberately NOT GPU-accelerated anywhere: CUDA context init makes GPU
/// *slower* for one-frame grabs (measured ~2x).
///
/// `start_s` is applied as an input seek (`-ss` before `-i`) for fast seeks;
/// pass `None` for clips that should start at 0 (Pexels/Pixabay stock loops).
pub(crate) fn build_stock_trim_command(
    gpu: &GpuConfig,
    input: &str,
    output: &str,
    duration_s: f64,
    start_s: Option<f64>,
    crop_filter: &str,
) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("ffmpeg");
    cmd.arg("-y");
    if let Some(s) = start_s {
        cmd.arg("-ss").arg(s.to_string());
    }
    gpu.add_input(&mut cmd);
    cmd.arg("-i")
        .arg(input)
        .arg("-t")
        .arg(duration_s.to_string())
        .arg("-vf")
        .arg(crop_filter);
    // Intermediates are 30fps yuv420p to match the render path's frame-count
    // assumptions (select='lte(n,K)' on trims assumes 30fps input).
    gpu.add_encoder(&mut cmd, "fast", 23, 30, false);
    cmd.arg("-an")
        .arg(output)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    cmd
}

/// Last non-empty ffmpeg stderr lines, for the "why did this trim fail"
/// diagnostic that feeds the scene-fall-to-procedural audit trail.
pub(crate) fn trim_stderr_tail(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .join(" | ")
}

/// Resolve a repo-relative path CWD-independently.
///
/// Priority:
///   1. If the path is absolute, return as-is.
///   2. If the path exists relative to CWD, return the CWD-relative path.
///   3. If OPENSCRIPT_ROOT env var is set, try `OPENSCRIPT_ROOT/path`.
///   4. Fall back to `CARGO_MANIFEST_DIR/../../path` (compile-time workspace root).
///   5. Last resort: return the relative path as-is (will likely fail downstream).
///
/// This fixes the round-2 UX audit GAP #12: background.search and other
/// asset-index tools only worked when CWD was the repo root because they
/// used relative paths like "mcp/assets/backgrounds_index.json". Now they
/// work from any CWD.
pub fn resolve_repo_path(rel: &str) -> std::path::PathBuf {
    let p = std::path::Path::new(rel);
    if p.is_absolute() {
        return p.to_path_buf();
    }
    // Try CWD-relative first (fast path)
    if p.exists() {
        return p.to_path_buf();
    }
    // Try OPENSCRIPT_ROOT
    if let Ok(root) = std::env::var("OPENSCRIPT_ROOT") {
        let resolved = std::path::Path::new(&root).join(rel);
        if resolved.exists() {
            return resolved;
        }
    }
    // Try CARGO_MANIFEST_DIR (compile-time workspace path)
    if let Some(d) = option_env!("CARGO_MANIFEST_DIR") {
        let resolved = std::path::Path::new(d).join("../../").join(rel);
        if resolved.exists() {
            return resolved;
        }
    }
    // Last resort: return the relative path (will likely fail, but with
    // a clear error downstream rather than a misleading "re-clone" message)
    p.to_path_buf()
}

fn sanitize_input_path<P: AsRef<std::path::Path>>(
    path: P,
) -> Result<std::path::PathBuf, ToolError> {
    let path = path.as_ref();
    for component in path.components() {
        if let std::path::Component::ParentDir = component {
            return Err(ToolError::InvalidArg(format!(
                "Path traversal not allowed: {}",
                path.display()
            )));
        }
    }

    let resolved = if path.exists() {
        path.canonicalize()
            .map_err(|e| ToolError::InvalidArg(format!("Cannot resolve path: {}", e)))?
    } else {
        path.to_path_buf()
    };

    // If OPENSCRIPT_WORKSPACE_ROOT is set, reject paths that resolve outside it.
    // This is a defense-in-depth measure — the MCP server trusts the agent by default,
    // but operators can opt into workspace confinement via this env var.
    if let Ok(workspace_root) = std::env::var("OPENSCRIPT_WORKSPACE_ROOT") {
        let root = std::path::Path::new(&workspace_root)
            .canonicalize()
            .map_err(|e| {
                ToolError::InvalidArg(format!("Invalid OPENSCRIPT_WORKSPACE_ROOT: {}", e))
            })?;
        if !resolved.starts_with(&root) {
            return Err(ToolError::Permission(format!(
                "Path '{}' resolves outside workspace root '{}'. Set OPENSCRIPT_WORKSPACE_ROOT to allow this path, or remove the env var to disable workspace confinement.",
                resolved.display(),
                root.display()
            )));
        }
    }

    Ok(resolved)
}

fn default_opt_arr(args: &serde_json::Value, key: &str) -> Option<Vec<String>> {
    args.get(key).and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect()
    })
}

fn default_timeline_path(source_video: &str) -> String {
    let path = Path::new(source_video);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default();
    format!("{}.timeline.json", stem)
}

fn voice_profiles_path() -> String {
    // Explicit env override first (AGENTS.md §9) — also lets integration
    // tests point the registry at a known location regardless of CWD.
    std::env::var("OPENSCRIPT_VOICE_PROFILES_PATH")
        .ok()
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| ".openscript/voice_profiles.json".to_string())
}

fn load_voice_profiles() -> Result<serde_json::Value, ToolError> {
    let path = voice_profiles_path();
    if Path::new(&path).exists() {
        let data = std::fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&data)?)
    } else {
        Ok(json!({}))
    }
}

fn save_voice_profiles(profiles: &serde_json::Value) -> Result<(), ToolError> {
    atomic_write_json(&voice_profiles_path(), profiles)
}

/// Atomically write a JSON value to `path`: write to `{path}.tmp` then rename
/// over the target. Readers that don't take the RegistryLock (list tools,
/// read-only validation phases) can therefore never observe a partially
/// written/truncated file — the rename is atomic on POSIX. Also crash-safe.
fn atomic_write_json(path: &str, value: &serde_json::Value) -> Result<(), ToolError> {
    if let Some(parent) = Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = format!("{}.tmp", path);
    let data = serde_json::to_string_pretty(value)?;
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Cross-process advisory lock for the JSON registries
/// (`.openscript/voice_profiles.json`, `.openscript/characters.json`).
///
/// Guards read-modify-write cycles that can run concurrently from multiple
/// MCP server processes (Tauri app, CLI, parallel agent sessions). Without a
/// lock, two parallel `character.design_emotion` / `voice.profile.add` calls
/// silently lose updates (last writer wins — the observed `firm` take loss).
///
/// Implemented as a `create_new`-only lockfile `<target>.lock` released by
/// deletion on Drop. No new dependency (no fs2 in the tree).
pub(crate) struct RegistryLock {
    path: std::path::PathBuf,
}

impl RegistryLock {
    /// Acquire the lock for `target` (a registry file path). Blocks up to 20s
    /// retrying; steals stale locks older than 60s (crashed process).
    pub(crate) fn acquire(target: &Path) -> Result<Self, ToolError> {
        let lock_path = std::path::PathBuf::from(format!("{}.lock", target.display()));
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock_path)
            {
                Ok(_) => {
                    // Write the PID so stale locks are debuggable.
                    let _ = std::fs::write(
                        &lock_path,
                        format!("pid={}", std::process::id()),
                    );
                    return Ok(RegistryLock { path: lock_path });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    // Stale lock (crashed holder): steal if older than 60s.
                    let stale = std::fs::metadata(&lock_path)
                        .and_then(|m| m.modified())
                        .map(|t| {
                            t.elapsed()
                                .map(|d| d > std::time::Duration::from_secs(60))
                                .unwrap_or(false)
                        })
                        .unwrap_or(false);
                    if stale {
                        let _ = std::fs::remove_file(&lock_path);
                        continue;
                    }
                    if std::time::Instant::now() > deadline {
                        return Err(ToolError::Io(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            format!(
                                "timed out acquiring registry lock {}",
                                lock_path.display()
                            ),
                        )));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => {
                    return Err(ToolError::Io(std::io::Error::new(
                        e.kind(),
                        format!(
                            "failed to acquire registry lock {}: {}",
                            lock_path.display(),
                            e
                        ),
                    )));
                }
            }
        }
    }
}

impl Drop for RegistryLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

// ---------------------------------------------------------------------------
// Handler: transcribe (native via openscript-transcribe)
// ---------------------------------------------------------------------------



// ---------------------------------------------------------------------------
// Handler: srt.read
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: srt.prepare
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: srt.apply_edit (native: parse edited SRT, build EDL, render)
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: edl.build (Native Rust)
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: render (Phase 1: shell to Python)
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: reelize (native: transcribe → prepare → edl.build → render)
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: overlay.generate (Phase 1: shell to Python)
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: timeline.build
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: timeline.load
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: timeline.validate
// ---------------------------------------------------------------------------



// ---------------------------------------------------------------------------
// Handler: timeline.upgrade
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: timeline.add_segment
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: timeline.add_track_event
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: voice.profile.add
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: voice.profile.list
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: voice.profile.remove
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Shared TTS routing helper — used by tts.generate, voiceover.generate, tts.commentary
// Routes to Kokoro (if profile.provider == "kokoro" and feature enabled) or sidecar.
// ---------------------------------------------------------------------------

struct TtsGenResult {
    output_path: String,
    duration_ms: i64,
    cached: bool,
    backend: String,
}

/// Generate speech via the appropriate TTS backend (Kokoro or sidecar).
/// `output_path` must be pre-validated (parent dir created).
/// Resolve the emotion take for a profile + scene emote. Returns the take
/// only when the profile registered one for this emotion id — otherwise
/// None (synthesize with the neutral base reference).
fn resolve_emotion_take<'p>(
    profile: &'p openscript_tts::profiles::VoiceProfile,
    emotion: Option<&str>,
) -> Option<&'p openscript_tts::profiles::EmotionTake> {
    match emotion {
        Some(e) if !e.is_empty() => profile.emotions.get(e),
        _ => None,
    }
}

/// Build the ffmpeg `-af` filter graph for speed/pitch post-processing.
/// Pure function so it is unit-testable. Uses the universally-available
/// asetrate/aresample/atempo chain:
///   pitch: asetrate=R*P,aresample=R,atempo=1/P  → pitch shifted, duration kept
///   speed: atempo (chained past the 0.5–2.0 per-instance range)
/// Returns an empty string when both are 1.0 (no-op).
pub(crate) fn build_speed_pitch_filter(speed: f64, pitch: f64, sample_rate: u32) -> String {
    let mut parts: Vec<String> = Vec::new();
    if (pitch - 1.0).abs() > 1e-6 && pitch > 0.0 {
        parts.push(format!(
            "asetrate={}*{:.6},aresample={},atempo={:.6}",
            sample_rate, pitch, sample_rate, 1.0 / pitch
        ));
    }
    if (speed - 1.0).abs() > 1e-6 && speed > 0.0 {
        let mut f = speed;
        let mut chain: Vec<String> = Vec::new();
        while f > 2.0 {
            chain.push("atempo=2.0".to_string());
            f /= 2.0;
        }
        while f < 0.5 {
            chain.push("atempo=0.5".to_string());
            f /= 0.5;
        }
        chain.push(format!("atempo={:.6}", f));
        parts.push(chain.join(","));
    }
    parts.join(",")
}

/// Probe the first audio stream's sample rate via ffprobe (best-effort;
/// falls back to 44100).
fn probe_sample_rate(path: &str) -> u32 {
    let out = std::process::Command::new("ffprobe")
        .args([
            "-v", "error", "-select_streams", "a:0",
            "-show_entries", "stream=sample_rate", "-of", "csv=p=0", path,
        ])
        .output();
    if let Ok(o) = out {
        if let Ok(s) = String::from_utf8(o.stdout) {
            if let Ok(rate) = s.trim().parse::<u32>() {
                return rate;
            }
        }
    }
    44100
}

/// Apply speed/pitch to an audio file in place (atomically via temp + rename).
/// Returns the new duration_ms (atempo math is exact: duration /= speed).
/// Non-fatal callers may ignore the error and keep the unprocessed audio.
pub(crate) fn apply_speed_pitch(path: &str, speed: f64, pitch: f64) -> Result<i64, String> {
    let filter = build_speed_pitch_filter(speed, pitch, probe_sample_rate(path));
    if filter.is_empty() {
        let dur = probe_audio_duration_ms(path);
        return Ok(dur);
    }
    let tmp = format!("{}.spstmp.wav", path);
    let status = std::process::Command::new("ffmpeg")
        .args(["-y", "-v", "error", "-i", path, "-af", &filter, &tmp])
        .status()
        .map_err(|e| format!("ffmpeg spawn failed for speed/pitch: {}", e))?;
    if !status.success() {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("ffmpeg speed/pitch post-processing failed for {}", path));
    }
    std::fs::rename(&tmp, path).map_err(|e| format!("rename {}: {}", tmp, e))?;
    Ok(probe_audio_duration_ms(path))
}

/// Probe audio duration in ms via ffprobe (best-effort).
fn probe_audio_duration_ms(path: &str) -> i64 {
    let out = std::process::Command::new("ffprobe")
        .args([
            "-v", "error", "-show_entries", "format=duration",
            "-of", "csv=p=0", path,
        ])
        .output();
    if let Ok(o) = out {
        if let Ok(s) = String::from_utf8(o.stdout) {
            if let Ok(secs) = s.trim().parse::<f64>() {
                return (secs * 1000.0).round() as i64;
            }
        }
    }
    0
}

/// Build the Qwen3 VoiceDesign `instruct` for one scene line: the character's
/// personality (base voice identity) + the scene's emotional-take instruct +
/// any natural-language `tone` direction. Reading the character schema
/// (`.openscript/characters.json`) keeps the SAME voice across scenes while
/// each line is attuned to its required delivery at synthesis time. Falls
/// back to the profile description (the persona stored by voice.design) when
/// no character entry exists.
fn build_voicedesign_instruct(
    profile: &openscript_tts::profiles::VoiceProfile,
    emotion: Option<&str>,
    tone: Option<&str>,
) -> String {
    let chars_path = std::env::var("OPENSCRIPT_CHARACTERS_PATH")
        .unwrap_or_else(|_| ".openscript/characters.json".to_string());
    let mut base = String::new();
    let mut emotion_instruct: Option<String> = None;
    if let Ok(data) = std::fs::read_to_string(&chars_path) {
        if let Ok(chars) = serde_json::from_str::<serde_json::Value>(&data) {
            if let Some(c) = chars.get(&profile.id) {
                if let Some(p) = c.get("personality").and_then(|v| v.as_str()) {
                    base = p.trim().to_string();
                }
                if let Some(em) = emotion {
                    if let Some(ei) = c
                        .get("emotions")
                        .and_then(|m| m.get(em))
                        .and_then(|t| t.get("instruct"))
                        .and_then(|v| v.as_str())
                    {
                        if !ei.trim().is_empty() {
                            emotion_instruct = Some(ei.trim().to_string());
                        }
                    }
                }
            }
        }
    }
    if base.is_empty() {
        // Fall back to the persona stored on the profile (voice.design writes
        // "voice.design persona: <instruct>"; character.create writes
        // "character base voice: <personality>").
        if let Some(d) = profile.description.as_deref() {
            base = d
                .strip_prefix("character base voice: ")
                .or_else(|| d.strip_prefix("voice.design persona: "))
                .unwrap_or(d)
                .trim()
                .to_string();
        }
    }
    let mut parts: Vec<String> = Vec::new();
    if !base.is_empty() {
        parts.push(base);
    }
    match emotion_instruct {
        Some(ei) if !ei.is_empty() => parts.push(ei),
        _ => {
            if let Some(em) = emotion {
                if !em.trim().is_empty() && em.trim() != "neutral" {
                    parts.push(format!("{} delivery", em.trim()));
                }
            }
        }
    }
    if let Some(t) = tone {
        if !t.trim().is_empty() {
            parts.push(t.trim().to_string());
        }
    }
    let instruct = parts.join(". ");
    if instruct.trim().is_empty() {
        "calm, clear, natural speech".to_string()
    } else {
        instruct
    }
}

async fn tts_generate_routed(
    voice_profile_id: &str,
    text: &str,
    output_path: &str,
    speed: f64,
    pitch: f64,
    volume: f64,
    format: &str,
    emotion: Option<&str>,
    tone: Option<&str>,
    temperature: Option<f64>,
    top_k: Option<u32>,
    top_p: Option<f64>,
    cfg_scale: Option<f64>,
    profile: &openscript_tts::profiles::VoiceProfile,
) -> Result<TtsGenResult, ToolError> {
    // Natural-language delivery direction — consumed here as a diagnostic so
    // it is not dead schema: logged per line and ready to feed any engine that
    // gains an instruction channel (VoiceDesign at design-time today; the
    // emotion-take mechanism carries the tonality at synth-time).
    if let Some(t) = tone {
        if !t.trim().is_empty() {
            tracing::info!(
                "[tts] tone direction for '{}': {}",
                voice_profile_id,
                t.trim()
            );
        }
    }
    let cache_dir =
        std::env::var("OPENSCRIPT_TTS_CACHE").unwrap_or_else(|_| "artifacts/tts".to_string());

    // Route to Kokoro backend if the profile's provider is "kokoro" and the
    // feature is enabled. Otherwise fall through to the sidecar.
    #[cfg(feature = "kokoro")]
    if profile.provider == "kokoro" {
        use openscript_tts::kokoro::{KokoroClient, KokoroConfig};

        let model_dir =
            std::env::var("KOKORO_MODEL_DIR").unwrap_or_else(|_| "mcp/assets/kokoro".to_string());
        let default_voice =
            std::env::var("KOKORO_DEFAULT_VOICE").unwrap_or_else(|_| "af_heart".to_string());

        let cfg = KokoroConfig {
            model_dir: std::path::PathBuf::from(&model_dir),
            default_voice,
            cache_dir: std::path::PathBuf::from(&cache_dir),
        };
        let kokoro_client = KokoroClient::new(cfg);

        let result = kokoro_client
            .generate(
                voice_profile_id,
                text,
                output_path,
                speed,
                pitch,
                volume,
                format,
                profile,
            )
            .await
            .map_err(|e| ToolError::Tts(e.to_string()))?;

        return Ok(TtsGenResult {
            output_path: result.output_path,
            duration_ms: result.duration_ms,
            cached: result.cached,
            backend: "kokoro".to_string(),
        });
    }

    #[cfg(not(feature = "kokoro"))]
    if profile.provider == "kokoro" {
        return Err(ToolError::Tts(
            "Voice profile uses Kokoro backend but the kokoro feature is not enabled. \
             Rebuild openscript-mcp with --features kokoro."
                .to_string(),
        ));
    }

    // Gepard path (high-quality native-English voice cloning — Qwen3.5 AR + NeMo
    // NanoCodec via the .venv-gepard sidecar; Apache-2.0 weights).
    if profile.provider == "gepard" {
        let take = resolve_emotion_take(profile, emotion);
        let params = openscript_tts::gepard::GepardSynthParams {
            emotion: emotion.map(|s| s.to_string()),
            ref_audio: take.map(|t| t.ref_audio.clone()),
            // Explicit request temperature/cfg_scale win; else the emotion
            // take's own cfg_scale; else None (engine default 0.7 / 1.0).
            cfg_scale: cfg_scale.or_else(|| take.and_then(|t| t.cfg_scale)),
            temperature,
            top_k,
            max_frames: None,
        };
        let (mut duration_ms, sample_rate) = openscript_tts::gepard::gepard_synthesize_params(
            text,
            &profile.id,
            output_path,
            &params,
        )
        .map_err(|e| ToolError::Tts(e))?;
        if take.is_some() {
            tracing::info!(
                "[tts] gepard emotion '{}' take for voice '{}' (ref={})",
                emotion.unwrap_or(""),
                profile.id,
                take.map(|t| t.ref_audio.as_str()).unwrap_or("")
            );
        }
        // Speed/pitch were previously SILENTLY DROPPED for clone engines;
        // apply them post-synthesis now (non-fatal on failure).
        if (speed - 1.0).abs() > 1e-6 || (pitch - 1.0).abs() > 1e-6 {
            match apply_speed_pitch(output_path, speed, pitch) {
                Ok(new_dur) if new_dur > 0 => duration_ms = new_dur,
                Ok(_) => {}
                Err(e) => tracing::warn!("[tts] gepard speed/pitch post-processing failed: {}", e),
            }
        }
        return Ok(TtsGenResult {
            output_path: output_path.to_string(),
            duration_ms,
            cached: false,
            backend: format!("gepard:{}hz", sample_rate),
        });
    }

    // Audio8 path (zero-shot voice cloning — default cloned-voice engine).
    // Emotion takes are pre-registered compound voices `{id}@{emotion}` at
    // voice.profile.add time; the take's presence switches the voice id.
    if profile.provider == "audio8" {
        let take = resolve_emotion_take(profile, emotion);
        let synth_voice = match take {
            Some(_) => format!("{}@{}", profile.id, emotion.unwrap_or("")),
            None => profile.id.clone(),
        };
        let params = openscript_tts::audio8::Audio8SynthParams {
            emotion: emotion.map(|s| s.to_string()),
            ref_audio: None,
            temperature,
            top_p,
            top_k,
            seed: None,
            max_new_tokens: None,
        };
        let (mut duration_ms, sample_rate) = openscript_tts::audio8::audio8_synthesize_params(
            text,
            &synth_voice,
            output_path,
            &params,
        )
        .map_err(|e| ToolError::Tts(e))?;
        if (speed - 1.0).abs() > 1e-6 || (pitch - 1.0).abs() > 1e-6 {
            match apply_speed_pitch(output_path, speed, pitch) {
                Ok(new_dur) if new_dur > 0 => duration_ms = new_dur,
                Ok(_) => {}
                Err(e) => tracing::warn!("[tts] audio8 speed/pitch post-processing failed: {}", e),
            }
        }
        return Ok(TtsGenResult {
            output_path: output_path.to_string(),
            duration_ms,
            cached: false,
            backend: format!("audio8:{}hz", sample_rate),
        });
    }

    // VoiceDesign path — Qwen3-TTS-1.7B-VoiceDesign ONNX int4, DIRECT
    // natural-language-instruction synthesis. The `instruct` is the character
    // personality + the scene's emotion/tone, so the same character voice
    // stays consistent while every line is attuned to its required delivery
    // BY THE VOICE-DESIGN MODEL itself. This is NOT cloning: gepard/audio8
    // never touch a voicedesign profile (their reference WAVs were only ever
    // design artifacts — the actual scene audio comes from Qwen3 here).
    if profile.provider == "voicedesign" {
        let instruct = build_voicedesign_instruct(profile, emotion, tone);
        tracing::info!(
            "[tts] voicedesign line for '{}': {}",
            profile.id,
            truncate_utf8(&instruct, 160)
        );
        // Stable per-character seed: Qwen3 VoiceDesign is zero-shot (the
        // instruct IS the voice), so a fresh RNG per line lets the timbre
        // drift scene-to-scene. Deriving the seed from the profile id locks
        // the voice across scenes while `temperature` still varies prosody.
        let char_seed = profile.id.bytes().fold(0i64, |acc, b| {
            acc.wrapping_mul(31).wrapping_add(i64::from(b))
        }) & 0x7fff_ffff;
        let (mut duration_ms, sample_rate, _written) =
            openscript_tts::voicedesign::voicedesign_synthesize(
                &instruct,
                text,
                output_path,
                &profile.language,
                Some(char_seed),
                None, // max_tokens — sidecar default (2048 ≈ 170 s @ 12 Hz codec)
                temperature,
                top_k,
            )
            .map_err(|e| ToolError::Tts(e))?;
        // Speed/pitch are post-processed exactly like the clone engines
        // (the Qwen3 pipeline has no native tempo/pitch knob).
        if (speed - 1.0).abs() > 1e-6 || (pitch - 1.0).abs() > 1e-6 {
            match apply_speed_pitch(output_path, speed, pitch) {
                Ok(new_dur) if new_dur > 0 => duration_ms = new_dur,
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!("[tts] voicedesign speed/pitch post-processing failed: {}", e)
                }
            }
        }
        return Ok(TtsGenResult {
            output_path: output_path.to_string(),
            duration_ms,
            cached: false,
            backend: format!("voicedesign:{}hz", sample_rate),
        });
    }

    // Sidecar path (faster-qwen3-tts)
    use openscript_tts::client::TtsClient;
    let tts_url = std::env::var("OPENSCRIPT_TTS_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:17493".to_string());
    let client = TtsClient::new(&tts_url, &cache_dir);

    if !client
        .health_check()
        .await
        .map_err(|e| ToolError::Tts(e.to_string()))?
    {
        return Err(ToolError::Tts(format!(
            "TTS sidecar server is not reachable at {}. \
             Start the faster-qwen3-tts server or set OPENSCRIPT_TTS_URL.",
            tts_url
        )));
    }

    let result = client
        .generate(
            voice_profile_id,
            text,
            output_path,
            speed,
            pitch,
            volume,
            format,
            profile,
        )
        .await
        .map_err(|e| ToolError::Tts(e.to_string()))?;

    Ok(TtsGenResult {
        output_path: result.output_path,
        duration_ms: result.duration_ms,
        cached: result.cached,
        backend: "sidecar".to_string(),
    })
}

// ---------------------------------------------------------------------------
// Handler: tts.generate (native via openscript-tts)
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: tts.estimate_duration
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: sfx.index (native via openscript-assets)
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: sfx.search (native via openscript-assets)
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: sfx.assign
// ---------------------------------------------------------------------------


    // ---------------------------------------------------------------------------
    // Handler: sfx.auto_assign (convenience wrapper)
    // ---------------------------------------------------------------------------

    async fn handle_sfx_auto_assign(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        use openscript_assets::sfx::SfxIndex;

        let timeline_path = extract_str(&args, "timeline_path")?;
        let gain_db = default_f64(&args, "gain_db", -10.0);
        let skip_hook = args.get("skip_hook").and_then(|v| v.as_bool()).unwrap_or(false);
        let skip_outro = args.get("skip_outro").and_then(|v| v.as_bool()).unwrap_or(false);

        let mut timeline = Timeline::load(timeline_path)?;
        let segments = timeline.segments.clone();
        if segments.is_empty() {
            return Ok(json!({"status": "warning", "message": "No segments found — cannot auto-assign SFX", "events_created": 0}));
        }

        let index_path = std::env::var("OPENSCRIPT_SFX_INDEX")
            .unwrap_or_else(|_| "mcp/assets/sfx_index.json".to_string());
        let sfx_index = SfxIndex::load(Some(&index_path)).ok();
        if sfx_index.is_none() {
            return Ok(json!({"status": "warning", "message": "SFX index not found — run sfx.index first", "events_created": 0}));
        }
        let sfx_index = sfx_index.unwrap();

        let mut events_created: Vec<serde_json::Value> = Vec::new();
        let mut current_idx = track_count(&timeline, &TrackType::Sfx);

        // 1) Hook SFX at the beginning
        if !skip_hook {
            let matched = sfx_index.search("", Some("intro"), None, 1).into_iter().next().cloned();
            if matched.is_none() {
                // No intro SFX in library — skip hook
            } else {
            current_idx += 1;
            let event_id = format!("sfx_{:03}", current_idx);
            let duration_ms = matched.as_ref().map(|a| a.duration_ms).unwrap_or(1000);
            let path = matched.as_ref().map(|a| a.path.clone());
            let event = openscript_core::timeline::TimelineEvent {
                id: event_id.clone(),
                asset_id: path.clone().unwrap_or_else(|| "hook".to_string()),
                start_ms: 0,
                end_ms: duration_ms,
                offset_ms: 0,
                gain_db,
                fade_in_ms: 50,
                fade_out_ms: 50,
                tags: vec!["hook".to_string()],
                provenance: Some(openscript_core::timeline::Provenance {
                    tool: "sfx.auto_assign".into(),
                    editorial_role: Some("hook".into()),
                    concept: None,
                }),
                kind: openscript_core::timeline::EventKind::Sfx {
                    editorial_role: "hook".to_string(),
                    category: String::new(),
                    subcategory: String::new(),
                    duration_ms,
                    sample_rate: 44100,
                    peak_db: 0.0,
                    loudness_lufs: -14.0,
                    recommended_gain_db: gain_db,
                    recommended_use: "single_hit".into(),
                    safe_overlay: true,
                },
            };
            timeline.add_track_event(TrackType::Sfx, event);
            if let Some(ref p) = path {
                timeline.add_asset("sfx", event_id.clone(), json!({"path": p}));
            }
            events_created.push(json!({"event_id": event_id, "role": "hook", "position_ms": 0}));
            } // end if matched.is_some()
        }

        // 2) Transition SFX at each segment boundary (after each segment ends)
        let timeline_end = segments.last().map(|s| (s.end * 1000.0) as i64).unwrap_or(0);
        let all_transitions: Vec<_> = sfx_index.search("", Some("transition"), None, 20).into_iter().cloned().collect();
        let num_transitions = all_transitions.len().max(1);
        for (i, seg) in segments.iter().enumerate() {
            if all_transitions.is_empty() { continue; }
            let matched = &all_transitions[i % num_transitions];
            current_idx += 1;
            let event_id = format!("sfx_{:03}", current_idx);
            let position_ms = (seg.end * 1000.0) as i64;
            let duration_ms = matched.duration_ms;
            let path = matched.path.clone();
            let event = openscript_core::timeline::TimelineEvent {
                id: event_id.clone(),
                asset_id: path.clone(),
                start_ms: position_ms,
                end_ms: (position_ms + duration_ms).min(timeline_end),
                offset_ms: 0,
                gain_db,
                fade_in_ms: 50,
                fade_out_ms: 50,
                tags: vec!["transition".to_string()],
                provenance: Some(openscript_core::timeline::Provenance {
                    tool: "sfx.auto_assign".into(),
                    editorial_role: Some("transition".into()),
                    concept: None,
                }),
                kind: openscript_core::timeline::EventKind::Sfx {
                    editorial_role: "transition".to_string(),
                    category: String::new(),
                    subcategory: String::new(),
                    duration_ms,
                    sample_rate: 44100,
                    peak_db: 0.0,
                    loudness_lufs: -14.0,
                    recommended_gain_db: gain_db,
                    recommended_use: "single_hit".into(),
                    safe_overlay: true,
                },
            };
            timeline.add_track_event(TrackType::Sfx, event);
            timeline.add_asset("sfx", event_id.clone(), json!({"path": &path}));
            events_created.push(json!({"event_id": event_id, "role": "transition", "position_ms": position_ms}));
        }

        // 3) Outro SFX at the end
        if !skip_outro {
            let last_end = segments.last().map(|s| (s.end * 1000.0) as i64).unwrap_or(0);
            let matched = sfx_index.search("", Some("outro"), None, 1).into_iter().next().cloned();
            if matched.is_some() {
            current_idx += 1;
            let event_id = format!("sfx_{:03}", current_idx);
            let duration_ms = matched.as_ref().map(|a| a.duration_ms).unwrap_or(1000);
            let path = matched.as_ref().map(|a| a.path.clone());
            let event = openscript_core::timeline::TimelineEvent {
                id: event_id.clone(),
                asset_id: path.clone().unwrap_or_else(|| "outro".to_string()),
                start_ms: (timeline_end - duration_ms).max(0),
                end_ms: timeline_end,
                offset_ms: 0,
                gain_db,
                fade_in_ms: 50,
                fade_out_ms: 50,
                tags: vec!["outro".to_string()],
                provenance: Some(openscript_core::timeline::Provenance {
                    tool: "sfx.auto_assign".into(),
                    editorial_role: Some("outro".into()),
                    concept: None,
                }),
                kind: openscript_core::timeline::EventKind::Sfx {
                    editorial_role: "outro".to_string(),
                    category: String::new(),
                    subcategory: String::new(),
                    duration_ms,
                    sample_rate: 44100,
                    peak_db: 0.0,
                    loudness_lufs: -14.0,
                    recommended_gain_db: gain_db,
                    recommended_use: "single_hit".into(),
                    safe_overlay: true,
                },
            };
            timeline.add_track_event(TrackType::Sfx, event);
            if let Some(ref p) = path {
                timeline.add_asset("sfx", event_id.clone(), json!({"path": p}));
            }
            events_created.push(json!({"event_id": event_id, "role": "outro", "position_ms": last_end}));
            } // end if matched.is_some()
        }

        timeline.save(timeline_path)?;
        Ok(json!({
            "status": "success",
            "events_created": events_created.len(),
            "positions": events_created,
            "timeline_path": timeline_path,
        }))
    }

    // ---------------------------------------------------------------------------
    // Handler: music.index (native via openscript-assets)
    // ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: music.search
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: music.assign
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: broll.suggest
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: broll.fetch (native via openscript-assets PexelsClient)
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: broll.assign
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: voiceover.generate
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: tts.commentary
// ---------------------------------------------------------------------------


/// Generate a single commentary voiceover segment: TTS synthesis + asset
/// registration + timeline event creation. Used by `handle_tts_commentary`
/// for intro / transition / outro segments. Prior versions had 3 near-identical
/// 50-line blocks; consolidated into this helper.
///
/// Returns `(event_id, duration_ms)` on success.
async fn generate_commentary_segment(
    timeline: &mut Timeline,
    timeline_dir: &std::path::Path,
    voice_profile_id: &str,
    text: &str,
    position_ms: i64,
    concept: &str,
    speed: f64,
    profile: &openscript_tts::profiles::VoiceProfile,
) -> Result<(String, i64), ToolError> {
    let event_id = format!(
        "voiceover_{:03}",
        track_count(timeline, &TrackType::Voiceover) + 1
    );
    let output_path = timeline_dir
        .join(format!("voiceover_{}.wav", event_id))
        .to_string_lossy()
        .to_string();

    if let Some(parent) = std::path::Path::new(&output_path).parent() {
        std::fs::create_dir_all(parent).ok();
    }

    let result = tts_generate_routed(
        voice_profile_id,
        text,
        &output_path,
        speed,
        1.0,
        1.0,
        "wav",
        None, // commentary segments carry no scene emotion
        None, // nor tone
        None, // temperature: engine default (expressive 0.7)
        None, // top_k
        None, // top_p
        None, // cfg_scale
        profile,
    )
    .await?;

    let duration_ms = result.duration_ms;

    timeline.add_asset(
        "voices",
        event_id.clone(),
        json!({
            "path": output_path.clone(),
            "voice_profile_id": voice_profile_id,
            "text": text,
        }),
    );

    let event = openscript_core::timeline::TimelineEvent {
        id: event_id.clone(),
        asset_id: output_path.clone(),
        start_ms: position_ms,
        end_ms: position_ms + duration_ms,
        offset_ms: 0,
        gain_db: -6.0,
        fade_in_ms: 50,
        fade_out_ms: 50,
        tags: vec!["commentary".to_string(), concept.to_string()],
        provenance: Some(openscript_core::timeline::Provenance {
            tool: "tts.commentary".into(),
            editorial_role: None,
            concept: Some(concept.to_string()),
        }),
        kind: openscript_core::timeline::EventKind::Voiceover {
            voice_profile_id: voice_profile_id.to_string(),
            text: text.to_string(),
            estimated_duration_ms: duration_ms,
        },
    };

    timeline.add_track_event(TrackType::Voiceover, event);
    Ok((event_id, duration_ms))
}

// ---------------------------------------------------------------------------
// Handler: timeline.diff
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: timeline.preview
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: tts.preview
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: music.ducking.plan
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: timeline.autofill_broll
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: timeline.render
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler removed: broll.director was a monolithic orchestrator
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Handler: broll.plan — segment inspector for agent-orchestrated b-roll
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: broll.keywords (LLM-mediated keyword extraction from transcripts)
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: broll.validate_keywords — Stage 2 relevance-validation/alignment
// ---------------------------------------------------------------------------
// Stage 1 (broll.keywords) drafts English keywords from the transcript. This
// stage closes the loop: it searches Pexels with those drafts, presents the
// REAL candidate videos (name/slug, duration, resolution) to the agent, and
// the agent validates each candidate against the spoken caption — producing
// final_keywords + the best video per segment. Drafts that Pexels can't
// serve (or that return irrelevant footage) are corrected HERE, before any
// download, instead of surfacing as mismatched b-roll in the render.


// ---------------------------------------------------------------------------
// Handler: broll.repair — gap-triggered re-pipeline with full timeline context
// ---------------------------------------------------------------------------
// The BROLL_GAP validator (timeline.validate / verify.production) flags any
// segment whose clip is shorter than its window. This tool re-triggers the
// whole agentic pipeline FOR EXACTLY THOSE GAPS, with the entire timeline as
// context (layer stack, all segments, already-covered concepts, already-used
// clips + the gap timestamps):
//   draft (agent) → search Pexels → validate candidates (agent) → download →
//   replace the event+asset. Non-looping: a clip is only placed when its
// source duration covers the window (+slack); non-redundant: already-used
// Pexels ids are excluded. Remaining gaps are returned for another pass.


// ---------------------------------------------------------------------------
// Handler: broll.auto (one-call A2V b-roll orchestrator — analyze → draft →
// validate → fetch → validate → repair loop until zero gaps remain)
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: broll.probe (all-engine stock candidate pool, normalized + ranked)
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: segment.analyze (transcript → clean segments for agent consumption)
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler:  (atomic tools)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Handler removed: audio.to_video was a monolithic orchestrator
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Handler: verify.audio
// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------


/// Parse an ASS (Advanced SubStation Alpha) caption file into SrtEntry-like
/// records so verify.captions can work with the ASS files that
/// script.to_video produces. Extracts Dialogue lines' start/end timestamps
/// and strips ASS override tags ({\...}) from the visible text.
///
/// ASS Dialogue format: `Dialogue: layer,start,end,style,name,L,R,E,text`
/// where start/end are `H:MM:SS.cc` (centiseconds, not milliseconds).
/// (UX audit round-2 GAP #10 fix — verify.captions previously only
/// accepted SRT, but script.to_video emits ASS.)
fn parse_ass_captions(path: &str) -> Result<Vec<openscript_core::srt::SrtEntry>, ToolError> {
    let content = std::fs::read_to_string(path).map_err(ToolError::Io)?;
    let mut entries = Vec::new();
    let mut idx = 1;

    for line in content.lines() {
        let line = line.trim();
        if !line.starts_with("Dialogue:") {
            continue;
        }
        // Dialogue: layer,start,end,style,name,L,R,E,text
        let parts: Vec<&str> = line.splitn(9, ',').collect();
        if parts.len() < 9 {
            continue;
        }
        let start_s = parse_ass_timestamp(parts[1].trim())?;
        let end_s = parse_ass_timestamp(parts[2].trim())?;
        // Strip ASS override tags {\...} from the visible text
        let text = strip_ass_tags(parts[8].trim());

        entries.push(openscript_core::srt::SrtEntry {
            idx,
            start: start_s,
            end: end_s,
            text,
        });
        idx += 1;
    }

    Ok(entries)
}

/// Parse an ASS timestamp `H:MM:SS.cc` into seconds (f64).
/// Example: "0:00:01.50" -> 1.5
fn parse_ass_timestamp(ts: &str) -> Result<f64, ToolError> {
    let parts: Vec<&str> = ts.split(':').collect();
    if parts.len() != 3 {
        return Err(ToolError::InvalidArg(format!(
            "Invalid ASS timestamp: {}",
            ts
        )));
    }
    let h: f64 = parts[0].parse().unwrap_or(0.0);
    let m: f64 = parts[1].parse().unwrap_or(0.0);
    let s: f64 = parts[2].parse().unwrap_or(0.0);
    Ok(h * 3600.0 + m * 60.0 + s)
}

/// Strip ASS override tags ({\...}) from text, leaving only visible characters.
fn strip_ass_tags(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut in_tag = false;
    for ch in text.chars() {
        if ch == '{' {
            in_tag = true;
        } else if ch == '}' {
            in_tag = false;
        } else if !in_tag {
            result.push(ch);
        }
    }
    result.trim().to_string()
}

// ---------------------------------------------------------------------------
// Handler: verify.captions
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: verify.render
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Production Quality KPIs — thin wrappers around openscript_core::production_quality
// ---------------------------------------------------------------------------

pub(crate) fn is_procedural_media_path(path: &str) -> bool {
    matches!(
        openscript_core::production_quality::classify_video_source(path, None),
        openscript_core::production_quality::VideoSourceClass::ProceduralSynthetic
    )
}

#[allow(dead_code)] // used by music rejection paths / tests
fn is_synthetic_music_file(path: &str) -> bool {
    openscript_core::production_quality::is_synthetic_music_file(path)
}

/// Legacy shim used by unit tests — maps to v2 scorer via a synthetic manifest.
#[cfg(test)]
fn compute_production_score(
    background_paths: &[String],
    music_path: Option<&str>,
    sticker_count: usize,
    meme_count: usize,
    has_dialogue: bool,
    rms_ok: bool,
    captions_present: bool,
) -> (i32, serde_json::Value, Vec<String>, Vec<String>) {
    use openscript_core::production_quality::*;
    let tl = Timeline::new(std::path::PathBuf::from("kpi.mp4"), "9:16", 30, None);
    let n = background_paths.len().max(1);
    let slice = (16_000 / n as i64).max(1);
    let backgrounds: Vec<BackgroundLayerInfo> = background_paths
        .iter()
        .enumerate()
        .map(|(i, p)| BackgroundLayerInfo {
            path: p.clone(),
            start_ms: i as i64 * slice,
            end_ms: (i as i64 + 1) * slice,
            source_hint: None,
            content_hash: Some(format!("shim_hash_{}", i)),
            video_id: None,
            search_query: Some(p.clone()),
            lexical_score: None,
            source_title: None,
            vision_score: None,
            vision_reason: None,
        })
        .collect();
    let stickers: Vec<StickerLayerInfo> = (0..sticker_count)
        .map(|i| StickerLayerInfo {
            path: format!("mcp/assets/stickers/giphy_{}.gif", i),
            start_ms: 0,
            end_ms: 1000,
            position: "top-left".into(),
            scale: 0.35,
        })
        .collect();
    let memes: Vec<MemeLayerInfo> = (0..meme_count)
        .map(|i| MemeLayerInfo {
            path: format!("meme_{}.mp4", i),
            start_ms: 2000 + i as i64 * 500,
            end_ms: 4500 + i as i64 * 500,
        })
        .collect();
    let caps = if captions_present {
        let p = std::env::temp_dir().join("kpi_caps.ass");
        let _ = std::fs::write(&p, b"[Script Info]\n");
        Some(p.to_string_lossy().to_string())
    } else {
        None
    };
    let music = music_path.map(|p| MusicLayerInfo {
        path: p.to_string(),
        gain_db: -12.0,
        ducking: true,
        mood: Some("neutral".into()),
        energy: Some("medium".into()),
        tags: vec!["pop".into()],
        selection_query: None,
        source: Some("library".into()),
    });
    let manifest = RenderManifest {
        duration_ms: 16_000,
        backgrounds,
        stickers,
        memes,
        music,
        captions_path: caps,
        voiceover_count: 1,
        sections: vec![],
        has_dialogue,
        rms_ok,
        video_keywords: vec![],
        theme: None,
        sfx_count: 2,
        caption_coverage_ratio: if captions_present { 0.95 } else { 0.0 },
        caption_style: if captions_present { Some("word_highlight".into()) } else { None },
        aspect_ratio: Some("9:16".into()),
        ..Default::default()
    };
    let report = evaluate_production_quality(&tl, &manifest);
    let dims = serde_json::to_value(&report.dimensions).unwrap_or(json!({}));
    (report.production_score, dims, report.hard_fails, report.next_actions)
}

/// Probe actual audio metrics from a rendered video using ffmpeg.
/// Returns (lufs, peak_dbfs, ducking_depth_db, music_gain_db).
/// Parse the integrated-LUFS value from ffmpeg loudnorm's print_format=json
/// output. The JSON block is printed at info level (so `-v error` suppresses
/// it) and may land on stdout (modern ffmpeg >=4.4) or stderr, followed by
/// the brace-free muxing summary on stderr. Anchor on the `"input_i"` key,
/// take the last '{' before it .. last '}' in the stream. PURE — unit-tested
/// (this exact parse has regressed multiple times: stdout/stderr split,
/// -v error suppression, trailing-summary parse failure).
pub(crate) fn parse_loudnorm_input_i(stream: &str) -> Option<f64> {
    let anchor = stream.find("\"input_i\"")?;
    let start = stream[..anchor].rfind('{')?;
    let end = stream.rfind('}')?;
    let v: serde_json::Value = serde_json::from_str(&stream[start..=end]).ok()?;
    v.get("input_i")
        .and_then(|x| x.as_str())
        .and_then(|s| s.parse::<f64>().ok())
}

/// Measure a WAV's integrated loudness (LUFS) via ffmpeg loudnorm JSON print.
/// Checks BOTH stdout and stderr (the block lands on stdout on modern ffmpeg
/// >=4.4 and stderr on older builds; `-v info` is implied by the default
/// level — `-v error` would suppress the print entirely).
pub(crate) async fn probe_audio_lufs(path: &str) -> Option<f64> {
    let out = tokio::process::Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-i",
            path,
            "-af",
            "loudnorm=print_format=json",
            "-f",
            "null",
            "-",
        ])
        .output()
        .await
        .ok()?;
    let stdout = String::from_utf8(out.stdout).ok();
    if let Some(i) = stdout.as_deref().and_then(parse_loudnorm_input_i) {
        return Some(i);
    }
    let stderr = String::from_utf8(out.stderr).ok()?;
    parse_loudnorm_input_i(&stderr)
}

async fn probe_audio_metrics(video_path: &str) -> (Option<f64>, Option<f64>, Option<f64>, Option<f64>) {
    use std::process::Stdio;

    // LUFS via loudnorm filter with JSON output — shared probe_audio_lufs
    // (checks stdout+stderr, anchored parse; previously lufs: null due to a
    // trailing-muxing-summary parse failure).
    let lufs = probe_audio_lufs(video_path).await;

    // Peak dBFS via volumedetect
    let peak = tokio::process::Command::new("ffmpeg")
        .args(["-i", video_path, "-af", "volumedetect", "-f", "null", "-"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .ok()
        .and_then(|o| String::from_utf8(o.stderr).ok())
        .and_then(|s| {
            for line in s.lines() {
                if let Some(idx) = line.find("max_volume:") {
                    let rest = &line[idx + "max_volume:".len()..];
                    let num = rest.split_whitespace().next().unwrap_or("");
                    if let Ok(v) = num.parse::<f64>() {
                        return Some(v);
                    }
                }
            }
            None
        });

    // Ducking depth: measure music level during speech vs non-speech segments.
    // This is a simplified approach - we compare overall RMS of music track
    // (not implemented yet, would need source separation).
    // For now, return None - can be enhanced later with better analysis.
    let ducking_depth_db = None::<f64>;

    // Music gain: try to infer from the music track if present
    // This would require extracting and analyzing the music track separately.
    // For now, return None - the manifest's planned gain_db will be used.
    let music_gain_db = None::<f64>;

    (lufs, peak, ducking_depth_db, music_gain_db)
}

async fn probe_dialogue_rms(video_path: &str) -> (bool, bool) {
    // Reuse ffprobe+ffmpeg volumedetect lightly
    let out = tokio::process::Command::new("ffmpeg")
        .args([
            "-i",
            video_path,
            "-af",
            "volumedetect",
            "-f",
            "null",
            "-",
        ])
        .output()
        .await;
    let stderr = out
        .map(|o| String::from_utf8_lossy(&o.stderr).to_string())
        .unwrap_or_default();
    let mut mean_db = -100.0_f64;
    for line in stderr.lines() {
        // ffmpeg prints: "[Parsed_volumedetect_0 @ 0x…] mean_volume: -16.5 dB"
        if let Some(idx) = line.find("mean_volume:") {
            let rest = &line[idx + "mean_volume:".len()..];
            let num = rest.split_whitespace().next().unwrap_or("");
            if let Ok(v) = num.parse::<f64>() {
                mean_db = v;
            }
        }
    }
    // Dialogue typically > -45 dB mean if present; pure silence ~ -91
    let has_dialogue = mean_db > -50.0;
    let rms_ok = (-30.0..=-8.0).contains(&mean_db);
    (has_dialogue, rms_ok)
}

/// Probe b-roll motion in the rendered video via ffmpeg's `scene` filter.
/// Returns (motion_ratio, longest_static_run_s):
/// - `motion_ratio`: fraction of frames with non-zero motion (scene_score > 0.001)
/// - `longest_static_run_s`: longest consecutive sequence of static frames, in seconds
///
/// Used by `handle_verify_production` to populate
/// `RenderManifest::broll_motion_ratio` and `RenderManifest::longest_static_run_s`,
/// which feed into the `score_broll_motion` dimension. This catches the
/// source-exhaustion bug where short Pexels sources are held as last-frame
/// after `seek_offset` exhausts the source mid-segment (produces 8-13s
/// static-image stretches on rendered output).
///
/// Cost: one ffmpeg pass, ~1s for a 30s video at 30fps. Returns (None, None)
/// on ffmpeg failure or zero frames so the verifier degrades gracefully.
async fn probe_broll_motion(video_path: &str) -> (Option<f64>, Option<f64>) {
    use std::process::Stdio;

    let out = tokio::process::Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-loglevel", "info",
            "-i", video_path,
            "-vf", "select='gte(scene\\,0)',metadata=print:file=-",
            "-an", "-f", "null", "/dev/null",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await;

    let buf = String::from_utf8_lossy(&out.as_ref().map(|o| &o.stdout).unwrap_or(&Vec::new())).to_string();

    // Also probe fps to convert frame counts to seconds.
    let fps_str = tokio::process::Command::new("ffprobe")
        .args([
            "-v", "error",
            "-select_streams", "v",
            "-show_entries", "stream=avg_frame_rate",
            "-of", "csv=p=0",
            video_path,
        ])
        .output()
        .await
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    let fps: f64 = fps_str
        .trim()
        .split('/')
        .collect::<Vec<_>>()
        .get(1)
        .and_then(|d| d.parse::<f64>().ok())
        .map(|d| {
            fps_str
                .trim()
                .split('/')
                .next()
                .and_then(|n| n.parse::<f64>().ok())
                .map(|n| n / d)
                .unwrap_or(30.0)
        })
        .unwrap_or(30.0)
        .max(1.0);

    use std::sync::OnceLock;
    use regex::Regex;
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"scene_score=([0-9.]+)").unwrap());

    let mut scores: Vec<f64> = Vec::new();
    for line in buf.lines() {
        if let Some(c) = re.captures(line) {
            if let Ok(v) = c[1].parse::<f64>() {
                scores.push(v);
            }
        }
    }
    if scores.is_empty() {
        return (None, None);
    }
    let n = scores.len();
    let motion_frames = scores.iter().filter(|s| **s > 0.001).count();
    let ratio = motion_frames as f64 / n as f64;

    // Longest consecutive run of static frames
    let mut longest_run: usize = 0;
    let mut cur_run: usize = 0;
    for s in &scores {
        if *s <= 0.001 {
            cur_run += 1;
            longest_run = longest_run.max(cur_run);
        } else {
            cur_run = 0;
        }
    }
    let run_s = longest_run as f64 / fps;

    (Some(ratio), Some(run_s))
}

/// Per-clip b-roll motion analysis using frame-hash comparison.
/// Extracts one JPEG frame per second from each clip's time window,
/// computes MD5 hashes, and measures uniqueness. A clip where all
/// frames hash identically is genuinely static (held frame). A clip
/// where hashes vary has inter-frame motion regardless of how subtle
/// the zoompan effect is.
///
/// Returns per-clip (motion_ratio, longest_static_run_s) where:
/// - motion_ratio = unique_hash_count / total_sampled_frames
/// - longest_static_run_s = longest consecutive run of identical hashes in seconds
async fn probe_broll_motion_per_clip(
    video_path: &str,
    clip_ranges: &[(f64, f64)],
) -> Vec<(usize, Option<f64>, Option<f64>)> {
    use std::process::Stdio;

    if clip_ranges.is_empty() {
        return Vec::new();
    }

    // For each clip, extract one frame per second at its center timestamps
    // and compute MD5 hashes. This directly tests whether the rendered
    // pixels change between frames (zoompan, motion, transitions) or are
    // held identically (static frame bug).
    let mut results: Vec<(usize, Option<f64>, Option<f64>)> = Vec::new();

    for (idx, &(start_s, end_s)) in clip_ranges.iter().enumerate() {
        let duration = end_s - start_s;
        if duration <= 0.0 {
            results.push((idx, Some(0.0), Some(0.0)));
            continue;
        }
        // Sample up to 10 frames per clip (1fps, capped)
        let sample_count = (duration.ceil() as usize).min(10).max(2);
        let interval = duration / sample_count as f64;

        // Extract each frame individually via ffmpeg -ss (fast seek)
        let mut hashes: Vec<u64> = Vec::new();
        for s in 0..sample_count {
            let ts = start_s + (s as f64 + 0.5) * interval; // center of each sub-window
            let path_str = format!("/tmp/broll_probe_{}_{}.jpg", idx, s);
            let _ = tokio::process::Command::new("ffmpeg")
                .args([
                    "-nostdin", "-loglevel", "error",
                    "-ss", &format!("{:.3}", ts),
                    "-i", video_path,
                    "-vframes", "1", "-q:v", "5", // low quality JPEG for speed
                    &path_str,
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .output()
                .await;
            // Hash the JPEG file content using stdlib DefaultHasher
            if let Ok(data) = tokio::fs::read(&path_str).await {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut hasher = DefaultHasher::new();
                data.hash(&mut hasher);
                let hash = hasher.finish();
                hashes.push(hash);
            }
            let _ = tokio::fs::remove_file(&path_str).await;
        }

        if hashes.is_empty() {
            results.push((idx, None, None));
            continue;
        }

        // Motion ratio: fraction of unique hashes (0.0 = all same = fully static)
        let unique_count = hashes.iter().collect::<std::collections::HashSet<_>>().len();
        let motion_ratio = unique_count as f64 / hashes.len() as f64;

        // Longest consecutive run of identical hashes
        let mut longest_run: usize = 1;
        let mut cur_run: usize = 1;
        for w in hashes.windows(2) {
            if w[0] == w[1] {
                cur_run += 1;
                longest_run = longest_run.max(cur_run);
            } else {
                cur_run = 1;
            }
        }
        // Convert run count to seconds (each sample covers `interval` seconds)
        let longest_run_s = longest_run as f64 * interval;

        results.push((idx, Some(motion_ratio), Some(longest_run_s)));
    }

    results
}

/// Probe every b-roll asset's real duration and compare it against the
/// segment window it is assigned to. Returns actionable coverage gaps —
/// segments whose clip is shorter than the window.
///
/// Per docs/SEGMENTATION_UPGRADE_PLAN.md Phase B: the renderer no longer
/// loops short clips to fill their window (Phase A — clips play exactly
/// once), so a short clip leaves the window tail holding the last frame.
/// These gaps are surfaced as validator errors so the agent loop re-runs
/// keyword generation + `broll.fetch` for exactly those segments.
/// Tolerance (seconds) above which a segment/scene end is considered to
/// overshoot the source media. Shared by the clamp + validate paths so they
/// stay in agreement (0.05s ≈ 1.5 audio frames at 30fps).
const SOURCE_DUR_TOLERANCE_S: f64 = 0.05;

/// Probe a media file's duration. Returns `None` when the path is empty,
/// missing, or unprobeable — callers treat that as "no source duration known"
/// and leave segments untouched (the render's `-shortest` still caps output).
async fn probe_source_duration(path: &Path) -> Option<f64> {
    if path.as_os_str().is_empty() || !path.exists() {
        return None;
    }
    match openscript_ffmpeg::probe::probe(path.to_string_lossy().as_ref()).await {
        Ok(m) if m.duration > 0.0 => Some(m.duration),
        _ => None,
    }
}

/// Clamp `segments` so every segment fits inside `src_dur` (the master clock).
/// Segments that START past the source end are dropped entirely (clamping them
/// would invert start>end and produce a negative-duration atrim in the render);
/// segments that merely END past it are clamped. Returns `(dropped, clamped)`.
fn clamp_segments_to_duration(
    segments: &mut Vec<openscript_core::timeline::Segment>,
    src_dur: f64,
) -> (usize, usize) {
    let before = segments.len();
    segments.retain(|s| s.start < src_dur);
    let dropped = before - segments.len();
    let mut clamped = 0usize;
    for seg in segments.iter_mut() {
        if seg.end > src_dur + SOURCE_DUR_TOLERANCE_S {
            seg.end = src_dur;
            clamped += 1;
        }
    }
    (dropped, clamped)
}

// ---------------------------------------------------------------------------
// Helpers: agentic b-roll relevance pipeline (Phase 136)
// ---------------------------------------------------------------------------

/// Extract the human-readable video slug from a Pexels video URL
/// (`https://www.pexels.com/video/government-building-crowd-12345/` →
/// "Government Building Crowd"). The Pexels API returns no separate title
/// field — this slug IS the stock video's name/description, and it is the
/// primary signal the relevance-validation agent scores candidates on.
fn pexels_url_slug(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    let slug = trimmed.rsplit('/').next().unwrap_or("");
    let mut words: Vec<&str> = slug.split('-').collect();
    if let Some(last) = words.last() {
        if !last.is_empty() && last.chars().all(|c| c.is_ascii_digit()) {
            words.pop();
        }
    }
    let name = words
        .iter()
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(first) => {
                    let mut word = first.to_uppercase().collect::<String>();
                    word.push_str(&w[first.len_utf8()..]);
                    word
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    if name.is_empty() {
        "stock footage".to_string()
    } else {
        name
    }
}

/// Extract the Pexels video id embedded in a cache path like
/// `mcp/assets/broll_cache/<concept>_<id>.mp4`. Used to exclude already-used
/// clips from the next fetch/repair pass (non-redundant footage rule).
fn cache_path_video_id(path: &str) -> Option<i64> {
    let stem = Path::new(path).file_stem()?.to_str()?;
    let last = stem.rsplit('_').next()?;
    last.parse::<i64>().ok()
}

/// Non-looping duration gate: a candidate is only eligible for a segment
/// window when its full source duration covers the window plus a small trim
/// slack. Selecting a shorter clip would force loop/held-frame rendering —
/// the exact static-tail defect the validator flags as BROLL_GAP.
fn candidates_covering_window<'a>(
    videos: &'a [openscript_assets::pexels::PexelsVideo],
    window_s: f64,
    slack_s: f64,
) -> Vec<&'a openscript_assets::pexels::PexelsVideo> {
    videos
        .iter()
        .filter(|v| v.duration as f64 >= window_s + slack_s)
        .collect()
}

/// Find the timeline segment whose window contains (or best overlaps) the
/// b-roll event `evt_id`. broll.fetch places one event per enriched segment,
/// but ids may drift — window matching is the robust key.
fn find_segment_for_window<'a>(
    timeline: &'a Timeline,
    evt_id: &str,
) -> Option<&'a openscript_core::timeline::Segment> {
    let broll_evt = timeline
        .tracks
        .get(&TrackType::Broll)
        .and_then(|evs| evs.iter().find(|e| e.id == evt_id))?;
    let start_ms = broll_evt.start_ms as f64;
    let end_ms = broll_evt.end_ms as f64;
    timeline
        .segments
        .iter()
        .find(|s| {
            let s_start = s.start * 1000.0;
            let s_end = s.end * 1000.0;
            start_ms >= s_start - 60.0 && end_ms <= s_end + 60.0
        })
        .or_else(|| {
            timeline.segments.iter().find(|s| {
                let s_start = s.start * 1000.0;
                let s_end = s.end * 1000.0;
                start_ms < s_end && end_ms > s_start
            })
        })
}

/// Normalize caption text for word-alignment comparison: lowercase, drop
/// punctuation, collapse whitespace. Used to decide whether a word SRT's
/// words actually match the phrase transcript (same language + content)
/// before adopting the word SRT's real per-word timings.
fn normalize_caption_text(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect::<String>()
}

/// Build per-word timings for one phrase. Real word-SRT timings are used ONLY
/// when the word entries overlapping the phrase window join to the same text
/// as the phrase (language + content match). Otherwise — the word SRT is a
/// different language (English word SRT over Hinglish audio) or stale/partial
/// (60s hole) — fall back to char-proportional estimates so the caption keeps
/// the phrase's language and full coverage.
///
/// `out_start_ms` is the phrase's start on the OUTPUT clock (crossfade-remapped
/// when applicable); `phrase_start_s` is the phrase's start on the SOURCE clock.
/// Word timings are translated onto the output clock the same way.
/// Token-set Jaccard similarity of two normalized caption strings
/// (0.0 = disjoint, 1.0 = identical token sets). Used to accept real
/// alignment windows when the ASR word text differs slightly from the
/// phrase text (Hinglish ASR: "nahin" vs "nahi", dropped filler words) —
/// the exact-equality gate was collapsing those to even-spacing estimates.
fn caption_text_similarity(a: &str, b: &str) -> f64 {
    // Tokenize the RAW strings (normalize_caption_text concatenates without
    // spaces, so it cannot be re-split into tokens). Each token is
    // lowercased and punctuation-stripped.
    let tokenize = |s: &str| -> std::collections::HashSet<String> {
        s.split_whitespace()
            .map(|t| {
                t.chars()
                    .filter(|c| c.is_alphanumeric())
                    .flat_map(|c| c.to_lowercase())
                    .collect::<String>()
            })
            .filter(|t| !t.is_empty())
            .collect()
    };
    let ta = tokenize(a);
    let tb = tokenize(b);
    if ta.is_empty() && tb.is_empty() {
        return 1.0;
    }
    let inter = ta.intersection(&tb).count();
    let union = ta.union(&tb).count();
    if union == 0 {
        0.0
    } else {
        inter as f64 / union as f64
    }
}

fn caption_words_for_phrase(
    phrase: &openscript_core::srt::SrtEntry,
    word_entries: &[openscript_core::srt::SrtEntry],
    out_start_ms: i64,
    phrase_start_s: f64,
) -> Vec<WordTiming> {
    // Words whose window overlaps this phrase's window (with tolerance).
    let tol = 0.05;
    let overlapping: Vec<&openscript_core::srt::SrtEntry> = word_entries
        .iter()
        .filter(|w| w.start >= phrase.start - tol && w.end <= phrase.end + tol)
        .collect();
    let joined: String = overlapping
        .iter()
        .map(|w| w.text.trim())
        .collect::<Vec<_>>()
        .join(" ");
    let joined_norm = normalize_caption_text(&joined);
    let phrase_norm = normalize_caption_text(&phrase.text);
    // Accept the alignment windows when the ASR word text matches the phrase
    // exactly OR closely (Jaccard >= 0.5) — minor ASR differences must not
    // throw away real timings. When accepted, the word TEXT is overridden
    // with the phrase's own words (caption text = ground truth; alignment
    // windows = real), zipped by index and truncated to the shorter side.
    let close_match = !joined_norm.is_empty()
        && !phrase_norm.is_empty()
        && (joined_norm == phrase_norm
            || caption_text_similarity(&joined, &phrase.text) >= 0.5);
    if !overlapping.is_empty() && close_match {
        let phrase_words: Vec<&str> = phrase.text.split_whitespace().collect();
        let mut out: Vec<WordTiming> = overlapping
            .iter()
            .zip(phrase_words.iter())
            .map(|(w, pw)| WordTiming {
                word: pw.to_string(),
                start_ms: out_start_ms + ((w.start - phrase_start_s) * 1000.0).round() as i64,
                end_ms: out_start_ms + ((w.end - phrase_start_s) * 1000.0).round() as i64,
            })
            .collect();
        // Fuzzy matches can have FEWER aligned words than the phrase (ASR
        // merged/dropped words). Pad the tail with char-proportional timings
        // over the remaining window so every phrase word gets a highlight cue
        // instead of the last words rendering unhighlighted.
        if out.len() < phrase_words.len() {
            let last_end = out.last().map(|w| w.end_ms).unwrap_or(out_start_ms);
            let window_ms = (phrase.end - phrase.start) * 1000.0;
            let elapsed_ms = (last_end - out_start_ms) as f64;
            let tail_ms = (window_ms - elapsed_ms).max(0.0);
            let remaining: Vec<&str> = phrase_words[out.len()..].to_vec();
            let chars: usize = remaining.iter().map(|w| w.len()).sum();
            let mut acc = last_end;
            for w in remaining {
                let dur = if chars > 0 {
                    (tail_ms * w.len() as f64 / chars as f64).round() as i64
                } else {
                    0
                };
                let start = acc;
                let end = (acc + dur).max(start + 1);
                out.push(WordTiming {
                    word: w.to_string(),
                    start_ms: start,
                    end_ms: end,
                });
                acc = end;
            }
        }
        out
    } else {
        estimate_word_timings(
            &phrase.text,
            out_start_ms,
            out_start_ms + ((phrase.end - phrase.start) * 1000.0).round() as i64,
        )
    }
}

/// Collect the Pexels video ids already in use across the timeline's broll
/// assets — the non-redundancy blocklist for the next fetch/repair pass.
fn used_broll_video_ids(timeline: &Timeline) -> std::collections::HashSet<i64> {
    let mut used = std::collections::HashSet::new();
    for (_, asset) in timeline.assets.broll.iter() {
        if let Some(p) = asset.get("path").and_then(|v| v.as_str()) {
            if let Some(id) = cache_path_video_id(p) {
                used.insert(id);
            }
        }
    }
    used
}

/// First `n` candidates whose Pexels id is NOT in `used_ids` — i.e. footage
/// not already placed on this timeline (the b-roll-repeat bug: the deterministic
/// LLM-down path could place the same clip on two segments when their concepts
/// resolved to the same first Pexels result). Falls back to the first `n`
/// candidates when the library is genuinely exhausted so a segment never goes
/// bare. Returns (selected, reused) where `reused` counts how many selected
/// candidates were actually already-used ids (the silent-repeat case) — the
/// caller surfaces this as a warning so reuse is never invisible.
fn fresh_candidates<'a>(
    videos: &'a [openscript_assets::pexels::PexelsVideo],
    used_ids: &std::collections::HashSet<i64>,
    n: usize,
) -> (Vec<&'a openscript_assets::pexels::PexelsVideo>, usize) {
    let fresh: Vec<&openscript_assets::pexels::PexelsVideo> = videos
        .iter()
        .filter(|v| !used_ids.contains(&v.id))
        .take(n)
        .collect();
    if fresh.is_empty() {
        // Library exhausted for this concept — reuse the top hits rather than
        // leaving the segment bare, but report exactly how many reused ids.
        (videos.iter().take(n).collect(), videos.iter().take(n).filter(|v| used_ids.contains(&v.id)).count())
    } else {
        (fresh, 0)
    }
}

/// Structured timeline-viewer context: the composition layer stack (bottom →
/// top as rendered), every track's events with asset/concept/timing, and the
/// b-roll coverage gaps. This is the meta-cognitive layer an agent in the
/// keyword→fetch→repair loop reasons over — it shows what is present, in what
/// order, and exactly where the holes are.
fn build_timeline_viewer_context(timeline: &Timeline) -> serde_json::Value {
    let layer_names = ["broll", "captions", "voiceover", "music", "sfx"];
    let layers: Vec<serde_json::Value> = layer_names
        .iter()
        .enumerate()
        .map(|(z, name)| {
            let events = timeline
                .tracks
                .iter()
                .find(|(t, _)| t.to_string() == *name)
                .map(|(_, e)| e.clone())
                .unwrap_or_default();
            let event_json: Vec<serde_json::Value> = events
                .iter()
                .map(|e| {
                    let (concept, path, src_dur) = match &e.kind {
                        openscript_core::timeline::EventKind::Broll { concept, .. } => {
                            let asset = timeline.assets.broll.get(&e.asset_id);
                            let p = asset
                                .and_then(|a| a.get("path"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let d = asset
                                .and_then(|a| a.get("source_duration_s"))
                                .and_then(|v| v.as_f64())
                                .unwrap_or(0.0);
                            (concept.clone(), p, d)
                        }
                        _ => (String::new(), String::new(), 0.0),
                    };
                    json!({
                        "id": e.id,
                        "asset_id": e.asset_id,
                        "start_ms": e.start_ms,
                        "end_ms": e.end_ms,
                        "duration_ms": e.end_ms - e.start_ms,
                        "concept": concept,
                        "path": path,
                        "source_duration_s": src_dur,
                    })
                })
                .collect();
            json!({
                "layer": name,
                "z_index": z + 1,
                "event_count": events.len(),
                "events": event_json,
            })
        })
        .collect();
    json!({
        "layer_order_bottom_to_top": layer_names,
        "layers": layers,
    })
}

/// Render the timeline-viewer context as compact prompt text for the agentic
/// stages (broll.repair). Keeps tokens low while preserving the operational
/// flow: layer stack, per-segment captions/windows, covered concepts, clips.
fn render_timeline_context_text(timeline: &Timeline) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "TIMELINE CONTEXT (source: {})",
        timeline.source.to_string_lossy()
    );
    let _ = writeln!(out, "--- LAYERS (bottom→top) ---");
    for (z, name) in ["broll", "captions", "voiceover", "music", "sfx"]
        .iter()
        .enumerate()
    {
        let events = timeline
            .tracks
            .iter()
            .find(|(t, _)| t.to_string() == *name)
            .map(|(_, e)| e.len())
            .unwrap_or(0);
        let _ = writeln!(out, "{}. {} ({} events)", z + 1, name, events);
    }
    let _ = writeln!(out, "--- SEGMENTS ({}) ---", timeline.segments.len());
    for s in &timeline.segments {
        let _ = writeln!(
            out,
            "[{}] {:.1}s–{:.1}s ({}s): {}",
            s.id,
            s.start,
            s.end,
            s.end - s.start,
            s.caption
        );
    }
    let _ = writeln!(out, "--- BROLL COVERAGE ---");
    for evt in timeline.tracks.get(&TrackType::Broll).cloned().unwrap_or_default() {
        let (concept, path, src_dur) = match &evt.kind {
            openscript_core::timeline::EventKind::Broll { concept, .. } => {
                let asset = timeline.assets.broll.get(&evt.asset_id);
                let p = asset
                    .and_then(|a| a.get("path"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let d = asset
                    .and_then(|a| a.get("source_duration_s"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                (concept.clone(), p, d)
            }
            _ => (String::new(), String::new(), 0.0),
        };
        let _ = writeln!(
            out,
            "[{}] {:.1}s–{:.1}s concept='{}' clip_dur={:.1}s {}",
            evt.id,
            evt.start_ms as f64 / 1000.0,
            evt.end_ms as f64 / 1000.0,
            concept,
            src_dur,
            path
        );
    }
    out
}

/// Loose JSON-object parser for LLM responses (tolerant of markdown prose
/// around the object) — same pattern used by broll.keywords.
fn parse_loose_json_obj(s: &str) -> serde_json::Value {
    let trimmed = s.trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return v;
    }
    if let Some(start) = trimmed.find('{') {
        if let Some(end) = trimmed.rfind('}') {
            if end > start {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&trimmed[start..=end]) {
                    return v;
                }
            }
        }
    }
    serde_json::json!({})
}

/// Stage 1 — agentic keyword draft: translate the spoken caption into English
/// visual search keywords, avoiding the concepts already covered in the
/// timeline (non-redundancy). Falls back to the Hinglish→English visual map +
/// stopword extraction when the LLM cascade is down.
async fn llm_draft_keywords(
    caption: &str,
    avoid_concepts: &[String],
    language: &str,
) -> (Vec<String>, String, String) {
    // Delegates to the unified keywords module — one LLM call, id-echo,
    // missing-id redraft, and the salience heuristic as the LLM-down fallback
    // (replaces the old first-three-words extract_broll_concept path).
    let input = crate::keywords::SegmentInput {
        segment_id: "seg_0".into(),
        caption: caption.to_string(),
        language_hint: if language.is_empty() { None } else { Some(language.to_string()) },
        duration_s: 0.0,
        scene_idx: 0,
        total_scenes: 1,
        video_title: String::new(),
        video_keywords: Vec::new(),
        covered_concepts: avoid_concepts.to_vec(),
    };
    let drafted = crate::keywords::draft_scene_keywords(&[input]).await;
    let d = &drafted[0];
    let kws = d.visual.clone();
    let (backend, model) = match d.source {
        crate::keywords::KeywordSource::Heuristic => {
            ("fallback".to_string(), "salience-v1".to_string())
        }
        _ => {
            let parts: Vec<&str> = d.backend.split('/').collect();
            if parts.len() == 2 {
                (parts[0].to_string(), parts[1].to_string())
            } else {
                (d.backend.clone(), String::new())
            }
        }
    };
    (kws, backend, model)
}

/// Stage 2 — relevance-validation/alignment: the agent scores real Pexels
/// candidates (video names/slugs, durations, resolution) against the spoken
/// caption + draft keywords and returns final keywords + the best video id.
/// Heuristic fallback (LLM down): keyword-token overlap with the slug, then
/// longest clip that covers the window. Returns
/// (best_id, final_keywords, relevance, reason, backend, model).
async fn llm_validate_candidates(
    caption: &str,
    draft_keywords: &[String],
    candidates: &[openscript_assets::pexels::PexelsVideo],
    window_s: f64,
    avoid_video_ids: &std::collections::HashSet<i64>,
) -> (Option<i64>, Vec<String>, f64, String, String, String) {
    let candidate_lines: Vec<String> = candidates
        .iter()
        .filter(|v| !avoid_video_ids.contains(&v.id))
        .take(6)
        .map(|v| {
            format!(
                "  id={} name=\"{}\" duration={}s size={}x{}",
                v.id,
                pexels_url_slug(&v.url),
                v.duration,
                v.width,
                v.height
            )
        })
        .collect();
    let system = "You are a short-form video director's relevance validator. \
        Given a spoken segment and candidate stock videos (Pexels name + duration + resolution), \
        decide which candidate best ILLUSTRATES the speech, and refine the search keywords. \
        Rules: prefer a concrete visual match to the speech MEANING; prefer duration >= the segment \
        window (never pick a clip that would loop); relevance 0.0-1.0. \
        Output ONLY compact JSON: \
        {\"best_video_id\":123,\"final_keywords\":[\"k1\"],\"relevance\":0.9,\"reason\":\"one sentence\"}";
    let user = format!(
        "Segment caption (spoken): \"{}\"\nSegment window: {:.1}s\nDraft keywords: [{}]\nCandidates:\n{}\n\
         Pick the single best video id (or null if none match). JSON only.",
        caption,
        window_s,
        draft_keywords.join(", "),
        candidate_lines.join("\n")
    );
    match crate::llm::chat_complete(system, &user, None).await {
        Ok(r) => {
            let parsed = parse_loose_json_obj(&r.text);
            let best_id = parsed
                .get("best_video_id")
                .and_then(|v| v.as_i64())
                .or_else(|| {
                    parsed
                        .get("best_video_id")
                        .and_then(|v| v.as_u64())
                        .map(|u| u as i64)
                });
            let kws: Vec<String> = parsed
                .get("final_keywords")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|k| k.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_else(|| draft_keywords.to_vec());
            let kws: Vec<String> = kws.into_iter().filter(|k| k.len() >= 3).collect();
            let rel = parsed.get("relevance").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let reason = parsed
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            (
                best_id,
                if kws.is_empty() { draft_keywords.to_vec() } else { kws },
                rel,
                reason,
                r.backend,
                r.model,
            )
        }
        Err(e) => {
            tracing::warn!(
                "[broll.repair] validation LLM failed: {} — heuristic pick",
                e
            );
            let eligible: Vec<&openscript_assets::pexels::PexelsVideo> = candidates
                .iter()
                .filter(|v| !avoid_video_ids.contains(&v.id))
                .collect();
            let mut best: Option<(i64, usize, i64)> = None;
            for v in eligible {
                let slug_lower = pexels_url_slug(&v.url).to_lowercase();
                let overlap = draft_keywords
                    .iter()
                    .filter(|k| slug_lower.contains(&k.to_lowercase()))
                    .count();
                let better = match &best {
                    Some((_, bo, bd)) => *bo < overlap || (*bo == overlap && *bd < v.duration),
                    None => true,
                };
                if better {
                    best = Some((v.id, overlap, v.duration));
                }
            }
            let chosen = best.map(|(id, _, _)| id);
            (
                chosen,
                draft_keywords.to_vec(),
                if chosen.is_some() { 0.7 } else { 0.0 },
                "heuristic fallback (LLM unavailable)".into(),
                "fallback".into(),
                "heuristic".into(),
            )
        }
    }
}

async fn probe_broll_gaps(timeline: &Timeline) -> Vec<openscript_core::production_quality::BrollGap> {
    use openscript_core::production_quality::BrollGap;

    let mut gaps = Vec::new();
    let Some(broll_track) = timeline.tracks.get(&TrackType::Broll) else {
        return gaps;
    };
    for evt in broll_track {
        let segment_dur_s = (evt.end_ms - evt.start_ms) as f64 / 1000.0;
        if segment_dur_s <= 0.0 {
            continue;
        }
        let path = timeline
            .assets
            .broll
            .get(&evt.asset_id)
            .and_then(|v| v.get("path").and_then(|p| p.as_str()))
            .unwrap_or("")
            .to_string();
        if path.is_empty() || path == "placeholder" {
            continue;
        }
        // Prefer the source_duration_s hint stored by broll.fetch (from Pexels
        // metadata) — no ffprobe round-trip. Fall back to probing the file.
        let hinted = timeline
            .assets
            .broll
            .get(&evt.asset_id)
            .and_then(|v| v.get("source_duration_s"))
            .and_then(|v| v.as_f64())
            .filter(|d| *d > 0.0);
        let available_s = if let Some(d) = hinted {
            d
        } else {
            match openscript_ffmpeg::probe::probe(&path).await {
                Ok(m) => m.duration,
                Err(e) => {
                    // Unprobeable clip (missing file / corrupt): report it as a
                    // 0s gap instead of silently passing — the renderer would
                    // emit loop=1, exhaust mid-window and hold the last frame,
                    // exactly the static-tail the validator must catch.
                    tracing::warn!(
                        "[broll_gaps] could not probe asset {} ({}): {} — flagging as uncovered",
                        evt.asset_id, path, e
                    );
                    0.0
                }
            }
        };
        // Tolerance: 0.25s — clip may end a hair early without a visible gap.
        // available_s == 0.0 (unprobeable) is ALWAYS a gap.
        if available_s <= 0.0 || available_s < segment_dur_s - 0.25 {
            // Unprobeable clip: report available_s = 0.0 (unknown).
            let available = if available_s > 0.0 { available_s } else { 0.0 };
            let gap_s = (segment_dur_s - available).max(0.0);
            let concept = match &evt.kind {
                openscript_core::timeline::EventKind::Broll { concept, .. } => concept.clone(),
                _ => String::new(),
            };
            gaps.push(BrollGap {
                segment_id: evt.id.clone(),
                concept,
                asset_id: evt.asset_id.clone(),
                asset_path: path,
                required_s: (segment_dur_s * 100.0).round() / 100.0,
                available_s: (available * 100.0).round() / 100.0,
                gap_s: (gap_s * 100.0).round() / 100.0,
                action: format!(
                    "re-run broll.keywords + broll.fetch for segment {} — need clip >= {:.1}s",
                    evt.id, segment_dur_s
                ),
            });
        }
    }
    gaps
}

/// Post-generation COMPOSITION AUDIT — the meta-cognitive layer the agent
/// needs to reason about its own render. Enumerates every layer that is
/// present in the composition, in bottom-to-top z-order, with per-layer event
/// counts and time ranges. Without this, an agent in an iterative loop cannot
/// tell whether the render it is judging actually contains the layers it
/// thinks it placed (e.g. captions that never burned, a music track that was
/// never mixed, stickers that silently dropped).
/// Map Stickers-track events into `StickerLayerInfo` so agent-placed stickers
/// (sticker.auto / sticker.auto_assign) are visible to the production-quality
/// scorer and the composition audit. Resolves the asset path via the registry
/// convention (asset_id = event_id key under `assets.broll`) with a fallback to
/// the event's `source_provider`.
fn stickers_from_timeline(
    timeline: &Timeline,
) -> Vec<openscript_core::production_quality::StickerLayerInfo> {
    use openscript_core::production_quality::StickerLayerInfo;
    use openscript_core::timeline::EventKind;

    let Some(sticker_track) = timeline.tracks.get(&TrackType::Stickers) else {
        return Vec::new();
    };
    let mut out: Vec<StickerLayerInfo> = Vec::new();
    for evt in sticker_track {
        let EventKind::Broll { source_provider, .. } = &evt.kind else {
            continue;
        };
        let registry = timeline.assets.broll.get(&evt.asset_id);
        let path = registry
            .and_then(|v| v.get("path").and_then(|p| p.as_str()))
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty() && s != "placeholder")
            .unwrap_or_else(|| source_provider.clone());
        if path.is_empty() {
            continue;
        }
        let position = registry
            .and_then(|v| v.get("position").and_then(|p| p.as_str()))
            .map(|s| s.to_string())
            .or_else(|| {
                evt.tags.iter().find(|t| {
                    matches!(
                        t.as_str(),
                        "top-left" | "top-right" | "bottom-left" | "bottom-right" | "center"
                    )
                }).cloned()
            })
            .unwrap_or_else(|| "top-right".to_string());
        let scale = registry
            .and_then(|v| v.get("scale").and_then(|s| s.as_f64()))
            .unwrap_or(0.35);
        out.push(StickerLayerInfo {
            path,
            start_ms: evt.start_ms,
            end_ms: evt.end_ms,
            position,
            scale,
        });
    }
    out
}

fn build_composition_audit(
    timeline: &Timeline,
    manifest: &openscript_core::production_quality::RenderManifest,
) -> serde_json::Value {
    // Bottom-to-top z-order of the render pipeline (multilayer_render.rs):
    // 1. Background concat [vbg]
    // 2. Meme b-roll overlays [vmb*]
    // 3. Captions burned on top [vcap]
    // 4. Sticker overlays on top [vst*]
    // Audio layers (voiceover, music, sfx) sit below/alongside in the mix.
    let mut layers: Vec<serde_json::Value> = Vec::new();

    // 1. B-roll / background bed (bottom-most video layer)
    let broll_events = timeline.tracks.get(&TrackType::Broll).cloned().unwrap_or_default();
    let broll_start = broll_events.first().map(|e| e.start_ms).unwrap_or(0);
    let broll_end = broll_events.iter().map(|e| e.end_ms).max().unwrap_or(0);
    layers.push(json!({
        "z": 1,
        "layer": "background_broll",
        "present": !broll_events.is_empty(),
        "count": broll_events.len(),
        "range_ms": [broll_start, broll_end],
        "note": if broll_events.is_empty() { "no b-roll bed — empty visual background".to_string() } else { format!("{} b-roll clips, bottom of video stack", broll_events.len()) },
    }));

    // 2. Meme overlays (if configured)
    let meme_start = manifest.memes.first().map(|m| m.start_ms).unwrap_or(0);
    let meme_end = manifest.memes.iter().map(|m| m.end_ms).max().unwrap_or(0);
    layers.push(json!({
        "z": 2,
        "layer": "meme_overlay",
        "present": !manifest.memes.is_empty(),
        "count": manifest.memes.len(),
        "range_ms": [meme_start, meme_end],
        "note": if manifest.memes.is_empty() { "no meme overlays".to_string() } else { format!("{} meme overlays above b-roll", manifest.memes.len()) },
    }));

    // 3. Captions
    let captions_present = manifest
        .captions_path
        .as_deref()
        .map(|p| !p.is_empty())
        .unwrap_or(false);
    let caption_events = timeline.tracks.get(&TrackType::Captions).cloned().unwrap_or_default();
    let cap_start = caption_events.first().map(|e| e.start_ms).unwrap_or(0);
    let cap_end = caption_events.iter().map(|e| e.end_ms).max().unwrap_or(0);
    layers.push(json!({
        "z": 3,
        "layer": "captions",
        "present": captions_present || !caption_events.is_empty(),
        "count": caption_events.len(),
        "range_ms": [cap_start, cap_end],
        "path": manifest.captions_path.clone().unwrap_or_default(),
        "style": manifest.caption_style.clone().unwrap_or_else(|| "default".into()),
        "note": if !captions_present && caption_events.is_empty() { "NO captions configured or on timeline — dialogue will be un-captioned".to_string() } else { format!("{} caption events, style: {}", caption_events.len(), manifest.caption_style.as_deref().unwrap_or("default")) },
    }));

    // 4. Stickers (topmost video layer). Fall back to Stickers-track events
    // when the manifest has none, so agent-placed stickers are reported.
    let timeline_stickers = stickers_from_timeline(timeline);
    let sticker_events: &[openscript_core::production_quality::StickerLayerInfo] =
        if !manifest.stickers.is_empty() {
            &manifest.stickers
        } else {
            &timeline_stickers
        };
    let sticker_start = sticker_events.first().map(|s| s.start_ms).unwrap_or(0);
    let sticker_end = sticker_events.iter().map(|s| s.end_ms).max().unwrap_or(0);
    layers.push(json!({
        "z": 4,
        "layer": "stickers",
        "present": !sticker_events.is_empty(),
        "count": sticker_events.len(),
        "range_ms": [sticker_start, sticker_end],
        "note": if sticker_events.is_empty() { "no sticker overlays".to_string() } else { format!("{} stickers, topmost video layer", sticker_events.len()) },
    }));

    // Audio layers (mix, not z-order — listed after video stack)
    let voiceover_events = timeline.tracks.get(&TrackType::Voiceover).cloned().unwrap_or_default();
    let dialogue_events = timeline.tracks.get(&TrackType::Dialogue).cloned().unwrap_or_default();
    let music_events = timeline.tracks.get(&TrackType::Music).cloned().unwrap_or_default();
    let sfx_events = timeline.tracks.get(&TrackType::Sfx).cloned().unwrap_or_default();
    let vo_start = voiceover_events.first().map(|e| e.start_ms).or_else(|| dialogue_events.first().map(|e| e.start_ms)).unwrap_or(0);
    let vo_end = voiceover_events.iter().chain(dialogue_events.iter()).map(|e| e.end_ms).max().unwrap_or(0);
    layers.push(json!({
        "z": 5,
        "layer": "voiceover",
        "present": !voiceover_events.is_empty() || !dialogue_events.is_empty(),
        "count": voiceover_events.len() + dialogue_events.len(),
        "range_ms": [vo_start, vo_end],
        "note": if voiceover_events.is_empty() && dialogue_events.is_empty() { "NO voiceover/dialogue events — silent video".to_string() } else { format!("{} voiceover + {} dialogue events", voiceover_events.len(), dialogue_events.len()) },
    }));
    let music_start = music_events.first().map(|e| e.start_ms).unwrap_or(0);
    let music_end = music_events.iter().map(|e| e.end_ms).max().unwrap_or(0);
    let music_present = !music_events.is_empty() || manifest.music.is_some();
    layers.push(json!({
        "z": 6,
        "layer": "music",
        "present": music_present,
        "count": music_events.len(),
        "range_ms": [music_start, music_end],
        "path": manifest.music.as_ref().map(|m| m.path.clone()).unwrap_or_default(),
        "note": if !music_present { "NO music layer — bed is silent".to_string() } else { format!("{} music events{}", music_events.len(), if manifest.music.is_some() { " + manifest music".to_string() } else { String::new() }) },
    }));
    let sfx_start = sfx_events.first().map(|e| e.start_ms).unwrap_or(0);
    let sfx_end = sfx_events.iter().map(|e| e.end_ms).max().unwrap_or(0);
    layers.push(json!({
        "z": 7,
        "layer": "sfx",
        "present": !sfx_events.is_empty(),
        "count": sfx_events.len(),
        "range_ms": [sfx_start, sfx_end],
        "note": if sfx_events.is_empty() { "no SFX events".to_string() } else { format!("{} SFX events", sfx_events.len()) },
    }));

    // Convenience summary: present layer names in z-order, and any MISSING
    // layers that the production model would expect for this video.
    let present: Vec<String> = layers
        .iter()
        .filter(|l| l.get("present").and_then(|v| v.as_bool()).unwrap_or(false))
        .map(|l| l.get("layer").and_then(|v| v.as_str()).unwrap_or("").to_string())
        .collect();
    let mut missing: Vec<String> = Vec::new();
    if broll_events.is_empty() {
        missing.push("background_broll".into());
    }
    if !captions_present && caption_events.is_empty() {
        missing.push("captions".into());
    }
    if !music_present {
        missing.push("music".into());
    }
    if voiceover_events.is_empty() && dialogue_events.is_empty() && manifest.voiceover_count == 0 {
        missing.push("voiceover".into());
    }

    json!({
        "layer_count": layers.len(),
        "layers": layers,
        "present_order": present,
        "missing": missing,
        "source": "composition audit — z-order per multilayer_render.rs; derived from the timeline + render manifest (planned composition), NOT frame-level inspection. Combine with per_clip_motion / broll_gaps to confirm what actually rendered.",
    })
}



/// Download a short stock clip via yt-dlp (no API key). Used when Pexels is unavailable.
/// Result of a unique stock fetch (path + identity for variance tracking).
pub(crate) struct StockClipFetch {
    pub(crate) path: String,
    pub(crate) video_id: String,
    pub(crate) content_hash: String,
    pub(crate) search_query: String,
    pub(crate) lexical_score: f64,
    pub(crate) source_title: String,
    /// L3 vision gate: 0–1 relevance of the ACTUAL extracted frame vs the
    /// scene, when a vision backend was available. None = gate skipped/failed.
    pub(crate) vision_score: Option<f64>,
    /// Short justification from the vision model (why it matched/mismatched).
    pub(crate) vision_reason: Option<String>,
}

pub(crate) fn file_content_fingerprint(path: &str) -> Option<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; 256 * 1024];
    let n = f.read(&mut buf).ok()?;
    if n == 0 {
        return None;
    }
    // FNV-1a 64-bit over first 256KiB — enough to catch identical YT re-downloads
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in &buf[..n] {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    // Include file size so different trims of same source still collide on full source
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    Some(format!("{:016x}_{}", hash, size))
}

/// YouTube search returning `YtCandidate`s with duration + thumbnail (L0).
/// Uses `--dump-json` (like stock_pool::yt_search_entries) so the L1 duration
/// preference and L2 thumbnail vision gate have the metadata they need.
async fn youtube_search_candidates(query: &str, limit: usize) -> Vec<crate::stock_signal::YtCandidate> {
    // Retry once with backoff: YouTube transiently throttles the search
    // endpoint during multi-scene bursts (same bot-detection family as the
    // download 403s). A silent empty result otherwise falls the scene to
    // procedural even though the query is valid.
    for attempt in 0..2 {
        let out = tokio::process::Command::new("yt-dlp")
            .args([
                "--flat-playlist",
                "--dump-json",
                "--no-warnings",
                "--quiet",
                "--socket-timeout",
                "25",
                &format!("ytsearch{}:{}", limit, query),
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .output()
            .await;
        let err_tail = out
            .as_ref()
            .map(|o| {
                String::from_utf8_lossy(&o.stderr)
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .rev()
                    .take(3)
                    .collect::<Vec<_>>()
                    .join(" | ")
            })
            .unwrap_or_default();
        match out {
            Ok(o) if o.status.success() => {
                let parsed: Vec<crate::stock_signal::YtCandidate> = String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                    .filter_map(|d| {
                        let id = d.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
                        let title = d.get("title").and_then(|x| x.as_str()).unwrap_or("").to_string();
                        if id.is_empty() || title.is_empty() {
                            return None;
                        }
                        let duration_s = d.get("duration").and_then(|x| x.as_f64()).unwrap_or(0.0);
                        let thumbnail_url = d
                            .get("thumbnail")
                            .and_then(|x| x.as_str())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", id));
                        Some(crate::stock_signal::YtCandidate {
                            id,
                            title,
                            duration_s,
                            thumbnail_url,
                        })
                    })
                    .collect();
                if !parsed.is_empty() {
                    return parsed;
                }
            }
            _ => {}
        }
        tracing::warn!(
            "[youtube stock] search failed attempt {} for query='{}' err={} — retrying",
            attempt + 1,
            query,
            err_tail
        );
        tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
    }
    Vec::new()
}

/// Fetch a stock clip that is content-unique **and** context-relevant.
///
/// Signal/noise gates (Phase CM):
/// 1. Prefer titles that lexically match scene signal tokens
/// 2. Unique video id + content hash
/// 3. Cover-crop with `setsar=1` (no stretch)
/// 4. Geometry probe rejects non-square SAR / wrong display aspect
async fn fetch_youtube_stock_clip_unique(
    query: &str,
    duration_s: f64,
    aspect: &str,
    out_path: &str,
    scene_idx: usize,
    used_video_ids: &mut std::collections::HashSet<String>,
    used_content_hashes: &mut std::collections::HashSet<String>,
) -> Option<StockClipFetch> {
    fetch_youtube_stock_clip_signal(
        query,
        &[],
        duration_s,
        aspect,
        out_path,
        scene_idx,
        used_video_ids,
        used_content_hashes,
        "",
        0.0,
        0.0,
    )
    .await
}

/// Threshold below which a vision-gated YouTube candidate is rejected.
/// Env override: OPENSCRIPT_YT_VISION_MIN_MATCH (0.0–1.0).
fn yt_vision_min_match() -> f64 {
    std::env::var("OPENSCRIPT_YT_VISION_MIN_MATCH")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.4)
}

/// Whether the L2/L3 vision gates are enabled. Default ON when a vision
/// backend is configured; disable entirely with OPENSCRIPT_YT_VISION_GATE=0.
fn yt_vision_gate_enabled() -> bool {
    if std::env::var("OPENSCRIPT_YT_VISION_GATE")
        .map(|v| v == "0" || v.eq_ignore_ascii_case("false"))
        .unwrap_or(false)
    {
        return false;
    }
    // Vision backend availability: openrouter key OR opencode key configured.
    !crate::config::resolve_api_key("openrouter").is_empty()
        || !crate::config::resolve_opencode_api_key().is_empty()
}

/// Parse `{score:{relevance,match,reason}}` from a vision call into an
/// (relevance, matched, reason) triple. Fail-open: unparseable → treated as
/// "gate passed" so a vision hiccup never blocks the pipeline.
fn parse_vision_score(v: &serde_json::Value) -> (f64, bool, Option<String>) {
    let score = v.get("score").unwrap_or(v);
    let relevance = score
        .get("relevance")
        .and_then(|x| x.as_f64())
        .unwrap_or(1.0);
    let matched = score
        .get("match")
        .and_then(|x| x.as_bool())
        .unwrap_or(true);
    let reason = score
        .get("reason")
        .and_then(|x| x.as_str())
        .map(String::from);
    (relevance, matched, reason)
}

/// Download a small stock image (YouTube thumbnail) to a temp file.
async fn download_thumbnail(url: &str, dest: &str) -> bool {
    let Ok(client) = reqwest::Client::builder()
        .user_agent("OpenScript/1.0 (+https://github.com/ishan-parihar/openscript)")
        .timeout(std::time::Duration::from_secs(15))
        .build()
    else {
        return false;
    };
    match client.get(url).send().await {
        Ok(resp) if resp.status().is_success() => match resp.bytes().await {
            Ok(bytes) => {
                if bytes.len() < 200 {
                    return false;
                }
                std::fs::write(dest, &bytes).is_ok()
            }
            _ => false,
        },
        _ => false,
    }
}

/// Full signal-aware YouTube stock fetch — 4-layer upgrade:
/// L0: yt-dlp `--dump-json` (id, title, duration, thumbnail)
/// L1: duration-preference ranking (lectures penalized) + duration bounds
/// L2: thumbnail vision pre-filter BEFORE the full download
/// L3: post-trim frame vision gate (verifies the actual extracted pixels)
/// plus the pre-existing lexical gate, content-hash dedup, cover-crop,
/// geometry gate.
pub(crate) async fn fetch_youtube_stock_clip_signal(
    query: &str,
    signal_tokens: &[String],
    duration_s: f64,
    aspect: &str,
    out_path: &str,
    scene_idx: usize,
    used_video_ids: &mut std::collections::HashSet<String>,
    used_content_hashes: &mut std::collections::HashSet<String>,
    scene_text: &str,
    min_duration_s: f64,
    max_duration_s: f64,
) -> Option<StockClipFetch> {
    let cache_dir = "mcp/assets/background_cache";
    std::fs::create_dir_all(cache_dir).ok()?;

    let diversified = query.to_string();
    let mut candidates = youtube_search_candidates(&diversified, 12).await;
    if candidates.is_empty() {
        // Fallback: shorter query (first 6 tokens)
        let short: String = query
            .split_whitespace()
            .take(6)
            .collect::<Vec<_>>()
            .join(" ");
        candidates = youtube_search_candidates(&short, 10).await;
    }

    // Drop already-used IDs
    candidates.retain(|c| !used_video_ids.contains(&c.id));
    if candidates.is_empty() {
        tracing::warn!(
            "[youtube stock] no unused video IDs for query='{}'",
            diversified
        );
        return None;
    }

    let signal: Vec<String> = if signal_tokens.is_empty() {
        crate::stock_signal::tokenize(&diversified)
    } else {
        signal_tokens.to_vec()
    };
    let min_lex = crate::stock_signal::min_lexical_accept();
    let vision = yt_vision_gate_enabled();
    let min_vision = yt_vision_min_match();
    let ranked = crate::stock_signal::rank_yt_candidates(
        &candidates,
        &signal,
        min_lex,
        min_duration_s,
        max_duration_s,
    );
    tracing::info!(
        "[youtube stock] ranked {} candidates (min_lex={:.2} vision={}) top={}",
        ranked.len(),
        min_lex,
        vision,
        ranked
            .first()
            .map(|c| format!("{}:{:.2}:{}s:{}", c.id, c.lexical, c.duration_s as i64, truncate_utf8(&c.title, 40)))
            .unwrap_or_else(|| "none".into())
    );
    if ranked.is_empty() && !candidates.is_empty() {
        // Diagnostic: WHY did ranking reject everything? Surface the raw
        // candidate pool (id, duration, title) + the active duration bounds
        // so a zero-result is attributable to the query, the gate, or the
        // signal — not silently blamed on the search.
        let raw: Vec<String> = candidates
            .iter()
            .take(10)
            .map(|c| format!("{}:{}s:{}", c.id, c.duration_s as i64, truncate_utf8(&c.title, 45)))
            .collect();
        tracing::warn!(
            "[youtube stock] ranked 0 from {} raw candidates (min_dur={:.1}s max_dur={:.1}s min_lex={:.2}) raw={:?}",
            candidates.len(),
            min_duration_s,
            max_duration_s,
            min_lex,
            raw
        );
    }

    let scene_kw: Vec<String> = signal.clone();
    let gpu = GpuConfig::resolve();
    for cand in ranked.into_iter().take(8) {
        let video_id = cand.id.clone();
        let title = cand.title.clone();
        let lex = cand.lexical;

        // L2: thumbnail vision pre-filter (cheap — ~10 KB download + 1 vision
        // call). Rejects lecture/thumbnail-bait candidates before the full
        // video is downloaded. Fail-open on any error.
        let mut thumb_ok = true;
        if vision && !cand.thumbnail_url.is_empty() {
            let thumb_path = format!("{}/yt_thumb_{}.jpg", cache_dir, video_id);
            if download_thumbnail(&cand.thumbnail_url, &thumb_path).await {
                match crate::llm::score_image_relevance(
                    &thumb_path,
                    if scene_text.is_empty() { diversified.as_str() } else { scene_text },
                    &scene_kw,
                    Some(&diversified),
                )
                .await
                {
                    Ok(v) => {
                        let (rel, matched, reason) = parse_vision_score(&v);
                        if !matched || rel < min_vision {
                            thumb_ok = false;
                            let reject_msg = format!(
                                "thumbnail reject rel={:.2} matched={} {}",
                                rel,
                                matched,
                                reason.unwrap_or_default()
                            );
                            tracing::info!(
                                "[youtube stock] {} id={} title='{}'",
                                reject_msg,
                                video_id,
                                truncate_utf8(&title, 50)
                            );
                            used_video_ids.insert(video_id.clone());
                        } else {
                            tracing::info!(
                                "[youtube stock] thumbnail PASS rel={:.2} id={} title='{}'",
                                rel,
                                video_id,
                                truncate_utf8(&title, 50)
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            "[youtube stock] thumbnail vision error id={} (fail-open): {}",
                            video_id,
                            e
                        );
                    }
                }
                let _ = std::fs::remove_file(&thumb_path);
            }
        }
        if !thumb_ok {
            continue;
        }

        let full_path = format!("{}/yt_id_{}.mp4", cache_dir, video_id);
        if !Path::new(&full_path).exists() {
            let url = format!("https://www.youtube.com/watch?v={}", video_id);
            // Video-only preferred (avoid audio-only / music streams ranked as "stock").
            // Cover-crop later handles landscape→portrait cleanly.
            let mut yt_cmd = tokio::process::Command::new("yt-dlp");
            yt_cmd.args([
                    "--format",
                    "bestvideo[height<=720][ext=mp4]+bestaudio/bestvideo[height<=720]+bestaudio/bestvideo[height<=720][ext=mp4]/bestvideo[height<=720]/best[vcodec!=none][height<=720]/best[vcodec!=none]/best",
                    "--merge-output-format",
                    "mp4",
                    "--output",
                    &full_path,
                    "--no-playlist",
                    "--quiet",
                    "--no-warnings",
                    "--socket-timeout",
                    "25",
                    "--retries",
                    "4",
                    "--retry-sleep",
                    "3",
                    "--extractor-retries",
                    "3",
                ]);
            if let Ok(cookies) = std::env::var("OPENSCRIPT_YT_COOKIES") {
                if !cookies.is_empty() && std::path::Path::new(&cookies).exists() {
                    yt_cmd.args(["--cookies", &cookies]);
                }
            }
            yt_cmd.arg(&url);
            let yt = yt_cmd
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .output()
                .await
                .ok();
            let ok = yt
                .as_ref()
                .map(|o| o.status.success() && Path::new(&full_path).exists())
                .unwrap_or(false);
            if !ok {
                let err_tail = yt
                    .as_ref()
                    .map(|o| {
                        String::from_utf8_lossy(&o.stderr)
                            .lines()
                            .filter(|l| !l.trim().is_empty())
                            .rev()
                            .take(4)
                            .collect::<Vec<_>>()
                            .join(" | ")
                    })
                    .unwrap_or_default();
                tracing::warn!(
                    "[youtube stock] download failed id={} title='{}' q={} err={}",
                    video_id,
                    truncate_utf8(&title, 50),
                    diversified,
                    err_tail
                );
                tokio::time::sleep(std::time::Duration::from_millis(900)).await;
                continue;
            }
        }

        let content_hash = match file_content_fingerprint(&full_path) {
            Some(h) => h,
            None => continue,
        };
        if used_content_hashes.contains(&content_hash) {
            tracing::info!(
                "[youtube stock] skip duplicate content hash {} (id={})",
                content_hash,
                video_id
            );
            used_video_ids.insert(video_id);
            continue;
        }

        // Cover-crop (no stretch) + square SAR
        let start_s = 1.5 + (scene_idx as f64) * 2.7;
        let crop = crop_filter_for_aspect(aspect);
        let trim = build_stock_trim_command(
            &gpu,
            &full_path,
            out_path,
            duration_s.max(2.0),
            Some(start_s),
            &crop,
        )
        .output()
        .await
        .ok()?;
        if !trim.status.success() || !Path::new(out_path).exists() {
            tracing::warn!(
                "[youtube stock] trim FAILED id={video_id} — skipped. ffmpeg: {}",
                trim_stderr_tail(&trim)
            );
            continue;
        }

        // Geometry gate: reject stretch / wrong display aspect
        let geo = crate::stock_signal::probe_geometry(out_path, aspect);
        if !geo.ok {
            tracing::warn!(
                "[youtube stock] geometry reject id={} reasons={:?} {}x{} sar={}:{}",
                video_id,
                geo.reasons,
                geo.width,
                geo.height,
                geo.sar_num,
                geo.sar_den
            );
            let _ = std::fs::remove_file(out_path);
            // Don't permanently burn the id — source may re-encode cleanly later;
            // still mark used to avoid tight loops on same bad file.
            used_video_ids.insert(video_id);
            continue;
        }

        // L3: frame vision gate — verify the ACTUAL extracted pixels match the
        // scene (the user-reported failure: "initial seconds don't depict what
        // is required"). Score the trimmed clip at its first second (0.4s in
        // — the cover-crop output starts at `start_s` of the source). Fail-open
        // on vision errors so a transient backend issue never blocks renders.
        let mut vision_score: Option<f64> = None;
        let mut vision_reason: Option<String> = None;
        if vision {
            match crate::llm::score_clip_relevance_at(
                out_path,
                Some(0.4),
                if scene_text.is_empty() { diversified.as_str() } else { scene_text },
                &scene_kw,
                Some(&diversified),
            )
            .await
            {
                Ok(v) => {
                    let (rel, matched, reason) = parse_vision_score(&v);
                    if !matched || rel < min_vision {
                        tracing::info!(
                            "[youtube stock] FRAME REJECT id={} rel={:.2} matched={} {}",
                            video_id,
                            rel,
                            matched,
                            reason.clone().unwrap_or_default()
                        );
                        let _ = std::fs::remove_file(out_path);
                        used_video_ids.insert(video_id);
                        continue;
                    }
                    vision_score = Some(rel);
                    vision_reason = reason;
                    tracing::info!(
                        "[youtube stock] frame PASS rel={:.2} id={} title='{}'",
                        rel,
                        video_id,
                        truncate_utf8(&title, 50)
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        "[youtube stock] frame vision error id={} (fail-open): {}",
                        video_id,
                        e
                    );
                }
            }
        }

        let out_hash = file_content_fingerprint(out_path).unwrap_or_else(|| content_hash.clone());
        if used_content_hashes.contains(&out_hash) {
            let _ = std::fs::remove_file(out_path);
            used_video_ids.insert(video_id);
            continue;
        }

        used_video_ids.insert(video_id.clone());
        used_content_hashes.insert(content_hash.clone());
        used_content_hashes.insert(out_hash.clone());
        tracing::info!(
            "[youtube stock] ACCEPT id={} lex={:.2} vision={:?} hash={} title='{}' query='{}' -> {}",
            video_id,
            lex,
            vision_score,
            &out_hash[..out_hash.len().min(20)],
            truncate_utf8(&title, 50),
            diversified,
            out_path
        );
        return Some(StockClipFetch {
            path: out_path.to_string(),
            video_id,
            content_hash: out_hash,
            search_query: format!("{} | title={}", diversified, title),
            lexical_score: lex,
            source_title: title,
            vision_score,
            vision_reason,
        });
    }
    None
}

/// Pixabay stock-footage path — mirrors `fetch_youtube_stock_clip_signal` but
/// via the Pixabay video API (`video_type=film` — real footage, NOT
/// `video_type=animation`, which returns motion graphics useless as b-roll;
/// see docs/MEDIA_SEARCH_AUDIT.md §4).
///
/// Flow: film search → stock_signal lexical gate → HTTP download of the best
/// file → cover-crop (setsar=1) → geometry gate → content-hash dedup. Returns
/// the same `StockClipFetch` contract as the YouTube path.
pub(crate) async fn fetch_pixabay_stock_clip_signal(
    query: &str,
    signal_tokens: &[String],
    duration_s: f64,
    min_duration_s: f64,
    max_duration_s: f64,
    aspect: &str,
    out_path: &str,
    used_video_ids: &mut std::collections::HashSet<String>,
    used_content_hashes: &mut std::collections::HashSet<String>,
) -> Option<StockClipFetch> {
    let cache_dir = "mcp/assets/background_cache";
    std::fs::create_dir_all(cache_dir).ok()?;

    let key = pixabay_key();
    if key.is_empty() {
        return None;
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .ok()?;

    // Film footage only — per_page 30 gives the ranker a decent pool to pick
    // from. Pixabay's API caps per_page at 200; 30 keeps the payload small.
    let url = format!(
        "https://pixabay.com/api/videos/?key={}&q={}&per_page=30&video_type=film",
        key,
        urlencoding::encode(query)
    );
    let body: serde_json::Value = match client.get(&url).send().await {
        Ok(resp) if resp.status().is_success() => match resp.json().await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("[pixabay stock] parse error: {}", e);
                return None;
            }
        },
        Ok(resp) => {
            tracing::warn!("[pixabay stock] API status {}", resp.status());
            return None;
        }
        Err(e) => {
            tracing::warn!("[pixabay stock] request failed: {}", e);
            return None;
        }
    };

    let hits = match body.get("hits").and_then(|v| v.as_array()) {
        Some(arr) if !arr.is_empty() => arr.clone(),
        _ => {
            tracing::warn!("[pixabay stock] no hits for query='{}'", query);
            return None;
        }
    };

    // Build (id, title) candidate pairs. Pixabay's `tags` field acts as the
    // title for lexical ranking; hit id is the dedup key.
    let mut candidates: Vec<(String, String)> = Vec::new();
    for h in &hits {
        let id = h.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
        if id == 0 {
            continue;
        }
        let id_s = id.to_string();
        if used_video_ids.contains(&id_s) {
            continue;
        }
        // SEGMENTATION_ARCHITECTURE min/max clip duration: skip clips shorter
        // than the request (they'd force looping) and clips beyond an explicit
        // max. 0 = no bound. Mirrors the Pexels priority's API filters.
        let dur = h.get("duration").and_then(|v| v.as_f64()).unwrap_or(0.0);
        if dur > 0.0 && dur < duration_s.max(min_duration_s) {
            continue;
        }
        if max_duration_s > 0.0 && dur > max_duration_s {
            continue;
        }
        let title = h
            .get("tags")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if title.is_empty() {
            continue;
        }
        candidates.push((id_s, title));
    }
    if candidates.is_empty() {
        tracing::warn!(
            "[pixabay stock] no unused film candidates for query='{}'",
            query
        );
        return None;
    }

    let signal: Vec<String> = if signal_tokens.is_empty() {
        crate::stock_signal::tokenize(query)
    } else {
        signal_tokens.to_vec()
    };
    let min_lex = crate::stock_signal::min_lexical_accept();
    let ranked = crate::stock_signal::rank_and_filter_candidates(&candidates, &signal, min_lex);
    tracing::info!(
        "[pixabay stock] ranked {} candidates (min_lex={:.2}) top={}",
        ranked.len(),
        min_lex,
        ranked
            .first()
            .map(|c| format!("{}:{:.2}:{}", c.id, c.lexical, truncate_utf8(&c.title, 40)))
            .unwrap_or_else(|| "none".into())
    );
    let gpu = GpuConfig::resolve();

    for cand in ranked.into_iter().take(8) {
        let video_id = cand.id.clone();
        let title = cand.title.clone();
        let lex = cand.lexical;

        // Re-locate the hit to grab its direct file URLs.
        let hit = match hits.iter().find(|h| {
            h.get("id")
                .and_then(|v| v.as_u64())
                .map(|i| i.to_string())
                .as_deref()
                == Some(video_id.as_str())
        }) {
            Some(h) => h,
            None => continue,
        };
        let direct_url = hit
            .get("videos")
            .and_then(|v| v.get("large"))
            .or_else(|| hit.get("videos").and_then(|v| v.get("medium")))
            .or_else(|| hit.get("videos").and_then(|v| v.get("small")))
            .and_then(|q| q.get("url"))
            .and_then(|u| u.as_str())
            .unwrap_or("")
            .to_string();
        if direct_url.is_empty() {
            tracing::warn!(
                "[pixabay stock] no direct url id={} title='{}'",
                video_id,
                truncate_utf8(&title, 50)
            );
            continue;
        }

        let full_path = format!("{}/pixabay_id_{}.mp4", cache_dir, video_id);
        if !Path::new(&full_path).exists() {
            match client.get(&direct_url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(bytes) = resp.bytes().await {
                        if std::fs::write(&full_path, &bytes).is_err() {
                            tracing::warn!("[pixabay stock] write failed id={}", video_id);
                            continue;
                        }
                    } else {
                        continue;
                    }
                }
                _ => {
                    tracing::warn!(
                        "[pixabay stock] download failed id={} title='{}' q={}",
                        video_id,
                        truncate_utf8(&title, 50),
                        query
                    );
                    continue;
                }
            }
        }

        let content_hash = match file_content_fingerprint(&full_path) {
            Some(h) => h,
            None => continue,
        };
        if used_content_hashes.contains(&content_hash) {
            tracing::info!(
                "[pixabay stock] skip duplicate content hash {} (id={})",
                content_hash,
                video_id
            );
            used_video_ids.insert(video_id);
            continue;
        }

        // Cover-crop (no stretch) + square SAR. Pixabay clips are stock loops;
        // start at 0 (no channel-intro skip needed like YouTube).
        let crop = crop_filter_for_aspect(aspect);
        let trim = build_stock_trim_command(
            &gpu,
            &full_path,
            out_path,
            duration_s.max(2.0),
            None,
            &crop,
        )
        .output()
        .await
        .ok()?;
        if !trim.status.success() || !Path::new(out_path).exists() {
            tracing::warn!(
                "[pixabay stock] trim FAILED id={video_id} — skipped. ffmpeg: {}",
                trim_stderr_tail(&trim)
            );
            continue;
        }

        // Geometry gate: reject stretch / wrong display aspect
        let geo = crate::stock_signal::probe_geometry(out_path, aspect);
        if !geo.ok {
            tracing::warn!(
                "[pixabay stock] geometry reject id={} reasons={:?} {}x{} sar={}:{}",
                video_id,
                geo.reasons,
                geo.width,
                geo.height,
                geo.sar_num,
                geo.sar_den
            );
            let _ = std::fs::remove_file(out_path);
            used_video_ids.insert(video_id);
            continue;
        }

        let out_hash = file_content_fingerprint(out_path).unwrap_or_else(|| content_hash.clone());
        if used_content_hashes.contains(&out_hash) {
            let _ = std::fs::remove_file(out_path);
            used_video_ids.insert(video_id);
            continue;
        }

        used_video_ids.insert(video_id.clone());
        used_content_hashes.insert(content_hash.clone());
        used_content_hashes.insert(out_hash.clone());
        tracing::info!(
            "[pixabay stock] ACCEPT id={} lex={:.2} hash={} title='{}' query='{}' -> {}",
            video_id,
            lex,
            &out_hash[..out_hash.len().min(20)],
            truncate_utf8(&title, 50),
            query,
            out_path
        );
        return Some(StockClipFetch {
            path: out_path.to_string(),
            video_id,
            content_hash: out_hash,
            search_query: format!("{} | title={}", query, title),
            lexical_score: lex,
            source_title: title,
            vision_score: None,
            vision_reason: None,
        });
    }
    None
}

/// Backward-compatible wrapper (no uniqueness set). Kept for call sites that
/// do not track identity sets (e.g. one-off background.fetch without multi-broll).
#[allow(dead_code)]
async fn fetch_youtube_stock_clip(
    query: &str,
    duration_s: f64,
    aspect: &str,
    out_path: &str,
) -> Option<String> {
    let mut ids = std::collections::HashSet::new();
    let mut hashes = std::collections::HashSet::new();
    fetch_youtube_stock_clip_unique(query, duration_s, aspect, out_path, 0, &mut ids, &mut hashes)
        .await
        .map(|f| f.path)
}

// ---------------------------------------------------------------------------
// Handler: reelize.brief
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: reelize.direct
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: script.parse — from-scratch video creation script parser
// ---------------------------------------------------------------------------



// ---------------------------------------------------------------------------
// Handler: script.generate_voices — TTS per scene
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: script.build_captions — ASS generation from word timings
// ---------------------------------------------------------------------------

/// Remap ASR-aligned word timings so the caption TEXT is the script's own
/// words (ground truth) while keeping the alignment's real timing windows.
///
/// Parakeet force-aligns by transcribing the TTS audio, and ASR can mis-hear
/// a cloned voice (e.g. "bias" → "pie"). Burning the transcription would put
/// wrong words on screen while the audio says the right ones. When the
/// aligned word count matches the script word count, keep the timings but
/// substitute the script words. On any mismatch (dropped/merged/mangled
/// words) fall back to char-proportional estimation over the segment window.
/// When the ASR returned MORE words than the script (whisper commonly appends a
/// trailing hallucinated token, or prepends a filler word), try trimming the
/// excess from the tail or the head and check the remaining text still matches
/// the script closely (Jaccard ≥ 0.5). If it does, keep the REAL timing windows
/// overridden with the script's words instead of collapsing to estimates.
/// Returns None when the count excess is >3 or the trimmed text diverges.
fn try_trim_align_to_script(
    timed: &[WordTiming],
    script_words: &[&str],
) -> Option<Vec<WordTiming>> {
    let excess = timed.len().checked_sub(script_words.len())?;
    if excess == 0 || excess > 3 || script_words.is_empty() {
        return None;
    }
    let script_text = script_words.join(" ");
    let tail_cut = &timed[..timed.len() - excess];
    let head_cut = &timed[excess..];
    let text_of = |slice: &[WordTiming]| -> String {
        slice
            .iter()
            .map(|w| w.word.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let tail_sim = caption_text_similarity(&text_of(tail_cut), &script_text);
    let head_sim = caption_text_similarity(&text_of(head_cut), &script_text);
    let (best_slice, best_sim) = if tail_sim >= head_sim {
        (tail_cut, tail_sim)
    } else {
        (head_cut, head_sim)
    };
    if best_sim < 0.5 {
        return None;
    }
    Some(
        best_slice
            .iter()
            .zip(script_words.iter())
            .map(|(tw, sw)| WordTiming {
                word: sw.to_string(),
                start_ms: tw.start_ms,
                end_ms: tw.end_ms,
            })
            .collect(),
    )
}

fn remap_words_to_script(
    text: &str,
    timed: Vec<WordTiming>,
    start_ms: i64,
    end_ms: i64,
) -> Vec<WordTiming> {
    let script_words: Vec<&str> = text.split_whitespace().collect();
    if script_words.is_empty() {
        // No script text to remap against — passthrough the aligned words.
        return timed;
    }
    if timed.is_empty() || timed.len() != script_words.len() {
        // No alignment, or the ASR dropped/merged words — try trimming ASR
        // hallucinations first, then estimate timings from the script text so
        // captions tile the window with correct words.
        if !timed.is_empty() && timed.len() > script_words.len() {
            if let Some(trimmed) = try_trim_align_to_script(&timed, &script_words) {
                return trimmed;
            }
        }
        return estimate_word_timings(text, start_ms, end_ms);
    }
    timed.iter()
        .zip(script_words.iter())
        .map(|(tw, sw)| WordTiming {
            word: sw.to_string(),
            start_ms: tw.start_ms,
            end_ms: tw.end_ms,
        })
        .collect()
}


/// Helper: read script from inline JSON or file path.
fn read_script_input(script_input: &str) -> Result<String, ToolError> {
    if script_input.trim_start().starts_with('{') {
        Ok(script_input.to_string())
    } else {
        let path = sanitize_input_path(script_input)?;
        if !path.exists() {
            return Err(ToolError::NotFound(format!(
                "Script file not found: {}",
                path.display()
            )));
        }
        Ok(std::fs::read_to_string(&path)?)
    }
}

/// Run Parakeet TDT force alignment on a TTS WAV file to get accurate word timestamps.
/// Falls back to even-spacing estimation if Parakeet is unavailable.
///
/// This replaces the old whisper_align.py which depended on the `openai-whisper`
/// Python package. Parakeet TDT is a faster, more accurate RNN-T model that
/// runs via `onnxruntime` (no PyTorch dependency).
/// (Whisper→Parakeet migration per user directive.)
async fn run_parakeet_alignment(
    wav_path: &str,
    offset_ms: i64,
    _scene_end_ms: i64,
) -> Result<Vec<WordTiming>, String> {
    // Write alignment to a temp JSON file
    let tmp_json = format!("{}.align.json", wav_path);

    // Resolve Parakeet model paths CWD-independently
    let parakeet_dir = resolve_repo_path("mcp/assets/parakeet");
    let encoder_path = parakeet_dir.join("encoder-model.int8.onnx");
    let decoder_path = parakeet_dir.join("decoder_joint-model.int8.onnx");
    let vocab_path = parakeet_dir.join("vocab.txt");

    // Resolve the sidecar script path
    let sidecar_script = resolve_repo_path("mcp/scripts/parakeet_align.py");

    let output = tokio::process::Command::new("python3")
        .arg(&sidecar_script)
        .arg("--wav")
        .arg(wav_path)
        .arg("--output")
        .arg(&tmp_json)
        .arg("--encoder")
        .arg(&encoder_path)
        .arg("--decoder")
        .arg(&decoder_path)
        .arg("--vocab")
        .arg(&vocab_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| format!("Failed to spawn parakeet: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let _ = std::fs::remove_file(&tmp_json);
        return Err(format!(
            "Parakeet failed: {}",
            stderr.lines().last().unwrap_or("unknown")
        ));
    }

    // Read the alignment JSON
    let align_str = std::fs::read_to_string(&tmp_json)
        .map_err(|e| format!("Failed to read alignment: {}", e))?;
    let _ = std::fs::remove_file(&tmp_json);

    let align: serde_json::Value = serde_json::from_str(&align_str)
        .map_err(|e| format!("Failed to parse alignment JSON: {}", e))?;

    // Convert to WordTiming with offset
    let mut words = Vec::new();
    if let Some(word_arr) = align.get("words").and_then(|v| v.as_array()) {
        for w in word_arr {
            let word = w
                .get("word")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let start_ms = w.get("start_ms").and_then(|v| v.as_i64()).unwrap_or(0);
            let end_ms = w.get("end_ms").and_then(|v| v.as_i64()).unwrap_or(start_ms);

            if !word.is_empty() {
                words.push(WordTiming {
                    word,
                    start_ms: start_ms + offset_ms,
                    end_ms: end_ms + offset_ms,
                });
            }
        }
    }

    if words.is_empty() {
        return Err("Parakeet returned no words".to_string());
    }

    Ok(words)
}

/// Run Whisper word-timestamp alignment on a TTS WAV (multilingual — used for
/// Hinglish/Hindi scripts where the English-only Parakeet TDT model drifts and
/// collapses captions to even-spacing estimates). `openai-whisper` transcribes
/// with `word_timestamps=True` and the `language` hint keeps it on the right
/// language; the timing windows are the value — `remap_words_to_script`
/// overrides the word TEXT with the script's ground truth downstream, so ASR
/// word errors never reach the caption.
///
/// Fresh-process per call (model reload ~1s for `base`) is acceptable here:
/// only Hinglish/Hindi scripts route through Whisper, and the load is bounded
/// per scene. Errors are returned as strings and callers fall back to Parakeet
/// / estimation.
async fn run_whisper_alignment(
    wav_path: &str,
    text: &str,
    language: &str,
    offset_ms: i64,
    _scene_end_ms: i64,
) -> Result<Vec<WordTiming>, String> {
    let sidecar_script = resolve_repo_path("mcp/scripts/whisper_align.py");
    let tmp_json = format!("{}.whisper.align.json", wav_path);

    let output = tokio::process::Command::new("python3")
        .arg(&sidecar_script)
        .arg("--wav")
        .arg(wav_path)
        .arg("--text")
        .arg(text)
        .arg("--language")
        .arg(language)
        .arg("--model")
        .arg("base")
        .arg("--output")
        .arg(&tmp_json)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| format!("Failed to spawn whisper align: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let _ = std::fs::remove_file(&tmp_json);
        return Err(format!(
            "Whisper align failed: {}",
            stderr.lines().last().unwrap_or("unknown")
        ));
    }

    let align_str = std::fs::read_to_string(&tmp_json)
        .map_err(|e| format!("Failed to read whisper alignment: {}", e))?;
    let _ = std::fs::remove_file(&tmp_json);

    let align: serde_json::Value = serde_json::from_str(&align_str)
        .map_err(|e| format!("Failed to parse whisper alignment JSON: {}", e))?;
    if let Some(err) = align.get("error").and_then(|v| v.as_str()) {
        return Err(format!("Whisper align error: {}", err));
    }

    // Whisper returns seconds RELATIVE TO THE WAV (0-based); convert to ms and
    // shift onto the global timeline clock by `offset_ms` (the scene start) —
    // mirroring run_parakeet_alignment. Without this, count-matched words for
    // scenes after the first land at 0.. instead of their real positions.
    let mut words = Vec::new();
    if let Some(word_arr) = align.get("words").and_then(|v| v.as_array()) {
        for w in word_arr {
            let word = w
                .get("word")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let start_ms = w
                .get("start_s")
                .and_then(|v| v.as_f64())
                .map(|s| (s * 1000.0).round() as i64 + offset_ms)
                .unwrap_or(offset_ms);
            let end_ms = w
                .get("end_s")
                .and_then(|v| v.as_f64())
                .map(|s| (s * 1000.0).round() as i64 + offset_ms)
                .unwrap_or(start_ms);
            if !word.is_empty() && end_ms > start_ms {
                words.push(WordTiming {
                    word,
                    start_ms,
                    end_ms,
                });
            }
        }
    }

    if words.is_empty() {
        return Err("Whisper returned no word timings".to_string());
    }

    Ok(words)
}

// ---------------------------------------------------------------------------
// Handler: background.fetch — Pexels API (primary) + YouTube (fallback)
// ---------------------------------------------------------------------------


// Handler: background.assign — assign clips to scenes
// ---------------------------------------------------------------------------


/// Simple MD5 hash for cache keys (no external dep needed for this use case).
fn md5_hash(data: &[u8]) -> u128 {
    // Simple FNV-1a hash as a lightweight cache key (not cryptographic)
    let mut hash: u128 = 0x6c62272e07bb014262b821756295c58d;
    for &byte in data {
        hash ^= byte as u128;
        hash = hash.wrapping_mul(0x0000000001000000000000000000013b);
    }
    hash
}

/// Format seconds as HH:MM:SS for yt-dlp --download-sections
fn format_seconds_to_timestamp(s: f64) -> String {
    let total = s as u64;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
}

/// Auto-select a music track from the 20-track stock catalog based on theme.
/// Returns the path if a suitable track is found, None otherwise.
/// (Round-3 UX audit PROBLEM 3b fix — ensures every video has background
/// music by default, even when the agent doesn't specify music.path.)
/// Try to download a free/stock music bed via yt-dlp when placeholders are the only local option.
#[allow(dead_code)]
async fn fetch_youtube_music_bed(theme: &str) -> Option<String> {
    let query = match theme {
        "calm" => "lofi study focus chill no copyright music",
        "energetic" => "upbeat corporate positive no copyright music",
        _ => "ambient chill background no copyright music",
    };
    fetch_youtube_music_bed_query(query).await
}

async fn fetch_youtube_music_bed_query(query: &str) -> Option<String> {
    let cache_dir = "mcp/assets/music_cache";
    std::fs::create_dir_all(cache_dir).ok()?;
    let out_tmpl = format!("{}/yt_music_%(id)s.%(ext)s", cache_dir);
    // Constrain size/duration — unrestricted ytsearch can pull 800MB+ streams.
    let result = tokio::process::Command::new("yt-dlp")
        .args([
            "-x",
            "--audio-format",
            "mp3",
            "--audio-quality",
            "7",
            "-f",
            "bestaudio[filesize<12M]/bestaudio/best[filesize<12M]",
            "--max-filesize",
            "15M",
            "--no-playlist",
            "--quiet",
            "--no-warnings",
            "--socket-timeout",
            "20",
            "-o",
            &out_tmpl,
            &format!("ytsearch1:{}", query),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
        .ok()?;
    if !result.status.success() {
        return None;
    }
    // Pick newest mp3 in music_cache
    let mut best: Option<(std::time::SystemTime, String)> = None;
    if let Ok(entries) = std::fs::read_dir(cache_dir) {
        for e in entries.filter_map(|x| x.ok()) {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("mp3") {
                let modified = e
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                let ps = p.to_string_lossy().to_string();
                if best.as_ref().map(|(t, _)| modified > *t).unwrap_or(true) {
                    best = Some((modified, ps));
                }
            }
        }
    }
    best.map(|(_, p)| p)
}

/// Pick intro + cut SFX from the tagged sfx_index (editorial_role / tags).
fn auto_select_sfx_hits(scene_durations: &[f64]) -> Vec<openscript_ffmpeg::multilayer_render::SfxHit> {
    let mut hits = Vec::new();
    let index_path = resolve_repo_path("mcp/assets/sfx_index.json");
    let Ok(raw) = std::fs::read_to_string(&index_path) else {
        return hits;
    };
    let Ok(index) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return hits;
    };
    let assets = index
        .get("assets")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let sfx_root = index
        .get("sfx_path")
        .and_then(|v| v.as_str())
        .unwrap_or("mcp/assets/sfx_pack");
    let resolve_path = |p: &str| -> Option<String> {
        let pb = std::path::Path::new(p);
        if pb.exists() {
            return Some(p.to_string());
        }
        let via_repo = resolve_repo_path(p);
        if via_repo.exists() {
            return Some(via_repo.to_string_lossy().into());
        }
        // Relative under index sfx_path / OPENSCRIPT_SFX_PATH
        for base in [
            Some(sfx_root.to_string()),
            std::env::var("OPENSCRIPT_SFX_PATH").ok(),
            Some("mcp/assets/sfx_pack".into()),
        ]
        .into_iter()
        .flatten()
        {
            let base_pb = resolve_repo_path(&base);
            let cand = base_pb.join(p);
            if cand.exists() {
                return Some(cand.to_string_lossy().into());
            }
            if let Some(name) = pb.file_name() {
                let cand2 = base_pb.join(name);
                if cand2.exists() {
                    return Some(cand2.to_string_lossy().into());
                }
            }
        }
        None
    };

    // Collect all resolvable SFX paths tagged by role for rotation.
    let mut intro_sfx: Vec<String> = Vec::new();
    let mut transition_sfx: Vec<String> = Vec::new();
    for a in &assets {
        let er = a.get("editorial_role").and_then(|v| v.as_str()).unwrap_or("");
        let tags = a
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        let path = a.get("path").and_then(|v| v.as_str()).unwrap_or("");
        if path.is_empty() {
            continue;
        }
        let tag_blob = format!("{} {} {}", er, tags, path.to_lowercase());
        if let Some(rp) = resolve_path(path) {
            if er == "intro" || tag_blob.contains("rise") || tag_blob.contains("whoosh") && er != "transition" {
                intro_sfx.push(rp.clone());
            }
            if er == "transition" || tag_blob.contains("transition") {
                transition_sfx.push(rp);
            } else if tag_blob.contains("whoosh") || tag_blob.contains("swish") || tag_blob.contains("hit") {
                transition_sfx.push(intro_sfx.last().cloned().unwrap_or(rp));
            }
        }
    }
    // Deduplicate each pool while preserving order.
    intro_sfx.dedup();
    transition_sfx.dedup();

    // Intro: pick first, or fallback to first transition SFX.
    let intro = intro_sfx.first().or(transition_sfx.first()).cloned();
    if let Some(p) = intro {
        hits.push(openscript_ffmpeg::multilayer_render::SfxHit {
            path: p,
            start_s: 0.05,
            volume: 0.28,
        });
    }
    // Transitions: rotate through available SFX to avoid repetition.
    let mut t = 0.0;
    let mut sfx_idx = 0usize;
    for (i, d) in scene_durations.iter().enumerate() {
        if i == 0 {
            t += *d;
            continue;
        }
        let pool = if transition_sfx.is_empty() { &intro_sfx } else { &transition_sfx };
        if !pool.is_empty() {
            let p = pool[sfx_idx % pool.len()].clone();
            sfx_idx += 1;
            hits.push(openscript_ffmpeg::multilayer_render::SfxHit {
                path: p,
                start_s: (t - 0.05).max(0.0),
                volume: 0.22,
            });
        }
        t += *d;
    }
    hits
}

/// Selected music bed with provenance for KPI / denylist.
struct MusicSelection {
    path: String,
    #[allow(dead_code)]
    mood: String,
    tags: Vec<String>,
    selection_query: String,
    source: String,
}

/// Simple deterministic hash for shuffle ordering (no `rand` dependency).
/// Returns a u32 from a string — used to shuffle entries within a mood bucket
/// so the same video doesn't always get the same track.
fn deterministic_hash(s: &str) -> u32 {
    let mut h: u32 = 0x811c9dc5;
    for b in s.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    h
}

/// Extract tags array from a JSON entry.
fn entry_tags(entry: &serde_json::Value) -> Vec<String> {
    entry
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Mood→energy affinity: low-energy moods prefer low-energy tracks, etc.
fn mood_energy_affinity(mood: &str, energy: &str) -> bool {
    matches!(
        (mood, energy),
        ("calm", "low")
            | ("calm", "medium")
            | ("sad", "low")
            | ("dark", "low")
            | ("dark", "medium")
            | ("dramatic", "medium")
            | ("dramatic", "high")
            | ("energetic", "high")
            | ("energetic", "medium")
            | ("upbeat", "high")
            | ("upbeat", "medium")
            | ("neutral", _)
    )
}

/// Topic-aware music selection: library index → Pixabay → yt-dlp.
///
/// Selection algorithm (Phase 1 upgrade):
/// 1. Filter library entries by mood field (primary signal)
/// 2. Within mood bucket, sort by deterministic hash for variety
/// 3. Fallback: energy-affinity match if mood bucket empty
/// 4. Fallback: title/tag keyword match if energy bucket empty
/// 5. Fallback: Pixabay, then yt-dlp
async fn auto_select_music(theme: &str, video_keywords: &[String]) -> Option<MusicSelection> {
    let calm =
        openscript_core::production_quality::is_calm_focus_context(Some(theme), video_keywords);
    let mood = if calm {
        "calm"
    } else if theme == "energetic" {
        "energetic"
    } else {
        "neutral"
    }
    .to_string();
    let search_terms: Vec<&str> = if calm {
        vec![
            "lofi", "chill", "ambient", "calm", "focus", "study", "peaceful", "meditation",
            "relax",
        ]
    } else if theme == "energetic" {
        vec!["upbeat", "electronic", "energy", "corporate", "positive"]
    } else {
        vec!["background", "chill", "lofi", "ambient"]
    };
    let deny = ["parade", "march", "military", "trailer", "stadium", "anthem", "circus"];

    // ── Library index: mood-first selection ──────────────────────────────
    let index_path = resolve_repo_path("mcp/assets/music_library_index.json");
    if index_path.exists() {
        if let Ok(index_str) = std::fs::read_to_string(&index_path) {
            if let Ok(index) = serde_json::from_str::<serde_json::Value>(&index_str) {
                let entries = index
                    .get("entries")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();

                // Pre-filter: valid music entries, not on denylist, not local-only.
                let candidates: Vec<&serde_json::Value> = entries
                    .iter()
                    .filter(|e| {
                        let title = e.get("title").and_then(|v| v.as_str()).unwrap_or("");
                        let title_lower = title.to_lowercase();
                        if deny.iter().any(|d| title_lower.contains(d)) {
                            return false;
                        }
                        let media_type = e
                            .get("media_type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("music");
                        if media_type != "music" && !media_type.is_empty() {
                            return false;
                        }
                        let source_type = e
                            .get("source_type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if source_type == "local" {
                            return false;
                        }
                        let tags = entry_tags(e);
                        if openscript_core::production_quality::music_hits_denylist(
                            title,
                            Some(&mood),
                            &tags,
                            Some(&title_lower),
                        ) {
                            return false;
                        }
                        true
                    })
                    .collect();

                // Tier 1: mood-field exact match, shuffled by deterministic hash.
                let mut mood_bucket: Vec<&&serde_json::Value> = candidates
                    .iter()
                    .filter(|e| {
                        let entry_mood = e.get("mood").and_then(|v| v.as_str()).unwrap_or("neutral");
                        entry_mood == mood
                    })
                    .collect();
                mood_bucket.sort_by_key(|e| {
                    let vid = e
                        .get("video_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    deterministic_hash(vid)
                });

                // Tier 2: energy-affinity match (if mood bucket empty).
                let energy_bucket: Vec<&&serde_json::Value> = if mood_bucket.is_empty() {
                    let mut bucket: Vec<&&serde_json::Value> = candidates
                        .iter()
                        .filter(|e| {
                            let entry_energy =
                                e.get("energy").and_then(|v| v.as_str()).unwrap_or("medium");
                            mood_energy_affinity(&mood, entry_energy)
                        })
                        .collect();
                    bucket.sort_by_key(|e| {
                        let vid = e
                            .get("video_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        deterministic_hash(vid)
                    });
                    bucket
                } else {
                    vec![]
                };

                // Tier 3: title/tag keyword match (if both mood and energy buckets empty).
                let keyword_bucket: Vec<&&serde_json::Value> =
                    if mood_bucket.is_empty() && energy_bucket.is_empty() {
                        candidates
                            .iter()
                            .filter(|e| {
                                let title = e
                                    .get("title")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_lowercase();
                                let tags = entry_tags(e);
                                let tag_blob = tags.join(" ").to_lowercase();
                                search_terms
                                    .iter()
                                    .any(|t| title.contains(t) || tag_blob.contains(t))
                            })
                            .collect()
                    } else {
                        vec![]
                    };

                // Try each tier in order.
                let selected: Vec<&&serde_json::Value> = if !mood_bucket.is_empty() {
                    mood_bucket
                } else if !energy_bucket.is_empty() {
                    energy_bucket
                } else {
                    keyword_bucket
                };

                for entry in selected {
                    let filename = entry
                        .get("filename")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if filename.is_empty() {
                        continue;
                    }
                    let tags = entry_tags(entry);
                    let music_cache = resolve_repo_path("mcp/assets/music_cache");
                    let _ = std::fs::create_dir_all(&music_cache);
                    let local_path = music_cache.join(filename);
                    if local_path.exists() {
                        return Some(MusicSelection {
                            path: local_path.to_string_lossy().into(),
                            mood: mood.clone(),
                            tags: tags.clone(),
                            selection_query: format!("library:{}", filename),
                            source: "library".into(),
                        });
                    }
                    let download_args = json!({
                        "filename": filename,
                        "output_dir": music_cache.to_string_lossy(),
                    });
                    if let Ok(result) = handle_library_download(download_args).await {
                        if let Some(path) = result.get("path").and_then(|v| v.as_str()) {
                            return Some(MusicSelection {
                                path: path.to_string(),
                                mood: mood.clone(),
                                tags,
                                selection_query: format!("library:{}", filename),
                                source: "library".into(),
                            });
                        }
                    }
                }
            }
        }
    } else {
        tracing::warn!(
            "[script.to_video] music_library_index.json missing — run library.build"
        );
    }

    // ── Pixabay fallback ─────────────────────────────────────────────────
    if let Some(path) = fetch_pixabay_music(if calm { "calm" } else { theme }).await {
        if !openscript_core::production_quality::music_hits_denylist(
            &path,
            Some(&mood),
            &[],
            Some("pixabay"),
        ) {
            return Some(MusicSelection {
                path,
                mood: mood.clone(),
                tags: search_terms.iter().map(|s| s.to_string()).collect(),
                selection_query: format!("pixabay:{}", theme),
                source: "pixabay".into(),
            });
        }
    }

    // ── yt-dlp fallback: topic-safe queries only ─────────────────────────
    let yt_q = if calm {
        "lofi study focus chill no copyright music"
    } else if theme == "energetic" {
        "upbeat corporate positive no copyright music"
    } else {
        "ambient chill background no copyright music"
    };
    if let Some(path) = fetch_youtube_music_bed_query(yt_q).await {
        if openscript_core::production_quality::music_hits_denylist(
            &path,
            Some(&mood),
            &[],
            Some(yt_q),
        ) {
            tracing::warn!("[script.to_video] rejecting denylist music path {}", path);
        } else {
            return Some(MusicSelection {
                path,
                mood,
                tags: search_terms.iter().map(|s| s.to_string()).collect(),
                selection_query: yt_q.into(),
                source: "youtube".into(),
            });
        }
    }

    // ── Fallback 6: Pick ANY cached MP3 from mcp/assets/music_cache/ ──
    // Phase 28: When all else fails (no library match, no Pixabay, no YouTube),
    // use any locally cached music file. This ensures music is always present
    // in the output when cached files exist, preventing silent music omission.
    {
        let music_cache = resolve_repo_path("mcp/assets/music_cache");
        if let Ok(entries) = std::fs::read_dir(&music_cache) {
            let mut mp3s: Vec<_> = entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.path()
                        .extension()
                        .map(|ext| ext == "mp3")
                        .unwrap_or(false)
                })
                .collect();
            // Deterministic shuffle for variety across runs
            mp3s.sort_by_key(|e| e.file_name());
            if let Some(entry) = mp3s.first() {
                let path = entry.path().to_string_lossy().to_string();
                tracing::info!(
                    "[script.to_video] music: using cached fallback {}",
                    path
                );
                return Some(MusicSelection {
                    path,
                    mood: mood.clone(),
                    tags: vec!["cached".into()],
                    selection_query: "cached-fallback".into(),
                    source: "cache".into(),
                });
            }
        }
    }

    None
}

/// Fetch royalty-free music from Pixabay's API.
/// Pixabay provides direct MP3 download URLs (no yt-dlp needed).
/// If PIXABAY_API_KEY is not set, uses the free tier (limited but functional).
async fn fetch_pixabay_music(theme: &str) -> Option<String> {
    let pixabay_key = pixabay_key();
    if pixabay_key.is_empty() {
        tracing::info!("[script.to_video] PIXABAY_API_KEY not set — skipping Pixabay music fetch");
        return None;
    }

    let search_query = match theme {
        "calm" => "meditation+calm",
        "energetic" => "upbeat+energetic",
        _ => "background+music",
    };

    let url = format!(
        "https://pixabay.com/api/audio/?key={}&q={}&per_page=3",
        pixabay_key,
        search_query
    );

    tracing::info!("[script.to_video] Fetching music from Pixabay: q='{}'", search_query);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .ok()?;

    let resp = client.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        tracing::warn!("[script.to_video] Pixabay API returned status: {}", resp.status());
        return None;
    }

    let body: serde_json::Value = resp.json().await.ok()?;
    let hits = body.get("hits").and_then(|v| v.as_array())?;

    for hit in hits {
        let audio_url = hit.get("audio").and_then(|v| v.as_str()).unwrap_or("");
        let title = hit.get("tags").and_then(|v| v.as_str()).unwrap_or("unknown");

        if audio_url.is_empty() {
            continue;
        }

        tracing::info!("[script.to_video] Pixabay music found: '{}'", title);

        // Download the MP3
        let music_cache = resolve_repo_path("mcp/assets/music_cache");
        let _ = std::fs::create_dir_all(&music_cache);
        let filename = format!("pixabay_{}.mp3",
            title.replace(' ', "_").chars().take(30).collect::<String>());
        let local_path = music_cache.join(&filename);

        if local_path.exists() {
            tracing::info!("[script.to_video] Pixabay music already cached: {}", local_path.display());
            return Some(local_path.to_string_lossy().to_string());
        }

        if let Ok(dl_resp) = client.get(audio_url).send().await {
            if dl_resp.status().is_success() {
                if let Ok(bytes) = dl_resp.bytes().await {
                    std::fs::write(&local_path, &bytes).ok();
                    tracing::info!(
                        "[script.to_video] Downloaded Pixabay music: {} ({} bytes)",
                        local_path.display(), bytes.len()
                    );
                    return Some(local_path.to_string_lossy().to_string());
                }
            }
        }
    }

    tracing::warn!("[script.to_video] No downloadable music found on Pixabay");
    None
}

/// Build a mood-aware, scene-text-aware GIPHY sticker search query.
///
/// Priority (first non-empty candidate wins):
/// 1. Theme-based keyword (calm → "meditation", energetic → "fire", etc.)
/// 2. Scene emote (if the first scene has an emote like "happy", "surprised")
/// 3. Scene-text noun extraction (first salient noun from scene text)
/// 4. Speaker preset ("robot", "cat", "default_person")
/// 5. Fallback: "talking head"
///
/// Round-5 audit: the old hardcoded "{speaker_name} talking" produced
/// irrelevant stickers because speaker names are abstract IDs like "alice"
/// or "narrator" — not content GIPHY has indexed.
fn build_sticker_query(
    _speaker_name: &str,
    speaker_spec: &openscript_core::script::SpeakerSpec,
    scenes: &[openscript_core::script::SceneSpec],
    theme: &str,
    used_queries: &mut std::collections::HashSet<String>,
) -> String {
    // Theme-based keyword pools (rotated to avoid duplicates across speakers)
    let calm_keywords = ["meditation", "lotus", "candle", "breathing", "zen", "calm"];
    let energetic_keywords = ["fire", "lightning", "thumbs up", "applause", "energy", "explosion"];
    let neutral_keywords = ["talking head", "speech", "microphone", "podcast", "speaker"];

    let pool: &[&str] = match theme {
        "calm" => &calm_keywords,
        "energetic" => &energetic_keywords,
        _ => &neutral_keywords,
    };

    // Build candidate list in priority order
    let mut candidates: Vec<String> = Vec::new();

    // 1. Theme keywords (try each until we find one not yet used)
    for kw in pool {
        candidates.push(kw.to_string());
    }

    // 2. Scene emote
    if let Some(scene) = scenes.first() {
        if let Some(ref emote) = scene.emote {
            if !emote.is_empty() {
                candidates.push(emote.clone());
            }
        }
    }

    // 3. Scene-text noun (simple heuristic: first non-stopword > 4 chars)
    if let Some(scene) = scenes.first() {
        if let Some(noun) = extract_salient_noun(&scene.text) {
            candidates.push(noun);
        }
    }

    // 4. Speaker preset (if it's a real preset name, not empty/default)
    if !speaker_spec.preset.is_empty() && speaker_spec.preset != "default_person" {
        candidates.push(speaker_spec.preset.clone());
    }

    // 5. Fallback
    candidates.push("talking head".to_string());

    // Return the first candidate not yet used
    for c in &candidates {
        if !used_queries.contains(c) {
            used_queries.insert(c.clone());
            return c.clone();
        }
    }

    // All used — return the last candidate anyway
    candidates.last().unwrap().clone()
}

/// Extract the first salient noun from a text string.
/// Simple heuristic: skip stopwords, return the first word > 4 characters.
/// Extract an emotional reaction query from scene text for GIPHY meme b-rolls.
///
/// Detects the emotional beat of the scene and returns a GIPHY-friendly
/// query that will produce a relevant reaction GIF. Uses keyword matching
/// in the scene text + the scene's emote field + the overall theme.
/// Build a GIPHY search query for meme b-rolls that is BOTH topic-relevant
/// AND scene-specific.
///
/// The old approach used emotion labels ("mind blown reaction", "happy reaction")
/// which returned generic reaction memes unrelated to the video's topic.
/// GIPHY's translate endpoint interprets these as "show me a reaction GIF"
/// — it returns random reaction memes, not content-relevant clips.
///
/// The new approach uses content keywords extracted from the scene text,
/// combined with the video topic keywords. This makes GIPHY return clips
/// that are actually about the scene's subject matter.
///
/// Example:
///   Scene: "Your brain uses 20 watts of power."
///   Topic: ["brain", "neuroscience"]
///   Old query: "brain reaction" → generic brain reaction meme (irrelevant)
///   New query: "brain power energy" → brain/power/energy themed GIF (relevant)
///
/// (Round-17: "GIF relevance is still an issue. The recently generated
/// round 16 has a lot of GIF Brolls that are fully irrelevant. They are
/// meme material but not relevant to the topic.")
/// Build multiple GIPHY search queries for meme b-rolls, ranked by relevance.
///
/// Uses GIPHY SDK features discovered via Context7 research:
/// 1. Multi-query strategy: try specific → broad → trending fallback
/// 2. Relevance scoring: check GIF `tags` and `title` against query keywords
/// 3. `remove_low_contrast=true`: filter low-quality results
/// 4. `channel_ids`: could filter to topic-relevant channels (future)
///
/// The function returns a list of (query, limit) pairs to try in order.
/// The caller iterates through them until it finds a suitable non-duplicate,
/// non-static GIF with an MP4 URL.
///
/// (Round-18: GIPHY SDK comprehensive integration — user asked to "check
/// GIPHY SDK details and investigate how to effectively implement the
/// topic/context relevant brolls through GIFs, and understand how the
/// GIPHY SDK provides more comprehensive features and utilities than
/// regular API")
fn build_meme_search_queries(
    scene_text: &str,
    video_keywords: &[String],
    theme: &str,
) -> Vec<(String, u32)> {
    let stop_words = [
        "the", "a", "an", "is", "are", "was", "were", "be", "been", "have",
        "has", "had", "do", "does", "did", "will", "would", "could", "should",
        "can", "may", "might", "it", "its", "this", "that", "these", "those",
        "i", "you", "he", "she", "we", "they", "what", "which", "who", "when",
        "where", "why", "how", "all", "each", "every", "both", "few", "more",
        "most", "other", "some", "such", "no", "nor", "not", "only", "own",
        "same", "so", "than", "too", "very", "just", "but", "and", "or", "if",
        "then", "else", "for", "of", "to", "in", "on", "at", "by", "with",
        "from", "as", "into", "through", "during", "before", "after", "about",
        "your", "yours", "their", "them", "our", "us", "my", "me",
        "here", "there", "now", "then", "also", "very",
    ];

    // Extract ALL content words from scene (not just first 3)
    let scene_words: Vec<String> = scene_text
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
        .filter(|w| !w.is_empty() && w.len() > 2 && !stop_words.contains(&w.as_str()))
        .collect();

    let topic_words: Vec<&str> = video_keywords.iter().take(3).map(|s| s.as_str()).collect();

    let mut queries: Vec<(String, u32)> = Vec::new();

    // Strategy 1: Most specific — topic + best scene content words
    // Pick the 2 longest scene words (usually the most informative nouns)
    let mut sorted_scene: Vec<&String> = scene_words.iter().collect();
    sorted_scene.sort_by(|a, b| b.len().cmp(&a.len()));
    let best_scene_words: Vec<&str> = sorted_scene.iter().take(2).map(|s| s.as_str()).collect();

    let mut q1_parts: Vec<&str> = Vec::new();
    for tw in &topic_words {
        if !q1_parts.contains(tw) {
            q1_parts.push(tw);
        }
    }
    for sw in &best_scene_words {
        if !q1_parts.contains(sw) && !topic_words.contains(sw) {
            q1_parts.push(sw);
        }
    }
    if !q1_parts.is_empty() {
        queries.push((q1_parts.join(" "), 10));
    }

    // Strategy 2: Broader — just topic keywords (for when scene words are too specific)
    if !topic_words.is_empty() {
        queries.push((topic_words.join(" "), 10));
    }

    // Strategy 3: Single best scene word alone (for when topic is too broad)
    if let Some(&best) = best_scene_words.first() {
        if !topic_words.contains(&best) {
            queries.push((best.to_string(), 5));
        }
    }

    // Strategy 4: Topic + theme (fallback for diversity)
    if !topic_words.is_empty() {
        let theme_suffix = match theme {
            "calm" => " nature",
            "energetic" => " action",
            _ => "",
        };
        queries.push((format!("{}{}", topic_words[0], theme_suffix), 5));
    }

    // Strategy 5: Ultimate fallback — trending GIFs (no query, just popular content)
    queries.push(("".to_string(), 5)); // Empty query = use trending endpoint

    queries
}

/// Score a GIPHY GIF's relevance to the search query.
/// Returns a score 0-100 based on how well the GIF's title, tags, and
/// source match the query keywords.
/// (Round-18: GIPHY SDK relevance scoring using GIF Object metadata)
fn score_gif_relevance(gif: &serde_json::Value, query: &str) -> u32 {
    let query_words: Vec<&str> = query.split_whitespace().collect();
    if query_words.is_empty() {
        return 50; // No query = neutral score for trending
    }

    let title = gif.get("title").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
    let slug = gif.get("slug").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
    let username = gif.get("username").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
    let source_tld = gif.get("source_tld").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();

    let mut score = 0u32;

    for word in &query_words {
        let w = word.to_lowercase();
        // Title match = strong signal (5 points per word)
        if title.contains(&w) {
            score += 5;
        }
        // Slug match = medium signal (3 points)
        if slug.contains(&w) {
            score += 3;
        }
        // Username match = weak signal (1 point)
        if username.contains(&w) {
            score += 1;
        }
        // Source TLD match = weak signal (1 point)
        if source_tld.contains(&w) {
            score += 1;
        }
    }

    // Bonus: GIF has alt_text (descriptive metadata) = higher quality
    if gif.get("alt_text").and_then(|v| v.as_str()).is_some() {
        score += 2;
    }

    score.min(100)
}

// build_meme_search_query + extract_emotion_query removed (YAGNI):
// superseded by build_meme_search_queries multi-query strategy (Phase BY/BZ).

fn extract_salient_noun(text: &str) -> Option<String> {
    let stopwords = [
        "the", "a", "an", "is", "are", "was", "were", "be", "been", "have", "has", "had",
        "do", "does", "did", "will", "would", "could", "should", "can", "may", "might",
        "this", "that", "these", "those", "your", "you", "they", "them", "their", "with",
        "from", "into", "about", "what", "when", "where", "which", "who", "how", "why",
        "for", "and", "but", "not", "all", "any", "some", "more", "most", "other", "such",
        "only", "own", "same", "than", "too", "very", "just", "also", "now", "then",
    ];
    for word in text.split_whitespace() {
        let clean: String = word.chars().filter(|c| c.is_alphanumeric()).collect();
        let lower = clean.to_lowercase();
        if lower.len() > 4 && !stopwords.contains(&lower.as_str()) {
            return Some(lower);
        }
    }
    None
}

/// Words that can produce undesirable Pexels results when used as search
/// queries. For example, "inhale" returns cigarette-smoking videos; "drink"
/// returns alcohol videos. Map them to safer equivalents that produce
/// on-topic calming/meditation content.
/// (Round-6 audit: "In the inhale section, the video was of someone
/// inhaling a cigarette.")




// ---------------------------------------------------------------------------
// Handler: background.search — search procedural background index by mood
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: sticker.load_preset — load SVG preset config
// ---------------------------------------------------------------------------



// ---------------------------------------------------------------------------
// Handler: sticker.render — generate animated sticker HTML composition
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: script.to_timeline — orchestrator for from-scratch video creation
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: script.to_video — one-call from-scratch video creation
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: stock.fetch — download stock music/videos from Pixabay/Pexels APIs
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: youtube.download — download YouTube video clips
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: youtube.search — search YouTube without downloading
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: stock.search — search Pixabay without downloading
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: media.search — PNG image search (Pexels Images + Openverse)
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: gif.search — GIPHY sticker search
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: timeline.inspect — deep-dive layer inspection
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: library.search — search music/SFX library index
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: library.download — download music/SFX on demand
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: library.build — rebuild the music/SFX library index
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: media.download (Phase I — unblock image workflow)
// ---------------------------------------------------------------------------


/// Extract file extension from a URL (defaults to "png").
fn url_extension(url: &str) -> String {
    let path = url.split('?').next().unwrap_or(url);
    if let Some(ext) = std::path::Path::new(path).extension() {
        if let Some(s) = ext.to_str() {
            let s = s.to_lowercase();
            if matches!(s.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp") {
                return s;
            }
        }
    }
    "png".to_string()
}

// ---------------------------------------------------------------------------
// Handler: gif.download (Phase I — unblock GIF workflow)
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: overlay.assign (Phase J — place images/GIFs/PNGs on the timeline)
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: sticker.keywords — agentic GIPHY sticker keyword generation
// (STAGE 1 of the sticker pipeline, parallel to broll.keywords)
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// LLM helper: sticker candidate validation (mirror of llm_validate_candidates)
// ---------------------------------------------------------------------------
// Presents the REAL GIPHY candidates (title + id) for a segment to the agent,
// which approves the single best match against the spoken caption's intent.
// Returns (best_candidate_index, final_keyword, relevance 0-1, reason, backend,
// model). LLM-down ⇒ no approval — relevance must never be assumed.

async fn llm_validate_sticker_candidates(
    caption: &str,
    intent: &str,
    draft_keywords: &[String],
    candidates: &[serde_json::Value],
    language: &str,
) -> (Option<usize>, String, f64, String, String, String) {
    if candidates.is_empty() {
        return (
            None,
            String::new(),
            0.0,
            "no candidates".into(),
            String::new(),
            String::new(),
        );
    }
    let candidate_lines: Vec<String> = candidates
        .iter()
        .enumerate()
        .take(6)
        .map(|(idx, c)| {
            let title = c.get("title").and_then(|v| v.as_str()).unwrap_or("?");
            // Only the numeric index is shown — the LLM must return the index,
            // not the opaque GIPHY id, to avoid best_idx ambiguity.
            format!("  idx={} title=\"{}\"", idx, title)
        })
        .collect();
    let system = format!(
        "You are a short-form video director's sticker relevance validator. \
        Given a spoken segment and candidate GIPHY stickers (index + title), \
        decide whether any sticker genuinely matches the segment's EMOTION/INTENT ('{}'). \
        Rules: \
        1. Approve only a sticker that clearly fits the spoken content's emotional beat; \
        a generic or contradictory sticker must be rejected. \
        2. 'relevance' 0.0-1.0 (how well the best sticker matches). \
        3. Output ONLY compact JSON: \
        {{\"best_idx\": 2, \"final_keyword\": \"angry eyes\", \"relevance\": 0.9, \"reason\": \"one sentence\"}} \
        or, if none fit: {{\"best_idx\": null, \"final_keyword\": \"\", \"relevance\": 0.0, \"reason\": \"why none fit\"}}. \
        Source language: {}.",
        intent, language
    );
    let user = format!(
        "Segment caption (spoken): \"{}\"\nDetected intent: {}\nDraft keywords: [{}]\nCandidates:\n{}\n\
         Pick the single best sticker index (or null if none genuinely fit). JSON only.",
        caption,
        intent,
        draft_keywords.join(", "),
        candidate_lines.join("\n")
    );
    match crate::llm::chat_complete(&system, &user, None).await {
        Ok(r) => {
            let parsed = parse_loose_json_obj(&r.text);
            let best_idx = parsed
                .get("best_idx")
                .and_then(|v| v.as_u64())
                .map(|u| u as usize)
                // Clamp: an out-of-range index from the LLM must not silently
                // reject a good sticker (fail toward the best candidate).
                .map(|u| u.min(candidates.len().saturating_sub(1)));
            let llm_kw = parsed
                .get("final_keyword")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let final_kw = if llm_kw.is_empty() {
                draft_keywords.first().cloned().unwrap_or_default()
            } else {
                llm_kw
            };
            let rel = parsed.get("relevance").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let reason = parsed
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            (best_idx, final_kw, rel, reason, r.backend, r.model)
        }
        Err(e) => {
            tracing::warn!("[sticker.validate_keywords] LLM failed: {}", e);
            (
                None,
                String::new(),
                0.0,
                format!("llm_failed: {}", e),
                String::new(),
                String::new(),
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Handler: sticker.validate_keywords — Stage 2 relevance-validation gate
// ---------------------------------------------------------------------------
// Stage 1 (sticker.keywords) drafts emotional/intent sticker keywords. This
// stage closes the loop: it searches GIPHY with those drafts, presents the
// REAL candidate stickers (title + id) to the agent, and the agent approves the
// best match against the spoken caption — producing final_keyword + best_sticker
// per segment. Segments with no emphatic keywords, no GIPHY results, or no
// approved match are skipped (better no sticker than an irrelevant one).


// ---------------------------------------------------------------------------
// Handler: sticker.auto — ONE-CALL agentic sticker pipeline (parallel to broll.auto)
// segment.analyze → sticker.keywords → GIPHY search → download → place on Stickers track
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Pure gating helpers (unit-tested): position cycling + placement spacing
// ---------------------------------------------------------------------------

/// Resolve the anchor position for the Nth placed sticker. "auto" cycles the
/// anchor so a sticker run is visually varied and never crowds one corner;
/// any explicit position passes through unchanged (manual override). The cycle
/// stays clear of the center-screen caption zone.
fn sticker_place_position(position: &str, placed_idx: usize) -> String {
    if position != "auto" {
        return position.to_string();
    }
    const CYCLE: [&str; 4] = ["top-right", "bottom-right", "center-left", "bottom-left"];
    CYCLE[placed_idx % CYCLE.len()].to_string()
}

/// Spacing gate: a sticker may be placed only when the segment starts at least
/// `min_gap_s` after the previous sticker's end. Prevents sticker spam on
/// consecutive segments (the E2E test placed a sticker on nearly every
/// segment, all at the same anchor).
fn sticker_spacing_allowed(prev_end_s: Option<f64>, seg_start_s: f64, min_gap_s: f64) -> bool {
    match prev_end_s {
        Some(prev) => seg_start_s >= prev + min_gap_s,
        None => true,
    }
}

// ---------------------------------------------------------------------------
// Handler: sticker.auto_assign — place stickers/GIFs on the Stickers track
// Uses enriched_segments (sticker.keywords output) when provided, else falls
// back to caption-word queries. Places on TrackType::Stickers so the renderer
// composites them as positioned PiP overlays above the b-roll.
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: voices.list — list all available TTS voices
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: timeline.to_hyperframes (Phase M — bridge EDL v2 → HF HTML)
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handlers: llm.complete / vision.analyze_clip / vision.score_clip
// ---------------------------------------------------------------------------






// ---------------------------------------------------------------------------
// Handler: system.capabilities (P1-2 from prior audit)
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Handler: system.doctor — cold-start production readiness
// ---------------------------------------------------------------------------


/// Lightweight HTTP probe — returns true if the URL responds with any HTTP
/// status (even 404). Used to check if a local sidecar is running without
/// depending on a specific endpoint shape.
async fn probe_http(url: &str) -> bool {
    match reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(800))
        .build()
    {
        Ok(client) => client.get(url).send().await.is_ok(),
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// Handler: help.tool (Recommendation 3.1 from prior audit)
// ---------------------------------------------------------------------------


#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------------
    // Emotion-take + speed/pitch plumbing tests (Phase: tonality templates)
    // ---------------------------------------------------------------------

    #[test]
    fn test_build_speed_pitch_filter_no_op_when_defaults() {
        assert_eq!(build_speed_pitch_filter(1.0, 1.0, 44100), "", "1.0/1.0 must be a no-op");
    }

    // ---------------------------------------------------------------------
    // loudnorm JSON parse tests (Phase 170) — this exact parse regressed
    // three times (stdout/stderr split, -v error suppression, trailing
    // muxing summary); lock it with the real observed ffmpeg output format.
    // ---------------------------------------------------------------------

    #[test]
    fn test_parse_loudnorm_input_i_handles_trailing_muxing_summary() {
        // Real ffmpeg stderr: loudnorm JSON block followed by the brace-free
        // muxing summary — from_str on the whole stream fails with trailing
        // data (the lufs: null bug).
        let stderr = concat!(
            "[Parsed_loudnorm_0 @ 0x55fb07cb04c0] {\n",
            "\t\"input_i\" : \"-18.17\",\n",
            "\t\"input_tp\" : \"-12.00\",\n",
            "\t\"input_lra\" : \"6.00\",\n",
            "\t\"input_thresh\" : \"-25.00\"\n",
            "}\n[null @ 0x55fb07cb04c0] video:0KiB audio:3361KiB subtitle:0KiB ",
            "muxing overhead: unknown\nsize=N/A time=00:00:09.00 bitrate=N/A speed=78.1x elapsed=0:00:00.11\n",
        );
        assert_eq!(parse_loudnorm_input_i(stderr), Some(-18.17));
    }

    #[test]
    fn test_parse_loudnorm_input_i_stdout_block() {
        // Modern ffmpeg can print the block on stdout with no trailing text.
        let stdout = concat!(
            "{\"input_i\" : \"-16.50\", ",
            "\"input_tp\" : \"-9.2\", ",
            "\"input_lra\" : \"7.0\", ",
            "\"input_thresh\" : \"-23.0\"}\n",
        );
        assert_eq!(parse_loudnorm_input_i(stdout), Some(-16.50));
    }

    #[test]
    fn test_parse_loudnorm_input_i_no_block_returns_none() {
        assert_eq!(parse_loudnorm_input_i("no json here"), None);
        assert_eq!(parse_loudnorm_input_i(""), None);
    }

    #[test]
    fn test_build_speed_pitch_filter_speed_only() {
        let f = build_speed_pitch_filter(1.5, 1.0, 22050);
        assert_eq!(f, "atempo=1.500000", "speed-only filter: {}", f);
    }

    #[test]
    fn test_build_speed_pitch_filter_pitch_chain() {
        // Pitch 1.2 at 22050 Hz → asetrate up, resample back, atempo restores duration.
        let f = build_speed_pitch_filter(1.0, 1.2, 22050);
        assert!(f.contains("asetrate=22050*1.200000"), "pitch start: {}", f);
        assert!(f.contains("aresample=22050"), "resample back: {}", f);
        assert!(f.contains("atempo=0.833333"), "duration restore: {}", f);
    }

    #[test]
    fn test_build_speed_pitch_filter_speed_chains_past_2x() {
        // atempo supports only 0.5–2.0 per instance → 4x must chain two atempo=2.0.
        let f = build_speed_pitch_filter(4.0, 1.0, 44100);
        assert_eq!(f, "atempo=2.0,atempo=2.000000", "chained atempo: {}", f);
    }

    #[test]
    fn test_build_speed_pitch_filter_combined() {
        let f = build_speed_pitch_filter(1.25, 0.9, 44100);
        assert!(f.starts_with("asetrate=44100*"), "pitch first: {}", f);
        assert!(f.contains(",atempo=1.111111"), "pitch restore: {}", f);
        assert!(f.ends_with("atempo=1.250000"), "speed last: {}", f);
    }

    #[test]
    fn test_resolve_emotion_take_matches_profile_emotions() {
        let mut profile = openscript_tts::profiles::VoiceProfile {
            id: "ishan".into(),
            provider: "gepard".into(),
            mode: "clone".into(),
            model: String::new(),
            ref_audio: "base.wav".into(),
            ref_text: String::new(),
            language: "English".into(),
            description: None,
            sample_rate: 22050,
            created_at: String::new(),
            emotions: std::collections::HashMap::new(),
        };
        profile.emotions.insert(
            "angry".into(),
            openscript_tts::profiles::EmotionTake {
                ref_audio: "angry.wav".into(),
                ref_text: "I am furious!".into(),
                cfg_scale: Some(1.5),
                speed: None,
            },
        );
        assert_eq!(
            resolve_emotion_take(&profile, Some("angry")).map(|t| t.ref_audio.as_str()),
            Some("angry.wav"),
            "registered emotion resolves to its take"
        );
        assert_eq!(
            resolve_emotion_take(&profile, Some("whisper")).map(|t| t.ref_audio.as_str()),
            None,
            "unregistered emotion falls back to base voice"
        );
        assert_eq!(
            resolve_emotion_take(&profile, None).map(|t| t.ref_audio.as_str()),
            None,
            "no emotion = base voice"
        );
        assert_eq!(
            resolve_emotion_take(&profile, Some("")).map(|t| t.ref_audio.as_str()),
            None,
            "empty emotion = base voice"
        );
    }

    #[test]
    fn test_pexels_search_url_min_max_duration_and_page() {
        // SEGMENTATION_ARCHITECTURE clip-duration matching: min/max duration
        // filters are appended so the API only returns clips that cover the
        // requested window (no short-clip looping).
        let url = pexels_search_url("crowd protest", "portrait", 2, 11.9, 0.0);
        assert!(url.starts_with(
            "https://api.pexels.com/videos/search?query=crowd%20protest&per_page=15&orientation=portrait&page=2"
        ), "base URL shape: {}", url);
        assert!(url.contains("&min_duration=11"), "min_duration floor: {}", url);
        assert!(!url.contains("max_duration"), "no max when 0: {}", url);

        let url2 = pexels_search_url("tech", "landscape", 1, 2.0, 6.0);
        assert!(url2.contains("&min_duration=2"), "min: {}", url2);
        assert!(url2.contains("&max_duration=6"), "max floor: {}", url2);

        let url3 = pexels_search_url("nature", "portrait", 1, 0.0, 0.0);
        assert!(!url3.contains("min_duration"), "no min when 0: {}", url3);
        assert!(!url3.contains("max_duration"), "no max when 0: {}", url3);
    }

    #[test]
    fn stock_trim_command_gpu_mode_has_hwaccel_and_nvenc() {
        // GPU path: NVDEC prelude BEFORE -i, NVENC encoder with p2 (fast) and
        // cq=crf+2=25, plus the yuv420p/30fps intermediates the render assumes.
        let gpu = GpuConfig {
            decode: true,
            encode_nvenc: true,
        };
        let cmd = build_stock_trim_command(
            &gpu,
            "/in/clip.mp4",
            "/out/clip_trim.mp4",
            5.5,
            Some(1.25),
            "scale=1080:1920:force_original_aspect_ratio=increase,crop=1080:1920",
        );
        let args: Vec<String> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        // Input seek lands before -i; hwaccel prelude must precede -i too.
        // Sequence: -y -ss 1.25 -hwaccel cuda -i <input> ...
        let i_idx = args.iter().position(|a| a == "-i").unwrap();
        assert_eq!(args[i_idx - 4], "-ss");
        assert_eq!(args[i_idx - 3], "1.25");
        assert_eq!(args[i_idx - 2], "-hwaccel");
        assert_eq!(args[i_idx - 1], "cuda");
        assert_eq!(args[i_idx + 1], "/in/clip.mp4");
        // NVENC encoder block.
        assert!(args.windows(2).any(|w| w == ["-c:v", "h264_nvenc"]));
        assert!(args.windows(2).any(|w| w == ["-preset", "p2"]));
        assert!(args.windows(2).any(|w| w == ["-cq", "25"]));
        assert!(args.windows(2).any(|w| w == ["-pix_fmt", "yuv420p"]));
        assert!(args.windows(2).any(|w| w == ["-r", "30"]));
        // -an then output path, in that order at the tail.
        let an_idx = args.iter().position(|a| a == "-an").unwrap();
        assert_eq!(args[an_idx + 1], "/out/clip_trim.mp4");
    }

    #[test]
    fn stock_trim_command_cpu_mode_matches_legacy_shape() {
        // CPU path: identical to the old inline libx264 fast crf23 command.
        let cpu = GpuConfig {
            decode: false,
            encode_nvenc: false,
        };
        let cmd = build_stock_trim_command(
            &cpu,
            "/in/clip.mp4",
            "/out/clip_trim.mp4",
            5.5,
            None,
            "crop=1080:1920",
        );
        let args: Vec<String> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert!(!args.contains(&"-hwaccel".to_string()), "no hwaccel in CPU mode");
        assert!(args.windows(2).any(|w| w == ["-c:v", "libx264"]));
        assert!(args.windows(2).any(|w| w == ["-preset", "fast"]));
        assert!(args.windows(2).any(|w| w == ["-crf", "23"]));
        // No -ss when start is None (Pexels/Pixabay start at 0).
        assert!(!args.contains(&"-ss".to_string()));
        let i_idx = args.iter().position(|a| a == "-i").unwrap();
        assert_eq!(args[i_idx - 1], "-y", "-y immediately before -i when no -ss");
    }

    #[test]
    fn test_normalize_caption_text_strips_punctuation_and_case() {
        assert_eq!(
            normalize_caption_text("Bhai sarkaar ki phati badhiya."),
            normalize_caption_text("bhai sarkaarki phati badhiya"),
            "punctuation/space/case-insensitive normalization"
        );
        assert_ne!(
            normalize_caption_text("The government started"),
            normalize_caption_text("Bhai sarkaar ki phati"),
            "different language must not normalize to the same text"
        );
    }

    #[test]
    fn test_caption_words_for_phrase_language_mismatch_falls_back() {
        use openscript_core::srt::SrtEntry;
        // Hinglish phrase (audio is Hinglish) vs ENGLISH word SRT — the A2V
        // caption-language bug. Real word timings must NOT be adopted (their
        // words don't match the phrase); char-proportional estimates on the
        // Hinglish phrase text must be used instead.
        let phrase = SrtEntry {
            idx: 1,
            start: 0.0,
            end: 3.31,
            text: "Bhai sarkaar ki phati badhiya. Sarkaar shuruaat".to_string(),
        };
        let english_words = vec![
            SrtEntry { idx: 1, start: 0.0, end: 0.46, text: "The".into() },
            SrtEntry { idx: 2, start: 0.46, end: 0.66, text: "government".into() },
            SrtEntry { idx: 3, start: 0.66, end: 2.60, text: "started".into() },
            SrtEntry { idx: 4, start: 2.60, end: 3.02, text: "the".into() },
            SrtEntry { idx: 5, start: 3.02, end: 3.31, text: "same".into() },
        ];
        let words = caption_words_for_phrase(&phrase, &english_words, 0, 0.0);
        assert!(
            !words.is_empty(),
            "must still produce words for the phrase"
        );
        assert_eq!(
            words.iter().map(|w| w.word.as_str()).collect::<Vec<_>>().join(" "),
            "Bhai sarkaar ki phati badhiya. Sarkaar shuruaat",
            "caption words must be the Hinglish phrase text, not the English word SRT"
        );
        // Estimates must tile the phrase window exactly.
        assert_eq!(words.first().unwrap().start_ms, 0);
        assert_eq!(words.last().unwrap().end_ms, 3310);
    }

    #[test]
    fn test_caption_words_for_phrase_aligned_uses_real_timings() {
        use openscript_core::srt::SrtEntry;
        // Matching word SRT (same language + content) → real alignments win.
        let phrase = SrtEntry {
            idx: 1,
            start: 0.0,
            end: 3.31,
            text: "the government started".to_string(),
        };
        let words_src = vec![
            SrtEntry { idx: 1, start: 0.0, end: 0.46, text: "the".into() },
            SrtEntry { idx: 2, start: 0.46, end: 0.66, text: "government".into() },
            SrtEntry { idx: 3, start: 0.66, end: 2.60, text: "started".into() },
        ];
        let words = caption_words_for_phrase(&phrase, &words_src, 0, 0.0);
        assert_eq!(words.len(), 3);
        assert_eq!(words[2].start_ms, 660, "real alignment start preserved");
        assert_eq!(words[2].end_ms, 2600, "real alignment end preserved");
    }

    #[test]
    fn test_caption_text_similarity_scores() {
        assert_eq!(caption_text_similarity("a b c", "a b c"), 1.0);
        assert_eq!(caption_text_similarity("", ""), 1.0);
        assert_eq!(caption_text_similarity("a b c", "x y z"), 0.0);
        // One token differs out of six → Jaccard 5/7 ≈ 0.71 (fuzzy gate ≥ 0.5).
        assert!(caption_text_similarity("a b c d e f", "a b c d e g") >= 0.5);
        // Disjoint Hinglish vs English stays below the gate.
        assert!(caption_text_similarity("bhai sarkaar ki phati", "the government started") < 0.5);
    }

    #[test]
    fn test_caption_words_for_phrase_fuzzy_match_keeps_real_timings() {
        use openscript_core::srt::SrtEntry;
        // Hinglish ASR transcribed one word slightly differently ("sarkaar" →
        // "sarkaari"). Exact equality fails but the token sets are ~identical;
        // the real alignment windows must be KEPT (not collapsed to estimates)
        // and the word TEXT overridden with the phrase's own words.
        let phrase = SrtEntry {
            idx: 1,
            start: 0.0,
            end: 3.31,
            text: "Bhai sarkaar ki phati badhiya".to_string(),
        };
        let words_src = vec![
            SrtEntry { idx: 1, start: 0.0, end: 0.5, text: "Bhai".into() },
            SrtEntry { idx: 2, start: 0.5, end: 1.0, text: "sarkaari".into() },
            SrtEntry { idx: 3, start: 1.0, end: 1.5, text: "ki".into() },
            SrtEntry { idx: 4, start: 1.5, end: 2.0, text: "phati".into() },
            SrtEntry { idx: 5, start: 2.0, end: 3.31, text: "badhiya".into() },
        ];
        let words = caption_words_for_phrase(&phrase, &words_src, 0, 0.0);
        assert_eq!(words.len(), 5, "real windows kept, not estimated");
        assert_eq!(words[1].word, "sarkaar", "text overridden with phrase word");
        assert_eq!(words[1].start_ms, 500, "real alignment start preserved");
        assert_eq!(words[1].end_ms, 1000, "real alignment end preserved");
        assert_eq!(words[4].end_ms, 3310);
    }

    #[test]
    fn test_remap_words_to_script_same_count_keeps_timings_script_text() {
        // ASR mis-heard the cloned voice: "bias" → "pie" (same word count).
        // The caption TEXT must be the script's words; the real alignment
        // timing windows are preserved.
        let timed = vec![
            WordTiming { word: "There's".into(), start_ms: 0, end_ms: 220 },
            WordTiming { word: "a".into(), start_ms: 220, end_ms: 300 },
            WordTiming { word: "pie".into(), start_ms: 300, end_ms: 480 }, // ASR error
            WordTiming { word: "so".into(), start_ms: 480, end_ms: 700 },
        ];
        let out = remap_words_to_script("There's a bias so", timed, 0, 700);
        let words: Vec<&str> = out.iter().map(|w| w.word.as_str()).collect();
        assert_eq!(words, vec!["There's", "a", "bias", "so"], "script text must win");
        assert_eq!(out[2].start_ms, 300, "real timing window preserved");
        assert_eq!(out[2].end_ms, 480);
    }

    #[test]
    fn test_remap_words_to_script_count_mismatch_estimates() {
        // ASR dropped a word → counts diverge → char-proportional estimate
        // on the SCRIPT text tiles the whole window (no wrong text, no holes).
        let timed = vec![WordTiming { word: "hello".into(), start_ms: 0, end_ms: 1500 }];
        let out = remap_words_to_script("hello world test", timed, 0, 3000);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].word, "hello");
        assert_eq!(out[1].word, "world");
        assert_eq!(out[2].word, "test");
        assert_eq!(out[0].start_ms, 0);
        assert_eq!(out[2].end_ms, 3000, "estimate must tile the full window");
    }

    #[test]
    fn test_remap_words_to_script_trims_trailing_hallucination() {
        // Whisper appended a trailing hallucinated token ("HBC") after the
        // real words — 17 aligned vs 16 script words. The trailing trim must
        // keep the REAL timing windows (not collapse to estimates) and apply
        // the script's text.
        let script = "Log kehte hain sab theek hai par sach yeh hai ki kuch bhi theek nahi hai";
        let mut timed: Vec<WordTiming> = script
            .split_whitespace()
            .enumerate()
            .map(|(i, w)| WordTiming {
                word: w.to_string(),
                start_ms: i as i64 * 300,
                end_ms: i as i64 * 300 + 250,
            })
            .collect();
        timed.push(WordTiming { word: "HBC".into(), start_ms: 4800, end_ms: 5100 });
        let out = remap_words_to_script(script, timed, 0, 5100);
        assert_eq!(out.len(), 16, "trailing hallucination trimmed, real windows kept");
        assert_eq!(out[15].word, "hai");
        assert_eq!(out[15].start_ms, 4500, "last real word timing preserved");
        assert_eq!(out[15].end_ms, 4750);
    }

    #[test]
    fn test_remap_words_to_script_empty_inputs() {
        // Empty aligned words → estimate (non-empty text).
        let out = remap_words_to_script("hello world", Vec::new(), 0, 2000);
        assert_eq!(out.len(), 2);
        // Empty text → passthrough unchanged.
        let timed = vec![WordTiming { word: "x".into(), start_ms: 0, end_ms: 500 }];
        let out2 = remap_words_to_script("", timed, 0, 500);
        assert_eq!(out2.len(), 1);
    }

    #[test]
    fn test_remap_words_to_script_idempotent_double_remap() {
        // generate_voices remaps at the source; build_captions remaps again
        // defensively. A second remap on already-remapped words must be
        // idempotent — including when the first pass fell to estimation.
        let raw = vec![WordTiming { word: "pie".into(), start_ms: 0, end_ms: 900 }];
        let once = remap_words_to_script("there's a bias", raw, 0, 3000);
        // count mismatch (1 vs 3) → estimate path on first pass
        assert_eq!(once.len(), 3);
        assert_eq!(once[2].word, "bias");
        let twice = remap_words_to_script("there's a bias", once.clone(), 0, 3000);
        assert_eq!(twice.len(), 3);
        assert_eq!(twice[2].word, "bias");
        assert_eq!(twice[0].start_ms, once[0].start_ms, "timings unchanged by 2nd remap");
        assert_eq!(twice[2].end_ms, once[2].end_ms);
        // Same-count zip path is also idempotent (text already = script words).
        let zipped = remap_words_to_script("a b c", once, 0, 3000);
        assert_eq!(zipped.len(), 3);
        assert_eq!(zipped[1].word, "b");
    }

    #[test]
    fn test_fresh_candidates_skips_used_ids() {
        use openscript_assets::pexels::{PexelsVideo, PexelsVideoFile};
        let mk = |id: i64| PexelsVideo {
            id,
            width: 1080,
            height: 1920,
            url: format!("https://www.pexels.com/video/clip-{}", id),
            image: String::new(),
            video_files: vec![PexelsVideoFile {
                id,
                quality: "hd".into(),
                width: 1080,
                height: 1920,
                link: format!("https://files.example/{}.mp4", id),
                size: 1000,
            }],
            duration: 10,
        };
        let vids = vec![mk(1), mk(2), mk(3)];
        let mut used = std::collections::HashSet::new();
        used.insert(1);
        // Top hit (id 1) is already placed → must be skipped (the repeat bug).
        let (fresh, reused) = fresh_candidates(&vids, &used, 1);
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].id, 2, "used clip must be skipped");
        assert_eq!(reused, 0, "no reuse when fresh candidates exist");
        // All used → library exhausted → fall back so the segment goes bare,
        // and the reused count is reported so the caller can warn.
        used.insert(2);
        used.insert(3);
        let (fallback, fallback_reused) = fresh_candidates(&vids, &used, 2);
        assert_eq!(fallback.len(), 2);
        assert_eq!(fallback[0].id, 1);
        assert_eq!(fallback_reused, 2, "exhausted library reuse must be reported");
    }

    #[test]
    fn test_sticker_place_position_cycles_on_auto() {
        // "auto" cycles the anchor so a sticker run is visually varied.
        assert_eq!(sticker_place_position("auto", 0), "top-right");
        assert_eq!(sticker_place_position("auto", 1), "bottom-right");
        assert_eq!(sticker_place_position("auto", 2), "center-left");
        assert_eq!(sticker_place_position("auto", 3), "bottom-left");
        assert_eq!(sticker_place_position("auto", 4), "top-right"); // wraps
        // Explicit position passes through unchanged (manual override).
        assert_eq!(sticker_place_position("top-left", 5), "top-left");
        assert_eq!(sticker_place_position("center", 0), "center");
    }

    #[test]
    fn test_sticker_spacing_allowed_gate() {
        // First sticker: always allowed.
        assert!(sticker_spacing_allowed(None, 0.0, 2.0));
        // Adjacent segment starts < min gap after previous end → blocked.
        assert!(!sticker_spacing_allowed(Some(5.0), 6.0, 2.0));
        // Exactly at the gap boundary → allowed.
        assert!(sticker_spacing_allowed(Some(5.0), 7.0, 2.0));
        // Well after → allowed.
        assert!(sticker_spacing_allowed(Some(5.0), 12.0, 2.0));
        // Zero gap → always allowed (legacy behavior).
        assert!(sticker_spacing_allowed(Some(5.0), 5.0, 0.0));
    }

    /// library.search returns results for keyword queries.
    #[tokio::test]
    async fn test_library_search_keyword_query() {
        let resp = handle_library_search(json!({"query": "chill", "limit": 5}))
            .await
            .expect("library.search should succeed");
        assert!(
            resp["status"] == "success" || resp["status"] == "searched",
            "library.search should succeed; got status={}",
            resp["status"]
        );
        let count = resp["count"].as_u64().unwrap_or(0);
        assert!(
            count > 0,
            "library.search should find results for 'chill'; got count={} resp={}",
            count,
            resp
        );
    }

    /// default_opt_bool: absent key → None; explicit true/false preserved.
    #[test]
    fn test_default_opt_bool_semantics() {
        assert_eq!(default_opt_bool(&json!({}), "loopable"), None);
        assert_eq!(
            default_opt_bool(&json!({"loopable": true}), "loopable"),
            Some(true)
        );
        assert_eq!(
            default_opt_bool(&json!({"loopable": false}), "loopable"),
            Some(false)
        );
    }

    #[test]
    fn test_production_score_procedural_only_is_fail_grade() {
        let bgs = vec![
            "mcp/assets/backgrounds/procedural_01.mp4".into(),
            "mcp/assets/backgrounds/procedural_02.mp4".into(),
        ];
        let (score, _dims, hard, _next) =
            compute_production_score(&bgs, None, 0, 0, true, true, true);
        assert!(score < 55, "procedural-only should not reach grade C; score={}", score);
        assert!(!hard.is_empty());
        assert_eq!(
            openscript_core::production_quality::production_grade(score),
            "F"
        );
    }

    #[test]
    fn test_production_score_full_stack_is_a() {
        let music = std::env::temp_dir().join("openscript_real_music_kpi.mp3");
        std::fs::write(&music, vec![1u8; 12_000]).expect("write temp music");
        let bgs = vec![
            "mcp/assets/background_cache/scene_001_yt.mp4".into(),
            "cache/stock_city.mp4".into(),
        ];
        let (score, _, _hard, _) = compute_production_score(
            &bgs,
            Some(music.to_str().unwrap()),
            3,
            2,
            true,
            true,
            true,
        );
        let _ = std::fs::remove_file(&music);
        // v2 scorer includes timeline/section dimensions; stock+stickers+music+speech
        // without full section/title map lands high-C / low-B.
        assert!(score >= 60, "full stack should be ≥C; score={}", score);
    }

    /// help.tool for NLE queries must prefer transcribe/reelize over script.to_video.
    #[tokio::test]
    async fn test_help_tool_nle_prefers_transcribe_over_script_to_video() {
        let resp = handle_help_tool(json!({
            "query": "edit existing footage with broll and captions",
            "limit": 8
        }))
        .await
        .expect("help.tool");
        assert_eq!(resp["status"], "success");
        let results = resp["results"].as_array().expect("results array");
        assert!(!results.is_empty());
        let names: Vec<&str> = results
            .iter()
            .filter_map(|r| r.get("name").and_then(|n| n.as_str()))
            .collect();
        // script.to_video must not be the top hit for NLE wording
        assert_ne!(
            names.first().copied(),
            Some("script.to_video"),
            "NLE query should not rank script.to_video first; got {:?}",
            names
        );
        // Prefer NLE tools over script.to_video in the ranking
        let script_pos = names.iter().position(|n| *n == "script.to_video");
        let nle_pos = names.iter().position(|n| {
            matches!(
                *n,
                "transcribe"
                    | "reelize.direct"
                    | "reelize.brief"
                    | "timeline.render"
                    | "timeline.build"
            )
        });
        assert!(
            nle_pos.is_some(),
            "expected an NLE tool in top results: {:?}",
            names
        );
        if let (Some(sp), Some(np)) = (script_pos, nle_pos) {
            assert!(
                np < sp,
                "NLE tool should rank above script.to_video; got {:?}",
                names
            );
        }
    }

    /// broll.fetch with no API key and no fallback_pool should return
    /// status:warning with missing_key:true (NOT hard-fail with an Err).
    /// This is the bug #16 regression test.
    ///
    /// Note: we set OPENSCRIPT_ROOT to a temp dir so get_api_key() doesn't
    /// find the real config file at mcp/assets/.openscript_config.json.
    /// Without this, the test would pass real Pexels keys through and
    /// broll.fetch would succeed instead of returning the warning.
    ///
    /// IMPORTANT: This test mutates process-global env vars (set_var/remove_var)
    /// and MUST be run single-threaded: cargo test -- --test-threads=1
    #[tokio::test]
    #[ignore] // env-var race — see #1132
    async fn test_broll_fetch_no_key_no_fallback_returns_warning() {
        // Ensure no Pexels key is set for this test (env + ~/.openscript).
        std::env::remove_var("PEXELS_API_KEY");
        let temp_cfg = std::env::temp_dir().join("openscript_test_no_key_cfg");
        let _ = std::fs::create_dir_all(&temp_cfg);
        // Empty config so resolve_api_key("pexels") is empty.
        let _ = std::fs::write(temp_cfg.join("config.json"), r#"{"version":1,"api_keys":{}}"#);
        std::env::set_var("OPENSCRIPT_CONFIG_DIR", &temp_cfg);
        crate::config::reload_config();
        let temp_root = std::env::temp_dir().join("openscript_test_no_key");
        let _ = std::fs::create_dir_all(&temp_root);
        std::env::set_var("OPENSCRIPT_ROOT", &temp_root);

        let args = json!({
            "concepts": ["technology", "city"],
        });

        let result = handle_broll_fetch(args).await;
        std::env::remove_var("OPENSCRIPT_ROOT");
        std::env::remove_var("OPENSCRIPT_CONFIG_DIR");
        crate::config::reload_config();
        let _ = std::fs::remove_dir_all(&temp_root);
        let _ = std::fs::remove_dir_all(&temp_cfg);

        assert!(result.is_ok(), "broll.fetch should not hard-fail without PEXELS_API_KEY");
        let resp = result.unwrap();
        assert_eq!(resp["status"], "warning");
        assert_eq!(resp["missing_key"], true);
        assert_eq!(resp["results"].as_array().unwrap().len(), 0);
    }

    /// broll.fetch with no API key but WITH a fallback_pool should return
    /// one fallback entry per concept with status:warning.
    ///
    /// IMPORTANT: This test mutates process-global env vars (set_var/remove_var)
    /// and MUST be run single-threaded: cargo test -- --test-threads=1
    #[tokio::test]
    #[ignore] // env-var race — see #1132
    async fn test_broll_fetch_no_key_with_fallback_uses_fallback_pool() {
        std::env::remove_var("PEXELS_API_KEY");
        let temp_cfg = std::env::temp_dir().join("openscript_test_no_key_fb_cfg");
        let _ = std::fs::create_dir_all(&temp_cfg);
        let _ = std::fs::write(temp_cfg.join("config.json"), r#"{"version":1,"api_keys":{}}"#);
        std::env::set_var("OPENSCRIPT_CONFIG_DIR", &temp_cfg);
        crate::config::reload_config();
        let temp_root = std::env::temp_dir().join("openscript_test_no_key_fb");
        let _ = std::fs::create_dir_all(&temp_root);
        std::env::set_var("OPENSCRIPT_ROOT", &temp_root);

        let args = json!({
            "concepts": ["technology", "city"],
            "fallback_pool": ["/tmp/clip1.mp4", "/tmp/clip2.mp4"],
            "download": true,
        });

        let result = handle_broll_fetch(args).await;
        std::env::remove_var("OPENSCRIPT_ROOT");
        std::env::remove_var("OPENSCRIPT_CONFIG_DIR");
        crate::config::reload_config();
        let _ = std::fs::remove_dir_all(&temp_root);
        let _ = std::fs::remove_dir_all(&temp_cfg);

        assert!(result.is_ok());
        let resp = result.unwrap();
        assert_eq!(resp["status"], "warning");
        assert_eq!(resp["missing_key"], true);
        let results = resp["results"].as_array().unwrap();
        assert_eq!(results.len(), 2, "one fallback entry per concept");
        assert_eq!(results[0]["cached_path"], "/tmp/clip1.mp4");
        assert_eq!(results[0]["source"], "fallback_pool");
        assert_eq!(results[1]["cached_path"], "/tmp/clip2.mp4");
    }

    /// broll.fetch with no concepts should fail (required arg).
    #[tokio::test]
    async fn test_broll_fetch_missing_concepts_fails() {
        let args = json!({});
        let result = handle_broll_fetch(args).await;
        assert!(result.is_err(), "broll.fetch without concepts must fail");
    }

    /// Verify broll.fetch tool definition exposes fallback_pool in its schema.
    #[test]
    fn test_broll_fetch_schema_has_fallback_pool() {
        let tools = tool_definitions();
        let broll_fetch = tools
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "broll.fetch")
            .expect("broll.fetch tool must exist");
        let props = broll_fetch["inputSchema"]["properties"].as_object().unwrap();
        assert!(
            props.contains_key("fallback_pool"),
            "broll.fetch schema must expose fallback_pool parameter"
        );
        // Description should mention duration is now returned.
        let desc = broll_fetch["description"].as_str().unwrap();
        assert!(
            desc.contains("duration"),
            "broll.fetch description should mention duration is returned"
        );
    }
}

#[cfg(test)]
mod library_search_tests {
    use super::*;

    /// Verify library.search tool definition exposes the new filter params.
    #[test]
    fn test_library_search_schema_has_new_filters() {
        let tools = tool_definitions();
        let lib_search = tools
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "library.search")
            .expect("library.search tool must exist");
        let props = lib_search["inputSchema"]["properties"].as_object().unwrap();
        for required in ["source", "license", "min_duration_s", "max_duration_s", "tag"] {
            assert!(
                props.contains_key(required),
                "library.search schema must expose '{}' parameter (audit bug #18)",
                required
            );
        }
    }
}

#[cfg(test)]
mod background_search_tests {
    use super::*;

    /// Verify background.search tool definition exists and exposes mood filter.
    #[test]
    fn test_background_search_schema_has_mood_filter() {
        let tools = tool_definitions();
        let bg_search = tools
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "background.search")
            .expect("background.search tool must exist");
        let props = bg_search["inputSchema"]["properties"].as_object().unwrap();
        for required in ["mood", "energy", "motion_intensity", "limit"] {
            assert!(
                props.contains_key(required),
                "background.search schema must expose '{}' parameter",
                required
            );
        }
    }

    /// Verify the backgrounds_index.json exists and has mood tags.
    /// Without this index, background.search returns NotFound and the
    /// fresh-agent UX gap (#2) returns.
    #[test]
    fn test_backgrounds_index_exists_and_has_moods() {
        // Tests run with CWD = crate dir, so resolve relative to CARGO_MANIFEST_DIR.
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let index_path = format!("{}/../../mcp/assets/backgrounds_index.json", manifest_dir);
        let index_str = std::fs::read_to_string(&index_path)
            .unwrap_or_else(|_| panic!("backgrounds_index.json must exist at {}", index_path));
        let index: serde_json::Value = serde_json::from_str(&index_str).unwrap();
        let entries = index.get("entries").and_then(|v| v.as_array()).unwrap();
        assert!(
            entries.len() >= 10,
            "expected at least 10 background entries, got {}",
            entries.len()
        );
        // Every entry must have a mood field
        for entry in entries {
            let mood = entry.get("mood").and_then(|v| v.as_str());
            assert!(mood.is_some(), "every background entry must have a mood");
        }
        // Must have at least some calm clips (for healing content)
        let calm_count = entries
            .iter()
            .filter(|e| e.get("mood").and_then(|v| v.as_str()) == Some("calm"))
            .count();
        assert!(calm_count >= 3, "expected at least 3 calm backgrounds, got {}", calm_count);
    }


// ---------------------------------------------------------------------------


#[cfg(test)]
mod srt_to_timeline_tests {
    use super::*;
    use std::io::Write;
    
    #[tokio::test]
    async fn parses_srt_and_creates_timeline() {
        // Create a temp SRT file with 3 entries
        let tmp_dir = std::env::temp_dir();
        let srt_path = tmp_dir.join("test_srt_to_timeline.srt");
        let timeline_path = tmp_dir.join("test_srt_to_timeline_out.json");
        
        let srt_content = r"1
00:00:01,000 --> 00:00:04,000
Hello world this is segment one

2
00:00:05,000 --> 00:00:08,000
Second segment with more text

3
00:00:09,500 --> 00:00:12,000
Third and final segment
";
        
        {
            let mut f = std::fs::File::create(&srt_path).unwrap();
            f.write_all(srt_content.as_bytes()).unwrap();
        }
        
        // Clean up any previous output
        let _ = std::fs::remove_file(&timeline_path);
        
        // Build the args
        let args = json!({
            "srt_path": srt_path.to_str().unwrap(),
            "timeline_path": timeline_path.to_str().unwrap(),
            "aspect": "9:16",
            "fps": 30,
        });
        
        // Call the handler
        let result = handle_srt_to_timeline(args).await;
        assert!(result.is_ok(), "handler failed: {:?}", result.err());
        
        let val = result.unwrap();
        assert_eq!(val["status"], "built");
        let segments_count = val["segments_count"].as_u64().unwrap();
        assert_eq!(segments_count, 3, "expected 3 segments, got {}", segments_count);
        
        // Verify the timeline file exists and has content
        assert!(timeline_path.exists(), "timeline file should exist");
        let tl_content = std::fs::read_to_string(&timeline_path).unwrap();
        assert!(tl_content.contains("Hello world"));
        
        // Cleanup
        let _ = std::fs::remove_file(&srt_path);
        let _ = std::fs::remove_file(&timeline_path);
    }
    
    #[tokio::test]
    async fn rejects_empty_srt() {
        let tmp_dir = std::env::temp_dir();
        let srt_path = tmp_dir.join("test_empty.srt");
        
        {
            let mut f = std::fs::File::create(&srt_path).unwrap();
            f.write_all(b"").unwrap(); // Truly empty SRT
        }
        
        let args = json!({
            "srt_path": srt_path.to_str().unwrap(),
        });
        
        let result = handle_srt_to_timeline(args).await;
        assert!(result.is_err(), "should fail on empty SRT");
        // Verify it's a Srt error type
        match result.err().unwrap() {
            ToolError::Srt(_) => {}
            e => panic!("expected ToolError::Srt, got {:?}", e),
        }
        
        let _ = std::fs::remove_file(&srt_path);
    }

    #[test]
    fn test_clamp_segments_to_duration_overshoot_and_inverted() {
        use openscript_core::timeline::Segment;
        let mk = |id: &str, start: f64, end: f64| Segment {
            id: id.into(),
            start,
            end,
            caption: String::new(),
            crossfade_ms: 0,
            semantic_role: None,
        };
        let mut segs = vec![
            mk("s1", 0.0, 3.0),   // inside
            mk("s2", 3.0, 6.0),   // inside
            mk("s3", 130.0, 139.4), // ends past src_dur 135.4 → clamped
            mk("s4", 138.0, 139.0), // STARTS past src_dur → dropped (would invert)
        ];
        let (dropped, clamped) = clamp_segments_to_duration(&mut segs, 135.4);
        assert_eq!(dropped, 1, "segment starting past source must be dropped");
        assert_eq!(clamped, 1, "segment ending past source must be clamped");
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[2].end, 135.4);
        assert!(
            segs.iter().all(|s| s.start < s.end),
            "no inverted segments after clamp: {:?}",
            segs
        );
        assert!(
            segs.iter().all(|s| s.end <= 135.4 + SOURCE_DUR_TOLERANCE_S),
            "all segments within source duration"
        );
    }

    #[test]
    fn test_pexels_url_slug_extracts_video_name() {
        let url = "https://www.pexels.com/video/government-building-crowd-protest-1234567/";
        assert_eq!(pexels_url_slug(url), "Government Building Crowd Protest");
        // Trailing id must be dropped even without a trailing slash.
        let url2 = "https://www.pexels.com/video/court-justice-law-books-6101694";
        assert_eq!(pexels_url_slug(url2), "Court Justice Law Books");
        // Empty / degenerate URLs fall back to a searchable default.
        assert_eq!(pexels_url_slug(""), "stock footage");
    }

    #[test]
    fn test_cache_path_video_id_parses_pexels_id() {
        assert_eq!(
            cache_path_video_id("mcp/assets/broll_cache/court_justice_law_books_6101694.mp4"),
            Some(6101694)
        );
        assert_eq!(cache_path_video_id("no_id_here.mp4"), None);
        assert_eq!(cache_path_video_id(""), None);
    }

    #[test]
    fn test_cache_path_video_id_same_id_different_slugs() {
        // The same Pexels video cached under two different query slugs must
        // resolve to the SAME id — that is what the BROLL_REPEAT validator
        // checks by id (the phase140 bug: one crowd clip under 11 slugs).
        let a = cache_path_video_id("mcp/assets/broll_cache/crowd_people_aavaaz_35340082.mp4");
        let b = cache_path_video_id("mcp/assets/broll_cache/crowd_people_yah_35340082.mp4");
        let c = cache_path_video_id("mcp/assets/broll_cache/crowd_people_account_35340082.mp4");
        assert_eq!(a, Some(35340082));
        assert_eq!(b, Some(35340082));
        assert_eq!(c, Some(35340082));
    }

    #[test]
    fn test_candidates_covering_window_filters_short_clips() {
        use openscript_assets::pexels::PexelsVideo;
        use openscript_assets::pexels::PexelsVideoFile;
        let mk = |id: i64, dur: i64| PexelsVideo {
            id,
            width: 1080,
            height: 1920,
            url: format!("https://www.pexels.com/video/clip-{}", id),
            image: String::new(),
            video_files: vec![PexelsVideoFile {
                id,
                quality: "hd".into(),
                width: 1080,
                height: 1920,
                link: format!("https://files.example/{}.mp4", id),
                size: 1000,
            }],
            duration: dur,
        };
        let vids = vec![mk(1, 3), mk(2, 6), mk(3, 12)];
        // Window 5.0s + 0.5s slack → only 6s and 12s qualify.
        let covering = candidates_covering_window(&vids, 5.0, 0.5);
        let ids: Vec<i64> = covering.iter().map(|v| v.id).collect();
        assert_eq!(ids, vec![2, 3], "short clip must be rejected (non-looping rule)");
        // Larger window → nothing qualifies.
        assert!(candidates_covering_window(&vids, 20.0, 0.5).is_empty());
    }

    #[test]
    fn test_registry_lock_serializes_concurrent_writers() {
        // Regression: parallel registry writers (character.design_emotion,
        // voice.profile.add) lost updates without a cross-process lock. The
        // lock must serialize: while one holder has it, another acquirer
        // blocks, and the lock file must not leak after release.
        let tmp = std::env::temp_dir().join(format!(
            "os_registry_lock_test_{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&tmp);
        let target = tmp.clone();

        // Thread A acquires the lock, signals the handshake, holds 150ms.
        // The handshake (not a sleep) guarantees A owns the lock before B
        // starts timing — otherwise the assertion is scheduler-flaky.
        let a_target = target.clone();
        let a_holds = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let a_signal = a_holds.clone();
        let a = std::thread::spawn(move || {
            let _l = RegistryLock::acquire(&a_target).unwrap();
            a_signal.store(true, std::sync::atomic::Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(150));
        });
        // Spin until A confirms it holds the lock (bounded — fail fast if the
        // acquire itself is broken).
        let spin_deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !a_holds.load(std::sync::atomic::Ordering::SeqCst) {
            assert!(
                std::time::Instant::now() < spin_deadline,
                "thread A never acquired the lock"
            );
            std::thread::yield_now();
        }

        // Thread B (this thread) must block until A releases: A holds for
        // 150ms, so B's acquire must take at least ~100ms.
        let start = std::time::Instant::now();
        let _l = RegistryLock::acquire(&target).unwrap();
        let elapsed = start.elapsed();
        assert!(
            elapsed >= std::time::Duration::from_millis(100),
            "second acquirer did not block: {:?}",
            elapsed
        );
        drop(_l);
        a.join().unwrap();

        // Lock file must be gone after release (no stale locks accumulate).
        assert!(
            !std::path::Path::new(&format!("{}.lock", target.display())).exists(),
            "lock file leaked after drop"
        );
        let _ = std::fs::remove_file(&tmp);
    }
}

    #[test]
    fn test_build_voicedesign_instruct_uses_personality_and_emotion() {
        // Write a throwaway characters.json so the router can resolve the
        // character's personality + per-emotion instruct.
        let tmp = std::env::temp_dir().join(format!(
            "vd_instruct_test_{}.json",
            std::process::id()
        ));
        std::fs::write(
            &tmp,
            r#"{"detective":{"personality":"grumpy old detective, low gravelly voice","emotions":{"angry":{"instruct":"raised voice, clipped words"}}}}"#,
        )
        .unwrap();
        std::env::set_var("OPENSCRIPT_CHARACTERS_PATH", &tmp);

        let profile = openscript_tts::profiles::VoiceProfile {
            id: "detective".into(),
            provider: "voicedesign".into(),
            mode: "design".into(),
            model: String::new(),
            ref_audio: String::new(),
            ref_text: String::new(),
            language: "English".into(),
            description: None,
            sample_rate: 24000,
            created_at: String::new(),
            emotions: std::collections::HashMap::new(),
        };
        let instruct = build_voicedesign_instruct(&profile, Some("angry"), Some("teeth clenched"));
        assert!(
            instruct.contains("grumpy old detective"),
            "personality must anchor the instruct: {}",
            instruct
        );
        assert!(
            instruct.contains("raised voice, clipped words"),
            "emotion instruct must flow in: {}",
            instruct
        );
        assert!(
            instruct.contains("teeth clenched"),
            "tone must append: {}",
            instruct
        );

        // Neutral emotion → no generic "neutral delivery" suffix.
        let neutral = build_voicedesign_instruct(&profile, Some("neutral"), None);
        assert!(!neutral.contains("neutral delivery"), "neutral: {}", neutral);

        let _ = std::fs::remove_file(&tmp);
        std::env::remove_var("OPENSCRIPT_CHARACTERS_PATH");
    }

    #[test]
    fn test_build_voicedesign_instruct_falls_back_to_profile_description() {
        std::env::remove_var("OPENSCRIPT_CHARACTERS_PATH");
        let profile = openscript_tts::profiles::VoiceProfile {
            id: "hero_teen".into(),
            provider: "voicedesign".into(),
            mode: "design".into(),
            model: String::new(),
            ref_audio: String::new(),
            ref_text: String::new(),
            language: "English".into(),
            description: Some("voice.design persona: male teen, tenor, confident".into()),
            sample_rate: 24000,
            created_at: String::new(),
            emotions: std::collections::HashMap::new(),
        };
        let instruct = build_voicedesign_instruct(&profile, None, None);
        assert!(
            instruct.contains("male teen"),
            "persona fallback: {}",
            instruct
        );
        assert!(
            !instruct.contains("voice.design persona:"),
            "prefix must be stripped: {}",
            instruct
        );
    }
}

// GROUP 2c HANDLERS: ASSET DEVELOPMENT — user-curated footage library
// (asset-development pipeline; separate from the generation pipeline).
// asset.* WRITES the library index; generation only READS it (Tier 1 in
// scene_media::fetch_scene_background).
// ---------------------------------------------------------------------------







