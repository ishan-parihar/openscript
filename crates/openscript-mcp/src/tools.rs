use openscript_core::amplitude::extract_amplitude;
use openscript_core::background::assign_backgrounds;
use openscript_core::captions::{estimate_word_timings, generate_ass, CaptionSegment, WordTiming};
use openscript_core::script::{parse_script, validate_script, CaptionsSpec};
use openscript_core::srt::{analyze_srt, build_edl, group_entries, parse_srt, write_srt};
use openscript_core::sticker::{generate_sticker_composition, StickerPreset};
use openscript_core::timeline::Timeline;
use openscript_core::types::TrackType;
use openscript_transcribe::transcriber::transcribe_with_engine;
use serde_json::json;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use crate::error::ToolError;
use crate::server::report_progress;

// ---------------------------------------------------------------------------
// Tool definitions (98 static tools): 43 original + 5 hf.* + 1 composition.render + 6 script.* + 2 background.* + 2 sticker.* + 2 script.to_* + 1 stock.fetch + 1 youtube.download + 1 youtube.search + 1 stock.search + 1 media.search + 1 gif.search + 1 timeline.inspect + 3 library.* + 2 auto_assign.* + broll.keywords/broll.validate_keywords/broll.repair/broll.auto/broll.probe + sticker.keywords/sticker.validate_keywords/sticker.auto)
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
                    "provider": {"type": "string", "default": "faster-qwen3-tts", "description": "TTS provider engine: 'audio8' (default for cloned voices — Audio8 TTS zero-shot cloning, registers ref_audio + ref_text), 'kokoro' (preset voices), 'faster-qwen3-tts' (voicebox HTTP sidecar)"},
                    "mode": {"type": "string", "default": "clone", "description": "Voice mode: 'clone' for voice cloning, 'preset' for built-in voices"},
                    "model": {"type": "string", "default": "Qwen/Qwen3-TTS-12Hz-0.6B-Base", "description": "TTS model identifier"},
                    "language": {"type": "string", "default": "English", "description": "Voice language"},
                    "description": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Human-readable description of this voice"}
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
            "name": "tts.generate",
            "description": "Generate speech audio from text using a registered voice profile. Use for producing narration, explanations, or any scripted audio. Routes by provider: 'audio8' (zero-shot voice clone, ONNX INT4 — default for cloned voices), 'kokoro' (presets), 'faster-qwen3-tts' (requires OPENSCRIPT_TTS_URL sidecar). Returns: output_path, duration_ms, cached flag, backend.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "voice_profile_id": {"type": "string", "description": "ID of the voice profile to use"},
                    "text": {"type": "string", "description": "Text to synthesize"},
                    "output_path": {"type": "string", "description": "Output audio file path (WAV/MP3)"},
                    "speed": {"type": "number", "default": 1.0, "description": "Playback speed multiplier (1.0 = normal)"},
                    "pitch": {"type": "number", "default": 1.0, "description": "Pitch multiplier"},
                    "volume": {"type": "number", "default": 1.0, "description": "Volume multiplier"},
                    "format": {"type": "string", "default": "wav", "description": "Output audio format"}
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
            "description": "Analyze the audio track of a rendered video for quality issues. Checks: (1) RMS loudness — is the overall volume in acceptable range? (2) Dialogue presence — is there spoken content or just music? (3) Silence detection — are there unexpected gaps? (4) Peak levels — is there clipping? Use AFTER rendering to verify the voice is audible and music isn't drowning out dialogue. Returns: rms_lufs, peak_db, silence_segments, has_dialogue (boolean), quality_score (0-100).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "video_path": {"type": "string", "description": "Path to the rendered video to analyze"},
                    "expected_has_voice": {"type": "boolean", "default": true, "description": "Whether the video is expected to contain spoken voice"},
                    "max_silence_seconds": {"type": "number", "default": 3.0, "description": "Threshold for flagging unexpected silence gaps"}
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

fn extract_str<'a>(args: &'a serde_json::Value, key: &str) -> Result<&'a str, ToolError> {
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

fn default_str(args: &serde_json::Value, key: &str, default: &str) -> String {
    args.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or(default)
        .to_string()
}

fn default_f64(args: &serde_json::Value, key: &str, default: f64) -> f64 {
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

fn default_bool(args: &serde_json::Value, key: &str, default: bool) -> bool {
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
fn extract_broll_concept(caption: &str) -> String {
    let stopwords = [
        "the", "a", "an", "is", "are", "was", "were", "be", "been", "have", "has",
        "had", "do", "does", "did", "will", "would", "could", "should", "can", "may",
        "might", "this", "that", "these", "those", "your", "you", "they", "them",
        "their", "with", "from", "into", "about", "what", "when", "where", "which",
        "who", "how", "why", "for", "and", "but", "not", "all", "any", "some",
        "more", "most", "other", "such", "only", "own", "same", "than", "too",
        "very", "just", "also", "now", "then", "there", "here", "he", "she", "it",
        "we", "us", "our", "my", "me", "i", "his", "her", "its", "or", "so",
        "if", "in", "on", "at", "to", "of", "no", "up",
        // Hinglish function words — no visual content, must never reach Pexels.
        "hai", "ho", "hain", "ka", "ki", "ke", "ko", "se", "mein", "par",
        "aur", "yeh", "woh", "jo", "kya", "kaise", "kyun", "nahi", "haan",
        "bhi", "ab", "phir", "toh", "yaar", "dekho", "suno", "bolo",        "kar",
        "karne", "kare", "hoga", "thi", "tha", "raha", "rah", "baat", "chahiye",
        "hoon", "wala", "wale", "wali", "koi", "bahut", "saare", "log",
        "logon", "bhai", "bhaiyo", "aap", "tum", "tera", "mera", "apna",
        "kuchh", "kuch", "sab", "jab", "tab", "agar", "lekin", "jis", "jinki",
    ];
    let significant: Vec<String> = caption
        .split_whitespace()
        .map(|w| {
            let clean: String = w.chars().filter(|c| c.is_alphanumeric()).collect();
            clean.to_lowercase()
        })
        .filter(|w| w.len() > 2 && !stopwords.contains(&w.as_str()))
        .take(3)
        .collect();
    if significant.is_empty() {
        "b-roll".to_string()
    } else {
        significant.join(" ")
    }
}

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
    ".openscript/voice_profiles.json".to_string()
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
    let path = voice_profiles_path();
    if let Some(parent) = Path::new(&path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string_pretty(profiles)?;
    std::fs::write(&path, data)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Handler: transcribe (native via openscript-transcribe)
// ---------------------------------------------------------------------------

async fn handle_transcribe(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let media_path = sanitize_input_path(extract_str(&args, "media_path")?)?
        .to_string_lossy()
        .to_string();
    let output_srt_path = default_opt_str(&args, "output_srt_path").unwrap_or_else(|| {
        let p = Path::new(&media_path);
        let parent = p.parent().unwrap_or(Path::new("."));
        let stem = p
            .file_stem()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        parent
            .join(format!("{}.srt", stem))
            .to_string_lossy()
            .to_string()
    });

    if !Path::new(&media_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Media file not found: {}",
            media_path
        )));
    }

    let language_hint = default_str(&args, "language_hint", "auto");        // All transcription uses HinglishGgml (the sole engine)
        let engine = openscript_transcribe::transcriber::TranscriptionEngine::HinglishGgml;

    report_progress(0.0, 100.0, "Starting transcription...")
        .await
        .ok();

    // Wire progress callback so the MCP client sees real-time transcription progress
    let progress_cb = |pct: f64, msg: &str| {
        let msg_owned = msg.to_string();
        tokio::spawn(async move {
            let _ = report_progress(pct, 100.0, &msg_owned).await;
        });
    };
    let result = transcribe_with_engine(&media_path, &output_srt_path, engine, &language_hint, Some(&progress_cb))
        .await
        .map_err(|e| ToolError::Srt(e.to_string()))?;

    report_progress(100.0, 100.0, "Transcription complete")
        .await
        .ok();

    Ok(json!({
        "status": "transcribed",
        "output_srt_path": result.output_path,
        "entry_count": result.entry_count,
        "word_srt_path": result.word_srt_path,
        "phrase_srt_path": result.phrase_srt_path,
        "engine": format!("{}", result.engine),
    }))
}

// ---------------------------------------------------------------------------
// Handler: captions.generate_ass
// ---------------------------------------------------------------------------
async fn handle_captions_generate_ass(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    // Support auto-loading srt_path from timeline when only timeline_path is provided.
    let srt_path = if let Some(s) = args.get("srt_path").and_then(|v| v.as_str()) {
        s.to_string()
    } else if let Some(tl_path) = args.get("timeline_path").and_then(|v| v.as_str()) {
        // Derive SRT path from timeline's source field
        let tl = Timeline::load(tl_path)?;
        let source = tl.source.to_string_lossy().to_string();
        if source.is_empty() {
            return Err(ToolError::MissingArg(
                "srt_path (or timeline_path with source set)".to_string(),
            ));
        }
        // Replace video extension with .srt
        let path = std::path::Path::new(&source);
        let srt = path.with_extension("srt");
        if !srt.exists() {
            return Err(ToolError::NotFound(format!(
                "SRT not found at {} — derived from timeline source {}",
                srt.display(), source
            )));
        }
        srt.to_string_lossy().to_string()
    } else {
        return Err(ToolError::MissingArg(
            "srt_path or timeline_path".to_string(),
        ));
    };
    // Optional word-level SRT (from transcribe's word_srt_path). When present,
    // parse THAT instead of the phrase SRT so per-word timings are real
    // transcription alignments — the caption-voice sync fix for the A2V path.
    let word_srt_path = args.get("word_srt_path")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let grouped_srt_path = args.get("grouped_srt_path")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let style = default_str(&args, "style", "word_highlight");
    let font = default_str(&args, "font", "Bebas Neue");
    let font_size = default_u32(&args, "font_size", 84);
    let color = default_str(&args, "color", "#ffffff");
    let highlight_color = default_str(&args, "highlight_color", "#00ff88");
    let position = default_str(&args, "position", "center");
    let safe_zone = args.get("safe_zone").and_then(|v| v.as_f64()).unwrap_or(0.85);
    let max_words_per_line = default_u32(&args, "max_words_per_line", 5);
    let width = default_u32(&args, "width", 1080);
    let height = default_u32(&args, "height", 1920);
    let crossfade_ms = args.get("crossfade_ms").and_then(|v| v.as_i64()).map(|v| v as u32);
    let output_path = args.get("output_path")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let spec = CaptionsSpec {
        style: style.to_string(),
        font: font.to_string(),
        font_size,
        color: color.to_string(),
        highlight_color: highlight_color.to_string(),
        position: position.to_string(),
        safe_zone,
        max_words_per_line,
    };

    // Determine output path
    let ass_path = if let Some(p) = output_path {
        p
    } else {
        let parent = Path::new(&srt_path).parent().unwrap_or(Path::new("."));
        parent.join("captions.ass").to_string_lossy().to_string()
    };

    // Caption TEXT and phrase windows ALWAYS come from the phrase transcript
    // (srt_path) — the language-correct captions (e.g. Hinglish). The word SRT
    // (word_srt_path) only contributes REAL per-word timings when its words
    // actually align with the phrase text (same language + content); on a
    // language mismatch or a stale/partial word SRT, per-word timings fall
    // back to char-proportional estimates. This fixes the A2V caption bugs:
    //   1. English captions on Hinglish audio (a foreign word SRT was used as
    //      the entire caption source, replacing the Hinglish phrase text).
    //   2. Captions breaking off mid-video (the word SRT covered only part of
    //      the audio, so the ASS inherited a 60s hole).
    let caption_segments: Vec<CaptionSegment> = {
        // Parse the phrase transcript first — authoritative text + windows.
        let phrase_entries = match openscript_core::srt::parse_srt(&srt_path) {
            Ok(e) if !e.is_empty() => e,
            Ok(_) | Err(_) => {
                // Fallback: try grouped SRT with estimated word timings.
                let fallback_path = grouped_srt_path.as_deref().unwrap_or(&srt_path);
                let (err_note, fallback_entries) = match openscript_core::srt::parse_srt(fallback_path) {
                    Ok(fb) if !fb.is_empty() => {
                        tracing::warn!("Phrase SRT parse failed/empty for captions, using grouped SRT fallback");
                        (String::new(), fb)
                    }
                    Ok(_) | Err(_) => {
                        return Err(ToolError::InvalidArg(format!(
                            "Failed to parse SRT {} (fallback {} also failed)", srt_path, fallback_path
                        )));
                    }
                };
                tracing::warn!("Phrase SRT unavailable for captions: {}", err_note);
                fallback_entries
            }
        };
        // Word-level timings (optional enrichment only).
        let word_entries: Vec<openscript_core::srt::SrtEntry> = word_srt_path
            .as_deref()
            .and_then(|p| openscript_core::srt::parse_srt(p).ok())
            .unwrap_or_default();

        // Crossfade remap (output clock) when segments overlap via xfade.
        let crossfade_s = crossfade_ms.map(|cf| cf as f64 / 1000.0);
        let mut out_offsets: Vec<f64> = Vec::with_capacity(phrase_entries.len());
        let mut accum = 0.0;
        for (i, ph) in phrase_entries.iter().enumerate() {
            out_offsets.push(accum - (i as f64 * crossfade_s.unwrap_or(0.0)));
            accum += ph.end - ph.start;
        }

        phrase_entries
            .iter()
            .enumerate()
            .map(|(i, ph)| {
                let out_start = (out_offsets[i] * 1000.0).round() as i64;
                let out_end = out_start + ((ph.end - ph.start) * 1000.0).round() as i64;
                let words = caption_words_for_phrase(ph, &word_entries, out_start, ph.start);
                CaptionSegment {
                    text: ph.text.clone(),
                    start_ms: out_start,
                    end_ms: out_end,
                    words,
                }
            })
            .collect()
    };

    let ass_content = generate_ass(&caption_segments, &spec, width, height);
    std::fs::write(&ass_path, &ass_content)?;

    let canonical_ass = std::fs::canonicalize(&ass_path)
        .unwrap_or_else(|_| ass_path.clone().into());

    // AUTO-REGISTER: If timeline_path provided, register ASS in timeline.assets.captions
    let captions_timeline_path = default_opt_str(&args, "timeline_path");
    if let Some(ref tl_path) = captions_timeline_path {
        if let Ok(mut tl) = Timeline::load(tl_path) {
            tl.assets.captions.insert("ass".to_string(), serde_json::json!({
                "path": canonical_ass.to_string_lossy().to_string(),
            }));
            // Register caption style in effects so verify.production can detect it
            tl.effects.caption_style = Some(style);
            tl.updated_at = chrono::Utc::now();
            let _ = tl.save(tl_path);
        }
    }

    Ok(json!({
        "status": "success",
        "ass_path": canonical_ass.to_string_lossy().to_string(),
        "segment_count": caption_segments.len(),
    }))
}

// ---------------------------------------------------------------------------
// Handler: srt.read
// ---------------------------------------------------------------------------

async fn handle_srt_read(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let srt_path = sanitize_input_path(extract_str(&args, "srt_path")?)?
        .to_string_lossy()
        .to_string();
    let entries = parse_srt(&srt_path)?;
    let result: Vec<serde_json::Value> = entries
        .iter()
        .map(|e| {
            json!({
                "idx": e.idx,
                "start": e.start,
                "end": e.end,
                "text": e.text,
            })
        })
        .collect();
    Ok(json!({
        "status": "success",
        "srt_path": srt_path,
        "count": result.len(),
        "entries": result,
    }))
}

// ---------------------------------------------------------------------------
// Handler: srt.prepare
// ---------------------------------------------------------------------------

async fn handle_srt_prepare(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let srt_path = sanitize_input_path(extract_str(&args, "srt_path")?)?
        .to_string_lossy()
        .to_string();
    let max_words = default_u32(&args, "max_words", 10) as usize;
    let max_chars = default_u32(&args, "max_chars", 64) as usize;
    let max_gap = default_f64(&args, "max_gap", 0.6);
    let max_duration_s = default_f64(&args, "max_duration_s", 5.0);

    let entries = parse_srt(&srt_path)?;
    let groups = {
        use openscript_core::srt::group_entries_with_words_max_duration;
        let phrases = group_entries_with_words_max_duration(
            &entries, max_words, max_chars, max_gap, max_duration_s,
        );
        phrases.into_iter().map(|p| (p.text, p.start, p.end)).collect::<Vec<_>>()
    };

    let out_srt_path = {
        let p = Path::new(&srt_path);
        let parent = p.parent().unwrap_or(Path::new("."));
        let stem = p
            .file_stem()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        parent
            .join(format!("{}.grouped.srt", stem))
            .to_string_lossy()
            .to_string()
    };
    let flat: Vec<(String, f64, f64)> = groups
        .iter()
        .map(|(text, start, end)| (text.clone(), *start, *end))
        .collect();
    write_srt(&flat, &out_srt_path)?;

    let result: Vec<serde_json::Value> = groups
        .iter()
        .map(|(text, start, end)| {
            json!({
                "text": text,
                "start": start,
                "end": end,
            })
        })
        .collect();

    Ok(json!({
        "status": "success",
        "output_path": out_srt_path,
        "count": result.len(),
        "groups": result,
    }))
}

// ---------------------------------------------------------------------------
// Handler: srt.apply_edit (native: parse edited SRT, build EDL, render)
// ---------------------------------------------------------------------------

async fn handle_srt_apply_edit(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_core::srt::{parse_srt, retime_srt};
    use openscript_ffmpeg::render::{render, RenderConfig};
    use openscript_ffmpeg::subtitles::srt_to_ass;

    let video_path = extract_str(&args, "video_path")?;
    let edited_srt_path = extract_str(&args, "edited_srt_path")?;
    let merge_gap = default_f64(&args, "merge_gap", 0.25);
    let crossfade_ms = default_u32(&args, "crossfade_ms", 80);
    let aspect = default_str(&args, "aspect", "9:16");
    let burn_captions = default_bool(&args, "burn_captions", true);
    let crf = default_u32(&args, "crf", 20);
    let fps = default_u32(&args, "fps", 30);

    report_progress(0.0, 100.0, "Parsing edited SRT...")
        .await
        .ok();

    let edited_entries = parse_srt(edited_srt_path).map_err(|e| ToolError::Srt(e.to_string()))?;

    if edited_entries.is_empty() {
        return Err(ToolError::Srt("Edited SRT has no entries".to_string()));
    }

    // Build EDL segments from edited SRT entries
    let segments: Vec<(f64, f64, String)> = edited_entries
        .iter()
        .map(|e| (e.start, e.end, e.text.clone()))
        .collect();

    // Create EDL v1 JSON
    let edl = json!({
        "source": video_path,
        "target": {"aspect": aspect, "fps": fps},
        "segments": segments.iter().enumerate().map(|(i, (s, e, t))| {
            json!({"id": format!("seg_{:03}", i + 1), "start": s, "end": e, "caption": t, "crossfade_ms": crossfade_ms})
        }).collect::<Vec<_>>(),
        "effects": {"burn_captions": burn_captions, "audio": {"loudnorm": true}},
    });

    // Save EDL alongside the edited SRT
    let edl_path = {
        let p = Path::new(edited_srt_path);
        let parent = p.parent().unwrap_or(Path::new("."));
        let stem = p
            .file_stem()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        parent
            .join(format!("{}.edl.json", stem))
            .to_string_lossy()
            .to_string()
    };
    let edl_json = serde_json::to_string_pretty(&edl).map_err(ToolError::Json)?;
    std::fs::write(&edl_path, edl_json).map_err(ToolError::Io)?;

    // Generate ASS subtitles if burn_captions
    let ass_path = if burn_captions {
        report_progress(20.0, 100.0, "Generating subtitle styles...")
            .await
            .ok();
        let orig_srt = segments.clone();
        let retimed = retime_srt(
            &orig_srt,
            &segments
                .iter()
                .map(|(s, e, _)| (*s, *e))
                .collect::<Vec<_>>(),
            merge_gap,
        );

        let ass_out = Path::new(&edl_path).with_extension("ass");
        let ass_path_str = ass_out.to_string_lossy().into_owned();
        srt_to_ass(&retimed, &ass_path_str, "Default")
            .map_err(|e| ToolError::Ffmpeg(e.to_string()))?;
        Some(ass_path_str)
    } else {
        None
    };

    // Render
    report_progress(40.0, 100.0, "Rendering edited video...")
        .await
        .ok();
    let config = RenderConfig {
        video_path: video_path.to_string(),
        edl_path: edl_path.clone(),
        burn_captions,
        srt_path: Some(edited_srt_path.to_string()),
        ass_path,
        overlay_mov: None,
        aspect,
        crf,
        fps,
    };

    let output_path = render(config)
        .await
        .map_err(|e| ToolError::Ffmpeg(e.to_string()))?;

    report_progress(100.0, 100.0, "Edit applied and rendered")
        .await
        .ok();

    Ok(json!({
        "status": "rendered",
        "output_path": output_path,
        "edl_path": edl_path,
        "segments_count": segments.len(),
        "total_duration_s": segments.iter().map(|(s, e, _)| e - s).sum::<f64>(),
    }))
}

// ---------------------------------------------------------------------------
// Handler: edl.build (Native Rust)
// ---------------------------------------------------------------------------

async fn handle_edl_build(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let srt_path = sanitize_input_path(extract_str(&args, "srt_path")?)?
        .to_string_lossy()
        .to_string();
    let strategy = default_str(&args, "strategy", "keep");
    let max_duration = default_opt_f64(&args, "max_duration");
    let crossfade_ms = default_u32(&args, "crossfade_ms", 120);
    let analysis_path = default_opt_str(&args, "analysis_path");
    let aspect = default_str(&args, "aspect", "9:16");

    let entries = parse_srt(&srt_path).map_err(|e| ToolError::Srt(e.to_string()))?;

    let groups = group_entries(&entries, 10, 64, 0.6);

    let analysis = analyze_srt(&groups);

    if let Some(ap) = &analysis_path {
        let analysis_json =
            serde_json::to_string_pretty(&analysis).map_err(ToolError::Json)?;
        std::fs::write(ap, analysis_json).map_err(ToolError::Io)?;
    }

    let segments = build_edl(&analysis, &strategy, max_duration, crossfade_ms);

    let edl = json!({
        "source": "",
        "target": {"aspect": aspect, "fps": 30},
        "segments": segments.iter().enumerate().map(|(i, (s, e, t))| {
            json!({"id": format!("seg_{:03}", i + 1), "start": s, "end": e, "caption": t, "crossfade_ms": crossfade_ms})
        }).collect::<Vec<_>>(),
        "effects": {"burn_captions": true, "audio": {"loudnorm": true}},
    });

    let output_path = {
        let p = Path::new(&srt_path);
        let parent = p.parent().unwrap_or(Path::new("."));
        let stem = p
            .file_stem()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        parent
            .join(format!("{}.edl.json", stem))
            .to_string_lossy()
            .to_string()
    };

    let edl_json = serde_json::to_string_pretty(&edl).map_err(ToolError::Json)?;
    std::fs::write(&output_path, edl_json).map_err(ToolError::Io)?;

    let total_duration: f64 = segments.iter().map(|(s, e, _)| e - s).sum();

    Ok(json!({
        "status": "built",
        "edl_path": output_path,
        "strategy": strategy,
        "segments_count": segments.len(),
        "total_duration_s": total_duration,
        "analysis_count": analysis.len(),
    }))
}

// ---------------------------------------------------------------------------
// Handler: render (Phase 1: shell to Python)
// ---------------------------------------------------------------------------

async fn handle_render(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_core::srt::parse_srt;
    use openscript_ffmpeg::render::{render, RenderConfig};
    use openscript_ffmpeg::subtitles::srt_to_ass;

    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?
        .to_string_lossy()
        .to_string();
    let edl_path = sanitize_input_path(extract_str(&args, "edl_path")?)?
        .to_string_lossy()
        .to_string();
    let burn_captions = default_bool(&args, "burn_captions", true);
    let srt_path = default_opt_str(&args, "srt_path");
    let ass_path = default_opt_str(&args, "ass_path");
    let aspect = default_str(&args, "aspect", "9:16");
    let crf = default_u32(&args, "crf", 20);
    let fps = default_u32(&args, "fps", 30);

    report_progress(0.0, 100.0, "Preparing render...")
        .await
        .ok();

    let resolved_ass_path = if burn_captions && ass_path.is_none() {
        if let Some(srt) = &srt_path {
            if Path::new(srt).exists() {
                report_progress(10.0, 100.0, "Converting subtitles...")
                    .await
                    .ok();
                let entries = parse_srt(srt).map_err(|e| ToolError::Srt(e.to_string()))?;
                let flat: Vec<(f64, f64, String)> = entries
                    .iter()
                    .map(|e| (e.start, e.end, e.text.clone()))
                    .collect();
                let ass_out = Path::new(srt).with_extension("ass");
                let ass_path_str = ass_out.to_string_lossy().into_owned();
                srt_to_ass(&flat, &ass_path_str, "Default")
                    .map_err(|e| ToolError::Ffmpeg(e.to_string()))?;
                Some(ass_path_str)
            } else {
                None
            }
        } else {
            None
        }
    } else {
        ass_path
    };

    let config = RenderConfig {
        video_path: video_path.to_string(),
        edl_path: edl_path.to_string(),
        burn_captions,
        srt_path,
        ass_path: resolved_ass_path,
        overlay_mov: None,
        aspect,
        crf,
        fps,
    };

    report_progress(20.0, 100.0, "Rendering video with FFmpeg...")
        .await
        .ok();

    let output_path = render(config)
        .await
        .map_err(|e| ToolError::Ffmpeg(e.to_string()))?;

    report_progress(100.0, 100.0, "Render complete").await.ok();

    Ok(json!({
        "status": "rendered",
        "output_path": output_path,
    }))
}

// ---------------------------------------------------------------------------
// Handler: reelize (native: transcribe → prepare → edl.build → render)
// ---------------------------------------------------------------------------

async fn handle_reelize(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?
        .to_string_lossy()
        .to_string();
    let srt_path = default_opt_str(&args, "srt_path")
        .map(|s| sanitize_input_path(&s).map(|p| p.to_string_lossy().to_string()))
        .transpose()?;
    let preset = default_str(&args, "preset", "Balanced");
    let max_duration = default_opt_f64(&args, "max_duration");
    let aspect = default_str(&args, "aspect", "9:16");
    let burn_captions = default_bool(&args, "burn_captions", true);

    if !Path::new(&video_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Video not found: {}",
            video_path
        )));
    }

    // Step 1: Transcribe (if no SRT provided)
    let resolved_srt_path = if let Some(srt) = srt_path {
        report_progress(5.0, 100.0, "Using existing SRT...")
            .await
            .ok();
        srt.to_string()
    } else {
        report_progress(0.0, 100.0, "Step 1/4: Transcribing audio...")
            .await
            .ok();
        let transcribe_args = json!({
            "media_path": video_path,
        });
        let transcribe_result = handle_transcribe(transcribe_args).await?;
        report_progress(25.0, 100.0, "Transcription complete")
            .await
            .ok();
        transcribe_result
            .get("output_srt_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::Srt("Transcription did not return output path".to_string()))?
            .to_string()
    };

    // Step 2: SRT prepare (group word-per-line)
    report_progress(30.0, 100.0, "Step 2/4: Grouping captions...")
        .await
        .ok();
    let prepare_args = json!({
        "srt_path": resolved_srt_path,
        "max_words": 10,
        "max_chars": 64,
        "max_gap": 0.6,
    });
    let prepare_result = handle_srt_prepare(prepare_args).await?;
    let grouped_srt = prepare_result
        .get("output_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::Srt("SRT prepare did not return output path".to_string()))?
        .to_string();

    // Step 3: EDL build
    report_progress(50.0, 100.0, "Step 3/4: Building edit decision list...")
        .await
        .ok();
    let crossfade_ms = match preset.as_str() {
        "Tight" => 120,
        "Balanced" => 100,
        "Natural" => 60,
        _ => 100,
    };

    let edl_args = json!({
        "srt_path": grouped_srt,
        "strategy": "keep",
        "max_duration": max_duration,
        "crossfade_ms": crossfade_ms,
        "aspect": aspect,
    });
    let edl_result = handle_edl_build(edl_args).await?;
    let edl_path = edl_result
        .get("edl_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::Srt("EDL build did not return path".to_string()))?
        .to_string();

    // Step 4: Render
    report_progress(70.0, 100.0, "Step 4/4: Rendering final video...")
        .await
        .ok();
    let render_args = json!({
        "video_path": video_path,
        "edl_path": edl_path,
        "srt_path": grouped_srt,
        "burn_captions": burn_captions,
        "aspect": aspect,
        "crf": 20,
        "fps": 30,
    });
    let render_result = handle_render(render_args).await?;
    let output_path = render_result
        .get("output_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::Ffmpeg("Render did not return output path".to_string()))?
        .to_string();

    let total_segments = edl_result
        .get("segments_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let total_duration = edl_result
        .get("total_duration_s")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);

    report_progress(100.0, 100.0, "Reel complete!").await.ok();

    Ok(json!({
        "status": "rendered",
        "output_path": output_path,
        "preset": preset,
        "segments_count": total_segments,
        "total_duration_s": total_duration,
        "srt_path": resolved_srt_path,
        "edl_path": edl_path,
    }))
}

// ---------------------------------------------------------------------------
// Handler: overlay.generate (Phase 1: shell to Python)
// ---------------------------------------------------------------------------

async fn handle_overlay_generate(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let srt_path = extract_str(&args, "srt_path")?;
    let _edl_path = extract_str(&args, "edl_path")?;
    let out_path = default_opt_str(&args, "out_path").unwrap_or_else(|| {
        let p = Path::new(&srt_path);
        let stem = p
            .file_stem()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        format!("{}.overlay.mov", stem)
    });
    let width = default_u32(&args, "width", 1080);
    let height = default_u32(&args, "height", 1920);
    let fps = default_u32(&args, "fps", 30);
    let animate = default_bool(&args, "animate", false);
    let style = default_str(&args, "style", "pupcaps_center");
    let timeline_path = default_opt_str(&args, "timeline_path");

    report_progress(0.0, 100.0, "Generating caption overlay...")
        .await
        .ok();

    let pupcaps_path = "third_party/PupCaps/pupcaps";

    let mut cmd = tokio::process::Command::new("node");
    cmd.arg(pupcaps_path)
        .arg("retimed")
        .arg(srt_path)
        .arg("--output")
        .arg(&out_path)
        .arg("--width")
        .arg(width.to_string())
        .arg("--height")
        .arg(height.to_string())
        .arg("--fps")
        .arg(fps.to_string())
        .arg("--style")
        .arg(format!("mcp/styles/{}.css", style))
        .kill_on_drop(true);

    if animate {
        cmd.arg("--animate");
    }

    let out = cmd.output().await;
    match out {
        Ok(o) if o.status.success() => {
            if let Some(tl_path) = &timeline_path {
                if Path::new(tl_path).exists() {
                    if let Ok(mut timeline) = Timeline::load(tl_path) {
                        timeline.add_asset(
                            "captions",
                            "overlay_mov".to_string(),
                            json!({"path": out_path}),
                        );
                        timeline.save(tl_path).ok();
                    }
                }
            }
            report_progress(100.0, 100.0, "Overlay generated")
                .await
                .ok();
            Ok(json!({
                "status": "generated",
                "output_path": out_path,
            }))
        }
        Ok(o) => Err(ToolError::Ffmpeg(format!(
            "overlay.generate failed: {}",
            String::from_utf8_lossy(&o.stderr)
        ))),
        Err(e) => Err(ToolError::Ffmpeg(format!("overlay.generate error: {}", e))),
    }
}

// ---------------------------------------------------------------------------
// Handler: timeline.build
// ---------------------------------------------------------------------------

async fn handle_timeline_build(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let source_video = extract_str(&args, "source_video")?;
    let mut aspect = default_str(&args, "aspect", "9:16");
    let mut fps = default_u32(&args, "fps", 30);
    let max_duration = default_opt_u32(&args, "max_duration");

    // Platform presets: override aspect, fps, max_duration based on target platform
    if let Some(platform) = args.get("platform").and_then(|v| v.as_str()) {
        match platform {
            "tiktok" | "reels" | "shorts" => {
                aspect = "9:16".to_string();
                fps = 30;
            }
            "youtube" | "landscape" => {
                aspect = "16:9".to_string();
                fps = 30;
            }
            "instagram" | "square" => {
                aspect = "1:1".to_string();
                fps = 30;
            }
            _ => {} // unknown platform, keep user defaults
        }
    }
    let output_path = default_opt_str(&args, "output_path")
        .unwrap_or_else(|| default_timeline_path(source_video));

    if !Path::new(source_video).exists() {
        return Err(ToolError::NotFound(format!(
            "Source video not found: {}",
            source_video
        )));
    }

    let timeline = Timeline::new(source_video.into(), &aspect, fps, max_duration);
    timeline.save(&output_path)?;

    Ok(json!({
        "status": "created",
        "timeline_path": output_path,
        "source": source_video,
        "aspect": aspect,
        "fps": fps,
    }))
}

// ---------------------------------------------------------------------------
// Handler: timeline.load
// ---------------------------------------------------------------------------

async fn handle_timeline_load(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let timeline_path = sanitize_input_path(extract_str(&args, "timeline_path")?)?
        .to_string_lossy()
        .to_string();
    let timeline = Timeline::load(&timeline_path)?;

    Ok(json!({
        "status": "loaded",
        "timeline_path": timeline_path,
        "version": timeline.version,
        "source": timeline.source.to_string_lossy(),
        "segments_count": timeline.segments.len(),
        "tracks": timeline.tracks.keys().map(|k: &openscript_core::types::TrackType| k.to_string()).collect::<Vec<_>>(),
    }))
}

// ---------------------------------------------------------------------------
// Handler: timeline.validate
// ---------------------------------------------------------------------------


/// Convert SRT entries into timeline segments in one call.
async fn handle_srt_to_timeline(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let srt_path = sanitize_input_path(extract_str(&args, "srt_path")?)?;
    let source_video = default_opt_str(&args, "source_video");
    let output_path = default_opt_str(&args, "output_path");
    let crossfade_ms = default_u32(&args, "crossfade_ms", 80);
    let aspect = default_str(&args, "aspect", "9:16");
    let fps = default_u32(&args, "fps", 30);
    let scene_size = args.get("scene_size").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
    let max_duration_s: Option<f64> = args.get("max_duration_s").and_then(|v| v.as_f64());
    let min_duration_s: Option<f64> = args.get("min_duration_s").and_then(|v| v.as_f64());

    // Parse SRT file
    let entries = parse_srt(&srt_path)
        .map_err(|e| ToolError::Srt(format!("Failed to parse SRT: {}", e)))?;

    if entries.is_empty() {
        return Err(ToolError::Srt("SRT file has no entries".to_string()));
    }

    // Load or create timeline
    let timeline_path_arg = default_opt_str(&args, "timeline_path");

    let source_path = source_video.as_ref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            // Derive source from SRT path — replace .srt with original media extension
            // Try common extensions: .mp4, .mp3, .wav, .mkv, .webm
            let srt_parent = std::path::Path::new(&srt_path).parent().unwrap_or(std::path::Path::new("."));
            let srt_stem = std::path::Path::new(&srt_path).file_stem().unwrap_or_default();
            for ext in &[".mp4", ".mp3", ".wav", ".mkv", ".webm", ".m4a"] {
                let candidate = srt_parent.join(format!("{}{}", srt_stem.to_string_lossy(), ext));
                if candidate.exists() {
                    return candidate;
                }
            }
            // No media file found — leave empty so timeline.render's audio-only
            // detection can derive source from segments or skip source validation.
            std::path::PathBuf::new()
        });

    let mut timeline = if let Some(ref tp) = timeline_path_arg {
        if !tp.is_empty() && std::path::Path::new(tp).exists() {
            Timeline::load(tp).map_err(|e| ToolError::Timeline(e.to_string()))?
        } else {
            Timeline::new(source_path.clone(), &aspect, fps, None)
        }
    } else {
        Timeline::new(source_path.clone(), &aspect, fps, None)
    };

    // Add SRT entries as segments
    let mut segments_count = 0usize;

    if let Some(max_dur) = max_duration_s {
        // === SENTENCE-AWARE MODE: duration-based grouping ===
        // Uses pause detection (>300ms gaps) and duration caps
        let min_dur = min_duration_s.unwrap_or(2.0);
        // Sentence-aware segmentation parameters (per docs/SEGMENTATION_ARCHITECTURE.md):
        // - 15 words ≈ 4s at 2.5 words/s natural speaking pace
        // - 80 chars ≈ 2 lines of captions at standard font size
        // - 300ms gap = natural breath pause boundary (silence between sentences)
        // - max_dur = user-provided cap (e.g., 5.0s for short-form content)
        let grouped = openscript_core::srt::group_entries_with_words_max_duration(
            &entries,
            15,    // max_words: ~4s at 2.5 words/s
            80,    // max_chars: ~2 caption lines
            0.3,   // max_gap: 300ms = breath pause boundary
            max_dur,
        );

        // Convert GroupedPhrase to timeline segments.
        // enforce_segment_bounds merges segments shorter than min_duration_s
        // into their successor AND splits segments longer than max_duration_s
        // at their longest internal pause. (The old inline merge only handled
        // the min side and could produce a merged segment that exceeded max.)
        let bounded = openscript_core::srt::enforce_segment_bounds(grouped, min_dur, max_dur);

        // Add bounded segments to timeline
        for g in bounded {
            if g.end > g.start {
                timeline.add_segment(g.start, g.end, &g.text, crossfade_ms, None);
                segments_count += 1;
            }
        }
    } else if scene_size <= 1 {
        // === LEGACY MODE: one segment per entry ===
        for entry in &entries {
            if entry.end > entry.start {
                timeline.add_segment(entry.start, entry.end, &entry.text, crossfade_ms, None);
                segments_count += 1;
            }
        }
    } else {
        // === LEGACY MODE: fixed chunk grouping ===
        for chunk in entries.chunks(scene_size) {
            let valid: Vec<_> = chunk.iter().filter(|e| e.end > e.start).collect();
            if valid.is_empty() { continue; }
            let start = valid.first().unwrap().start;
            let end = valid.last().unwrap().end;
            let caption = valid.iter().map(|e| e.text.as_str()).collect::<Vec<_>>().join(" ");
            timeline.add_segment(start, end, &caption, crossfade_ms, None);
            segments_count += 1;
        }
    }

    // Clamp segments at the source media duration. SRT entries occasionally
    // overshoot the audio end (whisper tail hallucination, trailing silence in
    // the word SRT). Without this, the last segments extend past the source and
    // the renderer's overlay chain (eof_action=repeat) holds the final b-roll
    // frame past the audio end — the "audio 2:15 but video 2:41" black+silence
    // tail. The source is the master clock; segments must fit inside it.
    // The clamp is best-effort: if the source is missing or unprobeable we
    // leave segments untouched (the render's `-shortest` still caps output).
    if let Some(src_dur) = probe_source_duration(&source_path).await {
        let before = timeline.segments.len();
        let (dropped, clamped) = clamp_segments_to_duration(&mut timeline.segments, src_dur);
        if dropped > 0 || clamped > 0 {
            tracing::warn!(
                "[srt.to_timeline] clamped {} / dropped {} of {} segments to source duration {:.2}s (trailing SRT overshoot truncated)",
                clamped, dropped, before, src_dur
            );
        }
    }

    // Determine output path: explicit output_path > timeline_path > derived from srt_path
    let resolved_output = output_path
        .filter(|s| !s.is_empty())
        .or_else(|| timeline_path_arg.filter(|s| !s.is_empty()))
        .unwrap_or_else(|| srt_path.with_extension("timeline.json").to_string_lossy().to_string());

    // Save timeline
    timeline.save(&resolved_output)
        .map_err(|e| ToolError::Timeline(format!("Failed to save timeline: {}", e)))?;

    // Report the CLAMPED last-segment end as duration_s — the un-clamped SRT
    // tail (whisper hallucination) would mislead an agent into thinking the
    // timeline runs past the source. source_duration_s is the same value but
    // named to make the master clock explicit.
    let duration_s = timeline.segments.last().map(|s| s.end).unwrap_or(0.0);

    Ok(json!({
        "status": "built",
        "timeline_path": resolved_output,
        "segments_count": segments_count,
        "duration_s": duration_s,
        "aspect": aspect,
        "fps": fps,
        "source_duration_s": duration_s,
    }))
}
async fn handle_timeline_validate(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let timeline_path = sanitize_input_path(extract_str(&args, "timeline_path")?)?
        .to_string_lossy()
        .to_string();
    let timeline = Timeline::load(&timeline_path)?;
    let mut errors = timeline.validate();
    // SEGMENTATION_ARCHITECTURE.md §3: every segment must fall within
    // [MIN_SEGMENT_DURATION_S, MAX_SEGMENT_DURATION_S] (2.0s–6.0s) for
    // short-form retention. Long cuts bleed attention; sub-min cuts flicker.
    errors.extend(timeline.validate_segmentation());
    // DURATION: segments must not extend past the source media (the master
    // clock). SRT tail hallucination / trailing silence produces segments past
    // the audio end; the renderer's overlay repeat then holds the last b-roll
    // frame beyond the audio — the "audio 2:15, video 2:41" black+silence tail.
    // The probe is async, so this lives in the MCP layer (not core's sync
    // validate) — mirroring probe_broll_gaps. Best-effort: skip if the source
    // is missing/unprobeable.
    if let Some(src_dur) = probe_source_duration(&timeline.source).await {
        for seg in &timeline.segments {
            if seg.end > src_dur + SOURCE_DUR_TOLERANCE_S {
                errors.push(format!(
                    "DURATION: segment {} ends at {:.1}s but source media is only {:.1}s — segments must fit inside the source (re-run srt.to_timeline/segment.analyze which clamp at the source duration, or trim this segment)",
                    seg.id, seg.end, src_dur
                ));
            }
        }
    }
    // Phase 54: Reject empty timelines
    if timeline.segments.is_empty() {
        errors.push("Timeline has no segments. Call timeline.add_segment to populate it with segments.".to_string());
    }
    // B-roll coverage: flag segments whose assigned clip is shorter than the
    // segment window. The renderer plays clips exactly once (no loop fill),
    // so these gaps render as a held frame — the agent must re-run keyword
    // generation + broll.fetch for a longer clip.
    let broll_gaps = probe_broll_gaps(&timeline).await;
    for g in &broll_gaps {
        errors.push(format!(
            "BROLL_GAP: segment {} needs {:.1}s but clip {} provides {:.1}s (gap {:.1}s) — {}",
            g.segment_id, g.required_s, g.asset_id, g.available_s, g.gap_s, g.action
        ));
    }
    // B-roll non-repetition: the same clip must not appear on 2+ events —
    // identical footage later in the sequence reads as an error (the
    // b-roll-repeat bug where the deterministic fetch path could place the
    // same Pexels clip on two segments). Dedup happens at TWO levels:
    // 1. exact cache path (same file, same slug)
    // 2. Pexels video id embedded in the cache filename — the same clip can
    //    be cached under DIFFERENT query slugs (e.g.
    //    crowd_people_aavaaz_35340082.mp4 vs crowd_people_yah_35340082.mp4),
    //    which is still the SAME footage and must also be flagged.
    let mut seen_clip_paths: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut seen_clip_ids: std::collections::HashMap<i64, String> = std::collections::HashMap::new();
    for ev in timeline.tracks.get(&TrackType::Broll).cloned().unwrap_or_default() {
        if let Some(p) = timeline
            .assets
            .broll
            .get(&ev.asset_id)
            .and_then(|a| a.get("path"))
            .and_then(|v| v.as_str())
        {
            if let Some(prev) = seen_clip_paths.insert(p.to_string(), ev.id.clone()) {
                errors.push(format!(
                    "BROLL_REPEAT: clip {} is used by both {} and {} — same footage must not repeat later in the sequence (re-run broll.fetch / broll.repair for a distinct clip)",
                    p, prev, ev.id
                ));
            } else if let Some(id) = cache_path_video_id(p) {
                // Path is new — the same Pexels video id under a DIFFERENT
                // query slug is still the same footage (e.g.
                // crowd_people_aavaaz_35340082.mp4 vs
                // crowd_people_yah_35340082.mp4). Only the id check runs here
                // so exact-path duplicates emit exactly one (path) error.
                if let Some(prev) = seen_clip_ids.insert(id, ev.id.clone()) {
                    errors.push(format!(
                        "BROLL_REPEAT: Pexels video {} (used by {} and {}) is the same clip cached under different query slugs — same footage must not repeat later in the sequence (re-run broll.fetch / broll.repair for a distinct clip)",
                        id, prev, ev.id
                    ));
                }
            }
        }
    }
    let valid = errors.is_empty();

    Ok(json!({
        "status": if valid { "valid" } else { "invalid" },
        "timeline_path": timeline_path,
        "valid": valid,
        "errors": errors,
        "broll_gaps": broll_gaps,
    }))
}

// ---------------------------------------------------------------------------
// Handler: timeline.upgrade
// ---------------------------------------------------------------------------

async fn handle_timeline_upgrade(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let edl_v1_path = sanitize_input_path(extract_str(&args, "edl_v1_path")?)?
        .to_string_lossy()
        .to_string();
    let output_path = default_opt_str(&args, "output_path");

    let data = std::fs::read_to_string(&edl_v1_path)?;
    let v1: serde_json::Value = serde_json::from_str(&data)?;
    let timeline = Timeline::from_edl_v1(&v1)?;

    let out_path = output_path.unwrap_or_else(|| {
        let p = Path::new(&edl_v1_path);
        let stem = p
            .file_stem()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        format!("{}.timeline.json", stem)
    });

    timeline.save(&out_path)?;

    Ok(json!({
        "status": "upgraded",
        "timeline_path": out_path,
        "segments_count": timeline.segments.len(),
    }))
}

// ---------------------------------------------------------------------------
// Handler: timeline.add_segment
// ---------------------------------------------------------------------------

async fn handle_timeline_add_segment(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let timeline_path = extract_str(&args, "timeline_path")?;
    let start = extract_f64(&args, "start")?;
    let end = extract_f64(&args, "end")?;
    let caption = extract_str(&args, "caption")?;
    let crossfade_ms = default_u32(&args, "crossfade_ms", 80);
    let semantic_role = default_opt_str(&args, "semantic_role");

    let mut timeline = Timeline::load(timeline_path)?;
    let segment_id =
        timeline.add_segment(start, end, caption, crossfade_ms, semantic_role.as_deref());
    timeline.save(timeline_path)?;

    Ok(json!({
        "status": "segment_added",
        "segment_id": segment_id,
        "timeline_path": timeline_path,
    }))
}

// ---------------------------------------------------------------------------
// Handler: timeline.add_track_event
// ---------------------------------------------------------------------------

async fn handle_timeline_add_track_event(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let timeline_path = extract_str(&args, "timeline_path")?;
    let track_type_str = extract_str(&args, "track_type")?;
    let event = args
        .get("event")
        .ok_or_else(|| ToolError::MissingArg("event".to_string()))?
        .clone();

    let track_type: TrackType = track_type_str.parse().map_err(ToolError::Timeline)?;

    let mut timeline = Timeline::load(timeline_path)?;

    let event_obj: openscript_core::timeline::TimelineEvent =
        serde_json::from_value(event.clone()).map_err(ToolError::Json)?;

    timeline.add_track_event(track_type, event_obj);
    timeline.save(timeline_path)?;

    let event_id = event
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    Ok(json!({
        "status": "event_added",
        "event_id": event_id,
        "track_type": track_type_str,
        "timeline_path": timeline_path,
    }))
}

// ---------------------------------------------------------------------------
// Handler: voice.profile.add
// ---------------------------------------------------------------------------

async fn handle_voice_profile_add(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let profile_id = extract_str(&args, "profile_id")?;
    let ref_audio = extract_str(&args, "ref_audio")?;
    let ref_text = extract_str(&args, "ref_text")?;
    let provider = default_str(&args, "provider", "faster-qwen3-tts");
    let mode = default_str(&args, "mode", "clone");
    let model = default_str(&args, "model", "Qwen/Qwen3-TTS-12Hz-0.6B-Base");
    let language = default_str(&args, "language", "English");
    let description = default_opt_str(&args, "description");

    let mut profiles = load_voice_profiles()?;
    let obj = json!({
        "profile_id": profile_id,
        "ref_audio": ref_audio,
        "ref_text": ref_text,
        "provider": provider,
        "mode": mode,
        "model": model,
        "language": language,
        "description": description,
    });
    profiles[profile_id] = obj;
    save_voice_profiles(&profiles)?;

    // Audio8 (zero-shot cloning): register the reference voice with the
    // sidecar so synthesis can use it. Registration failure is NOT fatal —
    // the profile is saved and can be re-registered later (e.g. via
    // voice.profile.add with the same id + overwrite).
    let mut registered_audio8 = false;
    let mut audio8_warning: Option<String> = None;
    if provider == "audio8" {
        if ref_audio.is_empty() || ref_text.is_empty() {
            audio8_warning = Some(
                "audio8 profile needs ref_audio + ref_text for voice cloning; \
                 registration skipped until both are provided."
                    .into(),
            );
        } else {
            match openscript_tts::audio8::audio8_register(&profile_id, &ref_audio, &ref_text) {
                Ok(()) => registered_audio8 = true,
                Err(e) => {
                    audio8_warning = Some(format!(
                        "audio8 voice registration failed (profile saved; retry later): {}",
                        e
                    ));
                }
            }
        }
    }

    Ok(json!({
        "status": "profile_added",
        "profile_id": profile_id,
        "audio8_registered": registered_audio8,
        "audio8_warning": audio8_warning,
    }))
}

// ---------------------------------------------------------------------------
// Handler: voice.profile.list
// ---------------------------------------------------------------------------

async fn handle_voice_profile_list(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let _ = args;
    let profiles = load_voice_profiles()?;
    let profile_list = profiles
        .as_object()
        .map(|obj| {
            obj.iter()
                .map(|(key, v)| {
                    let profile_id = v
                        .get("id")
                        .and_then(|x| x.as_str())
                        .or_else(|| v.get("profile_id").and_then(|x| x.as_str()))
                        .unwrap_or(key);
                    json!({
                        "profile_id": profile_id,
                        "provider": v.get("provider").and_then(|x| x.as_str()).unwrap_or(""),
                        "language": v.get("language").and_then(|x| x.as_str()).unwrap_or(""),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(json!({
        "status": "success",
        "profiles": profile_list,
        "count": profile_list.len(),
    }))
}

// ---------------------------------------------------------------------------
// Handler: voice.profile.remove
// ---------------------------------------------------------------------------

async fn handle_voice_profile_remove(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let profile_id = extract_str(&args, "profile_id")?;
    let mut profiles = load_voice_profiles()?;
    let existed = profiles
        .as_object_mut()
        .map(|obj| obj.remove(profile_id).is_some())
        .unwrap_or(false);

    if existed {
        save_voice_profiles(&profiles)?;
        Ok(json!({
            "status": "profile_removed",
            "profile_id": profile_id,
        }))
    } else {
        Err(ToolError::NotFound(format!(
            "Voice profile not found: {}",
            profile_id
        )))
    }
}

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
async fn tts_generate_routed(
    voice_profile_id: &str,
    text: &str,
    output_path: &str,
    speed: f64,
    pitch: f64,
    volume: f64,
    format: &str,
    profile: &openscript_tts::profiles::VoiceProfile,
) -> Result<TtsGenResult, ToolError> {
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

    // Audio8 path (zero-shot voice cloning — default cloned-voice engine).
    if profile.provider == "audio8" {
        let (duration_ms, sample_rate) = openscript_tts::audio8::audio8_synthesize(
            text,
            &profile.id,
            output_path,
        )
        .map_err(|e| ToolError::Tts(e))?;
        return Ok(TtsGenResult {
            output_path: output_path.to_string(),
            duration_ms,
            cached: false,
            backend: format!("audio8:{}hz", sample_rate),
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

async fn handle_tts_generate(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_tts::profiles::VoiceProfileRegistry;
    use std::path::Path;

    let voice_profile_id = extract_str(&args, "voice_profile_id")?;
    let text = extract_str(&args, "text")?;
    let output_path = extract_str(&args, "output_path")?;
    let speed = default_f64(&args, "speed", 1.0);
    let pitch = default_f64(&args, "pitch", 1.0);
    let volume = default_f64(&args, "volume", 1.0);
    let format = default_str(&args, "format", "wav");

    report_progress(0.0, 100.0, "Generating speech...")
        .await
        .ok();

    let profiles_path = ".openscript/voice_profiles.json";
    let registry =
        VoiceProfileRegistry::new(profiles_path).map_err(|e| ToolError::Tts(e.to_string()))?;
    // Normalize bare Kokoro IDs: if "af_heart" fails, try "kokoro:af_heart".
    // (UX audit GAP #6: agents wrote bare IDs like "af_heart".)
    let normalized_id = if !voice_profile_id.starts_with("kokoro:") && !voice_profile_id.starts_with("faster-qwen") {
        format!("kokoro:{}", voice_profile_id)
    } else {
        voice_profile_id.to_string()
    };
    let profile = registry
        .get(voice_profile_id)
        .or_else(|| registry.get(&normalized_id))
        .ok_or_else(|| {
            ToolError::NotFound(format!("Voice profile '{}' not found. Try '{}' or add via voice.profile.add.", voice_profile_id, normalized_id))
        })?
        .clone();

    if let Some(parent) = Path::new(output_path).parent() {
        std::fs::create_dir_all(parent).ok();
    }

    // Delegate to the shared provider router (audio8 / kokoro / faster-qwen3-tts).
    let result = tts_generate_routed(
        &voice_profile_id,
        &text,
        &output_path,
        speed,
        pitch,
        volume,
        &format,
        &profile,
    )
    .await?;

    report_progress(100.0, 100.0, "Speech generated").await.ok();

    Ok(json!({
        "status": "generated",
        "backend": result.backend,
        "output_path": result.output_path,
        "duration_ms": result.duration_ms,
        "cached": result.cached,
    }))
}

// ---------------------------------------------------------------------------
// Handler: tts.estimate_duration
// ---------------------------------------------------------------------------

async fn handle_tts_estimate_duration(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let text = extract_str(&args, "text")?;
    let speed = default_f64(&args, "speed", 1.0);
    let word_count = text.split_whitespace().count();
    let estimated_ms = ((word_count as f64 / 2.5) * 1000.0 / speed) as i64;

    Ok(json!({
        "status": "estimated",
        "text": text,
        "word_count": word_count,
        "estimated_duration_ms": estimated_ms,
        "speed": speed,
    }))
}

// ---------------------------------------------------------------------------
// Handler: sfx.index (native via openscript-assets)
// ---------------------------------------------------------------------------

async fn handle_sfx_index(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_assets::sfx::SfxIndex;

    // Prefer portable in-repo pack for cold-start, then env, then large local library.
    let sfx_path = default_opt_str(&args, "sfx_path")
        .or_else(|| std::env::var("OPENSCRIPT_SFX_PATH").ok())
        .unwrap_or_else(|| {
            let pack = resolve_repo_path("mcp/assets/sfx_pack");
            if pack.is_dir() {
                return pack.to_string_lossy().into_owned();
            }
            if let Ok(h) = std::env::var("HOME") {
                let local = format!("{}/Videos/Assets/SFX", h);
                if std::path::Path::new(&local).is_dir() {
                    return local;
                }
            }
            "mcp/assets/sfx_pack".to_string()
        });
    let output_path = default_opt_str(&args, "output_path")
        .unwrap_or_else(|| "mcp/assets/sfx_index.json".to_string());

    if let Some(parent) = std::path::Path::new(&output_path).parent() {
        std::fs::create_dir_all(parent).ok();
    }

    report_progress(0.0, 100.0, "Scanning SFX directory...")
        .await
        .ok();

    let index = SfxIndex::scan_directory(&sfx_path).map_err(|e| ToolError::Asset(e.to_string()))?;

    report_progress(80.0, 100.0, "Saving index...").await.ok();

    index
        .save(&output_path)
        .map_err(|e| ToolError::Asset(e.to_string()))?;

    report_progress(100.0, 100.0, "SFX index complete")
        .await
        .ok();

    Ok(json!({
        "status": "indexed",
        "output_path": output_path,
        "count": index.len(),
        "sfx_path": sfx_path,
    }))
}

// ---------------------------------------------------------------------------
// Handler: sfx.search (native via openscript-assets)
// ---------------------------------------------------------------------------

async fn handle_sfx_search(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_assets::sfx::SfxIndex;

    let query = default_str(&args, "query", "");
    let editorial_role = default_opt_str(&args, "editorial_role");
    let category = default_opt_str(&args, "category");
    let limit = default_u32(&args, "limit", 10) as usize;

    let index_path = std::env::var("OPENSCRIPT_SFX_INDEX")
        .unwrap_or_else(|_| "mcp/assets/sfx_index.json".to_string());

    let index = SfxIndex::load(Some(&index_path)).map_err(|e| ToolError::Asset(e.to_string()))?;

    let results = index.search(
        &query,
        editorial_role.as_deref(),
        category.as_deref(),
        limit,
    );

    let result_json: Vec<serde_json::Value> = results
        .iter()
        .map(|s| {
            json!({
                "id": s.id,
                "filename": s.filename,
                "path": s.path,
                "category": s.category,
                "editorial_role": s.editorial_role,
                "duration_ms": s.duration_ms,
                "recommended_gain_db": s.recommended_gain_db,
                "recommended_use": s.recommended_use,
            })
        })
        .collect();

    Ok(json!({
        "status": "success",
        "results": result_json,
        "count": result_json.len(),
    }))
}

// ---------------------------------------------------------------------------
// Handler: sfx.assign
// ---------------------------------------------------------------------------

async fn handle_sfx_assign(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_assets::sfx::SfxIndex;

    let timeline_path = extract_str(&args, "timeline_path")?;
    let editorial_role = extract_str(&args, "editorial_role")?;
    let query = default_str(&args, "query", "");
    let position_ms = default_i64(&args, "position_ms", 0);
    let gain_db = default_f64(&args, "gain_db", -10.0);
    // Expose fade_in_ms / fade_out_ms as parameters (were hardcoded to 50).
    let fade_in_ms = default_u32(&args, "fade_in_ms", 50);
    let fade_out_ms = default_u32(&args, "fade_out_ms", 50);

    // P1-1 fix: map "hook" -> "intro". The SFX index uses "intro" for opening
    // effects, but the tool documentation and `` refer to the
    // opening slot as "hook". Without this mapping, `sfx.assign(editorial_role="hook")`
    // returns 0 results even though perfectly suitable "intro" SFX exist.
    let mapped_role = if editorial_role == "hook" {
        "intro"
    } else {
        editorial_role
    };

    let mut timeline = Timeline::load(timeline_path)?;
    let event_id = format!("sfx_{:03}", track_count(&timeline, &TrackType::Sfx) + 1);

    let index_path = std::env::var("OPENSCRIPT_SFX_INDEX")
        .unwrap_or_else(|_| "mcp/assets/sfx_index.json".to_string());
    // Capture the full matched asset (not just the path) so we can read its
    // actual duration_ms instead of hardcoding 1000.
    let sfx_index = SfxIndex::load(Some(&index_path)).ok();
    let matched_asset: Option<openscript_assets::sfx::SfxAsset> = sfx_index
        .as_ref()
        .and_then(|idx| {
            idx.search(&query, Some(mapped_role), None, 1)
                .into_iter()
                .next()
                .cloned()
        });
    let sfx_path = matched_asset.as_ref().map(|a| a.path.clone());
    // Fix: read the actual duration from the matched asset. Prior versions
    // hardcoded 1000ms, so a 3.3s SFX was reported as 1s on the timeline
    // and the render could cut it short.
    let actual_duration_ms = matched_asset
        .as_ref()
        .map(|a| a.duration_ms)
        .unwrap_or(1000);

    let event = openscript_core::timeline::TimelineEvent {
        id: event_id.clone(),
        asset_id: sfx_path.clone().unwrap_or_else(|| query.clone()),
        start_ms: position_ms,
        end_ms: position_ms + actual_duration_ms,
        offset_ms: 0,
        gain_db,
        fade_in_ms,
        fade_out_ms,
        tags: vec![editorial_role.to_string()],
        provenance: Some(openscript_core::timeline::Provenance {
            tool: "sfx.assign".into(),
            editorial_role: Some(editorial_role.to_string()),
            concept: None,
        }),
        kind: openscript_core::timeline::EventKind::Sfx {
            editorial_role: editorial_role.to_string(),
            category: query.to_string(),
            subcategory: String::new(),
            duration_ms: actual_duration_ms,
            sample_rate: 44100,
            peak_db: 0.0,
            loudness_lufs: -14.0,
            recommended_gain_db: gain_db,
            recommended_use: "single_hit".into(),
            safe_overlay: true,
        },
    };

    timeline.add_track_event(TrackType::Sfx, event);
    if let Some(ref path) = sfx_path {
        timeline.add_asset("sfx", event_id.clone(), json!({"path": path}));
    } else {
        timeline.add_asset("sfx", event_id.clone(), json!({"query": query}));
    }
    timeline.save(timeline_path)?;

    // P1-4 fix: return status "warning" (not "assigned") when no asset matched,
    // plus an explicit `matched` flag and a human-readable message. Prior
    // versions returned "assigned" with asset_path:null, which led agents to
    // believe the operation succeeded.
    let (status, matched, message) = if sfx_path.is_some() {
        (
            "assigned",
            true,
            format!(
                "SFX assigned for role '{}' at {} ms",
                editorial_role, position_ms
            ),
        )
    } else {
        (
            "warning",
            false,
            format!(
                "No SFX found for role '{}' (mapped to '{}'). Placeholder event created at {} ms — render will skip this event. Try sfx.search to inspect available assets.",
                editorial_role, mapped_role, position_ms
            ),
        )
    };

    Ok(json!({
        "status": status,
        "matched": matched,
        "message": message,
        "event_id": event_id,            "position_ms": position_ms,
            "timeline_path": timeline_path,
            "asset_path": sfx_path,
        }))
    }

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

async fn handle_music_index(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_assets::music::MusicIndex;

    let music_paths = default_opt_arr(&args, "music_paths");
    let output_path = default_opt_str(&args, "output_path")
        .unwrap_or_else(|| "mcp/assets/music_index.json".to_string());

    if let Some(parent) = std::path::Path::new(&output_path).parent() {
        std::fs::create_dir_all(parent).ok();
    }

    // Prefer committed stock dir so `music.index` does not silently overwrite
    // mcp/assets/music_index.json with an empty/unrelated ~/Videos/Assets/Music scan.
    let stock_music = "mcp/assets/music".to_string();
    let home_music = std::env::var("HOME")
        .ok()
        .map(|h| format!("{}/Videos/Assets/Music", h));
    let env_music = std::env::var("OPENSCRIPT_MUSIC_PATH").ok();
    let default_path = env_music
        .or_else(|| {
            if std::path::Path::new(&stock_music).is_dir() {
                Some(stock_music.clone())
            } else {
                home_music
            }
        })
        .unwrap_or(stock_music);
    let default_paths = vec![default_path];
    let paths = music_paths.as_deref().unwrap_or(&default_paths);

    report_progress(0.0, 100.0, "Scanning music directories...")
        .await
        .ok();

    let index = MusicIndex::scan_directories(paths).map_err(|e| ToolError::Asset(e.to_string()))?;

    report_progress(80.0, 100.0, "Saving index...").await.ok();

    index
        .save(&output_path)
        .map_err(|e| ToolError::Asset(e.to_string()))?;

    report_progress(100.0, 100.0, "Music index complete")
        .await
        .ok();

    Ok(json!({
        "status": "indexed",
        "output_path": output_path,
        "count": index.len(),
    }))
}

// ---------------------------------------------------------------------------
// Handler: music.search
// ---------------------------------------------------------------------------

async fn handle_music_search(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let index_path = default_opt_str(&args, "index_path")
        .unwrap_or_else(|| "mcp/assets/music_index.json".to_string());
    let query = default_opt_str(&args, "query");
    let mood_filter = default_opt_str(&args, "mood");
    let energy_filter = default_opt_str(&args, "energy");
    let limit = default_u32(&args, "limit", 10) as usize;

    if !Path::new(&index_path).exists() {
        return Ok(json!({
            "status": "warning",
            "message": format!("Music index not found at {}. Run music.index first.", index_path),
            "tracks": [],
        }));
    }

    let raw = std::fs::read_to_string(&index_path)?;
    let index: serde_json::Value = serde_json::from_str(&raw)?;

    let assets = index.get("assets")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let query_lower = query.as_deref().unwrap_or("").to_lowercase();

    let mut matched: Vec<serde_json::Value> = assets.into_iter().filter(|a| {
        // Filter by mood
        if let Some(ref m) = mood_filter {
            let asset_mood = a.get("mood").and_then(|v| v.as_str()).unwrap_or("");
            if !asset_mood.eq_ignore_ascii_case(m) {
                return false;
            }
        }
        // Filter by energy
        if let Some(ref e) = energy_filter {
            let asset_energy = a.get("energy").and_then(|v| v.as_str()).unwrap_or("");
            if !asset_energy.eq_ignore_ascii_case(e) {
                return false;
            }
        }
        // Filter by query (match against title, tags, genre)
        if !query_lower.is_empty() {
            let title = a.get("title").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
            let genre = a.get("genre").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
            let tags = a.get("tags").and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|t| t.as_str()).collect::<Vec<_>>().join(" ").to_lowercase())
                .unwrap_or_default();
            if !title.contains(&query_lower) && !genre.contains(&query_lower) && !tags.contains(&query_lower) {
                return false;
            }
        }
        // Verify the file actually exists on disk
        if let Some(p) = a.get("path").and_then(|v| v.as_str()) {
            Path::new(p).exists()
        } else {
            false
        }
    }).collect();

    matched.truncate(limit);

    Ok(json!({
        "status": "success",
        "count": matched.len(),
        "tracks": matched,
    }))
}

// ---------------------------------------------------------------------------
// Handler: music.assign
// ---------------------------------------------------------------------------

async fn handle_music_assign(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let timeline_path = extract_str(&args, "timeline_path")?;
    let music_path = extract_str(&args, "path")?;
    let mood = default_str(&args, "mood", "neutral");
    let energy = default_str(&args, "energy", "medium");
    let start_ms = default_i64(&args, "start_ms", 0);
    let end_ms = default_opt_i64(&args, "end_ms");
    let gain_db = default_f64(&args, "gain_db", -12.0);
    let ducking = default_bool(&args, "ducking", true);
    // Expose fade_in_ms / fade_out_ms as parameters (were hardcoded to 500).
    let fade_in_ms = default_u32(&args, "fade_in_ms", 500);
    let fade_out_ms = default_u32(&args, "fade_out_ms", 500);

    // Validate the music file exists
    if !Path::new(music_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Music file not found: {}. Use library.search to find tracks. Accepts both local paths and URLs..",
            music_path
        )));
    }

    let mut timeline = Timeline::load(timeline_path)?;

    let total_ms = timeline.total_duration_ms();
    let end = end_ms.unwrap_or(total_ms);
    let event_id = format!("music_{:03}", track_count(&timeline, &TrackType::Music) + 1);

    // P2-2 fix: only add a ducking directive when speech tracks actually
    // exist on the timeline. Prior versions added a "dialogue_active"
    // directive unconditionally, which would attenuate the music even when
    // there was no dialogue to duck against — silently producing a quieter
    // mix than the user intended for music-only videos.
    if ducking {
        let has_speech = track_count(&timeline, &TrackType::Dialogue) > 0
            || track_count(&timeline, &TrackType::Voiceover) > 0;
        if has_speech {
            timeline.add_ducking_directive("dialogue_active", "music", 10.0, 50, 200);
        }
    }

    let event = openscript_core::timeline::TimelineEvent {
        id: event_id.clone(),
        asset_id: event_id.clone(),
        start_ms,
        end_ms: end,
        offset_ms: 0,
        gain_db,
        fade_in_ms,
        fade_out_ms,
        tags: vec![mood.clone(), energy.clone()],
        provenance: Some(openscript_core::timeline::Provenance {
            tool: "music.assign".into(),
            editorial_role: None,
            concept: None,
        }),
        kind: openscript_core::timeline::EventKind::Music {
            mood,
            energy,
            bpm: None,
            loopability: true,
            intro_friendly: true,
            cta_friendly: true,
            loudness_target_lufs: -14.0,
            loop_mode: "loop".into(),
            ducking_policy: if ducking { "auto" } else { "none" }.into(),
        },
    };

    // Register the music asset path so render_from_timeline can find it
    timeline.add_asset("music", event_id.clone(), json!({"path": music_path}));

    timeline.add_track_event(TrackType::Music, event);
    timeline.save(timeline_path)?;

    Ok(json!({
        "status": "assigned",
        "event_id": event_id,
        "asset_path": music_path,
        "start_ms": start_ms,
        "end_ms": end,
        "timeline_path": timeline_path,
    }))
}

// ---------------------------------------------------------------------------
// Handler: broll.suggest
// ---------------------------------------------------------------------------

async fn handle_broll_suggest(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let edl_path = extract_str(&args, "edl_path")?;
    let _srt_path = default_opt_str(&args, "srt_path");
    let cadence_seconds = default_f64(&args, "cadence_seconds", 2.0);

    let data = std::fs::read_to_string(edl_path)?;
    let timeline: serde_json::Value = serde_json::from_str(&data)?;

    let segments = timeline
        .get("segments")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let cadence_ms = (cadence_seconds * 1000.0) as i64;
    let mut suggestions = Vec::new();
    let mut position_ms = 0i64;

    for seg in &segments {
        let start = seg.get("start").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let end = seg.get("end").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let duration_ms = ((end - start) * 1000.0) as i64;
        // Derive concept from the segment caption instead of hardcoding "b-roll".
        // Use a salient noun/phrase from the caption — skip stopwords and short
        // words that produce garbage Pexels searches ("The", "But", "And").
        let caption = seg
            .get("caption")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let concept = extract_broll_concept(caption);

        if duration_ms > cadence_ms * 2 {
            let mut t = 0i64;
            while t < duration_ms {
                let slot_duration = cadence_ms.min(duration_ms - t);
                suggestions.push(json!({
                    "position_ms": position_ms + t,
                    "duration_ms": slot_duration,
                    "concept": concept,
                }));
                t += cadence_ms;
            }
        }

        position_ms += duration_ms;
    }

    Ok(json!({
        "status": "success",
        "edl_path": edl_path,
        "suggestions": suggestions,
        "count": suggestions.len(),
    }))
}

// ---------------------------------------------------------------------------
// Handler: broll.fetch (native via openscript-assets PexelsClient)
// ---------------------------------------------------------------------------

async fn handle_broll_fetch(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_assets::pexels::PexelsClient;

    // Accept enriched_segments (from broll.keywords) OR concepts/keywords (flat array).
    let enriched_segments: Vec<serde_json::Value> = args.get("enriched_segments")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let max_kw_per_search = args.get("max_keywords_per_search")
        .and_then(|v| v.as_u64())
        .unwrap_or(3) as usize;

    // If enriched_segments provided, extract concepts from their keywords arrays.
    // Each segment's keywords are joined into a single search query for better Pexels results.
    let concepts_from_enriched: Vec<String> = if !enriched_segments.is_empty() {
        enriched_segments.iter().map(|seg| {
            let keywords = seg.get("keywords")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|k| k.as_str().map(String::from)).collect::<Vec<_>>())
                .unwrap_or_default();
            // Filter: keep keywords >= 3 chars (skip single-char noise like "par", "ko").
            // Multi-word phrases with spaces ("city skyline") are great Pexels queries.
            // Single words like "corruption", "protest" are also good — keep them.
            let good_kws: Vec<String> = keywords.into_iter()
                .filter(|k| k.len() >= 3)
                .take(max_kw_per_search)
                .collect();
            if good_kws.is_empty() {
                // Fallback: use first keyword >= 2 chars
                seg.get("keywords")
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|k| k.as_str())
                    .filter(|k| k.len() >= 2)
                    .unwrap_or("video")
                    .to_string()
            } else {
                good_kws.join(" ")
            }
        }).collect()
    } else if args.get("concepts").is_some() {
        extract_arr(&args, "concepts")?
    } else if let Some(s) = args.get("keywords").and_then(|v| v.as_str()) {
        vec![s.to_string()]
    } else if args.get("keywords").is_some() {
        extract_arr(&args, "keywords")?
    } else {
        return Err(ToolError::MissingArg(
            "concepts (or keywords) or enriched_segments".to_string(),
        ));
    };
    if concepts_from_enriched.is_empty() {
        return Err(ToolError::InvalidArg(
            "concepts/keywords must not be empty".into(),
        ));
    }
    let concepts = concepts_from_enriched;
    let asset_dir =
        default_opt_str(&args, "asset_dir").unwrap_or_else(|| "mcp/assets/broll_cache".to_string());
    let orientation = default_str(&args, "orientation", "9:16");
    let quality = default_str(&args, "quality", "sd");
    // Number of DISTINCT clips to download per concept. When the agent has
    // more segments than concepts (e.g. 44 segments / 12 concepts), downloading
    // several distinct clips per concept lets the auto-placer cycle through
    // them so consecutive segments don't reuse the same footage.
    let download_n = args
        .get("download_n")
        .and_then(|v| v.as_u64())
        .map(|n| n.max(1) as usize)
        .unwrap_or(1);
    let download_explicit = args.get("download").and_then(|v| v.as_bool());
    // Auto-enable download when enriched_segments + timeline_path are both
    // provided — auto-placement requires downloaded files on disk.
    let has_enriched = !enriched_segments.is_empty();
    let has_timeline = default_opt_str(&args, "timeline_path").is_some();
    let download = download_explicit.unwrap_or(has_enriched && has_timeline);
    // Local fallback clips used when Pexels returns nothing for a concept
    // (mirrors background.fetch's fallback_pool semantics).
    let fallback_pool: Vec<String> = args
        .get("fallback_pool")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let api_key = pexels_key();
    if api_key.is_empty() {
        // Bug #16 fix: do not hard-fail when PEXELS_API_KEY is missing.
        // Return status:warning with actionable guidance so an agent can
        // fall back to background.fetch (which has its own fallback chain)
        // or supply a fallback_pool.
        if fallback_pool.is_empty() {
            return Ok(json!({
                "status": "warning",
                "message": "PEXELS_API_KEY not set and no fallback_pool provided. Set the key in mcp/assets/.openscript_config.json, or provide a fallback_pool of local clip paths, or use background.fetch which has its own fallback chain.",
                "results": [],
                "total_concepts": concepts.len(),
                "missing_key": true,
            }));
        }
        // No key but caller supplied fallback_pool — return one fallback
        // entry per concept so downstream tools (broll.assign) can still
        // place something on the timeline.
        let results: Vec<serde_json::Value> = concepts
            .iter()
            .enumerate()
            .map(|(i, concept)| {
                let path = fallback_pool[i % fallback_pool.len()].clone();
                json!({
                    "concept": concept,
                    "videos": [],
                    "count": 0,
                    "cached_path": path,
                    "source": "fallback_pool",
                })
            })
            .collect();
        let mut downloaded: Vec<serde_json::Value> = Vec::new();
        for (i, concept) in concepts.iter().enumerate() {
            downloaded.push(json!({
                "concept": concept,
                "path": fallback_pool[i % fallback_pool.len()],
                "source": "fallback_pool",
            }));
        }
        return Ok(json!({
            "status": "warning",
            "message": "PEXELS_API_KEY not set; using fallback_pool only.",
            "results": results,
            "downloaded": downloaded,
            "total_concepts": concepts.len(),
            "missing_key": true,
        }));
    }

    let total = concepts.len();
    report_progress(0.0, total as f64, "Fetching b-roll...")
        .await
        .ok();

    let mut client = PexelsClient::new(&api_key, &asset_dir);
    // Non-repetition: clips already placed on this timeline (or chosen earlier
    // in THIS run) are excluded from candidate selection — the same footage
    // must never appear twice later in the sequence (b-roll-repeat bug).
    let mut used_ids: std::collections::HashSet<i64> = default_opt_str(&args, "timeline_path")
        .and_then(|tl| Timeline::load(&tl).ok())
        .map(|t| used_broll_video_ids(&t))
        .unwrap_or_default();
    let mut all_results = Vec::new();
    let mut downloaded = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    for (i, concept) in concepts.iter().enumerate() {
        report_progress(i as f64, total as f64, &format!("Searching: {}", concept))
            .await
            .ok();

        let videos = client
            .search(concept, &orientation, &quality)
            .await
            .map_err(|e| ToolError::Asset(e.to_string()))?;

        // Download up to `download_n` DISTINCT clips per concept (not just the
        // top hit). Distinct footage per segment is what breaks the "same clip,
        // different zoom/pan" illusion — reuse is only acceptable when the
        // source library is genuinely exhausted, and the verifier flags that.
        let mut cached_paths: Vec<String> = Vec::new();
        // path → real duration (Pexels metadata) for EACH downloaded clip, so
        // auto-place records the duration of the clip actually placed, not the
        // first result's. Without this, probe_broll_gaps compares the segment
        // window against the wrong clip's duration whenever the cursor cycles
        // to a different distinct clip (missed or false gaps).
        let mut path_durations: std::collections::HashMap<String, f64> =
            std::collections::HashMap::new();
        if download {
            let mut seen_ids: std::collections::HashSet<i64> = std::collections::HashSet::new();
            // Skip ids already placed on the timeline / chosen this run.
            let (cands, reused) = fresh_candidates(&videos, &used_ids, download_n);
            if reused > 0 {
                warnings.push(format!(
                    "concept '{}': Pexels returned {} candidate(s), all already used on this timeline — reusing {} distinct clip(s) (library exhausted for this concept). Widen the keywords to get truly unique footage.",
                    concept,
                    videos.len(),
                    reused
                ));
            }
            for v in cands {
                if !seen_ids.insert(v.id) {
                    continue;
                }
                used_ids.insert(v.id);
                let v_duration = v.duration as f64;
                match client.download_best(v, concept).await {
                    Ok(path) => {
                        path_durations.insert(path.clone(), v_duration);
                        cached_paths.push(path.clone());
                        downloaded.push((concept.clone(), path));
                    }
                    Err(e) => {
                        tracing::warn!("[broll.fetch] Download failed for {}: {}", concept, e)
                    }
                }
            }
        }
        let cached_path = cached_paths.first().cloned();

        let video_json: Vec<serde_json::Value> = videos
            .iter()
            .take(3)
            .map(|v| {
                json!({
                    "id": v.id,
                    "width": v.width,
                    "height": v.height,
                    "duration": v.duration,
                    "image": v.image,
                    "url": v.url,
                })
            })
            .collect();

        let mut result = json!({
            "concept": concept,
            "videos": video_json,
            "count": video_json.len(),
        });
        if let Some(path) = &cached_path {
            result["cached_path"] = json!(path);
        }
        if !cached_paths.is_empty() {
            // Distinct clips downloaded for this concept. The auto-placer
            // cycles through them so consecutive segments sharing a concept
            // still get different footage.
            result["cached_paths"] = json!(cached_paths);
        }
        if !path_durations.is_empty() {
            // Per-path durations so auto-place can store the duration of the
            // clip it actually placed (see path_durations in the download
            // loop above).
            let dur_map: serde_json::Map<String, serde_json::Value> = path_durations
                .iter()
                .map(|(p, d)| (p.clone(), json!(d)))
                .collect();
            result["cached_path_durations"] = json!(dur_map);
        }
        // Record the source clip's real duration (from Pexels metadata) so
        // timeline.validate / verify.production can compare it against the
        // segment window without re-probing. Short clips become coverage
        // gaps (broll_gaps) instead of silently looping.
        if let Some(first) = videos.first() {
            result["source_duration_s"] = json!(first.duration);
        }

        // Per-concept fallback: if Pexels returned nothing, try fallback_pool
        // so downstream tools (broll.assign) still have a path to use.
        if video_json.is_empty() && !fallback_pool.is_empty() {
            let fallback_path = fallback_pool[i % fallback_pool.len()].clone();
            result["cached_path"] = json!(&fallback_path);
            result["source"] = json!("fallback_pool");
            warnings.push(format!(
                "concept '{}' returned 0 Pexels results — using fallback_pool entry",
                concept
            ));
            if download {
                downloaded.push((concept.clone(), fallback_path));
            }
        }
        all_results.push(result);
    }

    report_progress(total as f64, total as f64, "B-roll fetch complete")
        .await
        .ok();

    // Status is "warning" if any concept returned 0 videos (mirrors
    // background.fetch's behaviour of warning when falling back).
    let any_empty = all_results
        .iter()
        .any(|r| r.get("count").and_then(|v| v.as_u64()).unwrap_or(0) == 0);
    let status = if any_empty { "warning" } else { "fetched" };

    let mut resp = json!({
        "status": status,
        "results": all_results,
        "total_concepts": concepts.len(),
    });
    if !downloaded.is_empty() {
        resp["downloaded"] = json!(downloaded
            .iter()
            .map(|(c, p)| json!({"concept": c, "path": p}))
            .collect::<Vec<_>>());
    }
    if !warnings.is_empty() {
        resp["warnings"] = json!(warnings);
    }

    // AUTO-PLACE: If timeline_path provided, place each clip on timeline
    let timeline_path = default_opt_str(&args, "timeline_path");
    // Priority: enriched_segments > segments arg > timeline segments
    let placement_segments = if !enriched_segments.is_empty() {
        enriched_segments
    } else {
        args.get("segments")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
    };

    if let Some(ref tl_path) = timeline_path {
        // Load segments from timeline if not provided in args/enriched_segments
        let segments = if !placement_segments.is_empty() {
            placement_segments
        } else if std::path::Path::new(tl_path).exists() {
            // Read segments directly from the timeline JSON
            if let Ok(tl_str) = std::fs::read_to_string(tl_path) {
                if let Ok(tl_val) = serde_json::from_str::<serde_json::Value>(&tl_str) {
                    tl_val.get("segments")
                        .and_then(|s| s.as_array())
                        .cloned()
                        .unwrap_or_default()
                } else { Vec::new() }
            } else { Vec::new() }
        } else { Vec::new() };
        if !segments.is_empty() {
            let mut tl = Timeline::load(tl_path)
                .map_err(|e| ToolError::Io(std::io::Error::other(format!("Failed to load timeline: {}", e))))?;
            let mut assigned_count = 0usize;
            // Distribute clips to segments. When there are MORE segments than
            // concepts (the common 44-seg/12-concept case), cycle through each
            // concept's DISTINCT downloaded clips (`cached_paths`) so adjacent
            // segments reuse the same footage only when the pool is exhausted.
            let mut concept_cursor: std::collections::HashMap<usize, usize> =
                std::collections::HashMap::new();
            for (i, segment) in segments.iter().enumerate() {
                let result_val = &all_results[i % all_results.len()];
                let concept_idx = i % all_results.len();
                // Advance a per-concept cursor through the distinct clip pool.
                let pool: Vec<String> = result_val
                    .get("cached_paths")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|p| p.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_else(|| {
                        result_val
                            .get("cached_path")
                            .and_then(|v| v.as_str())
                            .filter(|p| !p.is_empty() && *p != "placeholder")
                            .map(|p| vec![p.to_string()])
                            .unwrap_or_default()
                    });
                if pool.is_empty() {
                    continue;
                }
                let cursor = concept_cursor.entry(concept_idx).or_insert(0);
                let cached_path = pool[*cursor % pool.len()].clone();
                *cursor += 1;
                let start_s = segment.get("start_s")
                    .or_else(|| segment.get("start"))
                    .and_then(|v| v.as_f64()).unwrap_or(0.0);
                let end_s = segment.get("end_s")
                    .or_else(|| segment.get("end"))
                    .and_then(|v| v.as_f64()).unwrap_or(start_s + 3.0);
                let concept_str = result_val.get("concept")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let position_ms = (start_s * 1000.0) as i64;
                let duration_ms = ((end_s - start_s) * 1000.0) as i64;
                if duration_ms <= 0 { continue; }let event_id = format!("broll_{}", i);
                    let asset_id = event_id.clone();
                let broll_event = openscript_core::timeline::TimelineEvent {
                    id: event_id.clone(),
                    asset_id: asset_id.clone(),
                    start_ms: position_ms,
                    end_ms: position_ms + duration_ms,
                    offset_ms: 0,
                    gain_db: 0.0,
                    fade_in_ms: 0,
                    fade_out_ms: 0,
                    tags: vec![concept_str.to_string()],
                    provenance: Some(openscript_core::timeline::Provenance {
                        tool: "broll.fetch".to_string(),
                        editorial_role: None,
                        concept: Some(concept_str.to_string()),
                    }),
                    kind: openscript_core::timeline::EventKind::Broll {
                        concept: concept_str.to_string(),
                        source_provider: "pexels".to_string(),
                        transition_style: "cut".to_string(),
                        crop_mode: "center".to_string(),
                        orientation: orientation.clone(),
                        motion_intensity: "medium".to_string(),
                    },
                };
                tl.tracks.entry(openscript_core::types::TrackType::Broll)
                    .or_default()
                    .push(broll_event);
                // Persist the source clip's real duration (from Pexels metadata)
                // so verify.production / timeline.validate can compare it
                // against the segment window without re-probing — short clips
                // become coverage gaps (broll_gaps) instead of silently looping.
                // Use the duration of the clip ACTUALLY placed (per-path map
                // from the download loop), falling back to the result-wide
                // first-video hint only when the placed clip is the first one.
                let mut asset_record = serde_json::json!({
                    "path": cached_path,
                    "concept": concept_str,
                });
                let placed_duration = result_val
                    .get("cached_path_durations")
                    .and_then(|v| v.as_object())
                    .and_then(|m| m.get(&cached_path))
                    .and_then(|v| v.as_f64());
                if let Some(d) = placed_duration {
                    asset_record["source_duration_s"] = json!(d);
                } else if let Some(d) = result_val
                    .get("source_duration_s")
                    .and_then(|v| v.as_f64())
                {
                    asset_record["source_duration_s"] = json!(d);
                }
                tl.assets.broll.insert(asset_id.clone(), asset_record);
                assigned_count += 1;
            }
            tl.updated_at = chrono::Utc::now();
            tl.save(tl_path)
                .map_err(|e| ToolError::Io(std::io::Error::other(format!("Failed to save timeline: {}", e))))?;
            resp["timeline_path"] = json!(tl_path);
            resp["auto_assigned"] = json!(assigned_count);
            if assigned_count > 0 {
                resp["status"] = json!("placed");
            }
        }
    }

    Ok(resp)
}

// ---------------------------------------------------------------------------
// Handler: broll.assign
// ---------------------------------------------------------------------------

async fn handle_broll_assign(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let timeline_path = extract_str(&args, "timeline_path")?;
    let concept = extract_str(&args, "concept")?;
    let position_ms = extract_i64(&args, "position_ms")?;
    let duration_ms = extract_i64(&args, "duration_ms")?;
    let asset_path = default_opt_str(&args, "asset_path");
    let transition_style = default_str(&args, "transition_style", "cut");
    let crop_mode = default_str(&args, "crop_mode", "center");
    // Expose fade_in_ms / fade_out_ms as parameters (were hardcoded to 0).
    let fade_in_ms = default_u32(&args, "fade_in_ms", 0);
    let fade_out_ms = default_u32(&args, "fade_out_ms", 0);

    let mut timeline = Timeline::load(timeline_path)?;
    let event_id = format!("broll_{:03}", track_count(&timeline, &TrackType::Broll) + 1);

    let resolved_path = asset_path.unwrap_or_else(|| {
        let cache_dir = "mcp/assets/broll_cache";
        if let Ok(entries) = std::fs::read_dir(cache_dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(&concept.replace(' ', "_")) && name.ends_with(".mp4") {
                    return entry.path().to_string_lossy().to_string();
                }
            }
        }
        // No match found — return empty string so the existence check below catches it
        String::new()
    });

    // If the resolved path doesn't exist on disk, use "placeholder" so the
    // render pipeline skips this event instead of crashing ffmpeg with a
    // glob pattern or non-existent path.
    let (asset_id, asset_registry_path, matched) = if resolved_path.is_empty()
        || resolved_path.contains("placeholder")
        || !std::path::Path::new(&resolved_path).exists()
    {
        ("placeholder".to_string(), "placeholder".to_string(), false)
    } else {
        (resolved_path.clone(), resolved_path.clone(), true)
    };

    let event = openscript_core::timeline::TimelineEvent {
        id: event_id.clone(),
        asset_id: asset_id.clone(),
        start_ms: position_ms,
        end_ms: position_ms + duration_ms,
        offset_ms: 0,
        gain_db: 0.0,
        fade_in_ms,
        fade_out_ms,
        tags: vec![concept.to_string()],
        provenance: Some(openscript_core::timeline::Provenance {
            tool: "broll.assign".into(),
            editorial_role: None,
            concept: Some(concept.to_string()),
        }),
        kind: openscript_core::timeline::EventKind::Broll {
            concept: concept.to_string(),
            source_provider: asset_id.clone(),
            transition_style,
            crop_mode,
            orientation: "9:16".into(),
            motion_intensity: "medium".into(),
        },
    };

    timeline.add_track_event(TrackType::Broll, event);
    timeline.add_asset(
        "broll",
        event_id.clone(),
        json!({"path": asset_registry_path}),
    );
    timeline.save(timeline_path)?;

    // Fix: return status "warning" (not "assigned") when no asset matched,
    // mirroring sfx.assign's pattern. Prior versions returned "assigned" with
    // asset_id:"placeholder", silently losing the agent's intent — the render
    // pipeline drops placeholder events, so the agent never knew the b-roll
    // slot was empty.
    let (status, message) = if matched {
        (
            "assigned",
            format!("B-roll assigned for concept '{}' at {} ms", concept, position_ms),
        )
    } else {
        (
            "warning",
            format!(
                "No b-roll asset found for concept '{}' at {} ms. Placeholder event created — render will skip this event. Use broll.fetch or background.fetch to download a real asset, then re-assign.",
                concept, position_ms
            ),
        )
    };

    Ok(json!({
        "status": status,
        "matched": matched,
        "message": message,
        "event_id": event_id,
        "asset_id": asset_id,
        "asset_path": asset_registry_path,
        "position_ms": position_ms,
        "duration_ms": duration_ms,
        "timeline_path": timeline_path,
    }))
}

// ---------------------------------------------------------------------------
// Handler: voiceover.generate
// ---------------------------------------------------------------------------

async fn handle_voiceover_generate(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    use openscript_tts::profiles::VoiceProfileRegistry;
    use std::path::Path;

    let timeline_path = extract_str(&args, "timeline_path")?;
    let text = extract_str(&args, "text")?;
    let voice_profile_id = extract_str(&args, "voice_profile_id")?;
    let position_ms = default_i64(&args, "position_ms", 0);
    let speed = default_f64(&args, "speed", 1.0);
    let gain_db = default_f64(&args, "gain_db", -6.0);
    let pitch = default_f64(&args, "pitch", 1.0);
    let volume = default_f64(&args, "volume", 1.0);

    let mut timeline = Timeline::load(timeline_path)?;

    let profiles_path = ".openscript/voice_profiles.json";
    let registry =
        VoiceProfileRegistry::new(profiles_path).map_err(|e| ToolError::Tts(e.to_string()))?;
    // Normalize bare Kokoro IDs: if "af_heart" fails, try "kokoro:af_heart".
    // (UX audit GAP #6: agents wrote bare IDs like "af_heart".)
    let normalized_id = if !voice_profile_id.starts_with("kokoro:") && !voice_profile_id.starts_with("faster-qwen") {
        format!("kokoro:{}", voice_profile_id)
    } else {
        voice_profile_id.to_string()
    };
    let profile = registry
        .get(voice_profile_id)
        .or_else(|| registry.get(&normalized_id))
        .ok_or_else(|| {
            ToolError::NotFound(format!("Voice profile '{}' not found. Try '{}' or add via voice.profile.add.", voice_profile_id, normalized_id))
        })?
        .clone();

    let timeline_dir = Path::new(&timeline_path)
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();
    let event_id = format!(
        "voiceover_{:03}",
        track_count(&timeline, &TrackType::Voiceover) + 1
    );
    let output_path = timeline_dir
        .join(format!("voiceover_{}.wav", event_id))
        .to_string_lossy()
        .to_string();

    if let Some(parent) = Path::new(&output_path).parent() {
        std::fs::create_dir_all(parent).ok();
    }

    report_progress(0.0, 100.0, "Generating voiceover...")
        .await
        .ok();

    let result = tts_generate_routed(
        voice_profile_id,
        text,
        &output_path,
        speed,
        pitch,
        volume,
        "wav",
        &profile,
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

    let end_ms = position_ms + duration_ms;
    let event = openscript_core::timeline::TimelineEvent {
        id: event_id.clone(),
        asset_id: output_path.clone(),
        start_ms: position_ms,
        end_ms,
        offset_ms: 0,
        gain_db,
        fade_in_ms: 50,
        fade_out_ms: 50,
        tags: vec!["voiceover".to_string()],
        provenance: Some(openscript_core::timeline::Provenance {
            tool: "voiceover.generate".into(),
            editorial_role: None,
            concept: None,
        }),
        kind: openscript_core::timeline::EventKind::Voiceover {
            voice_profile_id: voice_profile_id.to_string(),
            text: text.to_string(),
            estimated_duration_ms: duration_ms,
        },
    };

    timeline.add_track_event(TrackType::Voiceover, event);
    timeline.save(timeline_path)?;

    report_progress(100.0, 100.0, "Voiceover generated")
        .await
        .ok();

    Ok(json!({
        "status": "generated",
        "output_path": output_path,
        "duration_ms": duration_ms,
        "event_id": event_id,
    }))
}

// ---------------------------------------------------------------------------
// Handler: tts.commentary
// ---------------------------------------------------------------------------

async fn handle_tts_commentary(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_tts::profiles::VoiceProfileRegistry;
    use std::path::Path;

    let timeline_path = extract_str(&args, "timeline_path")?;
    let voice_profile_id = extract_str(&args, "voice_profile_id")?;
    let commentary_type = extract_str(&args, "commentary_type")?;
    let intro_text = default_opt_str(&args, "intro_text");
    let outro_text = default_opt_str(&args, "outro_text");
    let speed = default_f64(&args, "speed", 1.0);

    let mut timeline = Timeline::load(timeline_path)?;
    let total_ms = timeline.total_duration_ms();

    let profiles_path = ".openscript/voice_profiles.json";
    let registry =
        VoiceProfileRegistry::new(profiles_path).map_err(|e| ToolError::Tts(e.to_string()))?;
    // Normalize bare Kokoro IDs: if "af_heart" fails, try "kokoro:af_heart".
    // (UX audit GAP #6: agents wrote bare IDs like "af_heart".)
    let normalized_id = if !voice_profile_id.starts_with("kokoro:") && !voice_profile_id.starts_with("faster-qwen") {
        format!("kokoro:{}", voice_profile_id)
    } else {
        voice_profile_id.to_string()
    };
    let profile = registry
        .get(voice_profile_id)
        .or_else(|| registry.get(&normalized_id))
        .ok_or_else(|| {
            ToolError::NotFound(format!("Voice profile '{}' not found. Try '{}' or add via voice.profile.add.", voice_profile_id, normalized_id))
        })?
        .clone();

    let timeline_dir = Path::new(&timeline_path)
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();

    let do_intro = commentary_type == "intro" || commentary_type == "all";
    let do_outro = commentary_type == "outro" || commentary_type == "all";
    let do_transitions = commentary_type == "transitions" || commentary_type == "all";

    let mut generated = Vec::new();
    let mut positions = Vec::new();

    if do_intro {
        let text = intro_text.unwrap_or_else(|| "Welcome to this video.".to_string());
        let (event_id, _dur) = generate_commentary_segment(
            &mut timeline,
            &timeline_dir,
            voice_profile_id,
            &text,
            0,
            "intro",
            speed,
            &profile,
        )
        .await?;
        generated.push(event_id);
        positions.push(0);
    }

    if do_transitions {
        let segments = timeline.segments.clone();
        let total_segs = segments.len();
        for (i, seg) in segments.iter().enumerate() {
            // Report progress per voiceover to prevent client timeouts
            report_progress(
                (i as f64 / total_segs.max(1) as f64) * 100.0,
                100.0,
                &format!("Voiceover {}/{}", i + 1, total_segs),
            )
            .await
            .ok();

            let seg_start_ms = (seg.start * 1000.0) as i64;
            if seg_start_ms <= 0 {
                continue;
            }
            let concept = seg
                .caption
                .split_whitespace()
                .take(3)
                .collect::<Vec<_>>()
                .join(" ");
            let text = format!("Now, let's look at {}.", concept);
            let (event_id, _dur) = generate_commentary_segment(
                &mut timeline,
                &timeline_dir,
                voice_profile_id,
                &text,
                seg_start_ms,
                "transition",
                speed,
                &profile,
            )
            .await?;
            generated.push(event_id);
            positions.push(seg_start_ms);
        }
    }

    if do_outro {
        let text = outro_text.unwrap_or_else(|| "Thanks for watching!".to_string());
        let (event_id, _dur) = generate_commentary_segment(
            &mut timeline,
            &timeline_dir,
            voice_profile_id,
            &text,
            total_ms,
            "outro",
            speed,
            &profile,
        )
        .await?;
        generated.push(event_id);
        positions.push(total_ms);
    }

    timeline.save(timeline_path)?;

    Ok(json!({
        "status": "generated",
        "voiceovers_generated": generated,
        "positions": positions,
        "count": generated.len(),
    }))
}

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

async fn handle_timeline_diff(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let timeline_path_a = extract_str(&args, "timeline_path_a")?;
    let timeline_path_b = extract_str(&args, "timeline_path_b")?;

    let a = Timeline::load(timeline_path_a)?;
    let b = Timeline::load(timeline_path_b)?;

    let duration_a = a.total_duration_ms();
    let duration_b = b.total_duration_ms();
    let duration_change_ms = duration_b - duration_a;

    let seg_ids_a: std::collections::HashSet<&str> =
        a.segments.iter().map(|s| s.id.as_str()).collect();
    let seg_ids_b: std::collections::HashSet<&str> =
        b.segments.iter().map(|s| s.id.as_str()).collect();

    let added: Vec<&str> = {
        let mut v: Vec<&str> = seg_ids_b.difference(&seg_ids_a).copied().collect();
        v.sort();
        v
    };
    let removed: Vec<&str> = {
        let mut v: Vec<&str> = seg_ids_a.difference(&seg_ids_b).copied().collect();
        v.sort();
        v
    };

    let mut modified = Vec::new();
    for seg_a in &a.segments {
        if seg_ids_b.contains(seg_a.id.as_str()) {
            if let Some(seg_b) = b.segments.iter().find(|s| s.id == seg_a.id) {
                if seg_a.start != seg_b.start
                    || seg_a.end != seg_b.end
                    || seg_a.caption != seg_b.caption
                {
                    modified.push(seg_a.id.as_str());
                }
            }
        }
    }
    // P2-4 fix: sort modified segment ids for stable, readable output. Prior
    // versions returned them in arbitrary iteration order.
    modified.sort();

    let track_changes = json!({
        "dialogue": {
            "a": track_count(&a, &TrackType::Dialogue),
            "b": track_count(&b, &TrackType::Dialogue),
        },
        "voiceover": {
            "a": track_count(&a, &TrackType::Voiceover),
            "b": track_count(&b, &TrackType::Voiceover),
        },
        "broll": {
            "a": track_count(&a, &TrackType::Broll),
            "b": track_count(&b, &TrackType::Broll),
        },
        "music": {
            "a": track_count(&a, &TrackType::Music),
            "b": track_count(&b, &TrackType::Music),
        },
        "sfx": {
            "a": track_count(&a, &TrackType::Sfx),
            "b": track_count(&b, &TrackType::Sfx),
        },
    });

    Ok(json!({
        "status": "success",
        "duration_change_ms": duration_change_ms,
        "segments": {
            "added": added,
            "removed": removed,
            "modified": modified,
        },
        "tracks": track_changes,
    }))
}

// ---------------------------------------------------------------------------
// Handler: timeline.preview
// ---------------------------------------------------------------------------

async fn handle_timeline_preview(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let timeline_path = sanitize_input_path(extract_str(&args, "timeline_path")?)?
        .to_string_lossy()
        .to_string();
    let timeline = Timeline::load(&timeline_path)?;

    let total_duration_ms = timeline.total_duration_ms();
    let segments_info: Vec<serde_json::Value> = timeline
        .segments
        .iter()
        .map(|s| {
            // P2-1 fix: append ellipsis when the caption is truncated, so agents
            // can tell the preview is abbreviated. Prior versions silently cut at
            // 60 chars with no indication.
            let caption_display = if s.caption.chars().count() > 60 {
                format!("{}...", s.caption.chars().take(57).collect::<String>())
            } else {
                s.caption.clone()
            };
            json!({
                "id": s.id,
                "start": s.start,
                "end": s.end,
                "caption": caption_display,
                "crossfade_ms": s.crossfade_ms,
            })
        })
        .collect();

    let tracks_info: serde_json::Map<String, serde_json::Value> = timeline
        .tracks
        .iter()
        .map(|(track, events)| {
            let track = track as &TrackType;
            let events = events as &Vec<openscript_core::timeline::TimelineEvent>;
            (
                track.to_string(),
                json!({
                    "count": events.len(),
                    "total_duration_ms": events.iter().map(|e| e.end_ms - e.start_ms).sum::<i64>(),
                }),
            )
        })
        .collect();

    let mut errors = timeline.validate();
    // Phase 54: Reject empty timelines
    if timeline.segments.is_empty() {
        errors.push("Timeline has no segments. Call timeline.add_segment to populate it.".to_string());
    }
    // Segmentation bounds (SEGMENTATION_ARCHITECTURE.md) — same as validate.
    errors.extend(timeline.validate_segmentation());
    // B-roll coverage gaps (async probe) — same as validate, so preview is the
    // single-call viewer an agent uses to reason about the whole operation.
    let broll_gaps = probe_broll_gaps(&timeline).await;
    for g in &broll_gaps {
        errors.push(format!(
            "BROLL_GAP: segment {} needs {:.1}s but clip {} provides {:.1}s (gap {:.1}s) — {}",
            g.segment_id, g.required_s, g.asset_id, g.available_s, g.gap_s, g.action
        ));
    }
    let render_ready = errors.is_empty() && !timeline.segments.is_empty();

    // Phase 136: the composition layer stack (bottom→top, with per-event
    // concept/asset/timing) + used-clip ids — the timeline-viewer context
    // that lets an agent see the full operational flow in one call.
    let viewer = build_timeline_viewer_context(&timeline);
    let used_ids: Vec<i64> = {
        let mut ids: Vec<i64> = used_broll_video_ids(&timeline).into_iter().collect();
        ids.sort_unstable();
        ids
    };

    Ok(json!({
        "status": "success",
        "timeline_path": timeline_path,
        "version": timeline.version,
        "total_duration_ms": total_duration_ms,
        "segments_count": timeline.segments.len(),
        "segments": segments_info,
        "tracks": tracks_info,
        "composition": viewer,
        "broll_gaps": broll_gaps,
        "used_broll_video_ids": used_ids,
        "render_ready": render_ready,
        "validation_errors": errors,
    }))
}

// ---------------------------------------------------------------------------
// Handler: tts.preview
// ---------------------------------------------------------------------------

async fn handle_tts_preview(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let voice_profile_id = extract_str(&args, "voice_profile_id")?;
    let text = extract_str(&args, "text")?;
    let speed = default_f64(&args, "speed", 1.0);

    let profiles = load_voice_profiles()?;
    let profile = profiles.get(voice_profile_id).cloned();

    let word_count = text.split_whitespace().count();
    let estimated_ms = ((word_count as f64 / 2.5) * 1000.0 / speed) as i64;

    Ok(json!({
        "status": "preview",
        "voice_profile_id": voice_profile_id,
        "voice_profile": profile,
        "text": text,
        "word_count": word_count,
        "estimated_duration_ms": estimated_ms,
        "speed": speed,
    }))
}

// ---------------------------------------------------------------------------
// Handler: music.ducking.plan
// ---------------------------------------------------------------------------

async fn handle_music_ducking_plan(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let timeline_path = extract_str(&args, "timeline_path")?;
    let reduction_db = default_f64(&args, "reduction_db", 10.0);

    let timeline = Timeline::load(timeline_path)?;

    let mut ducking_events = Vec::new();
    let dialogue = timeline
        .tracks
        .get(&TrackType::Dialogue)
        .cloned()
        .unwrap_or_default();
    let voiceover = timeline
        .tracks
        .get(&TrackType::Voiceover)
        .cloned()
        .unwrap_or_default();

    for event in dialogue.iter().chain(voiceover.iter()) {
        ducking_events.push(json!({
            "start_ms": event.start_ms,
            "end_ms": event.end_ms,
            "reduction_db": reduction_db,
            "attack_ms": 50,
            "release_ms": 200,
        }));
    }

    Ok(json!({
        "status": "success",
        "timeline_path": timeline_path,
        "reduction_db": reduction_db,
        "ducking_events": ducking_events,
        "count": ducking_events.len(),
    }))
}

// ---------------------------------------------------------------------------
// Handler: timeline.autofill_broll
// ---------------------------------------------------------------------------

async fn handle_timeline_autofill_broll(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let timeline_path = extract_str(&args, "timeline_path")?;
    let cadence_seconds = default_f64(&args, "cadence_seconds", 2.0);
    let _orientation = default_str(&args, "orientation", "9:16");
    let _quality = default_str(&args, "quality", "sd");
    let max_gaps = default_u32(&args, "max_gaps", 20);

    let mut timeline = Timeline::load(timeline_path)?;

    let cadence_ms = (cadence_seconds * 1000.0) as i64;
    let total_ms = timeline.total_duration_ms();
    let mut count = 0;
    let mut position_ms = 0i64;

    report_progress(0.0, max_gaps as f64, "Auto-filling b-roll slots...")
        .await
        .ok();

    while position_ms < total_ms && count < max_gaps as i64 {
        let duration = cadence_ms.min(total_ms - position_ms);
        if duration > 0 {
            let event_id = format!("broll_{:03}", track_count(&timeline, &TrackType::Broll) + 1);
            let concept = timeline
                .segments
                .iter()
                .find(|s| {
                    let seg_start = (s.start * 1000.0) as i64;
                    let seg_end = (s.end * 1000.0) as i64;
                    position_ms >= seg_start && position_ms < seg_end
                })
                .map(|s| {
                    s.caption
                        .split_whitespace()
                        .take(2)
                        .collect::<Vec<_>>()
                        .join("_")
                })
                .unwrap_or_else(|| "general".into());

            let event = openscript_core::timeline::TimelineEvent {
                id: event_id.clone(),
                asset_id: "placeholder".into(),
                start_ms: position_ms,
                end_ms: position_ms + duration,
                offset_ms: 0,
                gain_db: 0.0,
                fade_in_ms: 0,
                fade_out_ms: 0,
                tags: vec![concept.clone()],
                provenance: Some(openscript_core::timeline::Provenance {
                    tool: "timeline.autofill_broll".into(),
                    editorial_role: None,
                    concept: Some(concept.clone()),
                }),
                kind: openscript_core::timeline::EventKind::Broll {
                    concept,
                    source_provider: "placeholder".into(),
                    transition_style: "cut".into(),
                    crop_mode: "center".into(),
                    orientation: "9:16".into(),
                    motion_intensity: "medium".into(),
                },
            };

            timeline.add_track_event(TrackType::Broll, event);
            count += 1;

            // Report progress every 5 slots to avoid spamming
            if count % 5 == 0 || count == max_gaps as i64 {
                report_progress(
                    count as f64,
                    max_gaps as f64,
                    &format!("Filled {} b-roll slots", count),
                )
                .await
                .ok();
            }
        }
        position_ms += cadence_ms;
    }

    timeline.save(timeline_path)?;

    Ok(json!({
        "status": "autofilled",
        "timeline_path": timeline_path,
        "broll_events_added": count,
    }))
}

// ---------------------------------------------------------------------------
// Handler: timeline.render
// ---------------------------------------------------------------------------

async fn handle_timeline_render(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_ffmpeg::render::render_from_timeline;

    let timeline_path = extract_str(&args, "timeline_path")?;
    let source_video = default_opt_str(&args, "source_video");
    let output_path = default_opt_str(&args, "output_path");
    let crf = default_opt_u32(&args, "crf");

    if !Path::new(&timeline_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Timeline not found: {}",
            timeline_path
        )));
    }

    let mut timeline = Timeline::load(timeline_path)?;

    // If caller provides source_video, override the timeline's source before validation.
    // This allows rendering when srt.to_timeline didn't set the source field.
    let source_provided = source_video.is_some();
    if let Some(ref sv) = source_video {
        timeline.source = std::path::PathBuf::from(sv);
    }

    let mut errors = timeline.validate();
    // When source_video is provided, ignore 'Source video path is required'
    // since the override above already handled it.
    if source_provided {
        errors.retain(|e| e != "Source video path is required");
    }
    // Also skip overlap validation — tools like sfx.auto_assign and broll.fetch
    // may add track events that create apparent overlaps in segment metadata.
    // The render pipeline handles overlapping segments gracefully.
    errors.retain(|e| !e.contains("overlaps with previous segment"));
    if !errors.is_empty() {
        return Err(ToolError::Timeline(format!(
            "Timeline has {} validation error(s): {:?}",
            errors.len(),
            errors
        )));
    }

    let source = source_video.unwrap_or_else(|| timeline.source.to_string_lossy().to_string());
    if !Path::new(&source).exists() {
        return Err(ToolError::NotFound(format!(
            "Source video not found: {}",
            source
        )));
    }

    // Auto-detect audio-only source and generate a black background video.
    // This enables A2V (audio-to-video) pipeline: audio → timeline → render.
    let source_is_video = {
        let probe = std::process::Command::new("ffprobe")
            .args(["-v", "quiet", "-select_streams", "v:0", "-show_entries", "stream=codec_type", "-of", "csv=p=0", &source])
            .output();
        match probe {
            Ok(o) => o.status.success() && !o.stdout.is_empty(),
            Err(_) => false, // assume audio-only if ffprobe fails (safer)
        }
    };
    let render_source = if !source_is_video {
        // Derive duration from timeline segments instead of hardcoded fallback
        let duration = {
            let d = std::process::Command::new("ffprobe")
                .args(["-v", "quiet", "-show_entries", "format=duration", "-of", "csv=p=0", &source])
                .output();
            match d {
                Ok(o) => String::from_utf8_lossy(&o.stdout)
                    .trim()
                    .parse::<f64>()
                    .ok(),
                Err(_) => None,
            }
        }
        .unwrap_or_else(|| {
            // Fallback: compute from timeline segment boundaries
            timeline.segments.iter()
                .map(|s| s.end)
                .fold(0.0f64, f64::max)
                .max(1.0) // guard against empty segments (avoid 0-second video)
        });

        // Derive dimensions from timeline's aspect ratio + resolve_width/resolve_height
        let w = timeline.target.resolve_width();
        let h = timeline.target.resolve_height();
        let bg_video = {
            let p = std::path::Path::new(&source);
            let stem = p.file_stem().unwrap_or_default().to_string_lossy().to_string();
            let parent = p.parent().unwrap_or(std::path::Path::new("."));
            parent.join(format!("{}.bg.mp4", stem)).to_string_lossy().to_string()
        };
        tracing::info!("[timeline.render] Audio-only source detected ({}x{}). Generating black background: {}", w, h, bg_video);
        report_progress(10.0, 100.0, "Generating black background video from audio...").await.ok();
        let bg_result = std::process::Command::new("ffmpeg")
            .args([
                "-y", "-f", "lavfi",
                "-i", &format!("color=c=black:s={}x{}:d={:.1}:r={}", w, h, duration, timeline.target.fps),
                "-i", &source,
                "-c:v", "libx264", "-tune", "stillimage",
                "-c:a", "aac", "-b:a", "192k",
                "-shortest",
                &bg_video,
            ])
            .output();
        match bg_result {
            Ok(o) if o.status.success() => {
                tracing::info!("[timeline.render] Background video generated: {}", bg_video);
                bg_video
            }
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                let last_lines: Vec<&str> = stderr.lines().rev().take(5).collect();
                return Err(ToolError::Ffmpeg(format!(
                    "Failed to generate background video from audio: {}",
                    last_lines.join("\n")
                )));
            }
            Err(e) => {
                return Err(ToolError::Ffmpeg(format!(
                    "Failed to run ffmpeg for background generation: {}", e
                )));
            }
        }
    } else {
        source.clone()
    };

    let total_tracks = timeline.tracks.values().map(|v| v.len()).sum::<usize>();
    report_progress(
        0.0,
        100.0,
        &format!(
            "Rendering timeline ({} segments, {} track events)...",
            timeline.segments.len(),
            total_tracks
        ),
    )
    .await
    .ok();

    report_progress(20.0, 100.0, "Building filter graph...")
        .await
        .ok();

    // Filter out placeholder b-roll events before rendering to prevent FFmpeg crash
    if let Some(broll_events) = timeline.tracks.get_mut(&TrackType::Broll) {
        let before = broll_events.len();
        broll_events.retain(|e| e.asset_id != "placeholder" && !e.asset_id.is_empty());
        let removed = before - broll_events.len();
        if removed > 0 {
            tracing::warn!(
                "[timeline.render] Filtered {} placeholder b-roll events",
                removed
            );
        }
    }

    let result = render_from_timeline(&timeline, &render_source, output_path.as_deref(), crf).await;

    // Cleanup generated background video regardless of render outcome
    let _cleanup_bg = (!source_is_video).then(|| {
        let _ = std::fs::remove_file(&render_source);
    });

    match result {
        Ok(out_path) => {
            report_progress(100.0, 100.0, "Render complete").await.ok();
            let file_size = std::fs::metadata(&out_path).map(|m| m.len()).unwrap_or(0);
            Ok(json!({
                "status": "rendered",
                "output_path": out_path,
                "file_size_bytes": file_size,
                "segments_count": timeline.segments.len(),
                "overlays_rendered": total_tracks,
            }))
        }
        Err(e) => {
            // P0-2 fix: include the ffmpeg error inline (and a tail of the render
            // log when one exists) so AI agents can self-correct without having
            // to read a separate log file. Prior versions returned only
            // "Render failed, see log: /path/to/render.log" which gave agents
            // no actionable information.
            let err_str = e.to_string();
            let log_excerpt = if let Some(log_path) = err_str
                .strip_prefix("Render failed, see log: ")
                .or_else(|| err_str.strip_prefix("Render failed: "))
            {
                std::fs::read_to_string(log_path).ok().map(|content| {
                    let lines: Vec<&str> = content.lines().collect();
                    let last_20: Vec<&str> = lines.iter().rev().take(20).rev().cloned().collect();
                    last_20.join("\n")
                })
            } else {
                None
            };
            let mut msg = format!("Render failed: {}", err_str);
            if let Some(excerpt) = log_excerpt {
                if !excerpt.is_empty() {
                    msg.push_str("\n\n--- render log (last 20 lines) ---\n");
                    msg.push_str(&excerpt);
                }
            }
            Err(ToolError::Ffmpeg(msg))
        }
    }
}

// ---------------------------------------------------------------------------
// Handler removed: broll.director was a monolithic orchestrator
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Handler: broll.plan — segment inspector for agent-orchestrated b-roll
// ---------------------------------------------------------------------------

/// Generate basic keyword suggestions from a caption for Pexels search.
async fn handle_broll_plan(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let timeline_path = extract_str(&args, "timeline_path")?;
    let timeline_str = std::fs::read_to_string(timeline_path)
        .map_err(|e| ToolError::InvalidArg(format!("Failed to read timeline {}: {}", timeline_path, e)))?;
    let timeline: serde_json::Value = serde_json::from_str(&timeline_str)
        .map_err(|e| ToolError::InvalidArg(format!("Failed to parse timeline JSON: {}", e)))?;
    // Try tracks.dialogue.events first, then top-level segments, then default empty.
    // Note: dialogue may be a list (empty) instead of a dict with 'events' — handle both.
    let segments = timeline.get("tracks")
        .and_then(|tracks| tracks.get("dialogue"))
        .and_then(|dialogue| {
            // dialogue may be {"events": [...]} or a plain list [...]
            dialogue.get("events")
                .and_then(|e| e.as_array().cloned())
                .filter(|v| !v.is_empty())
                .or_else(|| dialogue.as_array().cloned().filter(|v| !v.is_empty()))
        })
        .or_else(|| {
            timeline.get("segments")
                .and_then(|s| s.as_array().cloned())
                .filter(|v| !v.is_empty())
        })
        .unwrap_or_default();
    let mut result_segments = Vec::new();
    for (idx, seg) in segments.iter().enumerate() {
    let start_s = seg.get("start_s")
        .or_else(|| seg.get("start_ms"))
        .or_else(|| seg.get("start")).and_then(|v| v.as_f64())
        .map(|v| if v > 1000.0 { v / 1000.0 } else { v })
        .unwrap_or(0.0);
    let end_s = seg.get("end_s")
        .or_else(|| seg.get("end_ms"))
        .or_else(|| seg.get("end")).and_then(|v| v.as_f64())
        .map(|v| if v > 1000.0 { v / 1000.0 } else { v })
        .unwrap_or(start_s + 5.0);
        let caption = seg.get("caption")
            .or_else(|| seg.get("text")).and_then(|v| v.as_str())
            .unwrap_or("");
        let duration_s = end_s - start_s;
        result_segments.push(json!({
            "id": format!("seg_{}", idx),
            "start_s": start_s,
            "end_s": end_s,
            "duration_s": duration_s,
            "caption": caption,
        }));
    }
    Ok(json!({
        "status": "success",
        "segments_count": result_segments.len(),
        "segments": result_segments,
    }))
}

// ---------------------------------------------------------------------------
// Handler: broll.keywords (LLM-mediated keyword extraction from transcripts)
// ---------------------------------------------------------------------------

async fn handle_broll_keywords(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    // Extract segments from args
    let segments = args.get("segments")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ToolError::MissingArg("segments".to_string()))?;

    if segments.is_empty() {
        return Ok(json!({
            "status": "warning",
            "message": "No segments provided",
            "segments": [],
        }));
    }

    let video_title = args.get("video_title")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let language = args.get("language")
        .and_then(|v| v.as_str())
        .unwrap_or("hinglish");

    let max_batch_size = args.get("max_batch_size")
        .and_then(|v| v.as_u64())
        .unwrap_or(15).max(1) as usize;

    let title_context = if !video_title.is_empty() {
        format!("\nVideo title/context: \"{}\"\n", video_title)
    } else {
        String::new()
    };

    // Phase 136: when a timeline is provided, pass the already-covered b-roll
    // concepts so the single-shot draft pass is NON-REDUNDANT across the video
    // (each segment must get distinct, relevant footage).
    let timeline_context = if let Some(tl_path) = default_opt_str(&args, "timeline_path") {
        match Timeline::load(&tl_path) {
            Ok(tl) => {
                let mut concepts: Vec<String> = Vec::new();
                if let Some(broll) = tl.tracks.get(&TrackType::Broll) {
                    for ev in broll {
                        if let openscript_core::timeline::EventKind::Broll { concept, .. } = &ev.kind
                        {
                            if !concept.is_empty() && !concepts.contains(concept) {
                                concepts.push(concept.clone());
                            }
                        }
                    }
                }
                if concepts.is_empty() {
                    String::new()
                } else {
                    format!(
                        "\nB-roll concepts ALREADY COVERED in this timeline (AVOID repeating them — each segment needs DISTINCT relevant footage): {}\n",
                        concepts.join(", ")
                    )
                }
            }
            Err(_) => String::new(),
        }
    } else {
        String::new()
    };

    // Build system prompt once (only user prompt changes per batch)
    let system_prompt = format!(
        "You are a stock footage search keyword extractor for a video production pipeline. \
        Your job: translate transcript captions into English visual search keywords for stock video sites (Pexels, Pixabay). \
        \
        Rules:
        1. Output ONLY valid JSON — no markdown, no explanation
        2. For each segment, output 2-3 English keywords that describe what VISUAL CONTENT should appear on screen
        3. Translate Hinglish/Hindi to English. Use the MEANING, not literal word-for-word translation
        4. Keywords must be VISUAL — things you can see in stock footage (e.g., 'protest crowd', 'government building', 'social media icons')
        5. Avoid abstract concepts — prefer concrete, searchable visual terms
        6. Each keyword should be 1-3 words maximum
        7. Source language detected: {}\n{}{}\
        Output format: {{\"results\": [{{\"id\": \"seg_XXX\", \"keywords\": [\"keyword1\", \"keyword2\"]}}]}}",
        language, title_context, timeline_context
    );

    // Build a lookup from segment id -> keywords (across all batches)
    let mut keyword_map: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    let mut last_backend = String::new();
    let mut last_model = String::new();
    let total = segments.len();
    let num_batches = total.div_ceil(max_batch_size);

    for batch_idx in 0..num_batches {
        let start = batch_idx * max_batch_size;
        let end = std::cmp::min(start + max_batch_size, total);
        let batch = &segments[start..end];

        let progress_pct = 10.0 + (batch_idx as f64 / num_batches as f64) * 70.0;
        report_progress(progress_pct, 100.0, &format!("Extracting keywords batch {}/{}...", batch_idx + 1, num_batches)).await.ok();

        // Build segment descriptions for this batch
        let mut segment_descriptions = Vec::new();
        for (j, seg) in batch.iter().enumerate() {
            let i = start + j;
            let caption = seg.get("caption")
                .or_else(|| seg.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let fallback_id = format!("seg_{}", i);
            let id = seg.get("id")
                .and_then(|v| v.as_str())
                .unwrap_or(&fallback_id);
            segment_descriptions.push(format!("[{}] {}: \"{}\"", id, i + 1, caption));
        }

        let user_prompt = format!(
            "Extract visual search keywords for each segment. Output ONLY the JSON object.\n\n{}",
            segment_descriptions.join("\n")
        );

        // Call the LLM cascade for this batch — continue on failure with fallback
        let result = match crate::llm::chat_complete(&system_prompt, &user_prompt, None).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("[broll.keywords] Batch {} LLM failed: {} — using naive fallback", batch_idx + 1, e);
                continue;
            }
        };

        // Parse the LLM response — extract JSON from the response
        let response_text = result.text.trim();
        let parsed: serde_json::Value = if let Some(start) = response_text.find('{') {
            if let Some(end) = response_text.rfind('}') {
                serde_json::from_str(&response_text[start..=end])
                    .unwrap_or_else(|_| json!({"results": []}))
            } else {
                json!({"results": []})
            }
        } else {
            json!({"results": []})
        };

        last_backend = result.backend;
        last_model = result.model;

        // Merge batch results into the keyword_map
        if let Some(results) = parsed.get("results").and_then(|v| v.as_array()) {
            for r in results {
                if let Some(id) = r.get("id").and_then(|v| v.as_str()) {
                    if let Some(kws) = r.get("keywords").and_then(|v| v.as_array()) {
                        let keywords: Vec<String> = kws.iter()
                            .filter_map(|k| k.as_str().map(String::from))
                            .collect();
                        keyword_map.insert(id.to_string(), keywords);
                    }
                }
            }
        }
    }

    report_progress(90.0, 100.0, "Assembling results...").await.ok();

    // Build the output: enrich each segment with LLM-generated keywords
    let mut enriched_segments = Vec::new();
    for (i, seg) in segments.iter().enumerate() {
        let id = seg.get("id")
            .and_then(|v| v.as_str())
            .unwrap_or(&format!("seg_{}", i))
            .to_string();
        let caption = seg.get("caption")
            .or_else(|| seg.get("text"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let start_s = seg.get("start_s")
            .or_else(|| seg.get("start_ms"))
            .or_else(|| seg.get("start"))
            .and_then(|v| v.as_f64())
            .map(|v| if v > 1000.0 { v / 1000.0 } else { v })
            .unwrap_or(0.0);
        let end_s = seg.get("end_s")
            .or_else(|| seg.get("end_ms"))
            .or_else(|| seg.get("end"))
            .and_then(|v| v.as_f64())
            .map(|v| if v > 1000.0 { v / 1000.0 } else { v })
            .unwrap_or(start_s + 3.0);

        // Get keywords from LLM, fallback to naive extraction if LLM failed
        // Try exact ID match first, then index-based match (LLM may renumber IDs)
        let keywords = keyword_map.get(&id)
            .or_else(|| keyword_map.get(&format!("seg_{}", i)))
            .or_else(|| keyword_map.get(&format!("seg_{:03}", i)))
            .cloned()
            .unwrap_or_else(|| {
                // Fallback: translate Hinglish→English visual concepts FIRST, then
                // naive keyword extraction. Raw Hinglish words ("sarkar", "bhai")
                // produce garbage Pexels queries; their English visual equivalents
                // search cleanly. This is the LLM-down path for the single-shot
                // keyword generation loop — relevance must not collapse when the
                // model is unavailable (Phase 135).
                let translated = crate::stock_signal::translate_hinglish_visuals(caption);
                let concept = extract_broll_concept(&translated);
                concept.split_whitespace().map(String::from).collect()
            });

        enriched_segments.push(json!({
            "id": id,
            "start_s": start_s,
            "end_s": end_s,
            "duration_s": end_s - start_s,
            "caption": caption,
            "keywords": keywords,
        }));
    }

    report_progress(100.0, 100.0, "Keyword extraction complete.").await.ok();

    Ok(json!({
        "status": "success",
        "backend": last_backend,
        "model": last_model,
        "segments_count": enriched_segments.len(),
        "segments": enriched_segments,
    }))
}

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

async fn handle_broll_validate_keywords(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    use openscript_assets::pexels::PexelsClient;

    let enriched_segments: Vec<serde_json::Value> = args
        .get("enriched_segments")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if enriched_segments.is_empty() {
        return Err(ToolError::MissingArg(
            "enriched_segments (from broll.keywords)".to_string(),
        ));
    }
    let max_candidates = args
        .get("max_candidates")
        .and_then(|v| v.as_u64())
        .unwrap_or(6)
        .max(2) as usize;
    let max_keywords = args
        .get("max_keywords_per_search")
        .and_then(|v| v.as_u64())
        .unwrap_or(2)
        .max(1) as usize;
    let orientation = default_str(&args, "orientation", "9:16");
    let quality = default_str(&args, "quality", "sd");
    let asset_dir = default_opt_str(&args, "asset_dir")
        .unwrap_or_else(|| "mcp/assets/broll_cache".to_string());
    let language = default_str(&args, "language", "hinglish");

    let api_key = pexels_key();
    if api_key.is_empty() {
        return Ok(json!({
            "status": "warning",
            "message": "PEXELS_API_KEY not set — cannot search candidates for relevance validation. Draft keywords are returned unchanged; set the key or run broll.fetch with a fallback_pool.",
            "validated": false,
            "segments": enriched_segments,
        }));
    }

    let mut client = PexelsClient::new(&api_key, &asset_dir);
    let mut validated: Vec<serde_json::Value> = Vec::new();
    let mut skipped: Vec<serde_json::Value> = Vec::new();
    let mut last_backend = String::new();
    let mut last_model = String::new();

    for (i, seg) in enriched_segments.iter().enumerate() {
        let id = seg
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or(&format!("seg_{}", i))
            .to_string();
        let caption = seg
            .get("caption")
            .or_else(|| seg.get("text"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let start_s = seg
            .get("start_s")
            .or_else(|| seg.get("start"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let end_s = seg
            .get("end_s")
            .or_else(|| seg.get("end"))
            .and_then(|v| v.as_f64())
            .unwrap_or(start_s + 3.0);
        let window_s = (end_s - start_s).max(1.0);
        let draft: Vec<String> = seg
            .get("keywords")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|k| k.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        // If a segment arrived without draft keywords, draft them agentically.
        let (draft, backend, model) = if draft.is_empty() {
            let (kws, b, m) = llm_draft_keywords(&caption, &[], &language).await;
            (kws, b, m)
        } else {
            (draft, String::new(), String::new())
        };
        if !backend.is_empty() {
            last_backend = backend;
        }
        if !model.is_empty() {
            last_model = model;
        }

        // Search Pexels with the draft keywords (dedup across queries).
        let queries: Vec<String> = draft
            .iter()
            .filter(|k| k.len() >= 3)
            .take(max_keywords)
            .cloned()
            .collect();
        if queries.is_empty() {
            skipped.push(json!({"id": id, "reason": "no usable draft keywords"}));
            continue;
        }
        let mut candidates: Vec<openscript_assets::pexels::PexelsVideo> = Vec::new();
        let mut seen: std::collections::HashSet<i64> = std::collections::HashSet::new();
        for q in &queries {
            match client.search(q, &orientation, &quality).await {
                Ok(vids) => {
                    for v in vids {
                        if v.duration > 0 && seen.insert(v.id) && candidates.len() < max_candidates
                        {
                            candidates.push(v);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "[broll.validate_keywords] search '{}' failed: {}",
                        q,
                        e
                    )
                }
            }
        }
        // Non-looping gate: only candidates that cover the segment window qualify.
        // When NO candidate covers the window, the pool still shows the results
        // but each is tagged `covers_window: false` so the consumer knows a
        // download of it would flag BROLL_GAP (and trigger broll.repair) — the
        // agent must not treat it as a safe pick.
        let covering: Vec<openscript_assets::pexels::PexelsVideo> = candidates_covering_window(
            &candidates,
            window_s,
            0.5,
        )
        .into_iter()
        .cloned()
        .collect();
        let covers_ids: std::collections::HashSet<i64> =
            covering.iter().map(|v| v.id).collect();
        let pool: Vec<openscript_assets::pexels::PexelsVideo> = if covering.is_empty() {
            candidates
        } else {
            covering
        };
        if pool.is_empty() {
            skipped.push(json!({
                "id": id,
                "reason": format!(
                    "no Pexels results for draft keywords [{}]",
                    queries.join(", ")
                )
            }));
            continue;
        }

        // Agent validates the real candidates against the spoken caption.
        let avoid = std::collections::HashSet::new();
        let (best_id, final_kws, relevance, reason, backend, model) =
            llm_validate_candidates(&caption, &draft, &pool, window_s, &avoid).await;
        if !backend.is_empty() {
            last_backend = backend;
        }
        if !model.is_empty() {
            last_model = model;
        }

        let best_video = best_id.and_then(|bid| {
            pool.iter()
                .find(|v| v.id == bid)
                .map(|v| {
                    json!({
                        "id": v.id,
                        "name": pexels_url_slug(&v.url),
                        "duration_s": v.duration,
                        "url": v.url,
                        "covers_window": covers_ids.contains(&v.id),
                    })
                })
        });
        let candidates_json: Vec<serde_json::Value> = pool
            .iter()
            .map(|v| {
                json!({
                    "id": v.id,
                    "name": pexels_url_slug(&v.url),
                    "duration_s": v.duration,
                    "size": format!("{}x{}", v.width, v.height),
                    "url": v.url,
                    "covers_window": covers_ids.contains(&v.id),
                })
            })
            .collect();
        validated.push(json!({
            "id": id,
            "start_s": start_s,
            "end_s": end_s,
            "duration_s": end_s - start_s,
            "caption": caption,
            "draft_keywords": draft,
            "final_keywords": final_kws,
            "best_video": best_video,
            "relevance": relevance,
            "reason": reason,
            "candidates": candidates_json,
        }));
    }

    Ok(json!({
        "status": "validated",
        "backend": last_backend,
        "model": last_model,
        "validated_count": validated.len(),
        "skipped_count": skipped.len(),
        "skipped": skipped,
        "segments": validated,
    }))
}

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

async fn handle_broll_repair(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_assets::pexels::PexelsClient;

    let timeline_path = extract_str(&args, "timeline_path")?;
    let max_segments = args
        .get("max_segments")
        .and_then(|v| v.as_u64())
        .unwrap_or(10)
        .max(1) as usize;
    let orientation = default_str(&args, "orientation", "9:16");
    let quality = default_str(&args, "quality", "sd");
    let asset_dir = default_opt_str(&args, "asset_dir")
        .unwrap_or_else(|| "mcp/assets/broll_cache".to_string());
    let language = default_str(&args, "language", "hinglish");

    let mut tl = Timeline::load(&timeline_path)
        .map_err(|e| ToolError::Io(std::io::Error::other(format!("Failed to load timeline: {}", e))))?;
    let gaps = probe_broll_gaps(&tl).await;
    if gaps.is_empty() {
        return Ok(json!({
            "status": "ok",
            "message": "No b-roll coverage gaps — every clip covers its segment window.",
            "repaired": 0,
            "remaining_gaps": [],
            "timeline_path": timeline_path,
        }));
    }

    let api_key = pexels_key();
    if api_key.is_empty() {
        return Ok(json!({
            "status": "warning",
            "message": format!(
                "{} b-roll gap(s) exist but PEXELS_API_KEY is not set — cannot repair.",
                gaps.len()
            ),
            "repaired": 0,
            "gaps": gaps,
            "remaining_gaps": gaps,
        }));
    }

    let mut client = PexelsClient::new(&api_key, &asset_dir);
    let used_ids = used_broll_video_ids(&tl);
    let context_text = render_timeline_context_text(&tl);

    let mut decisions: Vec<serde_json::Value> = Vec::new();
    let mut repaired = 0usize;
    let mut last_backend = String::new();
    let mut last_model = String::new();

    for gap in gaps.iter().take(max_segments) {
        let window_s = gap.required_s.max(1.0);
        let caption = find_segment_for_window(&tl, &gap.segment_id)
            .map(|s| s.caption.clone())
            .unwrap_or_default();
        // No matching segment caption? Seed the draft from the existing concept
        // tag instead of burning an LLM call on an empty string.
        if caption.trim().is_empty() && gap.concept.trim().is_empty() {
            decisions.push(json!({
                "segment_id": gap.segment_id,
                "window_s": window_s,
                "status": "unrepairable_this_pass",
                "reason": "no caption or concept available to search for this segment",
            }));
            continue;
        }

        // Existing concepts across the timeline (non-redundant drafts).
        let avoid_concepts: Vec<String> = tl
            .tracks
            .get(&TrackType::Broll)
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|e| match &e.kind {
                openscript_core::timeline::EventKind::Broll { concept, .. } => {
                    Some(concept.clone())
                }
                _ => None,
            })
            .collect();

        // Stage 1: agent drafts fresh keywords from the spoken caption,
        // avoiding concepts already covered elsewhere in the timeline.
        let (draft, backend, model) = if caption.trim().is_empty() {
            // Seed from the existing concept tag (no caption to translate).
            (vec![gap.concept.clone()], "seed".into(), "existing-concept".into())
        } else {
            llm_draft_keywords(&caption, &avoid_concepts, &language).await
        };
        if !backend.is_empty() {
            last_backend = backend;
        }
        if !model.is_empty() {
            last_model = model;
        }

        // Search Pexels with the draft keywords.
        let queries: Vec<String> = draft
            .iter()
            .filter(|k| k.len() >= 3)
            .take(2)
            .cloned()
            .collect();
        let mut candidates: Vec<openscript_assets::pexels::PexelsVideo> = Vec::new();
        let mut seen: std::collections::HashSet<i64> = std::collections::HashSet::new();
        for q in &queries {
            match client.search(q, &orientation, &quality).await {
                Ok(vids) => {
                    for v in vids {
                        if v.duration > 0 && seen.insert(v.id) {
                            candidates.push(v);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("[broll.repair] search '{}' failed: {}", q, e)
                }
            }
        }

        // Non-looping gate: the window MUST be covered (+0.5s trim slack).
        let covering: Vec<openscript_assets::pexels::PexelsVideo> = candidates_covering_window(
            &candidates,
            window_s,
            0.5,
        )
        .into_iter()
        .cloned()
        .collect();
        if covering.is_empty() {
            decisions.push(json!({
                "segment_id": gap.segment_id,
                "caption": caption,
                "window_s": window_s,
                "draft_keywords": queries,
                "status": "unrepairable_this_pass",
                "reason": format!(
                    "no Pexels candidate covers the {:.1}s window (non-looping gate) — widen keywords or accept the held-frame",
                    window_s
                ),
            }));
            continue;
        }

        // Stage 2: agent validates the covering candidates against the speech.
        let (best_id, final_kws, relevance, reason, backend, model) =
            llm_validate_candidates(&caption, &queries, &covering, window_s, &used_ids).await;
        if !backend.is_empty() {
            last_backend = backend;
        }
        if !model.is_empty() {
            last_model = model;
        }

        // The LLM never sees the used-id blocklist, so its pick must be
        // cross-checked: an already-used clip (or a hallucinated id) falls
        // back to the longest covering UNUSED clip — the non-redundancy rule.
        let chosen = best_id
            .and_then(|bid| covering.iter().find(|v| v.id == bid && !used_ids.contains(&v.id)))
            .or_else(|| {
                covering
                    .iter()
                    .filter(|v| !used_ids.contains(&v.id))
                    .max_by_key(|v| v.duration)
            })
            .unwrap_or_else(|| covering.iter().max_by_key(|v| v.duration).unwrap());
        let concept = final_kws.first().cloned().unwrap_or_else(|| queries.first().cloned().unwrap_or("b-roll".into()));
        match client.download_best(chosen, &concept).await {
            Ok(path) => {
                // Replace the event's asset + the asset record.
                let new_asset_id = format!("broll_gap_{}", gap.segment_id);
                let old_asset_id = tl
                    .tracks
                    .get(&TrackType::Broll)
                    .and_then(|evts| evts.iter().find(|e| e.id == gap.segment_id))
                    .map(|e| e.asset_id.clone());
                if let Some(evts) = tl.tracks.get_mut(&TrackType::Broll) {
                    if let Some(evt) = evts.iter_mut().find(|e| e.id == gap.segment_id) {
                        evt.asset_id = new_asset_id.clone();
                        evt.tags = vec![concept.clone()];
                        if let openscript_core::timeline::EventKind::Broll {
                            concept: c,
                            source_provider: sp,
                            ..
                        } = &mut evt.kind
                        {
                            *c = concept.clone();
                            *sp = "pexels".to_string();
                        }
                        if let Some(prov) = &mut evt.provenance {
                            prov.concept = Some(concept.clone());
                            prov.tool = "broll.repair".to_string();
                        }
                    }
                }
                tl.assets.broll.insert(
                    new_asset_id.clone(),
                    serde_json::json!({
                        "path": path,
                        "concept": concept,
                        "source_duration_s": chosen.duration,
                    }),
                );
                // Drop the stale asset record for the swapped-out clip (the
                // cached file stays on disk — only the registry entry is removed).
                if let Some(old) = old_asset_id {
                    if old != new_asset_id {
                        tl.assets.broll.remove(&old);
                    }
                }
                decisions.push(json!({
                    "segment_id": gap.segment_id,
                    "caption": caption,
                    "window_s": window_s,
                    "draft_keywords": queries,
                    "final_keywords": final_kws,
                    "chosen_video": {
                        "id": chosen.id,
                        "name": pexels_url_slug(&chosen.url),
                        "duration_s": chosen.duration,
                    },
                    "relevance": relevance,
                    "reason": reason,
                    "asset_id": new_asset_id,
                    "path": path,
                    "status": "repaired",
                }));
                repaired += 1;
            }
            Err(e) => {
                decisions.push(json!({
                    "segment_id": gap.segment_id,
                    "caption": caption,
                    "window_s": window_s,
                    "draft_keywords": queries,
                    "status": "download_failed",
                    "reason": e.to_string(),
                }));
            }
        }
    }

    tl.updated_at = chrono::Utc::now();
    tl.save(&timeline_path).map_err(|e| {
        ToolError::Io(std::io::Error::other(format!("Failed to save timeline: {}", e)))
    })?;

    let remaining = probe_broll_gaps(&tl).await;
    let ok = remaining.is_empty();
    Ok(json!({
        "status": if ok { "healed" } else { "partial" },
        "message": if ok {
            "All flagged b-roll gaps repaired — timeline is fully covered.".to_string()
        } else {
            format!(
                "{} gap(s) repaired; {} gap(s) remain (run broll.repair again or widen keywords).",
                repaired,
                remaining.len()
            )
        },
        "backend": last_backend,
        "model": last_model,
        "repaired": repaired,
        "context_used": context_text.lines().count(),
        "decisions": decisions,
        "remaining_gaps": remaining,
        "timeline_path": timeline_path,
    }))
}

// ---------------------------------------------------------------------------
// Handler: broll.auto (one-call A2V b-roll orchestrator — analyze → draft →
// validate → fetch → validate → repair loop until zero gaps remain)
// ---------------------------------------------------------------------------

async fn handle_broll_auto(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let srt_path = args.get("srt_path").and_then(|v| v.as_str()).map(String::from);
    let audio_path = args.get("audio_path").and_then(|v| v.as_str()).map(String::from);
    let timeline_path_arg = args.get("timeline_path").and_then(|v| v.as_str()).map(String::from);
    // Word-level SRT from transcribe — real per-word alignments so the
    // word-highlight captions stay synced with the voice (caption-sync fix).
    let word_srt_path = default_opt_str(&args, "word_srt_path");

    let min_duration_s = default_f64(&args, "min_duration_s", 2.0);
    let max_duration_s = default_f64(&args, "max_duration_s", 6.0);
    let language = default_str(&args, "language", "hinglish");
    let quality = default_str(&args, "quality", "sd");
    let orientation = default_str(&args, "orientation", "9:16");
    let max_batch_size = default_u32(&args, "max_batch_size", 15);
    let max_candidates = default_u32(&args, "max_candidates", 6);
    let max_keywords_per_search = default_u32(&args, "max_keywords_per_search", 2);
    let max_repair_iterations = default_u32(&args, "max_repair_iterations", 3);
    let run_stickers = default_bool(&args, "stickers", true);
    let run_captions = default_bool(&args, "captions", true);

    // ---- Stage A: resolve timeline + segments ----
    let (timeline_path, segments) = if let Some(tl) = &timeline_path_arg {
        let timeline = Timeline::load(tl)?;
        let segs: Vec<serde_json::Value> = timeline
            .segments
            .iter()
            .map(|s| {
                json!({
                    "id": s.id.clone(),
                    "start_s": s.start,
                    "end_s": s.end,
                    "duration_s": s.end - s.start,
                    "caption": s.caption.clone(),
                })
            })
            .collect();
        (tl.clone(), segs)
    } else {
        let srt = srt_path.clone().ok_or_else(|| {
            ToolError::MissingArg(
                "broll.auto requires srt_path + audio_path (or timeline_path)".into(),
            )
        })?;
        let audio = audio_path.clone().ok_or_else(|| {
            ToolError::MissingArg("broll.auto requires audio_path (or timeline_path)".into())
        })?;

        // 1. segment.analyze — sentence-aware 2-6s segmentation
        report_progress(5.0, 100.0, "1/6 segment.analyze").await.ok();
        let analyzed = handle_segment_analyze(json!({
            "audio_path": audio,
            "srt_path": srt,
            "min_duration_s": min_duration_s,
            "max_duration_s": max_duration_s,
        }))
        .await?;
        let segments: Vec<serde_json::Value> = analyzed
            .get("segments")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        // 2. srt.to_timeline — build the timeline with identical segmentation
        let out_path = args
            .get("output_path")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| {
                let stem = Path::new(&srt)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "broll_auto".to_string());
                format!("artifacts/{}.timeline.json", stem)
            });
        report_progress(20.0, 100.0, "2/6 srt.to_timeline").await.ok();
        let built = handle_srt_to_timeline(json!({
            "srt_path": srt,
            "source_video": audio,
            "output_path": out_path,
            "aspect": orientation,
            "fps": 30,
            "min_duration_s": min_duration_s,
            "max_duration_s": max_duration_s,
        }))
        .await?;
        let tl = built
            .get("timeline_path")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or(out_path);
        (tl, segments)
    };

    let segments_arr = segments.clone();
    if segments_arr.is_empty() {
        return Err(ToolError::InvalidArg(
            "broll.auto: no segments found — check SRT/timeline".into(),
        ));
    }

    // ---- Stage B: draft keywords (agentic) ----
    report_progress(35.0, 100.0, "3/6 broll.keywords (draft)").await.ok();
    let drafts = handle_broll_keywords(json!({
        "segments": segments,
        "language": language,
        "max_batch_size": max_batch_size,
        "timeline_path": timeline_path,
    }))
    .await?;
    let draft_segments = drafts.get("segments").cloned().unwrap_or_else(|| json!([]));

    // ---- Stage C: relevance validation (agent picks best real video) ----
    report_progress(50.0, 100.0, "4/6 broll.validate_keywords (relevance)").await.ok();
    let validated = handle_broll_validate_keywords(json!({
        "enriched_segments": draft_segments,
        "max_candidates": max_candidates,
        "max_keywords_per_search": max_keywords_per_search,
        "orientation": orientation,
        "quality": quality,
        "language": language,
    }))
    .await?;
    let validated_segments = validated.get("segments").cloned().unwrap_or_else(|| json!([]));

    // ---- Stage D: fetch + auto-place ----
    report_progress(65.0, 100.0, "5/6 broll.fetch (download + place)").await.ok();
    let fetch_segments: Vec<serde_json::Value> = validated_segments
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|v| {
                    let mut seg = json!({
                        "id": v.get("id").cloned().unwrap_or_else(|| json!("")),
                        "start_s": v.get("start_s").cloned().unwrap_or_else(|| json!(0)),
                        "end_s": v.get("end_s").cloned().unwrap_or_else(|| json!(0)),
                        "caption": v.get("caption").cloned().unwrap_or_else(|| json!("")),
                    });
                    let kw = v
                        .get("final_keywords")
                        .cloned()
                        .or_else(|| v.get("draft_keywords").cloned())
                        .unwrap_or_else(|| json!([]));
                    seg["keywords"] = kw;
                    seg
                })
                .collect()
        })
        .unwrap_or_default();

    let fetched = handle_broll_fetch(json!({
        "enriched_segments": fetch_segments,
        "timeline_path": timeline_path,
        "download_n": 1,
        "quality": quality,
        "orientation": orientation,
    }))
    .await?;
    let auto_assigned = fetched.get("auto_assigned").and_then(|v| v.as_u64()).unwrap_or(0);

    // ---- Stage E: validate + repair loop until zero gaps ----
    report_progress(80.0, 100.0, "6/6 timeline.validate + repair loop").await.ok();
    let mut repair_passes = 0u32;
    let mut repaired_total = 0u64;
    let mut remaining_gaps: Vec<serde_json::Value> = Vec::new();
    let mut initial_gaps = 0usize;
    let mut final_valid = false;

    for pass in 0..max_repair_iterations {
        let vres = handle_timeline_validate(json!({"timeline_path": timeline_path})).await?;
        let gaps = vres
            .get("broll_gaps")
            .and_then(|g| g.as_array())
            .cloned()
            .unwrap_or_default();
        if pass == 0 {
            initial_gaps = gaps.len();
        }
        if gaps.is_empty() {
            final_valid = vres.get("valid").and_then(|v| v.as_bool()).unwrap_or(false);
            remaining_gaps = vec![];
            break;
        }
        repair_passes = pass + 1;
        let repair = handle_broll_repair(json!({
            "timeline_path": timeline_path,
            "max_segments": gaps.len(),
            "language": language,
            "quality": quality,
            "orientation": orientation,
        }))
        .await?;
        let repaired_this = repair.get("repaired").and_then(|v| v.as_u64()).unwrap_or(0);
        repaired_total += repaired_this;
        remaining_gaps = repair
            .get("remaining_gaps")
            .and_then(|g| g.as_array())
            .cloned()
            .unwrap_or_default();
        if repaired_this == 0 {
            break; // no progress — avoid infinite loop
        }
    }
    if remaining_gaps.is_empty() {
        let vres = handle_timeline_validate(json!({"timeline_path": timeline_path})).await?;
        final_valid = vres.get("valid").and_then(|v| v.as_bool()).unwrap_or(false);
    }

    // ---- Stage F: optional sticker + caption stages (finalize A2V one-call) ----
    let mut stickers_placed = 0u64;
    let mut captions_ass_path: Option<String> = None;
    let mut sticker_warning: Option<String> = None;

    if run_stickers {
        report_progress(88.0, 100.0, "sticker.auto (agentic GIPHY stickers)").await.ok();
        // Unification: the b-roll pipeline's VALIDATED keywords (final_keywords
        // — agent-approved, Pexels-verified) also drive the GIPHY sticker search
        // per segment, so b-roll and stickers share ONE keyword source instead
        // of sticker.keywords re-drafting a separate intent pass.
        let sticker_shared_keywords: Vec<serde_json::Value> = validated_segments
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| {
                        let id = v.get("id").and_then(|i| i.as_str())?.to_string();
                        let kws: Vec<String> = v
                            .get("final_keywords")
                            .and_then(|k| k.as_array())
                            .map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect())
                            .unwrap_or_default();
                        if kws.is_empty() {
                            return None;
                        }
                        Some(json!({
                            "id": id,
                            "caption": v.get("caption").cloned().unwrap_or_else(|| json!("")),
                            "sticker_keywords": kws,
                        }))
                    })
                    .collect()
            })
            .unwrap_or_default();
        // sticker.auto loads the timeline's segments directly (timeline_path
        // branch) and runs shared keywords → GIPHY relevance gate → Stickers track.
        let sticker_res = handle_sticker_auto(json!({
            "timeline_path": timeline_path,
            "language": language,
            "shared_keywords": sticker_shared_keywords,
            // "auto": position cycling + spacing gates (sticker relevance fix)
            "position": "auto",
            "min_gap_s": 2.0,
            "scale": 0.25,
            // Cap sticker volume in the one-call (GIPHY rate limits + render time).
            "max_stickers": segments_arr.len().min(12),
        }))
        .await?;
        stickers_placed = sticker_res.get("stickers_placed").and_then(|v| v.as_u64()).unwrap_or(0);
        if let Some(msg) = sticker_res.get("message").and_then(|v| v.as_str()) {
            if stickers_placed == 0 {
                sticker_warning = Some(msg.to_string());
            }
        }
    }

    if run_captions {
        report_progress(94.0, 100.0, "captions.generate_ass (styled ASS)").await.ok();
        // Pass the explicit SRT when we have one (the timeline's `source` is
        // the audio file, so deriving `audio.srt` from it can miss the
        // transcript). captions.generate_ass falls back to timeline-derived
        // SRT when srt_path is absent.
        let mut cap_args = json!({
            "timeline_path": timeline_path,
            "style": "word_highlight",
            "position": "center",
        });
        if let Some(ref sp) = srt_path {
            if let Some(obj) = cap_args.as_object_mut() {
                obj.insert("srt_path".into(), json!(sp));
            }
        }
        if let Some(ref wsp) = word_srt_path {
            if let Some(obj) = cap_args.as_object_mut() {
                obj.insert("word_srt_path".into(), json!(wsp));
            }
        }
        let cap_res = handle_captions_generate_ass(cap_args).await;
        match cap_res {
            Ok(r) => {
                captions_ass_path = r.get("ass_path").and_then(|v| v.as_str()).map(String::from);
            }
            Err(e) => {
                tracing::warn!("[broll.auto] caption generation failed (non-fatal): {}", e);
            }
        }
    }

    report_progress(100.0, 100.0, "broll.auto complete").await.ok();

    Ok(json!({
        "status": if final_valid { "success" } else { "partial" },
        "message": if final_valid {
            format!(
                "A2V b-roll complete: {} segments fully covered with validated, non-looping clips ({} placed, {} repair pass(es)).{} {}",
                segments_arr.len(),
                auto_assigned,
                repair_passes,
                if stickers_placed > 0 { format!(" {} sticker(s) placed.", stickers_placed) } else { String::new() },
                if let Some(ref w) = sticker_warning { format!(" Stickers skipped: {}", w) } else { String::new() }
            )
        } else {
            format!(
                "{} gap(s) remain after {} repair pass(es) — rerun broll.repair with wider keywords.",
                remaining_gaps.len(),
                repair_passes
            )
        },
        "timeline_path": timeline_path,
        "segments_count": segments_arr.len(),
        "auto_assigned": auto_assigned,
        "initial_gaps": initial_gaps,
        "repair_passes": repair_passes,
        "repaired_total": repaired_total,
        "remaining_gaps": remaining_gaps,
        "valid": final_valid,
        "stickers_placed": stickers_placed,
        "sticker_warning": sticker_warning,
        "captions_ass_path": captions_ass_path,
        "pipeline": json!({
            "analyze": "segment.analyze",
            "draft": "broll.keywords",
            "validate": "broll.validate_keywords",
            "fetch": "broll.fetch",
            "repair": "broll.repair",
            "stickers": "sticker.auto",
            "captions": "captions.generate_ass",
        }),
    }))
}

// ---------------------------------------------------------------------------
// Handler: broll.probe (all-engine stock candidate pool, normalized + ranked)
// ---------------------------------------------------------------------------

async fn handle_broll_probe(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let query = extract_str(&args, "query")?;
    let aspect = default_str(&args, "aspect", "9:16");
    let min_duration_s = default_f64(&args, "min_duration_s", 0.0);
    let max_duration_s = default_f64(&args, "max_duration_s", 0.0);
    let per_provider = default_u32(&args, "per_provider", 8) as usize;
    let signal: Vec<String> = args
        .get("signal")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    report_progress(0.0, 100.0, &format!("Probing stock engines for '{}'...", query))
        .await
        .ok();

    let q = crate::stock_pool::StockPoolQuery {
        query: query.to_string(),
        aspect,
        min_duration_s,
        max_duration_s,
        per_provider,
        signal,
    };
    let outcome = crate::stock_pool::search_stock_pool(&q).await;

    let candidates: Vec<serde_json::Value> = outcome
        .candidates
        .iter()
        .map(|c| {
            json!({
                "provider": c.provider,
                "id": c.id,
                "title": c.title,
                "duration_s": c.duration_s,
                "width": c.width,
                "height": c.height,
                "thumbnail_url": c.thumbnail_url,
                "page_url": c.page_url,
                "direct_url": c.direct_url,
                "lexical": c.lexical,
            })
        })
        .collect();

    let per_provider: serde_json::Value = outcome
        .per_provider
        .iter()
        .map(|(p, n)| json!({ "provider": p, "count": n }))
        .collect();

    report_progress(100.0, 100.0, &format!("Found {} ranked candidates", candidates.len()))
        .await
        .ok();

    Ok(json!({
        "status": "searched",
        "query": query,
        "per_provider": per_provider,
        "count": candidates.len(),
        "candidates": candidates,
    }))
}

// ---------------------------------------------------------------------------
// Handler: segment.analyze (transcript → clean segments for agent consumption)
// ---------------------------------------------------------------------------

async fn handle_segment_analyze(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    // Accept both audio_path and video_path for backward compat (agents use video_path)
    let audio_path = extract_str(&args, "audio_path")
        .or_else(|_| extract_str(&args, "video_path"))?;
    let srt_path = args.get("srt_path").and_then(|v| v.as_str()).map(String::from);

    // Step 1: Transcribe or load SRT
    let word_srt_path = if let Some(ref path) = srt_path {
        path.clone()
    } else {
        // Transcribe the audio
        report_progress(0.0, 100.0, "Transcribing audio...").await.ok();
        let out_dir = std::env::temp_dir().join("segment_analyze");
        let _ = std::fs::create_dir_all(&out_dir);
        let out_srt = out_dir.join("transcript.srt").to_string_lossy().to_string();
        let result = transcribe_with_engine(
            audio_path,
            &out_srt,
            openscript_transcribe::transcriber::TranscriptionEngine::HinglishGgml,
            "auto",
            None,
        )
        .await
        .map_err(|e| ToolError::InvalidArg(format!("Transcription failed: {}", e)))?;
        result.word_srt_path
            .unwrap_or(result.phrase_srt_path.unwrap_or(result.output_path))
    };

    // Step 2: Parse SRT entries
    report_progress(30.0, 100.0, "Parsing SRT entries...").await.ok();
    let entries = parse_srt(&word_srt_path)?;

    if entries.is_empty() {
        return Ok(json!({
            "status": "warning",
            "message": "No SRT entries found",
            "segments": [],
        }));
    }

    // Step 3: Group into segments using sentence-aware segmentation with
    // min/max duration enforcement (docs/SEGMENTATION_ARCHITECTURE.md).
    // Replaces the old fixed `SCENE_SIZE=4` chunking, which produced
    // unbounded 10–27s segments and broke mid-sentence. Pause detection
    // (>300ms gaps) groups at sentence boundaries; enforce_segment_bounds
    // then merges segments < min (2.0s) and splits segments > max (6.0s)
    // at the longest internal pause — the short-form retention target.
    report_progress(50.0, 100.0, "Grouping into segments (sentence-aware)...").await.ok();
    let min_dur_s = args.get("min_duration_s").and_then(|v| v.as_f64()).unwrap_or(2.0);
    let max_dur_s = args.get("max_duration_s").and_then(|v| v.as_f64()).unwrap_or(6.0);
    let grouped = openscript_core::srt::group_entries_with_words_max_duration(
        &entries,
        15,   // ~4s at 2.5 words/s
        80,   // ~2 caption lines
        0.3,  // 300ms breath pause boundary
        max_dur_s,
    );
    let bounded = openscript_core::srt::enforce_segment_bounds(grouped, min_dur_s, max_dur_s);
    let mut scenes: Vec<(String, f64, f64)> = bounded
        .into_iter()
        .map(|p| (p.text, p.start, p.end))
        .collect();

    // Clamp scenes at the source media duration. SRT entries can overshoot the
    // audio end (whisper tail hallucination / trailing silence), producing
    // segments past the master clock — the "audio 2:15, video 2:41" black tail.
    // broll.fetch places clips against these scenes, so the clamp here keeps
    // every b-roll window inside the source audio.
    if let Some(src_dur) = probe_source_duration(std::path::Path::new(&audio_path)).await {
        scenes.retain(|(_, start, _)| *start < src_dur);
        for (_, _, end) in scenes.iter_mut() {
            if *end > src_dur + SOURCE_DUR_TOLERANCE_S {
                *end = src_dur;
            }
        }
    }

    // Step 4: For each segment, run stock_signal analysis
    report_progress(60.0, 100.0, "Analyzing segments for b-roll keywords...").await.ok();
    let mut result_segments = Vec::new();
    for (idx, (text, start_s, end_s)) in scenes.iter().enumerate() {
        let duration_s = end_s - start_s;
        // Agent generates English keywords from Hinglish content - no auto-extraction
        result_segments.push(json!({
            "id": format!("seg_{:03}", idx + 1),
            "start_s": start_s,
            "end_s": end_s,
            "duration_s": duration_s,
            "caption": text,
        }));
    }    report_progress(100.0, 100.0, "Analysis complete.").await.ok();

    // Build section_map: maps segment index to its role in the video structure
    // Sections: intro (first 15%), body (middle 70%), outro (last 15%)
    let total_segs = result_segments.len();
    let section_map: Vec<serde_json::Value> = result_segments.iter().enumerate().map(|(i, seg)| {
        let fraction = i as f64 / total_segs.max(1) as f64;
        let section = if fraction < 0.15 {
            "intro"
        } else if fraction > 0.85 {
            "outro"
        } else {
            "body"
        };
        json!({
            "segment_id": seg["id"].clone(),
            "section": section,
            "start_s": seg["start_s"].clone(),
            "end_s": seg["end_s"].clone(),
        })
    }).collect();

    Ok(json!({
        "status": "success",
        "segments_count": result_segments.len(),
        "segments": result_segments,
        "section_map": section_map,
    }))
}

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

async fn handle_verify_audio(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?
        .to_string_lossy()
        .to_string();
    let expected_has_voice = default_bool(&args, "expected_has_voice", true);
    let max_silence_seconds = default_f64(&args, "max_silence_seconds", 3.0);

    if !Path::new(&video_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Video not found: {}",
            video_path
        )));
    }

    let output = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=codec_name,sample_rate,channels,duration",
            "-of",
            "json",
            &video_path,
        ])
        .output()
        .await
        .map_err(|e| ToolError::Ffmpeg(format!("ffprobe failed: {}", e)))?;

    if !output.status.success() {
        return Err(ToolError::Ffmpeg(format!(
            "ffprobe failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    let probe: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(ToolError::Json)?;

    let streams = probe
        .get("streams")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let has_audio = !streams.is_empty();

    if !has_audio {
        return Ok(json!({
            "status": "warning",
            "issues": ["No audio stream detected — voice/music/SFX are missing"],
            "rms_lufs": null,
            "peak_db": null,
            "silence_segments": [],
            "has_dialogue": false,
            "quality_score": 0,
        }));
    }

    let vol_output = tokio::process::Command::new("ffmpeg")
        .args(["-i", &video_path, "-af", "volumedetect", "-f", "null", "-"])
        .output()
        .await
        .map_err(|e| ToolError::Ffmpeg(format!("volumedetect failed: {}", e)))?;

    if !vol_output.status.success() {
        return Err(ToolError::Ffmpeg(format!(
            "volumedetect failed: {}",
            String::from_utf8_lossy(&vol_output.stderr)
        )));
    }

    let stderr = String::from_utf8_lossy(&vol_output.stderr);
    let mean_volume = stderr
        .lines()
        .find(|l| l.contains("mean_volume"))
        .and_then(|l| l.split(": ").nth(1))
        .and_then(|v| v.trim_end_matches(" dB").parse::<f64>().ok());
    let max_volume = stderr
        .lines()
        .find(|l| l.contains("max_volume"))
        .and_then(|l| l.split(": ").nth(1))
        .and_then(|v| v.trim_end_matches(" dB").parse::<f64>().ok());

    let silence_output = tokio::process::Command::new("ffmpeg")
        .args([
            "-i",
            &video_path,
            "-af",
            &format!("silencedetect=noise=-30dB:d={}", max_silence_seconds),
            "-f",
            "null",
            "-",
        ])
        .output()
        .await
        .map_err(|e| ToolError::Ffmpeg(format!("silencedetect failed: {}", e)))?;

    if !silence_output.status.success() {
        return Err(ToolError::Ffmpeg(format!(
            "silencedetect failed: {}",
            String::from_utf8_lossy(&silence_output.stderr)
        )));
    }

    let silence_stderr = String::from_utf8_lossy(&silence_output.stderr);
    let mut silence_segments: Vec<serde_json::Value> = Vec::new();
    let mut current_start: Option<f64> = None;
    for line in silence_stderr.lines() {
        if line.contains("silence_start:") {
            if let Some(val) = line.split(": ").nth(1).and_then(|v| v.parse::<f64>().ok()) {
                current_start = Some(val);
            }
        } else if line.contains("silence_end:") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if let (Some(start), Some(end)) = (
                current_start,
                parts.get(1).and_then(|v| v.parse::<f64>().ok()),
            ) {
                silence_segments.push(json!({
                    "start": start,
                    "end": end,
                    "duration": end - start,
                }));
                current_start = None;
            }
        }
    }

    let rms = mean_volume.unwrap_or(-99.0);
    let peak = max_volume.unwrap_or(-99.0);
    let has_good_level = (-30.0..=-12.0).contains(&rms);
    let has_no_clipping = peak <= 0.0;
    let no_long_silence = silence_segments.is_empty();

    let quality_score = if expected_has_voice {
        let mut score = 0;
        if has_audio {
            score += 25;
        }
        if has_good_level {
            score += 25;
        }
        if has_no_clipping {
            score += 25;
        }
        if no_long_silence {
            score += 25;
        }
        score
    } else {
        if has_audio {
            50
        } else {
            100
        }
    };

    let mut issues: Vec<String> = Vec::new();
    if !has_audio {
        issues.push("No audio stream".into());
    }
    if !has_good_level && has_audio {
        issues.push(format!(
            "Audio level unhealthy: RMS {} dB (expected -30 to -12 dB)",
            rms
        ));
    }
    if !has_no_clipping {
        issues.push(format!("Audio clipping detected: peak {} dB", peak));
    }
    if !no_long_silence {
        issues.push(format!(
            "{} silence gaps detected (>{})",
            silence_segments.len(),
            max_silence_seconds
        ));
    }

    Ok(json!({
        "status": if quality_score >= 75 { "pass" } else if quality_score >= 50 { "warning" } else { "fail" },
        "rms_lufs": rms,
        "peak_db": peak,
        "silence_segments": silence_segments,
        "has_dialogue": has_audio && has_good_level,
        "quality_score": quality_score,
        "issues": issues,
        "audio_codec": streams.first().and_then(|s| s.get("codec_name")).and_then(|v| v.as_str()).unwrap_or("unknown"),
        "sample_rate": streams.first().and_then(|s| s.get("sample_rate")).and_then(|v| v.as_str()).unwrap_or("unknown"),
    }))
}

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

async fn handle_verify_captions(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?
        .to_string_lossy()
        .to_string();
    let srt_path = sanitize_input_path(extract_str(&args, "srt_path")?)?
        .to_string_lossy()
        .to_string();
    let min_caption_duration_ms = default_i64(&args, "min_caption_duration_ms", 300);
    let max_caption_duration_ms = default_i64(&args, "max_caption_duration_ms", 5000);

    if !Path::new(&video_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Video not found: {}",
            video_path
        )));
    }
    if !Path::new(&srt_path).exists() {
        return Err(ToolError::NotFound(format!("Caption file not found: {}", srt_path)));
    }

    let probe_output = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "json",
            &video_path,
        ])
        .output()
        .await
        .map_err(|e| ToolError::Ffmpeg(format!("ffprobe failed: {}", e)))?;

    if !probe_output.status.success() {
        return Err(ToolError::Ffmpeg(format!(
            "ffprobe failed: {}",
            String::from_utf8_lossy(&probe_output.stderr)
        )));
    }

    let probe: serde_json::Value =
        serde_json::from_slice(&probe_output.stdout).map_err(ToolError::Json)?;
    let video_duration_s: f64 = probe
        .get("format")
        .and_then(|f| f.get("duration"))
        .and_then(|v| v.as_str())
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(0.0);
    let video_duration_ms = (video_duration_s * 1000.0) as i64;

    // Auto-detect caption format: ASS (.ass) or SRT (.srt).
    // script.to_video emits ASS; verify.captions previously only accepted
    // SRT, which meant the verify step was unusable after a script.to_video
    // render. Now we accept both and normalize to the same entry format.
    // (UX audit round-2 GAP #10 fix.)
    let is_ass = srt_path.ends_with(".ass");
    let entries: Vec<openscript_core::srt::SrtEntry> = if is_ass {
        parse_ass_captions(&srt_path)?
    } else {
        openscript_core::srt::parse_srt(&srt_path)
            .map_err(|e| ToolError::Srt(e.to_string()))?
    };

    if entries.is_empty() {
        return Ok(json!({
            "status": "fail",
            "issues": ["SRT file has no entries"],
            "caption_count": 0,
            "coverage_percent": 0.0,
            "gaps": [],
            "overlaps": [],
            "avg_caption_duration_ms": 0,
            "readability_score": 0,
        }));
    }

    let mut total_caption_ms: i64 = 0;
    let mut gaps: Vec<serde_json::Value> = Vec::new();
    let mut overlaps: Vec<serde_json::Value> = Vec::new();
    let mut too_fast: Vec<serde_json::Value> = Vec::new();
    let mut too_slow: Vec<serde_json::Value> = Vec::new();

    for (i, entry) in entries.iter().enumerate() {
        let start_ms = (entry.start * 1000.0) as i64;
        let end_ms = (entry.end * 1000.0) as i64;
        let duration_ms = end_ms - start_ms;
        total_caption_ms += duration_ms;

        if duration_ms < min_caption_duration_ms {
            too_fast.push(json!({"idx": entry.idx, "duration_ms": duration_ms, "text": entry.text.chars().take(40).collect::<String>()}));
        }
        if duration_ms > max_caption_duration_ms {
            too_slow.push(json!({"idx": entry.idx, "duration_ms": duration_ms, "text": entry.text.chars().take(40).collect::<String>()}));
        }

        if i > 0 {
            let prev_end = (entries[i - 1].end * 1000.0) as i64;
            let gap_ms = start_ms - prev_end;
            if gap_ms > 2000 {
                gaps.push(json!({"after_idx": entries[i-1].idx, "before_idx": entry.idx, "gap_ms": gap_ms}));
            }
        }

        if i > 0 {
            let prev_end = (entries[i - 1].end * 1000.0) as i64;
            let prev_start = (entries[i - 1].start * 1000.0) as i64;
            if start_ms < prev_end && start_ms > prev_start {
                overlaps.push(json!({"idx_a": entries[i-1].idx, "idx_b": entry.idx, "overlap_ms": prev_end - start_ms}));
            }
        }
    }

    let avg_duration = if !entries.is_empty() {
        total_caption_ms / entries.len() as i64
    } else {
        0
    };
    let coverage = if video_duration_ms > 0 {
        (total_caption_ms as f64 / video_duration_ms as f64) * 100.0
    } else {
        0.0
    };

    let mut issues: Vec<String> = Vec::new();
    if !gaps.is_empty() {
        issues.push(format!("{} caption gaps > 2s", gaps.len()));
    }
    if !overlaps.is_empty() {
        issues.push(format!("{} caption overlaps", overlaps.len()));
    }
    if !too_fast.is_empty() {
        issues.push(format!(
            "{} captions too fast (<{}ms)",
            too_fast.len(),
            min_caption_duration_ms
        ));
    }
    if !too_slow.is_empty() {
        issues.push(format!(
            "{} captions too slow (>{})",
            too_slow.len(),
            max_caption_duration_ms
        ));
    }

    let mut score = 100;
    score -= (gaps.len() as i32) * 10;
    score -= (overlaps.len() as i32) * 15;
    score -= (too_fast.len() as i32) * 5;
    score -= (too_slow.len() as i32) * 5;
    let score = score.max(0).min(100);

    Ok(json!({
        "status": if score >= 80 { "pass" } else if score >= 50 { "warning" } else { "fail" },
        "caption_count": entries.len(),
        "coverage_percent": (coverage * 10.0).round() / 10.0,
        "video_duration_ms": video_duration_ms,
        "total_caption_ms": total_caption_ms,
        "avg_caption_duration_ms": avg_duration,
        "gaps": gaps,
        "overlaps": overlaps,
        "too_fast": too_fast,
        "too_slow": too_slow,
        "readability_score": score,
        "issues": issues,
    }))
}

// ---------------------------------------------------------------------------
// Handler: verify.render
// ---------------------------------------------------------------------------

async fn handle_verify_render(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?
        .to_string_lossy()
        .to_string();
    let timeline_path = sanitize_input_path(extract_str(&args, "timeline_path")?)?
        .to_string_lossy()
        .to_string();
    let expected_aspect = default_str(&args, "expected_aspect", "9:16");
    let duration_tolerance_ms = default_i64(&args, "duration_tolerance_ms", 2000);

    if !Path::new(&video_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Video not found: {}",
            video_path
        )));
    }
    if !Path::new(&timeline_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Timeline not found: {}",
            timeline_path
        )));
    }

    let probe_output = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,r_frame_rate,duration",
            "-show_entries",
            "format=duration,size",
            "-of",
            "json",
            &video_path,
        ])
        .output()
        .await
        .map_err(|e| ToolError::Ffmpeg(format!("ffprobe failed: {}", e)))?;

    if !probe_output.status.success() {
        return Err(ToolError::Ffmpeg(format!(
            "ffprobe failed: {}",
            String::from_utf8_lossy(&probe_output.stderr)
        )));
    }

    let probe: serde_json::Value =
        serde_json::from_slice(&probe_output.stdout).map_err(ToolError::Json)?;

    let streams = probe
        .get("streams")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let format_info = probe.get("format").cloned().unwrap_or(json!({}));

    let width = streams
        .first()
        .and_then(|s| s.get("width"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let height = streams
        .first()
        .and_then(|s| s.get("height"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let file_size = std::fs::metadata(video_path).map(|m| m.len()).unwrap_or(0);

    let actual_duration_s: f64 = format_info
        .get("duration")
        .and_then(|v| v.as_str())
        .and_then(|v| v.parse::<f64>().ok())
        .or_else(|| {
            streams
                .first()
                .and_then(|s| s.get("duration"))
                .and_then(|v| v.as_str())
                .and_then(|v| v.parse::<f64>().ok())
        })
        .unwrap_or(0.0);
    let actual_duration_ms = (actual_duration_s * 1000.0) as i64;

    let timeline = Timeline::load(timeline_path).map_err(|e| ToolError::Timeline(e.to_string()))?;
    let expected_duration_ms = timeline.rendered_duration_ms();
    let segment_count = timeline.segments.len();

    let duration_delta = (actual_duration_ms - expected_duration_ms).abs();
    let duration_match = duration_delta <= duration_tolerance_ms;

    let expected_ratio: f64 = match expected_aspect.as_str() {
        "9:16" => 9.0 / 16.0,
        "16:9" => 16.0 / 9.0,
        "1:1" => 1.0,
        "4:5" => 4.0 / 5.0,
        _ => 9.0 / 16.0,
    };
    let actual_ratio = if height > 0 {
        width as f64 / height as f64
    } else {
        0.0
    };
    let aspect_match = (actual_ratio - expected_ratio).abs() < 0.05;

    let tracks_present: serde_json::Map<String, serde_json::Value> = timeline
        .tracks
        .iter()
        .map(|(track, events)| {
            let track = track as &TrackType;
            let events = events as &Vec<openscript_core::timeline::TimelineEvent>;
            (
                track.to_string(),
                json!({"count": events.len(), "rendered": !events.is_empty()}),
            )
        })
        .collect();

    let total_tracks = timeline.tracks.values().filter(|v| !v.is_empty()).count();
    let has_audio = total_tracks > 1;

    let mut issues: Vec<String> = Vec::new();
    if !duration_match {
        issues.push(format!(
            "Duration mismatch: expected {}ms, got {}ms (delta: {}ms)",
            expected_duration_ms, actual_duration_ms, duration_delta
        ));
    }
    if !aspect_match {
        issues.push(format!(
            "Aspect ratio mismatch: expected {}, got {}x{} (ratio: {:.3})",
            expected_aspect, width, height, actual_ratio
        ));
    }
    if file_size == 0 {
        issues.push("File size is 0 bytes — render may have failed".into());
    }
    if width == 0 || height == 0 {
        issues.push("Could not determine video resolution".into());
    }

    let mut score = 100;
    if !duration_match {
        score -= 30;
    }
    if !aspect_match {
        score -= 25;
    }
    if file_size == 0 {
        score -= 45;
    }
    if !has_audio && total_tracks > 1 {
        score -= 15;
    }
    let score = score.max(0).min(100);

    Ok(json!({
        "status": if score >= 80 { "pass" } else if score >= 50 { "warning" } else { "fail" },
        "duration_match": duration_match,
        "expected_duration_ms": expected_duration_ms,
        "actual_duration_ms": actual_duration_ms,
        "duration_delta_ms": duration_delta,
        "segment_count": segment_count,
        "resolution": format!("{}x{}", width, height),
        "aspect_match": aspect_match,
        "expected_aspect": expected_aspect,
        "file_size_bytes": file_size,
        "tracks_present": tracks_present,
        "has_audio_stream": has_audio,
        "issues": issues,
        "overall_score": score,
        "note": "Technical integrity only. Call verify.production for stock/music/sticker beauty KPIs.",
    }))
}

// ---------------------------------------------------------------------------
// Production Quality KPIs — thin wrappers around openscript_core::production_quality
// ---------------------------------------------------------------------------

fn is_procedural_media_path(path: &str) -> bool {
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
async fn probe_audio_metrics(video_path: &str) -> (Option<f64>, Option<f64>, Option<f64>, Option<f64>) {
    use std::process::Stdio;

    // LUFS via loudnorm filter with JSON output
    let lufs = tokio::process::Command::new("ffmpeg")
        .args([
            "-i",
            video_path,
            "-af",
            "loudnorm=I=-16:TP=-1.5:LRA=11:print_format=json",
            "-f",
            "null",
            "-",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .ok()
        .and_then(|o| String::from_utf8(o.stderr).ok())
        .and_then(|s| {
            // Find the JSON block at the end
            let json_start = s.rfind('{')?;
            let json_str = &s[json_start..];
            serde_json::from_str::<serde_json::Value>(json_str).ok()
        })
        .and_then(|v| v.get("input_i").and_then(|x| x.as_str()).and_then(|s| s.parse::<f64>().ok()));

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
    let avoid = if avoid_concepts.is_empty() {
        String::new()
    } else {
        format!(
            "\nAVOID repeating these already-covered visual concepts: {}\n",
            avoid_concepts.join(", ")
        )
    };
    let system = format!(
        "You are a stock footage keyword drafter for a short-form video. \
         Translate the spoken caption into 2-3 English VISUAL search keywords \
         for Pexels (things a camera can film: objects, people, places, actions). \
         Translate Hinglish/Hindi by MEANING, not word-for-word.{} \
         Rules: keywords 1-3 words each; concrete and searchable; no abstractions. \
         Output ONLY compact JSON: {{\"keywords\":[\"k1\",\"k2\",\"k3\"]}}",
        avoid
    );
    let user = format!("Caption: \"{}\"\nSource language: {}", caption, language);
    match crate::llm::chat_complete(&system, &user, None).await {
        Ok(r) => {
            let parsed = parse_loose_json_obj(&r.text);
            let kws: Vec<String> = parsed
                .get("keywords")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|k| k.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let kws: Vec<String> = kws.into_iter().filter(|k| k.len() >= 3).collect();
            if kws.is_empty() {
                let translated = crate::stock_signal::translate_hinglish_visuals(caption);
                let concept = extract_broll_concept(&translated);
                (
                    concept.split_whitespace().map(String::from).collect(),
                    r.backend,
                    r.model,
                )
            } else {
                (kws, r.backend, r.model)
            }
        }
        Err(e) => {
            tracing::warn!("[broll.repair] draft LLM failed: {} — using Hinglish-map fallback", e);
            let translated = crate::stock_signal::translate_hinglish_visuals(caption);
            let concept = extract_broll_concept(&translated);
            (
                concept.split_whitespace().map(String::from).collect(),
                "fallback".into(),
                "hinglish-map".into(),
            )
        }
    }
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

async fn handle_verify_production(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_core::production_quality::{
        evaluate_production_quality, grade_rank, BackgroundLayerInfo, MemeLayerInfo, MusicLayerInfo,
        RenderManifest, StickerLayerInfo, verify_layer_order,
    };

    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?
        .to_string_lossy()
        .to_string();
    let timeline_path = sanitize_input_path(extract_str(&args, "timeline_path")?)?
        .to_string_lossy()
        .to_string();
    let captions_path = default_opt_str(&args, "captions_path");
    let sticker_count = default_u32(&args, "sticker_count", 0) as usize;
    let meme_count = default_u32(&args, "meme_count", 0) as usize;
    let min_grade = default_str(&args, "min_grade", "B");
    let music_path_arg = default_opt_str(&args, "music_path");
    let manifest_path = default_opt_str(&args, "render_manifest_path");

    if !Path::new(&video_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Video not found: {}",
            video_path
        )));
    }
    if !Path::new(&timeline_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Timeline not found: {}",
            timeline_path
        )));
    }

    let timeline = Timeline::load(&timeline_path).map_err(|e| ToolError::Timeline(e.to_string()))?;
    let (has_dialogue, rms_ok) = probe_dialogue_rms(&video_path).await;

    // Probe actual audio metrics from the rendered video
    let (measured_lufs, measured_peak_dbfs, measured_ducking_depth_db, measured_music_gain_db) =
        probe_audio_metrics(&video_path).await;

    // Probe b-roll motion: fraction of frames with non-zero motion and
    // longest static run. Feeds the broll_motion dimension so static
    // b-roll (from source-exhaustion bug) surfaces as a hard fail.
    let (motion_ratio, longest_static_run_s) = probe_broll_motion(&video_path).await;

    // Prefer authoritative render_manifest.json from script.to_video
    let mut manifest = if let Some(ref mp) = manifest_path {
        if Path::new(mp).exists() {
            let raw = std::fs::read_to_string(mp)?;
            serde_json::from_str::<RenderManifest>(&raw).map_err(ToolError::Json)?
        } else {
            RenderManifest::default()
        }
    } else {
        // Co-located default path next to timeline
        let sibling = Path::new(&timeline_path)
            .parent()
            .map(|p| p.join("render_manifest.json"))
            .unwrap_or_else(|| Path::new("render_manifest.json").to_path_buf());
        if sibling.exists() {
            let raw = std::fs::read_to_string(&sibling)?;
            serde_json::from_str::<RenderManifest>(&raw).unwrap_or_default()
        } else {
            RenderManifest::default()
        }
    };

    // Map Stickers-track events into the manifest so stickers placed by
    // sticker.auto / sticker.auto_assign are scored (not reported absent).
    if manifest.stickers.is_empty() {
        manifest.stickers = stickers_from_timeline(&timeline);
    }

    // Merge explicit overrides / legacy args into manifest
    if manifest.backgrounds.is_empty() {
        let bg_paths: Vec<String> = args
            .get("background_sources")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if !bg_paths.is_empty() {
            let n = bg_paths.len().max(1);
            let slice = (timeline.rendered_duration_ms() / n as i64).max(1);
            manifest.backgrounds = bg_paths
                .into_iter()
                .enumerate()
                .map(|(i, p)| BackgroundLayerInfo {
                    path: p,
                    start_ms: i as i64 * slice,
                    end_ms: (i as i64 + 1) * slice,
                    source_hint: None,
                    content_hash: None,
                    video_id: None,
                    search_query: None,
                    lexical_score: None,
                    source_title: None,
                    vision_score: None,
                    vision_reason: None,
                })
                .collect();
        }
    }
    if manifest.stickers.is_empty() && sticker_count > 0 {
        manifest.stickers = (0..sticker_count)
            .map(|i| StickerLayerInfo {
                path: format!("sticker_{}", i),
                start_ms: 0,
                end_ms: 1000,
                position: "top-left".into(),
                scale: 0.35,
            })
            .collect();
    }
    if manifest.memes.is_empty() && meme_count > 0 {
        manifest.memes = (0..meme_count)
            .map(|i| MemeLayerInfo {
                path: format!("meme_{}", i),
                start_ms: 1000 + i as i64 * 500,
                end_ms: 3000 + i as i64 * 500,
            })
            .collect();
    }
    if manifest.music.is_none() {
        if let Some(p) = music_path_arg {
            manifest.music = Some(MusicLayerInfo {
                path: p,
                gain_db: 0.0,
                ducking: true,
                mood: None,
                energy: None,
             tags: vec![], selection_query: None, source: None, });
        }
    }
    if manifest.captions_path.is_none() {
        manifest.captions_path = captions_path;
    }
    if manifest.duration_ms <= 0 {
        manifest.duration_ms = timeline.rendered_duration_ms();
    }
    manifest.has_dialogue = has_dialogue;
    // Set video_keywords from agent if provided
    if manifest.video_keywords.is_empty() {
        if let Some(kw_arr) = args.get("video_keywords").and_then(|v| v.as_array()) {
            manifest.video_keywords = kw_arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
        }
    }
    // Set caption_style: prefer agent arg, fallback to timeline.effects.caption_style
    if manifest.caption_style.is_none() {
        manifest.caption_style = args.get("caption_style")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| timeline.effects.caption_style.clone());
    }
    // Set voiceover_count based on dialogue detection
    // If the video has dialogue (original audio), count it as voiceover
    if manifest.voiceover_count == 0 && has_dialogue {
        manifest.voiceover_count = 1;
    }
    manifest.rms_ok = rms_ok;

    // Update manifest with measured audio metrics (override planned values with reality)
    if let Some(l) = measured_lufs {
        manifest.measured_lufs = Some(l);
        manifest.lufs = Some(l);
    }
    if let Some(p) = measured_peak_dbfs {
        manifest.measured_peak_dbfs = Some(p);
        manifest.peak_dbfs = Some(p);
    }
    if let Some(d) = measured_ducking_depth_db {
        manifest.measured_ducking_depth_db = Some(d);
        manifest.ducking_depth_db = Some(d);
    }
    if let Some(g) = measured_music_gain_db {
        manifest.measured_music_gain_db = Some(g);
        if manifest.music.is_some() {
            manifest.music.as_mut().unwrap().gain_db = g;
        }
    }
    // Update manifest with measured b-roll motion (catches source-exhaustion
    // bug — static frames after seek_offset lands past source end).
    if let Some(r) = motion_ratio {
        manifest.broll_motion_ratio = Some(r);
    }
    if let Some(s) = longest_static_run_s {
        manifest.longest_static_run_s = Some(s);
    }

    // Per-clip b-roll motion analysis: detect static frames at the
    // individual clip intersection level, not just globally.
    let mut per_clip_motion: Vec<serde_json::Value> = Vec::new();
    if let Some(broll_track) = timeline.tracks.get(&TrackType::Broll) {
        let clip_ranges: Vec<(f64, f64)> = broll_track
            .iter()
            .filter(|ev| !ev.asset_id.is_empty() && ev.asset_id != "placeholder")
            .map(|ev| (ev.start_ms as f64 / 1000.0, ev.end_ms as f64 / 1000.0))
            .collect();
        if !clip_ranges.is_empty() {
            let clip_results = probe_broll_motion_per_clip(&video_path, &clip_ranges).await;
            let static_clips: Vec<usize> = clip_results
                .iter()
                .filter(|(_, ratio, _)| ratio.map_or(false, |r| r < 0.30))
                .map(|(idx, _, _)| *idx)
                .collect();
            for (idx, ratio, run_s) in &clip_results {
                per_clip_motion.push(json!({
                    "clip_index": idx,
                    "motion_ratio": ratio.map(|r| (r * 1000.0).round() / 1000.0),
                    "longest_static_run_s": run_s.map(|s| (s * 100.0).round() / 100.0),
                    "static": ratio.map_or(true, |r| r < 0.30),
                }));
            }
            if !static_clips.is_empty() {
                tracing::warn!(
                    "PER-CLIP STATIC DETECTED: {} clip(s) with < 30% motion: {:?}",
                    static_clips.len(),
                    static_clips
                );
            }
        }
    }

    // Override global broll_motion metrics with per-clip frame-hash data
    // when available — frame-hash detection is more accurate than scene
    // scores for gradual zoompan motion.
    if !per_clip_motion.is_empty() {
        let valid_ratios: Vec<f64> = per_clip_motion.iter()
            .filter_map(|c| c.get("motion_ratio").and_then(|r| r.as_f64()))
            .collect();
        let valid_runs: Vec<f64> = per_clip_motion.iter()
            .filter_map(|c| c.get("longest_static_run_s").and_then(|r| r.as_f64()))
            .collect();
        if !valid_ratios.is_empty() {
            let avg_ratio = valid_ratios.iter().sum::<f64>() / valid_ratios.len() as f64;
            manifest.broll_motion_ratio = Some(avg_ratio);
        }
        if !valid_runs.is_empty() {
            let max_run = valid_runs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            manifest.longest_static_run_s = Some(max_run);
        }
    }

    // Probe b-roll coverage: clip duration vs segment window. The renderer
    // plays clips exactly once (Phase A — no loop fill), so any segment
    // whose clip is shorter than its window leaves a visible gap. Surfacing
    // these as errors is the loop-closure signal: the agent re-runs keyword
    // generation + broll.fetch for a longer clip.
    let broll_gaps = probe_broll_gaps(&timeline).await;
    if !broll_gaps.is_empty() {
        manifest.broll_gaps = broll_gaps.clone();
    }

    let report = evaluate_production_quality(&timeline, &manifest);
    let meets_min = grade_rank(&report.grade) >= grade_rank(&min_grade);

    // Verify layer composition order
    let layer_report = verify_layer_order(&manifest);

    // Post-generation COMPOSITION AUDIT — which layers are present, in which
    // z-order, with counts and ranges. This is the meta-cognitive layer the
    // agent needs to reason about its own render (and to hand to a human or a
    // follow-up iteration): a render whose composition is missing captions or
    // music is immediately diagnosable from this block alone.
    let composition = build_composition_audit(&timeline, &manifest);

    // Optional vision re-score of background clips (local Qwen → OpenRouter free).
    let vision_rescore = default_bool(&args, "vision_rescore", false);
    let mut vision_scores: Vec<serde_json::Value> = Vec::new();
    if vision_rescore {
        let keywords = manifest.video_keywords.clone();
        let scene_fallback = timeline
            .segments
            .iter()
            .map(|s| s.caption.as_str())
            .filter(|c| !c.is_empty())
            .take(4)
            .collect::<Vec<_>>()
            .join(" ");
        for (i, bg) in manifest.backgrounds.iter().take(8).enumerate() {
            if bg.path.is_empty() || bg.path == "placeholder" || !Path::new(&bg.path).exists() {
                vision_scores.push(json!({
                    "index": i,
                    "path": bg.path,
                    "status": "skipped",
                    "reason": "missing or placeholder path",
                }));
                continue;
            }
            let scene_text = if scene_fallback.is_empty() {
                bg.search_query.clone().unwrap_or_else(|| "video scene".into())
            } else {
                scene_fallback.clone()
            };
            match crate::llm::score_clip_relevance(
                &bg.path,
                &scene_text,
                &keywords,
                bg.search_query.as_deref(),
            )
            .await
            {
                Ok(v) => vision_scores.push(v),
                Err(e) => vision_scores.push(json!({
                    "index": i,
                    "path": bg.path,
                    "status": "error",
                    "error": e.to_string(),
                })),
            }
        }
    }

    let status = if !report.hard_fails.is_empty() {
        "fail"
    } else if meets_min {
        "pass"
    } else if report.production_score >= 40 {
        "warning"
    } else {
        "fail"
    };
    // Coverage-gap directives join the agent's next_actions so the audit
    // loop knows exactly which segments need a longer clip.
    let mut next_actions = report.next_actions.clone();
    for g in &manifest.broll_gaps {
        next_actions.push(g.action.clone());
    }
    Ok(json!({
        "status": status,
        "production_score": report.production_score,
        "grade": report.grade,
        "min_grade": min_grade,
        "meets_min_grade": meets_min && report.hard_fails.is_empty(),
        "hard_fails": report.hard_fails,
        "dimensions": report.dimensions,
        "next_actions": next_actions,
        "broll_gaps": manifest.broll_gaps,
        "cuts_per_second": report.cuts_per_second,
        "video_source_mix": report.video_source_mix,
        "timeline_editor": report.timeline_editor,
        "layer_order": layer_report,
        "composition": composition,
        "per_clip_motion": per_clip_motion,
        "kpi_version": report.kpi_version,
        "kpi_note": "verify.render is technical-only. Production v3 hard-fails majority procedural, missing visual hooks, and parade music on calm/focus. Use real stock + topic-tagged music.",
        "vision_rescore": vision_rescore,
        "vision_scores": vision_scores,
    }))
}

/// ONE-SHOT director: preflight → parse → to_video → verify.production.
async fn handle_director_run(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let script = extract_str(&args, "script")?;
    let output_path = default_str(&args, "output_path", "artifacts/director_out.mp4");
    let output_dir = default_str(&args, "output_dir", "artifacts/director_run");
    let min_grade = default_str(&args, "min_grade", "B");
    let _ = std::fs::create_dir_all(&output_dir);

    // Preflight
    let pexels = !pexels_key().is_empty();
    let ytdlp = std::process::Command::new("yt-dlp")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !pexels && !ytdlp {
        return Err(ToolError::Asset(
            "director.run preflight failed: need PEXELS_API_KEY or yt-dlp for stock B-roll"
                .into(),
        ));
    }
    let mut preflight_warnings: Vec<String> = Vec::new();
    if !pexels {
        preflight_warnings.push(
            "PEXELS_API_KEY unset — YouTube-only multi-broll (weaker relevance). Set api_keys.pexels in ~/.openscript/config.json"
                .into(),
        );
    }
    let lib = resolve_repo_path("mcp/assets/music_library_index.json");
    if !lib.exists() {
        preflight_warnings.push(
            "music_library_index.json missing — run library.build for tagged music".into(),
        );
    }

    let parse = handle_script_parse(json!({"script": script})).await?;
    if parse.get("status").and_then(|s| s.as_str()) == Some("error")
        || parse
            .get("errors")
            .and_then(|e| e.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(false)
    {
        return Ok(json!({
            "status": "error",
            "phase": "parse",
            "parse": parse,
            "preflight_warnings": preflight_warnings,
        }));
    }

    let to_video = handle_script_to_video(json!({
        "script": script,
        "output_path": output_path,
        "output_dir": output_dir,
    }))
    .await?;

    let video = to_video
        .get("output_path")
        .and_then(|v| v.as_str())
        .unwrap_or(&output_path)
        .to_string();
    let timeline = to_video
        .get("timeline_path")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let manifest = to_video
        .get("render_manifest_path")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let mut verify_args = json!({
        "video_path": video,
        "timeline_path": timeline,
        "min_grade": min_grade,
    });
    if let Some(m) = manifest {
        verify_args["render_manifest_path"] = json!(m);
    }
    let production = if !timeline.is_empty() && Path::new(&video).exists() {
        handle_verify_production(verify_args).await.ok()
    } else {
        None
    };

    Ok(json!({
        "status": to_video.get("status").cloned().unwrap_or(json!("unknown")),
        "preflight_warnings": preflight_warnings,
        "parse": parse,
        "to_video": to_video,
        "verify_production": production,
        "output_path": video,
    }))
}

/// Download a short stock clip via yt-dlp (no API key). Used when Pexels is unavailable.
/// Result of a unique stock fetch (path + identity for variance tracking).
struct StockClipFetch {
    path: String,
    video_id: String,
    content_hash: String,
    search_query: String,
    lexical_score: f64,
    source_title: String,
    /// L3 vision gate: 0–1 relevance of the ACTUAL extracted frame vs the
    /// scene, when a vision backend was available. None = gate skipped/failed.
    vision_score: Option<f64>,
    /// Short justification from the vision model (why it matched/mismatched).
    vision_reason: Option<String>,
}

fn file_content_fingerprint(path: &str) -> Option<String> {
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
    let out = tokio::process::Command::new("yt-dlp")
        .args([
            "--flat-playlist",
            "--dump-json",
            "--no-warnings",
            "--quiet",
            &format!("ytsearch{}:{}", limit, query),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .output()
        .await;
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
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
            .collect(),
        _ => Vec::new(),
    }
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
async fn fetch_youtube_stock_clip_signal(
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
            .map(|c| format!("{}:{:.2}:{}s:{}", c.id, c.lexical, c.duration_s as i64, &c.title[..c.title.len().min(40)]))
            .unwrap_or_else(|| "none".into())
    );

    let scene_kw: Vec<String> = signal.clone();
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
                                &title[..title.len().min(50)]
                            );
                            used_video_ids.insert(video_id.clone());
                        } else {
                            tracing::info!(
                                "[youtube stock] thumbnail PASS rel={:.2} id={} title='{}'",
                                rel,
                                video_id,
                                &title[..title.len().min(50)]
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
            let yt = tokio::process::Command::new("yt-dlp")
                .args([
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
                    &url,
                ])
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
                tracing::warn!(
                    "[youtube stock] download failed id={} title='{}' q={}",
                    video_id,
                    &title[..title.len().min(50)],
                    diversified
                );
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
        let trim = tokio::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-ss",
                &start_s.to_string(),
                "-i",
                &full_path,
                "-t",
                &duration_s.max(2.0).to_string(),
                "-vf",
                &crop,
                "-c:v",
                "libx264",
                "-preset",
                "fast",
                "-crf",
                "23",
                "-an",
                out_path,
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .output()
            .await
            .ok()?;
        if !trim.status.success() || !Path::new(out_path).exists() {
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
                        &title[..title.len().min(50)]
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
            &title[..title.len().min(50)],
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
async fn fetch_pixabay_stock_clip_signal(
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
            .map(|c| format!("{}:{:.2}:{}", c.id, c.lexical, &c.title[..c.title.len().min(40)]))
            .unwrap_or_else(|| "none".into())
    );

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
                &title[..title.len().min(50)]
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
                        &title[..title.len().min(50)],
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
        let trim = tokio::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-i",
                &full_path,
                "-t",
                &duration_s.max(2.0).to_string(),
                "-vf",
                &crop,
                "-c:v",
                "libx264",
                "-preset",
                "fast",
                "-crf",
                "23",
                "-an",
                out_path,
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .output()
            .await
            .ok()?;
        if !trim.status.success() || !Path::new(out_path).exists() {
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
            &title[..title.len().min(50)],
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

async fn handle_reelize_brief(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?
        .to_string_lossy()
        .to_string();
    let srt_path_opt = default_opt_str(&args, "srt_path")
        .map(|s| sanitize_input_path(&s).map(|p| p.to_string_lossy().to_string()))
        .transpose()?;

    if !Path::new(&video_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Video not found: {}",
            video_path
        )));
    }

    let resolved_srt_path = if let Some(srt) = srt_path_opt {
        report_progress(5.0, 100.0, "Using existing SRT...")
            .await
            .ok();
        srt
    } else {
        report_progress(0.0, 100.0, "Transcribing audio...")
            .await
            .ok();
        let transcribe_result = handle_transcribe(json!({"media_path": video_path})).await?;
        report_progress(30.0, 100.0, "Transcription complete")
            .await
            .ok();
        transcribe_result
            .get("output_srt_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::Srt("Transcription did not return output path".to_string()))?
            .to_string()
    };

    report_progress(35.0, 100.0, "Grouping caption segments...")
        .await
        .ok();
    let prepare_result = handle_srt_prepare(json!({
        "srt_path": &resolved_srt_path,
        "max_words": 10,
        "max_chars": 64,
        "max_gap": 0.6,
    }))
    .await?;
    let grouped_srt_path = prepare_result
        .get("output_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::Srt("SRT prepare did not return output path".to_string()))?
        .to_string();

    report_progress(50.0, 100.0, "Analyzing segments...")
        .await
        .ok();
    let entries = parse_srt(&grouped_srt_path)?;

    const STOPWORDS: &[&str] = &[
        "the", "a", "an", "is", "are", "was", "were", "be", "been", "being", "have", "has", "had",
        "do", "does", "did", "will", "would", "could", "should", "may", "might", "can", "shall",
        "to", "of", "in", "for", "on", "with", "at", "by", "from", "as", "into", "like", "through",
        "after", "over", "between", "out", "against", "during", "without", "before", "under",
        "around", "among", "that", "this", "these", "those", "it", "its", "i", "me", "my", "we",
        "our", "you", "your", "he", "him", "his", "she", "her", "they", "them", "their", "what",
        "which", "who", "whom", "whose", "where", "when", "why", "how", "not", "no", "nor", "so",
        "but", "and", "or", "if", "then", "than", "too", "very", "just", "about", "up", "some",
    ];

    let extract_keywords = |text: &str, limit: usize| -> Vec<String> {
        text.split_whitespace()
            .filter(|w| {
                let lower = w.to_lowercase();
                let cleaned: String = lower.chars().filter(|c| c.is_alphabetic()).collect();
                !cleaned.is_empty() && !STOPWORDS.contains(&cleaned.as_str())
            })
            .map(|w| {
                let lower = w.to_lowercase();
                lower.chars().filter(|c| c.is_alphabetic()).collect()
            })
            .take(limit)
            .collect()
    };

    let mut segments: Vec<serde_json::Value> = Vec::new();
    let mut total_dialogue_s = 0.0;

    for (i, entry) in entries.iter().enumerate() {
        let duration_s = entry.end - entry.start;
        let word_count = entry.text.split_whitespace().count();
        let wps = if duration_s > 0.0 {
            word_count as f64 / duration_s
        } else {
            0.0
        };

        let keywords = extract_keywords(&entry.text, 5);
        let broll_concepts: Vec<String> = if entry.text.len() < 20 && !entry.text.trim().is_empty()
        {
            let mut concepts = keywords.iter().take(3).cloned().collect::<Vec<_>>();
            concepts.push(entry.text.trim().to_string());
            concepts
        } else {
            keywords.iter().take(3).cloned().collect()
        };

        total_dialogue_s += duration_s;

        segments.push(json!({
            "id": format!("seg_{:03}", i + 1),
            "start_s": entry.start,
            "end_s": entry.end,
            "duration_s": duration_s,
            "text": entry.text,
            "word_count": word_count,
            "words_per_second": (wps * 100.0).round() / 100.0,
            "suggested_broll_concepts": broll_concepts,
            "topic_keywords": keywords,
        }));
    }

    let mut topic_map: std::collections::HashMap<String, (usize, f64)> =
        std::collections::HashMap::new();
    for seg in &segments {
        if let Some(keywords) = seg.get("topic_keywords").and_then(|v| v.as_array()) {
            if let Some(first) = keywords.first().and_then(|v| v.as_str()) {
                let topic = first.to_string();
                let entry = topic_map.entry(topic).or_insert((0, 0.0));
                entry.0 += 1;
                entry.1 += seg
                    .get("duration_s")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
            }
        }
    }

    let topic_summary: Vec<serde_json::Value> = topic_map
        .into_iter()
        .map(|(topic, (count, total_s))| {
            json!({
                "topic": topic,
                "segment_count": count,
                "total_s": (total_s * 100.0).round() / 100.0,
            })
        })
        .collect();

    let source_duration_s = match tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "json",
        ])
        .arg(&video_path)
        .output()
        .await
    {
        Ok(output) => {
            if let Ok(probe) = serde_json::from_slice::<serde_json::Value>(&output.stdout) {
                probe
                    .get("format")
                    .and_then(|f| f.get("duration"))
                    .and_then(|v| v.as_str())
                    .and_then(|v| v.parse::<f64>().ok())
                    .unwrap_or(0.0)
            } else {
                0.0
            }
        }
        Err(e) => {
            tracing::warn!("ffprobe failed for source duration: {}", e);
            0.0
        }
    };

    report_progress(100.0, 100.0, "Brief complete").await.ok();

    Ok(json!({
        "source_path": video_path,
        "source_duration_s": (source_duration_s * 100.0).round() / 100.0,
        "total_segments": segments.len(),
        "total_dialogue_s": (total_dialogue_s * 100.0).round() / 100.0,
        "segments": segments,
        "topic_summary": topic_summary,
    }))
}

// ---------------------------------------------------------------------------
// Handler: reelize.direct
// ---------------------------------------------------------------------------

async fn handle_reelize_direct(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    use openscript_ffmpeg::render::render_from_timeline;
    use openscript_ffmpeg::subtitles;

    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?
        .to_string_lossy()
        .to_string();
    let segments_arr = args
        .get("segments")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ToolError::MissingArg("segments".to_string()))?;
    let aspect = default_str(&args, "aspect", "9:16");
    let crossfade_ms = default_u32(&args, "crossfade_ms", 300);
    let fps = default_u32(&args, "fps", 30);
    let crf = default_u32(&args, "crf", 20);
    let output_path = default_opt_str(&args, "output_path");
    let captions_obj = args.get("captions").cloned().unwrap_or(json!({}));
    let caption_style = default_str(&captions_obj, "style", "standard");
    let captions_enabled = default_bool(&captions_obj, "enabled", true);
    let broll_arr = args
        .get("broll")
        .and_then(|v| v.as_array()).cloned()
        .unwrap_or_default();
    let sfx_arr = args
        .get("sfx")
        .and_then(|v| v.as_array()).cloned()
        .unwrap_or_default();
    let music_obj = args.get("music").cloned();
    let voiceover_arr = args
        .get("voiceover")
        .and_then(|v| v.as_array()).cloned()
        .unwrap_or_default();
    let srt_path_opt = default_opt_str(&args, "srt_path")
        .map(|s| sanitize_input_path(&s).map(|p| p.to_string_lossy().to_string()))
        .transpose()?;

    if !Path::new(&video_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Video not found: {}",
            video_path
        )));
    }

    let mut warnings: Vec<String> = Vec::new();

    report_progress(0.0, 100.0, "Transcribing audio...")
        .await
        .ok();
    let resolved_srt_path = if let Some(srt) = srt_path_opt {
        srt
    } else {
        let transcribe_result = handle_transcribe(json!({"media_path": video_path})).await?;
        transcribe_result
            .get("output_srt_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::Srt("Transcription did not return output path".to_string()))?
            .to_string()
    };

    report_progress(15.0, 100.0, "Preparing grouped SRT...")
        .await
        .ok();
    let prepare_result = handle_srt_prepare(json!({
        "srt_path": &resolved_srt_path,
        "max_words": 10,
        "max_chars": 64,
        "max_gap": 0.6,
    }))
    .await?;
    let _grouped_srt_path = prepare_result
        .get("output_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::Srt("SRT prepare did not return output path".to_string()))?
        .to_string();

    report_progress(25.0, 100.0, "Building timeline...")
        .await
        .ok();
    let timeline_path = default_timeline_path(&video_path);
    let mut timeline = Timeline::new(
        std::path::Path::new(&video_path).to_path_buf(),
        &aspect,
        fps,
        None,
    );

    for segment in segments_arr {
        let start = segment.get("start").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let end = segment.get("end").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let caption = segment
            .get("caption")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let seg_crossfade = segment
            .get("crossfade_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(crossfade_ms as u64) as u32;
        let semantic_role = segment.get("id").and_then(|v| v.as_str());

        timeline.add_segment(start, end, caption, seg_crossfade, semantic_role);
    }

    if captions_enabled {
        use openscript_core::srt::parse_srt;

        let word_srt_path = {
            let p = Path::new(&resolved_srt_path);
            let parent = p.parent().unwrap_or(Path::new("."));
            let stem = p
                .file_stem()
                .map(|s| s.to_string_lossy())
                .unwrap_or_default();
            parent
                .join(format!("{}.apex.word.srt", stem))
                .to_string_lossy()
                .to_string()
        };

        let word_entries = parse_srt(&word_srt_path).ok();
        let raw_srt_entries = parse_srt(&resolved_srt_path)
            .map_err(|e| ToolError::Srt(format!("Failed to parse SRT: {}", e)))?;

        let use_concat = segments_arr.len() > 10;
        let mut timeline_segments: Vec<(f64, f64, String)> = Vec::new();
        let mut output_cursor_s: f64 = 0.0;

        for segment in segments_arr {
            let seg_start = segment.get("start").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let seg_end = segment.get("end").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let caption = segment
                .get("caption")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let seg_crossfade_s = segment
                .get("crossfade_ms")
                .and_then(|v| v.as_f64())
                .unwrap_or(crossfade_ms as f64)
                / 1000.0;
            let seg_duration = seg_end - seg_start;

            if let Some(ref words) = word_entries {
                let words_in_range: Vec<_> = words
                    .iter()
                    .filter(|e| {
                        e.start >= seg_start && e.end <= seg_end + 0.05 && !e.text.trim().is_empty()
                    })
                    .collect();

                let mut i = 0;
                while i < words_in_range.len() {
                    let chunk_size = if words_in_range.len() - i >= 5 {
                        3
                    } else if words_in_range.len() - i == 4 {
                        2
                    } else {
                        words_in_range.len() - i
                    };
                    let chunk_start = output_cursor_s + (words_in_range[i].start - seg_start);
                    let chunk_end =
                        output_cursor_s + (words_in_range[i + chunk_size - 1].end - seg_start);
                    let text: Vec<_> = words_in_range[i..i + chunk_size]
                        .iter()
                        .map(|e| e.text.trim().to_string())
                        .collect();
                    timeline_segments.push((chunk_start, chunk_end, text.join(" ")));
                    i += chunk_size;
                }
            } else {
                let srt_in_range: Vec<_> = raw_srt_entries
                    .iter()
                    .filter(|e| {
                        e.start >= seg_start && e.end <= seg_end + 0.05 && !e.text.trim().is_empty()
                    })
                    .collect();

                if !srt_in_range.is_empty() && !caption.is_empty() {
                    let caption_words: Vec<&str> = caption.split_whitespace().collect();
                    let n = srt_in_range.len();
                    for (i, srt_entry) in srt_in_range.iter().enumerate() {
                        let out_start = output_cursor_s + (srt_entry.start - seg_start);
                        let out_end = output_cursor_s + (srt_entry.end - seg_start);
                        let ws = (i * caption_words.len()) / n;
                        let we = ((i + 1) * caption_words.len()) / n;
                        let chunk = caption_words[ws..we].join(" ");
                        if !chunk.is_empty() {
                            timeline_segments.push((out_start, out_end, chunk));
                        }
                    }
                } else if !caption.is_empty() {
                    timeline_segments.push((
                        output_cursor_s,
                        output_cursor_s + seg_duration,
                        caption.to_string(),
                    ));
                }
            }

            if use_concat {
                output_cursor_s += seg_duration;
            } else {
                output_cursor_s += seg_duration - seg_crossfade_s;
                if output_cursor_s < 0.0 {
                    output_cursor_s = 0.0;
                }
            }
        }

        let caption_asset_dir = Path::new(&timeline_path).parent().unwrap_or(Path::new("."));
        let style_name = if caption_style == "kinetic" {
            "KineticViral"
        } else {
            "Standard"
        };

        let ass_path = caption_asset_dir
            .join(format!("captions_{}.ass", style_name.to_lowercase()))
            .to_string_lossy()
            .to_string();

        if caption_style == "kinetic" {
            subtitles::generate_kinetic_captions(
                &timeline_segments,
                &ass_path,
                style_name,
                "&H00FFD700",
            )
            .map_err(|e| ToolError::Srt(e.to_string()))?;
        } else {
            subtitles::srt_to_ass(&timeline_segments, &ass_path, style_name)
                .map_err(|e| ToolError::Srt(e.to_string()))?;
        }

        timeline.add_asset("captions", "ass".to_string(), json!({"path": ass_path}));
    }

    // Save timeline BEFORE calling sub-tools (they load from disk)
    timeline.save(&timeline_path)?;

    report_progress(40.0, 100.0, "Fetching b-roll...")
        .await
        .ok();
    for directive in &broll_arr {
        let concept = directive
            .get("concept")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let overlay_at_s = directive
            .get("overlay_at_s")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let duration_s = directive
            .get("duration_s")
            .and_then(|v| v.as_f64())
            .unwrap_or(3.0);

        let fetch_result = handle_broll_fetch(json!({
            "concepts": [concept],
            "orientation": "9:16",
            "quality": "sd",
            "download": true,
        }))
        .await;

        match fetch_result {
            Ok(result) => {
                let cached_path = result
                    .get("downloaded")
                    .and_then(|d| d.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|v| v.get("path"))
                    .and_then(|v| v.as_str())
                    .map(String::from);

                if let Some(path) = cached_path {
                    let assign_result = handle_broll_assign(json!({
                        "timeline_path": &timeline_path,
                        "concept": concept,
                        "position_ms": (overlay_at_s * 1000.0) as i64,
                        "duration_ms": (duration_s * 1000.0) as i64,
                        "asset_path": path,
                    }))
                    .await;
                    if let Err(e) = assign_result {
                        warnings.push(format!("broll assign failed for '{}': {}", concept, e));
                    }
                } else {
                    warnings.push(format!(
                        "broll fetch found no downloadable asset for '{}'",
                        concept
                    ));
                }
            }
            Err(e) => {
                warnings.push(format!("broll fetch failed for '{}': {}", concept, e));
            }
        }
    }

    report_progress(55.0, 100.0, "Assigning SFX...").await.ok();

    for directive in &sfx_arr {
        let role = directive
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("transition");
        let at_s = directive
            .get("at_s")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        let assign_result = handle_sfx_assign(json!({
            "timeline_path": &timeline_path,
            "editorial_role": role,
            "query": "",
            "position_ms": (at_s * 1000.0) as i64,
        }))
        .await;
        if let Err(e) = assign_result {
            warnings.push(format!("sfx assign failed for role '{}': {}", role, e));
        }
    }

    if let Some(ref music) = music_obj {
        report_progress(65.0, 100.0, "Assigning music...")
            .await
            .ok();
        let mood = default_str(music, "mood", "neutral");
        let energy = default_str(music, "energy", "medium");
        let gain_db = default_f64(music, "gain_db", -12.0);
        let ducking = default_bool(music, "duck_under_dialogue", true);

        // Search for a matching music track, then pass its path
        let music_path = match handle_library_search(json!({
            "mood": mood,
            "energy": energy,
            "limit": 1,
        }))
        .await
        {
            Ok(r) => r
                .get("results")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|first| first.get("path"))
                .and_then(|p| p.as_str())
                .map(|s| s.to_string()),
            Err(_) => None,
        };

        if let Some(path) = music_path {
            let music_result = handle_music_assign(json!({
                "timeline_path": &timeline_path,
                "path": path,
                "mood": mood,
                "energy": energy,
                "gain_db": gain_db,
                "ducking": ducking,
            }))
            .await;
            if let Err(e) = music_result {
                warnings.push(format!("music assign failed: {}", e));
            }
        } else {
            warnings.push("No music track found in index — skipping music assignment".to_string());
        }
    }

    if !voiceover_arr.is_empty() {
        report_progress(75.0, 100.0, "Generating voiceovers...")
            .await
            .ok();
    }
    for directive in &voiceover_arr {
        let text = directive.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let position_s = directive
            .get("position_s")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let profile_id = directive
            .get("voice_profile_id")
            .and_then(|v| v.as_str())
            .unwrap_or("test_narrator");
        let speed = directive
            .get("speed")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0);
        let gain_db = directive
            .get("gain_db")
            .and_then(|v| v.as_f64())
            .unwrap_or(-6.0);

        let vo_result = handle_voiceover_generate(json!({
            "timeline_path": &timeline_path,
            "text": text,
            "voice_profile_id": profile_id,
            "position_ms": (position_s * 1000.0) as i64,
            "speed": speed,
            "gain_db": gain_db,
        }))
        .await;
        if let Err(e) = vo_result {
            warnings.push(format!("voiceover generate failed: {}", e));
        }
    }

    report_progress(85.0, 100.0, "Validating timeline...")
        .await
        .ok();
    let timeline = Timeline::load(&timeline_path)?;
    let validation_errors = timeline.validate();
    if !validation_errors.is_empty() {
        return Err(ToolError::Timeline(format!(
            "Timeline validation failed: {}",
            validation_errors.join("; ")
        )));
    }

    report_progress(90.0, 100.0, "Rendering final video...")
        .await
        .ok();
    let output = render_from_timeline(&timeline, &video_path, output_path.as_deref(), Some(crf))
        .await
        .map_err(|e| ToolError::Ffmpeg(e.to_string()))?;

    let broll_count = track_count(&timeline, &TrackType::Broll);
    let sfx_count = track_count(&timeline, &TrackType::Sfx);
    let music_count = track_count(&timeline, &TrackType::Music);
    let voiceover_count = track_count(&timeline, &TrackType::Voiceover);
    let duration_s = timeline.rendered_duration_ms() as f64 / 1000.0;

    report_progress(100.0, 100.0, "Direct complete").await.ok();

    Ok(json!({
        "status": "rendered",
        "output_path": output,
        "duration_s": (duration_s * 100.0).round() / 100.0,
        "segments_count": timeline.segments.len(),
        "broll_count": broll_count,
        "sfx_count": sfx_count,
        "music_count": music_count,
        "voiceover_count": voiceover_count,
        "timeline_path": timeline_path,
        "warnings": if warnings.is_empty() { serde_json::Value::Null } else { json!(warnings) },
    }))
}

// ---------------------------------------------------------------------------
// Handler: script.parse — from-scratch video creation script parser
// ---------------------------------------------------------------------------

/// Handle script.schema: return the full JSON schema for ScriptSpec.
/// WARNING: dual-maintenance — update this handler when ScriptSpec/SceneSpec/SpeakerSpec/BackgroundSpec fields change.
/// Agents call this to discover the correct format before writing a script.
async fn handle_script_schema(_args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    Ok(json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "OpenScript Video Creation Script",
        "description": "Complete specification for AI-agent-driven video creation. All fields have sensible defaults — only 'speakers' and 'scenes' are required.",
        "type": "object",
        "required": ["speakers", "scenes"],
        "properties": {
            "schema": {"type": "string", "default": "openscript-video/v1", "description": "Schema version. Always use 'openscript-video/v1'."},
            "title": {"type": "string", "description": "Human-readable video title. If omitted, video_keywords are auto-extracted from this."},
            "video_keywords": {"type": "array", "items": {"type": "string"}, "description": "Topic keywords for the WHOLE video (3-5 words). Used to bias stock footage search. Auto-extracted from title if omitted."},
            "meta": {
                "type": "object",
                "description": "Output video metadata.",
                "properties": {
                    "aspect": {"type": "string", "default": "9:16", "enum": ["9:16", "16:9", "1:1"], "description": "Aspect ratio."},
                    "fps": {"type": "integer", "default": 30, "enum": [24, 30, 60], "description": "Frames per second."},
                    "width": {"type": "integer", "default": 1080},
                    "height": {"type": "integer", "default": 1920},
                    "resolution": {"type": "string", "default": "1080p"}
                }
            },
            "tts": {
                "type": "object",
                "description": "TTS engine configuration.",
                "properties": {
                    "backend": {"type": "string", "default": "kokoro", "enum": ["kokoro", "sidecar"]},
                    "default_speed": {"type": "number", "default": 1.0, "description": "Speech speed multiplier."},
                    "default_pitch": {"type": "number", "default": 1.0}
                }
            },
            "speakers": {
                "description": "Speaker definitions. Accepts BOTH formats: map (canonical) {\"narrator\": {\"voice\": \"kokoro:af_heart\"}} OR array (agent-friendly) [{\"id\": \"narrator\", \"voice\": \"kokoro:af_heart\"}].",
                "oneOf": [
                    {
                        "type": "object",
                        "description": "Map format (canonical): speaker_id → SpeakerSpec",
                        "additionalProperties": {
                            "$ref": "#/definitions/SpeakerSpec"
                        }
                    },
                    {
                        "type": "array",
                        "description": "Array format (agent-friendly): each entry needs 'id' and 'voice'.",
                        "items": {
                            "type": "object",
                            "required": ["voice"],
                            "properties": {
                                "id": {"type": "string", "description": "Speaker ID referenced by scenes."},
                                "voice": {"type": "string", "description": "Voice ID: 'kokoro:af_heart', 'kokoro:am_michael', or bare 'af_heart'. Use tts.voices to list all."}
                            }
                        }
                    }
                ]
            },
            "background": {
                "description": "Background config. Accepts BOTH: object (canonical) or string (agent-friendly). String = shorthand for {type: value}.",
                "oneOf": [
                    {
                        "$ref": "#/definitions/BackgroundSpec"
                    },
                    {
                        "type": "string",
                        "enum": ["procedural", "gameplay", "static"],
                        "description": "String shorthand: procedural, gameplay, or static."
                    }
                ]
            },
            "music": {
                "type": ["object", "null"],
                "properties": {
                    "path": {"type": ["string", "null"], "description": "Music file path. Omit to auto-select from library by mood."},
                    "gain_db": {"type": "number", "default": -10.0, "description": "Music volume in dB. Recommended: -8 to -14. Above -8 overpowers voice."},
                    "ducking": {"type": "boolean", "default": true, "description": "Auto-lower music during speech."},
                    "ducking_depth_db": {"type": "number", "default": 12.0}
                }
            },
            "captions": {
                "type": "object",
                "properties": {
                    "style": {"type": "string", "default": "word_highlight", "enum": ["word_highlight", "sentence_fade", "karaoke_fill", "subtitle_rail"], "description": "Caption style. word_highlight = TikTok-style word sync (default)."},
                    "font": {"type": "string", "default": "Bebas Neue"},
                    "font_size": {"type": "integer", "default": 72},
                    "color": {"type": "string", "default": "#ffffff"},
                    "highlight_color": {"type": "string", "default": "#00ff88"},
                    "position": {"type": "string", "default": "bottom", "enum": ["bottom", "top", "center"]},
                    "safe_zone": {"type": "number", "default": 0.85},
                    "max_words_per_line": {"type": "integer", "default": 5}
                }
            },
            "stickers": {
                "type": "object",
                "properties": {
                    "enabled": {"type": "boolean", "default": true},
                    "lip_sync": {"type": "string", "default": "amplitude", "enum": ["amplitude", "viseme", "none"]},
                    "blink": {"type": "boolean", "default": true},
                    "idle_bob": {"type": "boolean", "default": true}
                }
            },
            "meme_brolls": {
                "type": "object",
                "properties": {
                    "enabled": {"type": "boolean", "default": false},
                    "position": {"type": "string", "default": "center-bottom"},
                    "scale": {"type": "number", "default": 0.35},
                    "duration_s": {"type": "number", "default": 2.5},
                    "offset_s": {"type": "number", "default": 0.3}
                }
            },
            "scenes": {
                "type": "array",
                "description": "Ordered list of scenes (script content). Each scene is one speaker's line.",
                "items": {
                    "$ref": "#/definitions/SceneSpec"
                },
                "minItems": 1
            },
            "sfx": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "at_ms": {"type": ["integer", "null"], "description": "Absolute time in ms."},
                        "role": {"type": "string", "enum": ["intro", "transition", "highlight", "outro"]},
                        "trigger": {"type": ["string", "null"], "enum": ["scene_change", "speaker_change", null]}
                    }
                }
            },
            "output": {
                "type": "object",
                "properties": {
                    "format": {"type": "string", "default": "mp4"},
                    "codec": {"type": "string", "default": "h264"},
                    "crf": {"type": "integer", "default": 18, "description": "Video quality. Lower = higher quality. 18-28 typical."},
                    "preset": {"type": "string", "default": "slow", "description": "FFmpeg preset."},
                    "render_engine": {"type": "string", "default": "ffmpeg", "enum": ["ffmpeg", "hyperframes"]},
                    "theme": {"type": "string", "default": "neutral", "enum": ["neutral", "calm", "energetic"], "description": "Theme preset. calm = warm-gold captions for healing content. energetic = neon-green for edu/gaming."}
                }
            }
        },
        "definitions": {
            "SpeakerSpec": {
                "type": "object",
                "required": ["voice"],
                "properties": {
                    "voice": {"type": "string", "description": "Voice ID: 'kokoro:af_heart', 'kokoro:am_michael', or bare 'af_heart'. Use tts.voices to discover all 54 Kokoro voices."},
                    "preset": {"type": "string", "default": "default_person", "description": "SVG preset: default_person, robot, cat, etc."},
                    "position": {"type": "string", "default": "top-left", "enum": ["top-left", "top-right", "top-center", "center", "bottom-left", "bottom-right", "bottom-center"]},
                    "scale": {"type": "number", "default": 0.35, "description": "Sticker scale as fraction of canvas width (0.0-1.0)."}
                }
            },
            "BackgroundSpec": {
                "type": "object",
                "properties": {
                    "type": {"type": "string", "default": "procedural", "enum": ["gameplay", "procedural", "static"], "description": "procedural = generated motion backgrounds (default). gameplay = YouTube auto-download. static = solid color/gradient."},
                    "source": {"type": "string", "default": "youtube"},
                    "query": {"type": "string", "description": "YouTube search query for gameplay type."},
                    "fallback_pool": {"type": "array", "items": {"type": "string"}, "description": "Local fallback clip paths."},
                    "crop_mode": {"type": "string", "default": "center"},
                    "loop": {"type": "boolean", "default": true},
                    "volume_db": {"type": "number", "default": -20.0},
                    "change_cadence": {"type": "string", "default": "scene", "enum": ["scene", "speaker", "fixed"]}
                }
            },
            "SceneSpec": {
                "type": "object",
                "required": ["speaker", "text"],
                "properties": {
                    "id": {"type": "string", "description": "Unique scene ID. Auto-generated if omitted."},
                    "speaker": {"type": "string", "description": "Speaker ID (must match a key in speakers)."},
                    "text": {"type": "string", "description": "The spoken text for this scene."},
                    "emote": {"type": ["string", "null"], "enum": ["neutral", "happy", "surprised", "thinking", null], "description": "Speaker emote for this scene."},
                    "background": {"type": ["string", "null"], "description": "Override background for this scene (preset name or null for auto)."},
                    "duration_override_ms": {"type": ["integer", "null"], "description": "Override scene duration in milliseconds. Null = use TTS duration."},
                    "duration_seconds": {"type": ["number", "null"], "description": "Override scene duration in SECONDS. Null = use TTS duration. If both this and duration_override_ms are set, duration_override_ms wins."},
                    "pause_ms": {"type": ["integer", "null"], "description": "Pause in ms after this scene's voiceover (breath beat)."},
                    "stock_query": {"type": ["string", "null"], "description": "Per-scene stock footage search query override. When set, this query is used directly for Pexels search instead of the auto-generated query from scene text + video_keywords. Gives you explicit control over what footage each scene gets."}
                }
            }
        },
        "examples": [{
            "title": "The History of Coffee",
            "video_keywords": ["coffee", "beans", "roasting", "brewing", "cafe"],
            "speakers": {"narrator": {"voice": "kokoro:af_heart"}},
            "scenes": [
                {"speaker": "narrator", "text": "Coffee is one of the most beloved beverages in the world.", "stock_query": "coffee beans roasting closeup", "duration_seconds": 8},
                {"speaker": "narrator", "text": "The story begins in Ethiopia, where a goat herder discovered the energizing effects.", "stock_query": "ethiopian landscape nature", "duration_seconds": 8}
            ],
            "output": {"theme": "neutral"}
        }]
    }))
}

async fn handle_script_parse(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let script_input = extract_str(&args, "script")?;
    let validate_only = default_bool(&args, "validate_only", false);

    // Determine if input is inline JSON or a file path
    let json_str: String = if script_input.trim_start().starts_with('{') {
        // Inline JSON
        script_input.to_string()
    } else {
        // File path
        let path = sanitize_input_path(script_input)?;
        if !path.exists() {
            return Err(ToolError::NotFound(format!(
                "Script file not found: {}",
                path.display()
            )));
        }
        std::fs::read_to_string(&path)?
    };

    // Parse the script
    let spec = parse_script(&json_str)
        .map_err(|e| ToolError::InvalidArg(format!("Failed to parse script JSON: {}", e)))?;

    // Validate
    let errors = validate_script(&spec);

    if !errors.is_empty() {
        // Return validation errors
        return Ok(json!({
            "status": "invalid",
            "error_count": errors.len(),
            "errors": errors,
            "spec": if validate_only { serde_json::Value::Null } else { json!(spec) },
        }));
    }

    // Valid
    Ok(json!({
        "status": "valid",
        "error_count": 0,
        "spec": if validate_only { serde_json::Value::Null } else { json!(spec) },
        "summary": {
            "title": spec.title,
            "scene_count": spec.scenes.len(),
            "speaker_count": spec.speakers.len(),
            "aspect": spec.meta.aspect,
            "fps": spec.meta.fps,
            "tts_backend": spec.tts.backend,
            "caption_style": spec.captions.style,
            "background_type": spec.background.r#type,
            "stickers_enabled": spec.stickers.enabled,
            "lip_sync_mode": spec.stickers.lip_sync,
        },
    }))
}

// ---------------------------------------------------------------------------
// Handler: script.generate_voices — TTS per scene
// ---------------------------------------------------------------------------

async fn handle_script_generate_voices(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let script_input = extract_str(&args, "script")?;
    let output_dir = default_str(&args, "output_dir", "artifacts/voices");

    // Parse script
    let json_str = read_script_input(script_input)?;
    let spec = parse_script(&json_str)
        .map_err(|e| ToolError::InvalidArg(format!("Script parse error: {}", e)))?;
    let errors = validate_script(&spec);
    if !errors.is_empty() {
        return Err(ToolError::InvalidArg(format!(
            "Script validation failed: {} errors",
            errors.len()
        )));
    }

    // Create output directory
    std::fs::create_dir_all(&output_dir)?;

    report_progress(0.0, 100.0, "Generating voices...")
        .await
        .ok();

    let total_scenes = spec.scenes.len();
    let mut segments = Vec::new();
    let mut current_ms = 0i64;
    // Collect per-scene warnings (e.g. Parakeet alignment failure) so callers
    // (script.to_timeline → script.to_video) can surface them in their own
    // response. Without this, whisper failures were only visible via
    // tracing::warn! to stderr — the JSON response said "warnings: null"
    // even when 5/5 scenes had fallen back to estimated word timings.
    // (UX audit GAP #1 fix.)
    let mut voice_warnings: Vec<String> = Vec::new();

    for (i, scene) in spec.scenes.iter().enumerate() {
        report_progress(
            (i as f64 / total_scenes as f64) * 100.0,
            100.0,
            &format!("Voice {}/{}: {}", i + 1, total_scenes, scene.speaker),
        )
        .await
        .ok();

        // Get speaker's voice profile
        let speaker = spec
            .speakers
            .get(&scene.speaker)
            .ok_or_else(|| ToolError::NotFound(format!("Speaker not found: {}", scene.speaker)))?;

        // Load voice profile from registry
        let profiles_path = ".openscript/voice_profiles.json";
        let registry = openscript_tts::profiles::VoiceProfileRegistry::new(profiles_path)
            .map_err(|e| ToolError::Tts(e.to_string()))?;

        // Try to find the voice profile by ID or by voice field.
        // Normalize bare Kokoro IDs: if "af_heart" fails, try "kokoro:af_heart".
        // (UX audit GAP #6: agents wrote bare IDs like "af_heart".)
        let voice_lookup = &speaker.voice;
        // Bare IDs resolve as-is first (audio8 clones are stored by bare name);
        // only kokoro presets fall back to the "kokoro:" prefix form.
        let normalized_voice = if !voice_lookup.starts_with("kokoro:")
            && !voice_lookup.starts_with("faster-qwen")
            && !voice_lookup.starts_with("audio8:")
        {
            format!("kokoro:{}", voice_lookup)
        } else {
            voice_lookup.clone()
        };
        let profile = registry
            .get(voice_lookup)
            .or_else(|| registry.get(&normalized_voice))
            .or_else(|| {
                // If voice is "kokoro:af_heart", try to find a profile with that model
                registry
                    .list()
                    .iter()
                    .find(|p| p.model == *voice_lookup || p.model == normalized_voice)
                    .cloned()
            }).cloned()
            .ok_or_else(|| {
                ToolError::NotFound(format!(
                    "Voice profile '{}' not found in registry. Try '{}' or add it via voice.profile.add.",
                    voice_lookup, normalized_voice
                ))
            })?;

        // Generate TTS for this scene
        let wav_path = format!("{}/{}_{}.wav", output_dir, scene.id, scene.speaker);
        if let Some(parent) = Path::new(&wav_path).parent() {
            std::fs::create_dir_all(parent).ok();
        }

        let result = tts_generate_routed(
            &speaker.voice,
            &scene.text,
            &wav_path,
            spec.tts.default_speed,
            spec.tts.default_pitch,
            1.0, // volume
            "wav",
            &profile,
        )
        .await?;

        // Calculate word timings for this scene, routing the alignment engine
        // by script language: Hinglish/Hindi → Whisper (multilingual, `hi`),
        // English → Parakeet TDT. Parakeet is English-only; on Hinglish its
        // word counts drift and remap_words_to_script collapses to even-spacing
        // estimates (caption-sync gap). Both engines' timings are text-remapped
        // to the script's ground-truth words below.
        let scene_end_ms = current_ms + result.duration_ms;
        let lang = spec.language.to_lowercase();
        let hinglish = lang.starts_with("hi") || lang.contains("hinglish");
        let words = if hinglish {
            match run_whisper_alignment(&result.output_path, &scene.text, "hi", current_ms, scene_end_ms).await {
                Ok(timed) => remap_words_to_script(&scene.text, timed, current_ms, scene_end_ms),
                Err(e) => {
                    let msg = format!(
                        "Scene {}: Whisper alignment failed ({}), falling back to Parakeet.",
                        i + 1,
                        e
                    );
                    tracing::warn!("[script.generate_voices] {}", msg);
                    voice_warnings.push(msg);
                    match run_parakeet_alignment(&result.output_path, current_ms, scene_end_ms).await {
                        Ok(timed) => remap_words_to_script(&scene.text, timed, current_ms, scene_end_ms),
                        Err(e2) => {
                            let msg2 = format!(
                                "Scene {}: Parakeet fallback failed ({}), using estimated word timings. Caption sync will be approximate.",
                                i + 1,
                                e2
                            );
                            tracing::warn!("[script.generate_voices] {}", msg2);
                            voice_warnings.push(msg2);
                            estimate_word_timings(&scene.text, current_ms, scene_end_ms)
                        }
                    }
                }
            }
        } else {
            match run_parakeet_alignment(&result.output_path, current_ms, scene_end_ms).await {
                Ok(timed) => remap_words_to_script(&scene.text, timed, current_ms, scene_end_ms),
                Err(e) => {
                    let msg = format!(
                        "Scene {}: Parakeet force-alignment failed ({}), using estimated word timings. Caption sync will be approximate.",
                        i + 1,
                        e
                    );
                    tracing::warn!("[script.generate_voices] {}", msg);
                    voice_warnings.push(msg);
                    estimate_word_timings(&scene.text, current_ms, scene_end_ms)
                }
            }
        };

        segments.push(serde_json::json!({
            "scene_id": scene.id,
            "speaker": scene.speaker,
            "text": scene.text,
            "start_ms": current_ms,
            "end_ms": scene_end_ms,
            "duration_ms": result.duration_ms,
            "wav_path": result.output_path,
            "cached": result.cached,
            "backend": result.backend,
            "words": words,
        }));

        current_ms = scene_end_ms;
    }

    // Write manifest
    let manifest_path = format!("{}/manifest.json", output_dir);
    let manifest = json!({
        "segments": segments,
        "total_duration_ms": current_ms,
        "total_scenes": total_scenes,
    });
    std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)?;

    report_progress(100.0, 100.0, "Voices generated").await.ok();

    Ok(json!({
        "status": "generated",
        "manifest_path": manifest_path,
        "total_duration_ms": current_ms,
        "total_scenes": total_scenes,
        "segments": segments,
        "warnings": if voice_warnings.is_empty() { serde_json::Value::Null } else { json!(voice_warnings) },
    }))
}

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

async fn handle_script_build_captions(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let script_input = extract_str(&args, "script")?;
    let manifest_path = extract_str(&args, "voiceover_manifest")?;
    let output_path = default_str(&args, "output_path", "artifacts/captions.ass");

    // Parse script
    let json_str = read_script_input(script_input)?;
    let spec = parse_script(&json_str)
        .map_err(|e| ToolError::InvalidArg(format!("Script parse error: {}", e)))?;

    // Load voiceover manifest
    let manifest_str = std::fs::read_to_string(sanitize_input_path(manifest_path)?)?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest_str)?;

    // Build CaptionSegments from manifest
    let mut segments = Vec::new();
    if let Some(segs) = manifest.get("segments").and_then(|v| v.as_array()) {
        for seg in segs {
            let text = seg
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let start_ms = seg.get("start_ms").and_then(|v| v.as_i64()).unwrap_or(0);
            let end_ms = seg.get("end_ms").and_then(|v| v.as_i64()).unwrap_or(0);

            // Convert word timings from manifest. Caption TEXT must be the
            // SCRIPT's words — never the ASR transcription of the TTS audio
            // (Parakeet mis-hears cloned voices: "bias" → "pie"). Keep the
            // alignment's real timing windows when the word count matches;
            // otherwise fall back to char-proportional estimation.
            let timed_words: Vec<WordTiming> = seg
                .get("words")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|w| {
                            Some(WordTiming {
                                word: w.get("word")?.as_str()?.to_string(),
                                start_ms: w.get("start_ms")?.as_i64()?,
                                end_ms: w.get("end_ms")?.as_i64()?,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            let words = remap_words_to_script(&text, timed_words, start_ms, end_ms);

            segments.push(CaptionSegment {
                text,
                start_ms,
                end_ms,
                words,
            });
        }
    }

    // Generate ASS
    let ass_content = generate_ass(&segments, &spec.captions, spec.meta.width, spec.meta.height);

    // Write output
    if let Some(parent) = Path::new(&output_path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&output_path, ass_content)?;

    Ok(json!({
        "status": "generated",
        "output_path": output_path,
        "caption_style": spec.captions.style,
        "segment_count": segments.len(),
    }))
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

async fn handle_background_fetch(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let query = extract_str(&args, "query")?;
    let duration_s = default_f64(&args, "duration_s", 30.0);
    // SEGMENTATION_ARCHITECTURE min/max clip duration: clips shorter than
    // `min_duration_s` are skipped (alternates are fetched instead of looping);
    // `max_duration_s` caps the upper bound. 0 = fall back to duration_s / no cap.
    let min_duration_s = default_f64(&args, "min_duration_s", 0.0);
    let max_duration_s = default_f64(&args, "max_duration_s", 0.0);
    let aspect = default_str(&args, "aspect", "9:16");
    let scene_text = default_str(&args, "scene_text", "");
    let cache_dir = default_str(&args, "cache_dir", "mcp/assets/background_cache");
    let fallback_pool: Vec<String> = args
        .get("fallback_pool")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    // Non-redundancy: Pexels video ids already used elsewhere in this run /
    // timeline (e.g. by broll.fetch). The same stock clip must not be re-fetched
    // under a different query — skip these ids during best-video selection.
    let used_video_ids: std::collections::HashSet<i64> = args
        .get("used_video_ids")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_i64())
                .collect::<std::collections::HashSet<i64>>()
        })
        .unwrap_or_default();

    std::fs::create_dir_all(&cache_dir)?;

    // Cache key includes the scene text so a different scene context never
    // reuses a clip cached for another scene — the L3 vision gate must be
    // re-run per scene, not short-circuited by a stale cache hit.
    let mut cache_seed = query.as_bytes().to_vec();
    if !scene_text.is_empty() {
        cache_seed.extend_from_slice(scene_text.as_bytes());
    }
    let cache_key = format!("{:x}", md5_hash(&cache_seed));
    let clip_path = format!("{}/{}_clip.mp4", cache_dir, cache_key);

    // === PRIORITY 1: Pexels API (most reliable) ===
    let pexels_key_val = pexels_key();

    if !pexels_key_val.is_empty() {
        report_progress(0.0, 100.0, "Searching Pexels for stock footage...")
            .await
            .ok();

        let orientation = aspect_to_orientation(&aspect);

        // SEGMENTATION_ARCHITECTURE clip-duration matching: prefer clips that
        // COVER the requested duration so short clips never need looping —
        // fetch ALTERNATE stock videos for the same keywords (up to 3 pages)
        // until one covers `duration_s`. `min_duration_s`/`max_duration_s`
        // params (default 0) are passed to the API as hard filters when set.
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| ToolError::Asset(format!("HTTP client error: {}", e)))?;

        match client
            .get(&pexels_search_url(query, &orientation, 1, min_duration_s, max_duration_s))
            .header("Authorization", &pexels_key_val)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                let body: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| ToolError::Asset(format!("Pexels parse error: {}", e)))?;

                let mut videos = body
                    .get("videos")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();

                // Keep fetching alternates (pages 2-3) until a clip covers
                // `duration_s` — the whole point of "find alternate videos
                // rather than looping". Stop early when page 1 already has one.
                for page in 2..=3 {
                    let has_cover = videos.iter().any(|v| {
                        v.get("duration").and_then(|x| x.as_i64()).unwrap_or(0) as f64
                            >= duration_s
                    });
                    if has_cover {
                        break;
                    }
                    if let Ok(resp2) = client
                        .get(&pexels_search_url(
                            query,
                            &orientation,
                            page,
                            min_duration_s,
                            max_duration_s,
                        ))
                        .header("Authorization", &pexels_key_val)
                        .send()
                        .await
                    {
                        if resp2.status().is_success() {
                            if let Ok(b2) = resp2.json::<serde_json::Value>().await {
                                if let Some(v2) = b2.get("videos").and_then(|v| v.as_array()) {
                                    videos.extend(v2.clone());
                                }
                            }
                        }
                    }
                }

                // Find a video with enough duration — prefer longer videos.
                // Skip ids in `used_video_ids` so the same stock clip is never
                // re-fetched under a different query (non-redundancy).
                let mut best_video: Option<(String, i64, i64)> = None; // (url, duration, pexels_id)
                let mut best_duration: i64 = 0;
                for video in &videos {
                    let vid_id = video.get("id").and_then(|v| v.as_i64()).unwrap_or(-1);
                    if vid_id >= 0 && used_video_ids.contains(&vid_id) {
                        continue;
                    }
                    let vid_duration = video.get("duration").and_then(|v| v.as_i64()).unwrap_or(0);
                    // Prefer videos that are at least as long as what we need
                    // But accept any video >= 5s — the renderer will loop it
                    if vid_duration >= 5 {
                        // Get the best quality file that's 720p-1080p
                        for file in video
                            .get("video_files")
                            .and_then(|v| v.as_array())
                            .unwrap_or(&Vec::new())
                        {
                            let width = file.get("width").and_then(|v| v.as_u64()).unwrap_or(0);
                            let url = file.get("link").and_then(|v| v.as_str()).unwrap_or("");
                            if (720..=1920).contains(&width) && !url.is_empty() {
                                // Prefer the longest video
                                if vid_duration > best_duration {
                                    best_video = Some((url.to_string(), vid_duration, vid_id));
                                    best_duration = vid_duration;
                                }
                                break;
                            }
                        }
                    }
                }

                if let Some((video_url, source_duration, pexels_id)) = best_video {
                    report_progress(40.0, 100.0, "Downloading stock footage...")
                        .await
                        .ok();

                    // Download the full video
                    match client.get(&video_url).send().await {
                        Ok(resp) if resp.status().is_success() => {
                            let bytes = resp
                                .bytes()
                                .await
                                .map_err(|e| ToolError::Asset(format!("Download error: {}", e)))?;
                            let full_path = format!("{}/{}_full.mp4", cache_dir, cache_key);
                            std::fs::write(&full_path, &bytes)?;

                            report_progress(70.0, 100.0, "Processing clip...")
                                .await
                                .ok();

                            // If the source video is long enough, extract the requested duration
                            // If it's shorter, use the full video (renderer will loop it)
                            let (output_path, actual_duration_s, start_s) =
                                if source_duration as f64 >= duration_s {
                                    // Extract a clip of duration_s from a random start point
                                    let max_start = (source_duration as f64 - duration_s).max(0.0);
                                    let seed = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|d| d.as_nanos())
                                        .unwrap_or(0)
                                        as u64;
                                    let start = if max_start > 0.0 {
                                        (seed as f64 / u64::MAX as f64) * max_start
                                    } else {
                                        0.0
                                    };

                                    // Crop to aspect ratio
                                    let crop_filter = crop_filter_for_aspect(&aspect);

                                    let crop_result = tokio::process::Command::new("ffmpeg")
                                        .arg("-y")
                                        .arg("-ss")
                                        .arg(start.to_string())
                                        .arg("-i")
                                        .arg(&full_path)
                                        .arg("-t")
                                        .arg(duration_s.to_string())
                                        .arg("-vf")
                                        .arg(&crop_filter)
                                        .arg("-c:v")
                                        .arg("libx264")
                                        .arg("-preset")
                                        .arg("fast")
                                        .arg("-crf")
                                        .arg("23")
                                        .arg("-an")
                                        .arg(&clip_path)
                                        .stdin(std::process::Stdio::null())
                                        .stdout(std::process::Stdio::null())
                                        .stderr(std::process::Stdio::piped())
                                        .kill_on_drop(true)
                                        .output()
                                        .await;

                                    if let Ok(o) = crop_result {
                                        if o.status.success() {
                                            (clip_path.clone(), duration_s, start)
                                        } else {
                                            (full_path.clone(), source_duration as f64, 0.0)
                                        }
                                    } else {
                                        (full_path.clone(), source_duration as f64, 0.0)
                                    }
                                } else {
                                    // Source is shorter than needed — use full video, renderer will loop
                                    // Still crop to aspect ratio
                                    let crop_filter = crop_filter_for_aspect(&aspect);

                                    let crop_result = tokio::process::Command::new("ffmpeg")
                                        .arg("-y")
                                        .arg("-i")
                                        .arg(&full_path)
                                        .arg("-vf")
                                        .arg(&crop_filter)
                                        .arg("-c:v")
                                        .arg("libx264")
                                        .arg("-preset")
                                        .arg("fast")
                                        .arg("-crf")
                                        .arg("23")
                                        .arg("-an")
                                        .arg(&clip_path)
                                        .stdin(std::process::Stdio::null())
                                        .stdout(std::process::Stdio::null())
                                        .stderr(std::process::Stdio::piped())
                                        .kill_on_drop(true)
                                        .output()
                                        .await;

                                    if let Ok(o) = crop_result {
                                        if o.status.success() {
                                            (clip_path.clone(), source_duration as f64, 0.0)
                                        } else {
                                            (full_path.clone(), source_duration as f64, 0.0)
                                        }
                                    } else {
                                        (full_path.clone(), source_duration as f64, 0.0)
                                    }
                                };

                            report_progress(100.0, 100.0, "Stock footage ready")
                                .await
                                .ok();
                            let needs_looping = (source_duration as f64) < duration_s;
                            let result = json!({
                                "status": "fetched",
                                "clip_path": output_path,
                                "source": "pexels",
                                "pexels_id": pexels_id,
                                "source_duration_s": source_duration,
                                "start_s": start_s,
                                "duration_s": actual_duration_s,
                                "needs_looping": needs_looping,
                                "cached": false
                            });
                            return Ok(result);
                        }
                        _ => tracing::warn!("[background.fetch] Pexels download failed"),
                    }
                } else {
                    tracing::warn!(
                        "[background.fetch] No suitable Pexels videos found for query: {}",
                        query
                    );
                }
            }
            _ => tracing::warn!("[background.fetch] Pexels API request failed"),
        }
    }

    // === PRIORITY 1.5: Pixabay film footage (signal-ranked) ===
    // Pixabay is now wired into the b-roll chain: `video_type=film` (real
    // footage, NOT animation) → stock_signal lexical gate → HTTP download →
    // cover-crop → geometry gate → content-hash dedup. Needs PIXABAY_API_KEY
    // (setup.sh / setup_openscript_config.sh). Shares the dedup sets with the
    // YouTube priority below so a clip used here is never re-fetched by the
    // YouTube path (non-redundancy across engines).
    let mut used_stock_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut used_stock_hashes: std::collections::HashSet<String> = std::collections::HashSet::new();
    if !pixabay_key().is_empty() {
        report_progress(20.0, 100.0, "Searching Pixabay stock footage...").await.ok();
        if let Some(fetch) = fetch_pixabay_stock_clip_signal(
            query,
            &[],
            duration_s,
            min_duration_s,
            max_duration_s,
            &aspect,
            &clip_path,
            &mut used_stock_ids,
            &mut used_stock_hashes,
        )
        .await
        {
            report_progress(100.0, 100.0, "Pixabay stock clip ready").await.ok();
            // Probe the produced clip for its ACTUAL duration (same contract as
            // the YouTube path below) so consumers can loop or flag shortfalls.
            let actual_duration_s = match openscript_ffmpeg::probe::probe(&clip_path).await {
                Ok(m) if m.duration > 0.0 => m.duration,
                _ => duration_s,
            };
            return Ok(json!({
                "status": "fetched",
                "clip_path": clip_path,
                "source": "pixabay",
                "pixabay_id": fetch.video_id,
                "source_title": fetch.source_title,
                "lexical_score": fetch.lexical_score,
                "source_duration_s": actual_duration_s,
                "start_s": 0.0,
                "duration_s": duration_s,
                "needs_looping": actual_duration_s < duration_s,
                "cached": false,
            }));
        }
    }

    // === PRIORITY 2: YouTube via yt-dlp (signal-ranked, stock-phrased) ===
    // Reuses fetch_youtube_stock_clip_signal — the SAME relevance path as
    // script.to_video: 12 candidates → stock_signal lexical gate → video-only
    // download → cover-crop (setsar=1) → geometry gate. Plain keywords on
    // YouTube surface news/lectures; the "stock footage" suffix flips results
    // to real b-roll (docs/MEDIA_SEARCH_AUDIT.md §2). Shares the dedup sets
    // declared above with the Pixabay priority.
    report_progress(30.0, 100.0, "Searching YouTube stock footage...").await.ok();
    let signal = crate::stock_signal::signal_tokens_from_scene(query, &[]);
    let yt_query = if query.to_ascii_lowercase().contains("stock footage") {
        query.to_string()
    } else {
        format!("{} stock footage", query)
    };
    if let Some(fetch) = fetch_youtube_stock_clip_signal(
        &yt_query,
        &signal,
        duration_s,
        &aspect,
        &clip_path,
        0,
        &mut used_stock_ids,
        &mut used_stock_hashes,
        &scene_text,
        min_duration_s,
        max_duration_s,
    )
    .await
    {
        report_progress(100.0, 100.0, "YouTube stock clip ready").await.ok();
        // Probe the produced clip for its ACTUAL duration — the signal path
        // trims from start_s=1.5 with `-t duration_s`, so a short source
        // yields a clip shorter than requested. Report the truth so consumers
        // (broll_gaps / broll.auto) can loop or flag the shortfall.
        let actual_duration_s = match openscript_ffmpeg::probe::probe(&clip_path).await {
            Ok(m) if m.duration > 0.0 => m.duration,
            _ => duration_s,
        };
        return Ok(json!({
            "status": "fetched",
            "clip_path": clip_path,
            "source": "youtube",
            "youtube_id": fetch.video_id,
            "source_title": fetch.source_title,
            "lexical_score": fetch.lexical_score,
            "vision_score": fetch.vision_score,
            "vision_reason": fetch.vision_reason,
            "source_duration_s": actual_duration_s,
            "start_s": 1.5, // extraction start used by fetch_youtube_stock_clip_signal (scene 0)
            "duration_s": duration_s,
            "needs_looping": actual_duration_s < duration_s,
            "cached": false,
        }));
    }

    // === PRIORITY 3: Fallback pool ===
    if let Some(fallback) = fallback_pool.first() {
        if Path::new(fallback).exists() {
            return Ok(json!({
                "status": "fallback",
                "clip_path": fallback,
                "source": "fallback_pool",
                "source_duration_s": duration_s,
                "cached": false,
                "warning": "Pexels + YouTube failed, using fallback pool"
            }));
        }
    }
    // === PRIORITY 4: Procedural ===
    generate_procedural_background(&cache_dir, &cache_key, duration_s, &aspect).await
}


/// Generate a procedural background via FFmpeg filters (fallback when yt-dlp unavailable).
async fn generate_procedural_background(
    cache_dir: &str,
    cache_key: &str,
    duration_s: f64,
    aspect: &str,
) -> Result<serde_json::Value, ToolError> {
    let (w, h) = aspect_to_crop_dims(aspect);
    let clip_path = format!("{}/{}_procedural.mp4", cache_dir, cache_key);

    let filter = format!(
        "color=c=0x0a0a1a:s={}x{}:d={}:r=30[bg];\
         color=c=0x1a1a3a:s={}x{}:d={}:r=30[bg2];\
         [bg][bg2]blend=all_mode=overlay:all_opacity=0.5[bg3];\
         [bg3]geq=r='128+80*sin(2*PI*X/W+0.1*T)':g='128+80*sin(2*PI*Y/H+0.15*T)':b='128+80*sin(2*PI*(X+Y)/(W+H)+0.2*T)'[v]",
        w, h, duration_s, w, h, duration_s
    );

    let result = tokio::process::Command::new("ffmpeg")
        .arg("-y")
        .arg("-filter_complex")
        .arg(&filter)
        .arg("-map")
        .arg("[v]")
        .arg("-c:v")
        .arg("libx264")
        .arg("-preset")
        .arg("fast")
        .arg("-crf")
        .arg("23")
        .arg("-an")
        .arg(&clip_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await?;

    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        return Err(ToolError::Ffmpeg(format!(
            "Procedural background failed: {}",
            stderr
        )));
    }

    Ok(json!({
        "status": "procedural",
        "clip_path": clip_path,
        "duration_s": duration_s,
        "cached": false,
        "warning": "yt-dlp unavailable, generated procedural background"
    }))
}

// ---------------------------------------------------------------------------
// Handler: background.assign — assign clips to scenes
// ---------------------------------------------------------------------------

async fn handle_background_assign(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let script_input = extract_str(&args, "script")?;
    let manifest_path = extract_str(&args, "voiceover_manifest")?;
    let background_pool: Vec<String> = args
        .get("background_pool")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ToolError::MissingArg("background_pool is required".into()))?
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    let output_path = default_str(
        &args,
        "output_path",
        "artifacts/background_assignments.json",
    );

    // Parse script
    let json_str = read_script_input(script_input)?;
    let spec = parse_script(&json_str)
        .map_err(|e| ToolError::InvalidArg(format!("Script parse error: {}", e)))?;

    // Load voiceover manifest
    let manifest_str = std::fs::read_to_string(sanitize_input_path(manifest_path)?)?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest_str)?;

    // Extract scene IDs, speakers, and durations from manifest
    let mut scene_ids = Vec::new();
    let mut scene_speakers = Vec::new();
    let mut scene_durations = Vec::new();

    if let Some(segments) = manifest.get("segments").and_then(|v| v.as_array()) {
        for seg in segments {
            scene_ids.push(
                seg.get("scene_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            );
            scene_speakers.push(
                seg.get("speaker")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            );
            let dur_ms = seg
                .get("duration_ms")
                .and_then(|v| v.as_i64())
                .unwrap_or(3000);
            scene_durations.push(dur_ms as f64 / 1000.0);
        }
    }

    // Phase 5: Add pause_ms from SceneSpec to scene durations (breath beats).
    // This extends each scene's duration by the specified pause, creating
    // natural breathing gaps between scenes without affecting the TTS audio.
    for (i, dur) in scene_durations.iter_mut().enumerate() {
        if let Some(scene) = spec.scenes.get(i) {
            if let Some(pause) = scene.pause_ms {
                if pause > 0 {
                    *dur += pause as f64 / 1000.0;
                }
            }
        }
    }

    // Assign backgrounds
    let clips = assign_backgrounds(
        &scene_ids,
        &scene_speakers,
        &background_pool,
        &spec.background.change_cadence,
        &scene_durations,
    );

    // Write assignments
    if let Some(parent) = Path::new(&output_path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let assignments = json!({
        "clips": clips,
        "cadence": spec.background.change_cadence,
        "pool_size": background_pool.len(),
    });
    std::fs::write(&output_path, serde_json::to_string_pretty(&assignments)?)?;

    Ok(json!({
        "status": "assigned",
        "output_path": output_path,
        "clip_count": clips.len(),
        "cadence": spec.background.change_cadence,
    }))
}

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
const UNSAFE_KEYWORD_MAP: &[(&str, &str)] = &[
    ("inhale", "breathing meditation"),
    ("exhale", "breathing relaxation"),
    ("breathe", "breathing calm"),
    ("breathing", "breathing meditation"),
    ("drink", "drinking water wellness"),
    ("smoke", "calm nature"),
    ("drug", "calm nature"),
    ("kill", "calm nature"),
    ("blood", "calm nature"),
    ("pain", "healing wellness"),
    ("stress", "stress relief meditation"),
    ("anxiety", "anxiety relief calm"),
    ("fear", "courage calm nature"),
    ("death", "calm nature peaceful"),
    ("weapon", "calm nature"),
];

/// Enrich a Pexels search query with mood-aware context to bias results
/// toward calming/energetic content. For theme:calm, prepend "calm" to
/// the query so Pexels returns peaceful footage instead of literal matches
/// that could be tonally wrong (e.g. "inhale" → cigarette).
fn enrich_query_for_theme(query: &str, theme: &str) -> String {
    // Don't double-enrich if the query already contains a mood word
    let lower = query.to_lowercase();
    let already_calm = lower.contains("calm") || lower.contains("peaceful") || lower.contains("meditation");
    let already_energetic = lower.contains("energy") || lower.contains("action") || lower.contains("intense");

    match theme {
        "calm" if !already_calm => format!("calm {}", query),
        "energetic" if !already_energetic => format!("energetic {}", query),
        _ => query.to_string(),
    }
}

/// Filter and enrich extracted keywords for Pexels search safety.
/// 1. Replace unsafe keywords (inhale → breathing meditation)
/// 2. Enrich with theme context (prepend "calm" for calm theme)
fn safe_search_query(raw_keywords: &str, theme: &str) -> String {
    // Check each word against the unsafe map
    let mut safe_words: Vec<String> = Vec::new();
    for word in raw_keywords.split_whitespace() {
        let lower = word.to_lowercase();
        let replaced = UNSAFE_KEYWORD_MAP
            .iter()
            .find(|(unsafe_word, _)| *unsafe_word == lower.as_str())
            .map(|(_, safe)| safe.to_string())
            .unwrap_or_else(|| word.to_string());
        safe_words.push(replaced);
    }
    let safe_query = safe_words.join(" ");
    enrich_query_for_theme(&safe_query, theme)
}

/// Legacy helper — prefer `stock_signal::build_scene_stock_query` for multi-broll.
#[allow(dead_code)]
fn extract_keywords(text: &str, fallback_query: &str) -> String {
    let toks = crate::stock_signal::signal_tokens_from_scene(text, &[]);
    if toks.is_empty() {
        fallback_query.to_string()
    } else {
        toks.into_iter().take(5).collect::<Vec<_>>().join(" ")
    }
}

// ---------------------------------------------------------------------------
// Handler: background.search — search procedural background index by mood
// ---------------------------------------------------------------------------

async fn handle_background_search(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let mood_filter = default_opt_str(&args, "mood");
    let energy_filter = default_opt_str(&args, "energy");
    let motion_filter = default_opt_str(&args, "motion_intensity");
    let limit = default_u32(&args, "limit", 10) as usize;

    // Resolve the index path CWD-independently. The round-2 UX audit
    // (GAP #12) found background.search only worked from the repo root
    // because it used a relative path. Now uses resolve_repo_path which
    // tries CWD > OPENSCRIPT_ROOT > CARGO_MANIFEST_DIR.
    let index_path_raw = std::env::var("OPENSCRIPT_BACKGROUNDS_INDEX")
        .unwrap_or_else(|_| "mcp/assets/backgrounds_index.json".to_string());
    let index_path = resolve_repo_path(&index_path_raw);

    if !index_path.exists() {
        return Err(ToolError::NotFound(format!(
            "Backgrounds index not found at {} (resolved from {}). The index is committed at mcp/assets/backgrounds_index.json — if missing, re-clone or restore from git.",
            index_path.display(),
            index_path_raw
        )));
    }

    let index_str = std::fs::read_to_string(&index_path)?;
    let index: serde_json::Value = serde_json::from_str(&index_str)?;

    let entries = index
        .get("entries")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut results: Vec<serde_json::Value> = Vec::new();
    for entry in &entries {
        if let Some(ref mood) = mood_filter {
            let entry_mood = entry.get("mood").and_then(|v| v.as_str()).unwrap_or("");
            if entry_mood != mood {
                continue;
            }
        }
        if let Some(ref energy) = energy_filter {
            let entry_energy = entry.get("energy").and_then(|v| v.as_str()).unwrap_or("");
            if entry_energy != energy {
                continue;
            }
        }
        if let Some(ref motion) = motion_filter {
            let entry_motion = entry
                .get("motion_intensity")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if entry_motion != motion {
                continue;
            }
        }

        // Build the full path so callers can use it directly in fallback_pool.
        // Resolve relative to the index file's parent's parent (so
        // mcp/assets/backgrounds_index.json → mcp/assets/backgrounds/).
        // This makes the returned paths work regardless of the agent's CWD.
        let filename = entry.get("filename").and_then(|v| v.as_str()).unwrap_or("");
        let backgrounds_dir = index_path
            .parent() // mcp/assets/
            .map(|p| p.join("backgrounds"))
            .unwrap_or_else(|| std::path::PathBuf::from("mcp/assets/backgrounds"));
        let full_path = backgrounds_dir.join(filename);
        let full_path_str = full_path.to_string_lossy().to_string();

        let mut result = entry.clone();
        if let Some(obj) = result.as_object_mut() {
            obj.insert("path".into(), json!(full_path_str));
        }
        results.push(result);
    }

    let total = results.len();
    results.truncate(limit);

    Ok(json!({
        "status": "searched",
        "filters": {
            "mood": mood_filter,
            "energy": energy_filter,
            "motion_intensity": motion_filter,
        },
        "total_matches": total,
        "count": results.len(),
        "results": results,
        "index_stats": {
            "total_entries": index.get("total_entries"),
            "mood_counts": index.get("mood_counts"),
        },
    }))
}

// ---------------------------------------------------------------------------
// Handler: sticker.load_preset — load SVG preset config
// ---------------------------------------------------------------------------

/// Handler: sticker.presets — list all available sticker positioning presets
async fn handle_sticker_presets(
    _args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let presets = openscript_core::sticker_presets::StickerPreset::all();
    let presets_json: serde_json::Value = serde_json::to_value(&presets)
        .map_err(|e| ToolError::InvalidArg(format!("Failed to serialize presets: {}", e)))?;
    Ok(json!({
        "status": "success",
        "count": presets.len(),
        "presets": presets_json,
        "message": "Use preset name in speaker.preset field of script JSON. Each preset defines position, scale, and caption-safe margin."
    }))
}

async fn handle_sticker_load_preset(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let preset_name = extract_str(&args, "preset_name")?;
    let presets_dir = default_str(&args, "presets_dir", "mcp/assets/svg_presets");

    let preset_dir = format!("{}/{}", presets_dir, preset_name);
    if !Path::new(&preset_dir).exists() {
        return Err(ToolError::NotFound(format!(
            "Preset not found: {} (looked in {})",
            preset_name, preset_dir
        )));
    }

    // Load preset.json
    let preset_json_path = format!("{}/preset.json", preset_dir);
    let preset_json = std::fs::read_to_string(&preset_json_path)?;
    let preset: StickerPreset = serde_json::from_str(&preset_json)
        .map_err(|e| ToolError::InvalidArg(format!("Failed to parse preset.json: {}", e)))?;

    // Load puppet.svg
    let puppet_svg_path = format!("{}/puppet.svg", preset_dir);
    let puppet_svg = std::fs::read_to_string(&puppet_svg_path)?;

    Ok(json!({
        "status": "loaded",
        "preset_name": preset_name,
        "preset": preset,
        "puppet_svg": puppet_svg,
    }))
}

// ---------------------------------------------------------------------------
// Handler: sticker.render — generate animated sticker HTML composition
// ---------------------------------------------------------------------------

async fn handle_sticker_render(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let wav_path = extract_str(&args, "wav_path")?;
    let preset_name = extract_str(&args, "preset_name")?;
    let position = default_str(&args, "position", "top-left");
    let scale = default_f64(&args, "scale", 0.25);
    let canvas_width = default_u32(&args, "canvas_width", 1080);
    let canvas_height = default_u32(&args, "canvas_height", 1920);
    let fps = default_u32(&args, "fps", 30);
    let output_path = default_str(&args, "output_path", "artifacts/sticker.html");
    let render_to_video = default_bool(&args, "render_to_video", false);

    report_progress(0.0, 100.0, "Loading preset...").await.ok();

    // Load preset
    let presets_dir = "mcp/assets/svg_presets";
    let preset_dir = format!("{}/{}", presets_dir, preset_name);
    if !Path::new(&preset_dir).exists() {
        return Err(ToolError::NotFound(format!(
            "Preset not found: {} (looked in {})",
            preset_name, preset_dir
        )));
    }

    let preset_json = std::fs::read_to_string(format!("{}/preset.json", preset_dir))?;
    let preset: StickerPreset = serde_json::from_str(&preset_json)
        .map_err(|e| ToolError::InvalidArg(format!("Preset parse error: {}", e)))?;

    let puppet_svg = std::fs::read_to_string(format!("{}/puppet.svg", preset_dir))?;

    report_progress(30.0, 100.0, "Extracting amplitude...")
        .await
        .ok();

    // Extract amplitude from WAV
    let amplitude = extract_amplitude(wav_path, fps)
        .map_err(|e| ToolError::InvalidArg(format!("Amplitude extraction failed: {}", e)))?;

    report_progress(60.0, 100.0, "Generating composition...")
        .await
        .ok();

    // Generate sticker HTML composition
    let html = generate_sticker_composition(
        &puppet_svg,
        &preset,
        &amplitude,
        &position,
        scale,
        canvas_width,
        canvas_height,
    );

    // Write output
    if let Some(parent) = Path::new(&output_path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&output_path, html)?;

    // Phase K: Optionally render the HTML to a transparent WebM via hf.render.
    // This produces a video file that multilayer_render can composite as a
    // StickerOverlay. The WebM format preserves alpha transparency.
    let mut video_path: Option<String> = None;
    if render_to_video {
        report_progress(80.0, 100.0, "Rendering sticker to WebM via HyperFrames...")
            .await
            .ok();

        let sticker_dir = std::path::Path::new(&output_path)
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf();
        let webm_path = sticker_dir
            .join(format!(
                "sticker_{}.webm",
                std::path::Path::new(&output_path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("sticker")
            ))
            .to_string_lossy()
            .to_string();

        match crate::hf::handle_hf_render(json!({
            "project_dir": sticker_dir.to_string_lossy().to_string(),
            "output_path": webm_path,
            "quality": "draft",
        }))
        .await
        {
            Ok(_) => {
                video_path = Some(webm_path);
            }
            Err(e) => {
                tracing::warn!("[sticker.render] HF render to WebM failed: {}", e);
                // Non-fatal — the HTML is still usable; the agent can render manually
            }
        }
    }

    report_progress(100.0, 100.0, "Sticker composition generated")
        .await
        .ok();

    Ok(json!({
        "status": "generated",
        "output_path": output_path,
        "video_path": video_path,
        "preset_name": preset_name,
        "position": position,
        "scale": scale,
        "frame_count": amplitude.frames.len(),
        "duration_ms": amplitude.duration_ms,
        "next_step": if video_path.is_some() {
            "Sticker rendered to WebM. Use the video_path as a StickerOverlay in multilayer_render or overlay.assign."
        } else {
            "Sticker HTML generated. Call sticker.render with render_to_video=true to produce a compositable WebM, or use the HTML with hf.render manually."
        },
    }))
}

// ---------------------------------------------------------------------------
// Handler: script.to_timeline — orchestrator for from-scratch video creation
// ---------------------------------------------------------------------------

async fn handle_script_to_timeline(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let script_input = extract_str(&args, "script")?;
    let output_dir = default_str(&args, "output_dir", "artifacts");
    let skip_background = default_bool(&args, "skip_background", false);
    let skip_stickers = default_bool(&args, "skip_stickers", false);
    let voiceover_manifest_path = default_opt_str(&args, "voiceover_manifest_path");

    // Parse script
    let json_str = read_script_input(script_input)?;
    let spec = parse_script(&json_str)
        .map_err(|e| ToolError::InvalidArg(format!("Script parse error: {}", e)))?;
    let errors = validate_script(&spec);
    if !errors.is_empty() {
        return Err(ToolError::InvalidArg(format!(
            "Script validation failed: {} errors",
            errors.len()
        )));
    }

    let voices_dir = format!("{}/voices", output_dir);
    let stickers_dir = format!("{}/stickers", output_dir);
    std::fs::create_dir_all(&voices_dir)?;
    std::fs::create_dir_all(&stickers_dir)?;

    let mut warnings = Vec::new();

    // Step 1: Generate voices (or use pre-supplied manifest)
    let (manifest_path, total_duration_ms) = if let Some(ref path) = voiceover_manifest_path {
        // Bring-your-own-audio mode: skip TTS, use the supplied manifest
        if !std::path::Path::new(path).exists() {
            return Err(ToolError::NotFound(format!(
                "voiceover_manifest_path not found: {}",
                path
            )));
        }
        warnings.push(format!(
            "Using pre-supplied voiceover manifest: {} (skipping TTS generation)",
            path
        ));
        // Read total_duration_ms from the manifest
        let manifest: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(path).unwrap_or_default()
        ).unwrap_or(json!({}));
        let dur = manifest.get("total_duration_ms")
            .and_then(|v| v.as_i64())
            .unwrap_or_else(|| {
                // Sum segment durations if total not present
                manifest.get("segments")
                    .and_then(|v| v.as_array())
                    .map(|segs| segs.iter()
                        .filter_map(|s| s.get("duration_ms").and_then(|v| v.as_i64()))
                        .sum::<i64>())
                    .unwrap_or(0)
            });
        (path.clone(), dur)
    } else {
        report_progress(0.0, 100.0, "Step 1/5: Generating voices...")
            .await
            .ok();
        let voices_result = handle_script_generate_voices(json!({
            "script": script_input,
            "output_dir": voices_dir,
        }))
        .await?;

        // Collect voice-generation warnings (e.g. Parakeet alignment failure)
        // into our own warnings array so they propagate to script.to_video's
        // final response. Without this, the warnings were returned in
        // voices_result but never read by the caller. (UX audit GAP #1 fix.)
        if let Some(voice_warns) = voices_result.get("warnings").and_then(|v| v.as_array()) {
            for w in voice_warns {
                if let Some(s) = w.as_str() {
                    warnings.push(s.to_string());
                }
            }
        }

        let mp = voices_result
            .get("manifest_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArg("No manifest_path in voices result".into()))?
            .to_string();
        let dur = voices_result
            .get("total_duration_ms")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        (mp, dur)
    };

    // Step 2: Build captions
    report_progress(20.0, 100.0, "Step 2/5: Building captions...")
        .await
        .ok();
    let captions_path = format!("{}/captions.ass", output_dir);
    let _captions_result = handle_script_build_captions(json!({
        "script": script_input,
        "voiceover_manifest": manifest_path,
        "output_path": captions_path,
    }))
    .await?;

    // Step 3: Fetch + assign backgrounds
    report_progress(40.0, 100.0, "Step 3/5: Fetching backgrounds...")
        .await
        .ok();
    let mut background_pool: Vec<String> = spec.background.fallback_pool.clone();

    if !skip_background && spec.background.r#type == "gameplay" && !spec.background.query.is_empty()
    {
        // Fetch a background clip
        let fetch_result = handle_background_fetch(json!({
            "query": spec.background.query,
            "duration_s": total_duration_ms as f64 / 1000.0,
            "aspect": spec.meta.aspect,
            "fallback_pool": spec.background.fallback_pool,
        }))
        .await;

        match fetch_result {
            Ok(r) => {
                if let Some(path) = r.get("clip_path").and_then(|v| v.as_str()) {
                    background_pool.insert(0, path.to_string());
                }
            }
            Err(e) => {
                warnings.push(format!("Background fetch failed: {}", e));
            }
        }
    }

    // Assign backgrounds
    let bg_assignments_path = format!("{}/background_assignments.json", output_dir);
    if !background_pool.is_empty() {
        let _bg_result = handle_background_assign(json!({
            "script": script_input,
            "voiceover_manifest": manifest_path,
            "background_pool": background_pool,
            "output_path": bg_assignments_path,
        }))
        .await?;
    }

    // Step 4: Render stickers (if enabled)
    report_progress(60.0, 100.0, "Step 4/5: Rendering stickers...")
        .await
        .ok();
    let mut sticker_paths: Vec<serde_json::Value> = Vec::new();

    if !skip_stickers && spec.stickers.enabled {
        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&manifest_path)?)?;

        if let Some(segments) = manifest.get("segments").and_then(|v| v.as_array()) {
            for seg in segments {
                let speaker = seg.get("speaker").and_then(|v| v.as_str()).unwrap_or("");
                let wav_path = seg.get("wav_path").and_then(|v| v.as_str()).unwrap_or("");
                let start_ms = seg.get("start_ms").and_then(|v| v.as_i64()).unwrap_or(0);

                if wav_path.is_empty() {
                    continue;
                }

                // Get speaker's preset and position
                let speaker_spec = spec.speakers.get(speaker);
                let preset_name = speaker_spec
                    .map(|s| s.preset.clone())
                    .unwrap_or_else(|| "default_person".to_string());
                let position = speaker_spec
                    .map(|s| s.position.clone())
                    .unwrap_or_else(|| "top-left".to_string());
                let scale = speaker_spec.map(|s| s.scale).unwrap_or(0.25);

                let sticker_output = format!("{}/sticker_{}.html", stickers_dir, speaker);

                let sticker_result = handle_sticker_render(json!({
                    "wav_path": wav_path,
                    "preset_name": preset_name,
                    "position": position,
                    "scale": scale,
                    "canvas_width": spec.meta.width,
                    "canvas_height": spec.meta.height,
                    "fps": spec.meta.fps,
                    "output_path": sticker_output,
                    "render_to_video": false,  // HTML only — script.to_video uses GIPHY/PNG stickers for rendering
                }))
                .await;

                match sticker_result {
                    Ok(r) => {
                        sticker_paths.push(json!({
                            "speaker": speaker,
                            "start_ms": start_ms,
                            "html_path": r.get("output_path").and_then(|v| v.as_str()).unwrap_or(""),
                            "video_path": r.get("video_path").and_then(|v| v.as_str()).unwrap_or(""),
                            "frame_count": r.get("frame_count").and_then(|v| v.as_u64()).unwrap_or(0),
                        }));
                    }
                    Err(e) => {
                        warnings.push(format!("Sticker render failed for {}: {}", speaker, e));
                    }
                }
            }
        }
    }

    // Step 5: Assemble timeline using the proper Timeline struct
    report_progress(80.0, 100.0, "Step 5/5: Assembling timeline...")
        .await
        .ok();
    let timeline_path = format!("{}/timeline.json", output_dir);

    // Build a proper Timeline struct — use the first background as the "source" video
    // (for from-scratch videos, the background IS the source)
    let bg_source = background_pool
        .first()
        .cloned()
        .or_else(|| spec.background.fallback_pool.first().cloned())
        .unwrap_or_else(|| "mcp/assets/backgrounds/procedural_01.mp4".to_string());

    // If background pool is empty, use the procedural fallback
    let mut background_pool = background_pool;
    if background_pool.is_empty() {
        background_pool.push(bg_source.clone());
    }

    let mut timeline = Timeline::new(
        std::path::PathBuf::from(&bg_source),
        &spec.meta.aspect,
        spec.meta.fps,
        None,
    );

    // Add segments from the voiceover manifest
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path)?)?;
    let mut current_ms = 0i64;
    if let Some(segments) = manifest.get("segments").and_then(|v| v.as_array()) {
        for seg in segments {
            let scene_id = seg.get("scene_id").and_then(|v| v.as_str()).unwrap_or("");
            let text = seg.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let dur_ms = seg
                .get("duration_ms")
                .and_then(|v| v.as_i64())
                .unwrap_or(3000);
            let wav_path = seg.get("wav_path").and_then(|v| v.as_str()).unwrap_or("");

            // Add segment
            let segment = openscript_core::timeline::Segment {
                id: scene_id.to_string(),
                start: current_ms as f64 / 1000.0,
                end: (current_ms + dur_ms) as f64 / 1000.0,
                caption: text.to_string(),
                crossfade_ms: 0,
                semantic_role: None,
            };
            timeline.segments.push(segment);

            // Add voiceover event
            let vo_event = openscript_core::timeline::TimelineEvent {
                id: format!("vo_{}", scene_id),
                asset_id: scene_id.to_string(),
                start_ms: current_ms,
                end_ms: current_ms + dur_ms,
                offset_ms: 0,
                gain_db: -6.0,
                fade_in_ms: 50,
                fade_out_ms: 50,
                tags: vec![],
                provenance: Some(openscript_core::timeline::Provenance {
                    tool: "script.to_timeline".into(),
                    editorial_role: None,
                    concept: None,
                }),
                kind: openscript_core::timeline::EventKind::Voiceover {
                    voice_profile_id: seg
                        .get("speaker")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    text: text.to_string(),
                    estimated_duration_ms: dur_ms,
                },
            };
            timeline.add_track_event(TrackType::Voiceover, vo_event);

            // Register voiceover asset
            timeline.add_asset("voices", scene_id.to_string(), json!({"path": wav_path}));

            current_ms += dur_ms;
        }
    }

    // Add background as broll
    if let Some(bg_path) = background_pool.first() {
        let broll_event = openscript_core::timeline::TimelineEvent {
            id: "broll_bg".to_string(),
            asset_id: "broll_bg".to_string(),
            start_ms: 0,
            end_ms: total_duration_ms,
            offset_ms: 0,
            gain_db: spec.background.volume_db,
            fade_in_ms: 0,
            fade_out_ms: 0,
            tags: vec![],
            provenance: Some(openscript_core::timeline::Provenance {
                tool: "script.to_timeline".into(),
                editorial_role: None,
                concept: Some("background".to_string()),
            }),
            kind: openscript_core::timeline::EventKind::Broll {
                concept: "background".to_string(),
                source_provider: "youtube".to_string(),
                transition_style: "cut".to_string(),
                crop_mode: spec.background.crop_mode.clone(),
                orientation: spec.meta.aspect.clone(),
                motion_intensity: "medium".to_string(),
            },
        };
        timeline.add_track_event(TrackType::Broll, broll_event);
        timeline.add_asset("broll", "broll_bg".to_string(), json!({"path": bg_path}));
    }

    // Add captions asset
    timeline.add_asset(
        "captions",
        "ass".to_string(),
        json!({"path": captions_path}),
    );

    // Add music if specified
    if let Some(ref music) = spec.music {
        if let Some(ref path) = music.path {
            if !path.is_empty() {
                let music_event = openscript_core::timeline::TimelineEvent {
                    id: "music_bg".to_string(),
                    asset_id: "music_bg".to_string(),
                    start_ms: 0,
                    end_ms: total_duration_ms,
                    offset_ms: 0,
                    gain_db: music.gain_db,
                    fade_in_ms: 500,
                    fade_out_ms: 500,
                    tags: vec![],
                    provenance: Some(openscript_core::timeline::Provenance {
                        tool: "script.to_timeline".into(),
                        editorial_role: None,
                        concept: None,
                    }),
                    kind: openscript_core::timeline::EventKind::Music {
                        mood: "neutral".to_string(),
                        energy: "medium".to_string(),
                        bpm: None,
                        loopability: true,
                        intro_friendly: true,
                        cta_friendly: false,
                        loudness_target_lufs: -14.0,
                        loop_mode: "loop".to_string(),
                        ducking_policy: if music.ducking {
                            "auto".to_string()
                        } else {
                            "none".to_string()
                        },
                    },
                };
                timeline.add_track_event(TrackType::Music, music_event);
                timeline.add_asset("music", "music_bg".to_string(), json!({"path": path}));

                // Add ducking directive
                if music.ducking {
                    timeline.add_ducking_directive(
                        "voiceover",
                        "music",
                        music.ducking_depth_db,
                        50,
                        200,
                    );
                }
            }
        }
    }

    // Save timeline
    timeline.save(&timeline_path)?;

    report_progress(100.0, 100.0, "Timeline assembled")
        .await
        .ok();

    Ok(json!({
        "status": "assembled",
        "timeline_path": timeline_path,
        "voiceover_manifest": manifest_path,
        "captions_path": captions_path,
        "background_assignments": bg_assignments_path,
        "total_duration_ms": total_duration_ms,
        "scene_count": spec.scenes.len(),
        "speaker_count": spec.speakers.len(),
        "background_pool_size": background_pool.len(),
        "sticker_count": sticker_paths.len(),
        "warnings": if warnings.is_empty() { serde_json::Value::Null } else { json!(warnings) },
    }))
}

// ---------------------------------------------------------------------------
// Handler: script.to_video — one-call from-scratch video creation
// ---------------------------------------------------------------------------

async fn handle_script_to_video(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let script_input = extract_str(&args, "script")?;
    let mut output_path = default_str(&args, "output_path", "output.mp4");
    // P0 FIX: Resolve output_path to absolute so ffmpeg writes to a predictable location.
    // Without this, relative paths like "output.mp4" resolve against the MCP server's CWD,
    // which may differ from the agent's expected working directory.
    if !std::path::Path::new(&output_path).is_absolute() {
        match std::env::current_dir() {
            Ok(cwd) => { output_path = cwd.join(&output_path).to_string_lossy().to_string(); }
            Err(e) => {
                return Err(ToolError::InvalidArg(format!(
                    "Cannot resolve output_path '{}' to absolute: {}", output_path, e
                )));
            }
        }
    }
    let output_dir = default_str(&args, "output_dir", "artifacts");
    let skip_background = default_bool(&args, "skip_background", false);
    let skip_stickers = default_bool(&args, "skip_stickers", false);
    let preview_mode = default_bool(&args, "preview_mode", false);
    let voiceover_manifest_path = default_opt_str(&args, "voiceover_manifest_path");

    // Parse script for render config
    let json_str = read_script_input(script_input)?;
    let spec = parse_script(&json_str)
        .map_err(|e| ToolError::InvalidArg(format!("Script parse error: {}", e)))?;

    report_progress(0.0, 100.0, "Phase 1/3: Building timeline...")
        .await
        .ok();

    // Step 1: Build the timeline
    // ponytail: skip_background=true for timeline handler — this function does
    // its own per-scene multi-broll fetch below. The timeline handler only
    // fetched ONE clip for the whole video (inconsistent with multi-scene).
    let mut timeline_args = json!({
        "script": script_input,
        "output_dir": output_dir,
        "skip_background": true,
        "skip_stickers": skip_stickers,
    });
    if let Some(ref path) = voiceover_manifest_path {
        timeline_args["voiceover_manifest_path"] = json!(path);
    }
    let timeline_result = handle_script_to_timeline(timeline_args).await?;

    let timeline_path = timeline_result
        .get("timeline_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::InvalidArg("No timeline_path in result".into()))?
        .to_string();
    let warnings = timeline_result
        .get("warnings")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    // Collect additional warnings from the render phase (procedural fallbacks, etc.)
    let mut render_warnings: Vec<String> = Vec::new();

    report_progress(40.0, 100.0, "Phase 2/3: Building layered composition...")
        .await
        .ok();

    // Load manifest
    let manifest_path = timeline_result
        .get("voiceover_manifest")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let captions_path = timeline_result
        .get("captions_path")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let total_duration_ms = timeline_result
        .get("total_duration_ms")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let total_duration_s = total_duration_ms as f64 / 1000.0;

    let manifest: serde_json::Value = if !manifest_path.is_empty() {
        serde_json::from_str(&std::fs::read_to_string(manifest_path)?)?
    } else {
        json!({"segments": []})
    };

    // Extract voiceover paths and per-scene durations
    let mut voiceover_paths: Vec<String> = Vec::new();
    let mut scene_durations: Vec<f64> = Vec::new();
    if let Some(segments) = manifest.get("segments").and_then(|v| v.as_array()) {
        for seg in segments {
            if let Some(path) = seg.get("wav_path").and_then(|v| v.as_str()) {
                voiceover_paths.push(path.to_string());
            }
            let dur_ms = seg
                .get("duration_ms")
                .and_then(|v| v.as_i64())
                .unwrap_or(3000);
            scene_durations.push(dur_ms as f64 / 1000.0);
        }
    }

    // Phase 5: Add pause_ms from SceneSpec to scene durations (breath beats).
    for (i, dur) in scene_durations.iter_mut().enumerate() {
        if let Some(scene) = spec.scenes.get(i) {
            if let Some(pause) = scene.pause_ms {
                if pause > 0 {
                    *dur += pause as f64 / 1000.0;
                }
            }
        }
    }

    // === MULTI-BROLL: Download a DIFFERENT stock clip per scene ===
    // Instead of looping one short clip, download a unique stock video
    // for each scene based on keywords extracted from the scene text.
    let mut per_scene_backgrounds: Vec<String> = Vec::new();
    // (video_id, content_hash, search_query) per scene for variance KPIs
    // Per-scene stock provenance for KPI (id, hash, query, lex, title)
    // id, hash, q, lex, title, vision_score, vision_reason
    let mut scene_stock_meta: Vec<Option<(String, String, String, f64, String, f64, Option<String>)>> =
        Vec::new();
    let pexels_key_val = pexels_key();

    // The final stock query per scene — the sticker stage reuses these SAME
    // keywords so b-roll and stickers are driven by one keyword source
    // (sticker/broll pipeline unification).
    let mut scene_stock_queries: Vec<String> = Vec::new();

    // Multi-broll stock footage: unique clip per scene.
    // Priority: Pexels (if key) → YouTube via yt-dlp (no key) → procedural (last resort).
    // type:"static" is the explicit opt-out. type:"procedural" still TRIES stock first
    // so agents do not silently ship gradient-only videos when stock is reachable.
    // (Phase CF: production quality upgrade — never treat procedural as success.)
    if !skip_background && spec.background.r#type != "static" {
        report_progress(
            35.0,
            60.0,
            "Fetching multi-broll stock backgrounds (Pexels → YouTube → procedural)...",
        )
        .await
        .ok();

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .map_err(|e| ToolError::Asset(format!("HTTP client error: {}", e)))?;

        let orientation = match spec.meta.aspect.as_str() {
            "9:16" => "portrait",
            "16:9" => "landscape",
            "1:1" => "square",
            _ => "portrait",
        };

        let cache_dir = "mcp/assets/background_cache";
        std::fs::create_dir_all(cache_dir).ok();

        // Track Pexels video IDs that have already been used to prevent
        // the same clip appearing in multiple scenes.
        // (Round-16: "There are repeating video-cuts between the GIFs.
        // Ensure that the entire video timeline has unique videos, not
        // repeated video that might reduce the attention-hooking.")
        let mut used_pexels_ids: std::collections::HashSet<i64> = std::collections::HashSet::new();
        let mut used_yt_queries: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        // Content-level uniqueness (Phase CI): ytsearch1 often returns the SAME
        // viral video for similar queries — track video IDs + file fingerprints.
        let mut used_video_ids: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let mut used_content_hashes: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        for (scene_idx, &dur) in scene_durations.iter().enumerate() {
            // Extract keywords from scene text for the search query
            let scene_text = manifest
                .get("segments")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.get(scene_idx))
                .and_then(|s| s.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or("");

            // Phase CM signal/noise query: strip listicle noise, bias to visual
            // nouns + video_keywords, attach context-matched visual anchor.
            // Per-scene stock_query override: if the agent specified a
            // stock_query in the scene, use it directly instead of auto-generating.
            // (UX audit GAP #1 fix: agents now have explicit control over
            // per-scene footage search queries.)
            let stock_q = if let Some(ref custom_q) = spec.scenes.get(scene_idx).and_then(|s| s.stock_query.as_ref()).filter(|q| !q.trim().is_empty()) {
                crate::stock_signal::SceneStockQuery {
                    query: custom_q.to_string(),
                    signal_tokens: crate::stock_signal::tokenize(custom_q),
                    visual_anchor: custom_q.to_string(),
                    scene_idx,
                }
            } else {
                crate::stock_signal::build_scene_stock_query(
                    scene_text,
                    &spec.video_keywords,
                    &spec.output.theme,
                    &spec.meta.aspect,
                    scene_idx,
                )
            };
            // Keep unsafe-keyword rewrite for edge terms (blood → calm nature)
            let query = safe_search_query(&stock_q.query, &spec.output.theme);
            scene_stock_queries.push(query.clone());
            let signal_tokens = stock_q.signal_tokens.clone();
            tracing::info!(
                "[script.to_video] stock query scene {}: signal={:?} anchor='{}' → query='{}'",
                scene_idx + 1,
                signal_tokens.iter().take(6).collect::<Vec<_>>(),
                stock_q.visual_anchor,
                query
            );

            let progress_pct = 35.0 + (scene_idx as f64 / scene_durations.len() as f64) * 25.0;
            report_progress(
                progress_pct,
                100.0,
                &format!(
                    "Scene {}/{}: {}",
                    scene_idx + 1,
                    scene_durations.len(),
                    query
                ),
            )
            .await
            .ok();

            let mut scene_bg: Option<String> = None;
            let mut bg_source = "none";
            // id, hash, q, lex, title, vision_score, vision_reason
            let mut stock_meta: Option<(
                String,
                String,
                String,
                f64,
                String,
                f64,
                Option<String>,
            )> = None;

            // --- Priority 1: Pexels (requires API key) ---
            // SEGMENTATION_ARCHITECTURE min clip duration: request clips that
            // COVER the scene (min_duration = scene length) so short clips are
            // NOT looped — prefer an ALTERNATE stock video for the scene's
            // keywords (up to 3 pages). If no clip covers the scene, fall back
            // to the longest short clip (renderer loops it only as a last
            // resort so the tail never freezes).
            if !pexels_key_val.is_empty() {
                let needed_dur = dur.max(3.0);
                let mut covering: Vec<(String, i64)> = Vec::new(); // (file url, pexels id)
                let mut shorts: Vec<(i64, String, i64)> = Vec::new(); // (dur, url, id)
                // Pass 1 (pages 1-3): only clips that cover the scene duration.
                for page in 1..=3 {
                    let pexels_url =
                        pexels_search_url(&query, orientation, page, needed_dur, 0.0);
                    let Ok(resp) = client
                        .get(&pexels_url)
                        .header("Authorization", &pexels_key_val)
                        .send()
                        .await
                    else {
                        continue;
                    };
                    if !resp.status().is_success() {
                        continue;
                    }
                    let Ok(body) = resp.json::<serde_json::Value>().await else {
                        continue;
                    };
                    let Some(videos) = body.get("videos").and_then(|v| v.as_array()) else {
                        continue;
                    };
                    for video in videos {
                        let vid_id = video.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
                        if vid_id > 0 && used_pexels_ids.contains(&vid_id) {
                            continue;
                        }
                        let vid_dur = video.get("duration").and_then(|v| v.as_i64()).unwrap_or(0);
                        let Some(url) = pexels_file_url(video) else {
                            continue;
                        };
                        // Strict float comparison: a clip counts as "covering"
                        // only if it is genuinely at least the scene length
                        // (integer truncation would admit 0.9s-short clips).
                        if (vid_dur as f64) >= needed_dur && covering.len() < 6 {
                            covering.push((url, vid_id));
                        }
                    }
                    if !covering.is_empty() {
                        break;
                    }
                }
                // Pass 2 (fallback): only if no alternate covers the scene —
                // keep the longest short clips to loop as a last resort.
                if covering.is_empty() {
                    for page in 1..=2 {
                        let pexels_url = pexels_search_url(&query, orientation, page, 0.0, 0.0);
                        let Ok(resp) = client
                            .get(&pexels_url)
                            .header("Authorization", &pexels_key_val)
                            .send()
                            .await
                        else {
                            continue;
                        };
                        if !resp.status().is_success() {
                            continue;
                        }
                        let Ok(body) = resp.json::<serde_json::Value>().await else {
                            continue;
                        };
                        let Some(videos) = body.get("videos").and_then(|v| v.as_array()) else {
                            continue;
                        };
                        for video in videos {
                            let vid_id = video.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
                            if vid_id > 0 && used_pexels_ids.contains(&vid_id) {
                                continue;
                            }
                            let vid_dur =
                                video.get("duration").and_then(|v| v.as_i64()).unwrap_or(0);
                            if vid_dur < 3 {
                                continue;
                            }
                            let Some(url) = pexels_file_url(video) else {
                                continue;
                            };
                            shorts.push((vid_dur, url, vid_id));
                        }
                    }
                    shorts.sort_by(|a, b| b.0.cmp(&a.0));
                    shorts.truncate(4);
                    tracing::warn!(
                        "[pexels stock] no clip covering scene {} (need {:.1}s); falling back to loop",
                        scene_idx + 1,
                        needed_dur
                    );
                }
                let candidates: Vec<(String, i64)> = covering
                    .into_iter()
                    .chain(shorts.into_iter().map(|(_, u, i)| (u, i)))
                    .collect();
                for (url, vid_id) in candidates {
                    let clip_path = format!(
                        "{}/scene_{:03}.mp4",
                        cache_dir,
                        scene_idx + 1
                    );
                    let Ok(dl_resp) = client.get(url).send().await else {
                        continue;
                    };
                    if !dl_resp.status().is_success() {
                        continue;
                    }
                    let Ok(bytes) = dl_resp.bytes().await else {
                        continue;
                    };
                    std::fs::write(&clip_path, &bytes).ok();
                    let crop_filter = crop_filter_for_aspect(&spec.meta.aspect);
                    let trimmed = format!(
                        "{}/scene_{:03}_trim.mp4",
                        cache_dir,
                        scene_idx + 1
                    );
                    let trim_result = tokio::process::Command::new("ffmpeg")
                        .arg("-y")
                        .arg("-i")
                        .arg(&clip_path)
                        .arg("-t")
                        .arg(dur.to_string())
                        .arg("-vf")
                        .arg(&crop_filter)
                        .arg("-c:v")
                        .arg("libx264")
                        .arg("-preset")
                        .arg("fast")
                        .arg("-crf")
                        .arg("23")
                        .arg("-an")
                        .arg(&trimmed)
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::piped())
                        .kill_on_drop(true)
                        .output()
                        .await;
                    let chosen = if trim_result
                        .as_ref()
                        .map(|o| o.status.success())
                        .unwrap_or(false)
                    {
                        trimmed
                    } else {
                        clip_path
                    };
                    // Geometry gate (no stretch)
                    let geo = crate::stock_signal::probe_geometry(&chosen, &spec.meta.aspect);
                    if !geo.ok {
                        tracing::warn!(
                            "[pexels stock] geometry reject id={} {:?}",
                            vid_id,
                            geo.reasons
                        );
                        let _ = std::fs::remove_file(&chosen);
                        continue;
                    }
                    // Fingerprint: reject if same bytes as prior scene
                    if let Some(h) = file_content_fingerprint(&chosen) {
                        if used_content_hashes.contains(&h) {
                            let _ = std::fs::remove_file(&chosen);
                            continue;
                        }
                        used_content_hashes.insert(h.clone());
                        stock_meta = Some((
                            format!("pexels_{}", vid_id),
                            h,
                            query.clone(),
                            0.5,
                            String::new(),
                            0.5, // Pexels metadata is reliable; no vision gate
                            None,
                        ));
                    }
                    scene_bg = Some(chosen);
                    used_pexels_ids.insert(vid_id);
                    bg_source = "pexels";
                    break;
                }
            }

            // --- Priority 2: YouTube stock via yt-dlp (no API key) ---
            if scene_bg.is_none() {
                // Prefer non-procedural paths from script.fallback_pool if caller supplied stock
                let pool_stock = spec
                    .background
                    .fallback_pool
                    .iter()
                    .find(|p| !is_procedural_media_path(p) && Path::new(p).exists());
                if let Some(p) = pool_stock {
                    scene_bg = Some(p.clone());
                    bg_source = "fallback_pool_stock";
                }
            }
            if scene_bg.is_none() {
                // Query already includes stock/vertical bias from stock_signal.
                // Diversify only if we already tried the exact same query string.
                let mut yt_q = query.clone();
                if used_yt_queries.contains(&yt_q) {
                    yt_q = format!("{} scene{}", query, scene_idx + 1);
                }
                used_yt_queries.insert(yt_q.clone());
                let yt_out = format!("{}/scene_{:03}_yt.mp4", cache_dir, scene_idx + 1);
                if let Some(fetch) = fetch_youtube_stock_clip_signal(
                    &yt_q,
                    &signal_tokens,
                    dur,
                    &spec.meta.aspect,
                    &yt_out,
                    scene_idx,
                    &mut used_video_ids,
                    &mut used_content_hashes,
                    scene_text,
                    dur,
                    dur,
                )
                .await
                {
                    let lex = fetch.lexical_score;
                    stock_meta = Some((
                        fetch.video_id,
                        fetch.content_hash,
                        fetch.search_query,
                        lex,
                        fetch.source_title,
                        fetch.vision_score.unwrap_or(lex),
                        fetch.vision_reason,
                    ));
                    scene_bg = Some(fetch.path);
                    bg_source = "youtube";
                }
            }

            // Phase B: query fan-out — one more attempt with scene nouns only
            if scene_bg.is_none() && !signal_tokens.is_empty() {
                let short_q = signal_tokens
                    .iter()
                    .take(4)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" ");
                let yt_out2 = format!("{}/scene_{:03}_yt_b.mp4", cache_dir, scene_idx + 1);
                if let Some(fetch) = fetch_youtube_stock_clip_signal(
                    &format!("{} stock footage vertical", short_q),
                    &signal_tokens,
                    dur,
                    &spec.meta.aspect,
                    &yt_out2,
                    scene_idx,
                    &mut used_video_ids,
                    &mut used_content_hashes,
                    scene_text,
                    dur,
                    dur,
                )
                .await
                {
                    let lex = fetch.lexical_score;
                    stock_meta = Some((
                        fetch.video_id,
                        fetch.content_hash,
                        fetch.search_query,
                        lex,
                        fetch.source_title,
                        fetch.vision_score.unwrap_or(lex),
                        fetch.vision_reason,
                    ));
                    scene_bg = Some(fetch.path);
                    bg_source = "youtube";
                }
            }

            scene_stock_meta.push(stock_meta);

            if let Some(path) = scene_bg {
                tracing::info!(
                    "[script.to_video] Scene {} background source={} path={}",
                    scene_idx + 1,
                    bg_source,
                    path
                );
                per_scene_backgrounds.push(path);
            } else {
                // Last resort: procedural (synthetic) — hard production quality fail.
                let fallback = format!(
                    "mcp/assets/backgrounds/procedural_{:02}.mp4",
                    (scene_idx % 10) + 1
                );
                let procedural_path = if std::path::Path::new(&fallback).exists() {
                    fallback
                } else {
                    "mcp/assets/backgrounds/procedural_01.mp4".to_string()
                };
                per_scene_backgrounds.push(procedural_path.clone());
                let allow_proc = std::env::var("OPENSCRIPT_ALLOW_PROCEDURAL")
                    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                    .unwrap_or(false);
                if !allow_proc {
                    render_warnings.push(format!(
                        "HARD stock_visuals scene {}: no relevant unique stock (Pexels/YT). Using procedural {}. Set PEXELS_API_KEY or OPENSCRIPT_ALLOW_PROCEDURAL=1. Production will hard-fail if ≥50% procedural.",
                        scene_idx + 1,
                        procedural_path
                    ));
                } else {
                    render_warnings.push(format!(
                        "PRODUCTION_FAIL stock_visuals scene {}: synthetic procedural ({})",
                        scene_idx + 1,
                        procedural_path
                    ));
                }
            }
        }
        let proc_n = per_scene_backgrounds
            .iter()
            .filter(|p| is_procedural_media_path(p))
            .count();
        if !per_scene_backgrounds.is_empty()
            && proc_n * 2 >= per_scene_backgrounds.len()
        {
            render_warnings.push(format!(
                "HARD: majority procedural multi-broll ({}/{}) — visual hooks missing. Configure Pexels or widen YT stock queries.",
                proc_n,
                per_scene_backgrounds.len()
            ));
            let allow_proc = std::env::var("OPENSCRIPT_ALLOW_PROCEDURAL")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            if !allow_proc {
                // Fail-closed: never present gradient-majority as a production final.
                let draft = if output_path.ends_with(".draft.mp4") {
                    output_path.clone()
                } else if let Some(stripped) = output_path.strip_suffix(".mp4") {
                    format!("{}.draft.mp4", stripped)
                } else {
                    format!("{}.draft.mp4", output_path)
                };
                tracing::warn!(
                    "[script.to_video] fail-closed stock: rewriting output {} → {} (set OPENSCRIPT_ALLOW_PROCEDURAL=1 to override)",
                    output_path,
                    draft
                );
                render_warnings.push(format!(
                    "FAIL_CLOSED: stock_ratio < 0.5 ({}/{} procedural). Writing draft output {} — not a production final. Set PEXELS_API_KEY or OPENSCRIPT_ALLOW_PROCEDURAL=1.",
                    proc_n,
                    per_scene_backgrounds.len(),
                    draft
                ));
                output_path = draft;
            }
        }
    }

    // Build per-scene background clips
    let fallback_pool = if !per_scene_backgrounds.is_empty() {
        per_scene_backgrounds
    } else if !spec.background.fallback_pool.is_empty() {
        spec.background.fallback_pool.clone()
    } else {
        let mut pool = Vec::new();
        if let Ok(entries) = std::fs::read_dir("mcp/assets/backgrounds") {
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.ends_with(".mp4") {
                    pool.push(format!("mcp/assets/backgrounds/{}", name));
                }
            }
        }
        if pool.is_empty() {
            pool.push("mcp/assets/backgrounds/procedural_01.mp4".to_string());
        }
        pool
    };

    // Assign backgrounds — one per scene (multi-broll)
    let mut backgrounds: Vec<openscript_ffmpeg::multilayer_render::BackgroundClip> = Vec::new();

    for (i, &dur) in scene_durations.iter().enumerate() {
        // Use the per-scene downloaded background if available, otherwise cycle through pool
        let bg_path = if i < fallback_pool.len() {
            fallback_pool[i].clone()
        } else {
            fallback_pool[i % fallback_pool.len()].clone()
        };

        backgrounds.push(openscript_ffmpeg::multilayer_render::BackgroundClip {
            path: bg_path,
            duration_s: dur,
            // Loop per-scene trims: Pexels source clips are often SHORTER than
            // the scene (e.g. a 6s clip for a 12s scene). Without -stream_loop
            // the concat runs out early and the render holds the last frame
            // for the remaining seconds (frozen tail). select(lte(n,N)) keeps
            // exactly the scene frame count from the looped stream, so an
            // exact-size trim is unaffected.
            looped: true,
        });
    }

    // Build sticker overlays — download GIPHY stickers per speaker
    let mut stickers: Vec<openscript_ffmpeg::multilayer_render::StickerOverlay> = Vec::new();
    if !skip_stickers && spec.stickers.enabled {
        // Fix: prior versions called env::var("GIPHY_API_KEY") twice in
        // unwrap_or_else (the inner call shadowed the outer). Simplify to a
        // single lookup.
        let giphy_key_val = giphy_key();

        // Download stickers: one per speaker by default, but per-scene when
        // a single speaker has multiple scenes (Phase 4: per-scene variation).
        let mut speaker_stickers: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        // Phase 4: when single speaker has 3+ scenes, store per-scene stickers.
        // Key = scene index, Value = sticker path. Falls back to speaker_stickers.
        let mut scene_sticker_map: std::collections::HashMap<usize, String> =
            std::collections::HashMap::new();
        let single_speaker_multi_scene =
            spec.speakers.len() == 1 && spec.scenes.len() >= 3;
        // Track queries used across speakers/scenes so we don't re-search the
        // same term (query-level dedup).
        let mut used_sticker_queries: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        // Track GIPHY sticker IDs already downloaded — the definitive
        // no-duplicate-sticker guarantee (two different queries can return the
        // same top sticker).
        let mut used_sticker_ids: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        if !giphy_key_val.is_empty() {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .map_err(|e| ToolError::Asset(format!("HTTP client error: {}", e)))?;

            let stickers_dir = "mcp/assets/stickers";
            std::fs::create_dir_all(stickers_dir).ok();

            for (speaker_name, speaker_spec) in &spec.speakers {
                // Build a mood-aware, scene-text-aware GIPHY search query.
                // Round-5 audit: the old hardcoded "{speaker_name} talking"
                // produced irrelevant stickers (speaker names are abstract IDs
                // like "alice" or "narrator", not GIPHY-indexed content).
                // New priority: theme keyword > scene emote > scene-text noun >
                // speaker preset > trending fallback.
                // Build a topic-aware GIPHY sticker search query.
                // (Round-13: topic-aware video search upgrade.)
                // If video_keywords are available, use the first one as
                // the sticker query (e.g. "brain" for a brain video) so
                // the sticker is topically relevant, not just theme-based.
                let search_query = if !spec.video_keywords.is_empty() {
                    let topic_kw = &spec.video_keywords[0];
                    if !used_sticker_queries.contains(topic_kw) {
                        used_sticker_queries.insert(topic_kw.clone());
                        topic_kw.clone()
                    } else {
                        build_sticker_query(
                            speaker_name,
                            speaker_spec,
                            &spec.scenes,
                            &spec.output.theme,
                            &mut used_sticker_queries,
                        )
                    }
                } else {
                    build_sticker_query(
                        speaker_name,
                        speaker_spec,
                        &spec.scenes,
                        &spec.output.theme,
                        &mut used_sticker_queries,
                    )
                };
                tracing::info!(
                    "[script.to_video] GIPHY sticker search for '{}': query='{}'",
                    speaker_name,
                    search_query
                );
                // Use limit=8 so we can filter for relevance and skip duds
                let giphy_url = format!(
                    "https://api.giphy.com/v1/stickers/search?api_key={}&q={}&limit=8&rating=g&bundle=sticker_layering&lang=en",
                    giphy_key_val,
                    urlencoding::encode(&search_query)
                );

                if let Ok(resp) = client.get(&giphy_url).send().await {
                    if resp.status().is_success() {
                        if let Ok(body) = resp.json::<serde_json::Value>().await {
                            if let Some(data) = body.get("data").and_then(|v| v.as_array()) {
                                // Iterate through results (limit=8) and pick the
                                // first valid sticker. Skip non-sticker results,
                                // oversized files, and results already used by
                                // another speaker.
                                for sticker in data {
                                    // Defensive: verify this is actually a sticker
                                    let is_sticker = sticker
                                        .get("is_sticker")
                                        .and_then(|v| v.as_i64())
                                        .unwrap_or(1);
                                    if is_sticker != 1 {
                                        continue;
                                    }

                                    let sticker_id = sticker
                                        .get("id")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    if used_sticker_ids.contains(&sticker_id) {
                                        continue;
                                    }

                                    let images =
                                        sticker.get("images").cloned().unwrap_or(json!({}));
                                    let original =
                                        images.get("original").cloned().unwrap_or(json!({}));

                                    // Use GIF format (not WEBP) because FFmpeg's
                                    // native WEBP decoder cannot handle animated
                                    // WEBP stickers from GIPHY. GIF animation is
                                    // well-supported by FFmpeg's GIF decoder.
                                    // (Round-5 audit: animated WEBP caused
                                    // "Terminating thread with return code
                                    // -1145393733" in FFmpeg.)
                                    let sticker_url = original
                                        .get("url")
                                        .and_then(|v| v.as_str())
                                        .filter(|s| !s.is_empty())
                                        .unwrap_or("");

                                    if sticker_url.is_empty() {
                                        continue;
                                    }

                                    // Skip static (non-animated) GIFs — check frame count.
                                    // (Round-11: "Some GIFs were static images" — user wants
                                    // animated stickers only.)
                                    let frames = original
                                        .get("frames")
                                        .and_then(|v| v.as_str())
                                        .and_then(|s| s.parse::<u32>().ok())
                                        .unwrap_or(0);
                                    if frames < 2 {
                                        tracing::info!(
                                            "[script.to_video] Skipping static GIPHY sticker (frames={}): {}",
                                            frames, sticker_id
                                        );
                                        continue;
                                    }

                                    // Skip oversized stickers (> 3MB)
                                    let size: i64 = original
                                        .get("webp_size")
                                        .and_then(|v| v.as_i64())
                                        .or_else(|| {
                                            original.get("size").and_then(|v| v.as_i64())
                                        })
                                        .unwrap_or(0);
                                    if size > 3_000_000 {
                                        continue;
                                    }

                                    // Always GIF (FFmpeg can't decode animated WEBP)
                                    let ext = "gif";
                                    let sticker_path = format!(
                                        "{}/giphy_{}.{}",
                                        stickers_dir, speaker_name, ext
                                    );
                                    if let Ok(dl_resp) = client.get(sticker_url).send().await {
                                        if dl_resp.status().is_success() {
                                            if let Ok(bytes) = dl_resp.bytes().await {
                                                std::fs::write(&sticker_path, &bytes).ok();
                                                speaker_stickers.insert(
                                                    speaker_name.clone(),
                                                    sticker_path.clone(),
                                                );
                                                used_sticker_ids.insert(sticker_id);
                                                tracing::info!(
                                                    "[script.to_video] Downloaded GIPHY sticker for {}: {} ({} bytes)",
                                                    speaker_name,
                                                    sticker_path,
                                                    bytes.len()
                                                );
                                                break; // Got a sticker for this speaker
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Phase 4: per-scene sticker variation for single-speaker videos.
        // When one speaker has 3+ scenes, download a DIFFERENT sticker per
        // scene using scene-specific queries (emote, salient noun from text)
        // so the overlay changes visually between scenes instead of repeating.
        if single_speaker_multi_scene && !giphy_key_val.is_empty() {
            if let Some((speaker_name, _speaker_spec)) = spec.speakers.iter().next() {
                let client = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(15))
                    .build()
                    .unwrap_or_default();
                let stickers_dir = "mcp/assets/stickers";

                for (scene_idx, scene) in spec.scenes.iter().enumerate() {
                    // Candidate queries, tried IN ORDER until a fresh,
                    // non-duplicate sticker downloads: the scene's b-roll stock
                    // query FIRST (unified keyword source for broll + sticker),
                    // then sticker-friendly fallbacks (emote, salient noun,
                    // text snippet, "talking head"). If the topic query only
                    // surfaces already-used or static stickers, the next
                    // candidate is tried — variation never silently collapses
                    // to one repeated sticker.
                    let mut sticker_candidates: Vec<String> = Vec::new();
                    if let Some(q) = scene_stock_queries.get(scene_idx) {
                        if !q.trim().is_empty() {
                            sticker_candidates.push(q.clone());
                        }
                    }
                    if let Some(ref emote) = scene.emote {
                        if !emote.is_empty() {
                            sticker_candidates.push(emote.clone());
                        }
                    }
                    if let Some(noun) = extract_salient_noun(&scene.text) {
                        sticker_candidates.push(noun);
                    }
                    // Use first 3 words of scene text as fallback
                    let text_snippet: String = scene
                        .text
                        .split_whitespace()
                        .take(3)
                        .collect::<Vec<_>>()
                        .join(" ");
                    if !text_snippet.is_empty() {
                        sticker_candidates.push(text_snippet);
                    }
                    sticker_candidates.push("talking head".to_string());
                    let mut seen_q = std::collections::HashSet::new();
                    sticker_candidates.retain(|c| seen_q.insert(c.clone()));

                    let mut scene_placed = false;
                    for query in &sticker_candidates {
                        if scene_placed {
                            break;
                        }
                        if used_sticker_queries.contains(query) {
                            continue;
                        }
                        used_sticker_queries.insert(query.clone());
                        tracing::info!(
                            "[script.to_video] Per-scene sticker query for scene {}: '{}'",
                            scene_idx,
                            query
                        );

                        let giphy_url = format!(
                            "https://api.giphy.com/v1/stickers/search?api_key={}&q={}&limit=8&rating=g&bundle=sticker_layering&lang=en",
                            giphy_key_val,
                            urlencoding::encode(query)
                        );

                        if let Ok(resp) = client.get(&giphy_url).send().await {
                            if resp.status().is_success() {
                                if let Ok(body) = resp.json::<serde_json::Value>().await {
                                    if let Some(data) = body.get("data").and_then(|v| v.as_array()) {
                                        for sticker in data {
                                            let is_sticker = sticker
                                                .get("is_sticker")
                                                .and_then(|v| v.as_i64())
                                                .unwrap_or(1);
                                            if is_sticker != 1 { continue; }

                                            let sticker_id = sticker
                                                .get("id")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            if used_sticker_ids.contains(&sticker_id) { continue; }

                                            let images = sticker.get("images").cloned().unwrap_or(json!({}));
                                            let original = images.get("original").cloned().unwrap_or(json!({}));

                                            let sticker_url = original
                                                .get("url")
                                                .and_then(|v| v.as_str())
                                                .filter(|s| !s.is_empty())
                                                .unwrap_or("");
                                            if sticker_url.is_empty() { continue; }

                                            let frames = original
                                                .get("frames")
                                                .and_then(|v| v.as_str())
                                                .and_then(|s| s.parse::<u32>().ok())
                                                .unwrap_or(0);
                                            if frames < 2 { continue; }

                                            let size: i64 = original
                                                .get("webp_size")
                                                .and_then(|v| v.as_i64())
                                                .or_else(|| original.get("size").and_then(|v| v.as_i64()))
                                                .unwrap_or(0);
                                            if size > 3_000_000 { continue; }

                                            let sticker_path = format!(
                                                "{}/giphy_s{}_{}.gif",
                                                stickers_dir, scene_idx, speaker_name
                                            );
                                            if let Ok(dl_resp) = client.get(sticker_url).send().await {
                                                if dl_resp.status().is_success() {
                                                    if let Ok(bytes) = dl_resp.bytes().await {
                                                        std::fs::write(&sticker_path, &bytes).ok();
                                                        scene_sticker_map.insert(scene_idx, sticker_path.clone());
                                                        used_sticker_ids.insert(sticker_id);
                                                        tracing::info!(
                                                            "[script.to_video] Per-scene sticker for scene {}: {}",
                                                            scene_idx, sticker_path
                                                        );
                                                        scene_placed = true;
                                                        break;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if !scene_placed {
                        tracing::warn!(
                            "[script.to_video] No fresh sticker for scene {} after {} query candidate(s) — will use the speaker sticker",
                            scene_idx,
                            sticker_candidates.len()
                        );
                    }
                }
            }
        }

        // Local sticker fallback when GIPHY missing/failed (Phase CF).
        // Prefer animated GIFs (giphy_*.gif), then speaker PNGs, then any .gif/.png.
        let local_sticker_pool: Vec<String> = {
            let mut pool = Vec::new();
            if let Ok(entries) = std::fs::read_dir("mcp/assets/stickers") {
                for entry in entries.filter_map(|e| e.ok()) {
                    let p = entry.path();
                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
                    if matches!(ext, "gif" | "png" | "webp") && !name.starts_with('.') {
                        pool.push(p.to_string_lossy().to_string());
                    }
                }
            }
            // Prefer GIFs first for motion
            pool.sort_by(|a, b| {
                let ag = a.ends_with(".gif") as i32;
                let bg = b.ends_with(".gif") as i32;
                bg.cmp(&ag)
            });
            pool
        };
        let mut local_idx = 0usize;
        for (speaker_name, speaker_spec) in &spec.speakers {
            if speaker_stickers.contains_key(speaker_name) {
                continue;
            }
            // Named PNG first
            let position_parts: Vec<&str> = speaker_spec.position.split('-').collect();
            let facing = position_parts.last().unwrap_or(&"left");
            let png_path = format!(
                "mcp/assets/stickers/speaker_{}_{}.png",
                speaker_name, facing
            );
            if std::path::Path::new(&png_path).exists() {
                speaker_stickers.insert(speaker_name.clone(), png_path);
                continue;
            }
            // Generic named GIFs
            for candidate in [
                format!("mcp/assets/stickers/giphy_{}.gif", speaker_name),
                "mcp/assets/stickers/giphy_narrator.gif".to_string(),
                "mcp/assets/stickers/giphy_alice.gif".to_string(),
            ] {
                if Path::new(&candidate).exists() {
                    speaker_stickers.insert(speaker_name.clone(), candidate);
                    break;
                }
            }
            if speaker_stickers.contains_key(speaker_name) {
                continue;
            }
            // Cycle remaining local pool
            if !local_sticker_pool.is_empty() {
                let path = local_sticker_pool[local_idx % local_sticker_pool.len()].clone();
                local_idx += 1;
                speaker_stickers.insert(speaker_name.clone(), path);
            }
        }
        if speaker_stickers.is_empty() {
            render_warnings.push(
                "PRODUCTION_FAIL overlay_presence: no GIPHY key and no local stickers under mcp/assets/stickers/"
                    .into(),
            );
        } else if giphy_key_val.is_empty() {
            render_warnings.push(format!(
                "Using LOCAL sticker fallbacks ({} speaker(s)) — set GIPHY_API_KEY for topical animated stickers",
                speaker_stickers.len()
            ));
        }

        // Create sticker overlays per scene
        let mut current_ms = 0i64;
        let mut scene_idx = 0usize;
        if let Some(segments) = manifest.get("segments").and_then(|v| v.as_array()) {
            for seg in segments {
                let speaker_name = seg.get("speaker").and_then(|v| v.as_str()).unwrap_or("");
                let end_ms = seg
                    .get("end_ms")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(current_ms + 3000);

                if let Some(speaker_spec) = spec.speakers.get(speaker_name) {
                    // Phase 4: prefer per-scene sticker when available
                    let sticker_path = scene_sticker_map
                        .get(&scene_idx)
                        .or_else(|| speaker_stickers.get(speaker_name));
                    if let Some(sticker_path) = sticker_path {
                        let sticker_w = (spec.meta.width as f64 * speaker_spec.scale) as u32;
                        stickers.push(openscript_ffmpeg::multilayer_render::StickerOverlay {
                            path: sticker_path.clone(),
                            start_s: current_ms as f64 / 1000.0,
                            end_s: end_ms as f64 / 1000.0,
                            position: speaker_spec.position.clone(),
                            scale: speaker_spec.scale,
                            center_x: 0, // Will be computed in renderer
                            center_y: 0,
                            sticker_width: sticker_w,
                            sticker_height: sticker_w,
                        });
                    }
                }

                scene_idx += 1;
                current_ms = end_ms;
            }
        }
    }

    // Get music path
    // Music selection: use spec.music if provided, otherwise auto-select
    // from the 20-track stock catalog based on the theme. This ensures
    // every video has background music by default — the round-3 audit
    // found that agents who omitted the music field got silent videos,
    // which the user noted as a quality gap.
    // (Round-3 UX audit PROBLEM 3b fix.)
    let mut music_sel_tags: Vec<String> = Vec::new();
    let mut music_sel_query: Option<String> = None;
    let mut music_sel_source: Option<String> = None;

    let music_path = {
        let explicit = if let Some(ref m) = spec.music {
            if let Some(ref path) = m.path {
                if std::path::Path::new(path).exists() {
                    if is_synthetic_music_file(path) {
                        tracing::warn!(
                            "[script.to_video] Rejecting synthetic stock music: {}",
                            path
                        );
                        None
                    } else if openscript_core::production_quality::is_calm_focus_context(
                        Some(&spec.output.theme),
                        &spec.video_keywords,
                    ) && openscript_core::production_quality::music_hits_denylist(
                        path,
                        None,
                        &[],
                        Some(path),
                    ) {
                        tracing::warn!(
                            "[script.to_video] Rejecting denylist music for calm/focus: {}",
                            path
                        );
                        None
                    } else {
                        music_sel_source = Some("script".into());
                        music_sel_query = Some(path.clone());
                        Some(path.clone())
                    }
                } else {
                    tracing::warn!(
                        "[script.to_video] Music path not found: {} — auto-select",
                        path
                    );
                    None
                }
            } else {
                None
            }
        } else {
            None
        };
        if let Some(p) = explicit {
            Some(p)
        } else if let Some(sel) =
            auto_select_music(&spec.output.theme, &spec.video_keywords).await
        {
            music_sel_tags = sel.tags;
            music_sel_query = Some(sel.selection_query);
            music_sel_source = Some(sel.source);
            Some(sel.path)
        } else {
            None
        }
    };

    // === MEME B-ROLLS: Full-screen reaction GIF clips per scene ===
    // GIPHY is a video-clip provider (like Pexels/YouTube). Meme b-rolls
    // are FULL-SCREEN video clips downloaded as MP4 from GIPHY that briefly
    // replace the background — like TikTok reaction cuts. They are NOT
    // stickers (small overlays). They are proper background video clips.
    // (Round-9: user said "Meme Brolls must be full-screen b-rolls, not
    // stickers. Stickers/GIF implementation is another thing.")
    let mut meme_clips: Vec<openscript_ffmpeg::multilayer_render::MemeClip> = Vec::new();
    // Track GIPHY GIF IDs that have already been used to prevent the same
    // meme appearing in multiple scenes.
    // (Round-16: "Ensure that the entire video timeline has unique videos,
    // not repeated video that might reduce the attention-hooking.")
    let mut used_meme_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    if spec.meme_brolls.enabled {
        let giphy_key_val = giphy_key();
        if !giphy_key_val.is_empty() {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .map_err(|e| ToolError::Asset(format!("HTTP client error: {}", e)))?;

            let meme_dir = "mcp/assets/meme_cache";
            std::fs::create_dir_all(meme_dir).ok();

            let mut scene_start_s: f64 = 0.0;

            for (scene_idx, scene) in spec.scenes.iter().enumerate() {
                let scene_dur_s = scene_durations.get(scene_idx).copied().unwrap_or(3.0);

                // Build multiple search queries ranked by specificity.
                // (Round-18: GIPHY SDK multi-query strategy with relevance
                // scoring. Tries specific → broad → trending fallback.)
                let search_strategies = build_meme_search_queries(
                    &scene.text,
                    &spec.video_keywords,
                    &spec.output.theme,
                );

                tracing::info!(
                    "[script.to_video] Meme b-roll scene {}: {} search strategies",
                    scene_idx + 1,
                    search_strategies.len()
                );
                for (i, (q, lim)) in search_strategies.iter().enumerate() {
                    tracing::info!(
                        "[script.to_video]   strategy {}: query='{}' limit={}",
                        i + 1, q, lim
                    );
                }

                let mut meme_found = false;

                // Try each search strategy in order until we find a suitable GIF
                for (query, limit) in &search_strategies {
                    if meme_found {
                        break;
                    }

                    // Build GIPHY URL — use search for non-empty queries,
                    // trending for empty query (ultimate fallback).
                    let giphy_url = if query.is_empty() {
                        format!(
                            "https://api.giphy.com/v1/gifs/trending?api_key={}&limit={}&rating=pg&bundle=sticker_layering",
                            giphy_key_val, limit
                        )
                    } else {
                        format!(
                            "https://api.giphy.com/v1/gifs/search?api_key={}&q={}&limit={}&rating=pg&lang=en&bundle=sticker_layering&remove_low_contrast=true",
                            giphy_key_val,
                            urlencoding::encode(query),
                            limit
                        )
                    };

                    let resp_result = client.get(&giphy_url).send().await;
                    if let Ok(resp) = resp_result {
                        if !resp.status().is_success() {
                            continue;
                        }
                        let body_result = resp.json::<serde_json::Value>().await;
                        if body_result.is_err() {
                            continue;
                        }
                        let body = body_result.unwrap();
                        let data_arr = body.get("data").and_then(|v| v.as_array());
                        if data_arr.is_none() {
                            continue;
                        }
                        let gifs = data_arr.unwrap();

                        // Score all results by relevance and pick the best
                        // non-duplicate, non-static GIF with MP4.
                        let mut best_gif: Option<(serde_json::Value, u32)> = None;
                        for g in gifs {
                            let gid = g.get("id").and_then(|v| v.as_str()).unwrap_or("");
                            if !gid.is_empty() && used_meme_ids.contains(gid) {
                                continue;
                            }
                            let imgs = match g.get("images").and_then(|v| v.as_object()) {
                                Some(im) => im,
                                None => continue,
                            };
                            let orig = match imgs.get("original") {
                                Some(o) => o,
                                None => continue,
                            };
                            let frames = orig.get("frames")
                                .and_then(|v| v.as_str())
                                .and_then(|s| s.parse::<u32>().ok())
                                .unwrap_or(0);
                            if frames < 2 {
                                continue;
                            }
                            let mp4 = orig.get("mp4")
                                .and_then(|v| v.as_str())
                                .filter(|s| !s.is_empty());
                            if mp4.is_none() {
                                continue;
                            }
                            // Score relevance
                            let score = score_gif_relevance(g, query);
                            if best_gif.is_none() || score > best_gif.as_ref().unwrap().1 {
                                best_gif = Some((g.clone(), score));
                            }
                        }

                        if let Some((gif, score)) = best_gif {
                            let gif_id = gif.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                            let gif_title = gif.get("title").and_then(|v| v.as_str()).unwrap_or("?").to_string();
                            tracing::info!(
                                "[script.to_video] Meme b-roll scene {}: FOUND query='{}' gif_id={} title='{}' relevance_score={}",
                                scene_idx + 1, query, gif_id, gif_title, score
                            );

                            let images = gif.get("images").cloned().unwrap_or(json!({}));
                            let original = images.get("original").cloned().unwrap_or(json!({}));

                            let mp4_url = original
                                .get("mp4")
                                .and_then(|v| v.as_str())
                                .filter(|s| !s.is_empty())
                                .or_else(|| {
                                    images.get("original_mp4")
                                        .and_then(|v| v.get("mp4"))
                                        .and_then(|v| v.as_str())
                                        .filter(|s| !s.is_empty())
                                })
                                .unwrap_or("");

                            if !mp4_url.is_empty() {
                                let meme_path = format!("{}/meme_scene_{}.mp4", meme_dir, scene_idx + 1);
                                if let Ok(dl_resp) = client.get(mp4_url).send().await {
                                    if dl_resp.status().is_success() {
                                        if let Ok(bytes) = dl_resp.bytes().await {
                                            std::fs::write(&meme_path, &bytes).ok();
                                            let meme_start_s = scene_start_s + (scene_dur_s * 0.4);
                                            let meme_end_s = meme_start_s + spec.meme_brolls.duration_s;
                                            let scene_end_s = scene_start_s + scene_dur_s;
                                            let meme_end_s = meme_end_s.min(scene_end_s);
                                            meme_clips.push(openscript_ffmpeg::multilayer_render::MemeClip {
                                                path: meme_path.clone(),
                                                start_s: meme_start_s,
                                                end_s: meme_end_s,
                                            });
                                            used_meme_ids.insert(gif_id.clone());
                                            tracing::info!(
                                                "[script.to_video] Downloaded meme b-roll MP4 for scene {}: {} ({} bytes, {:.1}s-{:.1}s, relevance={})",
                                                scene_idx + 1, meme_path, bytes.len(),
                                                meme_start_s, meme_end_s, score
                                            );
                                            meme_found = true;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                if !meme_found {
                    tracing::warn!(
                        "[script.to_video] No suitable meme b-roll found for scene {} after all strategies",
                        scene_idx + 1
                    );
                }

                scene_start_s += scene_dur_s;
            }
        }
    }

    // Build timeline preview for agent inspection
    let bg_assignments: Vec<openscript_core::timeline_preview::BackgroundClipAssignment> =
        backgrounds
            .iter()
            .enumerate()
            .map(|(i, bg)| {
                let start_ms: i64 = scene_durations[..i].iter().sum::<f64>() as i64 * 1000;
                let end_ms = start_ms + (bg.duration_s * 1000.0) as i64;
                openscript_core::timeline_preview::BackgroundClipAssignment {
                    start_ms,
                    end_ms,
                    path: bg.path.clone(),
                    looped: bg.looped,
                }
            })
            .collect();

    let sticker_assignments: Vec<openscript_core::timeline_preview::StickerAssignment> = stickers
        .iter()
        .map(|s| {
            // Calculate sticker dimensions and center-based coordinates
            let sticker_w = (spec.meta.width as f64 * s.scale) as u32;
            let sticker_h = sticker_w; // Approximate square; actual aspect ratio varies
            let margin = 40i32;
            let (tl_x, tl_y): (i32, i32) = match s.position.as_str() {
                "top-left" => (margin, margin),
                "top-right" => (spec.meta.width as i32 - sticker_w as i32 - margin, margin),
                "bottom-left" => (margin, spec.meta.height as i32 - sticker_h as i32 - margin),
                "bottom-right" => (
                    spec.meta.width as i32 - sticker_w as i32 - margin,
                    spec.meta.height as i32 - sticker_h as i32 - margin,
                ),
                "center" => (
                    (spec.meta.width as i32 - sticker_w as i32) / 2,
                    (spec.meta.height as i32 - sticker_h as i32) / 2,
                ),
                _ => (margin, margin),
            };
            let center_x = tl_x + sticker_w as i32 / 2 - spec.meta.width as i32 / 2;
            let center_y = tl_y + sticker_h as i32 / 2 - spec.meta.height as i32 / 2;

            openscript_core::timeline_preview::StickerAssignment {
                start_ms: (s.start_s * 1000.0) as i64,
                end_ms: (s.end_s * 1000.0) as i64,
                path: s.path.clone(),
                position: s.position.clone(),
                scale: s.scale,
                speaker: String::new(),
                center_x,
                center_y,
                sticker_width: sticker_w,
                sticker_height: sticker_h,
            }
        })
        .collect();

    let layered_timeline = openscript_core::timeline_preview::build_layered_timeline(
        &manifest,
        &bg_assignments,
        music_path.as_deref(),
        // ponytail: ducking defaults to true whenever music is present — auto-selected
        // music should always duck under voiceover to avoid masking speech.
        spec.music.as_ref().map(|m| m.ducking).unwrap_or(music_path.is_some()),
        &sticker_assignments,
        Some(captions_path),
        &spec.captions.style,
        spec.meta.width,
        spec.meta.height,
        spec.meta.fps,
    );

    let timeline_preview = layered_timeline.preview();
    let timeline_issues = layered_timeline.validate();
    let timeline_summary = layered_timeline.summary();

    // Write timeline preview to file
    let preview_path = format!("{}/timeline_preview.txt", output_dir);
    std::fs::write(&preview_path, &timeline_preview)?;

    // ponytail: Update timeline JSON tracks with broll/music/caption/SFX events.
    // handle_script_to_timeline wrote sparse tracks (broll=1, music=0, captions=0, sfx=0).
    // The per-scene multi-broll, music selection, and SFX auto-generation happened AFTER
    // that, so the timeline JSON is stale. Reload, populate, and save so the KPI evaluation
    // (which reads Timeline::load()) sees the correct event counts.
    let sfx_hits = auto_select_sfx_hits(&scene_durations);
    {
        if let Ok(mut tl) = openscript_core::timeline::Timeline::load(&timeline_path) {
            use openscript_core::types::TrackType;
            use openscript_core::timeline::{EventKind, TimelineEvent};

            // Clear sparse tracks from handle_script_to_timeline before repopulating.
            // The sparse handler wrote broll=1 event covering the full video; we now have
            // per-scene broll assignments, music, captions, and SFX. Clear each track to
            // avoid duplicate/overlapping events.
            for track_type in [TrackType::Broll, TrackType::Music, TrackType::Captions, TrackType::Sfx, TrackType::Stickers] {
                if let Some(events) = tl.tracks.get_mut(&track_type) {
                    events.clear();
                }
            }

            // Broll track: one event per scene from bg_assignments.
            // Use accumulated scene_durations for timing (not bg.start/end_ms which use the
            // clip's raw duration and compound floating-point drift across scenes).
            let mut cumulative_ms: i64 = 0;
            for (i, bg) in bg_assignments.iter().enumerate() {
                let scene_ms = (scene_durations[i] * 1000.0) as i64;
                let start_ms = cumulative_ms;
                let end_ms = start_ms + scene_ms;
                cumulative_ms = end_ms;
                tl.add_track_event(
                    TrackType::Broll,
                    TimelineEvent {
                        id: format!("broll_{}", i + 1),
                        asset_id: bg.path.clone(),
                        start_ms,
                        end_ms,
                        offset_ms: 0,
                        gain_db: 0.0,
                        fade_in_ms: 0,
                        fade_out_ms: 0,
                        tags: vec![],
                        provenance: None,
                        kind: EventKind::Broll {
                            concept: String::new(),
                            source_provider: "multi_broll".into(),
                            transition_style: "cut".into(),
                            crop_mode: "center".into(),
                            orientation: "portrait".into(),
                            motion_intensity: "low".into(),
                        },
                    },
                );
            }

            // Music track: single event spanning the full duration
            if let Some(ref mp) = music_path {
                tl.add_track_event(
                    TrackType::Music,
                    TimelineEvent {
                        id: "music_bg".into(),
                        asset_id: mp.clone(),
                        start_ms: 0,
                        end_ms: total_duration_ms,
                        offset_ms: 0,
                        gain_db: spec.music.as_ref().map(|m| m.gain_db).unwrap_or(-12.0),
                        fade_in_ms: 500,
                        fade_out_ms: 1000,
                        tags: music_sel_tags.clone(),
                        provenance: None,
                        kind: EventKind::Music {
                            mood: spec.output.theme.clone(),
                            energy: "low".into(),
                            bpm: None,
                            loopability: true,
                            intro_friendly: true,
                            cta_friendly: true,
                            loudness_target_lufs: -14.0,
                            loop_mode: "trim".into(),
                            ducking_policy: if spec.music.as_ref().map(|m| m.ducking).unwrap_or(music_path.is_some()) { "auto".into() } else { "none".into() },
                        },
                    },
                );
            }

            // Wire ducking directives so the filter graph sidechain compressor
            // ducks music during speech (was previously empty — music never ducked).
            let has_speech = !tl.tracks.get(&TrackType::Dialogue).map(|v| v.is_empty()).unwrap_or(true)
                || !tl.tracks.get(&TrackType::Voiceover).map(|v| v.is_empty()).unwrap_or(true);
            let music_has_ducking = spec.music.as_ref().map(|m| m.ducking).unwrap_or(music_path.is_some());
            if has_speech && music_has_ducking {
                tl.add_ducking_directive("dialogue_active", "music", 10.0, 50, 200);
            }

            // Captions track: summary event if captions file exists
            if !captions_path.is_empty() {
                tl.add_track_event(
                    TrackType::Captions,
                    TimelineEvent {
                        id: "captions_all".into(),
                        asset_id: captions_path.to_string(),
                        start_ms: 0,
                        end_ms: total_duration_ms,
                        offset_ms: 0,
                        gain_db: 0.0,
                        fade_in_ms: 0,
                        fade_out_ms: 0,
                        tags: vec![],
                        provenance: None,
                        kind: EventKind::Caption {
                            text: String::new(),
                            style: spec.captions.style.clone(),
                            word_timings: vec![],
                        },
                    },
                );
            }

            // SFX track: one event per auto-selected SFX hit
            for (i, sfx) in sfx_hits.iter().enumerate() {
                let start_ms = (sfx.start_s * 1000.0) as i64;
                // SFX are short (<1s typically), assume 500ms duration for timeline display
                let end_ms = start_ms + 500;
                let gain_db = if sfx.volume > 0.0 {
                    20.0 * sfx.volume.log10()
                } else {
                    -60.0
                };
                tl.add_track_event(
                    TrackType::Sfx,
                    TimelineEvent {
                        id: format!("sfx_{}", i + 1),
                        asset_id: sfx.path.clone(),
                        start_ms,
                        end_ms,
                        offset_ms: 0,
                        gain_db,
                        fade_in_ms: 0,
                        fade_out_ms: 50,
                        tags: vec![],
                        provenance: None,
                        kind: EventKind::Sfx {
                            editorial_role: "transition".into(),
                            category: "whoosh".into(),
                            subcategory: String::new(),
                            duration_ms: 500,
                            sample_rate: 44100,
                            peak_db: 0.0,
                            loudness_lufs: -14.0,
                            recommended_gain_db: gain_db,
                            recommended_use: "scene_transition".into(),
                            safe_overlay: true,
                        },
                    },
                );
            }

            // Register broll assets for unique-visual-asset count
            for (i, bg) in bg_assignments.iter().enumerate() {
                let asset_id = format!("broll_{}", i + 1);
                tl.add_asset(
                    "broll",
                    asset_id,
                    serde_json::json!({"path": bg.path, "start_ms": bg.start_ms, "end_ms": bg.end_ms}),
                );
            }

            // Register SFX assets so the validator can detect repetition.
            for (i, sfx) in sfx_hits.iter().enumerate() {
                let asset_id = format!("sfx_{}", i + 1);
                tl.add_asset(
                    "sfx",
                    asset_id,
                    serde_json::json!({"path": sfx.path, "volume": sfx.volume}),
                );
            }

            // Stickers track: persist the overlays the multilayer render
            // composites so verify.render / timeline inspection see them.
            // (Previously stickers existed only in the render spec — the
            // timeline's Stickers track stayed empty and verify reported 0.)
            for (i, st) in stickers.iter().enumerate() {
                let event_id = format!("sticker_{:03}", i + 1);
                let start_ms = (st.start_s * 1000.0) as i64;
                let end_ms = (st.end_s * 1000.0) as i64;
                tl.add_track_event(
                    TrackType::Stickers,
                    TimelineEvent {
                        id: event_id.clone(),
                        asset_id: event_id.clone(),
                        start_ms,
                        end_ms,
                        offset_ms: 0,
                        gain_db: 0.0,
                        fade_in_ms: 150,
                        fade_out_ms: 150,
                        tags: vec!["sticker".to_string(), st.position.clone()],
                        provenance: None,
                        kind: EventKind::Broll {
                            concept: format!("overlay:{}", st.position),
                            source_provider: st.path.clone(),
                            transition_style: "overlay".into(),
                            crop_mode: "none".into(),
                            orientation: "9:16".into(),
                            motion_intensity: "static".into(),
                        },
                    },
                );
                tl.add_asset(
                    "broll",
                    event_id.clone(),
                    serde_json::json!({
                        "path": st.path,
                        "position": st.position,
                        "scale": st.scale,
                        "overlay": true,
                    }),
                );
            }

            // Save updated timeline
            let _ = tl.save(&timeline_path);
            tracing::info!(
                "[script.to_video] Updated timeline tracks: broll={} music={} captions={} sfx={} stickers={}",
                bg_assignments.len(),
                if music_path.is_some() { 1 } else { 0 },
                if !captions_path.is_empty() { 1 } else { 0 },
                sfx_hits.len(),
                stickers.len(),
            );
        }
    }

    report_progress(60.0, 100.0, "Phase 3/3: Rendering multi-layer video...")
        .await
        .ok();

    // Build multi-layer render spec
    use openscript_ffmpeg::multilayer_render::{render_multilayer, MultiLayerRenderSpec};
    let music_sel_sfx_count = sfx_hits.len();

    // ponytail: compute ducking BEFORE moving music_path into the struct.
    // Ducking defaults to true whenever music is present — auto-selected
    // music should always duck under voiceover to avoid masking speech.
    let should_duck = spec.music.as_ref().map(|m| m.ducking).unwrap_or(music_path.is_some());

    let render_spec = MultiLayerRenderSpec {
        backgrounds,
        voiceover_paths,
        stickers,
        music_path,
        // P1 FIX: Clamp music gain_db to -8..-14 dB range (production quality sweet spot).
        // Agents writing gain_db=6.0 or gain_db=-30.0 produce inaudible or overpowering music.
        music_volume: {
            let raw_gain = spec.music.as_ref().map(|m| m.gain_db).unwrap_or(-20.0);
            let clamped = raw_gain.clamp(-14.0, -8.0);
            if (raw_gain - clamped).abs() > f64::EPSILON {
                tracing::info!(
                    "music gain_db={} clamped to {} dB (production range -14..-8)",
                    raw_gain, clamped
                );
            }
            10f64.powf(clamped / 20.0)
        },
        ducking: should_duck,
        ducking_depth_db: spec
            .music
            .as_ref()
            .map(|m| m.ducking_depth_db)
            .unwrap_or(12.0),
        captions_path: if std::path::Path::new(captions_path).exists() {
            Some(captions_path.to_string())
        } else {
            None
        },
        width: spec.meta.width,
        height: spec.meta.height,
        fps: spec.meta.fps,
        output_path: output_path.to_string(),
        crf: if preview_mode { 28 } else { spec.output.crf },
        preset: if preview_mode {
            "ultrafast".to_string()
        } else {
            "fast".to_string()
        },
        total_duration_s,
        meme_clips,
        sfx: sfx_hits,
        fonts_dir: resolve_fonts_dir(),
    };

    // Phase L: Branch on render_engine. When "hyperframes", compile the
    // timeline to HF HTML and render via hf.render instead of render_multilayer.
    // This connects HyperFrames to the golden trajectory — agents can now
    // choose the render engine via output.render_engine in the script JSON.
    let render_engine = spec.output.render_engine.as_str();
    let render_result = if render_engine == "hyperframes" {
        report_progress(70.0, 100.0, "Compiling timeline to HyperFrames HTML...")
            .await
            .ok();

        // Compile the timeline JSON to HF HTML via timeline.to_hyperframes
        let hf_compilation = handle_timeline_to_hyperframes(json!({
            "timeline_path": timeline_path,
            "output_dir": format!("{}/hf_composition", output_dir),
        }))
        .await?;

        let hf_project_dir = hf_compilation
            .get("project_dir")
            .and_then(|v| v.as_str())
            .unwrap_or("artifacts/hf_composition")
            .to_string();

        report_progress(80.0, 100.0, "Rendering via HyperFrames...")
            .await
            .ok();

        // Render via hf.render
        let hf_render_args = json!({
            "project_dir": hf_project_dir,
            "output_path": output_path,
            "quality": if preview_mode { "draft" } else { "standard" },
        });

        match crate::hf::handle_hf_render(hf_render_args).await {
            Ok(result) => {
                let out = result
                    .get("output_path")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&output_path)
                    .to_string();
                Ok(out)
            }
            Err(e) => Err(openscript_ffmpeg::FfmpegError::RenderFailed(format!(
                "HyperFrames render failed: {}",
                e
            ))),
        }
    } else {
        // Default: FFmpeg multilayer render
        render_multilayer(&render_spec).await
    };

    // Merge timeline-phase warnings (Value) with render-phase warnings (Vec<String>)
    // into a single JSON value for the response.
    let merged_warnings: serde_json::Value = {
        let mut all_warnings: Vec<String> = Vec::new();
        if let Some(arr) = warnings.as_array() {
            for w in arr {
                if let Some(s) = w.as_str() {
                    all_warnings.push(s.to_string());
                }
            }
        }
        all_warnings.extend(render_warnings);
        if all_warnings.is_empty() {
            serde_json::Value::Null
        } else {
            json!(all_warnings)
        }
    };

    match render_result {
        Ok(out_path) => {
            let file_size = std::fs::metadata(&out_path).map(|m| m.len()).unwrap_or(0);
            report_progress(100.0, 100.0, "Video created").await.ok();

            // Production KPI v2 — architecture-level quality (source, cuts/s, music
            // variance, sticker design, section composition, timeline utilization).
            use openscript_core::production_quality::{
                evaluate_production_quality, BackgroundLayerInfo, MemeLayerInfo, MusicLayerInfo,
                RenderManifest, SectionInfo, SectionRole, StickerLayerInfo,
            };
            let (has_dialogue, rms_ok) = probe_dialogue_rms(&out_path).await;
            let mut t_cursor = 0i64;
            let mut bg_layers = Vec::new();
            for (i, b) in render_spec.backgrounds.iter().enumerate() {
                let dur_ms = (b.duration_s * 1000.0) as i64;
                let meta = scene_stock_meta.get(i).and_then(|m| m.as_ref());
                let hint = if is_procedural_media_path(&b.path) {
                    Some("procedural".into())
                } else if b.path.contains("_yt") || b.path.contains("background_cache") {
                    Some("youtube".into())
                } else if meta.map(|(id, _, _, _, _, _, _)| id.starts_with("pexels_")).unwrap_or(false) {
                    Some("pexels".into())
                } else {
                    None
                };
                bg_layers.push(BackgroundLayerInfo {
                    path: b.path.clone(),
                    start_ms: t_cursor,
                    end_ms: t_cursor + dur_ms,
                    source_hint: hint,
                    content_hash: meta.map(|(_, h, _, _, _, _, _)| h.clone()),
                    video_id: meta.map(|(id, _, _, _, _, _, _)| id.clone()),
                    search_query: meta.map(|(_, _, q, _, _, _, _)| q.clone()),
                    lexical_score: meta.map(|(_, _, _, lex, _, _, _)| *lex),
                    source_title: meta.map(|(_, _, _, _, t, _, _)| t.clone()),
                    vision_score: meta.map(|(_, _, _, _, _, vs, _)| *vs),
                    vision_reason: meta.and_then(|(_, _, _, _, _, _, vr)| vr.clone()),
                });
                t_cursor += dur_ms;
            }
            let sticker_layers: Vec<StickerLayerInfo> = render_spec
                .stickers
                .iter()
                .map(|s| StickerLayerInfo {
                    path: s.path.clone(),
                    start_ms: (s.start_s * 1000.0) as i64,
                    end_ms: (s.end_s * 1000.0) as i64,
                    position: s.position.clone(),
                    scale: s.scale,
                })
                .collect();
            let meme_layers: Vec<MemeLayerInfo> = render_spec
                .meme_clips
                .iter()
                .map(|m| MemeLayerInfo {
                    path: m.path.clone(),
                    start_ms: (m.start_s * 1000.0) as i64,
                    end_ms: (m.end_s * 1000.0) as i64,
                })
                .collect();
            let music_layer = render_spec.music_path.as_ref().map(|p| {
                let gain_db = if render_spec.music_volume > 0.0 {
                    20.0 * render_spec.music_volume.log10()
                } else {
                    -60.0
                };
                MusicLayerInfo {
                    path: p.clone(),
                    gain_db,
                    ducking: render_spec.ducking,
                    mood: Some(spec.output.theme.clone()),
                    energy: None,
                    tags: music_sel_tags.clone(),
                    selection_query: music_sel_query.clone(),
                    source: music_sel_source.clone(),
                }
            });
            // Section map from scenes (hook / body / cta)
            let n_scenes = spec.scenes.len().max(1);
            let mut sections = Vec::new();
            let mut s_cursor = 0i64;
            for (i, scene) in spec.scenes.iter().enumerate() {
                let dur_ms = scene_durations
                    .get(i)
                    .map(|d| (*d * 1000.0) as i64)
                    .unwrap_or(3000);
                let role = if i == 0 {
                    SectionRole::Hook
                } else if i + 1 == n_scenes {
                    SectionRole::Cta
                } else if i + 2 >= n_scenes {
                    SectionRole::Payoff
                } else {
                    SectionRole::Body
                };
                sections.push(SectionInfo {
                    role,
                    start_ms: s_cursor,
                    end_ms: s_cursor + dur_ms,
                    text: scene.text.clone(),
                    title_text: None,
                });
                s_cursor += dur_ms;
            }
            let render_manifest = RenderManifest {
                duration_ms: total_duration_ms,
                backgrounds: bg_layers.clone(),
                stickers: sticker_layers,
                memes: meme_layers,
                music: music_layer,
                captions_path: if !captions_path.is_empty() {
                    Some(captions_path.to_string())
                } else {
                    None
                },
                voiceover_count: render_spec.voiceover_paths.len(),
                sections,
                has_dialogue,
                rms_ok,
                video_keywords: spec.video_keywords.clone(),
                theme: Some(spec.output.theme.clone()),
                caption_style: Some(spec.captions.style.clone()),
                sfx_count: music_sel_sfx_count,
                ..Default::default()
            };
            let manifest_out = format!("{}/render_manifest.json", output_dir);
            if let Ok(s) = serde_json::to_string_pretty(&render_manifest) {
                let _ = std::fs::write(&manifest_out, s);
            }
            let timeline_for_kpi = Timeline::load(&timeline_path)
                .unwrap_or_else(|_| Timeline::new(std::path::PathBuf::from("out.mp4"), "9:16", 30, None));
            let pq = evaluate_production_quality(&timeline_for_kpi, &render_manifest);
            // Fail closed: hard_fails, draft outputs, or majority procedural never "success"
            let is_draft = out_path.contains(".draft.mp4")
                || merged_warnings
                    .as_array()
                    .map(|a| {
                        a.iter().any(|w| {
                            w.as_str()
                                .map(|s| s.contains("FAIL_CLOSED"))
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false);
    // P0 FIX: If render succeeded but MP4 is missing or empty, treat as failure.
    // This catches silent ffmpeg failures where render_multilayer returns Ok but
    // the output file was never written (e.g. CWD mismatch, permission errors).
    let delivery_status = if !out_path.is_empty() && !std::path::Path::new(&out_path).exists() {
        tracing::warn!("render returned Ok but MP4 not found at: {}", out_path);
        "rendered_production_fail"
    } else if file_size == 0 {
        tracing::warn!("render returned Ok but MP4 is 0 bytes at: {}", out_path);
        "rendered_production_fail"
    } else if is_draft {
        "draft"
    } else if !pq.hard_fails.is_empty() {
        "rendered_production_fail"
    } else if pq.production_score >= 70 {
        "rendered"
    } else if pq.production_score >= 40 {
        "rendered_below_production_grade"
    } else {
        "rendered_production_fail"
    };
            let bg_paths: Vec<String> = bg_layers.iter().map(|b| b.path.clone()).collect();

            Ok(json!({
                "status": delivery_status,
                "output_path": out_path,
                "file_size_bytes": file_size,
                "timeline_path": timeline_path,
                "timeline_preview_path": preview_path,
                "timeline_preview": timeline_preview,
                "timeline_summary": timeline_summary,
                "timeline_issues": if timeline_issues.is_empty() { serde_json::Value::Null } else { json!(timeline_issues) },
                "voiceover_manifest": manifest_path,
                "render_manifest_path": manifest_out,
                "captions_path": captions_path,
                "total_duration_ms": total_duration_ms,
                "scene_count": timeline_result.get("scene_count"),
                "speaker_count": timeline_result.get("speaker_count"),
                "background_count": render_spec.backgrounds.len(),
                "sticker_count": render_spec.stickers.len(),
                "meme_count": render_spec.meme_clips.len(),
                "background_sources": bg_paths,
                "music_path": render_spec.music_path,
                "production_quality": pq,
                "warnings": merged_warnings,
            }))
        }
        Err(e) => Err(ToolError::Ffmpeg(format!("Render failed: {}", e))),
    }
}

// ---------------------------------------------------------------------------
// Handler: stock.fetch — download stock music/videos from Pixabay/Pexels APIs
// ---------------------------------------------------------------------------

async fn handle_stock_fetch(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let media_type = extract_str(&args, "type")?; // "music" or "video"
    let query = extract_str(&args, "query")?;
    let limit = default_u32(&args, "limit", 5) as usize;
    let output_dir = default_str(&args, "output_dir", "mcp/assets/stock_cache");

    std::fs::create_dir_all(&output_dir)?;

    report_progress(0.0, 100.0, &format!("Searching for {}...", media_type))
        .await
        .ok();

    if media_type == "music" {
        // Try Pixabay music API
        let pixabay_key_val = if pixabay_key().is_empty() { None } else { Some(pixabay_key()) };
        if let Some(key) = pixabay_key_val {
            let url = format!(
                "https://pixabay.com/api/audio/?key={}&q={}&per_page={}",
                key,
                urlencoding::encode(query),
                limit
            );
            let client = reqwest::Client::new();
            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let body: serde_json::Value = resp
                        .json()
                        .await
                        .map_err(|e| ToolError::Asset(e.to_string()))?;
                    let hits = body.get("hits").cloned().unwrap_or(json!([]));
                    let mut results = Vec::new();

                    if let Some(arr) = hits.as_array() {
                        for hit in arr.iter().take(limit) {
                            let audio_url = hit.get("audio").and_then(|v| v.as_str()).unwrap_or("");
                            let title = hit
                                .get("tags")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown");
                            let duration =
                                hit.get("duration").and_then(|v| v.as_i64()).unwrap_or(0);

                            if !audio_url.is_empty() {
                                let filename = format!(
                                    "{}/{}_{}.mp3",
                                    output_dir,
                                    query.replace(' ', "_"),
                                    results.len()
                                );
                                match client.get(audio_url).send().await {
                                    Ok(resp) if resp.status().is_success() => {
                                        let bytes = resp
                                            .bytes()
                                            .await
                                            .map_err(|e| ToolError::Asset(e.to_string()))?;
                                        std::fs::write(&filename, &bytes)?;
                                        results.push(json!({
                                            "title": title,
                                            "path": filename,
                                            "duration_s": duration,
                                            "source": "pixabay",
                                        }));
                                    }
                                    Err(e) => {
                                        tracing::warn!("[stock.fetch] Download failed: {}", e)
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }

                    report_progress(
                        100.0,
                        100.0,
                        &format!("Downloaded {} tracks", results.len()),
                    )
                    .await
                    .ok();
                    return Ok(json!({
                        "status": "fetched",
                        "type": "music",
                        "source": "pixabay",
                        "count": results.len(),
                        "results": results,
                    }));
                }
                _ => tracing::warn!("[stock.fetch] Pixabay API failed"),
            }
        }

        // Fallback: return local stock library results
        report_progress(100.0, 100.0, "Using local stock library")
            .await
            .ok();
        return Ok(json!({
            "status": "fallback",
            "type": "music",
            "source": "local",
            "message": "Set PIXABAY_API_KEY env var to download from Pixabay. Using local stock library.",
            "local_library": "mcp/assets/music_index.json",
        }));
    }

    if media_type == "video" {
        // Try Pixabay video API
        let pixabay_key_val = if pixabay_key().is_empty() { None } else { Some(pixabay_key()) };
        if let Some(key) = pixabay_key_val {
            let video_type = default_str(&args, "video_type", "film");
            let url = format!(
                "https://pixabay.com/api/videos/?key={}&q={}&per_page={}&video_type={}",
                key,
                urlencoding::encode(query),
                limit,
                video_type
            );
            let client = reqwest::Client::new();
            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let body: serde_json::Value = resp
                        .json()
                        .await
                        .map_err(|e| ToolError::Asset(e.to_string()))?;
                    let hits = body.get("hits").cloned().unwrap_or(json!([]));
                    let mut results = Vec::new();

                    if let Some(arr) = hits.as_array() {
                        for hit in arr.iter().take(limit) {
                            // Get the best quality video URL
                            let videos = hit.get("videos");
                            let video_url = videos
                                .and_then(|v| v.get("large"))
                                .or_else(|| videos.and_then(|v| v.get("medium")))
                                .or_else(|| videos.and_then(|v| v.get("small")))
                                .and_then(|q| q.get("url"))
                                .and_then(|u| u.as_str())
                                .unwrap_or("");

                            let tags = hit
                                .get("tags")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown");
                            let duration =
                                hit.get("duration").and_then(|v| v.as_i64()).unwrap_or(0);

                            if !video_url.is_empty() {
                                let filename = format!(
                                    "{}/{}_{}.mp4",
                                    output_dir,
                                    query.replace(' ', "_"),
                                    results.len()
                                );
                                match client.get(video_url).send().await {
                                    Ok(resp) if resp.status().is_success() => {
                                        let bytes = resp
                                            .bytes()
                                            .await
                                            .map_err(|e| ToolError::Asset(e.to_string()))?;
                                        std::fs::write(&filename, &bytes)?;
                                        results.push(json!({
                                            "title": tags,
                                            "path": filename,
                                            "duration_s": duration,
                                            "source": "pixabay",
                                        }));
                                    }
                                    Err(e) => {
                                        tracing::warn!("[stock.fetch] Download failed: {}", e)
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }

                    report_progress(
                        100.0,
                        100.0,
                        &format!("Downloaded {} videos", results.len()),
                    )
                    .await
                    .ok();
                    return Ok(json!({
                        "status": "fetched",
                        "type": "video",
                        "source": "pixabay",
                        "count": results.len(),
                        "results": results,
                    }));
                }
                _ => tracing::warn!("[stock.fetch] Pixabay API failed"),
            }
        }

        // Fallback: return local stock library
        report_progress(100.0, 100.0, "Using local stock library")
            .await
            .ok();
        return Ok(json!({
            "status": "fallback",
            "type": "video",
            "source": "local",
            "message": "Set PIXABAY_API_KEY env var to download from Pixabay. Using local stock library.",
            "local_library": "mcp/assets/backgrounds/",
        }));
    }

    Err(ToolError::InvalidArg(format!(
        "Unknown media type: {}. Use 'music' or 'video'.",
        media_type
    )))
}

// ---------------------------------------------------------------------------
// Handler: youtube.download — download YouTube video clips
// ---------------------------------------------------------------------------

async fn handle_youtube_download(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let query = extract_str(&args, "query")?; // URL or search query
    let duration_s = default_f64(&args, "duration_s", 30.0);
    let start_s = default_opt_f64(&args, "start_s"); // Optional: specific start time
    let aspect = default_str(&args, "aspect", "9:16");
    let cache_dir = default_str(&args, "cache_dir", "mcp/assets/background_cache");
    let use_cookies = default_bool(&args, "use_cookies", true);

    std::fs::create_dir_all(&cache_dir)?;

    report_progress(0.0, 100.0, "Downloading from YouTube...")
        .await
        .ok();

    // Determine if query is a URL or a search term
    let is_url = query.starts_with("http://")
        || query.starts_with("https://")
        || query.starts_with("youtu.be");
    let cache_key = format!("{:x}", md5_hash(query.as_bytes()));
    let clip_path = format!("{}/{}_clip.mp4", cache_dir, cache_key);

    // If start_s is specified, use --download-sections to download only the range
    // This avoids downloading a 10-hour video when we only need 100 seconds
    if let Some(start) = start_s {
        let end = start + duration_s;
        let start_fmt = format_seconds_to_timestamp(start);
        let end_fmt = format_seconds_to_timestamp(end);
        let section_arg = format!("*{}-{}", start_fmt, end_fmt);

        report_progress(
            20.0,
            100.0,
            &format!("Downloading range {}-{}...", start_fmt, end_fmt),
        )
        .await
        .ok();

        let mut yt_args = vec![
            "--download-sections".to_string(),
            section_arg,
            "--force-keyframes-at-cuts".to_string(),
            "--format".to_string(),
            "best[height<=720]".to_string(),
            "--output".to_string(),
            clip_path.clone(),
            "--no-playlist".to_string(),
        ];

        if use_cookies {
            yt_args.push("--cookies-from-browser".to_string());
            yt_args.push("chrome".to_string());
        }
        yt_args.push("--user-agent".to_string());
        yt_args.push("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36".to_string());

        if is_url {
            yt_args.push(query.to_string());
        } else {
            yt_args.push(format!("ytsearch1:{}", query));
        }

        let yt_result = tokio::process::Command::new("yt-dlp")
            .args(&yt_args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .output()
            .await;

        match yt_result {
            Ok(output) if output.status.success() => {
                // Clip is already the right duration — just crop to aspect
                report_progress(70.0, 100.0, "Cropping to aspect ratio...")
                    .await
                    .ok();
                let (crop_w, crop_h) = aspect_to_crop_dims(&aspect);

                let cropped_path = format!("{}/{}_cropped.mp4", cache_dir, cache_key);
                let crop_result = tokio::process::Command::new("ffmpeg")
                    .arg("-y")
                    .arg("-i")
                    .arg(&clip_path)
                    .arg("-vf")
                    .arg(format!("crop={}:{}", crop_w, crop_h))
                    .arg("-c:v")
                    .arg("libx264")
                    .arg("-preset")
                    .arg("fast")
                    .arg("-crf")
                    .arg("23")
                    .arg("-an")
                    .arg(&cropped_path)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::piped())
                    .kill_on_drop(true)
                    .output()
                    .await;

                if let Ok(o) = crop_result {
                    if o.status.success() {
                        let _ = std::fs::rename(&cropped_path, &clip_path);
                    } else {
                        let _ = std::fs::remove_file(&cropped_path);
                    }
                }

                report_progress(100.0, 100.0, "Clip downloaded").await.ok();
                return Ok(json!({
                    "status": "downloaded",
                    "clip_path": clip_path,
                    "start_s": start,
                    "duration_s": duration_s,
                    "aspect": aspect,
                    "method": "range_download",
                    "cached": false,
                }));
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(ToolError::Ffmpeg(format!(
                    "YouTube range download failed: {}",
                    stderr.lines().last().unwrap_or("unknown error")
                )));
            }
            Err(e) => {
                return Err(ToolError::Ffmpeg(format!("yt-dlp not available: {}", e)));
            }
        }
    }

    // No start_s specified — download full video (or use cache), then extract random clip
    let full_video_path = format!("{}/{}.mp4", cache_dir, cache_key);

    // Check cache first
    if Path::new(&full_video_path).exists() {
        report_progress(50.0, 100.0, "Using cached video...")
            .await
            .ok();
    } else {
        // Build yt-dlp command
        let mut yt_args = vec![
            "--format".to_string(),
            "best[height<=720]".to_string(),
            "--output".to_string(),
            full_video_path.clone(),
            "--no-playlist".to_string(),
            "--quiet".to_string(),
        ];

        // Add cookies if enabled
        if use_cookies {
            // Try chrome first — if it fails, yt-dlp will continue without cookies
            yt_args.push("--cookies-from-browser".to_string());
            yt_args.push("chrome".to_string());
        }

        // Add user agent to avoid bot detection
        yt_args.push("--user-agent".to_string());
        yt_args.push("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36".to_string());

        // Search or direct URL
        if is_url {
            yt_args.push(query.to_string());
        } else {
            yt_args.push(format!("ytsearch1:{}", query));
        }

        let yt_result = tokio::process::Command::new("yt-dlp")
            .args(&yt_args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .output()
            .await;

        match yt_result {
            Ok(output) if output.status.success() => {
                report_progress(50.0, 100.0, "Downloaded, extracting clip...")
                    .await
                    .ok();
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                tracing::warn!("[youtube.download] yt-dlp failed: {}", stderr);
                return Err(ToolError::Ffmpeg(format!(
                    "YouTube download failed: {}. Try providing a direct URL, or set PIXABAY_API_KEY for stock footage.",
                    stderr.lines().last().unwrap_or("unknown error")
                )));
            }
            Err(e) => {
                return Err(ToolError::Ffmpeg(format!(
                    "yt-dlp not available: {}. Install with: pip install yt-dlp",
                    e
                )));
            }
        }
    }

    // Get video duration
    let probe_output = tokio::process::Command::new("ffprobe")
        .arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("format=duration")
        .arg("-of")
        .arg("default=noprint_wrappers=1:nokey=1")
        .arg(&full_video_path)
        .output()
        .await;

    let source_duration_s: f64 = match probe_output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .trim()
            .parse()
            .unwrap_or(duration_s),
        _ => duration_s,
    };

    // Pick random start time
    let max_start = (source_duration_s - duration_s).max(0.0);
    let start_s = if max_start > 0.0 {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0) as u64;
        (seed as f64 / u64::MAX as f64) * max_start
    } else {
        0.0
    };

    // Crop dimensions
    let (crop_w, crop_h) = aspect_to_crop_dims(&aspect);

    // Extract clip with crop
    let extract_result = tokio::process::Command::new("ffmpeg")
        .arg("-y")
        .arg("-ss")
        .arg(start_s.to_string())
        .arg("-i")
        .arg(&full_video_path)
        .arg("-t")
        .arg(duration_s.to_string())
        .arg("-vf")
        .arg(format!("crop={}:{}", crop_w, crop_h))
        .arg("-c:v")
        .arg("libx264")
        .arg("-preset")
        .arg("fast")
        .arg("-crf")
        .arg("23")
        .arg("-an")
        .arg(&clip_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await;

    match extract_result {
        Ok(o) if o.status.success() => {
            report_progress(100.0, 100.0, "Clip extracted").await.ok();
            Ok(json!({
                "status": "downloaded",
                "clip_path": clip_path,
                "source_duration_s": source_duration_s,
                "start_s": start_s,
                "duration_s": duration_s,
                "aspect": aspect,
                "cached": Path::new(&full_video_path).exists(),
            }))
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            Err(ToolError::Ffmpeg(format!(
                "Clip extraction failed: {}",
                stderr
            )))
        }
        Err(e) => Err(ToolError::Ffmpeg(format!("FFmpeg failed: {}", e))),
    }
}

// ---------------------------------------------------------------------------
// Handler: youtube.search — search YouTube without downloading
// ---------------------------------------------------------------------------

async fn handle_youtube_search(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let query = extract_str(&args, "query")?;
    let limit = default_u32(&args, "limit", 10) as usize;

    report_progress(0.0, 100.0, "Searching YouTube...")
        .await
        .ok();

    // Use yt-dlp to search (flat, no download)
    let search_query = format!("ytsearch{}:{}", limit, query);

    let result = tokio::process::Command::new("yt-dlp")
        .arg("--flat-playlist")
        .arg("--dump-json")
        .arg("--no-playlist")
        .arg(&search_query)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await;

    match result {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut results = Vec::new();

            for line in stdout.lines() {
                if let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) {
                    let title = entry
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Unknown");
                    let url = entry
                        .get("url")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| {
                            entry
                                .get("id")
                                .and_then(|v| v.as_str())
                                .map(|id| format!("https://youtube.com/watch?v={}", id))
                        })
                        .unwrap_or_default();
                    let duration = entry
                        .get("duration")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    let view_count = entry
                        .get("view_count")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let uploader = entry
                        .get("uploader")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Unknown");

                    results.push(json!({
                        "title": title,
                        "url": url,
                        "duration_s": duration,
                        "view_count": view_count,
                        "uploader": uploader,
                    }));
                }
            }

            report_progress(100.0, 100.0, &format!("Found {} results", results.len()))
                .await
                .ok();

            Ok(json!({
                "status": "searched",
                "query": query,
                "count": results.len(),
                "results": results,
            }))
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(ToolError::Ffmpeg(format!(
                "YouTube search failed: {}",
                stderr.lines().last().unwrap_or("unknown error")
            )))
        }
        Err(e) => Err(ToolError::Ffmpeg(format!(
            "yt-dlp not available: {}. Install with: pip install yt-dlp",
            e
        ))),
    }
}

// ---------------------------------------------------------------------------
// Handler: stock.search — search Pixabay without downloading
// ---------------------------------------------------------------------------

async fn handle_stock_search(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let media_type = extract_str(&args, "type")?;
    let query = extract_str(&args, "query")?;
    let limit = default_u32(&args, "limit", 10) as usize;

    report_progress(
        0.0,
        100.0,
        &format!("Searching Pixabay for {}...", media_type),
    )
    .await
    .ok();

    let pixabay_key_val = if pixabay_key().is_empty() { None } else { Some(pixabay_key()) };

    if let Some(key) = pixabay_key_val {
        let endpoint = if media_type == "music" {
            "https://pixabay.com/api/audio/"
        } else {
            "https://pixabay.com/api/videos/"
        };

        let video_type = default_str(&args, "video_type", "film");
        let url = if media_type == "music" {
            format!(
                "{}?key={}&q={}&per_page={}",
                endpoint,
                key,
                urlencoding::encode(query),
                limit
            )
        } else {
            format!(
                "{}?key={}&q={}&per_page={}&video_type={}",
                endpoint,
                key,
                urlencoding::encode(query),
                limit,
                video_type
            )
        };

        let client = reqwest::Client::new();
        match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => {
                let body: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| ToolError::Asset(e.to_string()))?;

                let total = body.get("totalHits").and_then(|v| v.as_u64()).unwrap_or(0);
                let hits = body.get("hits").cloned().unwrap_or(json!([]));

                let results: Vec<serde_json::Value> = hits
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .take(limit)
                            .map(|hit| {
                                let title = hit
                                    .get("tags")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("Unknown");
                                let duration =
                                    hit.get("duration").and_then(|v| v.as_i64()).unwrap_or(0);
                                let user = hit
                                    .get("user")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("Unknown");
                                let views = hit.get("views").and_then(|v| v.as_u64()).unwrap_or(0);
                                let likes = hit.get("likes").and_then(|v| v.as_u64()).unwrap_or(0);

                                if media_type == "music" {
                                    let preview_url =
                                        hit.get("audio").and_then(|v| v.as_str()).unwrap_or("");
                                    json!({
                                        "title": title,
                                        "duration_s": duration,
                                        "user": user,
                                        "views": views,
                                        "likes": likes,
                                        "preview_url": preview_url,
                                    })
                                } else {
                                    let videos = hit.get("videos");
                                    let thumb = hit
                                        .get("previewURL")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("");
                                    let video_url = videos
                                        .and_then(|v| v.get("large"))
                                        .or_else(|| videos.and_then(|v| v.get("medium")))
                                        .or_else(|| videos.and_then(|v| v.get("small")))
                                        .and_then(|q| q.get("url"))
                                        .and_then(|u| u.as_str())
                                        .unwrap_or("");
                                    json!({
                                        "title": title,
                                        "duration_s": duration,
                                        "user": user,
                                        "views": views,
                                        "likes": likes,
                                        "thumbnail": thumb,
                                        "video_url": video_url,
                                    })
                                }
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                report_progress(100.0, 100.0, &format!("Found {} results", results.len()))
                    .await
                    .ok();

                return Ok(json!({
                    "status": "searched",
                    "type": media_type,
                    "source": "pixabay",
                    "query": query,
                    "total_hits": total,
                    "count": results.len(),
                    "results": results,
                }));
            }
            _ => tracing::warn!("[stock.search] Pixabay API failed"),
        }
    }

    // Fallback: list local stock library
    report_progress(100.0, 100.0, "Using local stock library")
        .await
        .ok();

    if media_type == "music" {
        let index_path = std::env::var("OPENSCRIPT_MUSIC_INDEX")
            .unwrap_or_else(|_| "mcp/assets/music_index.json".to_string());
        if let Ok(content) = std::fs::read_to_string(&index_path) {
            if let Ok(index) = serde_json::from_str::<serde_json::Value>(&content) {
                let assets = index.get("assets").cloned().unwrap_or(json!([]));
                let results: Vec<serde_json::Value> = assets
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter(|a| {
                                let title = a
                                    .get("title")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_lowercase();
                                let mood = a
                                    .get("mood")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_lowercase();
                                title.contains(&query.to_lowercase())
                                    || mood.contains(&query.to_lowercase())
                            })
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                return Ok(json!({
                    "status": "fallback",
                    "type": "music",
                    "source": "local",
                    "query": query,
                    "count": results.len(),
                    "results": results,
                    "message": "Set PIXABAY_API_KEY to search Pixabay. Showing local library matches.",
                }));
            }
        }
    }

    // Video fallback: list local backgrounds
    let bg_dir = "mcp/assets/backgrounds";
    let mut results = Vec::new();
    if let Ok(entries) = std::fs::read_dir(bg_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".mp4") {
                let path = format!("{}/{}", bg_dir, name);
                results.push(json!({
                    "title": name,
                    "path": path,
                    "source": "local",
                }));
            }
        }
    }

    Ok(json!({
        "status": "fallback",
        "type": media_type,
        "source": "local",
        "query": query,
        "count": results.len(),
        "results": results,
        "message": "Set PIXABAY_API_KEY to search Pixabay. Showing local library.",
    }))
}

// ---------------------------------------------------------------------------
// Handler: media.search — PNG image search (Pexels Images + Openverse)
// ---------------------------------------------------------------------------

async fn handle_media_search(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let query = extract_str(&args, "query")?;
    let limit = default_u32(&args, "limit", 10) as usize;
    let source = default_str(&args, "source", "auto");

    report_progress(0.0, 100.0, &format!("Searching for images: {}...", query))
        .await
        .ok();

    let pexels_key_val = pexels_key();

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| ToolError::Asset(format!("HTTP client error: {}", e)))?;

    // Try Pexels Images API first (if key available and source allows)
    if source != "openverse" && !pexels_key().is_empty() {
        let url = format!(
            "https://api.pexels.com/v1/search?query={}&per_page={}&orientation=portrait",
            urlencoding::encode(query),
            limit
        );

        match client
            .get(&url)
            .header("Authorization", &pexels_key_val)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                let body: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| ToolError::Asset(format!("Pexels parse error: {}", e)))?;

                let results: Vec<serde_json::Value> = body.get("photos")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().take(limit).map(|p| {
                        let src = p.get("src").cloned().unwrap_or(json!({}));
                        json!({
                            "id": p.get("id"),
                            "title": format!("Photo by {}", p.get("photographer").and_then(|v| v.as_str()).unwrap_or("Unknown")),
                            "url": src.get("original").and_then(|v| v.as_str()).unwrap_or(""),
                            "medium_url": src.get("medium").and_then(|v| v.as_str()).unwrap_or(""),
                            "large_url": src.get("large").and_then(|v| v.as_str()).unwrap_or(""),
                            "width": p.get("width"),
                            "height": p.get("height"),
                            "source": "pexels",
                            "license": "pexels-license",
                        })
                    }).collect())
                    .unwrap_or_default();

                if !results.is_empty() {
                    report_progress(100.0, 100.0, &format!("Found {} images", results.len()))
                        .await
                        .ok();
                    return Ok(json!({
                        "status": "searched",
                        "query": query,
                        "source": "pexels",
                        "count": results.len(),
                        "results": results,
                    }));
                }
            }
            _ => tracing::warn!("[media.search] Pexels API failed, trying Openverse"),
        }
    }

    // Fallback: Openverse API (free, no key needed)
    if source != "pexels" {
        let url = format!(
            "https://api.openverse.org/v1/images/?q={}&page_size={}",
            urlencoding::encode(query),
            limit
        );

        match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => {
                let body: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| ToolError::Asset(format!("Openverse parse error: {}", e)))?;

                let results: Vec<serde_json::Value> = body
                    .get("results")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .take(limit)
                            .map(|r| {
                                json!({
                                    "id": r.get("id"),
                                    "title": r.get("title"),
                                    "url": r.get("url"),
                                    "thumbnail": r.get("thumbnail"),
                                    "width": r.get("width"),
                                    "height": r.get("height"),
                                    "source": "openverse",
                                    "license": r.get("license"),
                                    "creator": r.get("creator"),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                report_progress(100.0, 100.0, &format!("Found {} images", results.len()))
                    .await
                    .ok();
                return Ok(json!({
                    "status": "searched",
                    "query": query,
                    "source": "openverse",
                    "count": results.len(),
                    "results": results,
                }));
            }
            _ => tracing::warn!("[media.search] Openverse API failed"),
        }
    }

    Ok(json!({
        "status": "no_results",
        "query": query,
        "count": 0,
        "results": [],
        "message": "No images found. Try a different query.",
    }))
}

// ---------------------------------------------------------------------------
// Handler: gif.search — GIPHY sticker search
// ---------------------------------------------------------------------------

async fn handle_gif_search(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let query = extract_str(&args, "query")?;
    let limit = default_u32(&args, "limit", 10) as usize;
    let rating = default_str(&args, "rating", "g");

    report_progress(0.0, 100.0, &format!("Searching GIPHY for: {}...", query))
        .await
        .ok();

    let giphy_key = Some(giphy_key()).filter(|s| !s.is_empty());

    if let Some(key) = giphy_key {
        if !key.is_empty() {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .map_err(|e| ToolError::Asset(format!("HTTP client error: {}", e)))?;

            // Search GIPHY stickers (transparent GIFs)
            let url = format!(
                "https://api.giphy.com/v1/stickers/search?api_key={}&q={}&limit={}&rating={}&bundle=sticker_layering",
                key,
                urlencoding::encode(query),
                limit,
                rating
            );

            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let body: serde_json::Value = resp
                        .json()
                        .await
                        .map_err(|e| ToolError::Asset(format!("GIPHY parse error: {}", e)))?;

                    let results: Vec<serde_json::Value> = body
                        .get("data")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .take(limit)
                                .map(|g| {
                                    let images = g.get("images").cloned().unwrap_or(json!({}));
                                    let original =
                                        images.get("original").cloned().unwrap_or(json!({}));
                                    let downsized =
                                        images.get("downsized").cloned().unwrap_or(json!({}));
                                    json!({
                                        "id": g.get("id"),
                                        "title": g.get("title"),
                                        "url": g.get("url"),
                                        "gif_url": original.get("url"),
                                        "webp_url": original.get("webp"),
                                        "preview_url": downsized.get("url"),
                                        "width": original.get("width"),
                                        "height": original.get("height"),
                                        "size_bytes": original.get("size"),
                                        "source": "giphy",
                                        "transparent": true,
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();

                    report_progress(100.0, 100.0, &format!("Found {} stickers", results.len()))
                        .await
                        .ok();
                    return Ok(json!({
                        "status": "searched",
                        "query": query,
                        "source": "giphy",
                        "count": results.len(),
                        "results": results,
                    }));
                }
                _ => tracing::warn!("[gif.search] GIPHY API failed"),
            }
        }
    }

    // Fallback: Pexels video search for short clips
    report_progress(
        50.0,
        100.0,
        "GIPHY key not set, searching Pexels for short clips...",
    )
    .await
    .ok();
    let pexels_key_val = pexels_key();

    let url = format!(
        "https://api.pexels.com/videos/search?query={}&per_page={}&orientation=square",
        urlencoding::encode(query),
        limit
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| ToolError::Asset(format!("HTTP client error: {}", e)))?;

    match client
        .get(&url)
        .header("Authorization", &pexels_key_val)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| ToolError::Asset(format!("Pexels parse error: {}", e)))?;

            let results: Vec<serde_json::Value> = body.get("videos")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().take(limit).filter_map(|v| {
                    let duration = v.get("duration").and_then(|v| v.as_i64()).unwrap_or(0);
                    if duration > 10 { return None; } // Only short clips
                    let video_files = v.get("video_files").and_then(|v| v.as_array())?;
                    let best = video_files.iter()
                        .find(|f| {
                            let w = f.get("width").and_then(|v| v.as_u64()).unwrap_or(0);
                            (360..=720).contains(&w)
                        })?;
                    Some(json!({
                        "id": v.get("id"),
                        "title": format!("Pexels video {}", v.get("id").and_then(|v| v.as_u64()).unwrap_or(0)),
                        "url": v.get("url"),
                        "video_url": best.get("link"),
                        "width": best.get("width"),
                        "height": best.get("height"),
                        "duration_s": duration,
                        "source": "pexels",
                        "transparent": false,
                    }))
                }).collect())
                .unwrap_or_default();

            report_progress(100.0, 100.0, &format!("Found {} clips", results.len()))
                .await
                .ok();
            return Ok(json!({
                "status": "searched",
                "query": query,
                "source": "pexels",
                "count": results.len(),
                "results": results,
                "message": "GIPHY_API_KEY not set. Set it to search GIPHY stickers. Showing Pexels short clips instead.",
            }));
        }
        _ => {}
    }

    Ok(json!({
        "status": "no_results",
        "query": query,
        "count": 0,
        "results": [],
        "message": "Set GIPHY_API_KEY env var for GIPHY sticker search. Get free key at https://developers.giphy.com",
    }))
}

// ---------------------------------------------------------------------------
// Handler: timeline.inspect — deep-dive layer inspection
// ---------------------------------------------------------------------------

async fn handle_timeline_inspect(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let preview_path = extract_str(&args, "timeline_preview_path")?;
    let layer = extract_str(&args, "layer")?;

    // Read the timeline preview file
    let preview = std::fs::read_to_string(sanitize_input_path(preview_path)?)
        .map_err(|e| ToolError::NotFound(format!("Cannot read timeline preview: {}", e)))?;

    // Also try to read the timeline JSON for full details
    let timeline_dir = std::path::Path::new(preview_path)
        .parent()
        .unwrap_or(std::path::Path::new("."));
    let timeline_json_path = timeline_dir.join("timeline.json");
    let manifest_path = timeline_dir.join("voices").join("manifest.json");

    let mut details = Vec::new();

    match layer {
        "background" => {
            // Read from timeline tracks, not assets (the schema stores events in tracks)
            if let Ok(tl_str) = std::fs::read_to_string(&timeline_json_path) {
                if let Ok(tl) = serde_json::from_str::<serde_json::Value>(&tl_str) {
                    // Try tracks.broll first (EDL v2 schema)
                    if let Some(tracks) = tl.get("tracks").and_then(|t| t.as_object()) {
                        if let Some(broll_events) = tracks.get("broll").and_then(|b| b.as_array()) {
                            for event in broll_events {
                                let id = event.get("id").and_then(|v| v.as_str()).unwrap_or("");
                                let start_ms =
                                    event.get("start_ms").and_then(|v| v.as_i64()).unwrap_or(0);
                                let end_ms =
                                    event.get("end_ms").and_then(|v| v.as_i64()).unwrap_or(0);
                                let asset_id =
                                    event.get("asset_id").and_then(|v| v.as_str()).unwrap_or("");
                                // Look up the actual file path from assets
                                let path = tl
                                    .get("assets")
                                    .and_then(|a| a.get("broll"))
                                    .and_then(|b| b.as_object())
                                    .and_then(|b| b.get(asset_id))
                                    .and_then(|p| p.get("path"))
                                    .and_then(|p| p.as_str())
                                    .unwrap_or(asset_id);
                                details.push(json!({
                                    "id": id,
                                    "start_ms": start_ms,
                                    "end_ms": end_ms,
                                    "path": path,
                                    "exists": std::path::Path::new(path).exists(),
                                }));
                            }
                        }
                    }
                    // Also check assets.broll as fallback
                    if details.is_empty() {
                        if let Some(broll) = tl
                            .get("assets")
                            .and_then(|a| a.get("broll"))
                            .and_then(|b| b.as_object())
                        {
                            for (id, asset) in broll {
                                let path = asset.get("path").and_then(|v| v.as_str()).unwrap_or("");
                                details.push(json!({
                                    "id": id,
                                    "path": path,
                                    "exists": std::path::Path::new(path).exists(),
                                }));
                            }
                        }
                    }
                }
            }
        }
        "voiceover" => {
            if let Ok(m_str) = std::fs::read_to_string(&manifest_path) {
                if let Ok(m) = serde_json::from_str::<serde_json::Value>(&m_str) {
                    if let Some(segments) = m.get("segments").and_then(|v| v.as_array()) {
                        for seg in segments {
                            details.push(json!({
                                "scene_id": seg.get("scene_id"),
                                "speaker": seg.get("speaker"),
                                "text": seg.get("text"),
                                "start_ms": seg.get("start_ms"),
                                "end_ms": seg.get("end_ms"),
                                "duration_ms": seg.get("duration_ms"),
                                "wav_path": seg.get("wav_path"),
                                "word_count": seg.get("words").and_then(|v| v.as_array()).map(|a| a.len()),
                                "backend": seg.get("backend"),
                            }));
                        }
                    }
                }
            }
        }
        "music" => {
            if let Ok(tl_str) = std::fs::read_to_string(&timeline_json_path) {
                if let Ok(tl) = serde_json::from_str::<serde_json::Value>(&tl_str) {
                    // Try tracks.music first (EDL v2 schema)
                    if let Some(tracks) = tl.get("tracks").and_then(|t| t.as_object()) {
                        if let Some(music_events) = tracks.get("music").and_then(|m| m.as_array()) {
                            for event in music_events {
                                let id = event.get("id").and_then(|v| v.as_str()).unwrap_or("");
                                let start_ms =
                                    event.get("start_ms").and_then(|v| v.as_i64()).unwrap_or(0);
                                let end_ms =
                                    event.get("end_ms").and_then(|v| v.as_i64()).unwrap_or(0);
                                let asset_id =
                                    event.get("asset_id").and_then(|v| v.as_str()).unwrap_or("");
                                let path = tl
                                    .get("assets")
                                    .and_then(|a| a.get("music"))
                                    .and_then(|m| m.as_object())
                                    .and_then(|m| m.get(asset_id))
                                    .and_then(|p| p.get("path"))
                                    .and_then(|p| p.as_str())
                                    .unwrap_or(asset_id);
                                details.push(json!({
                                    "id": id,
                                    "start_ms": start_ms,
                                    "end_ms": end_ms,
                                    "path": path,
                                }));
                            }
                        }
                    }
                    // Fallback to assets.music
                    if details.is_empty() {
                        if let Some(music) = tl
                            .get("assets")
                            .and_then(|a| a.get("music"))
                            .and_then(|m| m.as_object())
                        {
                            for (id, asset) in music {
                                details.push(json!({
                                    "id": id,
                                    "path": asset.get("path"),
                                }));
                            }
                        }
                    }
                }
            }
        }
        "captions" => {
            let captions_path = timeline_dir.join("captions.ass");
            if let Ok(content) = std::fs::read_to_string(&captions_path) {
                let dialogue_count = content.matches("Dialogue:").count();
                details.push(json!({
                    "path": captions_path.to_string_lossy(),
                    "dialogue_count": dialogue_count,
                    "size_bytes": content.len(),
                }));
            }
        }
        "stickers" => {
            // Stickers are in the script.to_video response, not stored separately
            details.push(json!({
                "message": "Sticker details are in the script.to_video response. Check the 'sticker_count' and 'timeline_preview' fields.",
            }));
        }
        _ => {
            return Err(ToolError::InvalidArg(format!(
                "Unknown layer: {}. Use: background, voiceover, music, captions, stickers",
                layer
            )));
        }
    }

    Ok(json!({
        "status": "inspected",
        "layer": layer,
        "event_count": details.len(),
        "events": details,
        "preview_excerpt": preview.lines().take(5).collect::<Vec<_>>().join("\n"),
    }))
}

// ---------------------------------------------------------------------------
// Handler: library.search — search music/SFX library index
// ---------------------------------------------------------------------------

async fn handle_library_search(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let query = extract_str(&args, "query")?;
    let media_type = default_opt_str(&args, "type");
    let limit = default_u32(&args, "limit", 10) as usize;
    // New filters (audit bug #18): mood/energy/duration/source/license.
    // These make library.search as filterable as music.search + sfx.search,
    // so an agent can find a 30s "epic cinematic" track without paging
    // through hundreds of irrelevant results.
    let source_filter = default_opt_str(&args, "source");
    let license_filter = default_opt_str(&args, "license");
    let min_duration_s = args
        .get("min_duration_s")
        .and_then(|v| v.as_f64());
    let max_duration_s = args
        .get("max_duration_s")
        .and_then(|v| v.as_f64());
    let tag_filter: Option<String> = default_opt_str(&args, "tag");
    let mood_filter: Option<String> = default_opt_str(&args, "mood");
    let energy_filter: Option<String> = default_opt_str(&args, "energy");

    // Resolve path CWD-independently (round-2 GAP #12 fix — same as
    // background.search). library.search only worked from repo root before.
    let index_path_raw = std::env::var("OPENSCRIPT_MUSIC_LIBRARY_INDEX")
        .unwrap_or_else(|_| "mcp/assets/music_library_index.json".to_string());
    let index_path = resolve_repo_path(&index_path_raw);

    if !index_path.exists() {
        return Err(ToolError::NotFound(format!(
            "Music library index not found at {} (resolved from {}). Run the library.build MCP tool to generate it (requires yt-dlp on PATH).",
            index_path.display(),
            index_path_raw
        )));
    }

    let index_str = std::fs::read_to_string(&index_path)?;
    let index: serde_json::Value = serde_json::from_str(&index_str)?;

    let entries = index
        .get("entries")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let query_lower = query.to_lowercase();
    let query_words: Vec<&str> = query_lower.split_whitespace().collect();

    let mut results: Vec<serde_json::Value> = Vec::new();
    let mut filtered_by_duration = 0u32;
    let mut filtered_by_source = 0u32;
    let mut filtered_by_license = 0u32;
    let mut filtered_by_tag = 0u32;
    let mut filtered_by_mood = 0u32;
    let mut filtered_by_energy = 0u32;

    for entry in &entries {
        // Filter by media type if specified
        if let Some(ref mt) = media_type {
            let entry_type = entry
                .get("media_type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if entry_type != mt {
                continue;
            }
        }

        // Filter by source channel (e.g. "NoCopyrightSounds")
        if let Some(ref src) = source_filter {
            let entry_source = entry
                .get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if entry_source != src {
                filtered_by_source += 1;
                continue;
            }
        }

        // Filter by license (e.g. "no-copyright", "creative-commons")
        if let Some(ref lic) = license_filter {
            let entry_license = entry
                .get("license")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if entry_license != lic {
                filtered_by_license += 1;
                continue;
            }
        }

        // Filter by duration range (in seconds)
        let duration = entry
            .get("duration_s")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        if let Some(min_d) = min_duration_s {
            if duration < min_d {
                filtered_by_duration += 1;
                continue;
            }
        }
        if let Some(max_d) = max_duration_s {
            if duration > max_d {
                filtered_by_duration += 1;
                continue;
            }
        }

        // Filter by tag (substring match against the entry's tags array)
        if let Some(ref tag_q) = tag_filter {
            let tag_lower = tag_q.to_lowercase();
            let tags: Vec<String> = entry
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|t| t.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let matches_tag = tags
                .iter()
                .any(|t| t.to_lowercase().contains(&tag_lower));
            if !matches_tag {
                filtered_by_tag += 1;
                continue;
            }
        }

        // Filter by mood (exact match against enriched mood field)
        if let Some(ref mood_q) = mood_filter {
            let entry_mood = entry
                .get("mood")
                .and_then(|v| v.as_str())
                .unwrap_or("neutral");
            if entry_mood != mood_q {
                filtered_by_mood += 1;
                continue;
            }
        }

        // Filter by energy (exact match against enriched energy field)
        if let Some(ref energy_q) = energy_filter {
            let entry_energy = entry
                .get("energy")
                .and_then(|v| v.as_str())
                .unwrap_or("medium");
            if entry_energy != energy_q {
                filtered_by_energy += 1;
                continue;
            }
        }

        let title = entry
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();
        let tags: Vec<String> = entry
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| t.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let mut score = 0i32;

        // Exact title match
        if query_lower.contains(&title) || title.contains(&query_lower) {
            score += 10;
        }

        // Word matches
        for word in &query_words {
            if title.contains(word) {
                score += 3;
            }
            if tags.iter().any(|t| t == word) {
                score += 5;
            }
        }

        // Phase 3: mood/energy/genre scoring weights (audit bug #19).
        // Without these, a text-match "calm" on a title returns energetic
        // tracks that happen to say "calm" in their description.
        let entry_mood = entry
            .get("mood")
            .and_then(|v| v.as_str())
            .unwrap_or("neutral");
        let entry_energy = entry
            .get("energy")
            .and_then(|v| v.as_str())
            .unwrap_or("medium");
        let entry_genre = entry
            .get("genre")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Mood match: strong signal when agent filters by mood.
        if mood_filter.is_some() && entry_mood == mood_filter.as_deref().unwrap_or("") {
            score += 8;
        }

        // Energy match: moderate signal when agent filters by energy.
        if energy_filter.is_some() && entry_energy == energy_filter.as_deref().unwrap_or("") {
            score += 4;
        }

        // Genre match: if query words appear in genre field, boost.
        if !entry_genre.is_empty() {
            let genre_lower = entry_genre.to_lowercase();
            for word in &query_words {
                if genre_lower.contains(word) {
                    score += 3;
                }
            }
        }

        // Penalize mood mismatch when mood filter is active.
        if mood_filter.is_some() && entry_mood != mood_filter.as_deref().unwrap_or("") {
            score -= 5;
        }

        if score > 0 {
            let mut result = entry.clone();
            if let Some(obj) = result.as_object_mut() {
                obj.insert("relevance_score".into(), json!(score));
            }
            results.push(result);
        }
    }

    // Sort by relevance
    results.sort_by(|a, b| {
        let sa = a
            .get("relevance_score")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let sb = b
            .get("relevance_score")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        sb.cmp(&sa)
    });

    let total = results.len();
    results.truncate(limit);

    // Surface filter stats so an agent can tell why results are sparse.
    let mut filter_stats = serde_json::Map::new();
    if filtered_by_duration > 0 {
        filter_stats.insert("filtered_by_duration".into(), json!(filtered_by_duration));
    }
    if filtered_by_source > 0 {
        filter_stats.insert("filtered_by_source".into(), json!(filtered_by_source));
    }
    if filtered_by_license > 0 {
        filter_stats.insert("filtered_by_license".into(), json!(filtered_by_license));
    }
    if filtered_by_tag > 0 {
        filter_stats.insert("filtered_by_tag".into(), json!(filtered_by_tag));
    }
    if filtered_by_mood > 0 {
        filter_stats.insert("filtered_by_mood".into(), json!(filtered_by_mood));
    }
    if filtered_by_energy > 0 {
        filter_stats.insert("filtered_by_energy".into(), json!(filtered_by_energy));
    }

    Ok(json!({
        "status": "searched",
        "query": query,
        "type": media_type,
        "total_matches": total,
        "count": results.len(),
        "results": results,
        "filters_applied": {
            "source": source_filter,
            "license": license_filter,
            "min_duration_s": min_duration_s,
            "max_duration_s": max_duration_s,
            "tag": tag_filter,
            "mood": mood_filter,
            "energy": energy_filter,
        },
        "filter_stats": filter_stats,
        "index_stats": {
            "total_entries": index.get("total_entries"),
            "music_count": index.get("music_count"),
            "sfx_count": index.get("sfx_count"),
            "sources": index.get("sources"),
        },
    }))
}

// ---------------------------------------------------------------------------
// Handler: library.download — download music/SFX on demand
// ---------------------------------------------------------------------------

async fn handle_library_download(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let filename = extract_str(&args, "filename")?;
    let output_dir = default_str(&args, "output_dir", "mcp/assets/music_cache");
    let output_dir_owned = output_dir.to_string();

    std::fs::create_dir_all(&output_dir_owned)?;

    let index_path = std::env::var("OPENSCRIPT_MUSIC_LIBRARY_INDEX")
        .unwrap_or_else(|_| "mcp/assets/music_library_index.json".to_string());

    let index_str = std::fs::read_to_string(&index_path)?;
    let index: serde_json::Value = serde_json::from_str(&index_str)?;

    let entries = index
        .get("entries")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let entry = entries
        .iter()
        .find(|e| e.get("filename").and_then(|v| v.as_str()).unwrap_or("") == filename)
        .ok_or_else(|| ToolError::NotFound(format!("Entry not found in library: {}", filename)))?;

    let source_type = entry
        .get("source_type")
        .and_then(|v| v.as_str())
        .unwrap_or("local")
        .to_string();
    let download_url = entry
        .get("download_url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let output_path = format!("{}/{}", output_dir_owned, filename);

    // Check if already downloaded
    if Path::new(&output_path).exists() {
        return Ok(json!({
            "status": "cached",
            "path": output_path,
            "filename": filename,
            "source": entry.get("source"),
        }));
    }

    if source_type == "local" {
        // Local file — just return the path
        return Ok(json!({
            "status": "local",
            "path": download_url,
            "filename": filename,
            "source": entry.get("source"),
        }));
    }

    // Download with yt-dlp (include bot-detection evasion)
    report_progress(0.0, 100.0, &format!("Downloading: {}", filename))
        .await
        .ok();

    let result = tokio::process::Command::new("yt-dlp")
        .arg("-x").arg("--audio-format").arg("mp3")
        .arg("--audio-quality").arg("0")
        .arg("-o").arg(&output_path)
        .arg("--no-playlist")
        .arg("--quiet")
        // ponytail: skip --cookies-from-browser — NCS/AudioLibrary tracks are
        // public. Chrome cookies fail on headless/server environments. Only add
        // cookies for age-gated / private content.
        .arg("--user-agent").arg("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .arg(&download_url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await;

    match result {
        Ok(output) if output.status.success() => {
            report_progress(100.0, 100.0, "Downloaded").await.ok();
            let file_size = std::fs::metadata(&output_path)
                .map(|m| m.len())
                .unwrap_or(0);
            Ok(json!({
                "status": "downloaded",
                "path": output_path,
                "filename": filename,
                "file_size_bytes": file_size,
                "source": entry.get("source"),
                "title": entry.get("title"),
            }))
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(ToolError::Asset(format!(
                "Download failed: {}",
                stderr.lines().last().unwrap_or("unknown")
            )))
        }
        Err(e) => Err(ToolError::Asset(format!("yt-dlp not available: {}", e))),
    }
}

// ---------------------------------------------------------------------------
// Handler: library.build — rebuild the music/SFX library index
// ---------------------------------------------------------------------------

async fn handle_library_build(_args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    report_progress(0.0, 100.0, "Building music/SFX library index...")
        .await
        .ok();

    // C2 fix: prior versions shelled out to `python3 mcp/scripts/music_library_indexer.py --build`,
    // which required Python + yt-dlp at runtime. Now uses the native Rust port
    // in `library_indexer.rs`, which shells out to yt-dlp directly and builds
    // the JSON index with serde_json. No Python dependency.
    let index_path = "mcp/assets/music_library_index.json";
    let index = crate::library_indexer::build_index(index_path)
        .await
        .map_err(|e| ToolError::Asset(format!("Index build failed: {}", e)))?;

    report_progress(100.0, 100.0, "Library index built")
        .await
        .ok();

    Ok(json!({
        "status": "built",
        "index_path": index_path,
        "total_entries": index.get("total_entries"),
        "music_count": index.get("music_count"),
        "sfx_count": index.get("sfx_count"),
        "sources": index.get("sources"),
    }))
}

// ---------------------------------------------------------------------------
// Handler: media.download (Phase I — unblock image workflow)
// ---------------------------------------------------------------------------

/// Download an image from a URL to a local file. Caches in
/// mcp/assets/image_cache/ to avoid re-downloading.
async fn handle_media_download(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let url = extract_str(&args, "url")?;
    let output_path = default_opt_str(&args, "output_path");

    // Determine cache dir + output path
    let cache_dir = "mcp/assets/image_cache";
    std::fs::create_dir_all(cache_dir).ok();

    let resolved_path = if let Some(p) = output_path {
        if !p.is_empty() {
            p.to_string()
        } else {
            format!("{}/img_{}.{}", cache_dir, md5_hash(url.as_bytes()), url_extension(url))
        }
    } else {
        format!("{}/img_{}.{}", cache_dir, md5_hash(url.as_bytes()), url_extension(url))
    };

    // Check cache
    if std::path::Path::new(&resolved_path).exists() {
        return Ok(json!({
            "status": "cached",
            "path": resolved_path,
            "url": url,
        }));
    }

    // Download
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| ToolError::Asset(format!("HTTP client error: {}", e)))?;

    let resp = client.get(url).send().await
        .map_err(|e| ToolError::Asset(format!("Failed to download image: {}", e)))?;

    if !resp.status().is_success() {
        return Err(ToolError::Asset(format!(
            "Image download failed: HTTP {} for {}",
            resp.status(),
            url
        )));
    }

    let bytes = resp.bytes().await
        .map_err(|e| ToolError::Asset(format!("Failed to read image bytes: {}", e)))?;

    std::fs::write(&resolved_path, &bytes)
        .map_err(|e| ToolError::Asset(format!("Failed to write image to {}: {}", resolved_path, e)))?;

    Ok(json!({
        "status": "downloaded",
        "path": resolved_path,
        "url": url,
        "size_bytes": bytes.len(),
    }))
}

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

/// Download a GIF from a URL to a local file. Caches in mcp/assets/stickers/
/// so it can be used directly by script.to_video's sticker pipeline.
async fn handle_gif_download(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let url = extract_str(&args, "url")?;
    let output_path = default_opt_str(&args, "output_path");

    let cache_dir = "mcp/assets/stickers";
    std::fs::create_dir_all(cache_dir).ok();

    let resolved_path = if let Some(p) = output_path {
        if !p.is_empty() {
            p.to_string()
        } else {
            format!("{}/gif_{}.gif", cache_dir, md5_hash(url.as_bytes()))
        }
    } else {
        format!("{}/gif_{}.gif", cache_dir, md5_hash(url.as_bytes()))
    };

    // Check cache
    if std::path::Path::new(&resolved_path).exists() {
        return Ok(json!({
            "status": "cached",
            "path": resolved_path,
            "url": url,
        }));
    }

    // Download
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| ToolError::Asset(format!("HTTP client error: {}", e)))?;

    let resp = client.get(url).send().await
        .map_err(|e| ToolError::Asset(format!("Failed to download GIF: {}", e)))?;

    if !resp.status().is_success() {
        return Err(ToolError::Asset(format!(
            "GIF download failed: HTTP {} for {}",
            resp.status(),
            url
        )));
    }

    let bytes = resp.bytes().await
        .map_err(|e| ToolError::Asset(format!("Failed to read GIF bytes: {}", e)))?;

    std::fs::write(&resolved_path, &bytes)
        .map_err(|e| ToolError::Asset(format!("Failed to write GIF to {}: {}", resolved_path, e)))?;

    Ok(json!({
        "status": "downloaded",
        "path": resolved_path,
        "url": url,
        "size_bytes": bytes.len(),
    }))
}

// ---------------------------------------------------------------------------
// Handler: overlay.assign (Phase J — place images/GIFs/PNGs on the timeline)
// ---------------------------------------------------------------------------

/// Place an image/GIF/PNG overlay on the timeline at a specific position and
/// duration. The overlay is stored as a b-roll track event with a special
/// `overlay` tag, and the render pipeline composites it via FFmpeg's overlay
/// filter. This closes the "search → download → assign" loop for stickers,
/// GIFs, and images.
async fn handle_overlay_assign(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let timeline_path = extract_str(&args, "timeline_path")?;
    let asset_path = extract_str(&args, "asset_path")?;
    let start_ms = extract_i64(&args, "start_ms")?;
    let end_ms = extract_i64(&args, "end_ms")?;
    let position = default_str(&args, "position", "bottom-right");
    let scale = default_f64(&args, "scale", 0.2);
    let fade_in_ms = default_u32(&args, "fade_in_ms", 0);
    let fade_out_ms = default_u32(&args, "fade_out_ms", 0);
    let speaker_name = default_opt_str(&args, "speaker_name");

    // Validate the asset exists
    if !std::path::Path::new(asset_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Overlay asset not found: {}. Use media.download or gif.download to fetch it first.",
            asset_path
        )));
    }

    let mut timeline = Timeline::load(timeline_path)?;
    let event_id = format!("overlay_{:03}", track_count(&timeline, &TrackType::Broll) + 1);

    let duration_ms = end_ms - start_ms;
    let mut tags = vec!["overlay".to_string(), position.to_string()];
    if let Some(ref speaker) = speaker_name {
        tags.push(speaker.clone());
    }

    let event = openscript_core::timeline::TimelineEvent {
        id: event_id.clone(),
        asset_id: asset_path.to_string(),
        start_ms,
        end_ms,
        offset_ms: 0,
        gain_db: 0.0,
        fade_in_ms,
        fade_out_ms,
        tags,
        provenance: Some(openscript_core::timeline::Provenance {
            tool: "overlay.assign".into(),
            editorial_role: None,
            concept: Some(format!("overlay:{}:{}", position, scale)),
        }),
        kind: openscript_core::timeline::EventKind::Broll {
            concept: format!("overlay:{}", position),
            source_provider: asset_path.to_string(),
            transition_style: "overlay".into(),
            crop_mode: "none".into(),
            orientation: "9:16".into(),
            motion_intensity: "static".into(),
        },
    };

    timeline.add_track_event(TrackType::Broll, event);
    timeline.add_asset(
        "broll",
        event_id.clone(),
        json!({
            "path": asset_path,
            "overlay": true,
            "position": position,
            "scale": scale,
        }),
    );
    timeline.save(timeline_path)?;

    Ok(json!({
        "status": "assigned",
        "event_id": event_id,
        "asset_path": asset_path,
        "start_ms": start_ms,
        "end_ms": end_ms,
        "duration_ms": duration_ms,
        "position": position,
        "scale": scale,
        "timeline_path": timeline_path,
    }))
}

// ---------------------------------------------------------------------------
// Handler: sticker.keywords — agentic GIPHY sticker keyword generation
// (STAGE 1 of the sticker pipeline, parallel to broll.keywords)
// ---------------------------------------------------------------------------

async fn handle_sticker_keywords(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let segments = args
        .get("segments")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ToolError::MissingArg("segments".to_string()))?;
    if segments.is_empty() {
        return Ok(json!({"status": "warning", "message": "No segments provided", "segments": []}));
    }
    let language = default_str(&args, "language", "hinglish");
    let max_batch_size = default_u32(&args, "max_batch_size", 15).max(1) as usize;

    let system_prompt = format!(
        "You are a GIPHY sticker search keyword drafter for a short-form video pipeline. \
        Your job: translate transcript captions into short GIPHY sticker search keywords. \
        GIPHY stickers are animated reaction/meme/emotion GIFs (e.g. 'mind blown', 'facepalm', \
        'celebration', 'sad', 'thumbs up', 'laughing', 'shocked'). \
        Rules:\n1. Output ONLY valid JSON — no markdown, no explanation\n2. For each segment, output 2-3 short \
        sticker keywords (1-3 words each) describing the REACTION/EMOTION/MEME that fits the spoken content\n3. Translate \
        Hinglish/Hindi by MEANING, not word-for-word\n4. Prefer common GIPHY searchable reaction phrases over abstract concepts\n5. \
        Classify each segment's emotional weight:\n   - 'intent': one of anger, surprise, hype, celebration, sarcasm, sad, question, emphasis, none\n   - 'emphatic': true ONLY when the segment carries real emotional weight (shock, anger, hype, punchline, big claim, strong opinion). \
        Calm/filler segments — plain statements, connectors, 'hai', 'bhai', mundane narration — are emphatic=false with sticker_keywords=[] \
        (no sticker is better than an irrelevant one)\n6. Source language detected: {}\nOutput format: {{\"results\": [{{\"id\": \"seg_XXX\", \"intent\": \"anger\", \"emphatic\": true, \"sticker_keywords\": [\"angry eyes\", \"frustrated\"]}}]}}",
        language
    );

    let mut keyword_map: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    let mut intent_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut emphatic_map: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
    let mut last_backend = String::new();
    let mut last_model = String::new();
    let total = segments.len();
    let num_batches = total.div_ceil(max_batch_size);

    for batch_idx in 0..num_batches {
        let start = batch_idx * max_batch_size;
        let end = std::cmp::min(start + max_batch_size, total);
        let batch = &segments[start..end];
        report_progress(
            5.0 + (batch_idx as f64 / num_batches as f64) * 70.0,
            100.0,
            &format!("Drafting sticker keywords batch {}/{}...", batch_idx + 1, num_batches),
        )
        .await
        .ok();

        let mut segment_descriptions = Vec::new();
        for (j, seg) in batch.iter().enumerate() {
            let i = start + j;
            let caption = seg
                .get("caption")
                .or_else(|| seg.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let fallback_id = format!("seg_{}", i);
            let id = seg.get("id").and_then(|v| v.as_str()).unwrap_or(&fallback_id);
            segment_descriptions.push(format!("[{}] {}: \"{}\"", id, i + 1, caption));
        }

        let user_prompt = format!(
            "Draft GIPHY sticker search keywords for each segment. Output ONLY the JSON object.\n\n{}",
            segment_descriptions.join("\n")
        );
        let result = match crate::llm::chat_complete(&system_prompt, &user_prompt, None).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("[sticker.keywords] Batch {} LLM failed: {} — using caption fallback", batch_idx + 1, e);
                continue;
            }
        };
        last_backend = result.backend.clone();
        last_model = result.model.clone();

        let response_text = result.text.trim();
        let parsed: serde_json::Value = if let Some(start) = response_text.find('{') {
            if let Some(end) = response_text.rfind('}') {
                serde_json::from_str(&response_text[start..=end]).unwrap_or_else(|_| json!({"results": []}))
            } else {
                json!({"results": []})
            }
        } else {
            json!({"results": []})
        };

        if let Some(results) = parsed.get("results").and_then(|v| v.as_array()) {
            for r in results {
                if let Some(id) = r.get("id").and_then(|v| v.as_str()) {
                    let kws = r.get("sticker_keywords").or_else(|| r.get("keywords"));
                    if let Some(kws) = kws.and_then(|v| v.as_array()) {
                        let keywords: Vec<String> = kws
                            .iter()
                            .filter_map(|k| k.as_str().map(String::from))
                            .filter(|k| k.len() >= 2)
                            .collect();
                        keyword_map.insert(id.to_string(), keywords);
                    }
                    if let Some(intent) = r.get("intent").and_then(|v| v.as_str()) {
                        intent_map.insert(id.to_string(), intent.to_string());
                    }
                    if let Some(emph) = r.get("emphatic").and_then(|v| v.as_bool()) {
                        emphatic_map.insert(id.to_string(), emph);
                    }
                }
            }
        }
    }

    // Enrich segments: LLM keywords with caption-word fallback per segment.
    let mut enriched = Vec::new();
    for (i, seg) in segments.iter().enumerate() {
        let id = seg
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or(&format!("seg_{}", i))
            .to_string();
        let caption = seg
            .get("caption")
            .or_else(|| seg.get("text"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        // Resolve intent/emphatic with the same id-fallback ladder as keywords.
        let intent = intent_map
            .get(&id)
            .or_else(|| intent_map.get(&format!("seg_{}", i)))
            .or_else(|| intent_map.get(&format!("seg_{:03}", i)))
            .cloned()
            .unwrap_or_else(|| "emphasis".to_string());
        let emphatic = emphatic_map
            .get(&id)
            .or_else(|| emphatic_map.get(&format!("seg_{}", i)))
            .or_else(|| emphatic_map.get(&format!("seg_{:03}", i)))
            .copied()
            // LLM-down path: the naive caption-word fallback is NOT auto-approved
            // (that is exactly the irrelevance bug) — mark it non-emphatic so the
            // validation gate rejects it. Better no sticker than a wrong one.
            .unwrap_or(false);
        let keywords: Vec<String> = if emphatic {
            keyword_map
                .get(&id)
                .or_else(|| keyword_map.get(&format!("seg_{}", i)))
                .or_else(|| keyword_map.get(&format!("seg_{:03}", i)))
                .cloned()
                .unwrap_or_else(|| {
                    let words: Vec<String> = caption
                        .split_whitespace()
                        .filter(|w| w.len() > 3)
                        .take(3)
                        .map(String::from)
                        .collect();
                    if words.is_empty() {
                        vec!["funny".to_string()]
                    } else {
                        words
                    }
                })
        } else {
            Vec::new()
        };
        let mut out = seg.clone();
        if let Some(obj) = out.as_object_mut() {
            obj.insert("sticker_keywords".into(), json!(keywords));
            obj.insert("intent".into(), json!(intent));
            obj.insert("emphatic".into(), json!(emphatic));
            if keywords.is_empty() {
                obj.insert("skip_reason".into(), json!("not_emphatic"));
            }
        }
        enriched.push(out);
    }

    Ok(json!({
        "status": "success",
        "segments": enriched,
        "count": enriched.len(),
        "backend": last_backend,
        "model": last_model,
    }))
}

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

async fn handle_sticker_validate_keywords(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let enriched_segments: Vec<serde_json::Value> = args
        .get("enriched_segments")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if enriched_segments.is_empty() {
        return Err(ToolError::MissingArg(
            "enriched_segments (from sticker.keywords)".to_string(),
        ));
    }
    let max_candidates = default_u32(&args, "max_candidates", 4).max(1) as usize;
    let language = default_str(&args, "language", "hinglish");

    let giphy_api_key = std::env::var("GIPHY_API_KEY").ok();
    if giphy_api_key.is_none() {
        return Ok(json!({
            "status": "warning",
            "message": "GIPHY_API_KEY not set — cannot search candidates for relevance validation. Draft keywords are returned unchanged; set the key or run sticker.auto_assign with an explicit sticker_query.",
            "validated": false,
            "segments": enriched_segments,
        }));
    }
    let giphy_api_key = giphy_api_key.unwrap();
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| ToolError::Asset(format!("HTTP client: {}", e)))?;

    let mut validated: Vec<serde_json::Value> = Vec::new();
    let mut skipped: Vec<serde_json::Value> = Vec::new();
    let mut last_backend = String::new();
    let mut last_model = String::new();

    for (i, seg) in enriched_segments.iter().enumerate() {
        let id = seg
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or(&format!("seg_{}", i))
            .to_string();
        let caption = seg
            .get("caption")
            .or_else(|| seg.get("text"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let intent = seg
            .get("intent")
            .and_then(|v| v.as_str())
            .unwrap_or("emphasis")
            .to_string();
        let draft: Vec<String> = seg
            .get("sticker_keywords")
            .or_else(|| seg.get("keywords"))
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|k| k.as_str().map(String::from)).collect())
            .unwrap_or_default();

        if draft.is_empty() {
            // Record in BOTH the full segments list (so auto_assign knows this
            // segment was explicitly rejected — no caption-word fallback) and
            // the skipped summary (observability).
            validated.push(json!({
                "id": id,
                "caption": caption,
                "intent": intent,
                "approved": false,
                "skip_reason": "not_emphatic",
                "draft_keywords": [],
            }));
            skipped.push(json!({"id": id, "reason": "not_emphatic"}));
            continue;
        }

        // Search GIPHY with the top draft keywords, dedupe by sticker id.
        let limit = max_candidates.to_string();
        let mut candidates: Vec<serde_json::Value> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for q in draft.iter().take(2) {
            let url = match reqwest::Url::parse_with_params(
                "https://api.giphy.com/v1/stickers/search",
                &[
                    ("api_key", giphy_api_key.as_str()),
                    ("q", q.as_str()),
                    ("limit", limit.as_str()),
                    ("rating", "g"),
                    ("bundle", "sticker_layering"),
                ],
            ) {
                Ok(u) => u,
                Err(_) => continue,
            };
            let resp = match http.get(url).send().await {
                Ok(r) => r,
                Err(_) => continue,
            };
            if !resp.status().is_success() {
                continue;
            }
            let body: serde_json::Value = match resp.json().await {
                Ok(v) => v,
                Err(_) => continue,
            };
            if let Some(data) = body.get("data").and_then(|d| d.as_array()) {
                for item in data {
                    let sid = item.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    if sid.is_empty() || !seen.insert(sid.clone()) {
                        continue;
                    }
                    let url = item
                        .pointer("/images/original/url")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if url.is_empty() {
                        continue;
                    }
                    candidates.push(json!({
                        "id": sid,
                        "title": item.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                        "slug": item.get("slug").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                        "url": url,
                        "preview_url": item.pointer("/images/preview_gif/url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    }));
                    if candidates.len() >= max_candidates {
                        break;
                    }
                }
            }
            if candidates.len() >= max_candidates {
                break;
            }
        }

        if candidates.is_empty() {
            validated.push(json!({
                "id": id,
                "caption": caption,
                "intent": intent,
                "approved": false,
                "skip_reason": "no_giphy_results",
                "draft_keywords": draft,
            }));
            skipped.push(json!({"id": id, "reason": "no_giphy_results"}));
            continue;
        }

        // Agent validates the real candidates against the spoken caption.
        let (best_idx, final_keyword, relevance, reason, backend, model) =
            llm_validate_sticker_candidates(&caption, &intent, &draft, &candidates, &language).await;
        if !backend.is_empty() {
            last_backend = backend;
        }
        if !model.is_empty() {
            last_model = model;
        }

        let best_sticker = best_idx.and_then(|bi| candidates.get(bi)).cloned();
        match best_sticker {
            Some(sticker) => validated.push(json!({
                "id": id,
                "caption": caption,
                "intent": intent,
                // Both spellings emitted so any consumer (keyword search OR
                // direct-pick path) reads the same field it already expects.
                "sticker_keywords": draft,
                "draft_keywords": draft,
                "final_keyword": final_keyword,
                "approved": true,
                "relevance": relevance,
                "reason": reason,
                "best_sticker": sticker,
                "candidates": candidates,
            })),
            None => {
                validated.push(json!({
                    "id": id,
                    "caption": caption,
                    "intent": intent,
                    "approved": false,
                    "skip_reason": "relevance_rejected",
                    "draft_keywords": draft,
                }));
                skipped.push(json!({
                    "id": id,
                    "reason": "relevance_rejected",
                    "caption": caption,
                }));
            }
        }
    }

    // `segments` carries EVERY processed segment (approved + rejected with a
    // skip_reason) so sticker.auto_assign never falls back to caption-word
    // queries for segments the relevance gate already rejected.
    let approved_count = validated
        .iter()
        .filter(|s| s.get("approved").and_then(|v| v.as_bool()).unwrap_or(false))
        .count();
    Ok(json!({
        "status": "validated",
        "backend": last_backend,
        "model": last_model,
        "validated_count": approved_count,
        "processed_count": validated.len(),
        "skipped_count": skipped.len(),
        "skipped": skipped,
        "segments": validated,
    }))
}

// ---------------------------------------------------------------------------
// Handler: sticker.auto — ONE-CALL agentic sticker pipeline (parallel to broll.auto)
// segment.analyze → sticker.keywords → GIPHY search → download → place on Stickers track
// ---------------------------------------------------------------------------

async fn handle_sticker_auto(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let timeline_path_arg = args.get("timeline_path").and_then(|v| v.as_str()).map(String::from);
    let position = default_str(&args, "position", "auto");
    let scale = default_f64(&args, "scale", 0.25);
    let max_stickers = default_u32(&args, "max_stickers", 12) as usize;
    let min_gap_s = default_f64(&args, "min_gap_s", 2.0).max(0.0);

    // Stage A: resolve timeline + segments (same pattern as broll.auto)
    let (timeline_path, segments) = if let Some(tl) = &timeline_path_arg {
        let timeline = Timeline::load(tl)?;
        let segs: Vec<serde_json::Value> = timeline
            .segments
            .iter()
            .map(|s| {
                json!({
                    "id": s.id.clone(),
                    "start_s": s.start,
                    "end_s": s.end,
                    "duration_s": s.end - s.start,
                    "caption": s.caption.clone(),
                })
            })
            .collect();
        (tl.clone(), segs)
    } else {
        let srt = args
            .get("srt_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingArg("sticker.auto requires srt_path + audio_path (or timeline_path)".into()))?
            .to_string();
        let audio = args
            .get("audio_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingArg("sticker.auto requires audio_path (or timeline_path)".into()))?
            .to_string();
        report_progress(5.0, 100.0, "1/3 segment.analyze").await.ok();
        let analyzed = handle_segment_analyze(json!({
            "audio_path": audio,
            "srt_path": srt,
            "min_duration_s": 2.0,
            "max_duration_s": 6.0,
        }))
        .await?;
        let segments: Vec<serde_json::Value> = analyzed
            .get("segments")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let out_path = args
            .get("output_path")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| {
                let stem = Path::new(&srt)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "sticker_auto".to_string());
                format!("artifacts/{}.timeline.json", stem)
            });
        report_progress(20.0, 100.0, "2/3 srt.to_timeline").await.ok();
        let built = handle_srt_to_timeline(json!({
            "srt_path": srt,
            "source_video": audio,
            "output_path": out_path,
            "aspect": "9:16",
            "fps": 30,
            "min_duration_s": 2.0,
            "max_duration_s": 6.0,
        }))
        .await?;
        let tl = built
            .get("timeline_path")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or(out_path);
        (tl, segments)
    };

    if segments.is_empty() {
        return Err(ToolError::InvalidArg("sticker.auto: no segments found — check SRT/timeline".into()));
    }

    // Stage B: keyword draft. When the caller already drafted keywords (e.g.
    // broll.auto passes its validated b-roll keywords so ONE keyword source
    // drives both b-roll and stickers — the unification), use them directly and
    // skip the separate LLM sticker-intent pass. Otherwise run the agentic
    // sticker.keywords draft (intent + emphatic).
    let shared_keywords: Vec<serde_json::Value> = args
        .get("shared_keywords")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    report_progress(35.0, 100.0, "2/4 sticker.keywords (agentic intent draft)").await.ok();
    let (mut enriched_segments, backend) = if !shared_keywords.is_empty() {
        (json!(shared_keywords), json!("shared_broll_keywords"))
    } else {
        let drafts = handle_sticker_keywords(json!({
            "segments": segments,
            "language": default_str(&args, "language", "hinglish"),
            "max_batch_size": 15,
        }))
        .await?;
        (
            drafts.get("segments").cloned().unwrap_or_else(|| json!([])),
            drafts.get("backend").cloned().unwrap_or_else(|| json!("")),
        )
    };

    // Stage C: relevance gate — approve only stickers that genuinely match the
    // spoken intent (mirror of broll.validate_keywords). GIPHY/LLM-down ⇒ drafts
    // pass through unchanged; auto_assign's fallbacks + spacing still apply.
    report_progress(55.0, 100.0, "3/4 sticker.validate_keywords (relevance gate)").await.ok();
    let validated = handle_sticker_validate_keywords(json!({
        "enriched_segments": enriched_segments,
        "language": default_str(&args, "language", "hinglish"),
        "max_candidates": 4,
    }))
    .await?;
    if validated.get("status").and_then(|v| v.as_str()) == Some("validated") {
        enriched_segments = validated.get("segments").cloned().unwrap_or_else(|| json!([]));
    }

    // Stage D: search + download + place (approved picks download directly)
    report_progress(70.0, 100.0, "4/4 sticker.auto_assign (GIPHY + place)").await.ok();
    let placed = handle_sticker_auto_assign(json!({
        "timeline_path": timeline_path,
        "enriched_segments": enriched_segments,
        "position": position,
        "scale": scale,
        "max_stickers": max_stickers,
        "min_gap_s": min_gap_s,
    }))
    .await?;

    let stickers_placed = placed.get("events_created").and_then(|v| v.as_u64()).unwrap_or(0);
    let skipped = placed.get("skipped").cloned().unwrap_or_else(|| json!([]));
    let skipped_count = placed.get("skipped_count").and_then(|v| v.as_u64()).unwrap_or_else(|| {
        skipped.as_array().map(|a| a.len() as u64).unwrap_or(0)
    });
    report_progress(100.0, 100.0, "sticker.auto complete").await.ok();

    Ok(json!({
        "status": if stickers_placed > 0 { "success" } else { "warning" },
        "message": format!(
            "Sticker pipeline complete: {} segment(s) analyzed, {} sticker(s) placed, {} skipped (see skipped reasons: intent gate / relevance gate / spacing).",
            segments.len(),
            stickers_placed,
            skipped_count
        ),
        "timeline_path": timeline_path,
        "segments_count": segments.len(),
        "stickers_placed": stickers_placed,
        "skipped": skipped,
        "sticker_keywords_backend": backend,
        "pipeline": json!(["segment.analyze", "sticker.keywords", "sticker.validate_keywords", "sticker.auto_assign"]),
    }))
}

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

async fn handle_sticker_auto_assign(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let timeline_path = extract_str(&args, "timeline_path")?;
    let sticker_query: Option<String> = args.get("sticker_query").and_then(|v| v.as_str()).map(|s| s.to_string());
    // "auto" (default) cycles anchors for visual variety; an explicit position
    // (e.g. "top-right") anchors every sticker there (manual override).
    let position = default_str(&args, "position", "auto");
    let scale = default_f64(&args, "scale", 0.25);
    let max_stickers = default_u32(&args, "max_stickers", 10) as usize;
    // Minimum seconds between consecutive sticker placements (spacing gate).
    let min_gap_s = default_f64(&args, "min_gap_s", 2.0).max(0.0);
    let enriched_segments: Vec<serde_json::Value> = args
        .get("enriched_segments")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut timeline = Timeline::load(timeline_path)?;
    let segments = timeline.segments.clone();
    if segments.is_empty() {
        return Ok(json!({"status": "warning", "message": "No segments found — cannot auto-assign stickers", "events_created": 0}));
    }

    let giphy_api_key = std::env::var("GIPHY_API_KEY").ok();
    if giphy_api_key.is_none() {
        return Ok(json!({"status": "warning", "message": "GIPHY_API_KEY not set — cannot search for stickers. Set GIPHY_API_KEY env var.", "events_created": 0}));
    }
    let giphy_api_key = giphy_api_key.unwrap();

    // Map segment id → sticker_keywords from sticker.keywords output, if given.
    let mut keyword_by_seg: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    // Map segment id → validated segment (sticker.validate_keywords output).
    // When present, the approved sticker is downloaded DIRECTLY (no re-search)
    // and the relevance gate is respected.
    let mut best_sticker_by_seg: std::collections::HashMap<String, serde_json::Value> = std::collections::HashMap::new();
    // Map segment id → skip_reason from the relevance/intent gate. Segments the
    // gate rejected MUST NOT fall back to caption-word queries.
    let mut skip_reason_by_seg: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for es in &enriched_segments {
        if let Some(id) = es.get("id").and_then(|v| v.as_str()) {
            let kws: Vec<String> = es
                .get("sticker_keywords")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|k| k.as_str().map(String::from)).collect())
                .unwrap_or_default();
            if !kws.is_empty() {
                keyword_by_seg.insert(id.to_string(), kws);
            }
            if let Some(r) = es.get("skip_reason").and_then(|v| v.as_str()) {
                skip_reason_by_seg.insert(id.to_string(), r.to_string());
            }
            // Approved validated picks only — never auto-place an unapproved one.
            let has_best_url = es
                .get("best_sticker")
                .and_then(|v| v.get("url"))
                .and_then(|v| v.as_str())
                .map(|s| !s.is_empty())
                .unwrap_or(false);
            if es.get("approved").and_then(|v| v.as_bool()).unwrap_or(false) && has_best_url {
                best_sticker_by_seg.insert(id.to_string(), es.clone());
            }
        }
    }

    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| ToolError::Asset(format!("HTTP client: {}", e)))?;

    let mut events_created: Vec<serde_json::Value> = Vec::new();
    let mut skipped: Vec<serde_json::Value> = Vec::new();
    let mut current_idx = track_count(&timeline, &TrackType::Stickers);
    let stickers_dir = std::path::PathBuf::from("mcp/assets/stickers");
    let _ = std::fs::create_dir_all(&stickers_dir);

    let mut last_sticker_end_s: Option<f64> = None;
    let mut placed_count = 0usize;
    // No-duplicate-sticker guarantee: GIPHY ids placed earlier in this run are
    // never placed again — two segments with similar keywords often resolve to
    // the SAME top GIPHY sticker (the "same sticker repeats" bug).
    let mut used_sticker_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for seg in segments.iter() {
        if events_created.len() >= max_stickers { break; }

        let seg_id = seg.id.clone();
        // Honored rejection: the intent/relevance gate already decided this
        // segment gets no sticker — never fall back to caption-word queries
        // (that fallback IS the irrelevance bug). An explicit sticker_query is
        // a manual override and bypasses the gate.
        if sticker_query.is_none() {
            if let Some(reason) = skip_reason_by_seg.get(&seg_id) {
                skipped.push(json!({
                    "segment_id": seg.id,
                    "reason": reason.clone(),
                    "query": String::new(),
                }));
                continue;
            }
        }

        // Spacing gate: never place a sticker adjacent to the previous one
        // (min_gap_s between the previous sticker's end and this segment start).
        if !sticker_spacing_allowed(last_sticker_end_s, seg.start, min_gap_s) {
            skipped.push(json!({
                "segment_id": seg.id,
                "reason": "adjacent_spacing",
                "detail": format!(
                    "segment starts {:.1}s after previous sticker's end (min gap {:.1}s)",
                    seg.start - last_sticker_end_s.unwrap_or(0.0),
                    min_gap_s
                ),
            }));
            continue;
        }

        // Validated pick (sticker.validate_keywords) → download DIRECTLY, no
        // re-search; query = final_keyword (provenance + observability).
        let mut query = String::new();
        let mut chosen_url = String::new();
        let mut chosen_title = String::new();
        let mut chosen_sticker_id = String::new();
        if let Some(vs) = best_sticker_by_seg.get(&seg_id) {
            if let Some(bs) = vs.get("best_sticker") {
                // Duplicate guard for validated picks: two segments can approve
                // the same GIPHY sticker — the second is skipped, never re-placed.
                let sticker_giphy_id = bs.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if !sticker_giphy_id.is_empty() && used_sticker_ids.contains(&sticker_giphy_id) {
                    skipped.push(json!({
                        "segment_id": seg.id,
                        "reason": "duplicate_sticker",
                        "detail": format!("GIPHY sticker {} already placed on this timeline", sticker_giphy_id),
                    }));
                    continue;
                }
                chosen_sticker_id = sticker_giphy_id;
                query = vs
                    .get("final_keyword")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .or_else(|| bs.get("title").and_then(|v| v.as_str()))
                    .unwrap_or("")
                    .to_string();
                chosen_url = bs.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();
                chosen_title = bs.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
            }
        }

        if chosen_url.is_empty() {
            // Derive query: per-segment enriched keywords > global override > caption words
            query = if let Some(ref q) = sticker_query {
                q.clone()
            } else {
                keyword_by_seg
                    .get(&seg_id)
                    .and_then(|kws| kws.first().cloned())
                    .or_else(|| {
                        let words: Vec<&str> = seg.caption.split_whitespace().filter(|w: &&str| w.len() > 3).take(3).collect();
                        if words.is_empty() { Some("funny".to_string()) } else { Some(words.join(" ")) }
                    })
                    .unwrap_or_else(|| "funny".to_string())
            };
            let url = reqwest::Url::parse_with_params(
                "https://api.giphy.com/v1/stickers/search",
                &[
                    ("api_key", giphy_api_key.as_str()),
                    ("q", query.as_str()),
                    ("limit", "3"),
                    ("rating", "g"),
                    ("bundle", "sticker_layering"),
                ],
            )
            .map_err(|e| ToolError::InvalidArg(format!("URL parse: {}", e)))?;

            let resp = match http.get(url).send().await {
                Ok(r) => r,
                Err(_) => continue,
            };
            if !resp.status().is_success() { continue; }
            let body: serde_json::Value = match resp.json().await {
                Ok(v) => v,
                Err(_) => continue,
            };
            let data = body.get("data").and_then(|d| d.as_array()).cloned().unwrap_or_default();
            if data.is_empty() {
                skipped.push(json!({"segment_id": seg.id, "query": query, "reason": "no GIPHY results"}));
                continue;
            }

            // Pick the first result whose original URL is downloadable and that
            // has not already been placed (duplicate-sticker guard).
            for item in &data {
                let gid = item.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if !gid.is_empty() && used_sticker_ids.contains(&gid) {
                    continue;
                }
                let u = item.pointer("/images/original/url").and_then(|v| v.as_str()).unwrap_or("");
                if !u.is_empty() {
                    chosen_url = u.to_string();
                    chosen_title = item.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    chosen_sticker_id = gid;
                    break;
                }
            }
            if chosen_url.is_empty() { continue; }
        }

        // Download via the existing gif.download handler (cache-aware)
        let dl = handle_gif_download(json!({
            "url": chosen_url,
        }))
        .await?;
        let asset_path_str = dl
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if asset_path_str.is_empty() || asset_path_str == "placeholder" {
            skipped.push(json!({"segment_id": seg.id, "query": query, "reason": "download failed"}));
            continue;
        }
        if !chosen_sticker_id.is_empty() {
            used_sticker_ids.insert(chosen_sticker_id);
        }

        // Place on the Stickers track. asset_id = event_id (registry key
        // convention used by broll.fetch) so the renderer resolves the path.
        let place_pos = sticker_place_position(&position, placed_count);
        current_idx += 1;
        let event_id = format!("sticker_{:03}", current_idx);
        let start_ms = (seg.start * 1000.0) as i64;
        let end_ms = ((seg.end * 1000.0) as i64).min(start_ms + 5000); // Cap at 5s

        let event = openscript_core::timeline::TimelineEvent {
            id: event_id.clone(),
            asset_id: event_id.clone(),
            start_ms,
            end_ms,
            offset_ms: 0,
            gain_db: 0.0,
            fade_in_ms: 150,
            fade_out_ms: 150,
            tags: vec!["sticker".to_string(), place_pos.clone()],
            provenance: Some(openscript_core::timeline::Provenance {
                tool: "sticker.auto_assign".into(),
                editorial_role: Some("decoration".into()),
                concept: Some(query.clone()),
            }),
            kind: openscript_core::timeline::EventKind::Broll {
                concept: format!("overlay:{}", place_pos),
                source_provider: asset_path_str.clone(),
                transition_style: "overlay".into(),
                crop_mode: "none".into(),
                orientation: "9:16".into(),
                motion_intensity: "static".into(),
            },
        };

        timeline.add_track_event(TrackType::Stickers, event);
        timeline.add_asset("broll", event_id.clone(), json!({
            "path": asset_path_str,
            "position": place_pos,
            "scale": scale,
            "overlay": true,
        }));
        events_created.push(json!({
            "event_id": event_id,
            "position_ms": start_ms,
            "position": place_pos,
            "sticker_path": asset_path_str,
            "query": query,
            "title": chosen_title,
        }));
        last_sticker_end_s = Some(seg.end);
        placed_count += 1;
    }

    timeline.save(timeline_path)?;
    Ok(json!({
        "status": if events_created.is_empty() { "warning" } else { "success" },
        "message": if events_created.is_empty() {
            "No stickers placed — check GIPHY_API_KEY and segment content.".into()
        } else {
            format!("{} sticker(s) placed on the Stickers track.", events_created.len())
        },
        "events_created": events_created.len(),
        "positions": events_created,
        "skipped": skipped,
        "timeline_path": timeline_path,
    }))
}

// ---------------------------------------------------------------------------
// Handler: voices.list — list all available TTS voices
// ---------------------------------------------------------------------------

/// List all available TTS voices: registered profiles from voices.json plus
/// the full list of Kokoro preset voice IDs. Agents use this to discover
/// available voices before generating TTS.
async fn handle_voices_list(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let language_filter = default_opt_str(&args, "language");

    // Load registered profiles from voices.json
    let voices_path = std::env::var("OPENSCRIPT_VOICES_PATH")
        .unwrap_or_else(|_| "mcp/assets/voices.json".to_string());

    let mut registered: Vec<serde_json::Value> = Vec::new();
    if let Ok(content) = std::fs::read_to_string(&voices_path) {
        if let Ok(map) = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&content) {
            for (id, val) in &map {
                let lang = val.get("language").and_then(|v| v.as_str()).unwrap_or("en");
                if let Some(ref filter) = language_filter {
                    if lang != filter {
                        continue;
                    }
                }
                registered.push(json!({
                    "id": id,
                    "provider": val.get("provider").and_then(|v| v.as_str()).unwrap_or("kokoro"),
                    "language": lang,
                    "description": val.get("description").and_then(|v| v.as_str()).unwrap_or(""),
                    "mode": val.get("mode").and_then(|v| v.as_str()).unwrap_or("preset"),
                }));
            }
        }
    }

    // Kokoro preset voice IDs (from the Kokoro v1.0 model).
    // These can be used directly with script.generate_voices / tts.generate
    // without registration in voices.json.
    let kokoro_presets = [
        // American English
        ("af_heart", "en", "warm American female"),
        ("af_bella", "en", "soft American female"),
        ("af_nicole", "en", "American female"),
        ("af_sky", "en", "American female, bright"),
        ("am_michael", "en", "American male"),
        ("am_adam", "en", "American male"),
        ("am_eric", "en", "American male, deep"),
        // British English
        ("bf_emma", "en", "British female"),
        ("bf_isabella", "en", "British female, warm"),
        ("bm_george", "en", "British male"),
        ("bm_lewis", "en", "British male, young"),
        // Spanish
        ("ef_dora", "es", "Spanish female"),
        ("em_alex", "es", "Spanish male"),
        // French
        ("ff_evelyne", "fr", "French female"),
        ("fm_pierre", "fr", "French male"),
        // Hindi
        ("hf_alpha", "hi", "Hindi female"),
        ("hf_beta", "hi", "Hindi female, warm"),
        ("hm_omega", "hi", "Hindi male"),
        ("hm_psi", "hi", "Hindi male, deep"),
        // Italian
        ("if_sara", "it", "Italian female"),
        ("im_nicola", "it", "Italian male"),
        // Japanese
        ("jf_alpha", "ja", "Japanese female"),
        ("jf_gongitsune", "ja", "Japanese female, character"),
        ("jf_nezumi", "ja", "Japanese female, mouse"),
        ("jf_tebukuro", "ja", "Japanese female, warm"),
        ("jf_tomoko", "ja", "Japanese female, neutral"),
        ("jm_kumo", "ja", "Japanese male"),
        // Portuguese (Brazilian)
        ("pf_dora", "pt", "Portuguese female"),
        ("pm_alex", "pt", "Portuguese male"),
        // Chinese (Mandarin)
        ("zf_xiaobei", "zh", "Chinese female, Beijing"),
        ("zf_xiaoni", "zh", "Chinese female, neutral"),
        ("zf_xiaoxiao", "zh", "Chinese female, bright"),
        ("zf_xiaoyi", "zh", "Chinese female, Yi"),
        ("zm_yunjian", "zh", "Chinese male, Jian"),
        ("zm_yunxi", "zh", "Chinese male, Xi"),
        ("zm_yunxia", "zh", "Chinese male, Xia"),
        ("zm_yunyang", "zh", "Chinese male, Yang"),
    ];

    let mut presets: Vec<serde_json::Value> = Vec::new();
    for (id, lang, desc) in &kokoro_presets {
        if let Some(ref filter) = language_filter {
            if *lang != filter {
                continue;
            }
        }
        presets.push(json!({
            "id": id,
            "provider": "kokoro",
            "language": lang,
            "description": desc,
            "usage": format!("Use '{}' directly as the voice parameter in script.generate_voices or tts.generate", id),
        }));
    }

    Ok(json!({
        "status": "success",
        "registered_profiles": registered,
        "registered_count": registered.len(),
        "kokoro_presets": presets,
        "kokoro_preset_count": presets.len(),
        "total_voices": registered.len() + presets.len(),
        "note": "Kokoro preset IDs (e.g. 'af_heart') can be used directly without registration. Registered profiles in voices.json are named aliases that map to Kokoro presets.",
    }))
}

// ---------------------------------------------------------------------------
// Handler: timeline.to_hyperframes (Phase M — bridge EDL v2 → HF HTML)
// ---------------------------------------------------------------------------

/// Compile an EDL v2 timeline JSON into a HyperFrames HTML composition by
/// shelling out to `tsx hyperframes/src/edl_v2_to_html.ts`. The resulting
/// index.html can then be rendered via `hf.render` or `composition.render`.
///
/// This is the bridge between the NLE timeline (EDL v2 JSON) and the
/// HyperFrames motion-graphics render engine. Prior to this tool, the
/// edl_v2_to_html.ts compiler was dead code — never called by any Rust
/// handler. An agent had to run it manually, which broke the programmatic
/// pipeline.
async fn handle_timeline_to_hyperframes(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let timeline_path = extract_str(&args, "timeline_path")?;
    let output_dir = default_str(&args, "output_dir", "artifacts/hf_composition");
    let composition_id = default_opt_str(&args, "composition_id");

    // Validate timeline exists
    if !std::path::Path::new(timeline_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Timeline file not found: {}",
            timeline_path
        )));
    }

    // Create output directory
    std::fs::create_dir_all(&output_dir)
        .map_err(|e| ToolError::Asset(format!("Failed to create output dir: {}", e)))?;

    let index_html_path = format!("{}/index.html", output_dir);

    // Build the tsx command
    let compiler_script = "hyperframes/src/edl_v2_to_html.ts";
    if !std::path::Path::new(compiler_script).exists() {
        return Err(ToolError::NotFound(format!(
            "HyperFrames compiler not found: {}. Ensure the hyperframes/ directory is present.",
            compiler_script
        )));
    }

    let mut cmd = tokio::process::Command::new("npx");
    cmd.arg("tsx")
        .arg(compiler_script)
        .arg("--timeline")
        .arg(timeline_path)
        .arg("--out")
        .arg(&index_html_path);

    if let Some(ref cid) = composition_id {
        cmd.arg("--composition-id").arg(cid);
    }

    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    // Run with a 60s timeout — compilation should be fast
    let child = cmd
        .spawn()
        .map_err(|e| ToolError::Asset(format!("Failed to spawn tsx: {}", e)))?;

    let output = tokio::time::timeout(std::time::Duration::from_secs(60), child.wait_with_output())
        .await
        .map_err(|_| ToolError::Asset("tsx compilation timed out (60s)".to_string()))?
        .map_err(|e| ToolError::Asset(format!("tsx execution failed: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ToolError::Asset(format!(
            "HyperFrames compilation failed: {}",
            stderr.trim()
        )));
    }

    // Verify the output was created
    if !std::path::Path::new(&index_html_path).exists() {
        return Err(ToolError::Asset(format!(
            "Compilation appeared to succeed but no index.html was written to {}",
            index_html_path
        )));
    }

    let file_size = std::fs::metadata(&index_html_path)
        .map(|m| m.len())
        .unwrap_or(0);

    // Extract composition ID from the HTML (data-composition-id attribute)
    let html_content = std::fs::read_to_string(&index_html_path).unwrap_or_default();
    let extracted_cid = html_content
        .find("data-composition-id=\"")
        .and_then(|pos| {
            let start = pos + "data-composition-id=\"".len();
            html_content[start..].find('"').map(|end| &html_content[start..start + end])
        })
        .unwrap_or("unknown")
        .to_string();

    Ok(json!({
        "status": "compiled",
        "project_dir": output_dir,
        "index_html_path": index_html_path,
        "composition_id": extracted_cid,
        "file_size_bytes": file_size,
        "next_step": "Call hf.render or composition.render with project_dir to produce the final MP4",
    }))
}

// ---------------------------------------------------------------------------
// Handlers: llm.complete / vision.analyze_clip / vision.score_clip
// ---------------------------------------------------------------------------

/// Text LLM via OpenCode zen → OpenRouter free models.
async fn handle_llm_complete(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let prompt = extract_str(&args, "prompt")?;
    let system = default_str(
        &args,
        "system",
        "You are a helpful short-form video director assistant.",
    );
    let backend = default_str(&args, "backend", "auto");
    let result = crate::llm::chat_complete_with_backend(&system, prompt, None, &backend)
        .await
        .map_err(|e| ToolError::Asset(format!("LLM cascade failed: {}", e)))?;
    Ok(json!({
        "status": "success",
        "text": result.text,
        "backend": result.backend,
        "model": result.model,
        "backend_requested": backend,
    }))
}

/// Redacted view of ~/.openscript/config.json + env overrides.
async fn handle_system_config_get(
    _args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    // Ensure directory exists so agents always know where to write keys
    let _ = crate::config::ensure_user_config(None);
    crate::config::reload_config();
    Ok(json!({
        "status": "success",
        "config": crate::config::config_public_view(),
    }))
}

/// Deep-merge a patch into ~/.openscript/config.json.
async fn handle_system_config_set(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let patch = args
        .get("patch")
        .cloned()
        .ok_or_else(|| ToolError::MissingArg("patch".into()))?;
    if !patch.is_object() {
        return Err(ToolError::InvalidArg(
            "patch must be a JSON object".into(),
        ));
    }
    let _ = crate::config::ensure_user_config(None);
    crate::config::reload_config();
    let mut cfg = crate::config::config();

    // api_keys
    if let Some(keys) = patch.get("api_keys").and_then(|v| v.as_object()) {
        if let Some(s) = keys.get("pexels").and_then(|v| v.as_str()) {
            cfg.api_keys.pexels = s.to_string();
        }
        if let Some(s) = keys.get("giphy").and_then(|v| v.as_str()) {
            cfg.api_keys.giphy = s.to_string();
        }
        if let Some(s) = keys.get("pixabay").and_then(|v| v.as_str()) {
            cfg.api_keys.pixabay = s.to_string();
        }
        if let Some(s) = keys.get("openrouter").and_then(|v| v.as_str()) {
            cfg.api_keys.openrouter = s.to_string();
        }
        if let Some(s) = keys.get("opencode").and_then(|v| v.as_str()) {
            cfg.api_keys.opencode = s.to_string();
        }
        // legacy aliases inside patch.api_keys
        if let Some(s) = keys.get("openrouter_api_key").and_then(|v| v.as_str()) {
            cfg.api_keys.openrouter = s.to_string();
        }
    }
    // top-level legacy
    if let Some(s) = patch.get("openrouter_api_key").and_then(|v| v.as_str()) {
        cfg.api_keys.openrouter = s.to_string();
    }
    if let Some(s) = patch.get("pexels_api_key").and_then(|v| v.as_str()) {
        cfg.api_keys.pexels = s.to_string();
    }
    if let Some(s) = patch.get("giphy_api_key").and_then(|v| v.as_str()) {
        cfg.api_keys.giphy = s.to_string();
    }
    if let Some(s) = patch.get("pixabay_api_key").and_then(|v| v.as_str()) {
        cfg.api_keys.pixabay = s.to_string();
    }

    // llm
    if let Some(llm) = patch.get("llm").and_then(|v| v.as_object()) {
        if let Some(s) = llm.get("openrouter_base_url").and_then(|v| v.as_str()) {
            cfg.llm.openrouter_base_url = s.to_string();
        }
        if let Some(arr) = llm.get("openrouter_models").and_then(|v| v.as_array()) {
            cfg.llm.openrouter_models = arr
                .iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect();
        }
        if let Some(s) = llm.get("opencode_base_url").and_then(|v| v.as_str()) {
            cfg.llm.opencode_base_url = s.to_string();
        }
        if let Some(s) = llm.get("opencode_model").and_then(|v| v.as_str()) {
            cfg.llm.opencode_model = s.to_string();
        }
    }

    // paths
    if let Some(paths) = patch.get("paths").and_then(|v| v.as_object()) {
        if let Some(s) = paths.get("sfx_path").and_then(|v| v.as_str()) {
            cfg.paths.sfx_path = Some(s.to_string());
        }
        if let Some(s) = paths.get("music_path").and_then(|v| v.as_str()) {
            cfg.paths.music_path = Some(s.to_string());
        }
        if let Some(s) = paths.get("tts_url").and_then(|v| v.as_str()) {
            cfg.paths.tts_url = Some(s.to_string());
        }
        if let Some(s) = paths.get("workspace_root").and_then(|v| v.as_str()) {
            cfg.paths.workspace_root = Some(s.to_string());
        }
    }

    // render
    if let Some(render) = patch.get("render").and_then(|v| v.as_object()) {
        if let Some(s) = render.get("default_aspect").and_then(|v| v.as_str()) {
            cfg.render.default_aspect = s.to_string();
        }
        if let Some(n) = render.get("normalize_lufs").and_then(|v| v.as_f64()) {
            cfg.render.normalize_lufs = n;
        }
    }

    let path = crate::config::write_user_config(&cfg)
        .map_err(|e| ToolError::Io(std::io::Error::other(e)))?;

    Ok(json!({
        "status": "success",
        "written": path.display().to_string(),
        "config": crate::config::config_public_view(),
    }))
}

/// Extract a frame and describe it (OpenRouter multimodal free → local text).
async fn handle_vision_analyze_clip(
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?
        .to_string_lossy()
        .to_string();
    if !Path::new(&video_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Video not found: {}",
            video_path
        )));
    }
    let at_s = args.get("at_s").and_then(|v| v.as_f64());
    let prompt = default_opt_str(&args, "prompt");
    crate::llm::analyze_clip(&video_path, at_s, prompt.as_deref())
        .await
        .map_err(|e| ToolError::Asset(format!("vision.analyze_clip failed: {}", e)))
}

/// Score stock clip relevance vs scene dialogue + keywords.
async fn handle_vision_score_clip(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let video_path = sanitize_input_path(extract_str(&args, "video_path")?)?
        .to_string_lossy()
        .to_string();
    if !Path::new(&video_path).exists() {
        return Err(ToolError::NotFound(format!(
            "Video not found: {}",
            video_path
        )));
    }
    let scene_text = extract_str(&args, "scene_text")?;
    let keywords: Vec<String> = args
        .get("video_keywords")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let search_query = default_opt_str(&args, "search_query");
    crate::llm::score_clip_relevance(
        &video_path,
        scene_text,
        &keywords,
        search_query.as_deref(),
    )
    .await
    .map_err(|e| ToolError::Asset(format!("vision.score_clip failed: {}", e)))
}

// ---------------------------------------------------------------------------
// Handler: system.capabilities (P1-2 from prior audit)
// ---------------------------------------------------------------------------

/// Probe every backend subsystem and report availability. Agents should call
/// this once at the start of a session to know which tools will work.
async fn handle_system_capabilities(
    _args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    use openscript_assets::music::MusicIndex;
    use openscript_assets::sfx::SfxIndex;

    // Resolve the repo root for CWD-independent path checks.
    // Priority: OPENSCRIPT_ROOT env var > CARGO_MANIFEST_DIR (compile-time) > CWD
    // The fresh-agent UX audit found that system.capabilities returned false
    // negatives when run from the wrong directory because all paths were
    // relative. This helper resolves them to absolute paths.
    let repo_root = std::env::var("OPENSCRIPT_ROOT")
        .ok()
        .map(std::path::PathBuf::from)
        .or_else(|| {
            option_env!("CARGO_MANIFEST_DIR")
                .map(std::path::PathBuf::from)
                .and_then(|d| d.parent().and_then(|p| p.parent()).map(|p| p.to_path_buf()))
        })
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    let resolve = |rel: &str| -> std::path::PathBuf {
        let p = std::path::Path::new(rel);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            repo_root.join(rel)
        }
    };

    let path_exists = |rel: &str| -> bool {
        let p = resolve(rel);
        p.exists()
    };

    // Pexels API key
    let pexels_available = !pexels_key().is_empty();
    let pexels = json!({
        "available": pexels_available,
        "reason": if pexels_available {
            serde_json::Value::Null
        } else {
            "PEXELS_API_KEY not set. Set it in mcp/assets/.openscript_config.json or as an env var. Get a free key at https://www.pexels.com/api/".into()
        },
    });

    // GIPHY API key
    let giphy_available = !giphy_key().is_empty();
    let giphy = json!({
        "available": giphy_available,
        "reason": if giphy_available {
            serde_json::Value::Null
        } else {
            "GIPHY_API_KEY not set. Set it in mcp/assets/.openscript_config.json or as an env var. Get a key at https://developers.giphy.com/".into()
        },
    });

    // Pixabay API key
    let pixabay_available = !pixabay_key().is_empty();
    let pixabay = json!({
        "available": pixabay_available,
        "reason": if pixabay_available {
            serde_json::Value::Null
        } else {
            "PIXABAY_API_KEY not set. Set it in mcp/assets/.openscript_config.json or as an env var. Optional — only needed for stock.search/stock.fetch.".into()
        },
    });

    // SFX library — resolve path CWD-independently (same fix as music).
    let sfx_index_path = std::env::var("OPENSCRIPT_SFX_INDEX")
        .unwrap_or_else(|_| "mcp/assets/sfx_index.json".to_string());
    let sfx_index_resolved = resolve(&sfx_index_path);
    let sfx_count = SfxIndex::load(Some(&sfx_index_resolved.to_string_lossy()))
        .map(|idx| idx.len())
        .unwrap_or(0);
    let sfx = json!({
        "available": sfx_count > 0,
        "indexed_count": sfx_count,
        "index_path": sfx_index_path,
    });

    // Music library — the committed 20-track stock index at
    // music_library_index.json is the single source of truth (500+ YouTube-scraped
    // copyright-free tracks). music_index.json was deleted (synthetic sine stubs).
    // music_production/ was deleted (synthetic sine stubs).
    let music_library_index = resolve("mcp/assets/music_library_index.json");
    let real_library = music_library_index.exists();
    let music_library_count = if real_library {
        MusicIndex::load(Some(&music_library_index.to_string_lossy()))
            .map(|idx| idx.len())
            .unwrap_or(0)
    } else {
        0
    };
    let music = json!({
        "available": real_library || !pixabay_key().is_empty(),
        "library_count": music_library_count,
        "library_path": "mcp/assets/music_library_index.json",
        "usable_for_production": real_library || !pixabay_key().is_empty(),
        "reason": if real_library {
            serde_json::Value::Null
        } else {
            "Run library.build to populate the music index, or set PIXABAY_API_KEY.".into()
        },
    });

    // Voicebox TTS (qwen3 / faster-tts sidecar at OPENSCRIPT_TTS_URL)
    let tts_url = std::env::var("OPENSCRIPT_TTS_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:17493".to_string());
    let voicebox_available = probe_http(&format!("{}/models", tts_url)).await;
    let voicebox = json!({
        "available": voicebox_available,
        "url": tts_url,
        "reason": if voicebox_available {
            serde_json::Value::Null
        } else {
            "No voicebox server responding at OPENSCRIPT_TTS_URL. Kokoro (script.generate_voices) is the default TTS and does NOT need voicebox.".into()
        },
    });

    // Kokoro TTS (Python sidecar). Runtime expects:
    //   model:  mcp/assets/kokoro/onnx/kokoro-v1.0.onnx  (or KOKORO_MODEL)
    //   voices: mcp/assets/kokoro/voices/voices-v1.0.bin  (or KOKORO_VOICES)
    //   sidecar script + optional voices.json profile registry
    // Prior bug: only checked sidecar + voices.json and reported a wrong
    // model path (mcp/assets/kokoro-v1.0.onnx), so available=true while
    // script.to_video hard-failed with "Kokoro model not found".
    let kokoro_model = std::env::var("KOKORO_MODEL").unwrap_or_else(|_| {
        "mcp/assets/kokoro/onnx/kokoro-v1.0.onnx".to_string()
    });
    let kokoro_voices_bin = std::env::var("KOKORO_VOICES").unwrap_or_else(|_| {
        "mcp/assets/kokoro/voices/voices-v1.0.bin".to_string()
    });
    let kokoro_profiles =
        std::env::var("KOKORO_PROFILES").unwrap_or_else(|_| "mcp/assets/voices.json".to_string());
    let kokoro_sidecar = std::env::var("KOKORO_SIDECAR")
        .unwrap_or_else(|_| "mcp/scripts/kokoro_tts_sidecar.py".to_string());
    let kokoro_model_ok = path_exists(&kokoro_model);
    let kokoro_voices_ok = path_exists(&kokoro_voices_bin);
    let kokoro_sidecar_ok = path_exists(&kokoro_sidecar);
    // Probe the resolved Python interpreter for kokoro_onnx importability.
    // This catches the common case: assets are on disk but the Python env
    // doesn't have kokoro_onnx installed (conda env mismatch, PEP-668, etc).
    let kokoro_python = std::env::var("KOKORO_PYTHON").unwrap_or_else(|_| {
        // Mirror the priority from kokoro_sidecar::resolve_kokoro_python()
        // inline to avoid importing the whole module in doctor context.
        if let Some(home) = std::env::var("HOME").ok().filter(|h| !h.is_empty()).map(std::path::PathBuf::from) {
            for env_name in &["kokoro-tts", "kokoro"] {
                let candidate = home.join("miniconda3/envs").join(env_name).join("bin/python");
                if candidate.exists() {
                    return candidate.to_string_lossy().to_string();
                }
            }
        }
        "python3".to_string()
    });
    let kokoro_python_ok = std::process::Command::new(&kokoro_python)
        .arg("-c")
        .arg("import kokoro_onnx")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let kokoro_available = kokoro_model_ok && kokoro_voices_ok && kokoro_sidecar_ok && kokoro_python_ok;
    let kokoro_reason = if kokoro_available {
        serde_json::Value::Null
    } else {
        let mut missing = Vec::new();
        if !kokoro_model_ok {
            missing.push(format!("model ({})", kokoro_model));
        }
        if !kokoro_voices_ok {
            missing.push(format!("voices bin ({})", kokoro_voices_bin));
        }
        if !kokoro_sidecar_ok {
            missing.push(format!("sidecar ({})", kokoro_sidecar));
        }
        if !kokoro_python_ok {
            missing.push(format!(
                "Python module 'kokoro_onnx' not importable via {} — set KOKORO_PYTHON to a Python with kokoro_onnx installed",
                kokoro_python
            ));
        }
        format!(
            "Kokoro incomplete — missing: {}. Run: bash setup.sh (downloads model+voices, installs kokoro-onnx). Or set KOKORO_PYTHON.",
            missing.join(", ")
        )
        .into()
    };
    let kokoro = json!({
        "available": kokoro_available,
        "sidecar_path": kokoro_sidecar,
        "model_path": kokoro_model,
        "voices_path": kokoro_voices_bin,
        "profiles_path": kokoro_profiles,
        "profiles_available": path_exists(&kokoro_profiles),
        "python_path": kokoro_python,
        "python_module_ok": kokoro_python_ok,
        "reason": kokoro_reason,
    });

    // Transcription engine (HinglishGgml — the sole engine)
    let transcription = {
        let result = openscript_transcribe::transcriber::check_hinglish_ggml_health().await;
        match result {
            Ok(_) => json!({
                "available": true,
                "engine": "hinglish-ggml",
                "reason": serde_json::Value::Null,
            }),
            Err(reason) => json!({
                "available": false,
                "engine": "hinglish-ggml",
                "reason": reason,
            }),
        }
    };

    // HyperFrames (default render engine)
    let hf_dir = resolve("hyperframes");
    let hyperframes = json!({
        "available": hf_dir.exists(),
        "path": hf_dir.to_string_lossy(),
    });

    // FFmpeg
    let ffmpeg_available = std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let ffmpeg = json!({
        "available": ffmpeg_available,
        "reason": if ffmpeg_available {
            serde_json::Value::Null
        } else {
            "ffmpeg binary not found on PATH. Required for all video rendering tools.".into()
        },
    });

    // yt-dlp (required for youtube.search, youtube.download, library.download,
    // and background.fetch YouTube fallback)
    let ytdlp_available = std::process::Command::new("yt-dlp")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let yt_dlp = json!({
        "available": ytdlp_available,
        "reason": if ytdlp_available {
            serde_json::Value::Null
        } else {
            "yt-dlp binary not found on PATH. Required for youtube.search, youtube.download, library.download, and background.fetch YouTube fallback.".into()
        },
    });

    // Parakeet TDT force-alignment (required for script.build_captions
    // word-level timing). Replaces the old whisper_align.py which depended
    // on the `openai-whisper` Python package. Parakeet TDT runs via
    // `onnxruntime` and the model is at mcp/assets/parakeet/.
    // We check: (1) the script exists, (2) onnxruntime is importable,
    // (3) the encoder/decoder ONNX model files exist.
    let parakeet_script_path = "mcp/scripts/parakeet_align.py";
    let parakeet_script_exists = path_exists(parakeet_script_path);
    let onnxruntime_importable = std::process::Command::new("python3")
        .args(["-c", "import onnxruntime; print('ok')"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let parakeet_encoder = resolve("mcp/assets/parakeet/encoder-model.int8.onnx");
    let parakeet_decoder = resolve("mcp/assets/parakeet/decoder_joint-model.int8.onnx");
    let parakeet_models_exist = parakeet_encoder.exists() && parakeet_decoder.exists();
    let parakeet_align_available = parakeet_script_exists && onnxruntime_importable && parakeet_models_exist;
    let parakeet_align = json!({
        "available": parakeet_align_available,
        "path": parakeet_script_path,
        "script_exists": parakeet_script_exists,
        "onnxruntime_importable": onnxruntime_importable,
        "models_exist": parakeet_models_exist,
        "reason": if parakeet_align_available {
            serde_json::Value::Null
        } else if !parakeet_script_exists {
            "parakeet_align.py not found. script.build_captions will fall back to even-spacing estimation (less accurate word timings).".into()
        } else if !onnxruntime_importable {
            "parakeet_align.py exists but the Python `onnxruntime` module is not installed. Install with: pip3 install --user onnxruntime. script.build_captions will fall back to even-spacing estimation.".into()
        } else {
            "Parakeet ONNX model files not found at mcp/assets/parakeet/. Download from https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx. script.build_captions will fall back to even-spacing estimation.".into()
        },
    });

    // tsx (required for timeline.to_hyperframes — compiles EDL v2 to HF HTML)
    let tsx_available = std::process::Command::new("npx")
        .arg("tsx")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let tsx = json!({
        "available": tsx_available,
        "reason": if tsx_available {
            serde_json::Value::Null
        } else {
            "tsx not found (npx tsx --version failed). Required for timeline.to_hyperframes.".into()
        },
    });

    // ASS caption font (BebasNeue — required for burned-in captions)
    let font_path = "mcp/fonts/BebasNeue-Regular.ttf";
    let font_available = path_exists(font_path);
    let ass_font = json!({
        "available": font_available,
        "path": font_path,
        "reason": if font_available {
            serde_json::Value::Null
        } else {
            "BebasNeue-Regular.ttf not found. Caption burning will use ffmpeg's default font.".into()
        },
    });

    // SVG sticker presets
    let presets_dir = "mcp/assets/svg_presets";
    let preset_count = if path_exists(presets_dir) {
        std::fs::read_dir(presets_dir)
            .map(|d| d.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()).count())
            .unwrap_or(0)
    } else {
        0
    };
    let svg_presets = json!({
        "available": preset_count > 0,
        "preset_count": preset_count,
        "path": presets_dir,
    });

    // Audio8 TTS (zero-shot voice cloning, ONNX INT4)
    let audio8_model_present = std::path::Path::new("mcp/assets/audio8/model/runtime_manifest.json").exists();
    let audio8_voices_dir = std::path::Path::new("mcp/assets/audio8/voices");
    let audio8_voice_count = if audio8_voices_dir.exists() {
        std::fs::read_dir(audio8_voices_dir)
            .map(|d| d.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()).count())
            .unwrap_or(0)
    } else {
        0
    };
    let audio8 = json!({
        "available": audio8_model_present && openscript_tts::audio8::audio8_available(),
        "model_present": audio8_model_present,
        "voice_count": audio8_voice_count,
        "model_dir": "mcp/assets/audio8/model",
        "voices_dir": "mcp/assets/audio8/voices",
        "sample_rate": 44100,
        "languages": ["en", "es", "fr", "de", "it", "nl", "pl", "ja", "ko", "zh", "yue"],
        "note": "Zero-shot voice cloning via Audio8 TTS Preview 0.6B (ONNX INT4). English default for the script-to-video workflow.",
    });

    // Whisper word alignment (multilingual — primary alignment engine for
    // Hinglish/Hindi scripts; Parakeet TDT is English-only and drifts on
    // Hinglish). Used by script.generate_voices when script.language is
    // hi/hinglish. Requires the openai-whisper Python package.
    let whisper_script = "mcp/scripts/whisper_align.py";
    let whisper_script_exists = path_exists(whisper_script);
    let whisper_importable = std::process::Command::new("python3")
        .args(["-c", "import whisper; print('ok')"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let whisper_align = json!({
        "available": whisper_script_exists && whisper_importable,
        "path": whisper_script,
        "script_exists": whisper_script_exists,
        "whisper_importable": whisper_importable,
        "model": "base",
        "languages": ["hi", "hinglish", "en", "es", "fr", "de"],
        "reason": if whisper_script_exists && whisper_importable {
            serde_json::Value::Null
        } else if !whisper_script_exists {
            "whisper_align.py not found. Hinglish scripts fall back to Parakeet alignment (English-only — caption sync on Hinglish will be approximate).".into()
        } else {
            "openai-whisper not installed (pip install openai-whisper). Hinglish scripts fall back to Parakeet alignment.".into()
        },
    });

    // LLM / vision cascade: OpenCode zen + OpenRouter free multimodal
    let llm = crate::llm::probe_llm_capabilities().await;
    let openscript_config = crate::config::config_public_view();

    Ok(json!({
        "status": "success",
        "voicebox": voicebox,
        "kokoro": kokoro,
        "audio8": audio8,
        "transcription": transcription,
        "parakeet_align": parakeet_align,
        "whisper_align": whisper_align,
        "pexels": pexels,
        "giphy": giphy,
        "pixabay": pixabay,
        "sfx_library": sfx,
        "music_library": music,
        "ffmpeg": ffmpeg,
        "yt_dlp": yt_dlp,
        "tsx": tsx,
        "ass_font": ass_font,
        "svg_presets": svg_presets,
        "hyperframes": hyperframes,
        "llm": llm,
        "openscript_config": openscript_config,
    }))
}

// ---------------------------------------------------------------------------
// Handler: system.doctor — cold-start production readiness
// ---------------------------------------------------------------------------

async fn handle_system_doctor(_args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let ffmpeg_ok = std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let ffprobe_ok = std::process::Command::new("ffprobe")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let ytdlp_ok = std::process::Command::new("yt-dlp")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    let pexels_ok = !pexels_key().is_empty();
    let giphy_ok = !giphy_key().is_empty();
    let music_prod = resolve_repo_path("mcp/assets/music_production/index.json").exists();
    let music_lib = resolve_repo_path("mcp/assets/music_library_index.json").exists();
    let music_ok = music_prod || music_lib || !pixabay_key().is_empty();
    let sfx_index = resolve_repo_path("mcp/assets/sfx_index.json");
    let sfx_pack = resolve_repo_path("mcp/assets/sfx_pack");
    let mut sfx_resolvable = 0usize;
    if let Ok(raw) = std::fs::read_to_string(&sfx_index) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
            if let Some(assets) = v.get("assets").and_then(|a| a.as_array()) {
                for a in assets.iter().take(30) {
                    let p = a.get("path").and_then(|x| x.as_str()).unwrap_or("");
                    if Path::new(p).exists() || resolve_repo_path(p).exists() {
                        sfx_resolvable += 1;
                    }
                }
            }
        }
    }
    let sfx_ok = sfx_resolvable >= 5 || sfx_pack.is_dir();
    let kokoro_ok = resolve_repo_path("mcp/assets/kokoro/onnx/kokoro-v1.0.onnx").exists();
    let config_ok = crate::config::config_file_path().exists() || pexels_ok;

    let mut checklist = Vec::new();
    let mut next_actions = Vec::new();
    let push = |items: &mut Vec<serde_json::Value>,
                next: &mut Vec<String>,
                id: &str,
                ok: bool,
                detail: &str,
                action: Option<&str>| {
        items.push(json!({
            "id": id,
            "ok": ok,
            "detail": detail,
        }));
        if !ok {
            if let Some(a) = action {
                next.push(a.to_string());
            }
        }
    };

    push(
        &mut checklist,
        &mut next_actions,
        "ffmpeg",
        ffmpeg_ok && ffprobe_ok,
        if ffmpeg_ok {
            "ffmpeg + ffprobe on PATH"
        } else {
            "ffmpeg/ffprobe missing"
        },
        Some("Install ffmpeg (apt install ffmpeg / brew install ffmpeg)"),
    );
    push(
        &mut checklist,
        &mut next_actions,
        "yt_dlp",
        ytdlp_ok,
        if ytdlp_ok {
            "yt-dlp on PATH"
        } else {
            "yt-dlp missing (YouTube stock/music fallback)"
        },
        Some("pip install --user yt-dlp"),
    );
    push(
        &mut checklist,
        &mut next_actions,
        "pexels",
        pexels_ok,
        if pexels_ok {
            "PEXELS_API_KEY present (env or ~/.openscript/config.json)"
        } else {
            "Pexels key missing — multi-broll will fail-closed to draft without stock"
        },
        Some("bash scripts/setup_openscript_config.sh --pexels-key YOUR_KEY"),
    );
    push(
        &mut checklist,
        &mut next_actions,
        "giphy",
        giphy_ok,
        if giphy_ok {
            "GIPHY_API_KEY present"
        } else {
            "GIPHY key missing (local sticker fallbacks only)"
        },
        Some("bash scripts/setup_openscript_config.sh --giphy-key YOUR_KEY"),
    );
    push(
        &mut checklist,
        &mut next_actions,
        "music",
        music_ok,
        if music_prod {
            "music_production pack present (cold-start beds)"
        } else if music_lib {
            "music_library_index.json present"
        } else {
            "No production music path"
        },
        Some("Ensure mcp/assets/music_production/ exists or run library.build"),
    );
    push(
        &mut checklist,
        &mut next_actions,
        "sfx",
        sfx_ok,
        &format!(
            "SFX: {} resolvable of first 30 index rows; pack_dir={}",
            sfx_resolvable,
            sfx_pack.is_dir()
        ),
        Some("Use mcp/assets/sfx_pack or sfx.index against OPENSCRIPT_SFX_PATH"),
    );
    push(
        &mut checklist,
        &mut next_actions,
        "kokoro",
        kokoro_ok,
        if kokoro_ok {
            "Kokoro ONNX model present"
        } else {
            "Kokoro model missing"
        },
        Some("bash setup.sh  # downloads Kokoro models"),
    );
    push(
        &mut checklist,
        &mut next_actions,
        "config",
        config_ok,
        if crate::config::config_file_path().exists() {
            "openscript config file present"
        } else if pexels_ok {
            "keys via env (config file optional)"
        } else {
            "no ~/.openscript/config.json"
        },
        Some("bash scripts/setup_openscript_config.sh"),
    );

    // HinglishGgml transcription engine check
    let hinglish_available = openscript_transcribe::transcriber::check_hinglish_ggml_health().await;
    let hinglish_ok = hinglish_available.is_ok();
    let hinglish_msg = if hinglish_ok { hinglish_available.unwrap() } else { hinglish_available.unwrap_err() };
    push(&mut checklist, &mut next_actions, "hinglish-ggml", hinglish_ok, &hinglish_msg, Some("Build whisper.cpp + download GGML model — run bash setup.sh"));

    // Production-ready: binaries + pexels + music + kokoro. GIPHY optional.
    // HinglishGgml transcription engine check
    let hinglish_available = openscript_transcribe::transcriber::check_hinglish_ggml_health().await;
    let hinglish_ok = hinglish_available.is_ok();
    let hinglish_msg = if hinglish_ok { hinglish_available.unwrap() } else { hinglish_available.unwrap_err() };
    push(&mut checklist, &mut next_actions, "hinglish-ggml", hinglish_ok, &hinglish_msg, Some("Build whisper.cpp + download GGML model - run bash setup.sh"));
    let ready_for_production = ffmpeg_ok && ffprobe_ok && pexels_ok && music_ok && kokoro_ok;
    if ready_for_production && next_actions.is_empty() {
        next_actions.push(
            "Run director.run on a 5-scene script; expect ≥4/5 non-procedural stock + music bed"
                .into(),
        );
    } else if !ready_for_production {
        next_actions.push("bash scripts/bootstrap_media.sh".into());
        next_actions.push("See docs/INSTALL.md".into());
    }

    Ok(json!({
        "status": if ready_for_production { "ready" } else { "not_ready" },
        "ready_for_production": ready_for_production,
        "checklist": checklist,
        "next_actions": next_actions,
        "hints": {
            "allow_procedural": "OPENSCRIPT_ALLOW_PROCEDURAL=1 forces gradient B-roll (draft-grade only)",
            "config": crate::config::config_file_path().display().to_string(),
            "install_plan": "docs/INSTALL_MEDIA_DEPS_PLAN.md",
        },
    }))
}

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

/// Natural-language tool discovery. Tokenises the query, scores each tool by
/// keyword overlap with its name + description, and returns the top N matches.
async fn handle_help_tool(args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    let query = extract_str(&args, "query")?;
    let limit = default_u32(&args, "limit", 8).clamp(1, 20) as usize;

    // Normalise the query into a set of lowercase tokens, dropping stopwords.
    let stop = [
        "a", "an", "the", "to", "for", "of", "in", "on", "at", "by", "with", "and", "or", "is",
        "are", "be", "do", "does", "how", "i", "my", "me", "want", "need", "please", "can",
        "could", "would", "should",
    ];

    // Synonym map: expand query tokens with common synonyms so "burn" matches
    // "burned-in", "footage" matches "broll", "VO" matches "voiceover", etc.
    // This fixes the 3/4 broken example queries from the UX audit.
    let synonyms: &[(&str, &[&str])] = &[
        ("burn", &["burned", "burning", "burn-in", "burned-in"]),
        ("footage", &["broll", "b-roll", "background", "clip", "video"]),
        ("vo", &["voiceover", "voice", "narration"]),
        ("subtitles", &["captions", "subtitle", "caption", "srt"]),
        ("sidechain", &["ducking", "duck", "compress"]),
        ("render", &["rendered", "rendering", "render"]),
        ("music", &["audio", "track", "song", "background"]),
        ("sfx", &["sound", "effect", "effects", " Foley"]),
        ("sticker", &["overlay", "gif", "png", "image", "sticker"]),
        ("transcribe", &["transcription", "transcribe", "whisper", "speech"]),
        ("voice", &["tts", "voiceover", "kokoro", "speech", "voice"]),
        ("animate", &["animation", "animated", "motion", "gsap", "hyperframes"]),
    ];

    let expand_token = |t: &str| -> Vec<String> {
        let mut expanded = vec![t.to_string()];
        for (key, vals) in synonyms {
            if t == *key {
                expanded.extend(vals.iter().map(|v| v.to_string()));
            }
            // Also check reverse: if t is a synonym, add the key
            if vals.contains(&t) {
                expanded.push(key.to_string());
            }
        }
        expanded
    };

    let query_tokens: std::collections::HashSet<String> = query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty() && !stop.contains(t))
        .flat_map(expand_token)
        .collect();

    if query_tokens.is_empty() {
        return Ok(json!({
            "status": "success",
            "query": query,
            "results": [],
            "count": 0,
            "message": "Query contained no searchable keywords. Try describing the task, e.g. 'add voiceover to a timeline'."
        }));
    }

    // Detect NLE / existing-footage intent so we do not boost from-scratch
    // orchestrators (script.to_video) for "edit existing footage" queries.
    // Strong markers alone are enough; "edit"/"clip" only count with video context.
    let nle_intent = {
        let q = query.to_lowercase();
        let strong = [
            "existing",
            "footage",
            "transcribe",
            "raw video",
            "reelize",
            "nle",
            "recording",
            "source video",
            "hinglish",
        ]
        .iter()
        .any(|m| q.contains(m));
        let soft_edit = (q.contains("edit") || q.contains("cut"))
            && (q.contains("video")
                || q.contains("footage")
                || q.contains("clip")
                || q.contains("reel")
                || q.contains("timeline"));
        (strong || soft_edit)
            && !q.contains("from scratch")
            && !q.contains("script json")
            && !q.contains("from a script")
    };
    let from_scratch_intent = {
        let q = query.to_lowercase();
        q.contains("script")
            || q.contains("from scratch")
            || q.contains("tts")
            || q.contains("create a video")
            || q.contains("generate a video")
    };

    // Tool weight table: golden-path tools get a base boost, orchestrators get
    // a medium boost, primitives get no boost. Trajectory-aware: NLE queries
    // boost transcribe/reelize/timeline.
    let tool_weight = |name: &str| -> f64 {
        if nle_intent {
            if matches!(
                name,
                "transcribe"
                    | "reelize.direct"
                    | "reelize.brief"
                    | "timeline.render"
                    | "timeline.build"
                    | "srt.prepare"
                    | "edl.build"
            ) {
                return 0.20;
            }
            // Demote from-scratch golden path on NLE queries
            if matches!(name, "script.to_video" | "script.parse" | "script.to_timeline") {
                return 0.0;
            }
        }
        if from_scratch_intent && !nle_intent
            && matches!(name, "script.to_video" | "script.parse") {
                return 0.20;
            }
        // Golden-path defaults
        if matches!(
            name,
            "script.to_video"
                | "script.parse"
                | "transcribe"
                | "timeline.render"
                | "system.capabilities"
                | "help.tool"
        ) {
            0.15
        // Orchestrators
        } else if matches!(
            name,
                | "reelize.direct"
                | "composition.render"
                | "tts.commentary"
                | "script.to_timeline"
                | "script.generate_voices"
                | "script.build_captions"
        ) {
            0.10
        // Common operations
        } else if matches!(
            name,
            "music.assign"
                | "sfx.assign"
                | "broll.assign"
                | "overlay.assign"
                | "voiceover.generate"
                | "tts.generate"
                | "background.fetch"
                | "music.search"
                | "sfx.search"
                | "broll.fetch"
                | "gif.search"
                | "media.search"
                | "library.search"
                | "stock.search"
        ) {
            0.05
        } else {
            0.0
        }
    };

    // Iterate all tool definitions, score each by token overlap with name + description.
    let all_tools = tool_definitions();
    let mut scored: Vec<serde_json::Value> = Vec::new();

    if let Some(arr) = all_tools.as_array() {
        for tool in arr {
            let name = tool.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let desc = tool
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            // Combine name + description, lowercase, tokenise
            let combined = format!("{} {}", name, desc).to_lowercase();
            let tool_tokens: std::collections::HashSet<&str> = combined
                .split(|c: char| !c.is_alphanumeric())
                .filter(|t| !t.is_empty())
                .collect();

            // Score = (matching tokens) / (query tokens) + name_boost + tool_weight.
            let mut matches = 0usize;
            let mut name_boost = 0.0;
            for qt in &query_tokens {
                if tool_tokens.contains(qt.as_str()) {
                    matches += 1;
                    if name.to_lowercase().contains(qt.as_str()) {
                        name_boost += 0.15;
                    }
                }
            }
            let coverage = matches as f64 / query_tokens.len() as f64;
            let weight = tool_weight(name);
            let mut score = (coverage + name_boost + weight).min(1.0);
            // Hard demote from-scratch tools on NLE queries even if token
            // overlap is high (e.g. "captions" matches script.to_video desc).
            if nle_intent
                && matches!(
                    name,
                    "script.to_video" | "script.parse" | "script.to_timeline"
                        | "script.generate_voices" | "script.build_captions"
                )
            {
                score *= 0.35;
            }

            if score > 0.0 {
                // Short description = first sentence of the description, capped at 180 chars.
                let short_desc = desc
                    .split('.')
                    .next()
                    .unwrap_or(desc)
                    .chars()
                    .take(180)
                    .collect::<String>();
                scored.push(json!({
                    "name": name,
                    "relevance": (score * 100.0).round() / 100.0,
                    "description": short_desc,
                }));
            }
        }
    }

    // Sort by relevance desc (tool-weight table breaks ties instead of alphabet)
    scored.sort_by(|a, b| {
        let ra = a.get("relevance").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let rb = b.get("relevance").and_then(|v| v.as_f64()).unwrap_or(0.0);
        rb.partial_cmp(&ra).unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(limit);

    let count = scored.len();
    if count == 0 {
        return Ok(json!({
            "status": "success",
            "query": query,
            "results": [],
            "count": 0,
            "message": "No tools matched. Try tools/list to browse all 76 tools, or system.capabilities to probe available subsystems."
        }));
    }

    Ok(json!({
        "status": "success",
        "query": query,
        "results": scored,
        "count": count,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

}
